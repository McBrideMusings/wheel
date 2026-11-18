//! `turn-feed backend`: the plugin backend `wheeld` runs for the `turn-feed` package.
//!
//! Agents post one card per turn with the `post` action (granted to agent panes by the manifest's
//! `[agent]` table); the pane answers a card with `answer`. The `posts` feed holds one space's
//! cards, newest first, and follows the store folder and each markdown page a card draws, so a post
//! from any process, or an edit to its page, shows.

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
        let Some(space) = sub.params.get("space").and_then(Value::as_str).map(str::to_string) else {
            sub.peer.log("warn", "posts feed needs params {space}");
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
        // The folders of the markdown pages the feed draws, watched so an edit to a page redraws its post.
        let mut pages: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            let value = store.feed(&space);
            for dir in value["posts"].as_array().into_iter().flatten().filter_map(|p| p["page_dir"].as_str()) {
                if !pages.contains(dir) && watcher.watch(std::path::Path::new(dir), RecursiveMode::NonRecursive).is_ok() {
                    pages.insert(dir.to_string());
                }
            }
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
    backend.action("delivered", |call| async move {
        Store::from_env().delivered(&call.args).map_err(ActionError::new)
    });
    backend.action("delete", |call| async move {
        Store::from_env().delete(&call.args, &call.caller).map_err(ActionError::new)
    });
    // Shows the agent pane that wrote a post, through `wheel pane focus` (the backend runs with `wheel` on its PATH).
    backend.action("focus", |call| async move {
        let pane = Store::from_env().author(&call.args, &call.caller).map_err(ActionError::new)?;
        let out = tokio::process::Command::new("wheel")
            .args(["pane", "focus", &pane])
            .output()
            .await
            .map_err(|e| ActionError::new(format!("cannot run wheel pane focus: {e}")))?;
        if !out.status.success() {
            return Err(ActionError::new(format!("wheel pane focus {pane}: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        Ok(serde_json::json!({ "pane": pane }))
    });
    std::process::exit(backend.run().await);
}
