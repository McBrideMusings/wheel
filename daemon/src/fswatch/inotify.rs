//! The Linux backend: one inotify instance and one reader thread for every watch. inotify watches a
//! directory, not a tree, and one instance holds one watch per directory however many watches
//! name it, so this keeps the hub's watches (a folder and whether it is watched with what lies
//! below it) and watches exactly the directories some watch covers: a recursive watch's folder
//! and every directory under it, a flat watch's folder alone. A directory stops being watched when
//! the last watch covering it ends, and one made under a recursive watch is watched as it appears.
//! A watched folder that is deleted or moved away is watched again when a folder appears at its
//! path, as an FSEvents stream, which follows the path, would see it: until then the nearest folder
//! above it that exists is watched, as an anchor, to see it come.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use notify::event::{CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{EventKind, RecursiveMode};

pub(super) const KIND: notify::WatcherKind = notify::WatcherKind::Inotify;

fn mask() -> WatchMask {
    WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MODIFY
        | WatchMask::ATTRIB
        | WatchMask::MOVED_FROM
        | WatchMask::MOVED_TO
        | WatchMask::DELETE_SELF
        | WatchMask::MOVE_SELF
        | WatchMask::DONT_FOLLOW
        | WatchMask::ONLYDIR
}

struct State {
    watches: Watches,
    /// Every watched directory and its descriptor, both ways.
    dirs: HashMap<PathBuf, WatchDescriptor>,
    paths: HashMap<WatchDescriptor, PathBuf>,
    /// The hub's watches: how many name each folder, flat or recursive.
    roots: HashMap<(PathBuf, bool), usize>,
    /// Folders watched only to see a missing watched folder come back.
    anchors: HashSet<PathBuf>,
    /// Rescans found under the hub's lock, which cannot route them, handed over with the next event.
    pending: Vec<Out>,
    /// Why inotify stopped reading, after which no watch can be made.
    dead: Option<String>,
}

/// One event for the hub: its path, its kind and whether it is a rescan.
type Out = (PathBuf, EventKind, bool);

impl State {
    fn new(watches: Watches) -> State {
        State {
            watches,
            dirs: HashMap::new(),
            paths: HashMap::new(),
            roots: HashMap::new(),
            anchors: HashSet::new(),
            pending: Vec::new(),
            dead: None,
        }
    }

    /// End one of the hub's watches of `real`, and stop watching what no watch covers now. A watched
    /// folder this finds back is watched again, and its rescan waits in `pending`.
    fn drop_root(&mut self, real: &Path, deep: bool) {
        let key = (real.to_path_buf(), deep);
        let Some(count) = self.roots.get_mut(&key) else { return };
        *count -= 1;
        if *count == 0 {
            self.roots.remove(&key);
            self.uncover(real);
            let rescans = self.repair();
            self.pending.extend(rescans);
        }
    }

    /// Whether some watch needs `dir` watched: it is a watched folder, or lies under a recursive one.
    fn covered(&self, dir: &Path) -> bool {
        self.roots.keys().any(|(root, deep)| dir == root || (*deep && dir.starts_with(root)))
    }

    /// Whether a recursive watch covers `dir`, so what is made in it is watched too.
    fn deep(&self, dir: &Path) -> bool {
        self.roots.keys().any(|(root, deep)| *deep && dir.starts_with(root))
    }

    fn watch(&mut self, dir: &Path) -> std::io::Result<()> {
        if self.dirs.contains_key(dir) {
            return Ok(());
        }
        let wd = self.watches.add(dir, mask())?;
        self.dirs.insert(dir.to_path_buf(), wd.clone());
        self.paths.insert(wd, dir.to_path_buf());
        Ok(())
    }

    /// Watch `dir`, and with `deep` every directory under it. Returns what was found below `dir`, so
    /// a folder made a moment ago reports what was made in it before its watch began. A folder below
    /// that cannot be watched (unreadable, say) is skipped; running out of watches
    /// (`fs.inotify.max_user_watches`) fails, since the tree would be watched only in part.
    fn cover(&mut self, dir: &Path, deep: bool) -> std::io::Result<Vec<PathBuf>> {
        self.watch(dir)?;
        let mut found = Vec::new();
        if !deep {
            return Ok(found);
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(at) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&at) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                // Symlinks are not followed, as no FSEvents stream follows them either.
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if let Err(e) = self.watch(&path) {
                        if e.raw_os_error() == Some(libc::ENOSPC) {
                            return Err(e);
                        }
                        tracing::warn!(dir = %path.display(), error = %e, "inotify would not watch a folder");
                        continue;
                    }
                    stack.push(path.clone());
                }
                found.push(path);
            }
        }
        Ok(found)
    }

    /// Stop watching each directory at or under `dir` that no watch covers any more.
    fn uncover(&mut self, dir: &Path) {
        let gone: Vec<PathBuf> = self
            .dirs
            .keys()
            .filter(|d| d.starts_with(dir) && !self.covered(d) && !self.anchors.contains(*d))
            .cloned()
            .collect();
        for d in gone {
            self.forget(&d, true);
        }
    }

    /// Drop the watches of `dir` and everything under it: a folder moved away keeps its watches under
    /// a path that no longer names it.
    fn forget_tree(&mut self, dir: &Path) {
        let gone: Vec<PathBuf> = self.dirs.keys().filter(|d| d.starts_with(dir)).cloned().collect();
        for d in gone {
            self.forget(&d, true);
        }
    }

    /// Watch every watched folder that has lost its watch and exists again, with a rescan of it so
    /// its watchers read it afresh, and anchor the ones still missing at the nearest folder above.
    fn repair(&mut self) -> Vec<Out> {
        let missing: Vec<(PathBuf, bool)> =
            self.roots.keys().filter(|(root, _)| !self.dirs.contains_key(root)).cloned().collect();
        let (mut out, mut wanted) = (Vec::new(), HashSet::new());
        for (root, deep) in missing {
            if root.is_dir() {
                match self.cover(&root, deep) {
                    Ok(_) => {
                        out.push((root, EventKind::Other, true));
                        continue;
                    }
                    // Not left watched in part: it stays missing, anchored above, and is tried again.
                    Err(e) => {
                        tracing::error!(dir = %root.display(), error = %e, "inotify would not watch a folder again");
                        self.forget_tree(&root);
                    }
                }
            }
            if let Some(above) = root.ancestors().skip(1).find(|a| a.is_dir()) {
                match self.watch(above) {
                    Ok(()) => drop(wanted.insert(above.to_path_buf())),
                    Err(e) => tracing::error!(dir = %above.display(), error = %e, "inotify would not watch a folder"),
                }
            }
        }
        let stale: Vec<PathBuf> = self.anchors.difference(&wanted).cloned().collect();
        self.anchors = wanted;
        for dir in stale {
            if !self.covered(&dir) && !self.anchors.contains(&dir) {
                self.forget(&dir, true);
            }
        }
        out
    }

    /// After the kernel dropped events: watch the folders made under recursive watches meanwhile,
    /// and the watched folders that came back.
    fn recover(&mut self) {
        let deep: Vec<PathBuf> = self
            .roots
            .keys()
            .filter(|(root, deep)| *deep && self.dirs.contains_key(root))
            .map(|(root, _)| root.clone())
            .collect();
        for root in deep {
            if let Err(e) = self.cover(&root, true) {
                // Dropped whole, so the repair below anchors it and tries again rather than leaving it in part.
                tracing::error!(dir = %root.display(), error = %e, "inotify would not watch a folder");
                self.forget_tree(&root);
            }
        }
        self.repair();
    }

    /// Drop `dir`'s watch, removing it from inotify unless the kernel already did.
    fn forget(&mut self, dir: &Path, remove: bool) {
        if let Some(wd) = self.dirs.remove(dir) {
            self.paths.remove(&wd);
            if remove {
                let _ = self.watches.remove(wd);
            }
        }
    }
}

/// The shared inotify instance, held under the hub's lock; its reader thread takes only `state`.
/// `Err` holds why inotify would not start, and every watch fails with it.
pub(super) struct Streams {
    state: Result<Arc<Mutex<State>>, String>,
}

fn kind(mask: EventMask) -> EventKind {
    if mask.contains(EventMask::CREATE) {
        EventKind::Create(CreateKind::Any)
    } else if mask.intersects(EventMask::DELETE | EventMask::DELETE_SELF) {
        EventKind::Remove(RemoveKind::Any)
    } else if mask.intersects(EventMask::MOVED_FROM | EventMask::MOVED_TO | EventMask::MOVE_SELF) {
        EventKind::Modify(ModifyKind::Name(RenameMode::Any))
    } else if mask.contains(EventMask::MODIFY) {
        EventKind::Modify(ModifyKind::Data(DataChange::Any))
    } else if mask.contains(EventMask::ATTRIB) {
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))
    } else {
        EventKind::Other
    }
}

/// What one inotify event means: the events to hand the hub, after the watched directories are
/// brought up to date with it, after the rescans waiting since the last event.
fn read(state: &Mutex<State>, wd: &WatchDescriptor, mask: EventMask, name: Option<&OsStr>) -> Vec<Out> {
    let mut state = state.lock().unwrap();
    let mut out = std::mem::take(&mut state.pending);
    if mask.contains(EventMask::Q_OVERFLOW) {
        // The kernel dropped events: every watch rescans, named as the folder it watches.
        state.recover();
        out.push((PathBuf::from("/"), EventKind::Other, true));
        return out;
    }
    let Some(dir) = state.paths.get(wd).cloned() else { return out };
    if mask.contains(EventMask::IGNORED) {
        // The kernel ended the watch: the directory is gone, or this removed it.
        state.forget(&dir, false);
        out.extend(state.repair());
        return out;
    }
    let path = name.map_or_else(|| dir.clone(), |n| dir.join(n));
    out.push((path.clone(), kind(mask), false));
    if mask.contains(EventMask::MOVE_SELF) {
        state.forget_tree(&dir);
    } else if mask.contains(EventMask::ISDIR) {
        if mask.intersects(EventMask::CREATE | EventMask::MOVED_TO) && state.deep(&path) {
            match state.cover(&path, true) {
                Ok(found) => out.extend(found.into_iter().map(|p| (p, EventKind::Create(CreateKind::Any), false))),
                Err(e) => tracing::error!(dir = %path.display(), error = %e, "inotify would not watch a folder"),
            }
        } else if mask.contains(EventMask::MOVED_FROM) {
            state.forget_tree(&path);
        }
    }
    if mask.intersects(EventMask::ISDIR | EventMask::MOVE_SELF) {
        out.extend(state.repair());
    }
    out
}

/// The reader thread: hand every event to the hub until inotify fails to read, then mark the
/// state dead so every later watch fails rather than watching nothing.
fn route_events(mut inotify: Inotify, state: &Mutex<State>) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => events,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                tracing::error!(error = %e, "inotify stopped; files are no longer watched");
                state.lock().unwrap().dead = Some(format!("inotify stopped: {e}"));
                return;
            }
        };
        for event in events {
            // Handed over outside the state's lock: a handler may watch or unwatch.
            for (path, kind, rescan) in read(state, &event.wd, event.mask, event.name) {
                super::route(path, kind, rescan);
            }
        }
    }
}

impl Streams {
    pub(super) fn new() -> Streams {
        let started = Inotify::init().map_err(|e| format!("inotify would not start: {e}")).and_then(|inotify| {
            let state = Arc::new(Mutex::new(State::new(inotify.watches())));
            let reader = state.clone();
            std::thread::Builder::new()
                .name("wheel inotify route".into())
                .spawn(move || route_events(inotify, &reader))
                .map_err(|e| format!("the file-event router would not start: {e}"))?;
            Ok(state)
        });
        if let Err(e) = &started {
            tracing::error!(error = %e, "files cannot be watched");
        }
        Streams { state: started }
    }

    pub(super) fn add(&mut self, real: &Path, mode: RecursiveMode) -> notify::Result<()> {
        let fail = |why: &str| notify::Error::generic(why).add_path(real.to_path_buf());
        let deep = mode == RecursiveMode::Recursive;
        let mut state = self.state.as_ref().map_err(|e| fail(e))?.lock().unwrap();
        if let Some(why) = &state.dead {
            return Err(fail(why));
        }
        *state.roots.entry((real.to_path_buf(), deep)).or_default() += 1;
        if let Err(e) = state.cover(real, deep) {
            state.drop_root(real, deep);
            return Err(notify::Error::io(e).add_path(real.to_path_buf()));
        }
        Ok(())
    }

    pub(super) fn remove(&mut self, real: &Path, mode: RecursiveMode) {
        if let Ok(state) = &self.state {
            state.lock().unwrap().drop_root(real, mode == RecursiveMode::Recursive);
        }
    }

    /// Whether inotify is watching `real` for the hub.
    #[cfg(test)]
    pub(super) fn watching(&self, real: &Path) -> bool {
        self.state.as_ref().is_ok_and(|s| s.lock().unwrap().dirs.contains_key(real))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_found_back_by_an_unwatch_rescans_with_the_next_event() {
        let root = std::env::temp_dir().join(format!("wheel-inotify-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let inotify = Inotify::init().unwrap();
        let mut state = State::new(inotify.watches());
        for dir in [&a, &b] {
            state.roots.insert((dir.clone(), false), 1);
            state.cover(dir, false).unwrap();
        }
        let b_wd = state.dirs[&b].clone();

        // `a` goes and its loss is read; it comes back before its anchor's event is read.
        std::fs::remove_dir(&a).unwrap();
        state.forget(&a, true);
        state.repair();
        std::fs::create_dir(&a).unwrap();
        state.drop_root(&b, false);
        assert!(state.dirs.contains_key(&a), "the unwatch did not watch the folder that came back");

        let state = Mutex::new(state);
        let out = read(&state, &b_wd, EventMask::MODIFY, None);
        assert_eq!(out, vec![(a.clone(), EventKind::Other, true)]);
        assert!(read(&state, &b_wd, EventMask::MODIFY, None).is_empty(), "the rescan was handed over twice");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn every_watch_fails_when_inotify_cannot_start_or_has_stopped() {
        let dir = std::env::temp_dir();
        let mut never = Streams { state: Err("inotify would not start: EMFILE".into()) };
        assert!(never.add(&dir, RecursiveMode::NonRecursive).is_err());
        never.remove(&dir, RecursiveMode::NonRecursive);

        let mut state = State::new(Inotify::init().unwrap().watches());
        state.dead = Some("inotify stopped".into());
        let mut stopped = Streams { state: Ok(Arc::new(Mutex::new(state))) };
        assert!(stopped.add(&dir, RecursiveMode::NonRecursive).is_err());
        assert!(!stopped.watching(&dir));
    }
}
