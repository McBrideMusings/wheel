//! The agent interface ([`crate::agent`]) over pi: one `pi --mode rpc` process per conversation,
//! talking pi's RPC protocol (one JSON object per line) on its stdin and stdout.
//!
//! A command carries a `type` and an `id`; pi answers each with a `response` record repeating
//! the id. Wheel names a new conversation itself (`--session-id`, which pi creates) and resumes
//! one by handing `--session` the file pi keeps it in under the pane's directory. An id alone
//! would let pi match a prefix or a session of another directory, which pi then offers to fork by
//! asking on stdin, the channel this protocol runs on; a conversation with no file there is not
//! started at all. The brief goes in
//! with `--append-system-prompt`. Once up, the host asks `get_state`, the thinking levels the model
//! offers, and for a resumed conversation `get_messages`, whose turns become its history.
//!
//! A prompt is `prompt`, a cancel `abort`. The reply streams as session events: `message_update`
//! carries text and thinking deltas, `tool_execution_*` a tool call from start to result, and
//! `agent_settled` says pi will do nothing more on its own, which ends the turn; the host then asks
//! `get_session_stats` for how full the context window is. pi runs tools without asking. The only
//! questions it puts to a person are an extension's `select` and `confirm` dialogs
//! (`extension_ui_request`), which become permission requests; a dialog asking for typed text is
//! cancelled, since a permission card has nowhere to type.
//!
//! The one control is `thinking`, the reasoning level, among the levels the model offers.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use wheel_proto::{
    ConfigChoice, ConfigKind, ConfigOption, ContextUse, PermOption, SessionEvent, ToolContent,
};

use crate::agent::{
    self, Agent, AgentEvent, AgentInput, Conversation, Input, Launch, Process, ProcessGroup,
};
use crate::claude::History;
use crate::resume::{self, Agent as Harness};

/// Start `pi --mode rpc` for `launch`.
pub fn launch(launch: Launch) -> Result<Agent, String> {
    let mut args: Vec<String> = vec!["--mode".into(), "rpc".into()];
    match &launch.conversation {
        Conversation::New(id) => args.extend(["--session-id".into(), id.clone()]),
        Conversation::Resume(id) => {
            // pi files a session under its working directory with links resolved.
            let cwd = std::fs::canonicalize(&launch.cwd)
                .unwrap_or_else(|_| launch.cwd.clone())
                .display()
                .to_string();
            let file = launch
                .config_dir
                .clone()
                .or_else(|| Harness::Pi.daemon_home())
                .and_then(|home| resume::pi_session_file(&home, &cwd, id))
                .ok_or_else(|| format!("pi keeps no conversation {id} for {cwd}"))?;
            args.extend(["--session".into(), file.display().to_string()]);
        }
    }
    if let Some(brief) = &launch.brief {
        args.extend(["--append-system-prompt".into(), brief.clone()]);
    }
    if let Some(model) = &launch.model {
        args.extend(["--model".into(), model.clone()]);
    }
    let Process {
        pid,
        child,
        stdin,
        stdout,
        last_error,
    } = agent::spawn(&launch, args, "PI_CODING_AGENT_DIR")?;
    let (input, inputs) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let driver = Driver {
        pid,
        stdin,
        events,
        launch,
        next_request: 1,
        pending: HashMap::new(),
        conversation: None,
        thinking: String::new(),
        levels: Vec::new(),
        in_turn: false,
        aborted: false,
        failure: None,
        dialogs: HashMap::new(),
        shown: HashSet::new(),
        wrote_text: false,
        pad_text: false,
    };
    tokio::spawn(driver.run(stdout, inputs, child, last_error));
    Ok(Agent {
        input: AgentInput(input),
        events: events_rx,
        _group: ProcessGroup(pid),
    })
}

/// A command this side sent pi, waiting for its response.
enum Pending {
    State,
    Levels,
    Messages,
    /// The context stats a resumed session is asked for before it is ready; `history` waits.
    ResumeStats {
        history: History,
    },
    Prompt,
    Abort,
    /// The turn has settled; `aborted` says whether it was cancelled.
    Stats {
        aborted: bool,
    },
    Thinking {
        level: String,
        reply: oneshot::Sender<Result<Vec<ConfigOption>, String>>,
    },
}

/// An extension's dialog a person has not answered.
enum Dialog {
    Confirm,
    /// The choices, in order; an option's id is its place in this list.
    Select(Vec<String>),
}

struct Driver {
    pid: u32,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedSender<AgentEvent>,
    launch: Launch,
    next_request: u64,
    pending: HashMap<String, Pending>,
    /// pi's id for the session, from `get_state`.
    conversation: Option<String>,
    /// The thinking level in force.
    thinking: String,
    /// The thinking levels the model offers.
    levels: Vec<String>,
    /// A prompt has been sent and its turn has not ended.
    in_turn: bool,
    /// The running turn was cancelled, so it ends as `cancelled`.
    aborted: bool,
    /// Why the running turn failed, when the model's last reply or a retry said so.
    failure: Option<String>,
    /// Dialogs waiting on an answer, by the id the permission carries.
    dialogs: HashMap<String, Dialog>,
    /// Tool calls whose line shows their input (an edit's diff), which a successful result keeps.
    shown: HashSet<String>,
    /// The turn has already shown some reply text, so the next message starts a new paragraph.
    wrote_text: bool,
    /// The next reply text opens with a paragraph break.
    pad_text: bool,
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
            .request(Pending::State, json!({"type": "get_state"}))
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

    /// Send `command` under a fresh id, remembering what its response answers.
    async fn request(&mut self, pending: Pending, mut command: Value) -> Result<(), String> {
        let id = format!("wheel-{}", self.next_request);
        self.next_request += 1;
        command["id"] = json!(id);
        self.pending.insert(id, pending);
        self.write(command).await
    }

    async fn input(&mut self, input: Input) -> Result<(), String> {
        match input {
            Input::Prompt(text) => {
                self.in_turn = true;
                self.aborted = false;
                self.failure = None;
                self.wrote_text = false;
                self.pad_text = false;
                self.request(Pending::Prompt, json!({"type": "prompt", "message": text}))
                    .await
            }
            Input::Cancel => {
                if !self.in_turn || self.aborted {
                    return Ok(());
                }
                self.aborted = true;
                tracing::info!(pid = self.pid, "agent interrupted");
                self.request(Pending::Abort, json!({"type": "abort"})).await
            }
            Input::Answer { request, choice } => {
                let Some(dialog) = self.dialogs.remove(&request) else {
                    return Ok(());
                };
                let answer = match (dialog, choice.as_deref()) {
                    (_, None) => json!({"cancelled": true}),
                    (Dialog::Confirm, Some(choice)) => json!({"confirmed": choice == "allow"}),
                    (Dialog::Select(options), Some(choice)) => {
                        match choice.parse::<usize>().ok().and_then(|i| options.get(i)) {
                            Some(value) => json!({"value": value}),
                            None => json!({"cancelled": true}),
                        }
                    }
                };
                tracing::info!(pid = self.pid, %request, ?choice, "agent dialog answered");
                let mut line = json!({"type": "extension_ui_response", "id": request});
                if let (Some(line), Some(answer)) = (line.as_object_mut(), answer.as_object()) {
                    line.extend(answer.clone());
                }
                self.write(line).await
            }
            Input::SetOption { id, value, reply } => {
                if id != "thinking" {
                    let _ = reply.send(Err(format!("the agent has no option {id}")));
                    return Ok(());
                }
                if !self.levels.contains(&value) {
                    let _ = reply.send(Err(format!(
                        "{id} does not take {value}; it takes {:?}",
                        self.levels
                    )));
                    return Ok(());
                }
                self.request(
                    Pending::Thinking {
                        level: value.clone(),
                        reply,
                    },
                    json!({"type": "set_thinking_level", "level": value}),
                )
                .await
            }
        }
    }

    async fn output(&mut self, line: &str) -> Result<(), String> {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            tracing::warn!(
                pid = self.pid,
                bytes = line.len(),
                "agent printed a line that is not JSON"
            );
            return Ok(());
        };
        match record["type"].as_str().unwrap_or_default() {
            "response" => self.response(&record).await,
            "extension_ui_request" => self.dialog(&record).await,
            kind => {
                self.event(kind, &record).await;
                Ok(())
            }
        }
    }

    async fn response(&mut self, record: &Value) -> Result<(), String> {
        let Some(pending) = record["id"].as_str().and_then(|id| self.pending.remove(id)) else {
            if record["success"] == false {
                tracing::warn!(pid = self.pid, command = %record["command"], error = %record["error"], "agent refused a command");
            }
            return Ok(());
        };
        let data = &record["data"];
        let error = (record["success"] != true).then(|| {
            record["error"]
                .as_str()
                .unwrap_or("the agent refused")
                .to_string()
        });
        match pending {
            Pending::State => {
                if let Some(e) = error {
                    return Err(e);
                }
                self.conversation = data["sessionId"].as_str().map(String::from);
                self.thinking = data["thinkingLevel"].as_str().unwrap_or("off").to_string();
                tracing::info!(pid = self.pid, session = ?self.conversation, model = %data["model"]["id"], provider = %data["model"]["provider"], thinking = %self.thinking, "agent initialized");
                self.request(
                    Pending::Levels,
                    json!({"type": "get_available_thinking_levels"}),
                )
                .await
            }
            Pending::Levels => {
                if let Some(e) = error {
                    return Err(e);
                }
                self.levels = data["levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect();
                if matches!(self.launch.conversation, Conversation::Resume(_)) {
                    return self
                        .request(Pending::Messages, json!({"type": "get_messages"}))
                        .await;
                }
                self.ready(None);
                Ok(())
            }
            Pending::Messages => {
                if let Some(e) = error {
                    return Err(e);
                }
                let history = history(&data["messages"]);
                self.request(
                    Pending::ResumeStats { history },
                    json!({"type": "get_session_stats"}),
                )
                .await
            }
            Pending::ResumeStats { history } => {
                // A refused stats request leaves the pane without a figure, not without a session.
                if let Some(e) = error {
                    tracing::warn!(pid = self.pid, error = %e, "asking the resumed agent for its context failed");
                } else {
                    self.emit(AgentEvent::Context(context_use(data)));
                }
                self.ready(Some(history));
                Ok(())
            }
            Pending::Prompt => {
                if let Some(e) = error {
                    self.in_turn = false;
                    self.emit(AgentEvent::TurnFailed { message: e });
                } else if data["disposition"] == "handled" {
                    // An extension command took the prompt; no run starts, so none will settle.
                    self.in_turn = false;
                    self.emit(AgentEvent::TurnEnd {
                        stop_reason: "end_turn".into(),
                    });
                }
                Ok(())
            }
            Pending::Abort => {
                if let Some(e) = error {
                    tracing::warn!(pid = self.pid, error = %e, "agent abort refused");
                }
                Ok(())
            }
            Pending::Stats { aborted } => {
                match error {
                    None => self.emit(AgentEvent::Context(context_use(data))),
                    Some(e) => {
                        tracing::warn!(pid = self.pid, error = %e, "asking the agent for its context failed")
                    }
                }
                self.turn_over(aborted);
                Ok(())
            }
            Pending::Thinking { level, reply } => {
                let answer = match error {
                    Some(e) => Err(e),
                    None => {
                        self.thinking = level.clone();
                        Ok(vec![self.thinking_option()])
                    }
                };
                tracing::info!(pid = self.pid, option = "thinking", value = %level, ok = answer.is_ok(), "agent option set");
                let _ = reply.send(answer);
                Ok(())
            }
        }
    }

    fn ready(&self, history: Option<History>) {
        tracing::info!(pid = self.pid, session = ?self.conversation, levels = ?self.levels, resumed = history.is_some(), turns = history.as_ref().map_or(0, |h| h.turns.len()), "agent ready");
        self.emit(AgentEvent::Ready {
            options: vec![self.thinking_option()],
            conversation: self.conversation.clone(),
            history,
        });
    }

    /// The `thinking` control with the level in force chosen.
    fn thinking_option(&self) -> ConfigOption {
        ConfigOption {
            id: "thinking".into(),
            name: "Thinking".into(),
            description: None,
            category: Some("thought_level".into()),
            kind: ConfigKind::Select {
                current: self.thinking.clone(),
                choices: self
                    .levels
                    .iter()
                    .map(|level| ConfigChoice {
                        value: level.clone(),
                        name: level.clone(),
                        description: None,
                    })
                    .collect(),
            },
        }
    }

    async fn event(&mut self, kind: &str, record: &Value) {
        match kind {
            "message_start" if record["message"]["role"] == "assistant" => {
                // A second reply in a turn starts its own paragraph.
                self.pad_text = self.wrote_text;
            }
            "message_update" => {
                let update = &record["assistantMessageEvent"];
                let Some(delta) = update["delta"].as_str().filter(|d| !d.is_empty()) else {
                    return;
                };
                match update["type"].as_str() {
                    Some("text_delta") => {
                        let text = if std::mem::take(&mut self.pad_text) {
                            format!("\n\n{delta}")
                        } else {
                            delta.to_string()
                        };
                        self.wrote_text = true;
                        self.transcript(SessionEvent::Text { text });
                    }
                    Some("thinking_delta") => self.transcript(SessionEvent::Thought {
                        text: delta.to_string(),
                    }),
                    _ => {}
                }
            }
            "message_end" => {
                let message = &record["message"];
                if message["role"] == "assistant" && message["stopReason"] == "error" {
                    let said = message["errorMessage"]
                        .as_str()
                        .unwrap_or("the model reported an error");
                    tracing::warn!(pid = self.pid, error = %said, "agent reply ended in an error");
                    self.failure = Some(said.to_string());
                } else if message["role"] == "assistant" {
                    self.failure = None;
                }
            }
            "tool_execution_start" => {
                let id = record["toolCallId"].as_str().unwrap_or_default();
                let tool = record["toolName"].as_str().unwrap_or("tool");
                if let Some(call) = tool_call(id, tool, &record["args"], &mut self.shown) {
                    self.transcript(call);
                }
            }
            "tool_execution_update" => {
                let id = record["toolCallId"].as_str().unwrap_or_default();
                let text = result_text(&record["partialResult"]);
                if !id.is_empty() && !text.is_empty() && !self.shown.contains(id) {
                    self.transcript(SessionEvent::ToolUpdate {
                        id: id.to_string(),
                        title: None,
                        status: Some("in_progress".into()),
                        content: Some(vec![ToolContent::Text { text }]),
                    });
                }
            }
            "tool_execution_end" => {
                let id = record["toolCallId"].as_str().unwrap_or_default();
                let failed = record["isError"] == true;
                let shown = self.shown.remove(id);
                if let Some(update) = tool_result(id, &record["result"], failed, shown) {
                    self.transcript(update);
                }
            }
            "auto_retry_end" if record["success"] == false => {
                if let Some(said) = record["finalError"].as_str() {
                    self.failure = Some(said.to_string());
                }
            }
            "thinking_level_changed" => {
                if let Some(level) = record["level"].as_str() {
                    self.thinking = level.to_string();
                    self.emit(AgentEvent::Options(vec![self.thinking_option()]));
                }
            }
            "agent_settled" => {
                if !self.in_turn {
                    return;
                }
                let aborted = record["aborted"] == true || self.aborted;
                if let Err(e) = self
                    .request(
                        Pending::Stats { aborted },
                        json!({"type": "get_session_stats"}),
                    )
                    .await
                {
                    tracing::warn!(pid = self.pid, error = %e, "asking the agent for its context failed");
                    self.turn_over(aborted);
                }
            }
            "extension_error" => {
                tracing::warn!(pid = self.pid, extension = %record["extensionPath"], event = %record["event"], error = %record["error"], "agent extension failed");
            }
            _ => {}
        }
    }

    /// End the running turn: cancelled, failed with what the model or a retry said, or done.
    fn turn_over(&mut self, aborted: bool) {
        if !std::mem::take(&mut self.in_turn) {
            return;
        }
        self.aborted = false;
        self.shown.clear();
        // A dialog still open stays in `dialogs`: pi waits on it past the turn, and the session
        // withdraws its request when the turn ends, which answers it as cancelled.
        let failure = self.failure.take();
        let event = match (aborted, failure) {
            (true, _) => AgentEvent::TurnEnd {
                stop_reason: "cancelled".into(),
            },
            (false, Some(message)) => AgentEvent::TurnFailed { message },
            (false, None) => AgentEvent::TurnEnd {
                stop_reason: "end_turn".into(),
            },
        };
        tracing::info!(pid = self.pid, ?event, "agent turn over");
        self.emit(event);
    }

    /// An extension's request to the person: a choice or a yes/no becomes a permission request,
    /// a request for typed text is cancelled, and a notice or status line is only logged.
    async fn dialog(&mut self, record: &Value) -> Result<(), String> {
        let id = record["id"].as_str().unwrap_or_default().to_string();
        let method = record["method"].as_str().unwrap_or_default();
        let title = record["title"]
            .as_str()
            .unwrap_or("The agent asks")
            .to_string();
        let option = |id: String, name: &str, kind: &str| PermOption {
            id,
            name: name.into(),
            kind: kind.into(),
        };
        let (dialog, title, options) = match method {
            "confirm" => {
                let title = match record["message"].as_str().filter(|m| !m.is_empty()) {
                    Some(message) => format!("{title}: {message}"),
                    None => title,
                };
                let options = vec![
                    option("allow".into(), "Yes", "allow_once"),
                    option("deny".into(), "No", "reject_once"),
                ];
                (Dialog::Confirm, title, options)
            }
            "select" => {
                let choices: Vec<String> = record["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o.as_str().map(String::from))
                    .collect();
                let options = choices
                    .iter()
                    .enumerate()
                    .map(|(i, name)| option(i.to_string(), name, "other"))
                    // A choice none of pi's options is: `perm answer <id> deny`, and the way
                    // out of a select with no options.
                    .chain([option("dismiss".into(), "Dismiss", "reject_once")])
                    .collect();
                (Dialog::Select(choices), title, options)
            }
            "input" | "editor" => {
                tracing::info!(pid = self.pid, %id, %method, %title, "agent asked for typed text; cancelled");
                return self
                    .write(json!({"type": "extension_ui_response", "id": id, "cancelled": true}))
                    .await;
            }
            _ => {
                tracing::info!(pid = self.pid, %method, message = %record["message"], "agent extension notice");
                return Ok(());
            }
        };
        tracing::info!(pid = self.pid, request = %id, %method, %title, "agent asks permission");
        self.dialogs.insert(id.clone(), dialog);
        self.emit(AgentEvent::Permission {
            request: id,
            title,
            tool_call: None,
            options,
            why: wheel_proto::PermWhy::default(),
        });
        Ok(())
    }
}

/// How full the window is, from `get_session_stats`'s `data`. pi reports `tokens: null` right
/// after compaction, until the next reply: no figure, so nothing to show.
fn context_use(data: &Value) -> Option<ContextUse> {
    let usage = &data["contextUsage"];
    Some(ContextUse {
        used: usage["tokens"].as_u64()?,
        size: usage["contextWindow"].as_u64()?,
    })
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args[key].as_str().unwrap_or_default()
}

/// The tool call a tool starts, as a client draws it. A call whose line shows its input — an
/// edit's diff, a write's new file — is added to `shown`.
fn tool_call(
    id: &str,
    tool: &str,
    args: &Value,
    shown: &mut HashSet<String>,
) -> Option<SessionEvent> {
    if id.is_empty() {
        return None;
    }
    let path = arg(args, "path");
    let (title, kind) = match tool {
        "bash" => (arg(args, "command").to_string(), "execute"),
        "read" => (format!("Read {path}"), "read"),
        "edit" => (format!("Edit {path}"), "edit"),
        "write" => (format!("Write {path}"), "edit"),
        "grep" => (format!("grep {}", arg(args, "pattern")), "search"),
        "find" => (format!("Find {}", arg(args, "pattern")), "search"),
        "ls" => (format!("List {path}"), "read"),
        other => (other.to_string(), "other"),
    };
    // A title the arguments left empty is the tool's name.
    let title = if title.trim().is_empty() || title.ends_with(' ') {
        tool.to_string()
    } else {
        title
    };
    let diff = |old: Option<&str>, new: &str| ToolContent::Diff {
        path: path.to_string(),
        old_text: old.map(String::from),
        new_text: new.to_string(),
    };
    let content: Vec<ToolContent> = match tool {
        "edit" => args["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| diff(e["oldText"].as_str(), arg(e, "newText")))
            .collect(),
        "write" => vec![diff(None, arg(args, "content"))],
        _ => Vec::new(),
    };
    if !content.is_empty() {
        shown.insert(id.to_string());
    }
    tracing::debug!(id, tool, kind, %title, items = content.len(), "tool call");
    Some(SessionEvent::ToolCall {
        id: id.to_string(),
        title,
        tool_kind: kind.into(),
        status: "pending".into(),
        content,
    })
}

/// The text parts of a tool's result.
fn result_text(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The update that finishes a tool call. A call that shows its input keeps it unless it failed;
/// any other shows what the tool returned.
fn tool_result(id: &str, result: &Value, failed: bool, shown: bool) -> Option<SessionEvent> {
    if id.is_empty() {
        return None;
    }
    let status = if failed { "failed" } else { "completed" };
    let text = result_text(result);
    let content = (failed || !shown).then(|| {
        if text.is_empty() {
            Vec::new()
        } else {
            vec![ToolContent::Text { text: text.clone() }]
        }
    });
    tracing::debug!(id, status, bytes = text.len(), "tool update");
    Some(SessionEvent::ToolUpdate {
        id: id.to_string(),
        title: None,
        status: Some(status.into()),
        content,
    })
}

/// The text of a user message, whose content is a string or a list of parts.
fn user_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// A resumed session's messages as history: each prompt with what the agent did after it.
/// Messages before the first prompt (the system message) are not shown.
fn history(messages: &Value) -> History {
    let mut history = History::default();
    let mut shown = HashSet::new();
    for message in messages.as_array().into_iter().flatten() {
        match message["role"].as_str() {
            Some("user") => {
                history
                    .turns
                    .push((user_text(&message["content"]), Vec::new()));
            }
            Some("assistant") => {
                let Some((_, events)) = history.turns.last_mut() else {
                    continue;
                };
                for block in message["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let Some(text) = block["text"].as_str().filter(|t| !t.is_empty())
                            else {
                                continue;
                            };
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
                        Some("thinking") => {
                            if let Some(text) = block["thinking"].as_str().filter(|t| !t.is_empty())
                            {
                                events.push(SessionEvent::Thought { text: text.into() });
                            }
                        }
                        Some("toolCall") => {
                            let id = block["id"].as_str().unwrap_or_default();
                            let tool = block["name"].as_str().unwrap_or("tool");
                            events.extend(tool_call(id, tool, &block["arguments"], &mut shown));
                        }
                        _ => {}
                    }
                }
            }
            Some("toolResult") => {
                let Some((_, events)) = history.turns.last_mut() else {
                    continue;
                };
                let id = message["toolCallId"].as_str().unwrap_or_default();
                let failed = message["isError"] == true;
                let was_shown = shown.remove(id);
                events.extend(tool_result(id, message, failed, was_shown));
            }
            _ => {}
        }
    }
    history
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resumed_session_becomes_prompts_with_their_output() {
        let messages = json!([
            {"role": "system", "content": ""},
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "assistant", "stopReason": "toolUse", "content": [
                {"type": "thinking", "thinking": "run it"},
                {"type": "toolCall", "id": "c", "name": "bash", "arguments": {"command": "echo hi"}},
            ]},
            {"role": "toolResult", "toolCallId": "c", "toolName": "bash", "isError": false, "content": [{"type": "text", "text": "hi\n"}]},
            {"role": "assistant", "stopReason": "stop", "content": [{"type": "text", "text": "done"}]},
        ]);
        let history = history(&messages);
        assert_eq!(history.turns.len(), 1);
        let (prompt, events) = &history.turns[0];
        assert_eq!(prompt, "hi");
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(
            matches!(&events[1], SessionEvent::ToolCall { title, tool_kind, .. } if title == "echo hi" && tool_kind == "execute")
        );
        assert!(
            matches!(&events[2], SessionEvent::ToolUpdate { status: Some(s), content: Some(c), .. } if s == "completed" && c == &vec![ToolContent::Text { text: "hi\n".into() }])
        );
        assert!(matches!(&events[3], SessionEvent::Text { text } if text == "done"));
    }
}
