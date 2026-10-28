//! The agent interface ([`crate::agent`]) over Codex: one `codex app-server` process per
//! conversation, talking JSON-RPC (one JSON object per line, without the `jsonrpc` field) on its
//! stdin and stdout.
//!
//! The host opens with `initialize` and an `initialized` notification, then `thread/start` or
//! `thread/resume` (the brief goes in as `developerInstructions`). Codex names the thread, so the
//! conversation id is the one the answer carries, and a resumed thread's answer holds its turns.
//! A prompt is `turn/start`, a cancel `turn/interrupt`. The reply arrives as `item/*`
//! notifications: `agentMessage` and `reasoning` stream as deltas, `commandExecution`, `fileChange`
//! and `mcpToolCall` items become tool calls, `thread/tokenUsage/updated` carries the context use
//! and `turn/completed` ends the turn. Codex asks to run a command or change files with a server
//! request, `item/commandExecution/requestApproval` or `item/fileChange/requestApproval`, which the
//! host answers with a decision; any other server request is refused.
//!
//! The one control is `mode`, the approval policy: it is sent with every `turn/start`.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use wheel_proto::{
    ConfigChoice, ConfigKind, ConfigOption, ContextUse, PermOption, PlanItem, SessionEvent,
    ToolContent,
};

use crate::agent::{
    self, Agent, AgentEvent, AgentInput, Conversation, Input, Launch, Process, ProcessGroup,
};
use crate::claude::History;

/// The approval policies the `mode` control offers, in order, with their names. Codex asks for
/// most commands under the first.
const POLICIES: [(&str, &str); 3] = [
    ("untrusted", "Ask for most commands"),
    ("on-request", "Ask when it needs more access"),
    ("never", "Never ask"),
];

/// The names of the approval policies, for refusing one Codex does not have.
pub(crate) fn policy_names() -> Vec<&'static str> {
    POLICIES.iter().map(|(v, _)| *v).collect()
}

/// The `mode` control with `current` chosen.
fn mode_option(current: &str) -> ConfigOption {
    ConfigOption {
        id: "mode".into(),
        name: "Approvals".into(),
        description: None,
        category: Some("mode".into()),
        kind: ConfigKind::Select {
            current: current.into(),
            choices: POLICIES
                .iter()
                .map(|(value, name)| ConfigChoice {
                    value: (*value).into(),
                    name: (*name).into(),
                    description: None,
                })
                .collect(),
        },
    }
}

/// Start `codex app-server` for `launch`.
pub fn launch(launch: Launch) -> Result<Agent, String> {
    let Process {
        pid,
        child,
        stdin,
        stdout,
        last_error,
    } = agent::spawn(&launch, vec!["app-server".into()], "CODEX_HOME")?;
    let (input, inputs) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let policy = launch
        .permission_mode
        .clone()
        .unwrap_or_else(|| POLICIES[0].0.into());
    let driver = Driver {
        pid,
        stdin,
        events,
        launch,
        next_request: 1,
        pending: HashMap::new(),
        thread: None,
        turn: None,
        in_turn: false,
        interrupted: false,
        policy,
        approvals: HashMap::new(),
        changes: HashMap::new(),
        streamed: HashSet::new(),
        wrote_text: false,
        pad_text: false,
        usage: None,
    };
    tokio::spawn(driver.run(stdout, inputs, child, last_error));
    Ok(Agent {
        input: AgentInput(input),
        events: events_rx,
        _group: ProcessGroup(pid),
    })
}

/// A request this side sent Codex, waiting for its answer.
enum Pending {
    Initialize,
    /// `thread/start` or `thread/resume`.
    Thread,
    TurnStart,
    Interrupt,
}

/// A question Codex asked and a person has not answered.
struct Approval {
    /// Codex's id for the request, which the answer repeats.
    id: Value,
    /// Whether the answer is `acceptForSession` for "allow always".
    session_scope: bool,
}

struct Driver {
    pid: u32,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedSender<AgentEvent>,
    launch: Launch,
    next_request: u64,
    pending: HashMap<u64, Pending>,
    thread: Option<String>,
    /// The running turn's id, once Codex has named it.
    turn: Option<String>,
    /// A prompt has been sent and its `turn/completed` has not arrived.
    in_turn: bool,
    /// The running turn was cancelled, so it ends as `cancelled`.
    interrupted: bool,
    /// The approval policy sent with each turn.
    policy: String,
    /// Approval requests waiting on an answer, by the key the permission carries.
    approvals: HashMap<String, Approval>,
    /// The paths each file change touches, by item id, for the title of its approval.
    changes: HashMap<String, Vec<String>>,
    /// Items whose text arrived as deltas, so their whole copy is not sent again.
    streamed: HashSet<String>,
    /// The turn has already shown some reply text, so the next message starts a new paragraph.
    wrote_text: bool,
    /// The next reply text opens with a paragraph break.
    pad_text: bool,
    /// The conversation as the model last saw it.
    usage: Option<ContextUse>,
}

impl Driver {
    async fn run(
        mut self,
        stdout: BufReader<tokio::process::ChildStdout>,
        mut inputs: mpsc::UnboundedReceiver<Input>,
        child: tokio::process::Child,
        last_error: std::sync::Arc<std::sync::Mutex<String>>,
    ) {
        let mut lines = stdout.lines();
        let mut failed = self
            .request(
                Pending::Initialize,
                "initialize",
                json!({"clientInfo": {"name": "wheel", "title": "Wheel", "version": env!("CARGO_PKG_VERSION")}}),
            )
            .await
            .err();
        while failed.is_none() {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Err(e) = self.output(&line).await {
                            failed = Some(e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => failed = Some(format!("reading the agent's output failed: {e}")),
                },
                input = inputs.recv() => match input {
                    Some(input) => {
                        if let Err(e) = self.input(input).await {
                            failed = Some(e);
                        }
                    }
                    // The session dropped the agent; its process group is already killed.
                    None => return,
                },
            }
        }
        let error = agent::exit_error(child, failed, &last_error).await;
        tracing::info!(pid = self.pid, error = ?error, "agent process exited");
        let _ = self.events.send(AgentEvent::Exited { error });
    }

    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    fn transcript(&self, event: SessionEvent) {
        self.emit(AgentEvent::Transcript(event));
    }

    async fn write(&mut self, line: Value) -> Result<(), String> {
        let mut text = line.to_string();
        text.push('\n');
        self.stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("writing to the agent failed: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("writing to the agent failed: {e}"))
    }

    async fn request(
        &mut self,
        pending: Pending,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        let id = self.next_request;
        self.next_request += 1;
        self.pending.insert(id, pending);
        self.write(json!({"id": id, "method": method, "params": params}))
            .await
    }

    async fn input(&mut self, input: Input) -> Result<(), String> {
        match input {
            Input::Prompt(text) => {
                let Some(thread) = self.thread.clone() else {
                    return Err("a prompt arrived before the thread opened".into());
                };
                self.in_turn = true;
                self.interrupted = false;
                self.turn = None;
                self.wrote_text = false;
                self.pad_text = false;
                self.request(
                    Pending::TurnStart,
                    "turn/start",
                    json!({
                        "threadId": thread,
                        "input": [{"type": "text", "text": text, "text_elements": []}],
                        "approvalPolicy": self.policy,
                    }),
                )
                .await
            }
            Input::Cancel => {
                if !self.in_turn || self.interrupted {
                    return Ok(());
                }
                self.interrupted = true;
                tracing::info!(pid = self.pid, turn = ?self.turn, "agent interrupted");
                self.interrupt().await
            }
            Input::Answer { request, choice } => {
                let Some(approval) = self.approvals.remove(&request) else {
                    return Ok(());
                };
                let decision = match choice.as_deref() {
                    Some("allow") => "accept",
                    Some("allow_always") if approval.session_scope => "acceptForSession",
                    Some("allow_always") => "accept",
                    Some(_) => "decline",
                    None => "cancel",
                };
                self.write(json!({"id": approval.id, "result": {"decision": decision}}))
                    .await
            }
            Input::SetOption { id, value, reply } => {
                let answer = if id != "mode" {
                    Err(format!("the agent has no option {id}"))
                } else if !POLICIES.iter().any(|(v, _)| *v == value) {
                    let offered: Vec<&str> = POLICIES.iter().map(|(v, _)| *v).collect();
                    Err(format!("{id} does not take {value}; it takes {offered:?}"))
                } else {
                    self.policy = value.clone();
                    Ok(vec![mode_option(&value)])
                };
                tracing::info!(pid = self.pid, option = %id, %value, ok = answer.is_ok(), "agent option set");
                let _ = reply.send(answer);
                Ok(())
            }
        }
    }

    /// Ask Codex to stop the running turn, once it has named it.
    async fn interrupt(&mut self) -> Result<(), String> {
        let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) else {
            return Ok(());
        };
        self.request(
            Pending::Interrupt,
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
        )
        .await
    }

    async fn output(&mut self, line: &str) -> Result<(), String> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            tracing::warn!(
                pid = self.pid,
                bytes = line.len(),
                "agent printed a line that is not JSON"
            );
            return Ok(());
        };
        match (message["method"].as_str(), message.get("id")) {
            (Some(method), Some(id)) => {
                self.server_request(method, id.clone(), &message["params"])
                    .await
            }
            (Some(method), None) => self.notification(method, &message["params"]).await,
            (None, Some(_)) => self.response(&message).await,
            (None, None) => Ok(()),
        }
    }

    async fn response(&mut self, message: &Value) -> Result<(), String> {
        let Some(pending) = message["id"]
            .as_u64()
            .and_then(|id| self.pending.remove(&id))
        else {
            return Ok(());
        };
        if let Some(error) = message.get("error") {
            let said = error["message"].as_str().unwrap_or("the agent refused");
            return match pending {
                Pending::Initialize | Pending::Thread => Err(said.to_string()),
                Pending::TurnStart => {
                    self.turn_failed(said.to_string());
                    Ok(())
                }
                Pending::Interrupt => {
                    tracing::warn!(pid = self.pid, error = %said, "agent interrupt refused");
                    Ok(())
                }
            };
        }
        let result = &message["result"];
        match pending {
            Pending::Initialize => {
                tracing::info!(pid = self.pid, agent = %result["userAgent"], home = %result["codexHome"], "agent initialized");
                self.write(json!({"method": "initialized"})).await?;
                let mut params = json!({
                    "cwd": self.launch.cwd.display().to_string(),
                    "approvalPolicy": self.policy,
                });
                if let Some(brief) = &self.launch.brief {
                    params["developerInstructions"] = json!(brief);
                }
                if let Some(sandbox) = &self.launch.sandbox {
                    params["sandbox"] = json!(sandbox);
                }
                let method = match &self.launch.conversation {
                    Conversation::New(_) => "thread/start",
                    Conversation::Resume(id) => {
                        params["threadId"] = json!(id);
                        "thread/resume"
                    }
                };
                self.request(Pending::Thread, method, params).await
            }
            Pending::Thread => {
                let thread = &result["thread"];
                let id = thread["id"]
                    .as_str()
                    .ok_or("the agent opened a thread with no id")?
                    .to_string();
                self.thread = Some(id.clone());
                let history = matches!(self.launch.conversation, Conversation::Resume(_))
                    .then(|| history(thread));
                tracing::info!(pid = self.pid, thread = %id, model = %result["model"], resumed = history.is_some(), turns = history.as_ref().map_or(0, |h| h.turns.len()), "agent ready");
                self.emit(AgentEvent::Ready {
                    options: vec![mode_option(&self.policy)],
                    conversation: Some(id),
                    history,
                });
                Ok(())
            }
            Pending::TurnStart => {
                if let Some(turn) = result["turn"]["id"].as_str() {
                    self.turn = Some(turn.to_string());
                    if self.interrupted {
                        return self.interrupt().await;
                    }
                }
                Ok(())
            }
            Pending::Interrupt => Ok(()),
        }
    }

    async fn notification(&mut self, method: &str, params: &Value) -> Result<(), String> {
        match method {
            "turn/started" => {
                if let Some(turn) = params["turn"]["id"].as_str() {
                    self.turn = Some(turn.to_string());
                    if self.interrupted {
                        return self.interrupt().await;
                    }
                }
            }
            "item/started" => self.item_started(&params["item"]),
            "item/completed" => self.item_completed(&params["item"]),
            "item/agentMessage/delta" => {
                if let (Some(id), Some(delta)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    self.streamed.insert(id.to_string());
                    let text = if std::mem::take(&mut self.pad_text) {
                        format!("\n\n{delta}")
                    } else {
                        delta.to_string()
                    };
                    self.wrote_text = true;
                    self.transcript(SessionEvent::Text { text });
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                if let (Some(id), Some(delta)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    self.streamed.insert(id.to_string());
                    self.transcript(SessionEvent::Thought { text: delta.into() });
                }
            }
            "item/reasoning/summaryPartAdded" => {
                if params["summaryIndex"].as_u64().unwrap_or(0) > 0 {
                    self.transcript(SessionEvent::Thought {
                        text: "\n\n".into(),
                    });
                }
            }
            "turn/plan/updated" => {
                let entries = params["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|step| PlanItem {
                        content: step["step"].as_str().unwrap_or_default().to_string(),
                        status: match step["status"].as_str() {
                            Some("inProgress") => "in_progress",
                            Some("completed") => "completed",
                            _ => "pending",
                        }
                        .into(),
                    })
                    .collect();
                self.transcript(SessionEvent::Plan { entries });
            }
            "thread/tokenUsage/updated" => {
                let usage = &params["tokenUsage"];
                if let (Some(used), Some(size)) = (
                    usage["last"]["totalTokens"].as_u64(),
                    usage["modelContextWindow"].as_u64(),
                ) {
                    self.usage = Some(ContextUse { used, size });
                }
            }
            "serverRequest/resolved" => {
                let id = params["requestId"].clone();
                self.approvals.retain(|_, a| a.id != id);
            }
            "turn/completed" => self.turn_completed(&params["turn"]),
            "error" => {
                tracing::warn!(pid = self.pid, error = %params["error"]["message"], will_retry = %params["willRetry"], "agent reported an error");
            }
            _ => {}
        }
        Ok(())
    }

    fn item_started(&mut self, item: &Value) {
        match item["type"].as_str() {
            Some("agentMessage") => {
                // A second message in a turn starts its own paragraph.
                self.pad_text = self.wrote_text;
            }
            Some("fileChange") => {
                if let Some(id) = item["id"].as_str() {
                    self.changes.insert(id.to_string(), change_paths(item));
                }
                if let Some(call) = tool_call(item) {
                    self.transcript(call);
                }
            }
            Some("commandExecution" | "mcpToolCall" | "webSearch") => {
                if let Some(call) = tool_call(item) {
                    self.transcript(call);
                }
            }
            _ => {}
        }
    }

    fn item_completed(&mut self, item: &Value) {
        let id = item["id"].as_str().unwrap_or_default();
        let streamed = self.streamed.remove(id);
        match item["type"].as_str() {
            Some("agentMessage") if !streamed => {
                if let Some(text) = item["text"].as_str().filter(|t| !t.is_empty()) {
                    let text = if std::mem::take(&mut self.pad_text) {
                        format!("\n\n{text}")
                    } else {
                        text.to_string()
                    };
                    self.wrote_text = true;
                    self.transcript(SessionEvent::Text { text });
                }
            }
            Some("reasoning") if !streamed => {
                if let Some(text) = reasoning_text(item) {
                    self.transcript(SessionEvent::Thought { text });
                }
            }
            Some("commandExecution" | "fileChange" | "mcpToolCall" | "webSearch") => {
                if let Some(update) = tool_update(item) {
                    self.transcript(update);
                }
            }
            _ => {}
        }
    }

    fn turn_completed(&mut self, turn: &Value) {
        self.in_turn = false;
        self.turn = None;
        self.streamed.clear();
        // Nothing asked during the turn can still be answered.
        self.approvals.clear();
        if let Some(context) = self.usage.take() {
            self.emit(AgentEvent::Context(Some(context)));
        }
        if std::mem::take(&mut self.interrupted) || turn["status"] == "interrupted" {
            self.emit(AgentEvent::TurnEnd {
                stop_reason: "cancelled".into(),
            });
            return;
        }
        if turn["status"] == "failed" {
            let said = turn["error"]["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or("the turn failed");
            self.emit(AgentEvent::TurnFailed {
                message: said.to_string(),
            });
            return;
        }
        self.emit(AgentEvent::TurnEnd {
            stop_reason: "end_turn".into(),
        });
    }

    /// A turn that never started: Codex refused the prompt.
    fn turn_failed(&mut self, message: String) {
        self.in_turn = false;
        self.interrupted = false;
        self.emit(AgentEvent::TurnFailed { message });
    }

    async fn server_request(
        &mut self,
        method: &str,
        id: Value,
        params: &Value,
    ) -> Result<(), String> {
        let (title, tool_call, session_scope) = match method {
            "item/commandExecution/requestApproval" => (
                command_title(params),
                params["itemId"].as_str().map(String::from),
                params["availableDecisions"]
                    .as_array()
                    .is_some_and(|d| d.iter().any(|d| d == "acceptForSession")),
            ),
            "item/fileChange/requestApproval" => {
                let item = params["itemId"].as_str().unwrap_or_default();
                let paths = self.changes.get(item).cloned().unwrap_or_default();
                let title = if paths.is_empty() {
                    params["reason"]
                        .as_str()
                        .unwrap_or("Change files")
                        .to_string()
                } else {
                    format!("Edit {}", paths.join(", "))
                };
                (title, Some(item.to_string()), true)
            }
            _ => {
                tracing::info!(pid = self.pid, %method, "agent request refused: not one this host answers");
                return self
                    .write(json!({"id": id, "error": {"code": -32601, "message": "not supported by this host"}}))
                    .await;
            }
        };
        let request = id.to_string();
        self.approvals
            .insert(request.clone(), Approval { id, session_scope });
        tracing::info!(pid = self.pid, %request, %method, %title, "agent asks permission");
        self.emit(AgentEvent::Permission {
            request,
            title,
            tool_call,
            options: perm_options(session_scope),
        });
        Ok(())
    }
}

/// The options a permission request offers; `wheel perm answer <id> allow|deny` picks them by kind.
fn perm_options(session: bool) -> Vec<PermOption> {
    let option = |id: &str, name: &str, kind: &str| PermOption {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
    };
    let mut options = vec![option("allow", "Allow", "allow_once")];
    if session {
        options.push(option(
            "allow_always",
            "Allow for this session",
            "allow_always",
        ));
    }
    options.push(option("deny", "Deny", "reject_once"));
    options
}

/// What an approval for a command shows: the command as the model wrote it, without the shell
/// Codex wraps it in.
fn command_title(params: &Value) -> String {
    let actions: Vec<&str> = params["commandActions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["command"].as_str())
        .filter(|c| !c.is_empty())
        .collect();
    if actions.is_empty() {
        params["command"].as_str().unwrap_or("command").to_string()
    } else {
        actions.join("; ")
    }
}

fn change_paths(item: &Value) -> Vec<String> {
    item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["path"].as_str().map(String::from))
        .collect()
}

fn reasoning_text(item: &Value) -> Option<String> {
    let parts = |key: &str| {
        item[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let text = parts("summary");
    let text = if text.is_empty() {
        parts("content")
    } else {
        text
    };
    (!text.is_empty()).then_some(text)
}

/// The tool call an item starts, as a client draws it.
fn tool_call(item: &Value) -> Option<SessionEvent> {
    let id = item["id"].as_str()?.to_string();
    let (title, kind, content) = match item["type"].as_str()? {
        "commandExecution" => (command_title(item), "execute", Vec::new()),
        "fileChange" => {
            let paths = change_paths(item);
            let title = if paths.is_empty() {
                "Edit files".to_string()
            } else {
                format!("Edit {}", paths.join(", "))
            };
            let content = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let diff = c["diff"].as_str().filter(|d| !d.is_empty())?;
                    Some(ToolContent::Text {
                        text: format!("{}\n{diff}", c["path"].as_str().unwrap_or_default()),
                    })
                })
                .collect();
            (title, "edit", content)
        }
        "mcpToolCall" => (
            format!(
                "{}: {}",
                item["server"].as_str().unwrap_or("mcp"),
                item["tool"].as_str().unwrap_or("tool")
            ),
            "other",
            Vec::new(),
        ),
        "webSearch" => (
            format!("Search {}", item["query"].as_str().unwrap_or_default()),
            "fetch",
            Vec::new(),
        ),
        _ => return None,
    };
    tracing::debug!(id, kind, %title, "tool call");
    Some(SessionEvent::ToolCall {
        id,
        title,
        tool_kind: kind.into(),
        status: "pending".into(),
        content,
    })
}

/// The update that finishes a tool call's item. A file change keeps the diff its call showed.
fn tool_update(item: &Value) -> Option<SessionEvent> {
    let id = item["id"].as_str()?.to_string();
    let status = match item["status"].as_str() {
        Some("completed") => "completed",
        Some("inProgress") => return None,
        _ => "failed",
    };
    let text = match item["type"].as_str()? {
        "commandExecution" => item["aggregatedOutput"].as_str().unwrap_or_default(),
        _ => "",
    };
    tracing::debug!(id, status, bytes = text.len(), "tool update");
    Some(SessionEvent::ToolUpdate {
        id,
        title: None,
        status: Some(status.into()),
        content: (!text.is_empty()).then(|| vec![ToolContent::Text { text: text.into() }]),
    })
}

/// A resumed thread's turns as history: each prompt with what the agent did after it.
fn history(thread: &Value) -> History {
    let mut history = History::default();
    for turn in thread["turns"].as_array().into_iter().flatten() {
        let mut prompt = String::new();
        let mut events = Vec::new();
        for item in turn["items"].as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("userMessage") => {
                    prompt = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("");
                }
                Some("agentMessage") => {
                    if let Some(text) = item["text"].as_str().filter(|t| !t.is_empty()) {
                        let text = if events
                            .iter()
                            .any(|e| matches!(e, SessionEvent::Text { .. }))
                        {
                            format!("\n\n{text}")
                        } else {
                            text.to_string()
                        };
                        events.push(SessionEvent::Text { text });
                    }
                }
                Some("reasoning") => {
                    if let Some(text) = reasoning_text(item) {
                        events.push(SessionEvent::Thought { text });
                    }
                }
                Some("commandExecution" | "fileChange" | "mcpToolCall" | "webSearch") => {
                    events.extend(tool_call(item));
                    events.extend(tool_update(item));
                }
                _ => {}
            }
        }
        history.turns.push((prompt, events));
    }
    history
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_approval_is_titled_by_the_command_the_model_wrote() {
        let params = json!({
            "command": "/bin/zsh -lc 'touch a'",
            "commandActions": [{"type": "unknown", "command": "touch a"}],
        });
        assert_eq!(command_title(&params), "touch a");
        assert_eq!(
            command_title(&json!({"command": "ls", "commandActions": []})),
            "ls"
        );
    }

    #[test]
    fn a_resumed_thread_becomes_prompts_with_their_output() {
        let thread = json!({"turns": [{"items": [
            {"type": "userMessage", "id": "u", "content": [{"type": "text", "text": "hi"}]},
            {"type": "commandExecution", "id": "c", "command": "ls", "commandActions": [], "status": "completed", "aggregatedOutput": "a\n"},
            {"type": "agentMessage", "id": "m", "text": "done"},
        ]}]});
        let history = history(&thread);
        assert_eq!(history.turns.len(), 1);
        let (prompt, events) = &history.turns[0];
        assert_eq!(prompt, "hi");
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[2], SessionEvent::Text { text } if text == "done"));
    }
}
