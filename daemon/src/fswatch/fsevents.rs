//! The macOS backend: one FSEvents stream per watched folder, every stream delivering on one
//! serial dispatch queue, and one router thread that drops the copy a second stream sends when a
//! watched folder lies inside another. A stream starts when its folder is first watched and stops
//! when the last watch of it ends; no stream is ever restarted to take in another folder, so
//! watching a new folder never interrupts the ones already watched.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};

use fsevent_sys as fs;
use fsevent_sys::core_foundation as cf;
use notify::event::{CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{EventKind, RecursiveMode};

/// One event as FSEvents gave it: the path, its flags and its id.
type Raw = (PathBuf, fs::FSEventStreamEventFlags, fs::FSEventStreamEventId);

pub(super) const KIND: notify::WatcherKind = notify::WatcherKind::Fsevent;

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

/// Every watched folder's stream, held under the hub's lock.
pub(super) struct Streams {
    /// Every watched folder, as the file system names it.
    folders: HashMap<PathBuf, Folder>,
    /// Where every stream's callback sends what it saw, for the router.
    events: mpsc::Sender<Vec<Raw>>,
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

/// Hand one FSEvents event to the hub, unless it is the end of a stream's history.
fn deliver((path, flags, _id): Raw) {
    if flags & fs::kFSEventStreamEventFlagHistoryDone != 0 {
        return;
    }
    // An event naming no change to an item is FSEvents saying something under a folder changed
    // without saying what (it dropped events, say): a rescan.
    let kind = kind(flags);
    super::route(path, kind, kind == EventKind::Other);
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

impl Streams {
    pub(super) fn new() -> Streams {
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
                        deliver(raw);
                    }
                }
            })
            .expect("could not start the file-event router");
        Streams { folders: HashMap::new(), events }
    }

    /// One more watch of `real`. An FSEvents stream always covers the whole tree, so `mode` only
    /// decides, in the hub, which events a watch is handed.
    pub(super) fn add(&mut self, real: &Path, _mode: RecursiveMode) -> notify::Result<()> {
        if let Some(folder) = self.folders.get_mut(real) {
            folder.count += 1;
            return Ok(());
        }
        let stream = start(real, &self.events)?;
        self.folders.insert(real.to_path_buf(), Folder { count: 1, stream });
        Ok(())
    }

    pub(super) fn remove(&mut self, real: &Path, _mode: RecursiveMode) {
        let Some(folder) = self.folders.get_mut(real) else { return };
        folder.count -= 1;
        if folder.count == 0 {
            if let Some(folder) = self.folders.remove(real) {
                stop(folder.stream);
            }
        }
    }

    /// Whether the system is watching `real` for the hub.
    #[cfg(test)]
    pub(super) fn watching(&self, real: &Path) -> bool {
        self.folders.contains_key(real)
    }
}
