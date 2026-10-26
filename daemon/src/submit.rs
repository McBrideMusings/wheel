//! A page's Send (`POST .wheel/submit`, `wheel.submit(value)` in the page): one event the daemon
//! fans out to every delivery the artifact's entry asks for, so the page never knows where its
//! answer goes.
//!
//! - always: the value is stored under the page's `submitted` key, with the time added as `at`,
//!   which `wheel artifact state <pane> submitted --wait` returns;
//! - `reply_to = "<pane>"`: the value's `summary` text is typed into that agent pane, a chat agent
//!   as a message and a terminal as keystrokes and Enter;
//! - `webhook = "<url>"`: the daemon POSTs `{post, pane, submitted}` to the URL as JSON.
//!
//! The targets come from the entry, which whoever made the post wrote, never from the page: a page
//! can only send, not choose where.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use wheel_proto::PaneKind;

use crate::server::AppState;

/// The longest message typed into an agent pane.
const MAX_MESSAGE: usize = 4000;

/// How long a webhook has to answer.
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// The gap between the text and Enter, so a TUI with bracketed paste reads them as two reads.
const ENTER_DELAY: Duration = Duration::from_millis(100);

/// The text typed into an agent: one line, no control characters, cut to [`MAX_MESSAGE`].
fn one_line(summary: &str) -> String {
    let line: String = summary
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !c.is_control())
        .collect();
    line.chars().take(MAX_MESSAGE).collect()
}

impl AppState {
    /// The artifact web pane `pane` shows, and its space's directory.
    pub(crate) fn web_pane_artifact(
        &self,
        pane: &str,
    ) -> Option<(wheel_proto::ArtifactRef, String)> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .find_map(|s| match crate::spaces::find_pane(&s.root, pane) {
                Some(PaneKind::Web { artifact, .. }) => Some((artifact.clone(), s.cwd.clone())),
                _ => None,
            })
    }

    /// Store a page's Send and deliver it. The reply says what each delivery did: `state` always,
    /// `agent` and `webhook` only when the entry asks, each `"ok"` or the reason it failed. A
    /// failed delivery is never an error of the Send: the answer is stored either way.
    pub(crate) async fn page_submit(
        self: &Arc<Self>,
        pane: &str,
        mut value: Value,
    ) -> Result<Value, String> {
        let page = self.page(pane)?;
        let (artifact, cwd) = self
            .web_pane_artifact(pane)
            .ok_or_else(|| format!("{pane} is not a web pane"))?;
        let entry = self
            .artifact_entry(&artifact, &cwd)?
            .ok_or_else(|| format!("no entry for {}", artifact.name()))?;
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        match value.as_object_mut() {
            Some(object) => {
                object.insert("at".into(), json!(at));
            }
            None => return Err("a Send is one JSON object".into()),
        }
        page.set("submitted", value.clone())?;
        let mut delivered = serde_json::Map::new();
        delivered.insert("state".into(), json!("ok"));
        if let Some(target) = &entry.reply_to {
            let text = format!(
                "[{}] {}",
                artifact.name(),
                one_line(value["summary"].as_str().unwrap_or("answers sent"))
            );
            let result = self.type_into(target, &text).await;
            tracing::info!(%pane, post = artifact.name(), %target, ok = result.is_ok(), chars = text.len(), "page send typed into the agent");
            delivered.insert(
                "agent".into(),
                json!(result.map_or_else(|e| e, |()| "ok".to_string())),
            );
        }
        if let Some(url) = entry.webhook.clone() {
            let body = json!({"post": artifact.name(), "pane": pane, "submitted": value});
            let result = tokio::task::spawn_blocking({
                let url = url.clone();
                move || post_json(&url, &body)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            // The query can carry a secret, so only the address before it is logged.
            let shown = url.split(['?', '#']).next().unwrap_or_default();
            tracing::info!(%pane, post = artifact.name(), url = shown, ok = result.is_ok(), "page send posted to the webhook");
            delivered.insert(
                "webhook".into(),
                json!(result.map_or_else(|e| e, |()| "ok".to_string())),
            );
        }
        Ok(Value::Object(delivered))
    }

    /// Type `text` and Enter into agent pane `target`: a chat agent takes it as a message, a
    /// terminal as keystrokes.
    async fn type_into(&self, target: &str, text: &str) -> Result<(), String> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .find_map(|s| match crate::spaces::find_pane(&s.root, target) {
                    Some(PaneKind::Terminal { term, .. }) => Some(term.clone()),
                    _ => None,
                })
        };
        if let Some(term) = term {
            let term = self
                .term(&term)
                .ok_or_else(|| format!("{target}'s terminal is gone"))?;
            term.write(text.as_bytes().to_vec())
                .map_err(|e| e.to_string())?;
            tokio::time::sleep(ENTER_DELAY).await;
            return term.write(b"\r".to_vec()).map_err(|e| e.to_string());
        }
        let agent = self.resolve_agent(target)?;
        agent.session.send(text.to_string()).map(|_| ())
    }
}

/// POST `body` to `url` as JSON; an answer that is not 2xx is a failure.
fn post_json(url: &str, body: &Value) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(WEBHOOK_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("webhook: {e}"))?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        status => Err(format!("webhook answered {status}")),
    }
}
