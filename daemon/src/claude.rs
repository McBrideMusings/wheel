//! The agent interface ([`crate::agent`]) over Claude Code itself: one `claude --print` process
//! per conversation, talking stream-json on its stdin and stdout.
//!
//! A prompt is one stdin line `{"type":"user","message":{"role":"user","content":…}}`. Stdout
//! carries `stream_event` lines (the reply and thinking as they stream), `assistant` lines (whole
//! messages, whose `tool_use` blocks become tool calls), `user` lines (tool results), `system`
//! lines (`init` names the model, `status` a mode change), and one `result` per turn with its
//! usage. A permission prompt arrives as a `control_request` of subtype `can_use_tool` and is
//! answered with a `control_response` saying `allow` or `deny`; the host's own requests
//! (`initialize`, `interrupt`, `set_permission_mode`, `set_model`) go the other way.
//!
//! Claude Code keeps every conversation in `<config dir>/projects/<cwd>/<id>.jsonl`, which is also
//! where a conversation's history, and the list of past conversations, are read from here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use wheel_proto::{
    ConfigChoice, ConfigKind, ConfigOption, ContextUse, ConversationInfo, PermOption, PlanItem,
    SessionEvent, ToolContent,
};

use crate::agent::{
    self, Agent, AgentEvent, AgentInput, Conversation, Input, Launch, Process, ProcessGroup,
};
use crate::resume::{between_turns, claude_project_dir, Agent as Harness};

/// Tools the agent runs without asking, and the ones it never runs unasked: `wheel` itself is
/// Wheel's own surface, except for answering a permission request, which is the person's.
const ALLOWED: &str = "Bash(wheel:*)";
const DISALLOWED: &str = "Bash(wheel perm answer:*)";

/// The permission modes the `mode` control offers, in order, with their names.
const MODES: [(&str, &str); 5] = [
    ("default", "Default"),
    ("acceptEdits", "Accept edits"),
    ("plan", "Plan"),
    ("auto", "Auto"),
    ("dontAsk", "Don't ask"),
];

/// The options a permission request offers. `wheel perm answer <id> allow|deny` picks them by
/// kind. "Always allow" is offered when Claude Code suggests rules that would stop it asking again,
/// and answering with it hands those rules back.
fn perm_options(always: bool) -> Vec<PermOption> {
    let option = |id: &str, name: &str, kind: &str| PermOption {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
    };
    let mut options = vec![option("allow", "Allow", "allow_once")];
    if always {
        options.push(option("allow_always", "Always allow", "allow_always"));
    }
    options.push(option("deny", "Deny", "reject_once"));
    options
}

/// A command line split into words the way a shell would: quotes group, a backslash escapes.
pub(crate) fn words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.get_or_insert_with(String::new).push(next);
                }
            }
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    words
}

/// The arguments the driver adds after the command's own.
fn arguments(launch: &Launch) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if launch.brief.is_some() {
        args.extend(["--allowedTools".into(), ALLOWED.into()]);
        args.extend(["--disallowedTools".into(), DISALLOWED.into()]);
    }
    args.extend(
        [
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
        ]
        .map(String::from),
    );
    match &launch.conversation {
        Conversation::New(id) => args.extend(["--session-id".into(), id.clone()]),
        Conversation::Resume(id) => args.extend(["--resume".into(), id.clone()]),
    }
    if let Some(brief) = &launch.brief {
        args.extend(["--append-system-prompt".into(), brief.clone()]);
    }
    args
}

/// Start `claude --print` for `launch`.
pub fn launch(launch: Launch) -> Result<Agent, String> {
    let Process {
        pid,
        child,
        stdin,
        stdout,
        last_error,
    } = agent::spawn(&launch, arguments(&launch), "CLAUDE_CONFIG_DIR")?;
    let (input, inputs) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let driver = Driver {
        pid,
        stdin,
        events,
        pending: HashMap::new(),
        next_request: 1,
        permissions: HashMap::new(),
        options: Vec::new(),
        models: Vec::new(),
        in_turn: false,
        interrupted: false,
        streamed: HashSet::new(),
        calls: Calls::default(),
        last_usage: None,
    };
    tokio::spawn(driver.run(stdout, inputs, child, last_error));
    Ok(Agent {
        input: AgentInput(input),
        events: events_rx,
        _group: ProcessGroup(pid),
    })
}

/// A request this side sent the agent, waiting for its `control_response`.
enum Pending {
    Initialize,
    Interrupt,
    SetOption {
        id: String,
        value: String,
        reply: oneshot::Sender<Result<Vec<ConfigOption>, String>>,
    },
}

/// One agent process's state, owned by the one task that reads its output and writes its input.
struct Driver {
    pid: u32,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedSender<AgentEvent>,
    pending: HashMap<String, Pending>,
    next_request: u64,
    /// Permission requests waiting on an answer, by the agent's request id, with the tool input
    /// an `allow` hands back and the rules an `allow_always` does.
    permissions: HashMap<String, (Value, Value)>,
    options: Vec<ConfigOption>,
    /// Each model choice's value and the model it resolves to.
    models: Vec<(String, String)>,
    /// A prompt has been sent and its `result` has not arrived.
    in_turn: bool,
    /// The running turn was interrupted, so it ends as `cancelled`.
    interrupted: bool,
    /// Messages whose text arrived as stream events, so their whole copy is not sent again.
    streamed: HashSet<String>,
    /// The conversation's tool calls, for their results.
    calls: Calls,
    /// The model and the tokens of the last top-level request: the conversation as the model saw it.
    last_usage: Option<(String, u64)>,
}

impl Driver {
    async fn run(
        mut self,
        stdout: BufReader<tokio::process::ChildStdout>,
        mut inputs: mpsc::UnboundedReceiver<Input>,
        child: tokio::process::Child,
        last_error: Arc<Mutex<String>>,
    ) {
        let mut lines = stdout.lines();
        let mut failed = None;
        if let Err(e) = self
            .request(Pending::Initialize, json!({"subtype": "initialize"}))
            .await
        {
            failed = Some(e);
        }
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

    /// Send the agent a control request of this side's.
    async fn request(&mut self, pending: Pending, request: Value) -> Result<(), String> {
        let id = format!("wheel-{}", self.next_request);
        self.next_request += 1;
        self.pending.insert(id.clone(), pending);
        self.write(json!({"type": "control_request", "request_id": id, "request": request}))
            .await
    }

    async fn input(&mut self, input: Input) -> Result<(), String> {
        match input {
            Input::Prompt(text) => {
                self.in_turn = true;
                self.interrupted = false;
                self.write(json!({"type": "user", "message": {"role": "user", "content": text}}))
                    .await
            }
            Input::Cancel => {
                if !self.in_turn || self.interrupted {
                    return Ok(());
                }
                self.interrupted = true;
                tracing::info!(pid = self.pid, "agent interrupted");
                self.request(Pending::Interrupt, json!({"subtype": "interrupt"}))
                    .await
            }
            Input::Answer { request, choice } => {
                let Some((input, suggestions)) = self.permissions.remove(&request) else {
                    return Ok(());
                };
                let response = match choice.as_deref() {
                    Some("allow") => json!({"behavior": "allow", "updatedInput": input}),
                    Some("allow_always") => {
                        json!({"behavior": "allow", "updatedInput": input, "updatedPermissions": suggestions})
                    }
                    Some(_) => json!({"behavior": "deny", "message": "The person denied this."}),
                    None => json!({"behavior": "deny", "message": "The request was withdrawn."}),
                };
                self.write(json!({"type": "control_response", "response": {"subtype": "success", "request_id": request, "response": response}}))
                    .await
            }
            Input::SetOption { id, value, reply } => {
                let request = match option_request(&self.options, &id, &value) {
                    Ok(request) => request,
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return Ok(());
                    }
                };
                self.request(Pending::SetOption { id, value, reply }, request)
                    .await
            }
        }
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
        // A subagent's own messages belong to its tool call, not to the conversation.
        if !message["parent_tool_use_id"].is_null() {
            return Ok(());
        }
        match message["type"].as_str() {
            Some("stream_event") => self.stream_event(&message["event"]),
            Some("assistant") => self.assistant(&message["message"]),
            Some("user") => {
                // A local command such as `/compact` answers as a user message wrapping its output.
                if let Some(text) = local_command_output(&message["message"]["content"]) {
                    self.emit(AgentEvent::Transcript(SessionEvent::Text { text }));
                }
                for event in tool_results(&message["message"]["content"], &self.calls) {
                    self.emit(AgentEvent::Transcript(event));
                }
            }
            Some("system") => self.system(&message),
            Some("result") => self.result(&message),
            Some("conversation_reset") => {
                // `/clear` keeps the process and starts a new conversation under a new id.
                if let Some(id) = message["new_conversation_id"].as_str() {
                    tracing::info!(pid = self.pid, id, trigger = ?message["trigger"].as_str(), "agent started a new conversation");
                    self.emit(AgentEvent::Conversation { id: id.into() });
                }
            }
            Some("control_request") => self.control_request(&message).await?,
            Some("control_response") => self.control_response(&message["response"]),
            _ => {}
        }
        Ok(())
    }

    fn stream_event(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("message_start") => {
                if let Some(id) = event["message"]["id"].as_str() {
                    self.streamed.insert(id.to_string());
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str() {
                            self.emit(AgentEvent::Transcript(SessionEvent::Text {
                                text: text.into(),
                            }));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta["thinking"].as_str() {
                            self.emit(AgentEvent::Transcript(SessionEvent::Thought {
                                text: text.into(),
                            }));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, message: &Value) {
        if let Some(used) = usage_tokens(&message["usage"]) {
            let model = message["model"].as_str().unwrap_or_default().to_string();
            if model != "<synthetic>" {
                self.last_usage = Some((model, used));
            }
        }
        let streamed = message["id"]
            .as_str()
            .is_some_and(|id| self.streamed.contains(id));
        for block in message["content"].as_array().into_iter().flatten() {
            if streamed && matches!(block["type"].as_str(), Some("text" | "thinking")) {
                continue;
            }
            if let Some(event) = assistant_block(block, &mut self.calls) {
                self.emit(AgentEvent::Transcript(event));
            }
        }
    }

    fn system(&mut self, message: &Value) {
        match message["subtype"].as_str() {
            Some("init") => {
                if let Some(model) = message["model"].as_str() {
                    self.resolved_model(model);
                }
                if let Some(commands) = message["slash_commands"].as_array() {
                    let names = commands.iter().filter_map(|c| c.as_str().map(String::from));
                    self.emit(AgentEvent::Commands(names.collect()));
                }
            }
            Some("status") => {
                if let Some(mode) = message["permissionMode"].as_str() {
                    if set_current(&mut self.options, "mode", mode) {
                        tracing::info!(pid = self.pid, mode, "agent changed its mode");
                        self.emit(AgentEvent::Mode(mode.into()));
                    }
                }
            }
            _ => {}
        }
    }

    /// The model the conversation runs on, as `system/init` names it. The model control shows the
    /// choice that resolves to it, unless the one it shows already does.
    fn resolved_model(&mut self, model: &str) {
        let current = self
            .options
            .iter()
            .find(|o| o.id == "model")
            .and_then(|o| match &o.kind {
                ConfigKind::Select { current, .. } => Some(current.clone()),
                _ => None,
            });
        let resolves = |value: &str| self.models.iter().any(|(v, r)| v == value && r == model);
        if current.as_deref().is_some_and(resolves) {
            return;
        }
        let Some(value) = self
            .models
            .iter()
            .find(|(_, r)| r == model)
            .map(|(v, _)| v.clone())
        else {
            return;
        };
        if set_current(&mut self.options, "model", &value) {
            self.emit(AgentEvent::Options(self.options.clone()));
        }
    }

    fn result(&mut self, message: &Value) {
        self.in_turn = false;
        self.streamed.clear();
        // Nothing asked during the turn can still be answered.
        self.permissions.clear();
        if let Some((model, used)) = self.last_usage.take() {
            let size = message["modelUsage"][&model]["contextWindow"].as_u64();
            if let Some(size) = size {
                self.emit(AgentEvent::Context(Some(ContextUse { used, size })));
            }
        }
        let interrupted = std::mem::take(&mut self.interrupted);
        if interrupted {
            self.emit(AgentEvent::TurnEnd {
                stop_reason: "cancelled".into(),
            });
            return;
        }
        if message["is_error"] == true {
            let said = message["result"]
                .as_str()
                .filter(|r| !r.is_empty())
                .or_else(|| message["subtype"].as_str())
                .unwrap_or("the turn failed");
            self.emit(AgentEvent::TurnFailed {
                message: said.to_string(),
            });
            return;
        }
        let stop_reason = message["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();
        self.emit(AgentEvent::TurnEnd { stop_reason });
    }

    async fn control_request(&mut self, message: &Value) -> Result<(), String> {
        let id = message["request_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let request = &message["request"];
        if request["subtype"] != "can_use_tool" {
            tracing::info!(pid = self.pid, subtype = %request["subtype"], "agent request refused: not one this host answers");
            return self
                .write(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "not supported by this host"}}))
                .await;
        }
        let tool = request["tool_name"].as_str().unwrap_or("tool");
        let input = request["input"].clone();
        let (title, _) = tool_title(tool, &input);
        let tool_call = request["tool_use_id"].as_str().map(String::from);
        let suggestions = request["permission_suggestions"].clone();
        let always = suggestions.as_array().is_some_and(|s| !s.is_empty());
        self.permissions.insert(id.clone(), (input, suggestions));
        self.emit(AgentEvent::Permission {
            request: id,
            title,
            tool_call,
            options: perm_options(always),
        });
        Ok(())
    }

    fn control_response(&mut self, response: &Value) {
        let id = response["request_id"].as_str().unwrap_or_default();
        let Some(pending) = self.pending.remove(id) else {
            return;
        };
        let refused = (response["subtype"] != "success").then(|| {
            response["error"]
                .as_str()
                .unwrap_or("the agent refused")
                .to_string()
        });
        match pending {
            Pending::Initialize => {
                if let Some(e) = refused {
                    tracing::warn!(pid = self.pid, error = %e, "agent initialize refused");
                }
                let answer = &response["response"];
                self.models = answer["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        Some((
                            m["value"].as_str()?.to_string(),
                            m["resolvedModel"].as_str().unwrap_or_default().to_string(),
                        ))
                    })
                    .collect();
                self.options = initial_options(answer);
                tracing::info!(pid = self.pid, models = self.models.len(), mode = %answer["current_permission_mode"], version = %answer["claude_code_version"], "agent ready");
                self.emit(AgentEvent::Ready {
                    options: self.options.clone(),
                    conversation: None,
                    history: None,
                });
            }
            Pending::Interrupt => {}
            Pending::SetOption { id, value, reply } => {
                let answer = match refused {
                    Some(e) => Err(e),
                    None => {
                        set_current(&mut self.options, &id, &value);
                        Ok(self.options.clone())
                    }
                };
                tracing::info!(pid = self.pid, option = %id, %value, ok = answer.is_ok(), "agent option set");
                let _ = reply.send(answer);
            }
        }
    }
}

/// The `mode` and `model` controls as `initialize` describes them.
fn initial_options(answer: &Value) -> Vec<ConfigOption> {
    let mode = answer["current_permission_mode"]
        .as_str()
        .unwrap_or("default")
        .to_string();
    let mut modes: Vec<ConfigChoice> = MODES
        .iter()
        .map(|(value, name)| ConfigChoice {
            value: (*value).into(),
            name: (*name).into(),
            description: None,
        })
        .collect();
    if !modes.iter().any(|c| c.value == mode) {
        modes.push(ConfigChoice {
            value: mode.clone(),
            name: mode.clone(),
            description: None,
        });
    }
    let mut options = vec![ConfigOption {
        id: "mode".into(),
        name: "Mode".into(),
        description: None,
        category: Some("mode".into()),
        kind: ConfigKind::Select {
            current: mode,
            choices: modes,
        },
    }];
    let models: Vec<ConfigChoice> = answer["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(ConfigChoice {
                value: m["value"].as_str()?.to_string(),
                name: m["displayName"].as_str().unwrap_or_default().to_string(),
                description: m["description"].as_str().map(String::from),
            })
        })
        .collect();
    if let Some(first) = models.first() {
        options.push(ConfigOption {
            id: "model".into(),
            name: "Model".into(),
            description: None,
            category: Some("model".into()),
            kind: ConfigKind::Select {
                current: first.value.clone(),
                choices: models,
            },
        });
    }
    options
}

/// The control request that sets option `id` to `value`, or why it cannot be set.
fn option_request(options: &[ConfigOption], id: &str, value: &str) -> Result<Value, String> {
    let Some(option) = options.iter().find(|o| o.id == id) else {
        return Err(format!("the agent has no option {id}"));
    };
    let ConfigKind::Select { choices, .. } = &option.kind else {
        return Err(format!("the agent cannot set {id}"));
    };
    if !choices.iter().any(|c| c.value == value) {
        let offered: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        return Err(format!("{id} does not take {value}; it takes {offered:?}"));
    }
    Ok(match id {
        "mode" => json!({"subtype": "set_permission_mode", "mode": value}),
        _ => json!({"subtype": "set_model", "model": value}),
    })
}

/// Point select `id`'s current value at `value`; true when it moved.
fn set_current(options: &mut [ConfigOption], id: &str, value: &str) -> bool {
    let Some(ConfigKind::Select { current, .. }) =
        options.iter_mut().find(|o| o.id == id).map(|o| &mut o.kind)
    else {
        return false;
    };
    if current == value {
        return false;
    }
    *current = value.to_string();
    true
}

/// The tokens a request's usage block counts: its input, cache and output.
fn usage_tokens(usage: &Value) -> Option<u64> {
    if !usage.is_object() {
        return None;
    }
    let part = |key: &str| usage[key].as_u64().unwrap_or(0);
    Some(
        part("input_tokens")
            + part("cache_creation_input_tokens")
            + part("cache_read_input_tokens")
            + part("output_tokens"),
    )
}

fn input_str<'a>(input: &'a Value, key: &str) -> &'a str {
    input[key].as_str().unwrap_or_default()
}

/// A tool call's title and its kind, from the tool's name and input.
fn tool_title(tool: &str, input: &Value) -> (String, &'static str) {
    let path = || {
        let path = input_str(input, "file_path");
        if path.is_empty() {
            input_str(input, "notebook_path")
        } else {
            path
        }
    };
    let (title, kind) = match tool {
        "Bash" => (input_str(input, "command").to_string(), "execute"),
        "Read" => (format!("Read {}", path()), "read"),
        "Edit" | "MultiEdit" | "NotebookEdit" => (format!("Edit {}", path()), "edit"),
        "Write" => (format!("Write {}", path()), "edit"),
        "Grep" => (format!("grep {}", input_str(input, "pattern")), "search"),
        "Glob" => (format!("Find {}", input_str(input, "pattern")), "search"),
        "WebFetch" => (format!("Fetch {}", input_str(input, "url")), "fetch"),
        "WebSearch" => (format!("Search {}", input_str(input, "query")), "fetch"),
        "Task" | "Agent" => (input_str(input, "description").to_string(), "think"),
        other => (other.to_string(), "other"),
    };
    // A title the input left empty is the tool's name.
    let title = if title.trim().is_empty() || title.ends_with(' ') {
        tool.to_string()
    } else {
        title
    };
    (title, kind)
}

/// What a tool call shows of its input: an edit's diff, a write's new file.
fn tool_input_content(tool: &str, input: &Value) -> Vec<ToolContent> {
    let path = input_str(input, "file_path").to_string();
    let diff = |old: &Value, new: &Value| ToolContent::Diff {
        path: path.clone(),
        old_text: old.as_str().map(String::from),
        new_text: new.as_str().unwrap_or_default().to_string(),
    };
    match tool {
        "Edit" => vec![diff(&input["old_string"], &input["new_string"])],
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| diff(&e["old_string"], &e["new_string"]))
            .collect(),
        "Write" => vec![diff(&Value::Null, &input["content"])],
        _ => Vec::new(),
    }
}

/// What a conversation's tool calls need remembered until their results arrive.
#[derive(Default)]
struct Calls {
    /// TodoWrite calls, which show as a plan rather than a tool line, so their results show nothing.
    plans: HashSet<String>,
    /// Calls whose line shows their input — an edit's diff — which a successful result keeps.
    shown: HashSet<String>,
}

/// One block of an assistant message as an event: text, a thought, a tool call, or a plan.
/// The output a local command printed, from the user message that carries it between
/// `<local-command-stdout>` tags; none for any other message, or an empty output.
fn local_command_output(content: &Value) -> Option<String> {
    let text = content.as_str()?.trim();
    let inner = text
        .strip_prefix("<local-command-stdout>")?
        .strip_suffix("</local-command-stdout>")?
        .trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

fn assistant_block(block: &Value, calls: &mut Calls) -> Option<SessionEvent> {
    match block["type"].as_str()? {
        "text" => Some(SessionEvent::Text {
            text: block["text"].as_str()?.to_string(),
        }),
        "thinking" => Some(SessionEvent::Thought {
            text: block["thinking"].as_str()?.to_string(),
        }),
        "tool_use" => {
            let id = block["id"].as_str()?.to_string();
            let tool = block["name"].as_str().unwrap_or("tool");
            let input = &block["input"];
            if tool == "TodoWrite" {
                calls.plans.insert(id);
                let entries = input["todos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| PlanItem {
                        content: input_str(t, "content").to_string(),
                        status: input_str(t, "status").to_string(),
                    })
                    .collect();
                return Some(SessionEvent::Plan { entries });
            }
            let (title, kind) = tool_title(tool, input);
            let content = tool_input_content(tool, input);
            if !content.is_empty() {
                calls.shown.insert(id.clone());
            }
            tracing::debug!(id, tool, items = content.len(), "tool call");
            Some(SessionEvent::ToolCall {
                id,
                title,
                tool_kind: kind.into(),
                status: "pending".into(),
                content,
            })
        }
        _ => None,
    }
}

/// The tool results in a user message's content, each as the update that finishes its call. A
/// call that shows its input keeps it unless it failed; any other shows what the tool returned.
fn tool_results(content: &Value, calls: &Calls) -> Vec<SessionEvent> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "tool_result")
        .filter_map(|b| {
            let id = b["tool_use_id"].as_str()?.to_string();
            if calls.plans.contains(&id) {
                return None;
            }
            let text = match &b["content"] {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            let status = if b["is_error"] == true {
                "failed"
            } else {
                "completed"
            };
            tracing::debug!(id, status, bytes = text.len(), "tool update");
            let keeps_input = calls.shown.contains(&id) && status == "completed";
            Some(SessionEvent::ToolUpdate {
                content: (!text.is_empty() && !keeps_input)
                    .then(|| vec![ToolContent::Text { text }]),
                id,
                title: None,
                status: Some(status.into()),
            })
        })
        .collect()
}

/// The configuration directory whose `projects/` holds the conversations: the pane's own, else
/// the daemon's.
pub fn store(config_dir: Option<&Path>) -> Option<PathBuf> {
    config_dir
        .map(Path::to_path_buf)
        .or_else(|| Harness::Claude.daemon_home())
}

/// The directory as Claude Code names it in the store: the path the process sees, with every
/// symlink resolved.
fn store_cwd(cwd: &Path) -> String {
    std::fs::canonicalize(cwd)
        .unwrap_or_else(|_| cwd.to_path_buf())
        .display()
        .to_string()
}

fn transcript(home: &Path, cwd: &Path, id: &str) -> PathBuf {
    claude_project_dir(home, &store_cwd(cwd)).join(format!("{id}.jsonl"))
}

/// A fresh conversation id: a random version 4 UUID, as Claude Code's own are.
pub fn new_conversation_id() -> String {
    use std::io::Read;
    let mut raw = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut raw));
    if read.is_err() {
        // Unique enough for a conversation id when the random device is unreadable.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        raw = (nanos ^ (u128::from(std::process::id()) << 64)).to_be_bytes();
    }
    raw[6] = (raw[6] & 0x0f) | 0x40;
    raw[8] = (raw[8] & 0x3f) | 0x80;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A conversation's history read from its transcript: agent output before its first prompt, and
/// each prompt with the agent's output after it.
#[derive(Default, Debug)]
pub struct History {
    pub before: Vec<SessionEvent>,
    pub turns: Vec<(String, Vec<SessionEvent>)>,
}

/// The prompt a user entry carries, if it is one the person wrote.
fn prompt_text(entry: &Value) -> Option<String> {
    if entry["isMeta"] == true || between_turns(&entry["message"]) {
        return None;
    }
    let text = match &entry["message"]["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| b["type"] == "tool_result") {
                return None;
            }
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        }
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// Read conversation `id`'s history, or None when the store does not hold it.
pub fn history(home: &Path, cwd: &Path, id: &str) -> Option<History> {
    let text = std::fs::read_to_string(transcript(home, cwd, id)).ok()?;
    let mut history = History::default();
    let mut calls = Calls::default();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["isSidechain"] == true {
            continue;
        }
        let events = match entry["type"].as_str() {
            Some("user") => {
                if let Some(prompt) = prompt_text(&entry) {
                    history.turns.push((prompt, Vec::new()));
                    continue;
                }
                tool_results(&entry["message"]["content"], &calls)
            }
            Some("assistant") => entry["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| assistant_block(b, &mut calls))
                .collect(),
            _ => continue,
        };
        match history.turns.last_mut() {
            Some((_, output)) => output.extend(events),
            None => history.before.extend(events),
        }
    }
    Some(history)
}

/// How much of a transcript's start is read for its title.
const TITLE_HEAD: usize = 256 * 1024;

/// The conversations the store holds for `cwd`, newest first: each titled after its first prompt
/// and timed by its file. The store belongs to the directory, so this lists whatever else ran
/// Claude Code there too (ADR-0006).
pub fn conversations(home: &Path, cwd: &Path) -> Vec<ConversationInfo> {
    use std::io::Read;
    let dir = claude_project_dir(home, &store_cwd(cwd));
    let shown = cwd.display().to_string();
    let mut found: Vec<(std::time::SystemTime, ConversationInfo)> = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let mut head = Vec::new();
        if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.take(TITLE_HEAD as u64).read_to_end(&mut head);
        }
        let title = String::from_utf8_lossy(&head)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|e| e["type"] == "user" && e["isSidechain"] != true)
            .find_map(|e| prompt_text(&e));
        found.push((
            modified,
            ConversationInfo {
                id: id.to_string(),
                cwd: shown.clone(),
                title,
                updated_at: Some(iso8601(modified)),
                current: false,
            },
        ));
    }
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().map(|(_, info)| info).collect()
}

/// A time as an ISO 8601 UTC timestamp with microseconds, which sorts as text. Civil date from
/// days after Howard Hinnant's `civil_from_days`.
fn iso8601(at: std::time::SystemTime) -> String {
    let at = at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (seconds, micros) = (at.as_secs(), at.subsec_micros());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    // March-based years, so a leap day lands at the end of its own year.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}Z")
}

#[cfg(test)]
mod tests {
    use super::words;

    #[test]
    fn a_command_splits_into_words_as_a_shell_would() {
        assert_eq!(words("claude"), ["claude"]);
        assert_eq!(
            words(r#"'/My Tools/claude' --flag "a b" c\ d"#),
            ["/My Tools/claude", "--flag", "a b", "c d"]
        );
        assert_eq!(words(r#"x "" y"#), ["x", "", "y"]);
        assert!(words("  ").is_empty());
    }
}
