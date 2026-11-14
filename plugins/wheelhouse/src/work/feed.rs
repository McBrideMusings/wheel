//! The Implement and Swarm panes' data for `wheelhouse backend`: the agent panes of one project as
//! the `work` feed's value. The backend sends data only; the pane (Luau) decides what to draw.

use serde_json::{json, Value};

use crate::core::agents::{in_project, Agent};

/// The `work` feed's value for the project in `dir`, given every agent pane `all` that
/// `wheel agent watch` listed, whether it has delivered a list yet and why it could not run, if it could not.
pub fn value(dir: &str, all: &[Agent], loaded: bool, error: Option<String>) -> Value {
    json!({
        "name": crate::core::project::name_of(dir),
        "loaded": loaded,
        "error": error,
        "agents": super::rows(&in_project(all, dir)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agents::{Work, WorkItem};

    fn agent(cwd: &str, pane: &str, work: Option<Work>) -> Agent {
        Agent { space: "s".into(), cwd: cwd.into(), pane: pane.into(), agent: "w1".into(), harness: "claude".into(), model: Some("sonnet".into()),
            status: Some("working".into()), work }
    }

    #[test]
    fn the_value_holds_the_projects_rows_only() {
        let work = Work { items: vec![WorkItem { kind: "bead".into(), id: "a".into() }], stage: Some("edit".into()), note: Some("n".into()) };
        let all = [agent("/p/wheel", "p1", Some(work)), agent("/p/wheel", "p2", None), agent("/p/other", "p3", None)];
        let v = value("/p/wheel/", &all, true, None);
        assert_eq!((v["name"].as_str(), v["loaded"].as_bool()), (Some("wheel"), Some(true)));
        assert_eq!(v["agents"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["agents"][0],
            json!({ "pane": "p1", "space": "s", "agent": "w1", "model": "sonnet", "status": "working", "item": "bead a", "stage": "edit", "note": "n" })
        );
        assert_eq!(v["agents"][1]["item"], Value::Null);
        assert_eq!(value("/p/wheel", &[], false, None), json!({ "name": "wheel", "loaded": false, "error": null, "agents": [] }));
        assert_eq!(value("/p/wheel", &[], true, Some("boom".into()))["error"], "boom");
    }
}
