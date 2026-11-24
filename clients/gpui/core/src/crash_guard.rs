//! Names what the client was drawing when it died (CrashGuard.swift), so the next launch can
//! leave that one space or pane out instead of crashing on it again.
//!
//! A Rust panic in a render unwinds to the panic hook; a trap in the bridge's Swift or a bad
//! access ends the process by a signal. Both paths write the two marks to a file as the process
//! dies, and nothing touches the disk while the client runs well. The next launch reads that file
//! once (`Crash::read`), and `Quarantine` holds the named space and pane out until the person
//! opens them anyway. A mark lasts until the render that set it ends, so a crash while a view
//! tree is built names the space or pane being built, and a crash at any other time names nothing.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

const CAPACITY: usize = 160;
/// Fixed buffers: the signal handler may call no code that allocates or takes a lock.
static SPACE: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static PANE: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static FILE: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The signals a trap, a bad access or an abort raises.
pub const FATAL_SIGNALS: [i32; 6] = [libc::SIGTRAP, libc::SIGILL, libc::SIGSEGV, libc::SIGBUS, libc::SIGABRT, libc::SIGFPE];

/// Start recording to `file`. Call once, early, before any view is built.
pub fn install(file: &Path) {
    set(&FILE, file.to_string_lossy().as_bytes());
    clear();
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record();
        previous(info);
    }));
    for sig in FATAL_SIGNALS {
        // SAFETY: the handler touches only the static buffers and async-signal-safe calls.
        unsafe { libc::signal(sig, on_signal as extern "C" fn(i32) as usize) };
    }
}

extern "C" fn on_signal(sig: i32) {
    record();
    // SAFETY: restoring the default action and raising again ends the process as it would have.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// A new space starts its own work: a pane marked while another space was built is not this one's.
pub fn mark_space(id: &str) {
    PANE[0].store(0, Ordering::Relaxed);
    set(&SPACE, format!("space:{id}").as_bytes());
}

pub fn mark_pane(id: &str) {
    set(&PANE, format!("pane:{id}").as_bytes());
}

/// Forget both marks: the render that set them ended.
pub fn clear() {
    SPACE[0].store(0, Ordering::Relaxed);
    PANE[0].store(0, Ordering::Relaxed);
}

fn set(slot: &[AtomicU8], text: &[u8]) {
    let n = text.len().min(slot.len() - 1);
    for (i, b) in text[..n].iter().enumerate() {
        slot[i].store(*b, Ordering::Relaxed);
    }
    slot[n].store(0, Ordering::Relaxed);
}

fn len(slot: &[AtomicU8]) -> usize {
    slot.iter().position(|b| b.load(Ordering::Relaxed) == 0).unwrap_or(slot.len())
}

/// Runs in the signal handler and the panic hook: system calls and the fixed buffers only.
fn record() {
    let n = len(&FILE);
    if n == 0 {
        return;
    }
    let mut path = [0u8; 1025];
    for (i, b) in path.iter_mut().enumerate().take(n) {
        *b = FILE[i].load(Ordering::Relaxed);
    }
    // SAFETY: open/write/close on a NUL-terminated path from a fixed buffer.
    unsafe {
        let fd = libc::open(path.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600);
        if fd < 0 {
            return;
        }
        for slot in [&SPACE, &PANE] {
            let mut line = [0u8; CAPACITY + 1];
            let n = len(slot);
            for (i, b) in line.iter_mut().enumerate().take(n) {
                *b = slot[i].load(Ordering::Relaxed);
            }
            line[n] = b'\n';
            libc::write(fd, line.as_ptr().cast(), n + 1);
        }
        libc::close(fd);
    }
}

/// What the last run was drawing when it died.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    pub space: Option<String>,
    pub pane: Option<String>,
    /// The crash report macOS wrote for it, when one is there yet.
    pub report: Option<PathBuf>,
}

impl Crash {
    /// Read and delete the file `install` was given; None when the last run ended cleanly or died
    /// while drawing neither a space nor a pane.
    pub fn read(file: &Path, reports: &Path) -> Option<Crash> {
        let text = std::fs::read_to_string(file).ok()?;
        let _ = std::fs::remove_file(file);
        Self::parse(&text, newest_report(reports))
    }

    pub fn parse(text: &str, report: Option<PathBuf>) -> Option<Crash> {
        let space = text.lines().find_map(|l| l.strip_prefix("space:")).map(str::to_owned);
        let pane = text.lines().find_map(|l| l.strip_prefix("pane:")).map(str::to_owned);
        (space.is_some() || pane.is_some()).then_some(Crash { space, pane, report })
    }
}

static LAUNCH: std::sync::OnceLock<Option<Crash>> = std::sync::OnceLock::new();

/// Where the client records its marks: beside wheel's own support files.
pub fn default_marks_file() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/wheel-gpui/crash-marks")
}

/// Install the guard and read what the last run died drawing, once, before any window opens.
pub fn start(file: &Path, reports: &Path) -> Option<Crash> {
    let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")));
    let crash = Crash::read(file, reports);
    install(file);
    LAUNCH.get_or_init(|| crash.clone()).clone()
}

/// A window's quarantine as it opens: what `start` read, or nothing.
pub fn launch_quarantine() -> Quarantine {
    let mut q = Quarantine::default();
    if let Some(crash) = LAUNCH.get().cloned().flatten() {
        q.hold(crash);
    }
    q
}

/// Where macOS writes crash reports.
pub fn default_reports() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Logs/DiagnosticReports")
}

fn newest_report(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("wheel-gpui-") && name.ends_with(".ips")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// The spaces and panes the last crash named, held out of the grid until the person opens them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quarantine {
    pub spaces: HashSet<String>,
    pub panes: HashSet<String>,
    pub report: Option<PathBuf>,
}

impl Quarantine {
    /// Hold out what `crash` names: the pane when it names one, else the space.
    pub fn hold(&mut self, crash: Crash) {
        self.report = crash.report;
        if let Some(pane) = crash.pane {
            self.panes.insert(pane);
        } else if let Some(space) = crash.space {
            self.spaces.insert(space);
        }
    }
    pub fn isolates_space(&self, id: &str) -> bool {
        self.spaces.contains(id)
    }
    pub fn isolates_pane(&self, id: &str) -> bool {
        self.panes.contains(id)
    }
    pub fn release_space(&mut self, id: &str) {
        self.spaces.remove(id);
    }
    pub fn release_pane(&mut self, id: &str) {
        self.panes.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_written_and_read_back() {
        let dir = std::env::temp_dir().join(format!("wheel-gpui-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("marks");
        install(&file);
        mark_space("s1");
        mark_pane("p2");
        record();
        let crash = Crash::read(&file, &dir).unwrap();
        assert_eq!(crash, Crash { space: Some("s1".into()), pane: Some("p2".into()), report: None });
        assert!(!file.exists(), "read deletes the file");
        // A space mark drops the pane mark of the space before it; a clear drops both.
        mark_pane("p9");
        mark_space("s2");
        record();
        assert_eq!(Crash::read(&file, &dir).unwrap().pane, None);
        clear();
        record();
        assert_eq!(Crash::read(&file, &dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quarantine_holds_the_pane_over_the_space() {
        let mut q = Quarantine::default();
        q.hold(Crash::parse("space:s1\npane:p2\n", None).unwrap());
        assert!(q.isolates_pane("p2") && !q.isolates_space("s1"));
        q.hold(Crash::parse("space:s3\n\n", Some(PathBuf::from("/r.ips"))).unwrap());
        assert!(q.isolates_space("s3"));
        assert_eq!(q.report.as_deref(), Some(Path::new("/r.ips")));
        q.release_pane("p2");
        q.release_space("s3");
        assert!(q.spaces.is_empty() && q.panes.is_empty());
        assert_eq!(Crash::parse("\n\n", None), None);
    }
}
