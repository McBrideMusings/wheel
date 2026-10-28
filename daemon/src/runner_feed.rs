//! The runner built-in's list, kept current. While a web pane shows the machine's `runner`
//! artifact, wheeld watches this machine's `wheel.toml` and the pane's space's, and pushes the
//! `[runner]` list to the page whenever either changes — the same list `wheel run --json` prints,
//! so an edit by a person, an editor or an agent shows without a Reload.

use std::path::Path;

use tokio::task::JoinHandle;
use wheel_proto::{ArtifactRef, ArtifactScope};

use crate::server::AppState;
use crate::settings::FILE;

/// The built-in whose page lists the `[runner]` table.
const RUNNER: &str = "runner";

/// One pane's watch of the two `wheel.toml` files. Dropping it stops the watch.
pub(crate) struct Feed(JoinHandle<()>);

impl Drop for Feed {
    fn drop(&mut self) {
        // The task owns the watcher, so ending it stops the watch.
        self.0.abort();
    }
}

impl AppState {
    /// Start feeding web pane `pane` the `[runner]` list of the space whose directory is `cwd`
    /// when it shows the runner built-in, replacing any earlier feed of that pane; stop it when it
    /// shows anything else. Call from inside the tokio runtime.
    pub(crate) fn start_runner_feed(&self, pane: &str, artifact: &ArtifactRef, cwd: &str) {
        let mut feeds = self.runner_feeds.lock().unwrap();
        feeds.remove(pane);
        if artifact.scope() != ArtifactScope::Machine || artifact.name() != RUNNER {
            return;
        }
        let (machine, project) = (self.instance_dir.join(FILE), Path::new(cwd).join(FILE));
        let (mut watcher, mut rx) = match crate::web::watch_files(&[&machine, &project]) {
            Ok(watch) => watch,
            Err(e) => {
                tracing::warn!(%pane, error = %e, "could not watch the wheel.toml files; the runner list updates only on Reload");
                return;
            }
        };
        tracing::info!(%pane, machine = %machine.display(), project = %project.display(), "runner feed watching");
        let (me, id) = (self.me.clone(), pane.to_string());
        let task = tokio::spawn(async move {
            // Held for the task's life: dropping it ends the watch.
            let _watcher = &mut watcher;
            while rx.recv().await.is_some() {
                // A save is often several writes; one push covers them.
                tokio::time::sleep(crate::web::DEBOUNCE).await;
                while rx.try_recv().is_ok() {}
                let Some(state) = me.upgrade() else { return };
                let pane = id.clone();
                // The list is two file reads, off the runtime's threads.
                let _ = tokio::task::spawn_blocking(move || state.push_runners(&pane)).await;
            }
        });
        feeds.insert(pane.to_string(), Feed(task));
    }

    /// Stop feeding web pane `pane` the `[runner]` list.
    pub(crate) fn stop_runner_feed(&self, pane: &str) {
        if self.runner_feeds.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "runner feed stopped");
        }
    }

    /// Push the `[runner]` list of web pane `pane`'s space to its page.
    fn push_runners(&self, pane: &str) {
        let Some((_, _, space)) = self.web_pane_artifact(pane) else {
            return;
        };
        let pushed = self.runner_ls(Some(&space)).and_then(|runners| {
            let count = runners.len();
            let value = serde_json::to_value(runners).map_err(|e| e.to_string())?;
            self.page(pane)?.push(value);
            Ok(count)
        });
        match pushed {
            Ok(count) => tracing::info!(%pane, %space, count, "runner list pushed"),
            Err(e) => tracing::warn!(%pane, %space, error = %e, "could not push the runner list"),
        }
    }
}
