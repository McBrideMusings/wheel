//! Running an artifact's refresh command for the pane showing it.
//!
//! The artifact's entry in a `wheel.toml` declares the command; the daemon runs it through `/bin/sh` in the pane's *space*
//! directory and hands it the pane's data directory in `WHEEL_DATA_DIR`, which is where an artifact
//! writes what its page reads: the artifact's own folder is read-only static content a pane serves,
//! and a project's working tree is not for generated files. That is a place to write, not a
//! sandbox — the command is a shell the person approved, and it can reach whatever they can. A run
//! happens when the pane opens, when a caller asks for one, and — when the entry declares
//! `every` — on a timer the pane owns, which runs the same [`Refresh`] and has no way of its own to
//! run a command.
//!
//! No command runs until a person approves it at a permission request — the same request an agent's
//! tool call raises, which every connected client and the `perm` CLI verbs answer. Answering "always"
//! writes the pair into a trust file in the instance directory, so a restart does not ask again;
//! the approval is keyed by the artifact's *folder*, the space's directory and the command, so
//! editing the command asks afresh, and it carries a hash of each project file the command names,
//! so editing one of those asks afresh too. A timer tick never raises that request: it runs only a command remembered as approved, and skips the
//! tick rather than putting a prompt in front of someone who did nothing to ask for one.
//!
//! A write the command makes reaches the page through the data directory's watch (`web.rs`), the
//! same `pane_reload` a person editing a file there sends: the page's own load and the on-open run
//! race, so a run that wrote nothing back to the page would leave a pane showing an empty data
//! directory. Nothing in this build lets a page ask for a refresh, so no page can reload itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wheel_proto::{ArtifactRef, FileRead, PermOption, RefreshInfo, RunInfo, TimerState, TrustInfo};

use crate::artifacts::Entry;
use crate::perms::Perms;
use crate::server::AppState;

/// How much of a run's standard error a reader gets, in bytes off the end: enough to say why a
/// command failed without putting a whole build log in every `status`.
const STDERR_TAIL: usize = 2000;

/// How long one run may take before the daemon stops it. A refresh fills a pane with current data;
/// a command still going a minute later is not doing that, and leaving it to run would hold every
/// later refresh of that pane behind it.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// How long a run that has ended keeps reading output a process that left the command's group still
/// sends: enough for what was already written to arrive, short enough that nobody waits on it.
const PIPE_GRACE: Duration = Duration::from_secs(1);

/// Characters that end an unquoted shell word.
const OPERATORS: &str = ";&|<>(){}`";

/// What stands in a word for an expansion only the shell can make (another variable, `$?`): no
/// path holds it, so a piece of a word containing it names nothing, and the word's other pieces
/// (`./bin` in `PATH=./bin:$PATH`) still do.
const UNKNOWN: char = '\0';

/// The words of `command` as the shell reads them: quotes and backslashes taken out, and each
/// `$NAME` or `${NAME}` that `vars` holds expanded. Each word is also split the plain way, on
/// spaces, quotes and operators, so text inside a substitution or a quoted `sh -c` script counts
/// as words too.
fn words(command: &str, vars: &[(&str, &str)]) -> Vec<String> {
    let mut words: Vec<String> = command
        .split(|c: char| c.is_whitespace() || OPERATORS.contains(c) || c == '\'' || c == '"')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    let (mut word, mut quote) = (String::new(), None::<char>);
    let mut end = |word: &mut String| {
        if !word.is_empty() {
            words.push(std::mem::take(word));
        }
    };
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next().filter(|&n| n != '\n') {
                    word.push(next);
                }
            }
            (None, '\'' | '"') => quote = Some(c),
            (_, '$') => {
                let braced = chars.next_if_eq(&'{').is_some();
                let mut name = String::new();
                while let Some(n) = chars.next_if(|n| n.is_ascii_alphanumeric() || *n == '_') {
                    name.push(n);
                }
                let closed = !braced || chars.next_if_eq(&'}').is_some();
                match vars.iter().find(|(var, _)| *var == name) {
                    Some((_, value)) if closed && !value.is_empty() => word.push_str(value),
                    // `$(` is a substitution, which the plain split above has the words of.
                    _ if name.is_empty() && !braced && chars.peek() == Some(&'(') => {}
                    _ => word.push(UNKNOWN),
                }
            }
            (Some(_), c) => word.push(c),
            (None, c) if c.is_whitespace() || OPERATORS.contains(c) => end(&mut word),
            (None, c) => word.push(c),
        }
    }
    end(&mut word);
    words.sort();
    words.dedup();
    words
}

/// Every way `word` could name a path: itself, each piece of it between `=` and `:` (the value of
/// `--out=x`, `X=x` and each entry of `PATH=a:b`), and for a piece starting with `-`, what follows
/// its first `/` and what follows a one-letter flag (`-o/x`, `-ox`). Naming too many costs
/// nothing, since a piece names something only when a file is there.
fn pieces(word: &str, home: &str) -> Vec<String> {
    let mut out = vec![word.to_string()];
    for piece in word.split(['=', ':']) {
        out.push(piece.to_string());
        if let Some(flag) = piece.strip_prefix('-') {
            out.extend(piece.find('/').map(|at| piece[at..].to_string()));
            out.extend(flag.get(1..).map(str::to_string));
            out.push(flag.trim_start_matches('-').to_string());
        }
    }
    out.into_iter()
        .filter(|p| !p.is_empty() && !p.contains(UNKNOWN))
        .map(|p| match p.strip_prefix('~') {
            Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
                format!("{home}{rest}")
            }
            _ => p,
        })
        .collect()
}

/// `path` with `.` and `..` taken out by their names alone, for asking whether a path is written
/// inside a root; what it names on disk is [`std::fs::canonicalize`]'s answer, which follows links
/// the way the shell's own lookup does.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Every file inside the artifact's folder `dir` or the space's directory `cwd` that `command`'s
/// text names, as the real path it opens, sorted. Nothing here knows which word is a program or
/// what a program does with its arguments: every piece of every word ([`words`], [`pieces`]) is a
/// path the command may run or read. A relative piece is taken in `cwd`, where the command runs,
/// and in every directory inside the two that the command names, so `cd scripts && ./pull.sh`
/// names `scripts/pull.sh`. A directory named inside them, other than the two themselves, names
/// the files directly in it: what `make -C x`, `PATH=./bin:…` or `python -m pkg` reach. A path
/// under `panes` — the parent of every pane's data directory, which each run writes — is left out.
fn named_files(command: &str, dir: &str, cwd: &str, panes: &Path) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let vars = [("WHEEL_SPACE_DIR", cwd), ("HOME", home.as_str())];
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    let roots: Vec<(PathBuf, PathBuf)> = [dir, cwd]
        .iter()
        .map(|r| {
            (
                normal(Path::new(r)),
                canonical(Path::new(r)).unwrap_or_else(|| r.into()),
            )
        })
        .collect();
    let panes = (
        normal(panes),
        canonical(panes).unwrap_or_else(|| panes.into()),
    );
    let under = |written: &Path, real: &Path, (named, actual): &(PathBuf, PathBuf)| {
        written.starts_with(named) || real.starts_with(actual)
    };
    // What a written path opens, when that is inside a root and not under `panes`.
    let inside = |written: PathBuf| {
        let real = canonical(&written)?;
        let written = normal(&written);
        (roots.iter().any(|r| under(&written, &real, r)) && !under(&written, &real, &panes))
            .then_some(real)
    };
    let pieces: Vec<String> = words(command, &vars)
        .iter()
        .flat_map(|w| pieces(w, &home))
        .collect();
    let mut bases = vec![PathBuf::from(cwd)];
    bases.extend(
        pieces
            .iter()
            .filter_map(|p| inside(Path::new(cwd).join(p)))
            .filter(|real| real.is_dir()),
    );
    let mut files = Vec::new();
    for piece in &pieces {
        for base in &bases {
            let Some(real) = inside(base.join(piece)) else {
                continue;
            };
            if real.is_file() {
                files.push(real);
            } else if real.is_dir() && !roots.iter().any(|(_, root)| *root == real) {
                let entries = std::fs::read_dir(&real).into_iter().flatten().flatten();
                files.extend(
                    entries
                        .filter_map(|e| canonical(&e.path()))
                        .filter(|p| p.is_file()),
                );
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// What a file looked like to `stat` when its contents were last hashed.
#[derive(Clone, PartialEq)]
struct Stamp {
    len: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
    ino: u64,
    dev: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Stamp {
        use std::os::unix::fs::MetadataExt;
        Stamp {
            len: meta.len(),
            mtime_sec: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ino: meta.ino(),
            dev: meta.dev(),
        }
    }
}

/// What a [`HashCache`] knows, shared with its watcher's callback.
#[derive(Default)]
struct CacheState {
    /// Each hashed file with the [`Stamp`] it was hashed at.
    entries: HashMap<PathBuf, (Stamp, String)>,
    /// Counts the filesystem events seen, so a read can tell whether one arrived while it hashed.
    seq: u64,
    /// The `seq` of the last watch error: a read that began before it cannot be cached.
    reset: u64,
    /// The `seq` of the last event for each path that has had one.
    touched: HashMap<PathBuf, u64>,
    /// The directories being watched: a file is cached only while its directory is in here.
    watched: HashSet<PathBuf>,
}

/// The hashes a runner has already taken, each with the [`Stamp`] it was taken at: a file whose
/// stamp is unchanged costs one `stat` rather than a read of its whole contents.
///
/// A stamp alone cannot tell a same-length edit that restores the modification time (`touch -r`,
/// `cp -p`) from no edit, so the directory of each cached file is watched, and any event for the
/// file drops its hash. A file whose directory cannot be watched is hashed on every read.
pub(crate) struct HashCache {
    state: Arc<Mutex<CacheState>>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl Default for HashCache {
    fn default() -> HashCache {
        let state = Arc::new(Mutex::new(CacheState::default()));
        let seen = state.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            match event {
                Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                    let mut state = seen.lock().unwrap();
                    state.seq += 1;
                    let seq = state.seq;
                    for path in event.paths {
                        // An event for a watched directory itself: it may have been removed and
                        // made again, which ends the OS watch, so it is watched afresh next read.
                        if state.watched.remove(&path) {
                            state.entries.retain(|file, _| file.parent() != Some(path.as_path()));
                        }
                        state.entries.remove(&path);
                        if let Some(then) = state.touched.get_mut(&path) {
                            *then = seq;
                        } else {
                            state.touched.insert(path, seq);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    // An event may have been lost: nothing cached can be trusted to be current.
                    tracing::warn!(error = %e, "the refresh file watch failed; hashes are taken afresh");
                    let mut state = seen.lock().unwrap();
                    state.seq += 1;
                    state.reset = state.seq;
                    state.entries.clear();
                }
            }
        })
        .inspect_err(|e| tracing::warn!(error = %e, "could not watch the files a refresh command names; each is hashed on every read"))
        .ok();
        HashCache {
            state,
            watcher: Mutex::new(watcher),
        }
    }
}

impl HashCache {
    /// Watch the directory of every one of `files` and stop watching the ones no file is in now.
    fn watch(&self, files: &[PathBuf]) {
        let mut watcher = self.watcher.lock().unwrap();
        let Some(watcher) = watcher.as_mut() else {
            return;
        };
        use notify::Watcher;
        let dirs: HashSet<PathBuf> = files
            .iter()
            .filter_map(|f| f.parent().map(Path::to_path_buf))
            .collect();
        // notify's calls run with the state lock released: its callback takes that lock, and a
        // backend that waits on its event thread would otherwise deadlock against it.
        let (gone, fresh) = {
            let mut state = self.state.lock().unwrap();
            let gone: Vec<PathBuf> = state.watched.difference(&dirs).cloned().collect();
            for dir in &gone {
                state.watched.remove(dir);
            }
            let fresh: Vec<PathBuf> = dirs
                .into_iter()
                .filter(|d| !state.watched.contains(d))
                .collect();
            (gone, fresh)
        };
        for dir in gone {
            let _ = watcher.unwatch(&dir);
        }
        for dir in fresh {
            // Marked before the OS watch exists, so an event for the directory itself that arrives
            // once it does exist removes the mark rather than finding none.
            self.state.lock().unwrap().watched.insert(dir.clone());
            if watcher
                .watch(&dir, notify::RecursiveMode::NonRecursive)
                .is_err()
            {
                self.state.lock().unwrap().watched.remove(&dir);
            }
        }
    }
}

/// Each of `files` with the SHA-256 of its contents, streamed rather than read whole, or taken
/// from `cache` when the file's length, modification time, inode and device are those of the last
/// read and no filesystem event for it has arrived since. The stamp is taken before the read, so a
/// file that changes while it is hashed is hashed again next time. A file that cannot be read is
/// left out: the paths come from the command's text alone, so one that becomes readable later
/// still makes the list differ.
fn read_all(files: Vec<PathBuf>, cache: &HashCache) -> Vec<FileRead> {
    use sha2::Digest;
    cache.watch(&files);
    let reads: Vec<(PathBuf, Stamp, u64, bool, String)> = files
        .into_iter()
        .filter_map(|path| {
            let mut file = std::fs::File::open(&path).ok()?;
            let stamp = Stamp::of(&file.metadata().ok()?);
            let (cached, seq, watched) = {
                let state = cache.state.lock().unwrap();
                let watched = path.parent().is_some_and(|d| state.watched.contains(d));
                let cached = state
                    .entries
                    .get(&path)
                    .filter(|(then, _)| watched && *then == stamp)
                    .map(|(_, sha)| sha.clone());
                (cached, state.seq, watched)
            };
            let sha256 = match cached {
                Some(sha) => sha,
                None => {
                    let mut hash = sha2::Sha256::new();
                    std::io::copy(&mut file, &mut hash).ok()?;
                    format!("{:x}", hash.finalize())
                }
            };
            Some((path, stamp, seq, watched, sha256))
        })
        .collect();
    // Only what the command names now stays remembered, so the cache cannot grow past it, and only
    // a watched file no event touched since its read began: that event may be the edit that this
    // read's stamp and contents do not show.
    let mut state = cache.state.lock().unwrap();
    let mut kept = HashMap::new();
    for (path, stamp, seq, watched, sha) in &reads {
        let quiet = state.reset <= *seq && state.touched.get(path).is_none_or(|at| at <= seq);
        if *watched && quiet {
            kept.insert(path.clone(), (stamp.clone(), sha.clone()));
        }
    }
    state.entries = kept;
    let named: HashSet<&PathBuf> = reads.iter().map(|(path, ..)| path).collect();
    state.touched.retain(|path, _| named.contains(path));
    drop(state);
    reads
        .into_iter()
        .map(|(path, _, _, _, sha256)| FileRead {
            path: path.display().to_string(),
            sha256,
        })
        .collect()
}

/// What `command`, run for the artifact in folder `dir` in the space directory `cwd`, names inside
/// either of them now: the files an approval of it covers. `panes` is the parent of every pane's
/// data directory.
pub(crate) fn reads(
    command: &str,
    dir: &str,
    cwd: &str,
    panes: &Path,
    cache: &HashCache,
) -> Vec<FileRead> {
    read_all(named_files(command, dir, cwd, panes), cache)
}

/// Where a remembered approval leaves a command now.
#[derive(Debug, PartialEq)]
pub(crate) enum Standing {
    /// Approved, and every file it names is as it was then.
    Trusted,
    /// Nobody answered "always" to this command in this folder.
    Unapproved,
    /// Approved, but these files it names have changed, appeared or gone since.
    Changed(Vec<String>),
}

/// The file in the instance directory holding what a person answered "always" to. It lives there
/// rather than in the saved layout, where nothing about an artifact goes: a daemon that drops
/// unknown top-level keys when it rewrites the layout must not be able to lose it.
const TRUST_FILE: &str = "artifact-trust.json";

/// The trust file's contents.
#[derive(Serialize, Deserialize, Default)]
struct Trusted {
    #[serde(default)]
    trusted: Vec<TrustInfo>,
}

/// The remembered approvals: the only durable artifact state the daemon owns. Every read and every
/// write goes to the file itself, so an approval a person removes by hand takes effect at the next
/// run; one lock around the file is what makes a read-modify-write atomic.
pub(crate) struct TrustFile {
    path: PathBuf,
}

impl TrustFile {
    pub(crate) fn new(instance_dir: &Path) -> TrustFile {
        TrustFile {
            path: instance_dir.join(TRUST_FILE),
        }
    }

    /// Every remembered approval, sorted by folder, space directory and command. A file that is not there yet holds
    /// none, which is not an error; a file that exists and cannot be read or parsed is an error, so
    /// that a write never replaces approvals this could not see.
    fn read(&self) -> Result<Vec<TrustInfo>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        let mut all = serde_json::from_str::<Trusted>(&text)
            .map_err(|e| format!("{} is not a readable trust file: {e}", self.path.display()))?
            .trusted;
        all.sort_by(|a, b| (&a.dir, &a.cwd, &a.command).cmp(&(&b.dir, &b.cwd, &b.command)));
        Ok(all)
    }

    /// The same for a reader that has no one to report a damaged file to: it holds nothing, so a
    /// damaged file asks the person again rather than approving on its own.
    fn load(&self) -> Vec<TrustInfo> {
        self.read()
            .inspect_err(|e| {
                tracing::warn!(path = %self.path.display(), error = %e, "the artifact trust file cannot be read; nothing is trusted")
            })
            .unwrap_or_default()
    }

    /// Write the whole list through a temporary file, so a crash mid-write leaves the old one.
    fn save(&self, trusted: &[TrustInfo]) -> Result<(), String> {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            let all = Trusted {
                trusted: trusted.to_vec(),
            };
            std::fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
            std::fs::rename(tmp, &self.path)
        };
        write().map_err(|e| format!("write {}: {e}", self.path.display()))
    }

    /// Whether this exact folder, space directory and command were approved, and whether `now` —
    /// the files the command names, read just before ([`reads`]) — is what they were then. Read
    /// from the file every time, a timer's ticks included: reading and parsing a 20-row trust file
    /// takes 60µs, so one tick per second per refreshing pane costs nothing worth keeping a stale
    /// copy for, and an approval removed by hand takes effect at the next tick.
    pub(crate) fn standing(
        &self,
        dir: &str,
        cwd: &str,
        command: &str,
        now: &[FileRead],
    ) -> Standing {
        let all = self.load();
        let Some(then) = all
            .iter()
            .find(|t| t.dir == dir && t.cwd == cwd && t.command == command)
        else {
            return Standing::Unapproved;
        };
        if then.reads == now {
            return Standing::Trusted;
        }
        // Every path whose contents differ, or that only one of the two lists has.
        let sha = |reads: &[FileRead], path: &str| {
            reads
                .iter()
                .find(|r| r.path == path)
                .map(|r| r.sha256.clone())
        };
        let mut changed: Vec<String> = now
            .iter()
            .chain(&then.reads)
            .map(|r| r.path.clone())
            .filter(|path| sha(now, path) != sha(&then.reads, path))
            .collect();
        changed.sort();
        changed.dedup();
        Standing::Changed(changed)
    }

    /// Remember one approval, replacing an earlier one of the same folder, space directory and
    /// command rather than listing it twice. Both writers read with [`TrustFile::read`], so a file
    /// that cannot be read stops the write rather than replacing every approval in it with this
    /// one.
    fn remember(&self, entry: TrustInfo) -> Result<(), String> {
        let mut all = self.read()?;
        all.retain(|t| !(t.dir == entry.dir && t.cwd == entry.cwd && t.command == entry.command));
        all.push(entry);
        all.sort_by(|a, b| (&a.dir, &a.cwd, &a.command).cmp(&(&b.dir, &b.cwd, &b.command)));
        self.save(&all)
    }

    /// Forget every approval remembered for artifact `name`, whatever scope it was approved in and
    /// however many commands it has had. Refused when nothing is remembered for that name, so a
    /// typo is not a silent success.
    fn forget(&self, name: &str) -> Result<Vec<TrustInfo>, String> {
        let all = self.read()?;
        let left: Vec<TrustInfo> = all.iter().filter(|t| t.name != name).cloned().collect();
        if left.len() == all.len() {
            return Err(format!("no approval is remembered for artifact {name}"));
        }
        self.save(&left)?;
        tracing::info!(%name, forgot = all.len() - left.len(), left = left.len(), "artifact approval forgotten");
        Ok(left)
    }
}

/// What a reader of a runner sees: what it would run now, and what the last run did.
struct Seen {
    command: String,
    every: Option<u64>,
    /// The folder the entry's page really sat in at the last read.
    dir: String,
    runs: u64,
    last: Option<RunInfo>,
}

/// One pane's refresh runner: it owns that pane's command and its last result, and nothing else
/// runs a command for that pane. Every input it needs is taken when the pane opens, so a run holds
/// no lock of the daemon's but its own.
pub(crate) struct Refresh {
    pane: String,
    artifact: ArtifactRef,
    /// The space's directory: the command's working directory.
    cwd: String,
    /// The `wheel.toml` holding the artifact's entry, read afresh every run: the command, the
    /// interval and the folder trust is keyed by all come from it as it is now.
    entry: PathBuf,
    /// The pane's data directory: the one place the command is given to write.
    data: PathBuf,
    perms: Arc<Perms>,
    trust: Arc<Mutex<TrustFile>>,
    /// The hashes of the files this runner's command names, so an unchanged file is not read again.
    hashes: Arc<HashCache>,
    /// The program each run starts as its warden: `wheeld` itself.
    warden: PathBuf,
    /// The daemon's PATH, which each run's command is given as it starts.
    path: Arc<Mutex<wheel_proto::LoginPath>>,
    /// One run at a time per pane, so two shells never write one data directory and an on-demand
    /// run during the on-open one waits rather than racing it.
    running: tokio::sync::Mutex<()>,
    seen: Mutex<Seen>,
    /// Why the last tick skipped, until a tick runs again: a skip is logged when this changes, so a
    /// pane left open on an unapproved command writes one line rather than one per tick.
    skipped: Mutex<Option<&'static str>>,
    /// This pane's timer task, when the entry asked for one as the pane opened. Its liveness is
    /// what a reader is told about the timer, so a timer that is gone is never reported as ticking.
    timer: std::sync::OnceLock<tokio::task::AbortHandle>,
}

/// Record why a tick skipped; true when that differs from the last skip, so it is worth a log line.
fn note_skip(last: &mut Option<&'static str>, why: &'static str) -> bool {
    last.replace(why) != Some(why)
}

impl Refresh {
    /// Run the command the entry declares now, asking the person first unless the pair is
    /// already trusted. The record it returns is the one `status` reports.
    ///
    /// The entry is read afresh every run, so editing `command` changes what runs — and what is
    /// asked about — rather than running whatever the pane opened with. An error only when there is
    /// nothing to run at all; a command that failed, or that the person refused, is a successful
    /// read of a failed run.
    pub(crate) async fn run(&self) -> Result<RefreshInfo, String> {
        let _one = self.running.lock().await;
        let (command, dir) = self.declared()?;
        let started = self.start(&command);
        let allowed = self.approved(&command, &dir).await;
        self.finish(&command, started, allowed).await;
        Ok(self.info().await)
    }

    /// Where this pane's approval of `command` stands now, and the files it names: hashed before
    /// the trust file's lock is taken, so a large file read holds up no other pane. The hashing
    /// runs on a blocking thread, so it never holds an async worker, and skips a file whose stat is
    /// unchanged.
    async fn standing(&self, command: &str, dir: &Path) -> (Standing, Vec<FileRead>) {
        let dir = dir.display().to_string();
        let now = {
            let (command, dir, cwd) = (command.to_string(), dir.clone(), self.cwd.clone());
            let panes = self.data.parent().unwrap_or(&self.data).to_path_buf();
            let hashes = self.hashes.clone();
            tokio::task::spawn_blocking(move || reads(&command, &dir, &cwd, &panes, &hashes))
                .await
                .unwrap_or_else(|e| {
                    // No reads: the approval then reads as changed, so a failed hash asks again.
                    tracing::warn!(pane = %self.pane, error = %e, "the file hashing task did not finish");
                    Vec::new()
                })
        };
        let standing = self
            .trust
            .lock()
            .unwrap()
            .standing(&dir, &self.cwd, command, &now);
        (standing, now)
    }

    /// One timer tick: the same run as any other, except that it never asks. A tick that cannot run
    /// skips — it is not a run, so nothing about it is recorded as one — and says why, rather than
    /// raising a request nobody is there to expect or ending the timer: the next tick runs as soon
    /// as the entry declares a command again, or the person's answer to an on-demand refresh
    /// remembers this one.
    ///
    /// Taking the pane's run lock first is what keeps a tick from overlapping a run: a tick landing
    /// during an on-demand run — or during the wait on a person's answer to the opening one — waits
    /// for it, and reads the entry afresh once it has the lock.
    async fn tick(&self) {
        let _one = self.running.lock().await;
        let skip = |why: &'static str| {
            if note_skip(&mut self.skipped.lock().unwrap(), why) {
                tracing::info!(pane = %self.pane, entry = %self.entry.display(), %why, "refresh timer skipped a tick");
            }
        };
        let Ok((command, dir)) = self.declared() else {
            return skip(
                "the entry declares no refresh command any more, or its page is not there",
            );
        };
        if self.seen.lock().unwrap().every.is_none() {
            return skip("the entry does not ask for a refresh on a timer any more");
        }
        match self.standing(&command, &dir).await.0 {
            Standing::Trusted => {}
            Standing::Unapproved => {
                return skip("this command is not remembered as approved, and a tick never asks");
            }
            Standing::Changed(_) => {
                return skip("a file this command names changed since it was approved, and a tick never asks");
            }
        }
        *self.skipped.lock().unwrap() = None;
        let started = self.start(&command);
        self.finish(&command, started, Ok(true)).await;
    }

    /// What this pane's timer would do if it ticked now: read from the entry, the trust file and
    /// the timer task itself every time, so what a reader is told is the timer's state now and
    /// never an outcome a later edit or approval has outgrown.
    async fn timer(&self) -> TimerState {
        let Some(task) = self.timer.get() else {
            return TimerState::Off;
        };
        if task.is_finished() {
            return TimerState::Stopped;
        }
        let Ok((Entry { command, every, .. }, dir)) = self.definition() else {
            return TimerState::Idle;
        };
        let (Some(command), Some(_)) = (command, every) else {
            return TimerState::Idle;
        };
        match self.standing(&command, &dir).await.0 {
            Standing::Trusted => TimerState::Ticking,
            Standing::Unapproved | Standing::Changed(_) => TimerState::Untrusted,
        }
    }

    /// The artifact's entry as it is now, and the folder its page really sits in: an edited
    /// command, interval or path is what runs — and what is asked about — at the next run, rather
    /// than whatever the pane opened with.
    fn definition(&self) -> Result<(Entry, PathBuf), String> {
        let entry =
            crate::artifacts::entry(&self.entry, self.artifact.name())?.ok_or_else(|| {
                format!(
                    "{} has no entry for {} any more",
                    self.entry.display(),
                    self.artifact.name()
                )
            })?;
        let dir = entry.locate()?.root;
        Ok((entry, dir))
    }

    /// What the entry declares to run now, and the folder it is approved for, recorded for a
    /// reader along with the interval it asks for. An error when it declares no command at all, or
    /// its page is not anywhere: there is nothing to run.
    fn declared(&self) -> Result<(String, PathBuf), String> {
        let (Entry { command, every, .. }, dir) = self.definition()?;
        let Some(command) = command else {
            return Err(format!(
                "the entry for {} in {} declares no refresh command, so there is nothing to run for pane {}",
                self.artifact.name(),
                self.entry.display(),
                self.pane
            ));
        };
        let mut seen = self.seen.lock().unwrap();
        seen.command = command.clone();
        seen.every = every;
        seen.dir = dir.display().to_string();
        Ok((command, dir))
    }

    /// Mark a run as starting: when it started, for a reader, and the clock its duration comes off.
    fn start(&self, command: &str) -> (u64, Instant) {
        tracing::info!(pane = %self.pane, scope = self.artifact.scope().name(), name = self.artifact.name(), %command, cwd = %self.cwd, data = %self.data.display(), "refresh start");
        (unix_ms(SystemTime::now()), Instant::now())
    }

    /// Run `command` if `allowed` says the person allows it, and record what happened as this
    /// runner's last run either way: a refusal is a run that did not run, not a run that was lost.
    async fn finish(&self, command: &str, started: (u64, Instant), allowed: Result<bool, String>) {
        let (started_ms, at) = started;
        let (exit, error, stderr_tail) = match allowed {
            Ok(true) => self.exec(command).await,
            Ok(false) => (
                None,
                Some("the person refused this command".into()),
                String::new(),
            ),
            // The request was withdrawn: the pane stopped showing the artifact while it waited.
            Err(e) => (None, Some(e), String::new()),
        };
        let last = RunInfo {
            started_ms,
            duration_ms: at.elapsed().as_millis() as u64,
            exit,
            error,
            stderr_tail,
        };
        tracing::info!(pane = %self.pane, %command, exit = ?last.exit, error = last.error.as_deref().unwrap_or("-"), duration_ms = last.duration_ms, stderr_bytes = last.stderr_tail.len(), ok = last.ok(), "refresh end");
        let mut seen = self.seen.lock().unwrap();
        seen.runs += 1;
        seen.last = Some(last);
    }

    /// Whether the person allows `command` for this artifact's folder: at once when the pair is
    /// remembered, otherwise by asking, which waits for an answer. "Always" is remembered before
    /// the command runs, so a daemon that dies mid-run does not ask again for what it was told.
    async fn approved(&self, command: &str, dir: &Path) -> Result<bool, String> {
        let (standing, reads) = self.standing(command, dir).await;
        // Quoted, so the person sees the command exactly as the entry wrote it, and told which
        // artifact and which project directory it would run in.
        let title = format!(
            "Refresh the {} artifact {} in {}: run {command:?}",
            self.artifact.scope().name(),
            self.artifact.name(),
            self.cwd
        );
        let whose = Approval {
            pane: &self.pane,
            scope: self.artifact.scope(),
            name: self.artifact.name(),
            dir,
            cwd: &self.cwd,
            command,
        };
        ask_approval(&self.perms, &self.trust, &whose, title, standing, reads).await
    }

    /// Run `command` through `/bin/sh -c` in the space's directory, with the pane's data directory
    /// and the pane in its environment. Returns its exit status, why it has none, and the tail of
    /// its standard error.
    async fn exec(&self, command: &str) -> (Option<i32>, Option<String>, String) {
        // The command is given a directory to write into, so it is made again here: the pane's
        // data directory is removed when the pane stops showing the artifact, and a run races that.
        if let Err(e) = std::fs::create_dir_all(&self.data) {
            return (
                None,
                Some(format!("make {}: {e}", self.data.display())),
                String::new(),
            );
        }
        // Declared before the run's future, so it drops after it: a pane that closes mid-run aborts
        // the task holding this, and the limit below drops the run, and either way the whole group
        // dies rather than writing on into a directory nothing serves.
        let mut group = match Group::arm(&self.pane, &self.warden).await {
            Ok(group) => group,
            Err(e) => return (None, Some(e), String::new()),
        };
        let child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .env("PATH", self.path.lock().unwrap().path.clone())
            .env("WHEEL_PANE", &self.pane)
            .env("WHEEL_SPACE_DIR", &self.cwd)
            .env("WHEEL_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The shell joins the warden's group, so everything it starts — work it backgrounds
            // included — can be killed with it rather than outliving the run.
            .process_group(group.pgid)
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => return (None, Some(format!("run {command:?}: {e}")), String::new()),
        };
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        // Outside the run, so what was read stays readable when the limit drops the run.
        let (mut out, mut err) = (Captured::default(), Captured::default());
        let run = async {
            let reading = async {
                tokio::join!(drain(stdout, &mut out), drain(stderr, &mut err));
            };
            tokio::pin!(reading);
            let mut read = false;
            // The run ends when the shell does, whoever still holds its pipes.
            let status = tokio::select! {
                status = child.wait() => status,
                () = &mut reading => {
                    read = true;
                    child.wait().await
                }
            };
            // Whatever the shell left running is killed with its group, which closes the pipes it
            // held. A process that left the group holds them still, so the rest of the output gets
            // a moment rather than the run waiting on it until the limit.
            group.kill();
            if !read {
                let _ = tokio::time::timeout(PIPE_GRACE, &mut reading).await;
            }
            status
        };
        // A command that never ends would hold this pane's runner forever, so every later refresh
        // of that pane would wait behind it rather than reading fresh data. Dropping the run kills
        // its group, so the limit ends the command as well as the wait.
        let status = tokio::time::timeout(RUN_LIMIT, run).await;
        let status = match status {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return (None, Some(format!("run {command:?}: {e}")), tail(&err.end)),
            Err(_) => {
                tracing::warn!(pane = %self.pane, %command, limit_secs = RUN_LIMIT.as_secs(), "refresh command stopped at its time limit");
                return (
                    None,
                    Some(format!(
                        "the command ran longer than {}s and was stopped",
                        RUN_LIMIT.as_secs()
                    )),
                    tail(&err.end),
                );
            }
        };
        let stderr = tail(&err.end);
        tracing::info!(pane = %self.pane, stdout_bytes = out.total, stderr_bytes = err.total, %status, "refresh command finished");
        match status.code() {
            Some(code) => (Some(code), None, stderr),
            // Killed by a signal: there is no exit status to report, so the reason goes in `error`
            // rather than being reported as some number the shell never returned.
            None => {
                let signal = std::os::unix::process::ExitStatusExt::signal(&status);
                (
                    None,
                    Some(format!(
                        "the command was killed by signal {}",
                        signal.unwrap_or(0)
                    )),
                    stderr,
                )
            }
        }
    }

    /// This runner as a reader sees it.
    pub(crate) async fn info(&self) -> RefreshInfo {
        let timer = self.timer().await;
        let seen = self.seen.lock().unwrap();
        RefreshInfo {
            pane: self.pane.clone(),
            scope: self.artifact.scope(),
            name: self.artifact.name().to_string(),
            dir: seen.dir.clone(),
            cwd: self.cwd.clone(),
            data_dir: self.data.display().to_string(),
            command: seen.command.clone(),
            every: seen.every,
            timer,
            runs: seen.runs,
            last: seen.last.clone(),
        }
    }
}

/// What an approval is of: the command, the folder of the artifact that asks for it and the
/// directory it runs in, which is the key the trust file holds it under.
pub(crate) struct Approval<'a> {
    pub(crate) pane: &'a str,
    pub(crate) scope: wheel_proto::ArtifactScope,
    pub(crate) name: &'a str,
    pub(crate) dir: &'a Path,
    pub(crate) cwd: &'a str,
    pub(crate) command: &'a str,
}

/// Whether the person allows `of.command`: at once when `standing` says the pair is remembered,
/// otherwise by asking at pane `of.pane`, which waits for an answer. `title` is what the request
/// says; a change to a file the command names is appended to it. "Always" is remembered before the
/// command runs, so a daemon that dies mid-run does not ask again for what it was told. The one
/// procedure behind a refresh command and a command a page asks to run.
pub(crate) async fn ask_approval(
    perms: &Perms,
    trust: &Mutex<TrustFile>,
    of: &Approval<'_>,
    mut title: String,
    standing: Standing,
    reads: Vec<FileRead>,
) -> Result<bool, String> {
    let (pane, command) = (of.pane, of.command);
    let dir = of.dir.display().to_string();
    match &standing {
        Standing::Trusted => {
            tracing::info!(%pane, %dir, %command, "command already trusted");
            return Ok(true);
        }
        Standing::Changed(changed) => {
            tracing::info!(%pane, %dir, %command, changed = ?changed, "a file the approved command names changed; asking again");
            title.push_str(&format!(
                " ({} changed since it was approved)",
                changed.join(", ")
            ));
        }
        Standing::Unapproved => {}
    }
    // Remembering is the reads taken now, before the wait on an answer: a file that changes
    // while the person reads the request is not the one they approved, so the next run asks.
    let options = [
        ("allow", "Allow once", "allow_once"),
        ("always", "Always allow", "allow_always"),
        ("deny", "Deny", "reject_once"),
    ]
    .into_iter()
    .map(|(id, name, kind)| PermOption {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
    })
    .collect::<Vec<_>>();
    let choice = perms.ask(pane, title, options).await?;
    if choice == "always" {
        let files = reads.len();
        let entry = TrustInfo {
            dir: dir.clone(),
            cwd: of.cwd.to_string(),
            command: command.to_string(),
            scope: of.scope,
            name: of.name.to_string(),
            approved_ms: unix_ms(SystemTime::now()),
            reads,
        };
        match trust.lock().unwrap().remember(entry) {
            Ok(()) => {
                tracing::info!(%pane, %dir, %command, files, "command trusted from now on")
            }
            // The run still happens: the person allowed it. Only the remembering failed, so
            // the next run asks again rather than the allowed run being lost.
            Err(e) => {
                tracing::warn!(%pane, %dir, error = %e, "could not remember the approval; the next run asks again")
            }
        }
    }
    tracing::info!(%pane, %choice, "command answered");
    // An allow-list, so an option id this does not know refuses rather than runs: the one place
    // a command gets to run is the two answers that say so.
    Ok(choice == "allow" || choice == "always")
}

/// Tick `refresh` every `every` seconds for as long as the pane owns this task: the pane dropping
/// it is the only thing that ends the timer, so every other condition — a command nobody has
/// approved for good, an entry that lost its command, an `every` that came back — is a tick that
/// skips or runs and never a timer that cannot come back.
///
/// One sequential task, so at most one tick is ever pending: a run that outlasts its own interval
/// delays the next tick rather than being overlapped by it, and ticks do not pile up behind it. The
/// interval comes from each tick's own reading of the entry, so editing `every` takes effect at
/// the tick after the edit, and the first tick is one interval after the pane's opening run.
async fn ticking(refresh: Arc<Refresh>, mut every: u64) {
    loop {
        tokio::time::sleep(Duration::from_secs(every)).await;
        refresh.tick().await;
        // The interval the entry asks for now; the one last read when it asks for none, so that
        // re-adding `every` is picked up rather than needing the pane reopened.
        every = refresh.seen.lock().unwrap().every.unwrap_or(every);
    }
}

/// A pane's runner and every task running for it: the run the pane's opening asks for, each run a
/// caller asks for, and its timer. Dropping it stops all of them, so a pane that
/// stops showing its artifact stops refreshing — the command of a run in flight is killed with the
/// future holding it, rather than writing on into a data directory that is about to be removed.
pub(crate) struct Refresher {
    pub(crate) refresh: Arc<Refresh>,
    runs: Vec<tokio::task::AbortHandle>,
}

impl Refresher {
    /// Take a run's task under this runner, so the pane owns it. The handles of runs that have
    /// finished are dropped here, rather than collecting for as long as the pane is open.
    fn owns(&mut self, task: tokio::task::AbortHandle) {
        self.runs.retain(|t| !t.is_finished());
        self.runs.push(task);
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        for task in &self.runs {
            task.abort();
        }
    }
}

/// The process group a refresh runs in: the shell and everything it starts, led by the run's
/// warden. Killing it once, when the shell exits or when the run is dropped first, is what leaves
/// nothing of a run alive after it ends, whether it finished, hit its time limit or was cancelled by
/// its pane.
///
/// All of that runs inside wheeld, so a daemon killed with SIGKILL or crashing would leave the
/// group running with nothing to stop it — macOS has no signal that tells a child its parent died.
/// The warden covers that. It is wheeld itself, started as [`WARDEN_ARG`] before the shell exists,
/// leading the group the shell then joins; it asks the kernel to wake it when its parent exits
/// (kqueue's `NOTE_EXIT`) and kills its own group when that happens, at that moment and with no
/// restart needed. It holds nothing of wheeld's — no pipe, no descriptor — so no other process
/// wheeld started can keep it from seeing the exit. Killing the group kills the warden with it.
struct Group<'a> {
    pane: &'a str,
    /// The warden's pid, which is the group's id; 0 once killed.
    pgid: i32,
    /// The warden, never waited on while the group lives: an unreaped warden keeps its pid, and so
    /// the group's id, from being handed to any other process.
    _warden: tokio::process::Child,
}

/// The argument that starts `wheeld` as a refresh's warden rather than as the daemon.
pub const WARDEN_ARG: &str = "--refresh-warden";

/// What a warden prints once it is watching its parent, so a run never starts unwatched.
const ARMED: &str = "armed";

/// How long a warden has to say it is watching. It is one exec and two system calls.
const WARDEN_START: Duration = Duration::from_secs(5);

impl<'a> Group<'a> {
    /// Start a run's warden, `program` with [`WARDEN_ARG`], as the leader of a new group, and wait
    /// for it to say it is watching wheeld.
    async fn arm(pane: &'a str, program: &Path) -> Result<Group<'a>, String> {
        let mut warden = tokio::process::Command::new(program)
            .arg(WARDEN_ARG)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("start the refresh warden {}: {e}", program.display()))?;
        let pgid = warden.id().map_or(0, |id| id as i32);
        let mut said = String::new();
        if let Some(out) = warden.stdout.take() {
            let mut out = tokio::io::BufReader::new(out);
            let _ = tokio::time::timeout(
                WARDEN_START,
                tokio::io::AsyncBufReadExt::read_line(&mut out, &mut said),
            )
            .await;
        }
        // Built before the check, so a warden that said the wrong thing is killed on the way out.
        let group = Group {
            pane,
            pgid,
            _warden: warden,
        };
        if pgid <= 0 || said.trim_end() != ARMED {
            return Err(format!(
                "the refresh warden {} did not say it was watching wheeld (said {said:?})",
                program.display()
            ));
        }
        tracing::info!(pane = %pane, pgid, "refresh warden armed");
        Ok(group)
    }

    fn kill(&mut self) {
        if self.pgid <= 0 {
            return;
        }
        // SAFETY: killpg takes two integers and touches no memory of ours. The id is the warden's
        // pid, and the warden is not reaped until this group is dropped, so no other process can
        // have been given it. The warden is a member, so the group always has one to kill.
        let alive = unsafe { libc::killpg(self.pgid, libc::SIGKILL) } == 0;
        if alive {
            tracing::info!(pane = %self.pane, pgid = self.pgid, "refresh process group killed");
        }
        self.pgid = 0;
    }
}

impl Drop for Group<'_> {
    fn drop(&mut self) {
        // The warden goes with the group; tokio reaps it in the background once it is dropped.
        self.kill();
    }
}

/// Run as a refresh's warden: watch the parent that started this process, and when it exits, kill
/// this process's group — the refresh command and everything it started — and with it this process.
/// `wheeld` calls this for [`WARDEN_ARG`] before it starts a runtime, a thread or anything else.
pub fn warden() -> ! {
    // SAFETY: each call takes integers or a pointer to a `kevent` on this stack, valid for the call.
    unsafe {
        let parent = libc::getppid();
        let kq = libc::kqueue();
        let change = libc::kevent {
            ident: parent as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // The watch names a pid; the parent check after it is what says that pid is still the
        // process that started this one, and not one handed the pid after wheeld already died.
        let watching = parent > 1
            && kq >= 0
            && libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0
            && libc::getppid() == parent;
        if watching {
            println!("{ARMED}");
            let mut fired: libc::kevent = std::mem::zeroed();
            while libc::kevent(kq, std::ptr::null(), 0, &mut fired, 1, std::ptr::null()) < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
            }
        }
        libc::killpg(libc::getpgrp(), libc::SIGKILL);
        libc::_exit(1)
    }
}

/// The end of a stream and how long the whole stream was. It keeps one byte more than
/// `STDERR_TAIL`, so `tail` can tell output that was cut from output that just fit, and holds no
/// more however much the command writes.
#[derive(Default)]
struct Captured {
    end: Vec<u8>,
    total: u64,
}

impl Captured {
    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        self.end.extend_from_slice(chunk);
        let keep = STDERR_TAIL + 1;
        if self.end.len() > keep {
            self.end.drain(..self.end.len() - keep);
        }
    }
}

/// Read `pipe` until it closes, keeping the end of it in `seen`. A read error ends it with what was
/// read so far: the output is for a reader's tail, and the exit status is what says whether the run
/// worked.
async fn drain(pipe: Option<impl tokio::io::AsyncRead + Unpin>, seen: &mut Captured) {
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
            if n == 0 {
                break;
            }
            seen.push(&chunk[..n]);
        }
    }
}

/// The last `STDERR_TAIL` bytes of `bytes` as text, cut at a character boundary and marked when
/// anything was left off, so a reader is never shown a tail that looks like the whole output.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= STDERR_TAIL {
        return text.into_owned();
    }
    let mut from = text.len() - STDERR_TAIL;
    while from < text.len() && !text.is_char_boundary(from) {
        from += 1;
    }
    format!("…{}", &text[from..])
}

/// `at` in milliseconds since the Unix epoch; 0 for a clock before it.
fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl AppState {
    /// Start refreshing for web pane `pane`, which shows `artifact` in a space whose directory is
    /// `cwd` and whose data directory is `data`: register the runner and run it once, now, for the
    /// opening. Replaces any earlier runner of that pane. An artifact whose entry declares no
    /// command registers nothing, so nothing to run is nothing to report. Call from inside the
    /// tokio runtime.
    pub(crate) fn start_refresh(
        &self,
        pane: &str,
        artifact: &ArtifactRef,
        cwd: &str,
        data: PathBuf,
    ) {
        // The folder the pane serves from, and nothing else: an artifact whose page is not anywhere
        // serves nothing, so its command never runs either.
        let entry = self.artifact_registry(artifact.scope(), cwd);
        let (Entry { command, every, .. }, dir) = match self.artifact_entry(artifact, cwd).and_then(
            |found| {
                let found = found.ok_or("no entry for it")?;
                let dir = found.locate()?.root;
                Ok((found, dir))
            },
        ) {
            Ok(defined) => defined,
            Err(e) => {
                tracing::warn!(%pane, scope = artifact.scope().name(), name = artifact.name(), error = %e, "the artifact's page is not anywhere; nothing refreshes this pane");
                self.stop_refresh(pane);
                return;
            }
        };
        let Some(command) = command else {
            self.stop_refresh(pane);
            return;
        };
        let refresh = Arc::new(Refresh {
            pane: pane.to_string(),
            artifact: artifact.clone(),
            cwd: cwd.to_string(),
            entry,
            data,
            perms: self.perms.clone(),
            trust: self.trust.clone(),
            hashes: Arc::default(),
            warden: self.warden.clone(),
            path: self.login_path.clone(),
            running: tokio::sync::Mutex::new(()),
            seen: Mutex::new(Seen {
                command: command.clone(),
                every,
                dir: dir.display().to_string(),
                runs: 0,
                last: None,
            }),
            skipped: Mutex::new(None),
            timer: std::sync::OnceLock::new(),
        });
        let opening = refresh.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = opening.run().await {
                tracing::warn!(pane = %opening.pane, error = %e, "the pane's opening refresh ran nothing");
            }
        });
        tracing::info!(%pane, scope = artifact.scope().name(), name = artifact.name(), %command, ?every, "refreshing web pane");
        let mut refresher = Refresher {
            refresh,
            runs: Vec::new(),
        };
        refresher.owns(task.abort_handle());
        // The timer is one more task the pane owns, so a pane that closes or changes kind stops
        // ticking the same way it cancels a run: by this runner being dropped. The runner keeps the
        // handle as well as the pane, which is how a reader is told the timer is still there.
        if let Some(every) = every {
            let timer = tokio::spawn(ticking(refresher.refresh.clone(), every));
            let _ = refresher.refresh.timer.set(timer.abort_handle());
            refresher.owns(timer.abort_handle());
            tracing::info!(%pane, name = artifact.name(), every_secs = every, "refresh timer armed");
        }
        // The pane's earlier runner, if it had one, drops here: whatever was running for the
        // artifact it was showing is cancelled before the new one's first run can start.
        self.refreshers
            .lock()
            .unwrap()
            .insert(pane.to_string(), refresher);
    }

    /// Stop refreshing for pane `pane`: a run in flight is cancelled and its command killed.
    pub(crate) fn stop_refresh(&self, pane: &str) {
        if self.refreshers.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "stopped refreshing web pane");
        }
    }

    /// Run web pane `pane`'s refresh now and give back the run. Refused when that pane has no
    /// runner: it shows no artifact, or its artifact declares no command.
    ///
    /// The run happens in a task the pane's runner owns, not on the caller's: a pane that closes
    /// or changes kind while this waits — on a person's answer, or on the command itself — cancels
    /// it, and this says so rather than reporting a run that no longer belongs to anything.
    ///
    /// The runner is found and the run started before this returns, so a request queued behind
    /// this one on the same connection finds the run already owned by the pane; only the wait for
    /// the result is left to the returned future.
    pub(crate) fn refresh_pane(
        &self,
        pane: &str,
    ) -> Result<impl std::future::Future<Output = Result<RefreshInfo, String>>, String> {
        let task = {
            let mut refreshers = self.refreshers.lock().unwrap();
            let refresher = refreshers.get_mut(pane).ok_or_else(|| {
                format!("pane {pane} has no refresh: it shows no artifact, or that artifact's entry declares no command")
            })?;
            let refresh = refresher.refresh.clone();
            let task = tokio::spawn(async move { refresh.run().await });
            refresher.owns(task.abort_handle());
            task
        };
        let pane = pane.to_string();
        Ok(async move {
            match task.await {
                Ok(result) => result,
                Err(e) if e.is_cancelled() => Err(format!(
                    "pane {pane} stopped showing its artifact, so its refresh was cancelled"
                )),
                Err(e) => Err(format!("pane {pane}'s refresh could not be run: {e}")),
            }
        })
    }

    /// Every refreshing pane's runner, sorted by pane. Each runner is read after the runners' lock
    /// is let go: reading one stats the files its command names, and hashes a changed one, and a pane opening or closing
    /// meanwhile must not wait on that.
    pub(crate) async fn refreshes(&self) -> Vec<RefreshInfo> {
        let runners: Vec<Arc<Refresh>> = self
            .refreshers
            .lock()
            .unwrap()
            .values()
            .map(|r| r.refresh.clone())
            .collect();
        let mut all = Vec::with_capacity(runners.len());
        for runner in &runners {
            all.push(runner.info().await);
        }
        all.sort_by(|a, b| a.pane.cmp(&b.pane));
        all
    }

    /// Every remembered approval.
    pub(crate) fn trusted(&self) -> Vec<TrustInfo> {
        self.trust.lock().unwrap().load()
    }

    /// Forget artifact `name`'s approvals, so its command asks again; gives back what is left.
    pub(crate) fn trust_forget(&self, name: &str) -> Result<Vec<TrustInfo>, String> {
        self.trust.lock().unwrap().forget(name)
    }
}

/// The runners, keyed by pane.
pub(crate) type Refreshers = HashMap<String, Refresher>;

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::ArtifactScope;

    fn folder(case: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wheel-trust-{}-{case}-{}",
            std::process::id(),
            unix_ms(SystemTime::now())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A repeated skip reason logs once; a changed reason, or the same one after a run cleared it,
    /// logs again.
    #[test]
    fn a_skip_logs_when_its_reason_changes() {
        let mut last = None;
        assert!(note_skip(&mut last, "a"));
        assert!(!note_skip(&mut last, "a"));
        assert!(!note_skip(&mut last, "a"));
        assert!(note_skip(&mut last, "b"));
        last = None;
        assert!(note_skip(&mut last, "b"));
    }

    /// Writes to watched files while directories are being watched and dropped: no notify call may
    /// wait on the lock the event callback takes.
    #[test]
    fn writing_watched_files_while_watching_new_ones_finishes() {
        let root = folder("watch-race");
        let dirs: Vec<PathBuf> = (0..8).map(|i| root.join(format!("d{i}"))).collect();
        for dir in &dirs {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("f"), "0").unwrap();
        }
        let cache = Arc::new(HashCache::default());
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (dirs, done) = (dirs.clone(), done.clone());
            std::thread::spawn(move || {
                let mut n = 0;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    n += 1;
                    for dir in &dirs {
                        std::fs::write(dir.join("f"), n.to_string()).unwrap();
                    }
                }
            })
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = {
            let (cache, dirs) = (cache.clone(), dirs.clone());
            std::thread::spawn(move || {
                for round in 0..8 {
                    let files: Vec<PathBuf> = dirs
                        .iter()
                        .skip(round % 4)
                        .take(4)
                        .map(|d| d.join("f"))
                        .collect();
                    read_all(files, &cache);
                }
                tx.send(()).unwrap();
            })
        };
        // Each watch call restarts the FSEvents stream, about a second apiece on an idle machine and
        // several under load; a real deadlock never finishes, so the deadline only has to outlast load.
        let finished = rx.recv_timeout(Duration::from_secs(300));
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        finished.expect("a watch call and the event callback deadlocked");
        writer.join().unwrap();
        watcher.join().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn entry(dir: &str, command: &str, name: &str) -> TrustInfo {
        TrustInfo {
            dir: dir.into(),
            cwd: "/proj".into(),
            command: command.into(),
            scope: ArtifactScope::Machine,
            name: name.into(),
            approved_ms: 1,
            reads: Vec::new(),
        }
    }

    impl TrustFile {
        fn panes(&self) -> PathBuf {
            self.path.parent().unwrap().join("panes")
        }

        fn now(&self, dir: &str, cwd: &str, command: &str) -> Standing {
            self.standing(
                dir,
                cwd,
                command,
                &reads(command, dir, cwd, &self.panes(), &HashCache::default()),
            )
        }

        fn allows(&self, dir: &str, cwd: &str, command: &str) -> bool {
            self.now(dir, cwd, command) == Standing::Trusted
        }

        /// Answer "always" to `command` as it is now.
        fn approve(&self, dir: &str, cwd: &str, command: &str) {
            let reads = reads(command, dir, cwd, &self.panes(), &HashCache::default());
            let entry = TrustInfo {
                cwd: cwd.into(),
                reads,
                ..entry(dir, command, "dash")
            };
            self.remember(entry).unwrap();
        }
    }

    /// What the trust file answers: the approval as given, nothing else, and an approval that can
    /// be withdrawn. It is the folder, the space directory *and* the command, so editing the
    /// command asks again and so does opening the artifact in another project.
    #[test]
    fn trust_is_keyed_by_the_folder_and_the_command() {
        let instance = folder("pairs");
        let trust = TrustFile::new(&instance);
        assert!(trust.load().is_empty(), "nothing is trusted to start with");
        assert!(!trust.allows("/a/dash", "/proj", "bd ready"));

        trust
            .remember(entry("/a/dash", "bd ready", "dash"))
            .unwrap();
        assert!(trust.allows("/a/dash", "/proj", "bd ready"));
        assert!(
            !trust.allows("/a/dash", "/proj", "bd ready --json"),
            "the command changed in the entry"
        );
        assert!(
            !trust.allows("/b/dash", "/proj", "bd ready"),
            "the same command in another artifact's folder"
        );
        assert!(
            !trust.allows("/a/dash", "/other", "bd ready"),
            "the same artifact in another project"
        );

        // Approving the same pair again leaves one row; another command for the same folder is its
        // own row, so an entry that goes back to an approved command does not ask again.
        trust
            .remember(entry("/a/dash", "bd ready", "dash"))
            .unwrap();
        trust
            .remember(entry("/a/dash", "bd ready --json", "dash"))
            .unwrap();
        assert_eq!(
            trust
                .load()
                .iter()
                .map(|t| t.command.clone())
                .collect::<Vec<_>>(),
            ["bd ready", "bd ready --json"]
        );

        // Forgetting is by artifact name, and takes every command remembered for it.
        trust
            .remember(entry("/c/viewer", "epg pull", "viewer"))
            .unwrap();
        assert_eq!(trust.forget("dash").unwrap().len(), 1);
        assert!(!trust.allows("/a/dash", "/proj", "bd ready"));
        assert!(trust.allows("/c/viewer", "/proj", "epg pull"));
        assert!(
            trust.forget("dash").is_err(),
            "nothing is remembered for it now"
        );

        // A file nothing can read trusts nothing, rather than everything — and a write refuses
        // rather than replacing approvals it could not see with the one it was given.
        let damaged = "{ not json";
        std::fs::write(instance.join(TRUST_FILE), damaged).unwrap();
        assert!(trust.load().is_empty());
        assert!(!trust.allows("/c/viewer", "/proj", "epg pull"));
        assert!(trust.remember(entry("/d/new", "x", "new")).is_err());
        assert!(trust.forget("viewer").is_err());
        assert_eq!(
            std::fs::read_to_string(instance.join(TRUST_FILE)).unwrap(),
            damaged,
            "a file this cannot read is left alone"
        );
        std::fs::remove_dir_all(&instance).unwrap();
    }

    /// Every spelling of a path names the file the shell would open, whichever word is the program:
    /// quoting, escapes, `..` through a link, a `cd` before it, a flag's value, a `PATH` entry, a
    /// substitution, a known variable.
    #[test]
    fn a_command_names_the_files_the_shell_would_open() {
        let root = folder("names");
        let (proj, instance) = (root.join("proj"), root.join("inst"));
        let art = instance.join("artifacts/dash");
        let elsewhere = root.join("elsewhere/sub");
        for dir in [
            proj.join("scripts"),
            proj.join("tools"),
            art.clone(),
            instance.join("panes/p1"),
            elsewhere.clone(),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(proj.join("scripts/pull.sh"), "echo v1").unwrap();
        std::fs::write(art.join("gen.sh"), "echo a1").unwrap();
        std::fs::write(root.join("elsewhere/run.sh"), "echo far").unwrap();
        std::fs::write(instance.join("panes/p1/data.json"), "{}").unwrap();
        std::os::unix::fs::symlink(&elsewhere, proj.join("link")).unwrap();
        let (p, a) = (proj.to_str().unwrap(), art.to_str().unwrap());
        let real = |path: PathBuf| std::fs::canonicalize(path).unwrap().display().to_string();
        let pull = real(proj.join("scripts/pull.sh"));
        let files = |command: &str| {
            reads(
                command,
                a,
                p,
                &instance.join("panes"),
                &HashCache::default(),
            )
            .into_iter()
            .map(|r| r.path)
            .collect::<Vec<_>>()
        };
        for command in [
            "sh scripts/pull.sh".to_string(),
            "./scripts/pull.sh --all".into(),
            "python3 tools/../scripts/pull.sh".into(),
            "sh scripts/p\"u\"ll.sh".into(),
            "sh 'scripts/pull.sh'".into(),
            "sh scripts/pull\\.sh".into(),
            "cd scripts && ./pull.sh".into(),
            "sh \"$WHEEL_SPACE_DIR/scripts/pull.sh\"".into(),
            "sh ${WHEEL_SPACE_DIR}/scripts/pull.sh".into(),
            format!("cat --from={p}/scripts/pull.sh"),
            format!("cat -o{p}/scripts/pull.sh"),
            "cat -oscripts/pull.sh".into(),
            "bd ready $(sh scripts/pull.sh)".into(),
            "bd ready `sh scripts/pull.sh`".into(),
            "bd ready; X=scripts/pull.sh true".into(),
            "/usr/bin/env -S 'sh scripts/pull.sh'".into(),
            "/bin/sh -c \"sh scripts/pull.sh $1\"".into(),
            "PATH=./scripts:$PATH pull.sh".into(),
            "make -C scripts".into(),
        ] {
            assert_eq!(files(&command), std::slice::from_ref(&pull), "{command}");
        }
        // `..` after a link is the link's parent on disk, not the folder the link sits in.
        assert_eq!(
            files("sh link/../run.sh"),
            [real(root.join("elsewhere/run.sh"))]
        );
        // A machine artifact's own folder is in the instance directory, and its script counts; the
        // panes' data directories there, which every run writes, do not.
        assert_eq!(files(&format!("sh {a}/gen.sh")), [real(art.join("gen.sh"))]);
        assert!(files(&format!("cat {}/panes/p1/data.json", instance.display())).is_empty());
        // A program outside both, and words naming no file, name nothing.
        assert!(files("/opt/homebrew/bin/bd ready > /tmp/x").is_empty());
        assert!(files("sh $SOMETHING/pull.sh").is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An approval covers the files the command names as they were: it holds while they are
    /// unchanged, and asks again, naming the file, when one is edited, appears or goes.
    #[test]
    fn an_approval_holds_only_while_the_files_it_names_are_unchanged() {
        let root = folder("reads");
        let (proj, art, instance) = (root.join("proj"), root.join("art"), root.join("inst"));
        for dir in [proj.join("scripts"), art.clone(), instance.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let pull = proj.join("scripts/pull.sh");
        std::fs::write(&pull, "echo v1").unwrap();
        let pull_path = std::fs::canonicalize(&pull).unwrap().display().to_string();
        let (p, a) = (proj.to_str().unwrap(), art.to_str().unwrap());
        let trust = TrustFile::new(&instance);

        let script = "sh scripts/pull.sh";
        assert_eq!(trust.now(a, p, script), Standing::Unapproved);
        trust.approve(a, p, script);
        assert_eq!(trust.now(a, p, script), Standing::Trusted);
        std::fs::write(&pull, "echo v2").unwrap();
        let changed = Standing::Changed(vec![pull_path.clone()]);
        assert_eq!(trust.now(a, p, script), changed, "an edit asks again");
        std::fs::write(&pull, "echo v1").unwrap();
        assert_eq!(
            trust.now(a, p, script),
            Standing::Trusted,
            "the approved contents"
        );
        std::fs::remove_file(&pull).unwrap();
        assert_eq!(trust.now(a, p, script), changed, "a file that went");

        // A file named but absent when approved is covered too: one that appears there later is
        // code nobody saw.
        trust.approve(a, p, script);
        assert_eq!(trust.now(a, p, script), Standing::Trusted);
        std::fs::write(&pull, "echo new").unwrap();
        assert_eq!(trust.now(a, p, script), changed, "a file that appeared");

        // Approving it again replaces the row rather than adding one.
        trust.approve(a, p, script);
        assert_eq!(trust.load().len(), 1);

        // The same artifact's command in another project is approved there on its own row.
        let other = root.join("other");
        std::fs::create_dir_all(other.join("scripts")).unwrap();
        std::fs::write(other.join("scripts/pull.sh"), "echo other").unwrap();
        let o = other.to_str().unwrap();
        assert_eq!(trust.now(a, o, script), Standing::Unapproved);
        trust.approve(a, o, script);
        assert!(trust.allows(a, o, script));
        assert!(trust.allows(a, p, script));
        assert_eq!(trust.load().len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A tail is the end of the output, and says when it left something off.
    #[test]
    fn the_error_tail_keeps_the_end_and_says_so() {
        assert_eq!(tail(b""), "");
        assert_eq!(tail(b"boom"), "boom");
        let long = "x".repeat(STDERR_TAIL) + "the end";
        let cut = tail(long.as_bytes());
        assert!(cut.starts_with('…'), "{cut:.20}");
        assert!(cut.ends_with("the end"));
        assert_eq!(cut.len(), STDERR_TAIL + '…'.len_utf8());
        // A cut landing inside a character takes the next whole one instead of a broken byte.
        let wide = "é".repeat(STDERR_TAIL);
        let cut = tail(wide.as_bytes());
        assert!(cut.ends_with("é"), "{cut:.20}");
        assert!(cut.chars().count() < STDERR_TAIL);
    }
}
