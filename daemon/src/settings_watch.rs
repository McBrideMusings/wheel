//! A hand edit of a `wheel.toml` reaches clients. wheeld watches this machine's file and the
//! `wheel.toml` of every open space, and when one changes sends every client the same `Settings`
//! message a `setting_set` sends, so the chat's text size and the Settings window follow an
//! editor, a `sed` or an agent without a restart.
//!
//! The change is found by comparing what each scope resolves to now with what clients were last
//! told, so a write wheeld made itself (already broadcast by `setting_set`) and a save that changes
//! nothing send nothing. A file that no longer parses keeps the values clients last saw and
//! carries the parse error as each setting's `problem`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wheel_proto::{ServerMsg, SettingInfo};

use crate::server::AppState;
use crate::settings::FILE;
use crate::spaces::Spaces;

/// The running watch: the directories it covers, and the task that owns its watchers.
pub(crate) struct Watch {
    spaces: Vec<(String, String)>,
    task: JoinHandle<()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        // The task owns the watchers, so ending it stops the watch.
        self.task.abort();
    }
}

/// What each scope (`None` is the machine view, `Some(id)` a space) last showed clients.
pub(crate) type Seen = HashMap<Option<String>, Vec<SettingInfo>>;

impl AppState {
    /// Watch the files for the spaces that exist now. Call from inside the tokio runtime.
    pub fn sync_settings_watch(&self) {
        let spaces = self.spaces.lock().unwrap();
        self.sync_settings_watch_with(&spaces);
    }

    /// [`AppState::sync_settings_watch`] with the spaces lock already held: restarts the watch
    /// when the set of spaces and their directories is not the one being watched.
    pub(crate) fn sync_settings_watch_with(&self, spaces: &Spaces) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut now: Vec<(String, String)> = spaces
            .spaces
            .iter()
            .map(|s| (s.id.clone(), s.cwd.clone()))
            .collect();
        now.sort();
        let mut watch = self.settings_watch.lock().unwrap();
        if watch.as_ref().is_some_and(|w| w.spaces == now) {
            return;
        }
        let machine = self.machine_file();
        let mut files = vec![machine.clone()];
        files.extend(now.iter().map(|(_, cwd)| Path::new(cwd).join(FILE)));
        let (mut watchers, mut rx) = match watch_paths(&files) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "could not watch the wheel.toml files; a hand edit shows only after a restart");
                return;
            }
        };
        tracing::info!(files = files.len(), machine = %machine.display(), "settings watch started");
        let me = self.me.clone();
        let task = runtime.spawn(async move {
            // Held for the task's life: dropping it ends the watch.
            let _watchers = &mut watchers;
            let Some(state) = me.upgrade() else { return };
            // The baseline: what clients would be told now, so the first edit is a difference.
            {
                let state = state.clone();
                let _ = tokio::task::spawn_blocking(move || state.settings_baseline()).await;
            }
            drop(state);
            while let Some(first) = rx.recv().await {
                let mut touched = BTreeSet::from([first]);
                // A save is often several writes; one look covers them.
                tokio::time::sleep(crate::web::DEBOUNCE).await;
                while let Ok(path) = rx.try_recv() {
                    touched.insert(path);
                }
                let Some(state) = me.upgrade() else { return };
                let _ = tokio::task::spawn_blocking(move || state.settings_files_changed(&touched))
                    .await;
            }
        });
        *watch = Some(Watch { spaces: now, task });
    }

    /// Remember what every scope resolves to now, for scopes not yet remembered.
    fn settings_baseline(&self) {
        let scopes = self.settings_scopes();
        self.settings_seen.lock().unwrap().retain(|k, _| scopes.contains(k));
        for scope in scopes {
            if self.settings_seen.lock().unwrap().contains_key(&scope) {
                continue;
            }
            if let Ok((_, list)) = self.settings_ls(scope.as_deref()) {
                self.settings_seen.lock().unwrap().insert(scope, list);
            }
        }
    }

    /// The machine view and every space.
    fn settings_scopes(&self) -> Vec<Option<String>> {
        let spaces = self.spaces.lock().unwrap();
        std::iter::once(None)
            .chain(spaces.spaces.iter().map(|s| Some(s.id.clone())))
            .collect()
    }

    /// `touched` changed on disk: tell every client what now differs, and log it.
    fn settings_files_changed(&self, touched: &BTreeSet<PathBuf>) {
        let machine = self.machine_file();
        let mut changed_keys: BTreeSet<String> = BTreeSet::new();
        let mut problems: Vec<String> = Vec::new();
        for scope in self.settings_scopes() {
            let Ok((_, fresh)) = self.settings_ls(scope.as_deref()) else {
                continue;
            };
            let mut seen_all = self.settings_seen.lock().unwrap();
            let seen = seen_all.get(&scope).cloned();
            let list = match (&seen, self.settings_broken(scope.as_deref(), &machine)) {
                // The file no longer parses: keep what clients last saw, and say why.
                (Some(seen), Some(why)) => {
                    problems.push(why.clone());
                    seen.iter()
                        .cloned()
                        .map(|mut s| {
                            s.problem = Some(why.clone());
                            s
                        })
                        .collect()
                }
                _ => fresh,
            };
            let differ: Vec<String> = match &seen {
                Some(seen) => list
                    .iter()
                    .zip(seen.iter())
                    .filter(|(a, b)| a != b)
                    .map(|(a, _)| a.key.clone())
                    .collect(),
                None => Vec::new(),
            };
            seen_all.insert(scope.clone(), list.clone());
            drop(seen_all);
            if seen.is_some() && !differ.is_empty() {
                changed_keys.extend(differ);
                let _ = self.global.send(ServerMsg::Settings {
                    req: None,
                    space: scope,
                    settings: list,
                });
            }
        }
        let paths = touched
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let keys = changed_keys.into_iter().collect::<Vec<_>>().join(", ");
        if keys.is_empty() {
            tracing::info!(path = %paths, "settings file touched; no setting changed");
        } else {
            tracing::info!("settings file changed {paths}: {keys}");
        }
        for why in problems {
            tracing::warn!(%why, "a wheel.toml does not parse; the last good settings stay in force");
        }
    }

    /// Why a file feeding `scope` cannot be used, when one cannot be read or parsed.
    fn settings_broken(&self, scope: Option<&str>, machine: &Path) -> Option<String> {
        let mut files = vec![machine.to_path_buf()];
        if let Ok(Some((_, cwd))) = self.settings_space(scope) {
            files.push(Path::new(&cwd).join(FILE));
        }
        files.iter().find_map(|path| {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
                Err(e) => return Some(format!("{} cannot be read: {e}", path.display())),
            };
            toml::from_str::<toml::Table>(&text)
                .err()
                .map(|e| format!("{} is not valid TOML: {e}", path.display()))
        })
    }

    /// Record what a settings reply or broadcast told clients, so the watch does not tell them
    /// again about a write wheeld made itself.
    pub(crate) fn settings_told(&self, space: &Option<String>, settings: &[SettingInfo]) {
        self.settings_seen
            .lock()
            .unwrap()
            .insert(space.clone(), settings.to_vec());
        // A machine-wide write changes what every other space resolves to as well; they are
        // remembered as they now are, so the watch does not send the same write again.
        for scope in self.settings_scopes() {
            if &scope != space {
                if let Ok((_, list)) = self.settings_ls(scope.as_deref()) {
                    self.settings_seen.lock().unwrap().insert(scope, list);
                }
            }
        }
    }
}

/// Watch the folders holding `files`, sending the path of each file touched. Folders rather than
/// files, because an editor replaces a file by renaming a new one onto it.
fn watch_paths(
    files: &[PathBuf],
) -> Result<
    (
        Vec<notify::RecommendedWatcher>,
        mpsc::UnboundedReceiver<PathBuf>,
    ),
    String,
> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut watchers = Vec::new();
    let mut folders = HashSet::new();
    let mut real: HashMap<PathBuf, PathBuf> = HashMap::new();
    for file in files {
        let Some(parent) = file.parent() else {
            continue;
        };
        let Ok(parent) = parent.canonicalize() else {
            // A space whose directory is gone has nothing to watch.
            continue;
        };
        let Some(name) = file.file_name() else {
            continue;
        };
        real.insert(parent.join(name), file.clone());
        folders.insert(parent);
    }
    for folder in folders {
        let (tx, real) = (tx.clone(), real.clone());
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                for path in &event.paths {
                    if let Some(file) = real.get(path) {
                        let _ = tx.send(file.clone());
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        watcher
            .watch(&folder, RecursiveMode::NonRecursive)
            .map_err(|e| format!("{}: {e}", folder.display()))?;
        watchers.push(watcher);
    }
    Ok((watchers, rx))
}
