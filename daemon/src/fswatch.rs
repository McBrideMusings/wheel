//! One file-system event stream for the whole daemon. A `notify::RecommendedWatcher` on macOS
//! owns an OS thread and an FSEvents stream, and wheeld watched per pane, per agent and per file,
//! so threads grew with every pane. [`watcher`] hands out a [`Handle`] that implements
//! [`notify::Watcher`] like the watcher it replaces, but every handle shares one FSEvents stream:
//! a folder two handles watch is in the stream once, and each event goes only to the handles whose
//! folders hold it.
//!
//! FSEvents cannot add a path to a running stream, so a new folder means a new stream. notify
//! starts each new stream from "now", which drops whatever happened while it swapped streams; this
//! one starts each new stream from the last event id it saw, so FSEvents replays the gap, and a
//! handle ignores events from before it began watching.

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// The event id when the watch began; earlier events, which a restarted stream replays, are
    /// not this watch's.
    since: fs::FSEventStreamEventId,
}

struct Sub {
    handler: Handler,
    watched: Vec<Watched>,
}

/// The running stream: its run loop, to stop it, the thread that runs it, and whether it has
/// delivered all the history it was started with.
struct Stream {
    runloop: SendRef,
    thread: std::thread::JoinHandle<()>,
    caught_up: Arc<AtomicBool>,
}

struct SendRef(cf::CFRef);
// SAFETY: Core Foundation documents CFRunLoop and CFArray references as safe to pass between
// threads; this one is only stopped from another thread.
unsafe impl Send for SendRef {}

struct Hub {
    subs: HashMap<u64, Sub>,
    /// How many watches name each folder, as the file system names it.
    counts: HashMap<PathBuf, usize>,
    /// Asks [`rebuilder`] to rebuild the stream over `counts`.
    rebuild: mpsc::Sender<()>,
    /// The folders the running stream covers.
    streamed: std::collections::HashSet<PathBuf>,
    next: u64,
}

impl Hub {
    /// The earliest moment a watch the running stream does not cover yet began: the next stream
    /// has to start at or before it, or that watch's first changes go unseen.
    fn pending(&self) -> Option<fs::FSEventStreamEventId> {
        self.subs
            .values()
            .flat_map(|s| &s.watched)
            .filter(|w| !self.streamed.contains(&w.real))
            .map(|w| w.since)
            .min()
    }
}

/// Where the next stream starts: the floor, or the start of a watch it is the first to cover when
/// that is earlier.
fn start_point(floor: fs::FSEventStreamEventId, pending: Option<fs::FSEventStreamEventId>) -> fs::FSEventStreamEventId {
    pending.map_or(floor, |p| p.min(floor))
}

/// How many handed-out event ids the router holds before it drops those no stream can replay.
const KEPT: usize = 4096;

fn hub() -> &'static Mutex<Hub> {
    static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
    HUB.get_or_init(|| {
        let (events, rx) = mpsc::channel::<Vec<Raw>>();
        // SAFETY: a plain query of the system's current event id.
        let now = unsafe { fs::FSEventsGetCurrentEventId() };
        // Where the next stream starts: every event up to it was delivered. FSEvents hands a new
        // stream's history back in no set order, so it moves up to `last`, the newest id seen,
        // only once the stream started from it has delivered all of it.
        let floor = Arc::new(AtomicU64::new(now));
        let last = Arc::new(AtomicU64::new(now));
        let newest = last.clone();
        let (rebuild, asks) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("wheel fsevents rebuild".into())
            .spawn({
                let (events, floor, last) = (events.clone(), floor.clone(), last);
                move || rebuilder(asks, events, floor, last)
            })
            .expect("could not start the file-event rebuilder");
        std::thread::Builder::new()
            .name("wheel fsevents route".into())
            .spawn(move || {
                let mut delivered = std::collections::HashSet::new();
                for batch in rx {
                    for raw in batch {
                        if delivered.insert(raw.2) {
                            route(raw);
                        }
                    }
                    // A later stream starts no earlier than the newest id seen or the start of a
                    // watch it does not cover yet, so ids below both are never handed out again.
                    if delivered.len() > KEPT {
                        let replayable = hub()
                            .lock()
                            .unwrap()
                            .pending()
                            .map_or(newest.load(Ordering::SeqCst), |p| p.min(newest.load(Ordering::SeqCst)));
                        delivered.retain(|id| *id >= replayable);
                    }
                }
            })
            .expect("could not start the file-event router");
        Mutex::new(Hub {
            subs: HashMap::new(),
            counts: HashMap::new(),
            rebuild,
            streamed: Default::default(),
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

/// Hand one event to each handle whose folders hold its path and that was watching when it
/// happened. Handlers run outside the hub's lock, so one may watch or unwatch.
fn route((path, flags, id): Raw) {
    if flags & fs::kFSEventStreamEventFlagHistoryDone != 0 {
        return;
    }
    // An event naming no change to an item is FSEvents saying something under a folder changed
    // without saying what, as it does when it replays across a stream swap: a rescan. Its folder
    // may be one above a watched folder, so it reaches that watch too, named as the watched folder.
    let rescan = kind(flags) == EventKind::Other;
    let deliveries: Vec<(Handler, PathBuf)> = {
        let hub = hub().lock().unwrap();
        hub.subs
            .values()
            .filter_map(|sub| {
                sub.watched.iter().filter(|w| id > w.since).find_map(|w| {
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

/// What the stream's callback is given: where to send events, and the newest id to keep.
struct Context {
    events: mpsc::Sender<Vec<Raw>>,
    last: Arc<AtomicU64>,
    caught_up: Arc<AtomicBool>,
}

extern "C" fn release_context(info: *const std::ffi::c_void) {
    // SAFETY: `info` is the Box this module leaked into the stream's context; the stream calls
    // release once, as it is freed.
    unsafe { drop(Box::from_raw(info as *mut Context)) };
}

extern "C" fn callback(
    _stream: fs::FSEventStreamRef,
    info: *mut std::ffi::c_void,
    count: usize,
    paths: *mut std::ffi::c_void,
    flags: *const fs::FSEventStreamEventFlags,
    ids: *const fs::FSEventStreamEventId,
) {
    // SAFETY: FSEvents passes `count` C strings, flags and ids, and `info` is the live Context.
    let (context, batch) = unsafe {
        let paths = paths as *const *const std::ffi::c_char;
        let batch: Vec<Raw> = (0..count)
            .map(|i| {
                let path = CStr::from_ptr(*paths.add(i)).to_string_lossy().into_owned();
                (PathBuf::from(path), *flags.add(i), *ids.add(i))
            })
            .collect();
        (&*(info as *const Context), batch)
    };
    if let Some(newest) = batch.iter().map(|r| r.2).max() {
        context.last.fetch_max(newest, Ordering::SeqCst);
    }
    let done = batch.iter().any(|r| r.1 & fs::kFSEventStreamEventFlagHistoryDone != 0);
    let _ = context.events.send(batch);
    if done {
        context.caught_up.store(true, Ordering::SeqCst);
    }
}

extern "C" {
    fn CFRunLoopIsWaiting(runloop: cf::CFRunLoopRef) -> cf::Boolean;
}


/// Stop `old` and wait for its thread. Once it delivered all the history it was started with,
/// everything up to the newest id seen was delivered, so the floor moves up to it.
fn stop(old: Stream, floor: &AtomicU64, last: &AtomicU64) {
    // SAFETY: the run loop is live until its thread returns, which the join below waits for.
    unsafe {
        while CFRunLoopIsWaiting(old.runloop.0) == 0 {
            std::thread::yield_now();
        }
        cf::CFRunLoopStop(old.runloop.0);
    }
    let _ = old.thread.join();
    if old.caught_up.load(Ordering::SeqCst) {
        floor.fetch_max(last.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// A stream over `dirs` from `since`, sending what it sees on `events`.
fn start(
    dirs: &[PathBuf],
    since: fs::FSEventStreamEventId,
    events: &mpsc::Sender<Vec<Raw>>,
    last: &Arc<AtomicU64>,
) -> Option<Stream> {
    let caught_up = Arc::new(AtomicBool::new(false));
    // SAFETY: Core Foundation calls on objects made here; the array is released once the stream,
    // which retains it, is made.
    let stream = unsafe {
        let paths = cf::CFArrayCreateMutable(cf::kCFAllocatorDefault, 0, &cf::kCFTypeArrayCallBacks);
        for dir in dirs {
            let mut err: cf::CFErrorRef = std::ptr::null_mut();
            let text = dir.to_string_lossy();
            let path = cf::str_path_to_cfstring_ref(&text, &mut err);
            if path.is_null() {
                cf::CFRelease(err as cf::CFRef);
                continue;
            }
            cf::CFArrayAppendValue(paths, path);
            cf::CFRelease(path);
        }
        let context = Box::into_raw(Box::new(Context {
            events: events.clone(),
            last: last.clone(),
            caught_up: caught_up.clone(),
        }));
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
            since,
            0.0,
            fs::kFSEventStreamCreateFlagFileEvents | fs::kFSEventStreamCreateFlagNoDefer,
        );
        cf::CFRelease(paths);
        SendRef(stream)
    };
    let (tx, rx) = mpsc::channel::<SendRef>();
    let thread = std::thread::Builder::new()
        .name("wheel fsevents".into())
        .spawn(move || {
            let stream = stream;
            // SAFETY: the stream made above, scheduled, run and freed on this one thread.
            unsafe {
                let runloop = cf::CFRunLoopGetCurrent();
                fs::FSEventStreamScheduleWithRunLoop(stream.0, runloop, cf::kCFRunLoopDefaultMode);
                fs::FSEventStreamStart(stream.0);
                let _ = tx.send(SendRef(runloop));
                cf::CFRunLoopRun();
                fs::FSEventStreamStop(stream.0);
                fs::FSEventStreamInvalidate(stream.0);
                fs::FSEventStreamRelease(stream.0);
            }
        })
        .inspect_err(|e| tracing::error!(error = %e, "could not start the file-event stream; file changes go unseen"))
        .ok()?;
    let runloop = rx.recv().ok()?;
    Some(Stream { runloop, thread, caught_up })
}

/// Keep one stream over the folders the hub's watches name, rebuilding it each time it is asked
/// to. Asks that arrive while a rebuild runs are folded into the next one. A watch never waits
/// for a rebuild: the new stream starts at the floor, or earlier when a watch it is the first to
/// cover began before the floor, so FSEvents replays that watch's changes from its start.
fn rebuilder(
    asks: mpsc::Receiver<()>,
    events: mpsc::Sender<Vec<Raw>>,
    floor: Arc<AtomicU64>,
    last: Arc<AtomicU64>,
) {
    let mut stream: Option<Stream> = None;
    while asks.recv().is_ok() {
        while asks.try_recv().is_ok() {}
        if let Some(old) = stream.take() {
            stop(old, &floor, &last);
        }
        let (dirs, pending) = {
            let hub = hub().lock().unwrap();
            (hub.counts.keys().cloned().collect::<Vec<_>>(), hub.pending())
        };
        let from = start_point(floor.load(Ordering::SeqCst), pending);
        if !dirs.is_empty() {
            stream = start(&dirs, from, &events, &last);
        }
        hub().lock().unwrap().streamed = match stream {
            Some(_) => dirs.into_iter().collect(),
            None => Default::default(),
        };
    }
}

impl Hub {
    fn add(&mut self, real: &Path) {
        let count = self.counts.entry(real.to_path_buf()).or_insert(0);
        *count += 1;
        if *count == 1 {
            let _ = self.rebuild.send(());
        }
    }

    fn remove(&mut self, real: &Path) {
        let Some(count) = self.counts.get_mut(real) else { return };
        *count -= 1;
        if *count == 0 {
            self.counts.remove(real);
            let _ = self.rebuild.send(());
        }
    }
}


/// A watcher on the daemon's one shared stream. Dropping it stops its watches.
pub struct Handle {
    id: u64,
}

/// A watcher that calls `handler` for events under the folders it is given, as
/// `notify::recommended_watcher` does, on the daemon's one shared stream.
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
        // SAFETY: a plain query of the system's current event id.
        let since = unsafe { fs::FSEventsGetCurrentEventId() };
        hub.add(&real);
        if let Some(sub) = hub.subs.get_mut(&self.id) {
            sub.watched.push(Watched {
                given: path.to_path_buf(),
                real,
                mode,
                since,
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

    fn seen(rx: &mpsc::Receiver<PathBuf>, want: &Path) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(p) if p == want => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    fn handle(tx: mpsc::Sender<PathBuf>) -> Handle {
        watcher(move |e: notify::Result<Event>| {
            if let Ok(e) = e {
                for p in e.paths {
                    let _ = tx.send(p);
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
        assert_eq!(hub().lock().unwrap().counts.get(&a_dir), Some(&2));

        let flat = a_dir.join("f.txt");
        std::fs::write(&flat, "x").unwrap();
        assert!(seen(&rx1, &flat), "the flat watch missed a file in its folder");
        assert!(seen(&rx2, &flat), "the recursive watch missed a file in its folder");

        let nested = a_dir.join("deep").join("g.txt");
        std::fs::write(&nested, "x").unwrap();
        assert!(seen(&rx2, &nested), "the recursive watch missed a nested file");
        assert!(!rx1.try_iter().any(|p| p == nested), "the flat watch got a nested file");
        assert!(!rx3.try_iter().any(|p| p.starts_with(&a_dir)), "b's handle got a's events");

        drop(two);
        assert_eq!(hub().lock().unwrap().counts.get(&a_dir), Some(&1));
        let again = a_dir.join("h.txt");
        std::fs::write(&again, "x").unwrap();
        assert!(seen(&rx1, &again), "the flat watch stopped when the other handle dropped");

        drop(one);
        drop(three);
        assert!(!hub().lock().unwrap().counts.contains_key(&a_dir));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_next_stream_starts_where_a_folder_it_newly_covers_began_watching() {
        let (rebuild, _asks) = mpsc::channel();
        let (covered, fresh_dir) = (PathBuf::from("/covered"), PathBuf::from("/new"));
        let watch = |dir: &PathBuf, since| Watched {
            given: dir.clone(),
            real: dir.clone(),
            mode: RecursiveMode::NonRecursive,
            since,
        };
        let sub = |w: Watched| Sub {
            handler: Arc::new(Mutex::new(Box::new(|_: notify::Result<Event>| {}))),
            watched: vec![w],
        };
        let mut hub = Hub {
            subs: HashMap::from([(1, sub(watch(&covered, 10))), (2, sub(watch(&fresh_dir, 50)))]),
            counts: HashMap::from([(covered.clone(), 1), (fresh_dir.clone(), 1)]),
            rebuild,
            streamed: [covered.clone()].into_iter().collect(),
            next: 2,
        };
        // The covered folder's events moved the floor to 200 while /new was waiting to be added.
        assert_eq!(hub.pending(), Some(50));
        assert_eq!(start_point(200, hub.pending()), 50, "the new folder's changes from 50 to 200 would be lost");
        hub.streamed.insert(fresh_dir);
        assert_eq!(hub.pending(), None);
        assert_eq!(start_point(200, hub.pending()), 200);
    }

    #[test]
    fn a_change_made_while_another_folder_is_added_is_not_lost() {
        let root = fresh("gap");
        let (watched, other) = (root.join("watched"), root.join("other"));
        std::fs::create_dir_all(&watched).unwrap();
        let (tx, rx) = mpsc::channel::<(PathBuf, bool)>();
        let mut kept = watcher(move |e: notify::Result<Event>| {
            if let Ok(e) = e {
                let rescan = e.need_rescan();
                for p in e.paths {
                    let _ = tx.send((p, rescan));
                }
            }
        })
        .unwrap();
        kept.watch(&watched, RecursiveMode::NonRecursive).unwrap();

        // Each new folder restarts the stream; writes land while the streams swap.
        let mut handles = Vec::new();
        let mut written = Vec::new();
        for i in 0..20 {
            let file = watched.join(format!("f{i}.txt"));
            std::fs::write(&file, "x").unwrap();
            written.push(file);
            let dir = other.join(format!("d{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            let (tx, _rx) = mpsc::channel();
            let mut h = handle(tx);
            h.watch(&dir, RecursiveMode::NonRecursive).unwrap();
            handles.push(h);
        }
        // A change is delivered when its file is named, or when its folder is named as a rescan,
        // which is how FSEvents reports what it replays across a stream swap.
        let (mut got, mut rescanned) = (std::collections::HashSet::new(), false);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let covered = |got: &std::collections::HashSet<PathBuf>, rescanned: bool| {
            written.iter().all(|f| got.contains(f) || rescanned)
        };
        while !covered(&got, rescanned) {
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
        assert!(missed.is_empty(), "changes lost while streams swapped: {missed:?}");
        drop(handles);
        drop(kept);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
