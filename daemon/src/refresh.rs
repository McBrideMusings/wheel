//! Running an artifact's refresh command for the pane showing it.
//!
//! The manifest declares the command; the daemon runs it through `/bin/sh` in the pane's *space*
//! directory and hands it the pane's data directory in `WHEEL_DATA_DIR`, which is where an artifact
//! writes what its page reads: the artifact's own folder is read-only static content a pane serves,
//! and a project's working tree is not for generated files. That is a place to write, not a
//! sandbox — the command is a shell the person approved, and it can reach whatever they can. A run
//! happens when the pane opens and when a caller asks for one; the timer `every` asks for is a
//! later slice, which builds on [`Refresh`] and adds nothing of its own.
//!
//! No command runs until a person approves it at a permission request — the same request the lead's
//! tools raise, which every connected client and the `perm` CLI verbs answer. Answering "always"
//! writes the pair into a trust file in the instance directory, so a restart does not ask again;
//! the pair is the artifact's *folder* and the command, so editing the command asks afresh.
//!
//! A write the command makes reaches the page through the data directory's watch (`web.rs`), the
//! same `pane_reload` a person editing a file there sends: the page's own load and the on-open run
//! race, so a run that wrote nothing back to the page would leave a pane showing an empty data
//! directory. Nothing in this build lets a page ask for a refresh, so no page can reload itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wheel_proto::{ArtifactRef, PermOption, RefreshInfo, RunInfo, TrustInfo};

use crate::artifacts::Manifest;
use crate::perms::Perms;
use crate::server::AppState;

/// How much of a run's standard error a reader gets, in bytes off the end: enough to say why a
/// command failed without putting a whole build log in every `status`.
const STDERR_TAIL: usize = 2000;

/// How long one run may take before the daemon stops it. A refresh fills a pane with current data;
/// a command still going a minute later is not doing that, and leaving it to run would hold every
/// later refresh of that pane behind it.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// The file in the instance directory holding what a person answered "always" to. It lives there
/// rather than in the saved layout for the same reason the manifests do: a daemon that drops
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

    /// Every remembered pair, sorted by folder and then command. A file that is not there yet holds
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
        all.sort_by(|a, b| (&a.dir, &a.command).cmp(&(&b.dir, &b.command)));
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

    /// Whether this exact folder and command were approved before.
    fn allows(&self, dir: &str, command: &str) -> bool {
        self.load()
            .iter()
            .any(|t| t.dir == dir && t.command == command)
    }

    /// Remember one pair, replacing an earlier approval of the same one rather than listing it
    /// twice. Both writers read with [`TrustFile::read`], so a file that cannot be read stops the
    /// write rather than replacing every approval in it with this one.
    fn remember(&self, entry: TrustInfo) -> Result<(), String> {
        let mut all = self.read()?;
        all.retain(|t| !(t.dir == entry.dir && t.command == entry.command));
        all.push(entry);
        all.sort_by(|a, b| (&a.dir, &a.command).cmp(&(&b.dir, &b.command)));
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
    /// The artifact's folder: the manifest is read from here, and trust is keyed by it.
    dir: PathBuf,
    /// The pane's data directory: the one place the command is given to write.
    data: PathBuf,
    perms: Arc<Perms>,
    trust: Arc<Mutex<TrustFile>>,
    /// One run at a time per pane, so two shells never write one data directory and an on-demand
    /// run during the on-open one waits rather than racing it.
    running: tokio::sync::Mutex<()>,
    seen: Mutex<Seen>,
}

impl Refresh {
    /// Run the command the manifest declares now, asking the person first unless the pair is
    /// already trusted. The record it returns is the one `status` reports.
    ///
    /// The manifest is read afresh every run, so editing `command` changes what runs — and what is
    /// asked about — rather than running whatever the pane opened with. An error only when there is
    /// nothing to run at all; a command that failed, or that the person refused, is a successful
    /// read of a failed run.
    pub(crate) async fn run(&self) -> Result<RefreshInfo, String> {
        let _one = self.running.lock().await;
        let Manifest { command, every, .. } = crate::artifacts::load(&self.dir);
        let Some(command) = command else {
            return Err(format!(
                "{} declares no refresh command, so there is nothing to run for pane {}",
                self.dir.join("artifact.toml").display(),
                self.pane
            ));
        };
        {
            let mut seen = self.seen.lock().unwrap();
            seen.command = command.clone();
            seen.every = every;
        }
        let started_ms = unix_ms(SystemTime::now());
        let at = Instant::now();
        tracing::info!(pane = %self.pane, scope = self.artifact.scope().name(), name = self.artifact.name(), %command, cwd = %self.cwd, data = %self.data.display(), "refresh start");
        let (exit, error, stderr_tail) = match self.approved(&command).await {
            Ok(true) => self.exec(&command).await,
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
        drop(seen);
        Ok(self.info())
    }

    /// Whether the person allows `command` for this artifact's folder: at once when the pair is
    /// remembered, otherwise by asking, which waits for an answer. "Always" is remembered before
    /// the command runs, so a daemon that dies mid-run does not ask again for what it was told.
    async fn approved(&self, command: &str) -> Result<bool, String> {
        let dir = self.dir.display().to_string();
        if self.trust.lock().unwrap().allows(&dir, command) {
            tracing::info!(pane = %self.pane, %dir, %command, "refresh command already trusted");
            return Ok(true);
        }
        // Quoted, so the person sees the command exactly as the manifest wrote it, and told which
        // artifact and which project directory it would run in.
        let title = format!(
            "Refresh the {} artifact {} in {}: run {command:?}",
            self.artifact.scope().name(),
            self.artifact.name(),
            self.cwd
        );
        let options = [
            ("allow", "Allow once", "allow_once"),
            ("always", "Always allow", "allow_always"),
            ("deny", "Deny", "reject_once"),
        ]
        .map(|(id, name, kind)| PermOption {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
        })
        .to_vec();
        let choice = self.perms.ask(&self.pane, title, options).await?;
        if choice == "always" {
            let entry = TrustInfo {
                dir: dir.clone(),
                command: command.to_string(),
                scope: self.artifact.scope(),
                name: self.artifact.name().to_string(),
                approved_ms: unix_ms(SystemTime::now()),
            };
            match self.trust.lock().unwrap().remember(entry) {
                Ok(()) => {
                    tracing::info!(pane = %self.pane, %dir, %command, "refresh command trusted from now on")
                }
                // The run still happens: the person allowed it. Only the remembering failed, so
                // the next run asks again rather than the allowed run being lost.
                Err(e) => {
                    tracing::warn!(pane = %self.pane, %dir, error = %e, "could not remember the approval; the next run asks again")
                }
            }
        }
        tracing::info!(pane = %self.pane, %choice, "refresh command answered");
        // An allow-list, so an option id this does not know refuses rather than runs: the one place
        // a command gets to run is the two answers that say so.
        Ok(choice == "allow" || choice == "always")
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
        let output = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .env("WHEEL_PANE", &self.pane)
            .env("WHEEL_SPACE_DIR", &self.cwd)
            .env("WHEEL_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A pane that closes mid-run aborts the task holding the child; without this the
            // command would keep writing a directory nothing serves.
            .kill_on_drop(true)
            .output();
        // A command that never ends would hold this pane's runner forever, so every later refresh
        // of that pane would wait behind it rather than reading fresh data. Dropping the future
        // kills the child, so the limit ends the command as well as the wait.
        let output = match tokio::time::timeout(RUN_LIMIT, output).await {
            Ok(output) => output,
            Err(_) => {
                tracing::warn!(pane = %self.pane, %command, limit_secs = RUN_LIMIT.as_secs(), "refresh command stopped at its time limit");
                return (
                    None,
                    Some(format!(
                        "the command ran longer than {}s and was stopped",
                        RUN_LIMIT.as_secs()
                    )),
                    String::new(),
                );
            }
        };
        let output = match output {
            Ok(output) => output,
            Err(e) => return (None, Some(format!("run {command:?}: {e}")), String::new()),
        };
        let stderr = tail(&output.stderr);
        tracing::info!(pane = %self.pane, stdout_bytes = output.stdout.len(), stderr_bytes = output.stderr.len(), status = %output.status, "refresh command finished");
        match output.status.code() {
            Some(code) => (Some(code), None, stderr),
            // Killed by a signal: there is no exit status to report, so the reason goes in `error`
            // rather than being reported as some number the shell never returned.
            None => {
                let signal = std::os::unix::process::ExitStatusExt::signal(&output.status);
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
    pub(crate) fn info(&self) -> RefreshInfo {
        let seen = self.seen.lock().unwrap();
        RefreshInfo {
            pane: self.pane.clone(),
            scope: self.artifact.scope(),
            name: self.artifact.name().to_string(),
            dir: self.dir.display().to_string(),
            cwd: self.cwd.clone(),
            data_dir: self.data.display().to_string(),
            command: seen.command.clone(),
            every: seen.every,
            runs: seen.runs,
            last: seen.last.clone(),
        }
    }
}

/// A pane's runner and every task running for it: the run the pane's opening asks for, each run a
/// caller asks for, and the timer in a later slice. Dropping it stops all of them, so a pane that
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
    /// opening. Replaces any earlier runner of that pane. An artifact whose manifest declares no
    /// command registers nothing, so nothing to run is nothing to report. Call from inside the
    /// tokio runtime.
    pub(crate) fn start_refresh(
        &self,
        pane: &str,
        artifact: &ArtifactRef,
        cwd: &str,
        data: PathBuf,
    ) {
        // The folder the pane serves from, and nothing else: a root or a folder a symlink moved out
        // of the directory its scope belongs to serves nothing and is written to by nothing, so its
        // manifest is not read here either and its command never runs.
        let Some(dir) = self.artifact_dir_inside(artifact, cwd) else {
            tracing::warn!(%pane, scope = artifact.scope().name(), name = artifact.name(), "the artifact's folder is not inside its scope's root, or is not there; nothing refreshes this pane");
            self.stop_refresh(pane);
            return;
        };
        let Manifest { command, every, .. } = crate::artifacts::load(&dir);
        let Some(command) = command else {
            self.stop_refresh(pane);
            return;
        };
        let refresh = Arc::new(Refresh {
            pane: pane.to_string(),
            artifact: artifact.clone(),
            cwd: cwd.to_string(),
            dir,
            data,
            perms: self.perms.clone(),
            trust: self.trust.clone(),
            running: tokio::sync::Mutex::new(()),
            seen: Mutex::new(Seen {
                command: command.clone(),
                every,
                runs: 0,
                last: None,
            }),
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
    pub(crate) async fn refresh_pane(&self, pane: &str) -> Result<RefreshInfo, String> {
        let task = {
            let mut refreshers = self.refreshers.lock().unwrap();
            let refresher = refreshers.get_mut(pane).ok_or_else(|| {
                format!("pane {pane} has no refresh: it shows no artifact, or that artifact's artifact.toml declares no command")
            })?;
            let refresh = refresher.refresh.clone();
            let task = tokio::spawn(async move { refresh.run().await });
            refresher.owns(task.abort_handle());
            task
        };
        match task.await {
            Ok(result) => result,
            Err(e) if e.is_cancelled() => Err(format!(
                "pane {pane} stopped showing its artifact, so its refresh was cancelled"
            )),
            Err(e) => Err(format!("pane {pane}'s refresh could not be run: {e}")),
        }
    }

    /// Every refreshing pane's runner, sorted by pane.
    pub(crate) fn refreshes(&self) -> Vec<RefreshInfo> {
        let mut all: Vec<RefreshInfo> = self
            .refreshers
            .lock()
            .unwrap()
            .values()
            .map(|r| r.refresh.info())
            .collect();
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

    fn entry(dir: &str, command: &str, name: &str) -> TrustInfo {
        TrustInfo {
            dir: dir.into(),
            command: command.into(),
            scope: ArtifactScope::Shared,
            name: name.into(),
            approved_ms: 1,
        }
    }

    /// What the trust file answers: the pair as approved, nothing else, and an approval that can be
    /// withdrawn. The pair is the folder *and* the command, so editing the command asks again.
    #[test]
    fn trust_is_keyed_by_the_folder_and_the_command() {
        let instance = folder("pairs");
        let trust = TrustFile::new(&instance);
        assert!(trust.load().is_empty(), "nothing is trusted to start with");
        assert!(!trust.allows("/a/dash", "bd ready"));

        trust
            .remember(entry("/a/dash", "bd ready", "dash"))
            .unwrap();
        assert!(trust.allows("/a/dash", "bd ready"));
        assert!(
            !trust.allows("/a/dash", "bd ready --json"),
            "the command changed in the manifest"
        );
        assert!(
            !trust.allows("/b/dash", "bd ready"),
            "the same command in another artifact's folder"
        );

        // Approving the same pair again leaves one row; another command for the same folder is its
        // own row, so a manifest that goes back to an approved command does not ask again.
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
        assert!(!trust.allows("/a/dash", "bd ready"));
        assert!(trust.allows("/c/viewer", "epg pull"));
        assert!(
            trust.forget("dash").is_err(),
            "nothing is remembered for it now"
        );

        // A file nothing can read trusts nothing, rather than everything — and a write refuses
        // rather than replacing approvals it could not see with the one it was given.
        let damaged = "{ not json";
        std::fs::write(instance.join(TRUST_FILE), damaged).unwrap();
        assert!(trust.load().is_empty());
        assert!(!trust.allows("/c/viewer", "epg pull"));
        assert!(trust.remember(entry("/d/new", "x", "new")).is_err());
        assert!(trust.forget("viewer").is_err());
        assert_eq!(
            std::fs::read_to_string(instance.join(TRUST_FILE)).unwrap(),
            damaged,
            "a file this cannot read is left alone"
        );
        std::fs::remove_dir_all(&instance).unwrap();
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
