//! A scripted ACP agent for tests, so they never call a real model. Over stdio it:
//! - answers every prompt with `echo: <prompt>`;
//! - first says `context: yes` when the session was created with `_meta.systemPrompt.append`;
//! - for a prompt containing "permission", announces a tool call, asks permission for it, and
//!   reports `permission: <chosen option id>` (or `permission: cancelled`) before the echo;
//! - for a prompt containing "diff", announces tool call "call-2" editing `src/a.rs` (a text block
//!   and a diff `one\n` -> `two\n`), then completes it with an update whose content is a diff
//!   `one\n` -> `three\n` and a terminal `term-1`;
//! - for a prompt containing "space", calls `create_space` for `/tmp` on the session's http MCP
//!   server and reports `mcp: <the tool's text>` (or `mcp error: ...`) before the echo.
//!
//! With `--store <dir>` it also advertises `loadSession`: each session gets its own id, every turn
//! is kept in `<dir>/<id>.jsonl`, and loading a session replays each kept turn as the prompt (a user
//! message) and its echo. Loading an id with no file fails.
//!
//! It offers three config options — `mode` and `model` selects and a `fast` boolean — so a test can
//! drive `session/set_config_option` without a real model. Setting one to a value it does not offer
//! fails; every set answers with the whole list, as ACP requires.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, Diff, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, McpCapabilities, McpServer, McpServerHttp, Meta,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, SessionConfigBoolean,
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigSelect, SessionConfigSelectOption, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, Terminal, TextContent, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error, Result, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn say(cx: &ConnectionTo<Client>, session: &SessionId, text: &str) -> Result<()> {
    cx.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

/// The values the `mode` select takes; the first is where a session starts.
const MODES: [&str; 3] = ["auto", "default", "plan"];
/// The values the `model` select takes; the first is where a session starts.
const MODELS: [&str; 3] = ["default", "sonnet", "haiku"];

/// One select option, each choice named as its value in capitals.
fn select(
    id: &str,
    name: &str,
    category: SessionConfigOptionCategory,
    values: [&'static str; 3],
    current: &str,
) -> SessionConfigOption {
    let choices: Vec<SessionConfigSelectOption> = values
        .iter()
        .map(|value| SessionConfigSelectOption::new(*value, value.to_uppercase()))
        .collect();
    SessionConfigOption::new(
        SessionConfigId::new(id),
        name.to_string(),
        SessionConfigKind::Select(SessionConfigSelect::new(current.to_string(), choices)),
    )
    .category(category)
}

/// The agent's config options at their current values. `fast` is the boolean one.
fn config_options(current: &ConfigState) -> Vec<SessionConfigOption> {
    vec![
        select(
            "mode",
            "Mode",
            SessionConfigOptionCategory::Mode,
            MODES,
            &current.mode,
        ),
        select(
            "model",
            "Model",
            SessionConfigOptionCategory::Model,
            MODELS,
            &current.model,
        ),
        SessionConfigOption::new(
            SessionConfigId::new("fast"),
            "Fast mode".to_string(),
            SessionConfigKind::Boolean(SessionConfigBoolean::new(current.fast)),
        ),
    ]
}

struct ConfigState {
    mode: String,
    model: String,
    fast: bool,
}

impl ConfigState {
    fn new() -> Self {
        Self {
            mode: MODES[0].to_string(),
            model: MODELS[0].to_string(),
            fast: false,
        }
    }

    /// Take a value for one option, or say why it cannot.
    fn set(&mut self, id: &str, value: &SessionConfigOptionValue) -> Result<()> {
        let (chosen, values) = match id {
            "mode" => (&mut self.mode, MODES),
            "model" => (&mut self.model, MODELS),
            "fast" => {
                let SessionConfigOptionValue::Boolean { value } = value else {
                    return Err(Error::invalid_params().data("fast takes true or false"));
                };
                self.fast = *value;
                return Ok(());
            }
            _ => return Err(Error::invalid_params().data(format!("no config option {id}"))),
        };
        let SessionConfigOptionValue::ValueId { value } = value else {
            return Err(Error::invalid_params().data(format!("{id} takes one of its values")));
        };
        let value = value.to_string();
        if !values.contains(&value.as_str()) {
            return Err(Error::invalid_params()
                .data(format!("{id} does not take {value}; it takes {values:?}")));
        }
        *chosen = value;
        Ok(())
    }
}

/// Whether a new or loaded session asked for an appended system prompt, and its http MCP server.
fn session_setup(meta: &Option<Meta>, servers: &[McpServer]) -> (bool, Option<McpServerHttp>) {
    let append = meta
        .as_ref()
        .and_then(|m| m.get("systemPrompt"))
        .and_then(|p| p.get("append"))
        .and_then(|a| a.as_str());
    let mcp = servers.iter().find_map(|s| match s {
        McpServer::Http(http) => Some(http.clone()),
        _ => None,
    });
    (append.is_some_and(|a| !a.is_empty()), mcp)
}

/// A kept turn, one JSON line of a session's file under `--store`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    prompt: String,
    reply: String,
}

/// One MCP `tools/call` as a bare HTTP/1.1 POST; the daemon answers a stateless request with
/// one JSON body. Returns the tool's text, or why it failed.
async fn call_tool(
    server: &McpServerHttp,
    tool: &str,
    args: serde_json::Value,
) -> std::result::Result<String, String> {
    let rest = server
        .url
        .strip_prefix("http://")
        .ok_or("not an http url")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for header in &server.headers {
        request.push_str(&format!("{}: {}\r\n", header.name, header.value));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let mut stream = TcpStream::connect(authority)
        .await
        .map_err(|e| e.to_string())?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .map_err(|e| e.to_string())?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or("no http body")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(status.to_string());
    }
    let reply: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        Err(text.to_string())
    } else {
        Ok(text.to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let store: Option<PathBuf> = match (args.next().as_deref(), args.next()) {
        (Some("--store"), Some(dir)) => Some(dir.into()),
        (None, _) => None,
        _ => return Err(Error::invalid_params().data("usage: wheel-fake-agent [--store <dir>]")),
    };
    let file = |dir: &PathBuf, id: &SessionId| dir.join(format!("{id}.jsonl"));
    // Whether the session asked for an appended system prompt, and its http MCP server.
    let setup: Arc<Mutex<(bool, Option<McpServerHttp>)>> = Arc::default();
    let (on_new, on_load) = (setup.clone(), setup.clone());
    // One set of config values for the process; sessions here are never concurrent.
    let config = Arc::new(Mutex::new(ConfigState::new()));
    let (config_new, config_load, config_set) = (config.clone(), config.clone(), config.clone());
    let (new_store, load_store, prompt_store) = (store.clone(), store.clone(), store.clone());
    Agent
        .builder()
        .name("wheel-fake-agent")
        .on_receive_request(
            async move |init: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(init.protocol_version).agent_capabilities(
                        AgentCapabilities::new()
                            .mcp_capabilities(McpCapabilities::new().http(true))
                            .load_session(store.is_some()),
                    ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, _cx| {
                *on_new.lock().unwrap() = session_setup(&req.meta, &req.mcp_servers);
                let id = match &new_store {
                    // Unique across processes, so a restarted agent never reuses a kept session.
                    Some(_) => format!(
                        "fake-{}-{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ),
                    None => "fake-session".into(),
                };
                responder.respond(
                    NewSessionResponse::new(SessionId::new(id))
                        .config_options(config_options(&config_new.lock().unwrap())),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: LoadSessionRequest, responder, cx: ConnectionTo<Client>| {
                let Some(dir) = &load_store else {
                    return responder.respond_with_error(Error::method_not_found());
                };
                let Ok(kept) = std::fs::read_to_string(file(dir, &req.session_id)) else {
                    return responder.respond_with_error(Error::resource_not_found(Some(
                        req.session_id.to_string(),
                    )));
                };
                *on_load.lock().unwrap() = session_setup(&req.meta, &req.mcp_servers);
                for line in kept.lines() {
                    let kept: Kept =
                        serde_json::from_str(line).map_err(|_| Error::internal_error())?;
                    cx.send_notification(SessionNotification::new(
                        req.session_id.clone(),
                        SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new(kept.prompt),
                        ))),
                    ))?;
                    say(&cx, &req.session_id, &kept.reply)?;
                }
                responder.respond(
                    LoadSessionResponse::new()
                        .config_options(config_options(&config_load.lock().unwrap())),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: SetSessionConfigOptionRequest, responder, _cx| {
                let mut current = config_set.lock().unwrap();
                if let Err(e) = current.set(&req.config_id.to_string(), &req.value) {
                    return responder.respond_with_error(e);
                }
                // ACP: a set answers with the whole list, not just the option that moved.
                responder.respond(SetSessionConfigOptionResponse::new(config_options(
                    &current,
                )))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx: ConnectionTo<Client>| {
                let (context, mcp) = setup.lock().unwrap().clone();
                let store = prompt_store.clone();
                let worker = cx.clone();
                cx.spawn(async move {
                    let text: String = req
                        .prompt
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text(t) = b {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    let session = req.session_id.clone();
                    if text.contains("die now") {
                        // Simulate the agent process crashing mid-turn.
                        std::process::exit(3);
                    }
                    if context {
                        say(&worker, &session, "context: yes\n")?;
                    }
                    if text.contains("permission") {
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCall(ToolCall::new("call-1", "rm -rf build")),
                        ))?;
                        let ask = RequestPermissionRequest::new(
                            session.clone(),
                            ToolCallUpdate::new(
                                "call-1",
                                ToolCallUpdateFields::new().title("rm -rf build"),
                            ),
                            vec![
                                PermissionOption::new(
                                    "allow-once",
                                    "Allow",
                                    PermissionOptionKind::AllowOnce,
                                ),
                                PermissionOption::new(
                                    "reject-once",
                                    "Deny",
                                    PermissionOptionKind::RejectOnce,
                                ),
                            ],
                        );
                        let outcome = worker.send_request(ask).block_task().await?.outcome;
                        let chosen = match outcome {
                            RequestPermissionOutcome::Selected(s) => s.option_id.to_string(),
                            _ => "cancelled".to_string(),
                        };
                        say(&worker, &session, &format!("permission: {chosen}\n"))?;
                    }
                    if text.contains("diff") {
                        let diff = |new: &str| {
                            ToolCallContent::Diff(
                                Diff::new("src/a.rs", new).old_text("one\n".to_string()),
                            )
                        };
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCall(
                                ToolCall::new("call-2", "Edit src/a.rs")
                                    .kind(ToolKind::Edit)
                                    .content(vec![
                                        ToolCallContent::from("editing a.rs"),
                                        diff("two\n"),
                                    ]),
                            ),
                        ))?;
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                "call-2",
                                ToolCallUpdateFields::new()
                                    .status(ToolCallStatus::Completed)
                                    .content(vec![
                                        diff("three\n"),
                                        ToolCallContent::Terminal(Terminal::new("term-1")),
                                    ]),
                            )),
                        ))?;
                    }
                    if text.contains("space") {
                        let line = match &mcp {
                            Some(server) => match call_tool(
                                server,
                                "create_space",
                                serde_json::json!({ "cwd": "/tmp" }),
                            )
                            .await
                            {
                                Ok(text) => format!("mcp: {text}\n"),
                                Err(e) => format!("mcp error: {e}\n"),
                            },
                            None => "mcp error: the session has no http MCP server\n".into(),
                        };
                        say(&worker, &session, &line)?;
                    }
                    let reply = format!("echo: {text}");
                    say(&worker, &session, &reply)?;
                    if let Some(dir) = &store {
                        let kept = serde_json::to_string(&Kept {
                            prompt: text,
                            reply,
                        })
                        .map_err(|_| Error::internal_error())?;
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(file(dir, &session))
                            .and_then(|mut f| writeln!(f, "{kept}"))
                            .map_err(|_| Error::internal_error())?;
                    }
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
