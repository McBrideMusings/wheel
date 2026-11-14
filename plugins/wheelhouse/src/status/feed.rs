//! The Project pane's data for `wheelhouse backend`: one project's status against its vision as the
//! `project` feed's value, and the `vision_save` action. The backend sends data only; the pane (Luau)
//! decides what to draw.

use serde_json::{json, Value};

use super::collect;

/// The `project` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    let snap = collect(dir);
    let c = &snap.counts;
    json!({
        "name": snap.name,
        "has_tracker": snap.has_tracker,
        "vision": snap.vision,
        "counts": { "done": c.done, "progress": c.progress, "ready": c.ready, "blocked": c.blocked, "open": c.open, "deferred": c.deferred },
        "epics": snap.epics.iter().map(|e| json!({ "id": e.id, "title": e.title, "done": e.done, "all": e.all, "ready": e.ready, "closed": e.closed })).collect::<Vec<_>>(),
        "error": snap.error,
    })
}

/// Write `text` as the vision of the project in `dir`; the words for the pane.
pub fn save_vision(dir: &str, text: &str) -> Result<String, String> {
    crate::core::tracker::set_vision(dir, text).map(|()| "Vision saved".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wheelhouse-project-feed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("docs")).unwrap();
        d
    }

    #[test]
    fn a_project_without_a_tracker_has_a_vision_and_no_error() {
        let d = temp("value");
        std::fs::write(d.join("docs/CONTEXT.md"), "# T\n\nWhy this exists.\n").unwrap();
        let v = value(d.to_str().unwrap());
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        for k in ["name", "has_tracker", "vision", "counts", "epics", "error"] {
            assert!(keys.contains(&k), "missing {k}");
        }
        assert_eq!(v["has_tracker"], false);
        assert_eq!(v["error"], Value::Null);
        assert_eq!(v["vision"], "Why this exists.");
        assert!(v.get("now").is_none());
        assert_eq!(v["counts"], json!({ "done": 0, "progress": 0, "ready": 0, "blocked": 0, "open": 0, "deferred": 0 }));
        assert_eq!(v["epics"], json!([]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saving_the_vision_writes_the_file_and_an_empty_one_is_refused() {
        let d = temp("save");
        std::fs::write(d.join("docs/CONTEXT.md"), "# T\n\nOld.\n\n## Terms\nx\n").unwrap();
        let dir = d.to_str().unwrap();
        assert_eq!(save_vision(dir, "New one").unwrap(), "Vision saved");
        assert_eq!(std::fs::read_to_string(d.join("docs/CONTEXT.md")).unwrap(), "# T\n\nNew one\n\n## Terms\nx\n");
        assert!(save_vision(dir, " ").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
