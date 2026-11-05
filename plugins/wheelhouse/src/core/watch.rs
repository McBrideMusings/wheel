//! Noticing that a project's files changed. Only a project a host has subscribed to, for a view,
//! is watched; unsubscribing stops it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

struct Sub {
    uri: String,
    files: Vec<PathBuf>,
    seen: Vec<Option<SystemTime>>,
    every: u64,
    since: u64,
}

fn touched(files: &[PathBuf]) -> Vec<Option<SystemTime>> {
    files.iter().map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()).collect()
}

/// A subscription is due when one of its files moved, or `every` seconds passed without a check.
#[derive(Default)]
pub struct Watcher {
    subs: HashMap<(String, String), Sub>,
}

/// A subscription that is due: the view, the project and the URI to announce.
#[derive(Debug, PartialEq)]
pub struct Due {
    pub view: String,
    pub dir: String,
    pub uri: String,
}

impl Watcher {
    /// Watch `files` for `view` in project `dir`, announcing `uri`; `every` seconds is the longest
    /// it goes unchecked.
    pub fn watch(&mut self, view: &str, dir: &str, uri: &str, files: Vec<PathBuf>, every: u64) {
        let seen = touched(&files);
        self.subs.insert(
            (view.to_string(), dir.to_string()),
            Sub { uri: uri.to_string(), files, seen, every, since: 0 },
        );
    }

    pub fn unwatch(&mut self, view: &str, dir: &str) {
        self.subs.remove(&(view.to_string(), dir.to_string()));
    }

    /// Every subscription, as it is announced.
    pub fn subscriptions(&self) -> Vec<Due> {
        self.subs
            .iter()
            .map(|((view, dir), s)| Due { view: view.clone(), dir: dir.clone(), uri: s.uri.clone() })
            .collect()
    }

    pub fn watching(&self, view: &str, dir: &str) -> bool {
        self.subs.contains_key(&(view.to_string(), dir.to_string()))
    }

    /// One second passed: the subscriptions to refresh now.
    pub fn tick(&mut self) -> Vec<Due> {
        let mut due = vec![];
        for ((view, dir), s) in self.subs.iter_mut() {
            s.since += 1;
            let now = touched(&s.files);
            if now != s.seen || s.since >= s.every {
                s.seen = now;
                s.since = 0;
                due.push(Due { view: view.clone(), dir: dir.clone(), uri: s.uri.clone() });
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_file_or_the_interval_makes_a_project_due_and_only_a_watched_one() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("touched");
        std::fs::write(&file, "1").unwrap();
        let mut w = Watcher::default();
        let d = dir.to_string_lossy().into_owned();
        assert!(w.tick().is_empty(), "nothing is watched yet");
        w.watch("v", &d, "a2ui://v", vec![file.clone()], 3);
        assert!(w.watching("v", &d));
        assert!(w.tick().is_empty());
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(w.tick(), vec![Due { view: "v".into(), dir: d.clone(), uri: "a2ui://v".into() }]);
        assert!(w.tick().is_empty());
        assert!(w.tick().is_empty());
        assert_eq!(w.tick().len(), 1, "the third second without a check is due");
        w.unwatch("v", &d);
        assert!(!w.watching("v", &d));
        assert!(w.tick().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
