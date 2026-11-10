//! A view: one artifact's page, its tools and the files that make it stale. `wheelhouse serve`
//! holds a registry of views and answers each request from the view the host names in
//! `_meta` `wheel/artifact`, so one process serves every artifact (ADR-0028).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResponse, Tool};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::Value;

use crate::core::agents::Agent;

/// MIME type of the A2UI messages a page is drawn from.
pub const MIME: &str = "application/a2ui+json";

#[async_trait]
pub trait View: Send + Sync {
    /// The artifact name the host sends in `wheel/artifact`; also the host of the page's URI,
    /// `a2ui://<name>`.
    fn name(&self) -> &str;

    /// The tools the page calls.
    fn tools(&self) -> Vec<Tool>;

    /// Run one of this view's tools; the asking pane and project are in `context.meta`.
    async fn call(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData>;

    /// The tool and arguments a control's `data_changed` (a `path` and the `value` written there) stands for.
    /// The call a `data_changed` stands for (`change_call`) when this view has the tool the path names.
    fn changed(&self, path: &str, value: &Value) -> Option<(String, serde_json::Map<String, Value>)> {
        let (tool, args) = change_call(path, value)?;
        self.tools().iter().any(|t| t.name == tool).then_some((tool, args))
    }

    /// The A2UI messages a read of the page answers with before the first call draws it.
    fn skeleton(&self) -> Value;

    /// The files in project `dir` whose change makes the page stale.
    fn watches(&self, dir: &str) -> Vec<PathBuf>;

    /// The most seconds the page goes without a check for changes.
    fn every(&self) -> u64 {
        60
    }

    /// A host subscribed to the page for project `dir`: load what it needs.
    fn open(&self, dir: &str);

    /// The last subscription to the page for `dir` ended: drop what was loaded.
    fn close(&self, dir: &str);

    /// Whether the page shows the agents Wheel runs, and so is told when they change.
    fn follows_agents(&self) -> bool {
        false
    }

    /// The agents of project `dir` changed to `agents`; true when the page now differs.
    fn agents_changed(&self, _dir: &str, _agents: &[Agent]) -> bool {
        false
    }

    /// Work the page started in the background for project `dir` has finished, so the page should be drawn again; true once
    /// per finish.
    fn finished(&self, _dir: &str) -> bool {
        false
    }

    /// Re-read project `dir`; true when the page now differs.
    fn refresh(&self, dir: &str) -> bool;
}

/// The views a process serves, by name.
#[derive(Clone, Default)]
pub struct Registry {
    views: Vec<Arc<dyn View>>,
}

impl Registry {
    pub fn with(mut self, view: impl View + 'static) -> Self {
        assert!(self.get(view.name()).is_none(), "two views named {}", view.name());
        self.views.push(Arc::new(view));
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn View>> {
        self.views.iter().find(|v| v.name() == name).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn View>] {
        &self.views
    }

    /// The view a request is for: the one `artifact` names. A host that names none is asking a
    /// process that serves a single view, which is then the answer.
    pub fn of(&self, artifact: Option<&str>) -> Result<Arc<dyn View>, String> {
        match artifact {
            Some(name) => self.get(name).ok_or_else(|| {
                let names: Vec<&str> = self.views.iter().map(|v| v.name()).collect();
                format!("no view named {name}; views: {}", names.join(", "))
            }),
            None if self.views.len() == 1 => Ok(self.views[0].clone()),
            None => Err("the request names no artifact (_meta wheel/artifact)".into()),
        }
    }
}

/// A path is `/<tool>` or `/<tool>/<key>`; the arguments are the written value as `value` text and the key as `key`. A choice
/// (an array) sends its selections joined by commas, so one of several is just its own value.
pub fn change_call(path: &str, value: &Value) -> Option<(String, serde_json::Map<String, Value>)> {
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let tool = parts.next().filter(|t| !t.is_empty())?;
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(","),
        _ => String::new(),
    };
    let mut args = serde_json::Map::new();
    if let Some(key) = parts.next().filter(|k| !k.is_empty()) {
        args.insert("key".into(), Value::String(key.into()));
    }
    args.insert("value".into(), Value::String(text));
    Some((tool.to_string(), args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_path_names_the_tool_and_a_second_segment_its_key() {
        let (tool, args) = change_call("/set_filter/status", &json!(["open"])).unwrap();
        assert_eq!(tool, "set_filter");
        assert_eq!((args["key"].as_str(), args["value"].as_str()), (Some("status"), Some("open")));
        let (tool, args) = change_call("/set_query", &json!("bug")).unwrap();
        assert_eq!((tool.as_str(), args["value"].as_str(), args.get("key")), ("set_query", Some("bug"), None));
    }

    #[test]
    fn a_key_keeps_its_slashes_and_a_value_becomes_text() {
        assert_eq!(change_call("/set_new/a/b", &json!("x")).unwrap().1["key"], "a/b");
        assert_eq!(change_call("/set_hide", &json!(true)).unwrap().1["value"], "true");
        assert_eq!(change_call("/set_n", &json!(3)).unwrap().1["value"], "3");
        assert_eq!(change_call("/set_view", &json!([])).unwrap().1["value"], "");
        assert_eq!(change_call("/set_filter/label", &json!(["client", "mac"])).unwrap().1["value"], "client,mac");
    }

    #[test]
    fn a_path_with_no_tool_is_no_call() {
        assert!(change_call("/", &json!("x")).is_none());
        assert!(change_call("", &json!("x")).is_none());
    }
}
