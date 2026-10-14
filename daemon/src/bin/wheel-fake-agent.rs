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
//! - for a prompt containing "list tools", reports `tools: <names>`: its built-in tools less the ones
//!   the session was created without (`_meta.claudeCode.options.disallowedTools`), as the real
//!   agent's tool list would be.
//! - for a prompt containing "work slowly", works the way a real turn paces itself, so a client can
//!   show work in progress: a thought in three chunks 0.5 s apart, then three tool calls in a row —
//!   "call-3" reading `src/main.rs` (in progress, completed after 1 s), "call-4" searching (completed
//!   after 0.5 s) and "call-5" running `cargo test` (in progress for 6 s, or the number of seconds
//!   after "work slowly", then completed) — before the echo.
//!
//! With `--store <dir>` it also advertises `loadSession` and `session/list`: each session gets its
//! own id, every turn is kept in `<dir>/<id>.jsonl`, and loading a session replays each kept turn as
//! the prompt (a user message) and its echo. Loading an id with no file fails. `session/list`
//! reports one entry per kept file — the real agent's store belongs to the directory, so every
//! session it kept is listed whoever ran it — titled after its first prompt and timed by the file,
//! newest first. The `cwd` filter is reported back rather than matched: this store holds one
//! directory's sessions and nothing else. It also advertises `session/close`: a closed session takes
//! no prompt until it is loaded again, and each close appends the session's id to `<dir>/closed`,
//! one per line, so a test can read which conversations were closed.
//!
//! It offers three config options — `mode` and `model` selects and a `fast` boolean — so a test can
//! drive `session/set_config_option` without a real model. Setting one to a value it does not offer
//! fails; every set answers with the whole list, as ACP requires. With `--no-mode` it offers no
//! `mode` option, as an agent with no modes would; a prompt containing "switch mode" sends a
//! `current_mode_update` to `plan`, whether or not it offers the option.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk,
    CurrentModeUpdate, Diff, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, McpCapabilities, McpServer,
    McpServerHttp, Meta, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, ResourceLink, SessionCapabilities, SessionCloseCapabilities,
    SessionConfigBoolean, SessionConfigId, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect,
    SessionConfigSelectOption, SessionId, SessionInfo, SessionListCapabilities,
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
    let mode = current.has_mode.then(|| {
        select(
            "mode",
            "Mode",
            SessionConfigOptionCategory::Mode,
            MODES,
            &current.mode,
        )
    });
    mode.into_iter()
        .chain([
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
        ])
        .collect()
}

struct ConfigState {
    /// Whether the agent offers a `mode` option at all.
    has_mode: bool,
    mode: String,
    model: String,
    fast: bool,
}

impl ConfigState {
    fn new(has_mode: bool) -> Self {
        Self {
            has_mode,
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
type Setup = (bool, Option<McpServerHttp>, Vec<String>);

/// The built-in tools the fake agent offers a session that disallows none.
const TOOLS: [&str; 5] = ["Agent", "Bash", "Edit", "Read", "Task"];

fn session_setup(meta: &Option<Meta>, servers: &[McpServer]) -> Setup {
    let disallowed = meta
        .as_ref()
        .and_then(|m| m.get("claudeCode"))
        .and_then(|c| c.get("options"))
        .and_then(|o| o.get("disallowedTools"))
        .and_then(|d| d.as_array())
        .map(|d| {
            d.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let append = meta
        .as_ref()
        .and_then(|m| m.get("systemPrompt"))
        .and_then(|p| p.get("append"))
        .and_then(|a| a.as_str());
    let mcp = servers.iter().find_map(|s| match s {
        McpServer::Http(http) => Some(http.clone()),
        _ => None,
    });
    (append.is_some_and(|a| !a.is_empty()), mcp, disallowed)
}

/// A unix time as the ISO 8601 UTC timestamp `session/list` reports, so a client can sort the
/// entries as text. Carries microseconds, because two sessions made in the same second are the
/// ordinary case here and a client sorting on this must still put the newer one first. Civil date
/// from days, after Howard Hinnant's `civil_from_days`.
fn iso8601(at: std::time::Duration) -> String {
    let (seconds, micros) = (at.as_secs(), at.subsec_micros());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    // March-based years, so a leap day lands at the end of its own year and needs no special case.
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

/// Every session kept under `--store`, newest first: one entry per `<id>.jsonl` file, titled after
/// its first kept prompt and timed by the file's own mtime.
fn kept_sessions(dir: &PathBuf, cwd: &std::path::Path) -> Vec<SessionInfo> {
    let mut sessions: Vec<(std::time::Duration, SessionInfo)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let updated = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .unwrap_or_default();
        let title = std::fs::read_to_string(&path)
            .ok()
            .and_then(|kept| {
                let line = kept.lines().next()?.to_string();
                Some(serde_json::from_str::<Kept>(&line).ok()?.prompt)
            })
            .unwrap_or_else(|| id.to_string());
        let info = SessionInfo::new(SessionId::new(id), cwd.to_path_buf())
            .title(title)
            .updated_at(iso8601(updated));
        sessions.push((updated, info));
    }
    sessions.sort_by_key(|(updated, _)| std::cmp::Reverse(*updated));
    sessions.into_iter().map(|(_, info)| info).collect()
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
    let mut store: Option<PathBuf> = None;
    let mut has_mode = true;
    let usage =
        || Error::invalid_params().data("usage: wheel-fake-agent [--store <dir>] [--no-mode]");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(args.next().ok_or_else(usage)?.into()),
            "--no-mode" => has_mode = false,
            _ => return Err(usage()),
        }
    }
    let file = |dir: &PathBuf, id: &SessionId| dir.join(format!("{id}.jsonl"));
    // Whether the session asked for an appended system prompt, and its http MCP server.
    let setup: Arc<Mutex<Setup>> = Arc::default();
    let (on_new, on_load) = (setup.clone(), setup.clone());
    // One set of config values for the process; sessions here are never concurrent.
    let config = Arc::new(Mutex::new(ConfigState::new(has_mode)));
    let (config_new, config_load, config_set) = (config.clone(), config.clone(), config.clone());
    let (new_store, load_store, prompt_store, list_store, close_store) = (
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
    );
    // The sessions a prompt may run in: made or loaded, and not closed since.
    let open: Arc<Mutex<HashSet<SessionId>>> = Arc::default();
    let (open_new, open_load, open_close, open_prompt) =
        (open.clone(), open.clone(), open.clone(), open);
    Agent
        .builder()
        .name("wheel-fake-agent")
        .on_receive_request(
            async move |init: InitializeRequest, responder, _cx| {
                let sessions = SessionCapabilities::new()
                    .list(store.is_some().then(SessionListCapabilities::new))
                    .close(store.is_some().then(SessionCloseCapabilities::new));
                responder.respond(
                    InitializeResponse::new(init.protocol_version).agent_capabilities(
                        AgentCapabilities::new()
                            .mcp_capabilities(McpCapabilities::new().http(true))
                            .session_capabilities(sessions)
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
                let id = SessionId::new(id);
                open_new.lock().unwrap().insert(id.clone());
                responder.respond(
                    NewSessionResponse::new(id)
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
                open_load.lock().unwrap().insert(req.session_id.clone());
                responder.respond(
                    LoadSessionResponse::new()
                        .config_options(config_options(&config_load.lock().unwrap())),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ListSessionsRequest, responder, _cx| {
                let Some(dir) = &list_store else {
                    return responder.respond_with_error(Error::method_not_found());
                };
                let cwd = req.cwd.unwrap_or_else(|| PathBuf::from("/"));
                responder.respond(ListSessionsResponse::new(kept_sessions(dir, &cwd)))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: CloseSessionRequest, responder, _cx| {
                let Some(dir) = &close_store else {
                    return responder.respond_with_error(Error::method_not_found());
                };
                if !open_close.lock().unwrap().remove(&req.session_id) {
                    return responder.respond_with_error(Error::resource_not_found(Some(
                        req.session_id.to_string(),
                    )));
                }
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("closed"))
                    .and_then(|mut f| writeln!(f, "{}", req.session_id))
                    .map_err(|_| Error::internal_error())?;
                responder.respond(CloseSessionResponse::new())
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
                let (context, mcp, disallowed) = setup.lock().unwrap().clone();
                let store = prompt_store.clone();
                if store.is_some() && !open_prompt.lock().unwrap().contains(&req.session_id) {
                    return responder.respond_with_error(Error::resource_not_found(Some(
                        req.session_id.to_string(),
                    )));
                }
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
                    if text.contains("switch mode") {
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new("plan")),
                        ))?;
                    }
                    if text.contains("reflect") {
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new("pondering ")),
                            )),
                        ))?;
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new("the question")),
                            )),
                        ))?;
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::ResourceLink(ResourceLink::new(
                                    "notes.md",
                                    "file:///tmp/notes.md",
                                )),
                            )),
                        ))?;
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
                    if let Some(rest) = text.split("work slowly").nth(1) {
                        let secs: u64 = rest
                            .split_whitespace()
                            .next()
                            .and_then(|n| n.parse().ok())
                            .unwrap_or(6);
                        let send = |update| {
                            worker.send_notification(SessionNotification::new(
                                session.clone(),
                                update,
                            ))
                        };
                        let pause = |ms| tokio::time::sleep(std::time::Duration::from_millis(ms));
                        for chunk in ["Looking at ", "the tests that ", "touch the parser."] {
                            send(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(chunk)),
                            )))?;
                            pause(500).await;
                        }
                        let update = |id: &str, status| {
                            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                id.to_string(),
                                ToolCallUpdateFields::new().status(status),
                            ))
                        };
                        send(SessionUpdate::ToolCall(
                            ToolCall::new("call-3", "Read src/main.rs")
                                .kind(ToolKind::Read)
                                .status(ToolCallStatus::InProgress),
                        ))?;
                        pause(1000).await;
                        send(update("call-3", ToolCallStatus::Completed))?;
                        send(SessionUpdate::ToolCall(
                            ToolCall::new("call-4", "Search for parse_args")
                                .kind(ToolKind::Search)
                                .status(ToolCallStatus::InProgress),
                        ))?;
                        pause(500).await;
                        send(update("call-4", ToolCallStatus::Completed))?;
                        send(SessionUpdate::ToolCall(
                            ToolCall::new("call-5", "cargo test -p parser")
                                .kind(ToolKind::Execute)
                                .status(ToolCallStatus::InProgress)
                                .content(vec![ToolCallContent::from("Run the parser's tests")]),
                        ))?;
                        pause(secs * 1000).await;
                        send(update("call-5", ToolCallStatus::Completed))?;
                    }
                    if text.contains("list tools") {
                        let tools: Vec<&str> = TOOLS
                            .into_iter()
                            .filter(|t| !disallowed.iter().any(|d| d == t))
                            .collect();
                        say(&worker, &session, &format!("tools: {}\n", tools.join(" ")))?;
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
