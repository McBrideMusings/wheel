//! A web pane's two-way channel with its page. The page writes values into a state store over HTTP
//! (`PUT .wheel/state/<key>`), and `wheel artifact state` reads them, optionally waiting for a key;
//! `wheel artifact data` pushes one JSON value to the page, which receives it on the
//! `.wheel/events` stream — the latest one first, so a page that loads later still has it.
//!
//! Both live in the pane's data directory as hidden files (`.state.json`, `.data.json`), so they
//! are never served as files, never reload the pane, survive a daemon restart and go when the
//! pane does.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;

use crate::server::AppState;

const STATE_FILE: &str = ".state.json";
const DATA_FILE: &str = ".data.json";

/// The largest value a page may store under one key, or the CLI may push.
pub(crate) const MAX_VALUE: usize = 1024 * 1024;

/// The most keys a page's state store holds; a new key past it is refused (an existing key may
/// still be replaced), so an untrusted page cannot grow the store and its file without limit.
pub(crate) const MAX_KEYS: usize = 256;

/// The longest a `wait_ms` is honored, the CLI's own limit.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(3600);

pub(crate) struct Page {
    pane: String,
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// Counts every change to the state or the pushed value; a waiter wakes on it.
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct Inner {
    state: BTreeMap<String, Value>,
    /// The latest pushed value and its sequence number (1 for the first push of this run).
    data: Option<(u64, Value)>,
    pushes: u64,
}

impl Page {
    fn load(pane: &str, dir: PathBuf) -> Page {
        let read = |file: &str| -> Option<Value> {
            let text = std::fs::read_to_string(dir.join(file)).ok()?;
            serde_json::from_str(&text)
                .inspect_err(|e| tracing::warn!(%pane, file, error = %e, "could not read the page's saved value; starting without it"))
                .ok()
        };
        let state: BTreeMap<String, Value> = read(STATE_FILE)
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let data = read(DATA_FILE).map(|v| (1, v));
        let pushes = data.is_some() as u64;
        tracing::info!(%pane, dir = %dir.display(), keys = ?state.keys().collect::<Vec<_>>(), has_data = data.is_some(), "page store opened");
        Page {
            pane: pane.to_string(),
            dir,
            inner: Mutex::new(Inner {
                state,
                data,
                pushes,
            }),
            changed: watch::channel(0).0,
        }
    }

    fn save(&self, file: &str, value: &impl serde::Serialize) {
        let tmp = self.dir.join(format!("{file}.tmp"));
        let result = serde_json::to_vec(value)
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(&tmp, bytes).map_err(|e| e.to_string()))
            .and_then(|()| std::fs::rename(&tmp, self.dir.join(file)).map_err(|e| e.to_string()));
        if let Err(e) = result {
            tracing::warn!(pane = %self.pane, file, error = %e, "could not save the page's value; it holds until the daemon stops");
        }
    }

    /// Store `value` under `key`.
    pub(crate) fn set(&self, key: &str, value: Value) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.state.contains_key(key) && inner.state.len() >= MAX_KEYS {
            return Err(format!("the page already stores {MAX_KEYS} keys"));
        }
        tracing::info!(pane = %self.pane, key, bytes = value.to_string().len(), "page state written");
        inner.state.insert(key.to_string(), value);
        self.save(STATE_FILE, &inner.state);
        drop(inner);
        self.changed.send_modify(|n| *n += 1);
        Ok(())
    }

    /// Push `value` to every open page and keep it for the pages that load later.
    pub(crate) fn push(&self, value: Value) {
        let mut inner = self.inner.lock().unwrap();
        inner.pushes += 1;
        tracing::info!(pane = %self.pane, seq = inner.pushes, bytes = value.to_string().len(), receivers = self.changed.receiver_count(), "page data pushed");
        self.save(DATA_FILE, &value);
        inner.data = Some((inner.pushes, value));
        drop(inner);
        self.changed.send_modify(|n| *n += 1);
    }

    /// One key's value, or all of them.
    pub(crate) fn values(&self, key: Option<&str>) -> BTreeMap<String, Value> {
        let inner = self.inner.lock().unwrap();
        match key {
            Some(key) => inner
                .state
                .get(key)
                .map(|v| (key.to_string(), v.clone()))
                .into_iter()
                .collect(),
            None => inner.state.clone(),
        }
    }

    /// Wait up to `wait` for `key` to be written; the values as they are then, and whether the
    /// wait ran out.
    pub(crate) async fn wait_for(
        &self,
        key: &str,
        wait: Duration,
    ) -> (BTreeMap<String, Value>, bool) {
        let mut rx = self.changed.subscribe();
        let deadline = tokio::time::Instant::now() + wait.min(MAX_WAIT);
        loop {
            let values = self.values(Some(key));
            if !values.is_empty() {
                return (values, false);
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return (self.values(Some(key)), true);
            }
        }
    }

    /// The pushed value newer than `seen`, if any.
    pub(crate) fn data_after(&self, seen: u64) -> Option<(u64, Value)> {
        self.inner
            .lock()
            .unwrap()
            .data
            .clone()
            .filter(|(seq, _)| *seq > seen)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

impl AppState {
    /// The page store of web pane `pane`, opened from its data directory on first use. An error
    /// when `pane` is not a web pane.
    pub(crate) fn page(&self, pane: &str) -> Result<Arc<Page>, String> {
        let is_web = self.spaces.lock().unwrap().spaces.iter().any(|s| {
            matches!(
                crate::spaces::find_pane(&s.root, pane),
                Some(wheel_proto::PaneKind::Web { .. })
            )
        });
        if !is_web {
            return Err(format!("{pane} is not a web pane"));
        }
        let dir = self.pane_data_dir(pane)?;
        let mut pages = self.pages.lock().unwrap();
        Ok(pages
            .entry(pane.to_string())
            .or_insert_with(|| Arc::new(Page::load(pane, dir)))
            .clone())
    }

    /// Forget web pane `pane`'s page store; its files go with the data directory.
    pub(crate) fn drop_page(&self, pane: &str) {
        self.pages.lock().unwrap().remove(pane);
    }
}
