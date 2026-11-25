//! One place the whole daemon watches files from. A `notify::RecommendedWatcher` owns an OS thread
//! (and on macOS an FSEvents stream), and wheeld watched per pane, per agent and per file, so
//! threads grew with every pane. [`watcher`] hands out a [`Handle`] that implements
//! [`notify::Watcher`] like the watcher it replaces, but every handle shares one backend: FSEvents
//! on macOS (`fswatch/fsevents.rs`), inotify on Linux (`fswatch/inotify.rs`). A folder any number
//! of handles watch is watched by the system once, and each event goes only to the handles whose
//! folders hold it, so the daemon spends a fixed number of threads on files however many folders
//! it watches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use notify::{Event, EventHandler, EventKind, RecursiveMode, Watcher};

#[cfg(target_os = "macos")]
#[path = "fswatch/fsevents.rs"]
mod os;
#[cfg(target_os = "linux")]
#[path = "fswatch/inotify.rs"]
mod os;

type Handler = Arc<Mutex<Box<dyn EventHandler>>>;

struct Watched {
    given: PathBuf,
    real: PathBuf,
    mode: RecursiveMode,
}

struct Sub {
    handler: Handler,
    watched: Vec<Watched>,
}

struct Hub {
    subs: HashMap<u64, Sub>,
    os: os::Streams,
    next: u64,
}

fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| {
        Mutex::new(Hub {
            subs: HashMap::new(),
            os: os::Streams::new(),
            next: 0,
        })
    })
}

/// Whether `path` is in the folder `dir` a handle watches with `mode`: the folder itself or what
/// sits in it, and for a recursive watch anything below it.
fn holds(dir: &Path, mode: RecursiveMode, path: &Path) -> bool {
    match mode {
        RecursiveMode::Recursive => path.starts_with(dir),
        RecursiveMode::NonRecursive => path == dir || path.parent() == Some(dir),
    }
}

/// Hand one event to each handle whose folders hold its path. A `rescan` is the system saying
/// something under `path` changed without saying what (it dropped events, say); its folder may be
/// one above a watched folder, so it reaches that watch too, named as the watched folder. Handlers
/// run outside the hub's lock, so one may watch or unwatch.
fn route(path: PathBuf, kind: EventKind, rescan: bool) {
    let deliveries: Vec<(Handler, PathBuf)> = {
        let hub = hub().lock().unwrap();
        hub.subs
            .values()
            .filter_map(|sub| {
                sub.watched.iter().find_map(|w| {
                    if holds(&w.real, w.mode, &path) || holds(&w.given, w.mode, &path) {
                        Some((sub.handler.clone(), path.clone()))
                    } else if rescan && w.real.starts_with(&path) {
                        Some((sub.handler.clone(), w.real.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect()
    };
    for (handler, named) in deliveries {
        let mut event = Event::new(kind).add_path(named);
        if rescan {
            event = event.set_flag(notify::event::Flag::Rescan);
        }
        handler.lock().unwrap().handle_event(Ok(event));
    }
}

/// Watch `dir` with `mode`, or, while it does not exist, the nearest folder above it that does,
/// so the folder being made is seen. A cache that reads `dir` drops what it kept on either.
pub(crate) fn watch_folder(watch: &mut Handle, dir: &Path, mode: RecursiveMode) -> notify::Result<()> {
    match dir.is_dir() {
        true => watch.watch(dir, mode),
        false => match dir.ancestors().skip(1).find(|a| a.is_dir()) {
            Some(above) => watch.watch(above, RecursiveMode::NonRecursive),
            None => Ok(()),
        },
    }
}

/// A watcher on the daemon's shared streams. Dropping it stops its watches.
pub struct Handle {
    id: u64,
}

/// A watcher that calls `handler` for events under the folders it is given, as
/// `notify::recommended_watcher` does, on the daemon's shared streams.
pub fn watcher<F: EventHandler>(handler: F) -> notify::Result<Handle> {
    let mut hub = hub().lock().unwrap();
    hub.next += 1;
    let id = hub.next;
    hub.subs.insert(
        id,
        Sub {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            watched: Vec::new(),
        },
    );
    Ok(Handle { id })
}

impl Watcher for Handle {
    fn new<F: EventHandler>(handler: F, _config: notify::Config) -> notify::Result<Self> {
        watcher(handler)
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        if !path.exists() {
            return Err(notify::Error::path_not_found().add_path(path.to_path_buf()));
        }
        let real = path.canonicalize().map_err(notify::Error::io)?;
        let mut hub = hub().lock().unwrap();
        hub.os.add(&real, mode)?;
        if let Some(sub) = hub.subs.get_mut(&self.id) {
            sub.watched.push(Watched {
                given: path.to_path_buf(),
                real,
                mode,
            });
        }
        Ok(())
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        let mut hub = hub().lock().unwrap();
        let gone: Vec<(PathBuf, RecursiveMode)> = match hub.subs.get_mut(&self.id) {
            Some(sub) => {
                let (gone, kept): (Vec<Watched>, Vec<Watched>) =
                    sub.watched.drain(..).partition(|w| w.given == path);
                sub.watched = kept;
                gone.into_iter().map(|w| (w.real, w.mode)).collect()
            }
            None => Vec::new(),
        };
        if gone.is_empty() {
            return Err(notify::Error::watch_not_found().add_path(path.to_path_buf()));
        }
        for (real, mode) in gone {
            hub.os.remove(&real, mode);
        }
        Ok(())
    }

    fn configure(&mut self, _config: notify::Config) -> notify::Result<bool> {
        Ok(false)
    }

    fn kind() -> notify::WatcherKind {
        os::KIND
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut hub = hub().lock().unwrap();
        if let Some(sub) = hub.subs.remove(&self.id) {
            for w in sub.watched {
                hub.os.remove(&w.real, w.mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Whether `want` changed: named itself, or covered by a rescan of a folder above it.
    fn seen(rx: &mpsc::Receiver<(PathBuf, bool)>, want: &Path) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok((p, rescan)) if p == want || (rescan && want.starts_with(&p)) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    fn handle(tx: mpsc::Sender<(PathBuf, bool)>) -> Handle {
        watcher(move |e: notify::Result<Event>| {
            if let Ok(e) = e {
                let rescan = e.need_rescan();
                for p in e.paths {
                    let _ = tx.send((p, rescan));
                }
            }
        })
        .unwrap()
    }

    fn fresh(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wheel-fswatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn handles_share_one_watch_and_each_gets_only_its_folders_events() {
        let root = fresh("share");
        let (a_dir, b_dir) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(a_dir.join("deep")).unwrap();
        std::fs::create_dir_all(&b_dir).unwrap();

        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let (tx3, rx3) = mpsc::channel();
        let mut one = handle(tx1);
        let mut two = handle(tx2);
        let mut three = handle(tx3);
        one.watch(&a_dir, RecursiveMode::NonRecursive).unwrap();
        two.watch(&a_dir, RecursiveMode::Recursive).unwrap();
        three.watch(&b_dir, RecursiveMode::NonRecursive).unwrap();
        assert!(hub().lock().unwrap().os.watching(&a_dir));

        let flat = a_dir.join("f.txt");
        std::fs::write(&flat, "x").unwrap();
        assert!(seen(&rx1, &flat), "the flat watch missed a file in its folder");
        assert!(seen(&rx2, &flat), "the recursive watch missed a file in its folder");

        let nested = a_dir.join("deep").join("g.txt");
        std::fs::write(&nested, "x").unwrap();
        assert!(seen(&rx2, &nested), "the recursive watch missed a nested file");
        assert!(!rx1.try_iter().any(|(p, _)| p == nested), "the flat watch got a nested file");
        assert!(!rx3.try_iter().any(|(p, _)| p.starts_with(&a_dir)), "b's handle got a's events");

        drop(two);
        assert!(hub().lock().unwrap().os.watching(&a_dir), "a folder stopped while a handle still watches it");
        let again = a_dir.join("h.txt");
        std::fs::write(&again, "x").unwrap();
        assert!(seen(&rx1, &again), "the flat watch stopped when the other handle dropped");

        drop(one);
        drop(three);
        assert!(!hub().lock().unwrap().os.watching(&a_dir));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_folder_made_under_a_recursive_watch_is_watched() {
        let root = fresh("made");
        let (tx, rx) = mpsc::channel();
        let mut deep = handle(tx);
        deep.watch(&root, RecursiveMode::Recursive).unwrap();
        // Made and written at once, before any watch of the new folders could start.
        let made = root.join("new").join("inner");
        std::fs::create_dir_all(&made).unwrap();
        let early = made.join("early.txt");
        std::fs::write(&early, "x").unwrap();
        assert!(seen(&rx, &early), "a file made with its folder was missed");
        let later = made.join("later.txt");
        std::fs::write(&later, "x").unwrap();
        assert!(seen(&rx, &later), "a file in a folder made under the watch was missed");
        drop(deep);
        assert!(!hub().lock().unwrap().os.watching(&made));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_watched_folder_deleted_or_moved_away_is_watched_when_it_comes_back() {
        let root = fresh("back");
        let dir = root.join("watched");
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut flat = handle(tx);
        flat.watch(&dir, RecursiveMode::NonRecursive).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let after_delete = dir.join("a.txt");
        std::fs::write(&after_delete, "x").unwrap();
        assert!(seen(&rx, &after_delete), "the folder made again after a delete went unwatched");

        std::fs::rename(&dir, root.join("old")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let after_move = dir.join("b.txt");
        std::fs::write(&after_move, "x").unwrap();
        assert!(seen(&rx, &after_move), "the folder made in place of a moved one went unwatched");
        drop(flat);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_change_made_while_another_folder_is_added_is_not_lost() {
        let root = fresh("gap");
        let (watched, other) = (root.join("watched"), root.join("other"));
        std::fs::create_dir_all(&watched).unwrap();
        let (tx, rx) = mpsc::channel::<(PathBuf, bool)>();
        let mut kept = handle(tx);
        kept.watch(&watched, RecursiveMode::NonRecursive).unwrap();

        // Each new folder starts a new stream while a writer keeps changing the watched folder.
        let writing = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let writer = {
            let (watched, writing) = (watched.clone(), writing.clone());
            std::thread::spawn(move || {
                let mut written = Vec::new();
                let mut i = 0;
                while writing.load(std::sync::atomic::Ordering::SeqCst) {
                    let file = watched.join(format!("f{i}.txt"));
                    std::fs::write(&file, "x").unwrap();
                    written.push(file);
                    i += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
                written
            })
        };
        let mut handles = Vec::new();
        for i in 0..20 {
            let dir = other.join(format!("d{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            let (tx, _rx) = mpsc::channel();
            let mut h = handle(tx);
            h.watch(&dir, RecursiveMode::NonRecursive).unwrap();
            handles.push(h);
        }
        writing.store(false, std::sync::atomic::Ordering::SeqCst);
        let written = writer.join().unwrap();

        let (mut got, mut rescanned) = (HashSet::new(), false);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !rescanned && !written.iter().all(|f| got.contains(f)) {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else { break };
            match rx.recv_timeout(left) {
                Ok((p, rescan)) if rescan && p == watched => rescanned = true,
                Ok((p, _)) => {
                    got.insert(p);
                }
                Err(_) => break,
            }
        }
        let missed: Vec<_> = written.iter().filter(|f| !got.contains(*f) && !rescanned).collect();
        assert!(missed.is_empty(), "{} of {} changes lost while streams swapped: {missed:?}", missed.len(), written.len());
        drop(handles);
        drop(kept);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
