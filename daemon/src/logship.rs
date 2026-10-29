//! The daemon's log lines, its usage journal and the apps' log lines, kept on this machine and
//! shipped to a Loki push endpoint (`logs.push_url`) whenever one is set and reachable.
//!
//! Three files are the record, and each is also the outbox for its own lines: `wheeld.log` (the
//! daemon's tracing output), `usage.jsonl` (one line per request a client made: what, by whom, on
//! which space and pane, whether it worked and how long it took — never terminal input, prompt
//! text or a token) and `applog.jsonl` (lines the Mac and iOS apps sent over their connection).
//!
//! A shipper reads each file past the position Loki last accepted, posts the lines, and moves the
//! position only on a 2xx, so a Mac that sleeps, or a store that is down, delays lines rather than
//! losing them. The position names the file by inode as well as by offset: a file that rolled over
//! is a different inode, read from its start, and nothing but the shipper ever writes a position.
//! It is synced and renamed into place, so a crash leaves the old position or the new one.
//!
//! Every file rolls to `<name>.1` past [`CAP`] once Loki has all of it, or at twice [`CAP`]
//! regardless, so a store that never answers cannot fill the disk; [`RollingFile`] is that rule,
//! for the files this module writes and for `wheeld.log`, which tracing writes through it.
//!
//! The shipper's own log lines never ship (they are filtered out by their target), so its warnings
//! about a store that refuses lines cannot become more lines to refuse.
//!
//! Labels are few and fixed (`app`, `source`, `host`, `client`, and `kind` for journal lines), so
//! the store indexes a handful of streams; identifiers stay in the line.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use wheel_proto::{AppLogLine, LogShipping};

/// How often the shipper looks for lines to send.
pub(crate) const SHIP_EVERY: Duration = Duration::from_secs(5);
/// A file past this size rolls to `<name>.1` once its lines are delivered.
pub const CAP: u64 = 20 * 1024 * 1024;
/// The most bytes one push carries from one file.
const BATCH: u64 = 512 * 1024;
/// How long one push may take.
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest the shipper waits after failures before trying again, whatever the store asks.
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// The longest app line kept, and the longest error text a journal line carries.
const MAX_TEXT: usize = 8 * 1024;
const MAX_ERR: usize = 200;
/// The apps a `log` message may name; any other name is kept as `other`.
const APPS: &[&str] = &["mac", "iphone", "ipad"];
/// The tracing target of this module's own lines, which never ship.
const OWN_TARGET: &str = "wheeld::logship";

/// The request kinds that are not a person's action: keystrokes, resizes, the apps' status and
/// usage polls, render handshakes. Journaling them would bury the actions under the traffic.
const NOT_ACTIONS: &[&str] = &[
    "status",
    "term_input",
    "term_resize",
    "usage",
    "session_wait",
    "render_offer",
    "rendered",
    "render_refused",
    "artifact_state",
    "log",
];

/// The reports programs send on their own: one per dashboard push, per status-line tick, per
/// agent state change. Journaled only when one fails, so a failing report stays visible.
const REPORTS: &[&str] = &["artifact_data", "pane_report_context", "pane_report"];

/// One request a client made, as the journal records it.
pub(crate) struct Action<'a> {
    pub op: &'a str,
    /// `mac`, `local` (the CLI or another local client) or `device`.
    pub client: &'a str,
    /// The paired device's name, for a `device` client.
    pub device: Option<&'a str>,
    pub space: Option<&'a str>,
    pub pane: Option<&'a str>,
    /// None when the reply goes out later, from a task of its own.
    pub ok: Option<bool>,
    pub ms: u64,
    pub err: Option<&'a str>,
}

#[derive(Clone, Copy)]
enum Source {
    Journal,
    App,
    Wheeld,
}

impl Source {
    const ALL: [Source; 3] = [Source::Journal, Source::App, Source::Wheeld];

    fn name(self) -> &'static str {
        match self {
            Source::Journal => "journal",
            Source::App => "app",
            Source::Wheeld => "wheeld",
        }
    }
}

/// Where Loki's copy of a file ends: the file's inode and the byte offset in it.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Position {
    ino: u64,
    offset: u64,
}

/// The position file for the log file at `file`: `logship/<name>.offset` in `dir`.
pub fn position_path(dir: &Path, file: &Path) -> PathBuf {
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("log");
    dir.join("logship").join(format!("{name}.offset"))
}

fn read_position(path: &Path) -> Option<Position> {
    let text = fs::read_to_string(path).ok()?;
    let mut parts = text.split_whitespace();
    Some(Position {
        ino: parts.next()?.parse().ok()?,
        offset: parts.next()?.parse().ok()?,
    })
}

/// Written beside the target, synced, and renamed over it. False when it could not be saved.
fn write_position(path: &Path, at: Position) -> bool {
    write_atomic(path, &format!("{} {}", at.ino, at.offset))
}

/// `text` written beside `path`, synced, and renamed over it, so a crash leaves the old file or
/// the new one. False when it could not be saved.
fn write_atomic(path: &Path, text: &str) -> bool {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let done = (|| -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = &done {
        tracing::warn!(target: OWN_TARGET, file = %path.display(), error = %e, "cannot save the log shipper's state");
    }
    done.is_ok()
}

/// Where the apps' taken positions are kept: `logship/applog.taken.json` in `dir`.
fn taken_path(dir: &Path) -> PathBuf {
    dir.join("logship").join("applog.taken.json")
}

fn read_taken(path: &Path) -> HashMap<String, (u64, u64)> {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// An append-only log file that rolls to `<name>.1` past [`CAP`] once Loki has all of it (by the
/// position file for it), or at twice [`CAP`] regardless.
pub struct RollingFile {
    path: PathBuf,
    position: PathBuf,
    file: File,
    len: u64,
    /// When to look at the position again while waiting for Loki to catch up.
    next_check: u64,
}

impl RollingFile {
    pub fn open(path: PathBuf, position: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            position,
            file,
            len,
            next_check: CAP,
        })
    }

    fn roll_if_full(&mut self) {
        if self.len < self.next_check {
            return;
        }
        let ino = self.file.metadata().map(|m| m.ino()).unwrap_or(0);
        let delivered =
            read_position(&self.position).is_some_and(|p| p.ino == ino && p.offset >= self.len);
        if !delivered && self.len < 2 * CAP {
            // Look again after another megabyte rather than on every line.
            self.next_check = self.len + 1024 * 1024;
            return;
        }
        let rolled = PathBuf::from(format!("{}.1", self.path.display()));
        let reopened = fs::rename(&self.path, &rolled).and_then(|()| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        });
        match reopened {
            Ok(file) => {
                if !delivered {
                    tracing::warn!(target: OWN_TARGET, file = %self.path.display(), "log file rolled over with lines the store never got; they are in the .1 file");
                }
                self.file = file;
                self.len = 0;
                self.next_check = CAP;
            }
            Err(e) => {
                tracing::warn!(target: OWN_TARGET, file = %self.path.display(), error = %e, "cannot roll a full log file");
                self.next_check = self.len + 1024 * 1024;
            }
        }
    }
}

impl Write for RollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.roll_if_full();
        let n = self.file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Default)]
struct Shipping {
    url: String,
    last_ok_ms: Option<u64>,
    error: Option<String>,
    /// No push before this; set by a failure, cleared by a success.
    wait_until: Option<Instant>,
    backoff: Duration,
}

/// A line for the writer thread to append: the journal's or an app's.
enum Line {
    Journal(Map<String, Value>),
    App(String, AppLogLine),
    /// Answered once every line sent before it is written.
    Drained(mpsc::Sender<()>),
    /// The apps' taken positions, saved once the lines before it are written.
    Taken(HashMap<String, (u64, u64)>),
}

pub(crate) struct Logship {
    dir: PathBuf,
    log_path: PathBuf,
    host: String,
    /// Lines for the writer thread, which owns the journal and app files: appends never run on
    /// the async runtime, and lines land in the order they were sent.
    lines: Mutex<mpsc::Sender<Line>>,
    /// One push at a time: the timer and a test's `ship_now` never send the same lines twice.
    pushing: Mutex<()>,
    shipping: Mutex<Shipping>,
    /// Bytes Loki does not have yet, as the last shipping pass counted them.
    waiting: AtomicU64,
    /// Per app on one connecting device (or `local`), where in its log file the lines already taken
    /// end: its file's inode and offset. Saved to [`taken_path`] after each batch, so a daemon
    /// restart remembers it.
    taken: Mutex<HashMap<String, (u64, u64)>>,
}

impl Logship {
    pub(crate) fn new(instance_dir: &Path, log_path: &str) -> Self {
        let dir = instance_dir.to_path_buf();
        let (tx, rx) = mpsc::channel::<Line>();
        let journal = dir.join("usage.jsonl");
        let applog = dir.join("applog.jsonl");
        let journal_at = position_path(&dir, &journal);
        let applog_at = position_path(&dir, &applog);
        let taken_at = taken_path(&dir);
        let taken = read_taken(&taken_at);
        std::thread::Builder::new()
            .name("logship-writer".into())
            .spawn(move || write_lines(rx, journal, journal_at, applog, applog_at, taken_at))
            .expect("the log writer thread starts");
        Self {
            dir,
            log_path: PathBuf::from(log_path),
            host: hostname(),
            lines: Mutex::new(tx),
            pushing: Mutex::new(()),
            shipping: Mutex::new(Shipping::default()),
            waiting: AtomicU64::new(0),
            taken: Mutex::new(taken),
        }
    }

    fn path(&self, source: Source) -> PathBuf {
        match source {
            Source::Journal => self.dir.join("usage.jsonl"),
            Source::App => self.dir.join("applog.jsonl"),
            Source::Wheeld => self.log_path.clone(),
        }
    }

    fn send(&self, line: Line) {
        let _ = self.lines.lock().unwrap().send(line);
    }

    /// Record one request in the journal, unless it is traffic rather than an action.
    pub(crate) fn journal(&self, action: &Action) {
        if NOT_ACTIONS.contains(&action.op)
            || (REPORTS.contains(&action.op) && action.ok != Some(false))
        {
            return;
        }
        let mut line = Map::new();
        line.insert("op".into(), json!(action.op));
        line.insert("client".into(), json!(action.client));
        for (key, value) in [
            ("device", action.device),
            ("space", action.space),
            ("pane", action.pane),
        ] {
            if let Some(v) = value {
                line.insert(key.into(), json!(v));
            }
        }
        if let Some(ok) = action.ok {
            line.insert("ok".into(), json!(ok));
        }
        line.insert("ms".into(), json!(action.ms));
        // The daemon's own reason, cut short: it can quote a path or a name the request carried.
        if let Some(err) = action.err {
            line.insert("err".into(), json!(cut(err, MAX_ERR)));
        }
        self.send(Line::Journal(line));
    }

    /// Keep lines an app sent. `client` becomes a label, so only a known app's name is kept as
    /// itself. `source` names the device the app runs on (`local` for the Mac app). Lines the app
    /// places in `inode` at or before the offset already taken from it are dropped: the app sends
    /// a batch again when the connection dropped before the `ok`.
    pub(crate) fn app_lines(
        &self,
        client: &str,
        source: &str,
        inode: Option<u64>,
        lines: &[AppLogLine],
    ) {
        let client = if APPS.contains(&client) {
            client
        } else {
            "other"
        };
        let key = format!("{client}:{source}");
        let mut taken = self.taken.lock().unwrap();
        let mut seen = inode.and_then(|i| taken.get(&key).filter(|t| t.0 == i).map(|t| t.1));
        let mut moved = false;
        for l in lines {
            if let (Some(end), Some(seen)) = (l.end, seen) {
                if end <= seen {
                    continue;
                }
            }
            self.send(Line::App(
                client.to_string(),
                AppLogLine {
                    ts_ms: l.ts_ms,
                    msg: cut(&l.msg, MAX_TEXT),
                    end: None,
                },
            ));
            if let (Some(i), Some(end)) = (inode, l.end) {
                taken.insert(key.clone(), (i, end));
                seen = Some(end);
                moved = true;
            }
        }
        if moved {
            self.send(Line::Taken(taken.clone()));
        }
    }

    /// What `status` shows. `waiting_bytes` is as of the last shipping pass, at most
    /// [`SHIP_EVERY`] old.
    pub(crate) fn status(&self, url: &str) -> LogShipping {
        let shipping = self.shipping.lock().unwrap();
        let current = shipping.url == url;
        let state = match (url.is_empty(), current, &shipping.error) {
            (true, _, _) => "off",
            (false, true, Some(_)) => "failing",
            (false, true, None) if shipping.last_ok_ms.is_some() => "ok",
            _ => "waiting",
        };
        LogShipping {
            url: url.to_string(),
            state: state.to_string(),
            last_ok_ms: shipping.last_ok_ms.filter(|_| current),
            waiting_bytes: self.waiting.load(Ordering::Relaxed),
            error: shipping.error.clone().filter(|_| current),
        }
    }

    /// Send everything waiting to `url`, a batch per file at a time. Blocking: run it off the async
    /// runtime's threads. Returns early while a failure's back-off lasts; the first failure's
    /// back-off is `retry`, and each further one doubles it.
    pub(crate) fn ship(&self, url: &str, retry: Duration) {
        let _pushing = self.pushing.lock().unwrap();
        // Every line sent before this pass is on disk before it reads the files.
        let (tx, rx) = mpsc::channel();
        self.send(Line::Drained(tx));
        let _ = rx.recv_timeout(Duration::from_secs(5));
        self.ship_locked(url, retry);
        self.waiting.store(self.count_waiting(), Ordering::Relaxed);
    }

    fn ship_locked(&self, url: &str, retry: Duration) {
        {
            let mut shipping = self.shipping.lock().unwrap();
            if shipping.url != url {
                *shipping = Shipping {
                    url: url.to_string(),
                    ..Shipping::default()
                };
            }
            if url.is_empty() || shipping.wait_until.is_some_and(|t| Instant::now() < t) {
                return;
            }
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(PUSH_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        for source in Source::ALL {
            let at = position_path(&self.dir, &self.path(source));
            while let Some((end, body, lines)) = self.batch(source) {
                let result = if lines == 0 {
                    Ok(())
                } else {
                    push(&agent, url, &body)
                };
                // Delivered, or refused for good: either way these lines are behind us. A
                // position that cannot be saved stops the pass, or the same lines would go again.
                if matches!(result, Ok(()) | Err(Push::Rejected(_))) && !write_position(&at, end) {
                    self.failed(url, "cannot save the shipper's position".into(), None, retry);
                    return;
                }
                match result {
                    Ok(()) => {
                        let mut shipping = self.shipping.lock().unwrap();
                        shipping.last_ok_ms = Some(now_ms());
                        if let Some(was) = shipping.error.take() {
                            tracing::info!(target: OWN_TARGET, %url, previous_error = %was, "log lines reach the store again");
                        }
                        shipping.wait_until = None;
                        shipping.backoff = Duration::ZERO;
                        tracing::debug!(target: OWN_TARGET, source = source.name(), lines, bytes = body.len(), "log lines delivered");
                    }
                    Err(Push::Rejected(why)) => {
                        // The store will never take these lines; skipping them lets the lines
                        // behind them through. Shown as failing until a push succeeds, and paced
                        // like any failure so a store refusing everything is not asked in a loop.
                        self.failed(url, format!("refused {lines} lines: {why}"), None, retry);
                        return;
                    }
                    Err(Push::Retry(why, after)) => {
                        self.failed(url, why, after, retry);
                        return;
                    }
                }
            }
        }
    }

    fn failed(&self, url: &str, why: String, after: Option<Duration>, retry: Duration) {
        let mut shipping = self.shipping.lock().unwrap();
        let backoff = after
            .unwrap_or((shipping.backoff * 2).max(retry))
            .min(MAX_BACKOFF);
        if shipping.error.as_deref() != Some(why.as_str()) {
            tracing::warn!(target: OWN_TARGET, %url, error = %why, retry_in_s = backoff.as_secs(), "cannot ship log lines");
        }
        shipping.error = Some(why);
        shipping.backoff = backoff;
        shipping.wait_until = Some(Instant::now() + backoff);
    }

    /// The file's inode, where Loki's copy of it ends, and its length: from the start when the
    /// file is not the one the position names.
    fn start(&self, source: Source, file: &File) -> Option<(u64, u64, u64)> {
        let meta = file.metadata().ok()?;
        let (ino, len) = (meta.ino(), meta.len());
        let offset = read_position(&position_path(&self.dir, &self.path(source)))
            .filter(|p| p.ino == ino && p.offset <= len)
            .map_or(0, |p| p.offset);
        Some((ino, offset, len))
    }

    fn count_waiting(&self) -> u64 {
        Source::ALL
            .iter()
            .filter_map(|&s| {
                let file = File::open(self.path(s)).ok()?;
                let (_, offset, len) = self.start(s, &file)?;
                Some(len - offset)
            })
            .sum()
    }

    /// The next batch of whole lines from `source` as a Loki push body: the position after them,
    /// the body, and how many lines it holds. None when nothing whole is waiting.
    fn batch(&self, source: Source) -> Option<(Position, String, usize)> {
        let mut file = File::open(self.path(source)).ok()?;
        let (ino, offset, len) = self.start(source, &file)?;
        if offset == len {
            return None;
        }
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = Vec::new();
        (&mut file).take(BATCH).read_to_end(&mut buf).ok()?;
        let whole = match buf.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            // One line longer than a batch: nothing this module writes is that long, so it is
            // skipped whole rather than sent cut in two.
            None if buf.len() as u64 == BATCH => {
                let skip = skip_line(&mut file, offset)?;
                tracing::warn!(target: OWN_TARGET, source = source.name(), bytes = skip - offset, "skipped a log line too long to ship");
                let at = Position { ino, offset: skip };
                return Some((at, String::new(), 0));
            }
            None => return None,
        };
        let text = String::from_utf8_lossy(&buf[..whole]);
        let mut streams: Vec<(Map<String, Value>, Vec<Value>)> = Vec::new();
        let mut last_ns = now_ms() as u128 * 1_000_000;
        let mut count = 0;
        for (seq, raw) in text.lines().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            let (ns, labels, line) = self.entry(source, raw, last_ns);
            last_ns = ns;
            // The shipper's own lines stay on this machine.
            if matches!(source, Source::Wheeld) && raw.contains(OWN_TARGET) {
                continue;
            }
            // A per-line nudge keeps lines from one millisecond distinct and in order.
            let ts = (ns + seq as u128 % 1_000_000).to_string();
            match streams.iter_mut().find(|(l, _)| *l == labels) {
                Some((_, values)) => values.push(json!([ts, line])),
                None => streams.push((labels, vec![json!([ts, line])])),
            }
            count += 1;
        }
        let body = json!({
            "streams": streams
                .into_iter()
                .map(|(stream, values)| json!({ "stream": stream, "values": values }))
                .collect::<Vec<_>>()
        });
        let at = Position {
            ino,
            offset: offset + whole as u64,
        };
        Some((at, body.to_string(), count))
    }

    /// One line's timestamp in nanoseconds, labels and text.
    fn entry(
        &self,
        source: Source,
        raw: &str,
        previous_ns: u128,
    ) -> (u128, Map<String, Value>, String) {
        let mut labels = Map::new();
        labels.insert("app".into(), json!("wheel"));
        labels.insert("source".into(), json!(source.name()));
        labels.insert("host".into(), json!(self.host));
        match source {
            Source::Wheeld => {
                // Lines carry an RFC 3339 time; a continuation line takes the one before it.
                let ns = rfc3339_ns(raw).unwrap_or(previous_ns);
                (ns, labels, raw.to_string())
            }
            Source::Journal | Source::App => {
                let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
                let ns = parsed["ts_ms"]
                    .as_u64()
                    .map_or(previous_ns, |ms| ms as u128 * 1_000_000);
                let client = parsed["client"].as_str().unwrap_or("unknown");
                labels.insert("client".into(), json!(client));
                if let Source::Journal = source {
                    let kind = parsed["op"].as_str().unwrap_or("unknown");
                    labels.insert("kind".into(), json!(kind));
                    return (ns, labels, raw.to_string());
                }
                let msg = parsed["msg"].as_str().unwrap_or(raw).to_string();
                (ns, labels, msg)
            }
        }
    }
}

/// The writer thread: append each line to its file, stamping a journal line with the time it is
/// written, so the journal's lines are in time order.
fn write_lines(
    rx: mpsc::Receiver<Line>,
    journal: PathBuf,
    journal_at: PathBuf,
    applog: PathBuf,
    applog_at: PathBuf,
    taken_at: PathBuf,
) {
    let mut journal = RollingFile::open(journal, journal_at);
    let mut applog = RollingFile::open(applog, applog_at);
    for line in rx {
        let (file, text) = match line {
            Line::Drained(done) => {
                for f in [&mut journal, &mut applog].into_iter().flatten() {
                    let _ = f.flush();
                }
                let _ = done.send(());
                continue;
            }
            Line::Taken(taken) => {
                if let Ok(text) = serde_json::to_string(&taken) {
                    write_atomic(&taken_at, &text);
                }
                continue;
            }
            Line::Journal(mut fields) => {
                fields.insert("ts_ms".into(), json!(now_ms()));
                (&mut journal, Value::Object(fields).to_string())
            }
            Line::App(client, l) => {
                let text = json!({ "ts_ms": l.ts_ms, "client": client, "msg": l.msg });
                (&mut applog, text.to_string())
            }
        };
        let written = match file {
            Ok(f) => writeln!(f, "{text}"),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        };
        if let Err(e) = written {
            tracing::warn!(target: OWN_TARGET, error = %e, "cannot write a journal or app log line");
        }
    }
}

/// The offset just past the line that starts at `from`, reading on until its newline; None while
/// that line is still being written.
fn skip_line(file: &mut File, from: u64) -> Option<u64> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut at = from;
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        if let Some(i) = buf[..n].iter().position(|&b| b == b'\n') {
            return Some(at + i as u64 + 1);
        }
        at += n as u64;
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

enum Push {
    /// Try the same lines again later, after the server's Retry-After when it gave one.
    Retry(String, Option<Duration>),
    /// The store will never take these lines.
    Rejected(String),
}

fn push(agent: &ureq::Agent, url: &str, body: &str) -> Result<(), Push> {
    let mut response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| Push::Retry(e.to_string(), None))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let said = response.body_mut().read_to_string().unwrap_or_default();
    let why = format!("HTTP {status}: {}", cut(said.trim(), MAX_ERR));
    match status {
        400 | 422 => Err(Push::Rejected(why)),
        _ => Err(Push::Retry(why, retry_after)),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn hostname() -> String {
    std::process::Command::new("/bin/hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The nanoseconds since the epoch of a line starting `YYYY-MM-DDTHH:MM:SS[.frac]Z`.
fn rfc3339_ns(line: &str) -> Option<u128> {
    let b = line.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| line.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut frac_ns: u128 = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let padded = format!("{:0<9}", &line[start..i]);
        frac_ns = padded.get(..9)?.parse().ok()?;
    }
    if b.get(i) != Some(&b'Z') {
        return None;
    }
    // Days from 1970-01-01 to the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3_600 + mi * 60 + s;
    Some(secs as u128 * 1_000_000_000 + frac_ns)
}
