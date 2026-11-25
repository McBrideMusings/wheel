//! One place the whole daemon watches files from. A `notify::RecommendedWatcher` on macOS owns an
//! OS thread and an FSEvents stream, and wheeld watched per pane, per agent and per file, so threads
//! grew with every pane. [`watcher`] hands out a [`Handle`] that implements [`notify::Watcher`]
//! like the watcher it replaces, but every handle shares the same streams: a folder any number of
//! handles watch has one FSEvents stream, and each event goes only to the handles whose folders hold
//! it. Every stream delivers on one serial dispatch queue, so the daemon spends one queue thread
//! and one router thread on files however many folders it watches.
//!
//! A folder's stream starts when it is first watched and stops when the last watch of it ends; no
//! stream is ever restarted to take in another folder, so watching a new folder never interrupts
//! the ones already watched.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use fsevent_sys as fs;
use fsevent_sys::core_foundation as cf;
use notify::event::{CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventHandler, EventKind, RecursiveMode, Watcher};

type Handler = Arc<Mutex<Box<dyn EventHandler>>>;

/// One event as FSEvents gave it: the path, its flags and its id.
type Raw = (PathBuf, fs::FSEventStreamEventFlags, fs::FSEventStreamEventId);

struct Watched {
    given: PathBuf,
    real: PathBuf,
    mode: RecursiveMode,
}

struct Sub {
    handler: Handler,
    watched: Vec<Watched>,
}

struct SendRef(*mut std::ffi::c_void);
// SAFETY: FSEvents streams and dispatch queues are documented as usable from any thread; these are
// only started, stopped and released through the hub's lock.
unsafe impl Send for SendRef {}
unsafe impl Sync for SendRef {}

/// One watched folder: how many watches name it, and its stream.
struct Folder {
    count: usize,
    stream: SendRef,
}

struct Hub {
    subs: HashMap<u64, Sub>,
    /// Every watched folder, as the file system names it.
    folders: HashMap<PathBuf, Folder>,
    /// Where every stream's callback sends what it saw, for the router.
    events: mpsc::Sender<Vec<Raw>>,
    next: u64,
}

/// How many delivered event ids the router keeps, to drop the copy a second stream sends when a
/// watched folder lies inside another watched folder.
const KEPT: usize = 4096;

extern "C" {
    fn FSEventStreamSetDispatchQueue(stream: fs::FSEventStreamRef, queue: *mut std::ffi::c_void);
    fn dispatch_queue_create(label: *const std::ffi::c_char, attr: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

/// The serial queue every stream delivers on.
fn queue() -> *mut std::ffi::c_void {
    static QUEUE: OnceLock<SendRef> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            // SAFETY: a static C string label and no attributes, which makes a serial queue.
            SendRef(unsafe { dispatch_queue_create(c"wheel.fsevents".as_ptr(), std::ptr::null_mut()) })
        })
        .0
}

fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| {
        let (events, rx) = mpsc::channel::<Vec<Raw>>();
        std::thread::Builder::new()
            .name("wheel fsevents route".into())
            .spawn(move || {
                let (mut seen, mut order) = (HashSet::new(), VecDeque::new());
                for batch in rx {
                    for raw in batch {
                        if !seen.insert(raw.2) {
                            continue;
                        }
                        order.push_back(raw.2);
                        if order.len() > KEPT {
                            if let Some(old) = order.pop_front() {
                                seen.remove(&old);
                            }
                        }
                        route(raw);
                    }
                }
            })
            .expect("could not start the file-event router");
        Mutex::new(Hub {
            subs: HashMap::new(),
            folders: HashMap::new(),
            events,
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

fn kind(flags: fs::FSEventStreamEventFlags) -> EventKind {
    let has = |f: fs::FSEventStreamEventFlags| flags & f != 0;
    if has(fs::kFSEventStreamEventFlagItemCreated) {
        EventKind::Create(CreateKind::Any)
    } else if has(fs::kFSEventStreamEventFlagItemRemoved) {
        EventKind::Remove(RemoveKind::Any)
    } else if has(fs::kFSEventStreamEventFlagItemRenamed) {
        EventKind::Modify(ModifyKind::Name(RenameMode::Any))
    } else if has(fs::kFSEventStreamEventFlagItemModified) {
        EventKind::Modify(ModifyKind::Data(DataChange::Any))
    } else if has(fs::kFSEventStreamEventFlagItemInodeMetaMod) {
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))
    } else {
        EventKind::Other
    }
}

/// Hand one event to each handle whose folders hold its path. Handlers run outside the hub's lock,
/// so one may watch or unwatch.
fn route((path, flags, _id): Raw) {
    if flags & fs::kFSEventStreamEventFlagHistoryDone != 0 {
        return;
    }
    // An event naming no change to an item is FSEvents saying something under a folder changed
    // without saying what (it dropped events, say): a rescan. Its folder may be one above a watched
    // folder, so it reaches that watch too, named as the watched folder.
    let rescan = kind(flags) == EventKind::Other;
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
        let mut event = Event::new(kind(flags)).add_path(named);
        if rescan {
            event = event.set_flag(notify::event::Flag::Rescan);
        }
        handler.lock().unwrap().handle_event(Ok(event));
    }
}

extern "C" fn release_context(info: *const std::ffi::c_void) {
    // SAFETY: `info` is the Box this module leaked into the stream's context; the stream calls
    // release once, as it is freed.
    unsafe { drop(Box::from_raw(info as *mut mpsc::Sender<Vec<Raw>>)) };
}

extern "C" fn callback(
    _stream: fs::FSEventStreamRef,
    info: *mut std::ffi::c_void,
    count: usize,
    paths: *mut std::ffi::c_void,
    flags: *const fs::FSEventStreamEventFlags,
    ids: *const fs::FSEventStreamEventId,
) {
    // SAFETY: FSEvents passes `count` C strings, flags and ids, and `info` is the live sender.
    let (events, batch) = unsafe {
        let paths = paths as *const *const std::ffi::c_char;
        let batch: Vec<Raw> = (0..count)
            .map(|i| {
                let path = CStr::from_ptr(*paths.add(i)).to_string_lossy().into_owned();
                (PathBuf::from(path), *flags.add(i), *ids.add(i))
            })
            .collect();
        (&*(info as *const mpsc::Sender<Vec<Raw>>), batch)
    };
    let _ = events.send(batch);
}

/// A stream over `dir` from now, delivering on the shared queue; running when this returns.
fn start(dir: &Path, events: &mpsc::Sender<Vec<Raw>>) -> notify::Result<SendRef> {
    // SAFETY: Core Foundation and FSEvents calls on objects made here; the path and array are
    // released once the stream, which retains them, is made.
    unsafe {
        let mut err: cf::CFErrorRef = std::ptr::null_mut();
        let path = cf::str_path_to_cfstring_ref(&dir.to_string_lossy(), &mut err);
        if path.is_null() {
            cf::CFRelease(err as cf::CFRef);
            return Err(notify::Error::path_not_found().add_path(dir.to_path_buf()));
        }
        let paths = cf::CFArrayCreateMutable(cf::kCFAllocatorDefault, 0, &cf::kCFTypeArrayCallBacks);
        cf::CFArrayAppendValue(paths, path);
        cf::CFRelease(path);
        let context = Box::into_raw(Box::new(events.clone()));
        let stream_context = fs::FSEventStreamContext {
            version: 0,
            info: context as *mut std::ffi::c_void,
            retain: None,
            release: Some(release_context),
            copy_description: None,
        };
        let stream = fs::FSEventStreamCreate(
            cf::kCFAllocatorDefault,
            callback,
            &stream_context,
            paths,
            fs::kFSEventStreamEventIdSinceNow,
            0.0,
            fs::kFSEventStreamCreateFlagFileEvents | fs::kFSEventStreamCreateFlagNoDefer,
        );
        cf::CFRelease(paths);
        if stream.is_null() {
            return Err(notify::Error::generic(&format!("FSEvents made no stream for {}", dir.display())));
        }
        FSEventStreamSetDispatchQueue(stream, queue());
        if fs::FSEventStreamStart(stream) == 0 {
            fs::FSEventStreamInvalidate(stream);
            fs::FSEventStreamRelease(stream);
            return Err(notify::Error::generic(&format!("FSEvents would not start a stream for {}", dir.display())));
        }
        Ok(SendRef(stream))
    }
}

/// Stop a stream, delivering what FSEvents still holds for it first.
fn stop(stream: SendRef) {
    // SAFETY: a stream `start` made and started, stopped and freed once.
    unsafe {
        fs::FSEventStreamFlushSync(stream.0);
        fs::FSEventStreamStop(stream.0);
        fs::FSEventStreamInvalidate(stream.0);
        fs::FSEventStreamRelease(stream.0);
    }
}

impl Hub {
    fn add(&mut self, real: &Path) -> notify::Result<()> {
        if let Some(folder) = self.folders.get_mut(real) {
            folder.count += 1;
            return Ok(());
        }
        let stream = start(real, &self.events)?;
        self.folders.insert(real.to_path_buf(), Folder { count: 1, stream });
        Ok(())
    }

    fn remove(&mut self, real: &Path) {
        let Some(folder) = self.folders.get_mut(real) else { return };
        folder.count -= 1;
        if folder.count == 0 {
            if let Some(folder) = self.folders.remove(real) {
                stop(folder.stream);
            }
        }
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
        hub.add(&real)?;
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
        let gone: Vec<PathBuf> = match hub.subs.get_mut(&self.id) {
            Some(sub) => {
                let (gone, kept): (Vec<Watched>, Vec<Watched>) =
                    sub.watched.drain(..).partition(|w| w.given == path);
                sub.watched = kept;
                gone.into_iter().map(|w| w.real).collect()
            }
            None => Vec::new(),
        };
        if gone.is_empty() {
            return Err(notify::Error::watch_not_found().add_path(path.to_path_buf()));
        }
        for real in gone {
            hub.remove(&real);
        }
        Ok(())
    }

    fn configure(&mut self, _config: notify::Config) -> notify::Result<bool> {
        Ok(false)
    }

    fn kind() -> notify::WatcherKind {
        notify::WatcherKind::Fsevent
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut hub = hub().lock().unwrap();
        if let Some(sub) = hub.subs.remove(&self.id) {
            for w in sub.watched {
                hub.remove(&w.real);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(hub().lock().unwrap().folders.get(&a_dir).map(|f| f.count), Some(2));

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
        assert_eq!(hub().lock().unwrap().folders.get(&a_dir).map(|f| f.count), Some(1));
        let again = a_dir.join("h.txt");
        std::fs::write(&again, "x").unwrap();
        assert!(seen(&rx1, &again), "the flat watch stopped when the other handle dropped");

        drop(one);
        drop(three);
        assert!(!hub().lock().unwrap().folders.contains_key(&a_dir));
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
