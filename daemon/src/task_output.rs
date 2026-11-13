//! The output of an agent's running background tasks: the file Claude Code names for a task,
//! watched for writes, or the deltas Codex streams, held in memory. It fills each task's last
//! lines, size and time of last output for the footer, and reads the whole recent output for
//! `task_output`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use notify::{RecursiveMode, Watcher};
use wheel_proto::BackgroundTask;

/// Lines of output a task shows in the footer.
pub const TAIL_LINES: usize = 8;
/// The most output `task_output` returns, and the most a streamed task keeps.
pub const KEEP_BYTES: usize = 256 * 1024;
/// How far back a tail read reaches for its lines before it gives up looking for more.
const TAIL_REACH: u64 = 64 * 1024;

/// Output held in memory for a task whose agent streams it.
#[derive(Default)]
struct Streamed {
    text: String,
    bytes: u64,
    last_ms: Option<u64>,
    truncated: bool,
}

/// Every running task's output for one agent.
#[derive(Default)]
pub struct TaskOutputs {
    streamed: HashMap<String, Streamed>,
    /// The directories watched for the named output files, each with its watcher.
    watched: HashMap<PathBuf, notify::RecommendedWatcher>,
}

impl TaskOutputs {
    /// Match the watches and buffers to the running tasks: a directory holding a task's output
    /// file is watched, and whatever no running task needs is dropped. `changed` is called from the
    /// watcher's thread on every write in a watched directory.
    pub fn sync(&mut self, tasks: &[BackgroundTask], changed: impl Fn() + Clone + Send + 'static) {
        self.streamed.retain(|id, _| tasks.iter().any(|t| &t.id == id));
        let dirs: Vec<PathBuf> = tasks
            .iter()
            .filter_map(|t| t.output_path.as_deref())
            .filter_map(|p| Path::new(p).parent().map(Path::to_path_buf))
            .collect();
        self.watched.retain(|dir, _| dirs.contains(dir));
        for dir in dirs {
            if self.watched.contains_key(&dir) {
                continue;
            }
            let ping = changed.clone();
            let made = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    ping();
                }
            });
            match made {
                Ok(mut watcher) => match watcher.watch(&dir, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        tracing::info!(dir = %dir.display(), "watching task output");
                        self.watched.insert(dir, watcher);
                    }
                    Err(e) => tracing::warn!(dir = %dir.display(), error = %e, "cannot watch task output"),
                },
                Err(e) => tracing::warn!(error = %e, "cannot make a task output watcher"),
            }
        }
    }

    /// Take output the agent streamed for task `id`.
    pub fn append(&mut self, id: &str, chunk: &str) {
        let held = self.streamed.entry(id.to_string()).or_default();
        held.text.push_str(chunk);
        held.bytes += chunk.len() as u64;
        held.last_ms = Some(now_ms());
        if held.text.len() > KEEP_BYTES {
            let mut cut = held.text.len() - KEEP_BYTES;
            while !held.text.is_char_boundary(cut) {
                cut += 1;
            }
            held.text.drain(..cut);
            held.truncated = true;
        }
    }

    /// Fill each task's last lines, size, time of last output and read error from its output.
    pub fn fill(&self, tasks: &mut [BackgroundTask]) {
        for task in tasks {
            if let Some(held) = self.streamed.get(&task.id) {
                task.tail = last_lines(&held.text, TAIL_LINES);
                task.output_bytes = held.bytes;
                task.last_output_ms = held.last_ms;
                continue;
            }
            let Some(path) = task.output_path.as_deref() else { continue };
            match tail_file(Path::new(path)) {
                Ok(Some((tail, bytes, last_ms))) => {
                    task.tail = tail;
                    task.output_bytes = bytes;
                    task.last_output_ms = last_ms;
                    task.output_error = None;
                }
                // Not written yet: the agent makes the file when the task first prints.
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(task = %task.id, path, error = %e, "task output unreadable");
                    task.output_error = Some(format!("cannot read {path}: {e}"));
                }
            }
        }
    }

    /// The task's recent output, its last `KEEP_BYTES`, and whether earlier output was left out.
    pub fn read(&self, task: &BackgroundTask) -> Result<(String, bool), String> {
        if let Some(held) = self.streamed.get(&task.id) {
            return Ok((clean(&held.text), held.truncated));
        }
        let path = task.output_path.as_deref().ok_or("the agent named no output file for this task")?;
        let (raw, truncated) = match read_end(Path::new(path), KEEP_BYTES as u64) {
            Ok(read) => read,
            // Not written yet: the agent makes the file when the task first prints.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => return Err(format!("cannot read {path}: {e}")),
        };
        Ok((clean(&raw), truncated))
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The file's last lines, size and time of last write; none while it does not exist.
fn tail_file(path: &Path) -> std::io::Result<Option<(Vec<String>, u64, Option<u64>)>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let bytes = meta.len();
    let last_ms = (bytes > 0)
        .then(|| meta.modified().ok())
        .flatten()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    let (raw, _) = read_end(path, TAIL_REACH)?;
    Ok(Some((last_lines(&raw, TAIL_LINES), bytes, last_ms)))
}

/// The last `reach` bytes of the file, read by seeking from its end, never the whole file, and
/// whether anything before them was left out.
fn read_end(path: &Path, reach: u64) -> std::io::Result<(String, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(reach);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.take(reach).read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // The first line was cut where the read began.
        if let Some(at) = text.find('\n') {
            text.drain(..=at);
        }
    }
    Ok((text, start > 0))
}

/// The output as it reads on a terminal: escape sequences dropped, and a carriage return
/// starting the line over, so a progress bar shows only its latest state.
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut line = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: ESC [ params final; OSC: ESC ] ... BEL or ESC \; a character set: ESC ( B
                // and its kin; others: one char. A line break after ESC stays.
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        chars.next();
                        while let Some(c) = chars.next() {
                            if c == '\u{7}' {
                                break;
                            }
                            if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    Some('(' | ')' | '*' | '+') => {
                        chars.next();
                        chars.next();
                    }
                    Some('\n') | None => {}
                    Some(_) => {
                        chars.next();
                    }
                }
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => line.clear(),
            '\n' => {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            c if c.is_control() && c != '\t' => {}
            c => line.push(c),
        }
    }
    out.push_str(&line);
    out
}

/// The last `n` lines with text, oldest first; a final line with no newline yet counts.
fn last_lines(raw: &str, n: usize) -> Vec<String> {
    let text = clean(raw);
    let mut lines: Vec<String> = text.lines().rev().filter(|l| !l.trim().is_empty()).take(n).map(String::from).collect();
    lines.reverse();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_go_and_a_carriage_return_keeps_the_latest_progress() {
        let raw = "\u{1b}[32mok\u{1b}[0m one\nfetch 10%\rfetch 55%\rfetch 100%\npartial";
        assert_eq!(clean(raw), "ok one\nfetch 100%\npartial");
        assert_eq!(last_lines(raw, 2), vec!["fetch 100%", "partial"]);
        assert_eq!(clean("\u{1b}(Bplain\u{1b}\nnext"), "plain\nnext");
    }

    #[test]
    fn a_tail_reads_only_the_end_and_names_size_and_time() {
        let dir = std::env::temp_dir().join(format!("wheel-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.output");
        let body: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let (tail, bytes, last) = tail_file(&path).unwrap().unwrap();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail.last().unwrap(), "line 19999");
        assert_eq!(bytes, body.len() as u64);
        assert!(last.is_some());
        let (text, truncated) = read_end(&path, 100).unwrap();
        assert!(truncated);
        assert!(text.ends_with("line 19999\n"));
        assert!(text.starts_with("line "), "the cut first line is dropped: {text:?}");
        assert_eq!(tail_file(&dir.join("missing")).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_streamed_task_keeps_its_last_bytes_and_fills_its_tail() {
        let mut outputs = TaskOutputs::default();
        let mut tasks = vec![BackgroundTask { id: "i1".into(), ..Default::default() }];
        outputs.append("i1", "a\nb\n");
        outputs.append("i1", "c");
        outputs.fill(&mut tasks);
        assert_eq!(tasks[0].tail, vec!["a", "b", "c"]);
        assert_eq!(tasks[0].output_bytes, 5);
        assert!(tasks[0].last_output_ms.is_some());
        outputs.append("i1", &"x".repeat(KEEP_BYTES));
        let (text, truncated) = outputs.read(&tasks[0]).unwrap();
        assert!(truncated);
        assert_eq!(text.len(), KEEP_BYTES);
    }

    #[test]
    fn an_unreadable_output_file_is_said_on_the_task() {
        let outputs = TaskOutputs::default();
        let dir = std::env::temp_dir().join(format!("wheel-tail-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A directory where the file should be: it exists and cannot be read as one.
        let mut tasks = vec![BackgroundTask { id: "t".into(), output_path: Some(dir.display().to_string()), ..Default::default() }];
        outputs.fill(&mut tasks);
        assert!(tasks[0].output_error.as_deref().unwrap().starts_with("cannot read"), "{:?}", tasks[0].output_error);
        let missing = BackgroundTask { id: "m".into(), output_path: Some(dir.join("not-yet").display().to_string()), ..Default::default() };
        assert_eq!(outputs.read(&missing).unwrap(), (String::new(), false));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
