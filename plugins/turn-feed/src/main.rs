//! `turn-feed backend`: the plugin backend `wheeld` runs for the `turn-feed` package.
//!
//! Agents post one card per turn with the `post` action (granted to agent panes by the manifest's
//! `[agent]` table); the pane answers a card with `answer`. The `posts` feed holds a project's
//! cards, newest first, and follows the store folder, so a post from any process shows.

mod store;

use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use store::Store;
use tokio::sync::mpsc::unbounded_channel;
use wheel_plugin_sdk::{ActionError, Backend};

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() != Some("backend") {
        eprintln!("usage: turn-feed backend");
        std::process::exit(2);
    }
    let mut backend = Backend::new();
    backend.feed("posts", |sub| async move {
        let Some(dir) = sub.params.get("dir").and_then(Value::as_str).map(str::to_string) else {
            sub.peer.log("warn", "posts feed needs params {dir}");
            return;
        };
        let store = Store::from_env();
        if let Err(e) = store.ensure() {
            sub.peer.log("error", format!("cannot create {}: {e}", store.dir().display()));
            return;
        }
        let (tx, mut rx) = unbounded_channel::<()>();
        let watcher = notify::recommended_watcher(move |_| {
            let _ = tx.send(());
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                sub.peer.log("error", format!("cannot watch: {e}"));
                return;
            }
        };
        if let Err(e) = watcher.watch(store.dir(), RecursiveMode::NonRecursive) {
            sub.peer.log("error", format!("cannot watch {}: {e}", store.dir().display()));
            return;
        }
        let mut last: Option<Value> = None;
        loop {
            let value = store.feed(&dir);
            if last.as_ref() != Some(&value) {
                sub.value(value.clone());
                last = Some(value);
            }
            if rx.recv().await.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            while rx.try_recv().is_ok() {}
        }
    });
    backend.action("post", |call| async move {
        Store::from_env().post(&call.args, &call.caller).map_err(ActionError::new)
    });
    backend.action("answer", |call| async move {
        Store::from_env().answer(&call.args, &call.caller).map_err(ActionError::new)
    });
    std::process::exit(backend.run().await);
}
