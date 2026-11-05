//! What the agents Wheel is running are doing, read from `wheel status --json`.

use serde_json::Value;

use super::run;

/// One agent pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub space: String,
    pub pane: String,
    pub harness: String,
    pub model: String,
    /// `busy`, `idle`, `waiting`: what `state.phase` says.
    pub phase: String,
}

/// The agent panes in the spaces whose directory is `dir`.
pub fn in_project(dir: &str) -> Result<Vec<Agent>, String> {
    let status = run::json("wheel", dir, &["status", "--json"])?;
    Ok(from_status(&status, dir))
}

/// The agent panes of `status` in the spaces whose directory is `dir`.
pub fn from_status(status: &Value, dir: &str) -> Vec<Agent> {
    let mut out = vec![];
    let dir = dir.trim_end_matches('/');
    for space in status["spaces"].as_array().into_iter().flatten() {
        let cwd = space["cwd"].as_str().unwrap_or("").trim_end_matches('/');
        if cwd == dir {
            panes(&space["root"], space["name"].as_str().unwrap_or(""), &mut out);
        }
    }
    out
}

fn panes(node: &Value, space: &str, out: &mut Vec<Agent>) {
    match node["node"].as_str() {
        Some("split") => {
            panes(&node["a"], space, out);
            panes(&node["b"], space, out);
        }
        Some("pane") if node["kind"] == "agent" => out.push(Agent {
            space: space.to_string(),
            pane: node["id"].as_str().unwrap_or("").to_string(),
            harness: node["harness"].as_str().unwrap_or("").to_string(),
            model: node["model"].as_str().unwrap_or("").to_string(),
            phase: node["state"]["phase"].as_str().unwrap_or("").to_string(),
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agents_of_a_project_are_found_through_splits() {
        let status = json!({ "spaces": [
            { "name": "wheel", "cwd": "/p/wheel", "root": { "node": "split",
                "a": { "node": "pane", "kind": "agent", "id": "p1", "harness": "claude", "model": "sonnet",
                       "state": { "phase": "busy" } },
                "b": { "node": "pane", "kind": "web", "id": "p2" } } },
            { "name": "other", "cwd": "/p/other", "root": { "node": "pane", "kind": "agent", "id": "p3" } },
        ] });
        let got = from_status(&status, "/p/wheel/");
        assert_eq!(got, vec![Agent { space: "wheel".into(), pane: "p1".into(), harness: "claude".into(),
            model: "sonnet".into(), phase: "busy".into() }]);
    }
}
