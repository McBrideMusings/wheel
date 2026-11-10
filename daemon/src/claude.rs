//! The agent interface ([`crate::driver`]) over Claude Code itself: one `claude --print` process
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
    AgentEvent, ToolContent,
};

use crate::driver::{
    self, DriverHandle, DriverEvent, DriverInput, Conversation, Input, Launch, Process, ProcessGroup,
};
use crate::resume::{between_turns, claude_project_dir, Harness};

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

/// Claude Code's `--permission-mode` values a pane can start in: the modes the `mode` control
/// offers, plus `bypassPermissions`, which only a start can choose.
pub(crate) fn start_modes() -> Vec<&'static str> {
    let mut modes: Vec<&str> = MODES.iter().map(|(v, _)| *v).collect();
    modes.push("bypassPermissions");
    modes
}

/// The options a permission request offers. `wheel perm answer <id> allow|deny` picks them by
/// kind. "Always allow" is offered when Claude Code suggests rules that would stop it asking again,
/// and answering with it hands those rules back.
fn perm_options(always: bool) -> Vec<PermOption> {
    let option = |id: &str, name: &str, kind: &str| PermOption {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        description: None,
    };
    let mut options = vec![option("allow", "Allow", "allow_once")];
    if always {
        options.push(option("allow_always", "Always allow", "allow_always"));
    }
    options.push(option("deny", "Deny", "reject_once"));
    options
}

/// A question the agent put to the person with `AskUserQuestion`, waiting on one answer. The
/// tool's input holds up to four questions; they are asked one after another, each as its own
/// request, and the tool call is answered once the last has an answer.
struct Ask {
    /// The id of the agent's `can_use_tool` request, which the one final response answers.
    control: String,
    /// The tool's input, which the final response hands back with the answers added.
    input: Value,
    /// The answers so far: each question's text and the label of the option picked.
    answers: serde_json::Map<String, Value>,
    /// The index of the question now waiting.
    at: usize,
    tool_call: Option<String>,
}

/// The request that asks question `ask.at`: its text is the reason, each option's label a choice
/// (id `pick:<index>`, kind `answer`) with its description, and Skip refuses the whole tool call.
fn question_request(ask: &Ask, id: &str) -> Option<DriverEvent> {
    let question = &ask.input["questions"][ask.at];
    let text = question["question"].as_str()?;
    let choices = question["options"].as_array()?.iter().map(|option| {
        Some((option["label"].as_str()?.to_string(), option["description"].as_str().map(String::from)))
    });
    Some(DriverEvent::Permission {
        request: id.to_string(),
        title: text.to_string(),
        tool_call: ask.tool_call.clone(),
        options: driver::question_options(choices.collect::<Option<Vec<_>>>()?),
        why: wheel_proto::PermWhy {
            reason: question["header"].as_str().map(String::from),
            ..Default::default()
        },
    })
}

/// The permission updates an "Always allow" hands back: Claude Code's own suggestions when it sent
/// any, else one rule Wheel builds for this agent. For `Bash` the rule covers the command's first
/// word (`Bash(ls:*)`); any other tool gets a rule for the whole tool. A `Bash` call with no
/// command gets none, since a rule for all of `Bash` would allow everything.
fn always_updates(tool: &str, input: &Value, suggestions: &Value) -> Value {
    if suggestions.as_array().is_some_and(|s| !s.is_empty()) {
        return suggestions.clone();
    }
    let rule = if tool == "Bash" {
        let command = input_str(input, "command");
        // A leading `FOO=1` is an environment assignment, not the program.
        let Some(first) = words(command).into_iter().find(|w| !w.contains('=')) else {
            return json!([]);
        };
        json!({"toolName": tool, "ruleContent": format!("{first}:*")})
    } else {
        json!({"toolName": tool})
    };
    json!([{"type": "addRules", "rules": [rule], "behavior": "allow", "destination": "session"}])
}

/// The rules in permission updates as Claude Code writes them: `Bash(ls:*)`, `Edit`.
fn rule_names(updates: &Value) -> Option<String> {
    let names: Vec<String> = updates
        .as_array()?
        .iter()
        .flat_map(|u| u["rules"].as_array().into_iter().flatten())
        .filter_map(|r| {
            let tool = r["toolName"].as_str()?;
            Some(match r["ruleContent"].as_str() {
                Some(content) => format!("{tool}({content})"),
                None => tool.to_string(),
            })
        })
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
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
fn arguments(launch: &Launch, fork_at: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if launch.orchestrator {
        // The orchestrator runs only the whitelisted `wheel` commands: Bash is its one tool, and
        // `dontAsk` refuses everything the list does not name instead of asking a person.
        args.extend(["--tools".into(), "Bash".into()]);
        // No settings file is read, the person's or the project's: their allow rules and hooks
        // would otherwise widen what this agent may run.
        args.extend(["--setting-sources".into(), String::new()]);
        args.extend(["--allowedTools".into(), crate::orchestrator::ALLOWED.join(",")]);
        args.extend(["--permission-mode".into(), "dontAsk".into()]);
    } else if launch.brief.is_some() {
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
            // Each user message comes back as it is taken in, which is how a steered one is known
            // to have been.
            "--replay-user-messages",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
        ]
        .map(String::from),
    );
    if let Some(mode) = launch.permissions.permission_mode.as_ref().filter(|_| !launch.orchestrator) {
        args.extend(["--permission-mode".into(), mode.clone()]);
    }
    match &launch.conversation {
        Conversation::New(id) => args.extend(["--session-id".into(), id.clone()]),
        Conversation::Resume(id) => args.extend(["--resume".into(), id.clone()]),
        // Claude Code copies the conversation into a new one under an id it reports. With a
        // message to stop at, the copy ends there; without, it holds everything.
        Conversation::Fork { from, id, .. } => {
            args.extend([
                "--resume".into(),
                from.clone(),
                "--fork-session".into(),
                "--session-id".into(),
                id.clone(),
            ]);
            if let Some(at) = fork_at {
                args.extend(["--resume-session-at".into(), at.into()]);
            }
        }
    }
    if let Some(model) = &launch.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(brief) = &launch.brief {
        args.extend(["--append-system-prompt".into(), brief.clone()]);
    }
    args
}

/// Start `claude --print` for `launch`.
pub fn launch(launch: Launch) -> Result<DriverHandle, String> {
    let fork_at = match &launch.conversation {
        Conversation::Fork { from, turns, .. } => {
            let home = store(launch.config_dir.as_deref())
                .ok_or("the agent's configuration directory is not known")?;
            fork_point(&home, &launch.cwd, from, *turns)?
        }
        _ => None,
    };
    let Process {
        pid,
        child,
        stdin,
        stdout,
        last_error,
    } = driver::spawn(&launch, arguments(&launch, fork_at.as_deref()), "CLAUDE_CONFIG_DIR")?;
    let (input, inputs) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let driver = Driver {
        pid,
        stdin,
        events,
        model: launch.model.clone(),
        pending: HashMap::new(),
        next_request: 1,
        permissions: HashMap::new(),
        asks: HashMap::new(),
        options: Vec::new(),
        models: Vec::new(),
        in_turn: false,
        interrupted: false,
        streamed: HashMap::new(),
        streaming: None,
        calls: Calls::default(),
        last_usage: None,
        reset_from: None,
        steered: HashMap::new(),
    };
    tokio::spawn(driver.run(stdout, inputs, child, last_error));
    Ok(DriverHandle {
        input: DriverInput(input),
        events: events_rx,
        steers: true,
        withdraws: true,
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
    /// A steered message's withdrawal: the answer says whether the agent still held it.
    Withdraw { turn: u64 },
    /// A steered message pulled out of the agent's queue to be sent again as an interrupt, when the
    /// agent still held it.
    SendNow { turn: u64 },
}

/// A message written to the agent mid-turn that it has not taken in yet.
struct Steered {
    /// The id the agent echoes when it takes the message in.
    uuid: String,
    text: String,
}

/// The text and thoughts of one message that arrived as stream events, one entry per content
/// block by the block's index, and how many of the message's whole blocks have been seen.
#[derive(Default)]
struct Streamed {
    blocks: Vec<String>,
    seen: usize,
}

/// How a whole block compares with what its stream events carried.
#[derive(Debug, PartialEq)]
enum Rest<'a> {
    /// The stream carried all of it.
    Nothing,
    /// The stream carried the start; this is the end it left out.
    Missed(&'a str),
    /// The whole block is not the streamed text continued: it was rewritten after streaming.
    /// `streamed` is what the stream carried for it, which the whole block replaces.
    Revised { streamed: String },
}

impl Streamed {
    fn push(&mut self, index: usize, text: &str) {
        if self.blocks.len() <= index {
            self.blocks.resize(index + 1, String::new());
        }
        self.blocks[index].push_str(text);
    }

    /// Compare the next whole block (the message's blocks arrive in index order, one at a time,
    /// whatever their kind) with what streamed for the same index. Only a block the stream
    /// started is completed from the whole copy; one that differs is left as streamed, because
    /// sending it again shows the reader the same words twice.
    fn take_rest<'a>(&mut self, whole: &'a str) -> Rest<'a> {
        let have = self.blocks.get(self.seen).map_or("", String::as_str);
        self.seen += 1;
        if have.starts_with(whole) {
            Rest::Nothing
        } else if let Some(rest) = whole.strip_prefix(have) {
            Rest::Missed(rest)
        } else {
            Rest::Revised { streamed: have.to_string() }
        }
    }

    /// A whole block that is not text or a thought still takes its index.
    fn skip(&mut self) {
        self.seen += 1;
    }
}

/// One agent process's state, owned by the one task that reads its output and writes its input.
struct Driver {
    pid: u32,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedSender<DriverEvent>,
    /// The model the process was started with, which `initialize` is checked against.
    model: Option<String>,
    pending: HashMap<String, Pending>,
    next_request: u64,
    /// Permission requests waiting on an answer, by the agent's request id, with the tool input
    /// an `allow` hands back and the rules an `allow_always` does.
    permissions: HashMap<String, (Value, Value)>,
    /// Questions waiting on an answer, by the request id they were put under (`<agent's id>#<n>`).
    asks: HashMap<String, Ask>,
    options: Vec<ConfigOption>,
    /// Each model choice's value and the model it resolves to.
    models: Vec<(String, String)>,
    /// A prompt has been sent and its `result` has not arrived.
    in_turn: bool,
    /// The running turn was interrupted, so it ends as `cancelled`.
    interrupted: bool,
    /// Messages whose text arrived as stream events, with how much of it, so their whole copy is
    /// sent again only past what streamed.
    streamed: HashMap<String, Streamed>,
    /// The message the stream events now arriving belong to.
    streaming: Option<String>,
    /// The conversation's tool calls, for their results.
    calls: Calls,
    /// The model and the tokens of the last top-level request: the conversation as the model saw it.
    last_usage: Option<(String, u64)>,
    /// A `/clear` has started a new conversation, whose id is not known yet: the session id the
    /// reset message itself carried, which the first message of the new conversation differs from.
    reset_from: Option<String>,
    /// Messages written mid-turn that the agent has not taken in, by their turn.
    steered: HashMap<u64, Steered>,
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
                    // The agent dropped the agent; its process group is already killed.
                    None => return,
                },
            }
        }
        let error = driver::exit_error(child, failed, &last_error).await;
        tracing::info!(pid = self.pid, error = ?error, "agent process exited");
        let _ = self.events.send(DriverEvent::Exited { error });
    }

    fn emit(&self, event: DriverEvent) {
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
            Input::Steer { turn, text } => {
                self.write_steered(turn, text, false).await
            }
            Input::Withdraw { turns } => {
                for turn in turns {
                    let Some(held) = self.steered.get(&turn) else {
                        continue;
                    };
                    let uuid = held.uuid.clone();
                    self.request_cancel(Pending::Withdraw { turn }, &uuid).await?;
                }
                Ok(())
            }
            Input::SendNow { turn } => {
                let Some(held) = self.steered.get(&turn) else {
                    return Ok(());
                };
                let uuid = held.uuid.clone();
                self.request_cancel(Pending::SendNow { turn }, &uuid).await
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
                if let Some(ask) = self.asks.remove(&request) {
                    return self.answer_question(ask, choice.as_deref()).await;
                }
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

    /// Write a user message the agent takes in mid-turn, under a new id it echoes back. `now` has
    /// the agent abort what it is doing and run the message first.
    async fn write_steered(&mut self, turn: u64, text: String, now: bool) -> Result<(), String> {
        let uuid = new_conversation_id();
        let mut message = json!({
            "type": "user",
            "uuid": uuid,
            "message": {"role": "user", "content": text},
        });
        if now {
            message["priority"] = json!("now");
            // The turn it aborts ends as a stop, as with Escape.
            if self.in_turn {
                self.interrupted = true;
            }
        }
        self.steered.insert(turn, Steered { uuid, text });
        self.write(message).await
    }

    /// Ask the agent to drop the queued message `uuid`; `pending` is what to do with its answer.
    async fn request_cancel(&mut self, pending: Pending, uuid: &str) -> Result<(), String> {
        self.request(
            pending,
            json!({"subtype": "cancel_async_message", "message_uuid": uuid}),
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
        // The id a `/clear` announces is not the one the transcript is written under; the
        // messages that follow carry the real one in `session_id`.
        if let (Some(from), Some(id)) = (&self.reset_from, message["session_id"].as_str()) {
            if id != from {
                tracing::info!(pid = self.pid, id, "agent is in the conversation its first message after a clear names");
                self.emit(DriverEvent::Conversation { id: id.into() });
                self.reset_from = None;
            }
        }
        // A subagent's own messages belong to its tool call, not to the conversation.
        if !message["parent_tool_use_id"].is_null() {
            return Ok(());
        }
        match message["type"].as_str() {
            Some("stream_event") => {
                self.begin_unprompted_turn();
                self.stream_event(&message["event"])
            }
            Some("assistant") => {
                self.begin_unprompted_turn();
                self.assistant(&message["message"])
            }
            Some("user") => {
                // A steered message echoes back when the agent takes it in.
                if message["isReplay"] == true {
                    let uuid = message["uuid"].as_str().unwrap_or_default();
                    let turn = self
                        .steered
                        .iter()
                        .find(|(_, held)| held.uuid == uuid)
                        .map(|(turn, _)| *turn);
                    if let Some(turn) = turn {
                        self.steered.remove(&turn);
                        if !self.in_turn {
                            // It started a turn of its own: the one running ended first.
                            self.in_turn = true;
                            self.interrupted = false;
                        }
                        self.emit(DriverEvent::Steered { turn });
                    }
                    return Ok(());
                }
                // A local command such as `/compact` answers as a user message wrapping its output.
                if let Some(text) = local_command_output(&message["message"]["content"]) {
                    self.emit(DriverEvent::Transcript(AgentEvent::Text { text }));
                }
                for event in tool_results(&message["message"]["content"], &self.calls) {
                    self.emit(DriverEvent::Transcript(event));
                }
            }
            Some("system") => self.system(&message),
            Some("result") => self.result(&message),
            Some("conversation_reset") => {
                // `/clear` keeps the process and starts a new conversation. Its id comes from the
                // first message after this one, not from `new_conversation_id`.
                if let Some(from) = message["session_id"].as_str() {
                    tracing::info!(pid = self.pid, announced = ?message["new_conversation_id"].as_str(), trigger = ?message["trigger"].as_str(), "agent started a new conversation");
                    self.reset_from = Some(from.into());
                }
            }
            Some("control_request") => self.control_request(&message).await?,
            Some("control_response") => self.control_response(&message["response"]).await?,
            _ => {}
        }
        Ok(())
    }

    fn stream_event(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("message_start") => {
                if let Some(id) = event["message"]["id"].as_str() {
                    self.streamed.entry(id.to_string()).or_default();
                    self.streaming = Some(id.to_string());
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str() {
                            self.streamed_of_current().push(index, text);
                            self.emit(DriverEvent::Transcript(AgentEvent::Text {
                                text: text.into(),
                            }));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta["thinking"].as_str() {
                            self.streamed_of_current().push(index, text);
                            self.emit(DriverEvent::Transcript(AgentEvent::Thought {
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
        let id = message["id"].as_str();
        for block in message["content"].as_array().into_iter().flatten() {
            let kind = block["type"].as_str();
            if let Some(streamed) = id.and_then(|id| self.streamed.get_mut(id)) {
                if let Some(kind @ ("text" | "thinking")) = kind {
                    let thought = kind == "thinking";
                    let whole = block[if thought { "thinking" } else { "text" }]
                        .as_str()
                        .unwrap_or_default();
                    let event = match streamed.take_rest(whole) {
                        Rest::Nothing => None,
                        // The deltas that did arrive are the start of the block; what they
                        // missed is sent from the whole copy.
                        Rest::Missed(rest) => {
                            tracing::warn!(
                                pid = self.pid,
                                id,
                                missed = rest.len(),
                                "agent's stream events left out part of a block; sent from the whole message"
                            );
                            let text = rest.to_string();
                            Some(if thought {
                                AgentEvent::Thought { text }
                            } else {
                                AgentEvent::Text { text }
                            })
                        }
                        Rest::Revised { streamed } => {
                            tracing::info!(
                                pid = self.pid,
                                id,
                                streamed = streamed.len(),
                                whole = whole.len(),
                                "agent's whole block differs from its stream events; revised"
                            );
                            let text = whole.to_string();
                            Some(if thought {
                                AgentEvent::ThoughtRevised { streamed, text }
                            } else {
                                AgentEvent::TextRevised { streamed, text }
                            })
                        }
                    };
                    if let Some(event) = event {
                        self.emit(DriverEvent::Transcript(event));
                    }
                    continue;
                }
                streamed.skip();
            }
            if let Some(event) = assistant_block(block, &mut self.calls) {
                self.emit(DriverEvent::Transcript(event));
            }
        }
    }

    /// Claude Code starts a turn of its own when a subagent or a background task finishes; its
    /// first output with no prompt in flight says so, and the turn ends with a `result` like any
    /// other.
    fn begin_unprompted_turn(&mut self) {
        if self.in_turn {
            return;
        }
        self.in_turn = true;
        self.interrupted = false;
        tracing::info!(pid = self.pid, "agent began a turn no prompt started");
        self.emit(DriverEvent::TurnStart);
    }

    fn streamed_of_current(&mut self) -> &mut Streamed {
        let id = self.streaming.clone().unwrap_or_default();
        self.streamed.entry(id).or_default()
    }

    fn system(&mut self, message: &Value) {
        match message["subtype"].as_str() {
            Some("init") => {
                if let Some(model) = message["model"].as_str() {
                    self.resolved_model(model);
                }
                if let Some(commands) = message["slash_commands"].as_array() {
                    let names = commands.iter().filter_map(|c| c.as_str().map(String::from));
                    self.emit(DriverEvent::Commands(names.collect()));
                }
            }
            // The level signal: the whole set of live background tasks, sent whenever it changes.
            // An ambient task (one that is always there, such as a monitor) is not work.
            Some("background_tasks_changed") => {
                let live: Vec<&Value> = message["tasks"]
                    .as_array()
                    .map(|tasks| tasks.iter().filter(|t| t["ambient"] != true).collect())
                    .unwrap_or_default();
                let agents = live.iter().filter(|t| t["task_type"] == "local_agent").count() as u32;
                let shells = live.len() as u32 - agents;
                tracing::info!(pid = self.pid, shells, agents, "background tasks changed");
                self.emit(DriverEvent::Background { shells, agents });
            }
            Some("status") => {
                if let Some(mode) = message["permissionMode"].as_str() {
                    if set_current(&mut self.options, "mode", mode) {
                        tracing::info!(pid = self.pid, mode, "agent changed its mode");
                        self.emit(DriverEvent::Mode(mode.into()));
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
            self.emit(DriverEvent::Options(self.options.clone()));
        }
    }

    /// Whether the agent runs `model`. Claude Code lists a model given with `--model` among its
    /// choices whatever it is, and resolves only a name it knows (an alias such as `sonnet`) to a
    /// different model id; a full id (`claude-…`) names itself, and is the agent's to refuse at the
    /// first turn. A `[1m]`-style suffix is the agent's context-window selector, not part of the name.
    fn knows_model(&self, model: &str) -> bool {
        let name = model.split('[').next().unwrap_or(model);
        name.starts_with("claude-")
            || self
                .models
                .iter()
                .any(|(value, resolved)| (value == model || value == name) && resolved != value)
    }

    fn result(&mut self, message: &Value) {
        self.in_turn = false;
        self.streamed.clear();
        // Nothing asked during the turn can still be answered.
        self.permissions.clear();
        self.asks.clear();
        if let Some((model, used)) = self.last_usage.take() {
            let size = message["modelUsage"][&model]["contextWindow"].as_u64();
            if let Some(size) = size {
                self.emit(DriverEvent::Context(Some(ContextUse { used, size })));
            }
        }
        let interrupted = std::mem::take(&mut self.interrupted);
        if interrupted {
            self.emit(DriverEvent::TurnEnd {
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
            self.emit(DriverEvent::TurnFailed {
                message: said.to_string(),
            });
            return;
        }
        let stop_reason = message["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();
        self.emit(DriverEvent::TurnEnd { stop_reason });
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
        let tool_call = request["tool_use_id"].as_str().map(String::from);
        if tool == "AskUserQuestion" && input["questions"].as_array().is_some_and(|q| !q.is_empty()) {
            let ask = Ask { control: id, input, answers: serde_json::Map::new(), at: 0, tool_call };
            return self.put_question(ask).await;
        }
        let (title, _) = tool_title(tool, &input);
        // Fields of a `can_use_tool` request, from Claude Code 2.1.314's own schema and the SDK's
        // `CanUseTool` options (https://code.claude.com/docs/en/agent-sdk/typescript): `description`,
        // `title`, `display_name`, `decision_reason`, `decision_reason_type`, `blocked_path`,
        // `suppress_always_allow_rule`, `default_to_no`, `agent_id`, `permission_suggestions`.
        let text = |value: &Value| value.as_str().filter(|t| !t.trim().is_empty()).map(String::from);
        let updates = always_updates(tool, &input, &request["permission_suggestions"]);
        let always = request["suppress_always_allow_rule"] != true
            && updates.as_array().is_some_and(|u| !u.is_empty());
        let why = wheel_proto::PermWhy {
            reason: text(&request["description"]).or_else(|| text(&input["description"])),
            decision_reason: text(&request["decision_reason"]),
            blocked_path: text(&request["blocked_path"]),
            always_rule: rule_names(&updates).filter(|_| always),
        };
        self.permissions.insert(id.clone(), (input, updates));
        self.emit(DriverEvent::Permission {
            request: id,
            title,
            tool_call,
            options: perm_options(always),
            why,
        });
        Ok(())
    }

    /// Put question `ask.at` to the person, or refuse the tool call when the agent's input does not
    /// hold a question with options.
    async fn put_question(&mut self, ask: Ask) -> Result<(), String> {
        let id = format!("{}#{}", ask.control, ask.at);
        let Some(event) = question_request(&ask, &id) else {
            tracing::info!(pid = self.pid, "AskUserQuestion refused: question {} has no text or options", ask.at);
            return self.respond(&ask.control, json!({"behavior": "deny", "message": "The question had no text or options."})).await;
        };
        self.asks.insert(id, ask);
        self.emit(event);
        Ok(())
    }

    /// Take the person's choice for the waiting question: a pick records its label and asks the next
    /// question, or answers the tool call with every answer in `updatedInput.answers` after the
    /// last; anything else refuses the tool call.
    async fn answer_question(&mut self, mut ask: Ask, choice: Option<&str>) -> Result<(), String> {
        let question = &ask.input["questions"][ask.at];
        let label = driver::picked(choice)
            .and_then(|i| question["options"][i]["label"].as_str())
            .map(String::from);
        let Some(label) = label else {
            let message = match choice {
                Some(_) => "The person skipped the question.",
                None => "The request was withdrawn.",
            };
            return self.respond(&ask.control, json!({"behavior": "deny", "message": message})).await;
        };
        let text = question["question"].as_str().unwrap_or_default().to_string();
        ask.answers.insert(text, Value::String(label));
        ask.at += 1;
        if ask.at < ask.input["questions"].as_array().map_or(0, Vec::len) {
            return self.put_question(ask).await;
        }
        let mut input = ask.input;
        input["answers"] = Value::Object(ask.answers);
        self.respond(&ask.control, json!({"behavior": "allow", "updatedInput": input})).await
    }

    /// Answer the agent's `can_use_tool` request `id`.
    async fn respond(&mut self, id: &str, response: Value) -> Result<(), String> {
        self.write(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}}))
            .await
    }

    async fn control_response(&mut self, response: &Value) -> Result<(), String> {
        let id = response["request_id"].as_str().unwrap_or_default();
        let Some(pending) = self.pending.remove(id) else {
            return Ok(());
        };
        let refused = (response["subtype"] != "success").then(|| {
            response["error"]
                .as_str()
                .unwrap_or("the agent refused")
                .to_string()
        });
        match pending {
            Pending::Withdraw { turn } => {
                let held = response["response"]["cancelled"] == true;
                tracing::info!(pid = self.pid, turn, held, "steered message withdrawn");
                if held {
                    self.steered.remove(&turn);
                }
            }
            Pending::SendNow { turn } => {
                let held = response["response"]["cancelled"] == true;
                tracing::info!(pid = self.pid, turn, held, "steered message sent now");
                if held {
                    if let Some(Steered { text, .. }) = self.steered.remove(&turn) {
                        self.write_steered(turn, text, true).await?;
                    }
                }
            }
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
                if let Some(model) = &self.model {
                    if !self.knows_model(model) {
                        let offered: Vec<&str> = self
                            .models
                            .iter()
                            .filter(|(v, r)| v != r)
                            .map(|(v, _)| v.as_str())
                            .collect();
                        let error = format!(
                            "the agent does not know the model {model}; it offers {offered:?}, or a full model id starting claude-"
                        );
                        tracing::warn!(pid = self.pid, %model, %error, "agent started on an unknown model");
                        self.emit(DriverEvent::Exited { error: Some(error) });
                        return Ok(());
                    }
                }
                self.options = initial_options(answer, self.model.as_deref());
                tracing::info!(pid = self.pid, models = self.models.len(), mode = %answer["current_permission_mode"], version = %answer["claude_code_version"], "agent ready");
                self.emit(DriverEvent::Ready {
                    options: self.options.clone(),
                    conversation: None,
                    history: None,
                });
                // After Ready, which drops events before it: the `init` message that also lists commands arrives only with the first prompt,
                // so a fresh pane's composer would have no skill picker until a message is sent.
                if let Some(commands) = answer["commands"].as_array() {
                    let names = commands.iter().filter_map(|c| c["name"].as_str().map(String::from));
                    self.emit(DriverEvent::Commands(names.collect()));
                }
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
        Ok(())
    }
}

/// The `mode` and `model` controls as `initialize` describes them.
fn initial_options(answer: &Value, requested: Option<&str>) -> Vec<ConfigOption> {
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
    let mut models: Vec<ConfigChoice> = answer["models"]
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
    // A model asked for by a full id is not among the offered choices; it is the current one.
    if let Some(requested) = requested.filter(|r| !models.iter().any(|c| c.value == *r)) {
        models.insert(
            0,
            ConfigChoice {
                value: requested.into(),
                name: requested.into(),
                description: None,
            },
        );
    }
    let current = requested
        .map(String::from)
        .or_else(|| models.first().map(|c| c.value.clone()));
    if let Some(current) = current {
        options.push(ConfigOption {
            id: "model".into(),
            name: "Model".into(),
            description: None,
            category: Some("model".into()),
            kind: ConfigKind::Select {
                current,
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
        "Grep" => {
            let pattern = input_str(input, "pattern");
            match input_str(input, "path") {
                "" => (format!("grep {pattern}"), "search"),
                at => (format!("grep {pattern} in {at}"), "search"),
            }
        }
        "Glob" => (format!("Find {}", input_str(input, "pattern")), "search"),
        "WebFetch" => (format!("Fetch {}", input_str(input, "url")), "fetch"),
        "WebSearch" => (format!("Search {}", input_str(input, "query")), "fetch"),
        "Task" | "Agent" => (input_str(input, "description").to_string(), "subagent"),
        "Skill" => (format!("Skill {}", input_str(input, "skill")), "skill"),
        other if other.starts_with("mcp__") => (other.to_string(), "mcp"),
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

/// A tool call's line: `tool_title`, except that a command the agent described is titled by the
/// description and carries the command as its detail.
fn tool_line(tool: &str, input: &Value) -> (String, &'static str, Option<String>) {
    let (title, kind) = tool_title(tool, input);
    let description = input_str(input, "description").trim();
    if tool == "Bash" && !description.is_empty() {
        return (description.to_string(), kind, Some(title));
    }
    (title, kind, None)
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

fn assistant_block(block: &Value, calls: &mut Calls) -> Option<AgentEvent> {
    match block["type"].as_str()? {
        "text" => Some(AgentEvent::Text {
            text: block["text"].as_str()?.to_string(),
        }),
        "thinking" => Some(AgentEvent::Thought {
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
                return Some(AgentEvent::Plan { entries });
            }
            let (title, kind, detail) = tool_line(tool, input);
            let content = tool_input_content(tool, input);
            if !content.is_empty() {
                calls.shown.insert(id.clone());
            }
            tracing::debug!(id, tool, items = content.len(), "tool call");
            Some(AgentEvent::ToolCall {
                id,
                title,
                tool_kind: kind.into(),
                status: "pending".into(),
                content,
                detail,
            })
        }
        _ => None,
    }
}

/// The tool results in a user message's content, each as the update that finishes its call. A
/// call that shows its input keeps it unless it failed; any other shows what the tool returned.
fn tool_results(content: &Value, calls: &Calls) -> Vec<AgentEvent> {
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
            Some(AgentEvent::ToolUpdate {
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
    pub before: Vec<AgentEvent>,
    pub turns: Vec<(String, Vec<AgentEvent>)>,
}

/// Whether a user entry is Claude Code's note that a background task ended, not something the
/// person wrote: the entry says so in `origin.kind`, and its text opens with a
/// `<task-notification>` block. A message that only mentions the tag later in its text is the
/// person's.
fn is_task_notification(entry: &Value) -> bool {
    let text = match &entry["message"]["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    entry["origin"]["kind"] == "task-notification" || text.trim_start().starts_with("<task-notification>")
}

/// The prompt a user entry carries, if it is one the person wrote.
fn prompt_text(entry: &Value) -> Option<String> {
    if entry["isMeta"] == true || between_turns(&entry["message"]) || is_task_notification(entry) {
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

/// The message a fork of conversation `id` through its first `turns` turns stops at: the uuid of
/// the last message before turn `turns + 1` starts. A turn is counted as `history` counts it, by
/// the prompts the person wrote. None when the fork holds the whole conversation, because there is
/// no turn after the `turns`th. An error when the conversation has fewer turns, or none.
pub fn fork_point(home: &Path, cwd: &Path, id: &str, turns: usize) -> Result<Option<String>, String> {
    if turns == 0 {
        return Err("a fork holds at least one turn".into());
    }
    let text = std::fs::read_to_string(transcript(home, cwd, id))
        .map_err(|e| format!("the agent keeps no conversation {id} for {}: {e}", cwd.display()))?;
    let (mut seen, mut last) = (0usize, None::<String>);
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry["isSidechain"] == true {
            continue;
        }
        let kind = entry["type"].as_str();
        if kind == Some("user") && prompt_text(&entry).is_some() {
            seen += 1;
            if seen > turns {
                return Ok(last);
            }
        }
        if matches!(kind, Some("user" | "assistant")) {
            if let Some(uuid) = entry["uuid"].as_str() {
                last = Some(uuid.to_string());
            }
        }
    }
    if seen < turns {
        return Err(format!("conversation {id} has {seen} turn(s); a fork needs {turns}"));
    }
    Ok(None)
}

/// True when the store holds no transcript for conversation `id`: the file is not there. A file
/// that is there but cannot be read is not missing — that may pass.
pub fn missing(home: &Path, cwd: &Path, id: &str) -> bool {
    matches!(
        std::fs::metadata(transcript(home, cwd, id)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
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
                if is_task_notification(&entry) {
                    continue;
                } else if let Some(prompt) = prompt_text(&entry) {
                    history.turns.push((prompt, Vec::new()));
                    continue;
                } else {
                    tool_results(&entry["message"]["content"], &calls)
                }
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
pub(crate) fn iso8601(at: std::time::SystemTime) -> String {
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
    use super::{is_task_notification, prompt_text, words, Rest, Streamed};
    use serde_json::json;

    fn user(content: &str) -> serde_json::Value {
        json!({"type": "user", "message": {"role": "user", "content": content}})
    }

    fn notification(status: &str, summary: &str) -> String {
        format!("<task-notification>\n<task-id>b1</task-id>\n<status>{status}</status>\n<summary>{summary}</summary>\n</task-notification>")
    }

    #[test]
    fn a_task_notification_is_not_a_prompt() {
        let entry = user(&notification("completed", "Background command \"wait\" completed (exit code 0)"));
        assert_eq!(prompt_text(&entry), None);
        assert!(is_task_notification(&entry));
        let killed = user(&notification("killed", "Background command \"wait\" was stopped"));
        assert_eq!(prompt_text(&killed), None);
        assert!(is_task_notification(&killed));
    }

    #[test]
    fn an_entry_marked_as_a_task_notification_is_one_whatever_its_text() {
        let mut entry = user("something else");
        entry["origin"] = json!({"kind": "task-notification"});
        assert_eq!(prompt_text(&entry), None);
        assert!(is_task_notification(&entry));
    }

    #[test]
    fn a_message_that_mentions_the_tag_mid_text_is_still_the_persons() {
        let text = "why does <task-notification> show up as my message?";
        let entry = user(text);
        assert!(!is_task_notification(&entry));
        assert_eq!(prompt_text(&entry).as_deref(), Some(text));
    }

    #[test]
    fn history_leaves_a_task_notification_out_of_its_turn() {
        use super::history;
        let home = std::env::temp_dir().join(format!("wheel-j6ry-{}", std::process::id()));
        let cwd = std::path::Path::new("/work/proj");
        let dir = crate::resume::claude_project_dir(&home, &super::store_cwd(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [
            user("start the build"),
            user(&notification("completed", "Background command \"build\" completed (exit code 0)")),
        ];
        let body: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        std::fs::write(dir.join("c1.jsonl"), body.join("\n")).unwrap();
        let found = history(&home, cwd, "c1").unwrap();
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(found.turns.len(), 1);
        assert_eq!(found.turns[0].0, "start the build");
        assert!(found.turns[0].1.is_empty());
    }

    #[test]
    fn a_fork_point_is_the_last_message_before_the_next_prompt() {
        use super::fork_point;
        let home = std::env::temp_dir().join(format!("wheel-fork-{}", std::process::id()));
        let cwd = std::path::Path::new("/work/proj");
        let dir = crate::resume::claude_project_dir(&home, &super::store_cwd(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        let line = |kind: &str, uuid: &str, text: &str| match kind {
            "user" => serde_json::json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": text}}),
            _ => serde_json::json!({"type": "assistant", "uuid": uuid, "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}}),
        };
        let lines = [
            line("user", "u1", "one"),
            line("assistant", "a1", "reply one"),
            line("assistant", "a1b", "more of reply one"),
            line("user", "u2", "two"),
            line("assistant", "a2", "reply two"),
        ];
        let body: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        std::fs::write(dir.join("c1.jsonl"), body.join("\n")).unwrap();
        let first = fork_point(&home, cwd, "c1", 1);
        let all = fork_point(&home, cwd, "c1", 2);
        let past = fork_point(&home, cwd, "c1", 3);
        let none = fork_point(&home, cwd, "c1", 0);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(first, Ok(Some("a1b".to_string())));
        assert_eq!(all, Ok(None));
        assert!(past.unwrap_err().contains("has 2 turn(s)"));
        assert!(none.is_err());
    }

    #[test]
    fn a_fork_resumes_the_source_as_a_copy_stopped_at_the_message() {
        use crate::driver::{Conversation, Launch};
        let launch = |turns| Launch {
            agent: crate::resume::Harness::Claude,
            command: "claude".into(),
            cwd: "/work/proj".into(),
            config_dir: None,
            brief: None,
            env: Vec::new(),
            conversation: Conversation::Fork { from: "src".into(), turns, id: "new".into() },
            model: None,
            permissions: Default::default(),
            orchestrator: false,
        };
        let cut = super::arguments(&launch(1), Some("a1b"));
        let at = cut.iter().position(|a| a == "--resume-session-at").unwrap();
        assert_eq!(cut[at + 1], "a1b");
        let resume = cut.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(cut[resume + 1], "src");
        let named = cut.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(cut[named + 1], "new");
        assert!(cut.contains(&"--fork-session".to_string()));
        let whole = super::arguments(&launch(2), None);
        assert!(whole.contains(&"--fork-session".to_string()));
        assert!(!whole.contains(&"--resume-session-at".to_string()));
    }

    #[test]
    fn a_block_the_stream_carried_is_not_sent_again() {
        let mut s = Streamed::default();
        s.push(0, "");
        s.push(1, "Tea is ");
        s.push(1, "old.");
        assert_eq!(s.take_rest(""), Rest::Nothing);
        assert_eq!(s.take_rest("Tea is old."), Rest::Nothing);
    }

    #[test]
    fn a_whole_block_trimmed_shorter_than_the_stream_is_not_sent_again() {
        let mut s = Streamed::default();
        s.push(0, "First.\n\n");
        s.push(2, "Second.");
        assert_eq!(s.take_rest("First."), Rest::Nothing);
        s.skip();
        assert_eq!(s.take_rest("Second."), Rest::Nothing);
    }

    #[test]
    fn only_what_the_stream_missed_is_sent() {
        let mut s = Streamed::default();
        s.push(0, "Hello");
        assert_eq!(s.take_rest("Hello world"), Rest::Missed(" world"));
        assert_eq!(s.take_rest("never streamed"), Rest::Missed("never streamed"));
    }

    #[test]
    fn a_block_that_differs_replaces_what_streamed() {
        let mut s = Streamed::default();
        s.push(0, "abc def");
        assert_eq!(s.take_rest("abc xyz"), Rest::Revised { streamed: "abc def".into() });
    }

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
