//! The global orchestrator's tasks as the Tasks views read them (ADR-0054). The folder's
//! `tools/on-event` writes `tasks/<id>.json`; wheeld only reads them, with the inbox events naming
//! each task's agents, writes the person's `done`, and tells clients when `tasks/` or `inbox/`
//! changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use wheel_proto::{InboxEvent, OrchestratorTask, ServerMsg};

use crate::server::AppState;

/// Events kept under one task: the newest.
const EVENTS_PER_TASK: usize = 50;

/// The watch on one folder; dropping it ends the watch.
pub(crate) struct Watch {
    dir: PathBuf,
    _watcher: notify::RecommendedWatcher,
}

/// The `.json` files directly in `dir`, by name; none when it does not exist.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    paths.sort();
    paths
}

/// A task file as a view shows it; None, and a log line, for one that does not parse.
fn task(path: &Path) -> Option<OrchestratorTask> {
    let parsed = std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string()))
        .and_then(|mut v| {
            let Some(fields) = v.as_object_mut() else { return Err("not an object".to_string()) };
            // The script writes a PR it could not read as a number as text; a view needs a number.
            if !fields.get("pr").is_some_and(serde_json::Value::is_u64) {
                fields.remove("pr");
            }
            serde_json::from_value::<OrchestratorTask>(v).map_err(|e| e.to_string())
        });
    match parsed {
        Ok(task) => Some(task),
        Err(error) => {
            tracing::warn!(file = %path.display(), %error, "task file does not parse; not shown");
            None
        }
    }
}

/// The tasks in folder `dir`, newest first, each with the inbox events naming its agents.
pub(crate) fn read(dir: &Path) -> Vec<OrchestratorTask> {
    let mut tasks: Vec<OrchestratorTask> = json_files(&dir.join("tasks")).iter().filter_map(|p| task(p)).collect();
    let mut events: Vec<(PathBuf, InboxEvent)> = ["new", "cur"]
        .iter()
        .flat_map(|box_| json_files(&dir.join("inbox").join(box_)))
        .filter_map(|p| {
            let event = serde_json::from_slice::<InboxEvent>(&std::fs::read(&p).ok()?).ok()?;
            Some((p, event))
        })
        .collect();
    // Names sort in the order wheeld wrote them, whichever box a file sits in.
    events.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
    for task in &mut tasks {
        let mut mine: Vec<InboxEvent> = events
            .iter()
            .filter(|(_, e)| e.agent.as_ref().is_some_and(|a| task.agents.contains(a)))
            .map(|(_, e)| e.clone())
            .collect();
        let skip = mine.len().saturating_sub(EVENTS_PER_TASK);
        task.events = mine.split_off(skip);
    }
    tasks.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| b.id.cmp(&a.id)));
    tasks
}

/// Write the person's `done` into task `id`'s file in folder `dir`, through a rename so the
/// script never reads half a file.
pub(crate) fn mark_done(dir: &Path, id: &str) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("{id:?} is not a task id"));
    }
    let path = dir.join("tasks").join(format!("{id}.json"));
    let bytes = std::fs::read(&path).map_err(|e| format!("no task {id}: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("task {id} does not parse: {e}"))?;
    let Some(fields) = value.as_object_mut() else { return Err(format!("task {id} is not an object")) };
    fields.insert("done".into(), serde_json::Value::Bool(true));
    let tmp = dir.join("tasks").join(format!(".{id}.json.done"));
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

impl AppState {
    /// The global orchestrator's tasks; starts the watch that tells clients when they change.
    pub(crate) fn orchestrator_tasks(&self) -> Result<Vec<OrchestratorTask>, String> {
        let dir = self.orchestrator_home()?;
        self.watch_tasks(&dir);
        Ok(read(&dir))
    }

    pub(crate) fn orchestrator_task_done(&self, id: &str) -> Result<(), String> {
        mark_done(&self.orchestrator_home()?, id)
    }

    /// Watch `dir` (made when missing), replacing a watch on another folder, and send every
    /// client `orchestrator_tasks_changed` once per burst of changes in it.
    fn watch_tasks(&self, dir: &Path) {
        let mut watch = self.tasks_watch.lock().unwrap();
        if watch.as_ref().is_some_and(|w| w.dir == dir) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let made = std::fs::create_dir_all(dir.join("tasks"))
            .and_then(|()| std::fs::create_dir_all(dir.join("inbox")))
            .map_err(|e| e.to_string())
            .and_then(|()| {
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if event.is_ok_and(|e| !e.kind.is_access()) {
                        let _ = tx.send(());
                    }
                })
                .map_err(|e| e.to_string())
            })
            .and_then(|mut w| {
                for sub in ["tasks", "inbox"] {
                    w.watch(&dir.join(sub), RecursiveMode::Recursive).map_err(|e| e.to_string())?;
                }
                Ok(w)
            });
        let watcher = match made {
            Ok(w) => w,
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "cannot watch the orchestrator's tasks; views update on reopen");
                return;
            }
        };
        let me = self.me.clone();
        runtime.spawn(async move {
            while rx.recv().await.is_some() {
                // One message per burst: the script rewrites several files per event.
                tokio::time::sleep(Duration::from_millis(200)).await;
                while rx.try_recv().is_ok() {}
                let Some(state) = me.upgrade() else { return };
                let _ = state.global.send(ServerMsg::OrchestratorTasksChanged {});
            }
        });
        tracing::info!(dir = %dir.display(), "orchestrator tasks watch started");
        *watch = Some(Watch { dir: dir.to_path_buf(), _watcher: watcher });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_read_with_their_agents_events_and_done_is_written() {
        let dir = std::env::temp_dir().join(format!("wheel-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["tasks", "inbox/new", "inbox/cur"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let write = |p: &str, v: serde_json::Value| std::fs::write(dir.join(p), v.to_string()).unwrap();
        write("tasks/t1.json", serde_json::json!({"id":"t1","asked":"Fix login","agents":["w1"],"created_ms":1,"status":"working","pr":"x","last_pushed":null}));
        write("tasks/t2.json", serde_json::json!({"id":"t2","asked":"Docs","agents":["w2"],"created_ms":2,"pr":12}));
        std::fs::write(dir.join("tasks/broken.json"), "{").unwrap();
        std::fs::write(dir.join("tasks/list.json"), "[]").unwrap();
        std::fs::write(dir.join("tasks/.t3.json.tmp"), "{}").unwrap();
        let event = |agent: &str, text: &str| serde_json::json!({"kind":"finished","orchestrator":"p1","agent":agent,"text":text,"at_ms":5});
        write("inbox/cur/0001.json", event("w1", "first"));
        write("inbox/new/0002.json", event("w1", "second"));
        write("inbox/new/0003.json", event("w2", "other"));

        let tasks = read(&dir);
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t2", "t1"], "newest first, the broken and hidden files left out");
        let texts: Vec<&str> = tasks[1].events.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"], "its agent's events from cur and new, in order");
        assert_eq!((tasks[0].pr, tasks[1].pr), (Some(12), None));

        mark_done(&dir, "t1").unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("tasks/t1.json")).unwrap()).unwrap();
        assert_eq!((saved["done"].as_bool(), saved["status"].as_str(), saved["pr"].as_str()), (Some(true), Some("working"), Some("x")));
        assert!(mark_done(&dir, "../t1").is_err());
        assert!(mark_done(&dir, "t9").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
