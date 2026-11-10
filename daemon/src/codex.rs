//! The agent interface ([`crate::driver`]) over Codex: one `codex app-server` process per
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
    ConfigChoice, ConfigKind, ConfigOption, ContextUse, PermOption, PlanItem, AgentEvent,
    ToolContent,
};

use crate::driver::{
    self, DriverHandle, DriverEvent, DriverInput, Conversation, Input, Launch, Process, ProcessGroup,
};
use crate::claude::History;

/// The approval policies the `mode` control offers, in order, with their names. Codex asks for
/// most commands under the first.
const POLICIES: [(&str, &str); 3] = [
    ("untrusted", "Ask for most commands"),
    ("on-request", "Ask when it needs more access"),
    ("never", "Never ask"),
];

/// The policy a Codex agent starts in when none was chosen: the one Codex itself defaults to.
const DEFAULT_POLICY: &str = "on-request";

/// The sandboxes the `sandbox` control offers, in order: the value `thread/start` takes, its name,
/// and the `type` of the `sandboxPolicy` a turn carries (which a thread's answer names it by).
const SANDBOXES: [(&str, &str, &str); 3] = [
    ("read-only", "Read-only", "readOnly"),
    ("workspace-write", "Edit the workspace", "workspaceWrite"),
    ("danger-full-access", "Full access", "dangerFullAccess"),
];

/// The sandbox a Codex agent starts in when none was chosen: the one Codex itself defaults to.
const DEFAULT_SANDBOX: &str = "workspace-write";

/// One model `model/list` names.
struct ModelInfo {
    id: String,
    name: String,
    description: String,
    hidden: bool,
    /// The reasoning efforts the model takes, each with what it means, and the one it starts on.
    efforts: Vec<(String, String)>,
    default_effort: String,
}

impl ModelInfo {
    fn parse(m: &Value) -> Option<Self> {
        let id = m["model"].as_str().or_else(|| m["id"].as_str())?.to_string();
        Some(Self {
            name: m["displayName"].as_str().unwrap_or(&id).to_string(),
            description: m["description"].as_str().unwrap_or_default().to_string(),
            hidden: m["hidden"].as_bool().unwrap_or(false),
            efforts: m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    Some((
                        e["reasoningEffort"].as_str()?.to_string(),
                        e["description"].as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect(),
            default_effort: m["defaultReasoningEffort"].as_str().unwrap_or_default().to_string(),
            id,
        })
    }
}

/// A `select` control.
fn select(
    id: &str,
    name: &str,
    category: Option<&str>,
    current: &str,
    choices: Vec<ConfigChoice>,
) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category: category.map(Into::into),
        kind: ConfigKind::Select {
            current: current.into(),
            choices,
        },
    }
}

fn choice(value: &str, name: &str, description: Option<&str>) -> ConfigChoice {
    ConfigChoice {
        value: value.into(),
        name: name.into(),
        description: description.filter(|d| !d.is_empty()).map(Into::into),
    }
}

/// The names of the approval policies, for refusing one Codex does not have.
pub(crate) fn policy_names() -> Vec<&'static str> {
    POLICIES.iter().map(|(v, _)| *v).collect()
}

/// The `mode` control with `current` chosen.
fn mode_option(current: &str) -> ConfigOption {
    select(
        "mode",
        "Approvals",
        Some("mode"),
        current,
        POLICIES.iter().map(|(v, n)| choice(v, n, None)).collect(),
    )
}

/// The `sandbox` control with `current` chosen.
fn sandbox_option(current: &str) -> ConfigOption {
    select(
        "sandbox",
        "Sandbox",
        Some("sandbox"),
        current,
        SANDBOXES.iter().map(|(v, n, _)| choice(v, n, None)).collect(),
    )
}

/// Start `codex app-server` for `launch`.
pub fn launch(launch: Launch) -> Result<DriverHandle, String> {
    let Process {
        pid,
        child,
        stdin,
        stdout,
        last_error,
    } = driver::spawn(&launch, vec!["app-server".into()], "CODEX_HOME")?;
    let (input, inputs) = mpsc::unbounded_channel();
    let (events, events_rx) = mpsc::unbounded_channel();
    let policy = launch
        .permissions
        .permission_mode
        .clone()
        .unwrap_or_else(|| DEFAULT_POLICY.into());
    let sandbox = launch
        .permissions
        .sandbox
        .clone()
        .unwrap_or_else(|| DEFAULT_SANDBOX.into());
    let driver = Driver {
        pid,
        stdin,
        events,
        launch,
        models: Vec::new(),
        model: String::new(),
        effort: None,
        sandbox,
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
        steered: HashMap::new(),
    };
    tokio::spawn(driver.run(stdout, inputs, child, last_error));
    Ok(DriverHandle {
        input: DriverInput(input),
        events: events_rx,
        steers: true,
        withdraws: false,
        _group: ProcessGroup(pid),
    })
}

/// A request this side sent Codex, waiting for its answer.
enum Pending {
    Initialize,
    /// `model/list`, asked before the thread opens.
    Models,
    /// `thread/start` or `thread/resume`.
    Thread,
    TurnStart,
    Interrupt,
    /// `turn/steer` for the message of `turn`, which comes back as the `userMessage` item `id`.
    Steer { turn: u64, id: String },
}

/// A question Codex asked and a person has not answered.
struct Approval {
    /// Codex's id for the request, which the answer repeats.
    id: Value,
    /// Whether the answer is `acceptForSession` for "allow always".
    agent_scope: bool,
    /// Set when the request is a `request_user_input` call: the questions are put one at a time.
    ask: Option<Ask>,
}

/// The questions of one `item/tool/requestUserInput` request, asked one after another.
struct Ask {
    questions: Vec<Value>,
    /// Each answered question's id with `{"answers": [<label>]}`.
    answers: serde_json::Map<String, Value>,
    /// The index of the question now waiting.
    at: usize,
    /// Codex's id for the tool call, which every question of the request repeats.
    tool_call: Option<String>,
}

struct Driver {
    pid: u32,
    stdin: tokio::process::ChildStdin,
    events: mpsc::UnboundedSender<DriverEvent>,
    launch: Launch,
    /// The models `model/list` has named.
    models: Vec<ModelInfo>,
    /// The model each turn runs, from the thread's answer until a person picks another.
    model: String,
    /// The reasoning effort each turn runs at; None leaves the model's own.
    effort: Option<String>,
    /// The sandbox each turn runs in, by the name `thread/start` takes.
    sandbox: String,
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
    /// Messages sent with `turn/steer` that Codex has not taken in, by the id they were sent
    /// under, with their turn.
    steered: HashMap<String, u64>,
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

    fn transcript(&self, event: AgentEvent) {
        self.emit(DriverEvent::Transcript(event));
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
                let mut params = json!({
                    "threadId": thread,
                    "input": [{"type": "text", "text": text, "text_elements": []}],
                    "approvalPolicy": self.policy,
                });
                if !self.model.is_empty() {
                    params["model"] = json!(self.model);
                }
                if let Some(effort) = &self.effort {
                    params["effort"] = json!(effort);
                }
                if let Some((_, _, kind)) = SANDBOXES.iter().find(|(v, _, _)| *v == self.sandbox) {
                    params["sandboxPolicy"] = json!({"type": kind});
                }
                self.request(Pending::TurnStart, "turn/start", params).await
            }
            Input::Steer { turn, text } => {
                let (Some(thread), Some(running), true) =
                    (self.thread.clone(), self.turn.clone(), self.in_turn)
                else {
                    self.emit(DriverEvent::SteerRefused { turn });
                    return Ok(());
                };
                // The id comes back on the `userMessage` item when Codex takes the message in.
                let id = format!("wheel-steer-{turn}");
                self.steered.insert(id.clone(), turn);
                self.request(
                    Pending::Steer { turn, id: id.clone() },
                    "turn/steer",
                    json!({
                        "threadId": thread,
                        "expectedTurnId": running,
                        "clientUserMessageId": id,
                        "input": [{"type": "text", "text": text, "text_elements": []}],
                    }),
                )
                .await
            }
            // Codex has no way to take a steered message back (`withdraws` is false), and a
            // steered message is already one it holds: nothing to do.
            Input::Withdraw { .. } | Input::SendNow { .. } => Ok(()),
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
                if let Some(ask) = approval.ask {
                    return self.answer_question(approval.id, ask, choice.as_deref()).await;
                }
                let decision = match choice.as_deref() {
                    Some("allow") => "accept",
                    Some("allow_always") if approval.agent_scope => "acceptForSession",
                    Some("allow_always") => "accept",
                    Some(_) => "decline",
                    None => "cancel",
                };
                self.write(json!({"id": approval.id, "result": {"decision": decision}}))
                    .await
            }
            Input::SetOption { id, value, reply } => {
                let answer = self.set_option(&id, &value);
                tracing::info!(pid = self.pid, option = %id, %value, ok = answer.is_ok(), "agent option set");
                let _ = reply.send(answer);
                Ok(())
            }
        }
    }

    /// Change the control `id` to `value`, answering with the controls as they stand. Each applies
    /// from the next turn: they travel with every `turn/start`.
    fn set_option(&mut self, id: &str, value: &str) -> Result<Vec<ConfigOption>, String> {
        let refuse = |valid: Vec<&str>| format!("{id} does not take {value}; it takes {valid:?}");
        match id {
            "mode" => {
                if !POLICIES.iter().any(|(v, _)| *v == value) {
                    return Err(refuse(policy_names()));
                }
                self.policy = value.into();
            }
            "sandbox" => {
                if !SANDBOXES.iter().any(|(v, _, _)| *v == value) {
                    return Err(refuse(SANDBOXES.iter().map(|(v, _, _)| *v).collect()));
                }
                self.sandbox = value.into();
            }
            "model" => {
                let Some(model) = self.models.iter().find(|m| m.id == value) else {
                    return Err(refuse(self.models.iter().filter(|m| !m.hidden).map(|m| m.id.as_str()).collect()));
                };
                // An effort the new model lacks falls back to the model's own.
                if !self.effort.as_ref().is_some_and(|e| model.efforts.iter().any(|(v, _)| v == e)) {
                    self.effort = Some(model.default_effort.clone()).filter(|e| !e.is_empty());
                }
                self.model = value.into();
            }
            "effort" => {
                let efforts = self.current_model().map(|m| &m.efforts[..]).unwrap_or_default();
                if !efforts.iter().any(|(v, _)| v == value) {
                    return Err(refuse(efforts.iter().map(|(v, _)| v.as_str()).collect()));
                }
                self.effort = Some(value.into());
            }
            _ => return Err(format!("the agent has no option {id}")),
        }
        Ok(self.options())
    }

    fn current_model(&self) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == self.model)
    }

    /// The controls: the model, how hard it reasons, what it may do without asking, and the sandbox
    /// it runs commands in.
    fn options(&self) -> Vec<ConfigOption> {
        let mut options = Vec::new();
        if !self.model.is_empty() {
            let mut choices: Vec<ConfigChoice> = self
                .models
                .iter()
                .filter(|m| !m.hidden || m.id == self.model)
                .map(|m| choice(&m.id, &m.name, Some(&m.description)))
                .collect();
            if !choices.iter().any(|c| c.value == self.model) {
                choices.insert(0, choice(&self.model, &self.model, None));
            }
            options.push(select("model", "Model", Some("model"), &self.model, choices));
        }
        if let (Some(model), Some(effort)) = (self.current_model(), &self.effort) {
            if !model.efforts.is_empty() {
                options.push(select(
                    "effort",
                    "Effort",
                    Some("thought_level"),
                    effort,
                    model.efforts.iter().map(|(v, d)| choice(v, v, Some(d))).collect(),
                ));
            }
        }
        options.push(mode_option(&self.policy));
        options.push(sandbox_option(&self.sandbox));
        options
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
                Pending::Initialize | Pending::Models | Pending::Thread => Err(said.to_string()),
                Pending::TurnStart => {
                    self.turn_failed(said.to_string());
                    Ok(())
                }
                Pending::Interrupt => {
                    tracing::warn!(pid = self.pid, error = %said, "agent interrupt refused");
                    Ok(())
                }
                Pending::Steer { turn, id } => {
                    tracing::warn!(pid = self.pid, turn, error = %said, "agent steer refused");
                    self.steered.remove(&id);
                    self.emit(DriverEvent::SteerRefused { turn });
                    Ok(())
                }
            };
        }
        let result = &message["result"];
        match pending {
            Pending::Initialize => {
                tracing::info!(pid = self.pid, agent = %result["userAgent"], home = %result["codexHome"], "agent initialized");
                self.write(json!({"method": "initialized"})).await?;
                // `thread/start` takes any model name and fails only at the first turn, so a model
                // the host chose is checked against the list, which also fills the model control.
                self.request(
                    Pending::Models,
                    "model/list",
                    json!({"includeHidden": true}),
                )
                .await
            }
            Pending::Models => {
                self.models.extend(
                    result["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(ModelInfo::parse),
                );
                // The list comes in pages.
                if let Some(cursor) = result["nextCursor"].as_str() {
                    let params = json!({"includeHidden": true, "cursor": cursor});
                    return self.request(Pending::Models, "model/list", params).await;
                }
                if let Some(model) = &self.launch.model {
                    if !self.models.iter().any(|m| m.id == *model) {
                        let offered: Vec<&str> = self.models.iter().map(|m| m.id.as_str()).collect();
                        return Err(format!(
                            "the agent does not know the model {model}; it offers {offered:?}"
                        ));
                    }
                }
                self.open_thread().await
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
                self.model = result["model"].as_str().unwrap_or_default().to_string();
                self.effort = result["reasoningEffort"].as_str().map(String::from);
                // A resumed thread keeps the sandbox it ran in unless the host chose one.
                if self.launch.permissions.sandbox.is_none() {
                    if let Some(name) = SANDBOXES
                        .iter()
                        .find(|(_, _, kind)| result["sandbox"]["type"] == *kind)
                        .map(|(value, _, _)| *value)
                    {
                        self.sandbox = name.to_string();
                    }
                }
                tracing::info!(pid = self.pid, thread = %id, model = %self.model, effort = ?self.effort, sandbox = %self.sandbox, resumed = history.is_some(), turns = history.as_ref().map_or(0, |h| h.turns.len()), "agent ready");
                self.emit(DriverEvent::Ready {
                    options: self.options(),
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
            Pending::Interrupt | Pending::Steer { .. } => Ok(()),
        }
    }

    /// Open the thread: `thread/start`, or `thread/resume` of the conversation the host names.
    async fn open_thread(&mut self) -> Result<(), String> {
        let mut params = json!({
            "cwd": self.launch.cwd.display().to_string(),
            "approvalPolicy": self.policy,
        });
        if let Some(brief) = &self.launch.brief {
            params["developerInstructions"] = json!(brief);
        }
        if let Some(sandbox) = &self.launch.permissions.sandbox {
            params["sandbox"] = json!(sandbox);
        }
        if let Some(model) = &self.launch.model {
            params["model"] = json!(model);
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
                    self.transcript(AgentEvent::Text { text });
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                if let (Some(id), Some(delta)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                {
                    self.streamed.insert(id.to_string());
                    self.transcript(AgentEvent::Thought { text: delta.into() });
                }
            }
            "item/reasoning/summaryPartAdded" => {
                if params["summaryIndex"].as_u64().unwrap_or(0) > 0 {
                    self.transcript(AgentEvent::Thought {
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
                self.transcript(AgentEvent::Plan { entries });
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
            Some("userMessage") => {
                // A steered message comes back under the id it was sent with.
                let turn = item["clientId"].as_str().and_then(|id| self.steered.remove(id));
                if let Some(turn) = turn {
                    self.emit(DriverEvent::Steered { turn });
                }
            }
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
                    self.transcript(AgentEvent::Text { text });
                }
            }
            Some("reasoning") if !streamed => {
                if let Some(text) = reasoning_text(item) {
                    self.transcript(AgentEvent::Thought { text });
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
            self.emit(DriverEvent::Context(Some(context)));
        }
        if std::mem::take(&mut self.interrupted) || turn["status"] == "interrupted" {
            self.emit(DriverEvent::TurnEnd {
                stop_reason: "cancelled".into(),
            });
            return;
        }
        if turn["status"] == "failed" {
            let said = turn["error"]["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or("the turn failed");
            self.emit(DriverEvent::TurnFailed {
                message: said.to_string(),
            });
            return;
        }
        self.emit(DriverEvent::TurnEnd {
            stop_reason: "end_turn".into(),
        });
    }

    /// A turn that never started: Codex refused the prompt.
    fn turn_failed(&mut self, message: String) {
        self.in_turn = false;
        self.interrupted = false;
        self.emit(DriverEvent::TurnFailed { message });
    }

    /// Put question `ask.at` to the person under the request id `<Codex's id>#<index>`.
    fn put_question(&mut self, id: Value, ask: Ask) {
        let question = &ask.questions[ask.at];
        let choices = question["options"].as_array().into_iter().flatten().map(|o| {
            (o["label"].as_str().unwrap_or_default().to_string(), o["description"].as_str().map(String::from))
        });
        let request = format!("{id}#{}", ask.at);
        tracing::info!(pid = self.pid, %request, question = %question["question"], "agent asks a question");
        self.emit(DriverEvent::Permission {
            request: request.clone(),
            title: question["question"].as_str().unwrap_or_default().to_string(),
            tool_call: ask.tool_call.clone(),
            options: driver::question_options(choices),
            why: wheel_proto::PermWhy {
                reason: question["header"].as_str().filter(|h| !h.is_empty()).map(String::from),
                ..Default::default()
            },
        });
        self.approvals.insert(request, Approval { id, agent_scope: false, ask: Some(ask) });
    }

    /// Take the person's choice for the waiting question: a pick records it and asks the next one,
    /// or answers Codex's request after the last. Skip, or a withdrawn request, refuses the whole request.
    async fn answer_question(&mut self, id: Value, mut ask: Ask, choice: Option<&str>) -> Result<(), String> {
        let question = &ask.questions[ask.at];
        let label = driver::picked(choice)
            .and_then(|i| question["options"][i]["label"].as_str())
            .map(String::from);
        if let Some(label) = label {
            let key = question["id"].as_str().unwrap_or_default().to_string();
            ask.answers.insert(key, json!({"answers": [label]}));
            ask.at += 1;
            if ask.at < ask.questions.len() {
                self.put_question(id, ask);
                return Ok(());
            }
            return self.write(json!({"id": id, "result": {"answers": ask.answers}})).await;
        }
        let message = if choice.is_some() { "The person skipped the question." } else { "The request was withdrawn." };
        self.write(json!({"id": id, "error": {"code": -32000, "message": message}})).await
    }

    async fn server_request(
        &mut self,
        method: &str,
        id: Value,
        params: &Value,
    ) -> Result<(), String> {
        if method == "item/tool/requestUserInput" {
            let questions: Vec<Value> = params["questions"].as_array().cloned().unwrap_or_default();
            // A question with no options wants typed text, which a card has nowhere to take.
            let choosable = !questions.is_empty()
                && questions.iter().all(|q| q["options"].as_array().is_some_and(|o| !o.is_empty()));
            if !choosable {
                tracing::info!(pid = self.pid, "agent question refused: it takes typed text");
                return self
                    .write(json!({"id": id, "error": {"code": -32601, "message": "not supported by this host"}}))
                    .await;
            }
            let ask = Ask { questions, answers: serde_json::Map::new(), at: 0, tool_call: params["itemId"].as_str().map(String::from) };
            self.put_question(id, ask);
            return Ok(());
        }
        let (title, tool_call, agent_scope) = match method {
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
            .insert(request.clone(), Approval { id, agent_scope, ask: None });
        tracing::info!(pid = self.pid, %request, %method, %title, "agent asks permission");
        let reason = params["reason"]
            .as_str()
            .filter(|r| !r.is_empty() && *r != title)
            .map(String::from);
        self.emit(DriverEvent::Permission {
            request,
            title,
            tool_call,
            options: perm_options(agent_scope),
            why: wheel_proto::PermWhy {
                reason,
                ..Default::default()
            },
        });
        Ok(())
    }
}

/// The options a permission request offers; `wheel perm answer <id> allow|deny` picks them by kind.
fn perm_options(agent: bool) -> Vec<PermOption> {
    let option = |id: &str, name: &str, kind: &str| PermOption {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        description: None,
    };
    let mut options = vec![option("allow", "Allow", "allow_once")];
    if agent {
        options.push(option(
            "allow_always",
            "Allow for this agent",
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
fn tool_call(item: &Value) -> Option<AgentEvent> {
    let id = item["id"].as_str()?.to_string();
    let (title, kind, content) = match item["type"].as_str()? {
        "commandExecution" => (command_title(item), "execute", Vec::new()),
        "fileChange" => {
            let paths = change_paths(item);
            // A change set that only removes files is a delete, not an edit.
            let deletes = item["changes"].as_array().is_some_and(|c| {
                !c.is_empty() && c.iter().all(|c| c["kind"]["type"].as_str() == Some("delete"))
            });
            let verb = if deletes { "Delete" } else { "Edit" };
            let title = if paths.is_empty() {
                format!("{verb} files")
            } else {
                format!("{verb} {}", paths.join(", "))
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
            (title, if deletes { "delete" } else { "edit" }, content)
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
    Some(AgentEvent::ToolCall {
        id,
        title,
        tool_kind: kind.into(),
        status: "pending".into(),
        content,
        detail: None,
    })
}

/// The update that finishes a tool call's item. A file change keeps the diff its call showed.
fn tool_update(item: &Value) -> Option<AgentEvent> {
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
    Some(AgentEvent::ToolUpdate {
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
                            .any(|e| matches!(e, AgentEvent::Text { .. }))
                        {
                            format!("\n\n{text}")
                        } else {
                            text.to_string()
                        };
                        events.push(AgentEvent::Text { text });
                    }
                }
                Some("reasoning") => {
                    if let Some(text) = reasoning_text(item) {
                        events.push(AgentEvent::Thought { text });
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
    fn a_file_change_that_only_deletes_is_a_delete_call() {
        let change = |kind: &str| json!({"path": "src/old.rs", "kind": {"type": kind}, "diff": ""});
        let call = |changes: Vec<Value>| tool_call(&json!({"type": "fileChange", "id": "f", "status": "inProgress", "changes": changes}));
        assert!(matches!(call(vec![change("delete")]), Some(AgentEvent::ToolCall { title, tool_kind, .. }) if title == "Delete src/old.rs" && tool_kind == "delete"));
        assert!(matches!(call(vec![change("delete"), change("update")]), Some(AgentEvent::ToolCall { tool_kind, .. }) if tool_kind == "edit"));
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
        assert!(matches!(&events[2], AgentEvent::Text { text } if text == "done"));
    }
}
