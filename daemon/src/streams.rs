//! Merged streams (ADR-0030): a `[streams.<group>]` table in a `wheel.toml` names a group's
//! sources; wheeld runs a group when asked and owns it until it is stopped, its space closes or
//! wheeld exits. Each source's lines are stamped with the time they arrived, held for
//! [`HOLD_MS`], emitted in order, appended to a file under the temp directory and kept in memory
//! for `wheel stream tail`, `wheel status` and the streams page.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::sync::{broadcast, mpsc, Notify};
use wheel_proto::{
    rfc3339_ms, unix_ms_now, SettingOrigin, StreamGroupInfo, StreamLine, StreamSourceInfo,
    StreamStatus,
};

use crate::server::AppState;
use crate::settings::FILE;
use crate::stream_time;

/// How long wheeld holds a line after it arrives before emitting it, so a line stated earlier
/// but delivered later can still be placed before it.
pub(crate) const HOLD_MS: u64 = 2000;

/// How many emitted lines a running group keeps in memory.
pub(crate) const KEEP: usize = 5000;

/// How long the readers of an exited source get to finish what its pipes still hold.
const DRAIN: Duration = Duration::from_millis(300);

const COMMAND: &str = "command";
const RUN: &str = "run";

/// The groups one `wheel.toml` names, by group name. A group that is not a table is skipped and
/// logged; a source that is wrong is kept, with the problem set, so the listing can report it.
pub(crate) fn groups(path: &Path) -> BTreeMap<String, Vec<StreamSourceInfo>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml is not valid TOML; it names no streams");
            return BTreeMap::new();
        }
    };
    let Some(streams) = table.get("streams").and_then(|s| s.as_table()) else {
        return BTreeMap::new();
    };
    let mut found = BTreeMap::new();
    for (group, value) in streams {
        let Some(sources) = value.as_table() else {
            tracing::warn!(path = %path.display(), %group, kind = value.type_str(), "a [streams] entry is not a table of sources; it is ignored");
            continue;
        };
        let sources: Vec<StreamSourceInfo> = sources
            .iter()
            .map(|(name, value)| source(name, value))
            .collect();
        for s in sources.iter().filter(|s| s.problem.is_some()) {
            tracing::warn!(path = %path.display(), %group, source = %s.name, problem = ?s.problem, "a stream source has a problem");
        }
        found.insert(group.clone(), sources);
    }
    found
}

/// One source entry: a command string, or a table with exactly one of `command` / `run` and an
/// optional `tz`.
fn source(name: &str, value: &toml::Value) -> StreamSourceInfo {
    let mut info = StreamSourceInfo {
        name: name.to_string(),
        kind: COMMAND.into(),
        command: String::new(),
        tz: None,
        problem: None,
    };
    fn text(v: &toml::Value) -> Option<&str> {
        v.as_str().map(str::trim).filter(|s| !s.is_empty())
    }
    match value {
        toml::Value::String(_) => match text(value) {
            Some(command) => info.command = command.to_string(),
            None => info.problem = Some("the command is empty".into()),
        },
        toml::Value::Table(t) => {
            if let Some(key) = t
                .keys()
                .find(|k| !["command", "run", "tz"].contains(&k.as_str()))
            {
                info.problem = Some(format!(
                    "unknown key {key:?}; a source takes command or run, and tz"
                ));
                return info;
            }
            let (command, run) = (t.get("command"), t.get("run"));
            let (kind, given) = match (command, run) {
                (Some(c), None) => (COMMAND, c),
                (None, Some(r)) => (RUN, r),
                (Some(_), Some(_)) => {
                    info.problem = Some("a source takes command or run, not both".into());
                    return info;
                }
                (None, None) => {
                    info.problem = Some("a source needs command or run".into());
                    return info;
                }
            };
            info.kind = kind.into();
            match text(given) {
                Some(command) => info.command = command.to_string(),
                None => info.problem = Some(format!("{kind} must be a non-empty string")),
            }
            if let Some(tz) = t.get("tz") {
                match tz.as_str() {
                    Some(zone) if jiff::tz::TimeZone::get(zone).is_ok() => {
                        info.tz = Some(zone.to_string());
                    }
                    Some(zone) => {
                        info.problem
                            .get_or_insert(format!("tz = {zone:?} is not a known time zone"));
                    }
                    None => {
                        info.problem
                            .get_or_insert("tz must be a time zone name".into());
                    }
                }
            }
        }
        other => {
            info.problem = Some(format!(
                "a source is a command string or a table, not a {}",
                other.type_str()
            ));
        }
    }
    info
}

/// Holds lines for [`HOLD_MS`] and hands them out in order. Pure: every call is told the time.
#[derive(Default)]
pub(crate) struct Merger {
    pending: Vec<(u64, StreamLine)>,
    seq: u64,
}

impl Merger {
    pub(crate) fn push(&mut self, line: StreamLine) {
        self.seq += 1;
        self.pending.push((self.seq, line));
    }

    /// When the earliest held line is due, in Unix milliseconds.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.pending.iter().map(|(_, l)| l.arrived + HOLD_MS).min()
    }

    /// Every line held for the full [`HOLD_MS`] by `now`, ordered by the time it is shown by
    /// ([`StreamLine::time`]); ties keep arrival order.
    pub(crate) fn due(&mut self, now: u64) -> Vec<StreamLine> {
        self.take(|l| l.arrived + HOLD_MS <= now)
    }

    /// Every held line, in order.
    pub(crate) fn flush(&mut self) -> Vec<StreamLine> {
        self.take(|_| true)
    }

    fn take(&mut self, ready: impl Fn(&StreamLine) -> bool) -> Vec<StreamLine> {
        let (mut out, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, l)| ready(l));
        self.pending = keep;
        out.sort_by_key(|(seq, l)| (l.time(), *seq));
        out.into_iter().map(|(_, l)| l).collect()
    }
}

/// What a following tail hears.
#[derive(Clone)]
pub(crate) enum Event {
    Line(StreamLine),
    Ended,
}

/// The lines emitted so far, each with the Unix ms it was emitted at, under the lock that also
/// orders subscriptions after them.
struct Held {
    lines: VecDeque<(u64, StreamLine)>,
    emitted: u64,
}

/// One started group, running or stopped.
pub(crate) struct Running {
    space: String,
    group: String,
    started_at: u64,
    file: PathBuf,
    sources: Vec<String>,
    held: Mutex<Held>,
    events: broadcast::Sender<Event>,
    running: AtomicBool,
    /// Each source's process-group id, for the kill.
    groups: Mutex<Vec<(String, i32)>>,
    stop: Notify,
    /// Lines emitted; mirrors `held.emitted` for readers that take no lock.
    count: AtomicU64,
}

impl Running {
    pub(crate) fn file(&self) -> &Path {
        &self.file
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub(crate) fn status(&self) -> StreamStatus {
        StreamStatus {
            space: self.space.clone(),
            group: self.group.clone(),
            running: self.is_running(),
            started_at: self.started_at,
            sources: self.sources.clone(),
            lines: self.count.load(Ordering::SeqCst),
            file: self.file.display().to_string(),
        }
    }

    /// The held lines emitted at or after `since` (Unix ms). The window counts emission, not
    /// arrival, because every line is held [`HOLD_MS`] before it is emitted. With `follow`, also
    /// a receiver of every later line, subscribed under the lock the emitter holds so no line
    /// falls between the two.
    pub(crate) fn tail(
        &self,
        since: Option<u64>,
        follow: bool,
    ) -> (Vec<StreamLine>, Option<broadcast::Receiver<Event>>) {
        let held = self.held.lock().unwrap();
        let lines = held
            .lines
            .iter()
            .filter(|(emitted, _)| since.is_none_or(|t| *emitted >= t))
            .map(|(_, l)| l.clone())
            .collect();
        let rx = (follow && self.is_running()).then(|| self.events.subscribe());
        (lines, rx)
    }

    /// Kill every source's process group, end the pipeline and tell followers. A second call does
    /// nothing.
    fn stop(&self, why: &str) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        let groups = self.groups.lock().unwrap().clone();
        for (source, group) in &groups {
            // SAFETY: killpg only signals the group wheeld made for that source.
            let rc = unsafe { libc::killpg(*group, libc::SIGKILL) };
            tracing::info!(space = %self.space, group = %self.group, %source, pgid = group, rc, %why, "stream source killed");
        }
        // The merger flushes what it holds, then ends the stream.
        self.stop.notify_one();
        tracing::info!(space = %self.space, group = %self.group, %why, pgids = ?groups.iter().map(|(_, g)| *g).collect::<Vec<_>>(), "stream stopped");
    }

    /// Add `lines` to the emitted lines, the file and the followers. Called by the merger only.
    fn emit(&self, lines: Vec<StreamLine>, file: &mut std::fs::File) {
        if lines.is_empty() {
            return;
        }
        let mut held = self.held.lock().unwrap();
        for line in lines {
            if let Err(e) = writeln!(file, "{}", line.plain()) {
                tracing::warn!(file = %self.file.display(), error = %e, "a stream line was not written to its file");
            }
            held.lines.push_back((unix_ms_now(), line.clone()));
            if held.lines.len() > KEEP {
                held.lines.pop_front();
            }
            held.emitted += 1;
            let _ = self.events.send(Event::Line(line));
        }
        self.count.store(held.emitted, Ordering::SeqCst);
        let _ = file.flush();
    }
}

/// The merging task: take lines as sources produce them, hold each [`HOLD_MS`], emit in order.
async fn merge(
    run: Arc<Running>,
    mut rx: mpsc::UnboundedReceiver<StreamLine>,
    mut file: std::fs::File,
) {
    let mut merger = Merger::default();
    let mut open = true;
    loop {
        let wait = merger
            .next_due()
            .map(|due| Duration::from_millis(due.saturating_sub(unix_ms_now())));
        let mut flush = false;
        tokio::select! {
            line = rx.recv(), if open => match line {
                Some(line) => merger.push(line),
                None => open = false,
            },
            _ = async { tokio::time::sleep(wait.unwrap_or_default()).await }, if wait.is_some() => {}
            _ = run.stop.notified() => {
                while let Ok(line) = rx.try_recv() {
                    merger.push(line);
                }
                flush = true;
            }
        }
        let lines = if flush {
            merger.flush()
        } else {
            merger.due(unix_ms_now())
        };
        run.emit(lines, &mut file);
        if flush {
            let _ = run.events.send(Event::Ended);
            return;
        }
        if !open && merger.next_due().is_none() {
            // Every source ended and everything is out; the group stays "running" until stopped
            // so a start finds it and a stop ends it.
            run.stop.notified().await;
            let _ = run.events.send(Event::Ended);
            return;
        }
    }
}

/// Read `pipe` as lines into `tx`, each stamped with its arrival and the time it states.
async fn read_lines(
    pipe: impl AsyncRead + Unpin,
    source: String,
    tz: Option<String>,
    tx: mpsc::UnboundedSender<StreamLine>,
) {
    let local = stream_time::local_zone();
    let mut reader = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim_end_matches(['\n', '\r']).to_string();
                let arrived = unix_ms_now();
                let line = StreamLine {
                    arrived,
                    occurred: stream_time::stated_time(&text, tz.as_deref(), arrived, &local),
                    source: source.clone(),
                    text,
                };
                if tx.send(line).is_err() {
                    return;
                }
            }
        }
    }
}

/// Every running or stopped group, by space and group.
#[derive(Default)]
pub(crate) struct Streams {
    all: Mutex<BTreeMap<(String, String), Arc<Running>>>,
}

impl Streams {
    pub(crate) fn get(&self, space: &str, group: &str) -> Option<Arc<Running>> {
        self.all
            .lock()
            .unwrap()
            .get(&(space.to_string(), group.to_string()))
            .cloned()
    }

    pub(crate) fn statuses(&self) -> Vec<StreamStatus> {
        self.all
            .lock()
            .unwrap()
            .values()
            .map(|r| r.status())
            .collect()
    }

    /// Stop every group of `space` and forget them.
    pub(crate) fn close_space(&self, space: &str) {
        let mut all = self.all.lock().unwrap();
        all.retain(|(s, _), run| {
            if s == space {
                run.stop("space closed");
                return false;
            }
            true
        });
    }

    /// Stop every running group; wheeld is exiting.
    pub(crate) fn stop_all(&self) {
        for run in self.all.lock().unwrap().values() {
            run.stop("wheeld exiting");
        }
    }
}

/// `YYYYMMDDTHHMMSSZ` for Unix milliseconds.
fn file_stamp(ms: u64) -> String {
    let stamp = jiff::Timestamp::from_millisecond(ms as i64).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    stamp.strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// A space name as one folder name.
fn folder_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\0' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    match clean.trim() {
        "" | "." | ".." => "_".into(),
        t => t.to_string(),
    }
}

impl AppState {
    /// Kill every running stream's sources; wheeld is exiting.
    pub fn stop_streams(&self) {
        self.streams.stop_all();
    }

    /// The groups of `space`'s `[streams]`: the project's over this machine's, group by group.
    pub(crate) fn stream_ls(&self, space: Option<&str>) -> Result<Vec<StreamGroupInfo>, String> {
        let space_id = self.settings_space_id(space)?;
        let cwd = self.stream_space(&space_id)?.1;
        let mut found: BTreeMap<String, (SettingOrigin, Vec<StreamSourceInfo>)> = BTreeMap::new();
        for (path, origin) in [
            (self.machine_file(), SettingOrigin::Machine),
            (Path::new(&cwd).join(FILE), SettingOrigin::Project),
        ] {
            for (group, sources) in groups(&path) {
                found.insert(group, (origin, sources));
            }
        }
        Ok(found
            .into_iter()
            .map(|(group, (origin, sources))| {
                let run = self
                    .streams
                    .get(&space_id, &group)
                    .filter(|r| r.is_running());
                StreamGroupInfo {
                    group,
                    origin,
                    sources,
                    running: run.is_some(),
                    file: run.map(|r| r.file().display().to_string()),
                }
            })
            .collect())
    }

    /// The name and directory of space `id`.
    fn stream_space(&self, id: &str) -> Result<(String, String), String> {
        self.spaces
            .lock()
            .unwrap()
            .spaces
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.name.clone(), s.cwd.clone()))
            .ok_or(format!("no space {id}"))
    }

    /// Start `group` in `space`, or return the instance already running.
    pub(crate) async fn stream_start(
        self: &Arc<Self>,
        group: &str,
        space: Option<&str>,
    ) -> Result<StreamStatus, String> {
        let space_id = self.settings_space_id(space)?;
        if let Some(run) = self
            .streams
            .get(&space_id, group)
            .filter(|r| r.is_running())
        {
            return Ok(run.status());
        }
        let listed = self.stream_ls(Some(&space_id))?;
        let info = listed
            .into_iter()
            .find(|g| g.group == group)
            .ok_or(format!("no group {group} in [streams]"))?;
        let problems: Vec<String> = info
            .sources
            .iter()
            .filter_map(|s| s.problem.as_ref().map(|p| format!("{}: {p}", s.name)))
            .collect();
        if !problems.is_empty() {
            return Err(format!(
                "group {group} has problems: {}",
                problems.join("; ")
            ));
        }
        if let Some(s) = info.sources.iter().find(|s| s.kind == RUN) {
            return Err(format!("{}: run sources are not supported yet", s.name));
        }
        if info.sources.is_empty() {
            return Err(format!("group {group} has no sources"));
        }
        let (space_name, cwd) = self.stream_space(&space_id)?;
        let started_at = unix_ms_now();
        let dir = std::env::temp_dir()
            .join("wheel-streams")
            .join(folder_name(&space_name))
            .join(folder_name(group));
        std::fs::create_dir_all(&dir).map_err(|e| format!("make {}: {e}", dir.display()))?;
        let file_path = dir.join(format!("{}.log", file_stamp(started_at)));
        let file = std::fs::File::options()
            .create(true)
            .append(true)
            .open(&file_path)
            .map_err(|e| format!("open {}: {e}", file_path.display()))?;

        let (events, _) = broadcast::channel(1024);
        let run = Arc::new(Running {
            space: space_id.clone(),
            group: group.to_string(),
            started_at,
            file: file_path.clone(),
            sources: info.sources.iter().map(|s| s.name.clone()).collect(),
            held: Mutex::new(Held {
                lines: VecDeque::new(),
                emitted: 0,
            }),
            events,
            running: AtomicBool::new(true),
            groups: Mutex::new(Vec::new()),
            stop: Notify::new(),
            count: AtomicU64::new(0),
        });
        // Checked and inserted under one lock, so two starts of a group never both spawn.
        {
            let mut all = self.streams.all.lock().unwrap();
            let key = (space_id.clone(), group.to_string());
            if let Some(running) = all.get(&key).filter(|r| r.is_running()) {
                return Ok(running.status());
            }
            all.insert(key, run.clone());
        }
        let forget = |run: &Running| {
            run.stop("a source did not start");
            let mut all = self.streams.all.lock().unwrap();
            if all
                .get(&(run.space.clone(), run.group.clone()))
                .is_some_and(|r| std::ptr::eq(&**r, run))
            {
                all.remove(&(run.space.clone(), run.group.clone()));
            }
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let mut tasks = vec![tokio::spawn(merge(run.clone(), rx, file))];
        let env = crate::shell::command_env(
            self.control.lock().unwrap().as_ref(),
            &self.instance_dir,
            &self.path(),
        );
        for s in &info.sources {
            let mut command = tokio::process::Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(&s.command)
                .current_dir(&cwd)
                .envs(env.iter().cloned())
                .env("WHEEL_SPACE", &space_id)
                .env("WHEEL_SPACE_DIR", &cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .kill_on_drop(true);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => {
                    forget(&run);
                    for t in tasks {
                        t.abort();
                    }
                    return Err(format!("start {}: {e}", s.name));
                }
            };
            let Some(pid) = child.id().map(|p| p as i32).filter(|p| *p > 0) else {
                forget(&run);
                for t in tasks {
                    t.abort();
                }
                return Err(format!("start {}: the process has no id", s.name));
            };
            run.groups.lock().unwrap().push((s.name.clone(), pid));
            tracing::info!(space = %space_id, %group, source = %s.name, command = %s.command, tz = ?s.tz, pid, file = %file_path.display(), "stream source started");
            let (out, err) = (child.stdout.take(), child.stderr.take());
            let (name, tz, tx) = (s.name.clone(), s.tz.clone(), tx.clone());
            let (space_log, group_log) = (space_id.clone(), group.to_string());
            let run_exit = run.clone();
            tasks.push(tokio::spawn(async move {
                let readers = [
                    out.map(|p| tokio::spawn(read_lines(p, name.clone(), tz.clone(), tx.clone()))),
                    err.map(|p| tokio::spawn(read_lines(p, name.clone(), tz.clone(), tx.clone()))),
                ];
                let status = child.wait().await;
                for reader in readers.into_iter().flatten() {
                    let _ = tokio::time::timeout(DRAIN, reader).await;
                }
                // Reaped: the group id may be reused, so it is never signalled again.
                run_exit.groups.lock().unwrap().retain(|(_, g)| *g != pid);
                let code = match &status {
                    Ok(s) => s.code().map(|c| c.to_string()).or_else(|| {
                        s.signal().map(|n| format!("signal {n}"))
                    }),
                    Err(e) => Some(format!("error: {e}")),
                }
                .unwrap_or_else(|| "unknown".into());
                tracing::info!(space = %space_log, group = %group_log, source = %name, pid, exit = %code, "stream source exited");
                let _ = tx.send(StreamLine {
                    arrived: unix_ms_now(),
                    occurred: None,
                    source: name,
                    text: format!("exited {code}"),
                });
            }));
        }
        drop(tx);
        tracing::info!(space = %space_id, %group, sources = run.sources.len(), file = %file_path.display(), started = %rfc3339_ms(started_at), "stream started");
        Ok(run.status())
    }

    /// Stop `group` in `space`; refused when it is not running.
    pub(crate) fn stream_stop(&self, group: &str, space: Option<&str>) -> Result<(), String> {
        let space_id = self.settings_space_id(space)?;
        match self
            .streams
            .get(&space_id, group)
            .filter(|r| r.is_running())
        {
            Some(run) => {
                run.stop("stopped");
                Ok(())
            }
            None => Err(format!("group {group} is not running")),
        }
    }

    /// The instance of `group` in `space`, running or stopped.
    pub(crate) fn stream_find(
        &self,
        group: &str,
        space: Option<&str>,
    ) -> Result<Arc<Running>, String> {
        let space_id = self.settings_space_id(space)?;
        self.streams
            .get(&space_id, group)
            .ok_or(format!("group {group} has not been started"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(arrived: u64, occurred: Option<u64>, source: &str) -> StreamLine {
        StreamLine {
            arrived,
            occurred,
            source: source.into(),
            text: String::new(),
        }
    }

    fn parse(toml_text: &str) -> BTreeMap<String, Vec<StreamSourceInfo>> {
        let dir = std::env::temp_dir().join(format!("wheel-streams-test-{}", unix_ms_now()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wheel.toml");
        std::fs::write(&path, toml_text).unwrap();
        let found = groups(&path);
        let _ = std::fs::remove_dir_all(&dir);
        found
    }

    #[test]
    fn a_line_is_held_two_seconds() {
        let mut m = Merger::default();
        m.push(line(1000, None, "a"));
        assert_eq!(m.next_due(), Some(3000));
        assert!(m.due(2999).is_empty());
        assert_eq!(m.due(3000).len(), 1);
        assert_eq!(m.next_due(), None);
    }

    #[test]
    fn lines_due_together_come_out_in_arrival_order() {
        let mut m = Merger::default();
        for (i, s) in ["a", "b", "c"].iter().enumerate() {
            m.push(line(1000 + i as u64, None, s));
        }
        let out: Vec<_> = m.due(10_000).into_iter().map(|l| l.source).collect();
        assert_eq!(out, ["a", "b", "c"]);
    }

    /// The sort key is the stated time else the arrival: a line stated earlier but arriving
    /// within the hold comes out first.
    #[test]
    fn a_line_stated_earlier_comes_out_first_within_the_hold() {
        let mut m = Merger::default();
        m.push(line(1000, None, "late-arrival-first"));
        m.push(line(1500, Some(900), "stated-earlier"));
        let out: Vec<_> = m.due(3500).into_iter().map(|l| l.source).collect();
        assert_eq!(out, ["stated-earlier", "late-arrival-first"]);
    }

    #[test]
    fn a_line_stated_one_second_earlier_arriving_one_and_a_half_later_goes_first() {
        let mut m = Merger::default();
        m.push(line(10_000, Some(10_000), "early"));
        m.push(line(11_500, Some(9_000), "stated-earlier"));
        let out: Vec<_> = m.due(13_500).into_iter().map(|l| l.source).collect();
        assert_eq!(out, ["stated-earlier", "early"]);
    }

    #[test]
    fn a_line_after_the_hold_keeps_arrival_order_behind_emitted_lines() {
        let mut m = Merger::default();
        m.push(line(1000, None, "first"));
        let first = m.due(3000);
        m.push(line(3100, Some(500), "too-late"));
        let second = m.due(5100);
        assert_eq!(first[0].source, "first");
        assert_eq!(second[0].source, "too-late");
    }

    #[test]
    fn a_source_is_a_string_or_a_table_and_anything_else_is_a_problem() {
        let found = parse(
            r#"
[streams.t]
a = "echo a"
b = { command = "echo b", tz = "UTC" }
c = { run = "npm run dev" }
d = { command = "x", run = "y" }
e = { command = "x", colour = "red" }
f = { command = "x", tz = "Mars/Base" }
g = {}
h = 4
"#,
        );
        let s = &found["t"];
        let get = |n: &str| s.iter().find(|x| x.name == n).unwrap();
        assert_eq!(get("a").problem, None);
        assert_eq!(get("a").command, "echo a");
        assert_eq!(
            get("b").tz.as_deref(),
            Some("UTC"),
            "{:?}",
            get("b").problem
        );
        assert_eq!(
            (get("c").kind.as_str(), get("c").problem.clone()),
            ("run", None)
        );
        for bad in ["d", "e", "f", "g", "h"] {
            assert!(get(bad).problem.is_some(), "{bad} should have a problem");
        }
        assert!(get("f").problem.as_ref().unwrap().contains("Mars/Base"));
    }

    #[test]
    fn the_file_stamp_is_utc_compact() {
        assert_eq!(file_stamp(1_793_286_245_123), "20261029T150405Z");
        assert_eq!(rfc3339_ms(1_793_286_245_123), "2026-10-29T15:04:05.123Z");
    }
}
