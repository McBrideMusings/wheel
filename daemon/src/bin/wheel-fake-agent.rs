//! A scripted ACP agent for tests, so they never call a real model. Over stdio it:
//! - answers every prompt with `echo: <prompt>`;
//! - for a prompt containing "show setup", reports `setup: sources=<settingSources> tools=<tools>
//!   config_dir=<CLAUDE_CONFIG_DIR>` as the session was created (`_meta.claudeCode.options`) and as
//!   this process was started, each `-` when unset.
//! - for a prompt containing "permission", announces a tool call, asks permission for it, and
//!   reports `permission: <chosen option id>` (or `permission: cancelled`) before the echo;
//! - for a prompt containing "diff", announces tool call "call-2" editing `src/a.rs` (a text block
//!   and a diff `one\n` -> `two\n`), then completes it with an update whose content is a diff
//!   `one\n` -> `three\n` and a terminal `term-1`;
//! - for a prompt containing "space", calls `create_space` for `/tmp` on the session's http MCP
//!   server and reports `mcp: <the tool's text>` (or `mcp error: ...`) before the echo.
//! - for a prompt containing "list tools", reports `tools: <names>`: the built-in tools the session was
//!   created with (`_meta.claudeCode.options.tools`), or all of them when it named none, as the real
//!   agent's tool list would be.
//! - for a prompt containing "work slowly", works the way a real turn paces itself, so a client can
//!   show work in progress: a thought in three chunks 0.5 s apart, then three tool calls in a row —
//!   "call-3" reading `src/main.rs` (in progress, completed after 1 s), "call-4" searching (completed
//!   after 0.5 s) and "call-5" running `cargo test` (in progress for 6 s, or the number of seconds
//!   after "work slowly", then completed) — before the echo.
//! - for a prompt containing "then a tool", ends the turn on a completed tool call "call-6" (reading
//!   `src/last.rs`) after the echo.
//! - for a prompt containing "Order these ready beads" (a supervisor's planning turn), replies with
//!   only a JSON array queueing the beads the prompt lists (`- <id> ...` lines) last first, with the
//!   models `sonnet`, `haiku`, `opus` in turn, and no echo.
//! - for a worker's message carrying "Your supervisor answered" (a blocked worker started again with
//!   its supervisor's answer), closes the bead in its working directory with `bd close`.
//! - for a prompt starting "Inbox item <id> from worker" (a supervisor's wake with one inbox item
//!   and the worker's last turn, which echoes the bead's title), settles the item on the session's
//!   http MCP server by a marker in the prompt: "[file]" calls `file_bead`, "[forward]" calls `forward_to_lead`, "[answer]"
//!   calls `message_worker`, "[drop]" calls `close_item`, and with none it leaves the item. It
//!   reports `settled: <the tool's text>` (or `settled error: ...`), or `left it`.
//! - for a worker's first message ("You are the worker for bead <id>: <title>"), acts on the bead
//!   in its working directory by what the title says: "close" closes it with `bd close`, "block"
//!   sets it blocked with `bd update --status blocked`, "stall once" goes silent the first time
//!   (leaving `.stalled-once` in the directory) and closes the bead when started again, "stall
//!   always" goes silent every time, and a title with none of these leaves the bead open. A title
//!   with "ask edit inside" / "ask edit outside" (a file in or outside the directory), "ask run
//!   declared" (`cargo test -p wheeld`), "ask run other" (`rm -rf build`) or "ask bd own" has the
//!   worker ask permission for that call first and report `permission: <chosen option id>`.
//!   containing "write <file>" first writes the bead's id to `<file>` and commits it, and one
//!   containing "with sibling" first waits until another worker's worktree stands beside its own,
//!   so two such workers both branch from main before either lands. Every
//!   message after the first, which a worker gets as a continue, only echoes.
//!
//! With `--store <dir>` it also advertises `loadSession` and `session/list`: each session gets its
//! own id, every turn is kept in `<dir>/<id>.jsonl`, and loading a session replays each kept turn as
//! the prompt (a user message) and its echo. Loading an id with no file fails. `session/list`
//! reports one entry per kept file — the real agent's store belongs to the directory, so every
//! session it kept is listed whoever ran it — titled after its first prompt and timed by the file,
//! newest first. The `cwd` filter is reported back rather than matched: this store holds one
//! directory's sessions and nothing else. It also advertises `session/close`: a closed session takes
//! no prompt until it is loaded again, and each close appends the session's id to `<dir>/closed`,
//! one per line, so a test can read which conversations were closed. `--list-secs <n>` holds each
//! `session/list` answer `n` seconds, so a client can be checked before a listing lands. While a
//! file named `fail-list` exists in the store, `session/list` answers with an error instead.
//!
//! With `--replay <dump.json>` every prompt is answered instead by a recorded conversation: the
//! `text`, `tool_call` and `tool_update` events of a `wheel agent read --json` dump from its first
//! turn on, each sent after the gap it had when recorded, so a client can be driven through a real
//! conversation's exact sequence. `--replay-secs <n>` stops the replay `n` seconds after that first
//! turn started.
//!
//! With `--auth` it needs a login: when the client's `initialize` says it can run a terminal login,
//! it advertises two `terminal` methods, `claude-ai-login` (arguments `--login-exit 0`) and
//! `console-login` (arguments `--login-exit 3`, environment `FAKE_LOGIN=console`), and every prompt
//! fails with `authRequired` until `$CLAUDE_CONFIG_DIR/logged-in` exists. Started with
//! `--login-exit <n>` it is the login command instead: it appends its arguments and `FAKE_LOGIN` to
//! `$CLAUDE_CONFIG_DIR/login-ran`, creates `logged-in` when `n` is 0, and exits with `n`.
//! `--auth-start` is `--auth` plus `session/new` and `session/load` failing with `authRequired`
//! until `logged-in` exists, so the agent never reaches a prompt.
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
    AgentCapabilities, AuthMethod, AuthMethodTerminal, CloseSessionRequest, CloseSessionResponse,
    ContentBlock, ContentChunk, CurrentModeUpdate, Diff, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    McpCapabilities, McpServer, McpServerHttp, Meta, NewSessionRequest, NewSessionResponse,
    PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, ResourceLink, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigBoolean, SessionConfigId, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelect, SessionConfigSelectOption, SessionId, SessionInfo,
    SessionListCapabilities, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, Terminal, TextContent, ToolCall, ToolCallContent,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error, Result, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Streams a recorded conversation into `session`: the `text`, `tool_call` and `tool_update`
/// events of a `wheel agent read --json` dump from its first turn on, each after the gap it had
/// when recorded, up to `secs` seconds after that turn started.
async fn replay(
    cx: &ConnectionTo<Client>,
    session: &SessionId,
    dump: &serde_json::Value,
    secs: f64,
) -> Result<()> {
    let events = dump["events"].as_array().cloned().unwrap_or_default();
    let start = events
        .iter()
        .find(|e| e["kind"] == "turn_start")
        .and_then(|e| e["at"].as_f64())
        .unwrap_or(0.0);
    let content = |e: &serde_json::Value| -> Option<Vec<ToolCallContent>> {
        let items = e["content"].as_array()?;
        Some(
            items
                .iter()
                .filter_map(|c| match c["type"].as_str()? {
                    "text" => Some(ToolCallContent::from(c["text"].as_str()?.to_string())),
                    "diff" => Some(ToolCallContent::Diff(
                        Diff::new(c["path"].as_str()?, c["new_text"].as_str()?.to_string())
                            .old_text(c["old_text"].as_str().map(str::to_string)),
                    )),
                    _ => None,
                })
                .collect(),
        )
    };
    let status = |e: &serde_json::Value| match e["status"].as_str() {
        Some("pending") => Some(ToolCallStatus::Pending),
        Some("in_progress") => Some(ToolCallStatus::InProgress),
        Some("completed") => Some(ToolCallStatus::Completed),
        Some("failed") => Some(ToolCallStatus::Failed),
        _ => None,
    };
    let kind = |e: &serde_json::Value| match e["tool_kind"].as_str() {
        Some("read") => Some(ToolKind::Read),
        Some("edit") => Some(ToolKind::Edit),
        Some("execute") => Some(ToolKind::Execute),
        Some("search") => Some(ToolKind::Search),
        _ => None,
    };
    let mut now = start;
    for e in events
        .iter()
        .filter(|e| e["at"].as_f64().is_some_and(|at| at >= start))
    {
        let at = e["at"].as_f64().unwrap_or(now);
        if (at - start) / 1000.0 > secs {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis((at - now).max(0.0) as u64)).await;
        now = at;
        let update = match e["kind"].as_str() {
            Some("text") => SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Text(TextContent::new(e["text"].as_str().unwrap_or_default())),
            )),
            Some("tool_call") => {
                let mut call = ToolCall::new(
                    e["id"].as_str().unwrap_or_default().to_string(),
                    e["title"].as_str().unwrap_or_default(),
                );
                if let Some(k) = kind(e) {
                    call = call.kind(k);
                }
                if let Some(s) = status(e) {
                    call = call.status(s);
                }
                if let Some(c) = content(e) {
                    call = call.content(c);
                }
                SessionUpdate::ToolCall(call)
            }
            Some("tool_update") => {
                let mut fields = ToolCallUpdateFields::new();
                if let Some(t) = e["title"].as_str() {
                    fields = fields.title(t.to_string());
                }
                if let Some(s) = status(e) {
                    fields = fields.status(s);
                }
                if let Some(c) = content(e) {
                    fields = fields.content(c);
                }
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    e["id"].as_str().unwrap_or_default().to_string(),
                    fields,
                ))
            }
            _ => continue,
        };
        cx.send_notification(SessionNotification::new(session.clone(), update))?;
    }
    Ok(())
}

fn say(cx: &ConnectionTo<Client>, session: &SessionId, text: &str) -> Result<()> {
    cx.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

/// Run `bd` in `cwd` the way a working agent would: off the agent's own thread, with a thought
/// chunk every half second while it runs, so a slow `bd` on a busy machine is work the daemon
/// hears about and not silence it counts toward a stall. A failed `bd` is said in the reply,
/// since a test reads what the worker did from the bead, and the agent stays up.
async fn run_bd(
    cx: &ConnectionTo<Client>,
    session: &SessionId,
    cwd: &std::path::Path,
    args: &[&str],
) -> Result<()> {
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let cwd = cwd.to_path_buf();
    let mut running = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bd")
            .args(&owned)
            .current_dir(&cwd)
            .output()
    });
    let out = loop {
        tokio::select! {
            done = &mut running => break done,
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                cx.send_notification(SessionNotification::new(
                    session.clone(),
                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new("running bd "),
                    ))),
                ))?;
            }
        }
    };
    let said = match out {
        Ok(Ok(out)) if out.status.success() => return Ok(()),
        Ok(Ok(out)) => format!(
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Ok(Err(e)) => e.to_string(),
        Err(e) => e.to_string(),
    };
    say(cx, session, &format!("bd {args:?} failed: {said}\n"))
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

/// What a new or loaded session was created with: its `settingSources` and `tools` (None when it
/// named none), and its http MCP server.
type Setup = (
    Option<Vec<String>>,
    Option<Vec<String>>,
    Option<McpServerHttp>,
);

/// The built-in tools the fake agent offers a session that names none.
const TOOLS: [&str; 5] = ["Agent", "Bash", "Edit", "Read", "Task"];

fn session_setup(meta: &Option<Meta>, servers: &[McpServer]) -> Setup {
    let option = |name: &str| -> Option<Vec<String>> {
        meta.as_ref()
            .and_then(|m| m.get("claudeCode"))
            .and_then(|c| c.get("options"))
            .and_then(|o| o.get(name))
            .and_then(|d| d.as_array())
            .map(|d| {
                d.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
    };
    let mcp = servers.iter().find_map(|s| match s {
        McpServer::Http(http) => Some(http.clone()),
        _ => None,
    });
    (option("settingSources"), option("tools"), mcp)
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
    let mut recorded: Option<serde_json::Value> = None;
    let mut replay_secs = f64::INFINITY;
    let mut list_secs = 0.0;
    let mut needs_login = false;
    let mut login_at_start = false;
    let mut login_exit: Option<i32> = None;
    let usage = || {
        Error::invalid_params().data(
            "usage: wheel-fake-agent [--store <dir> [--list-secs <n>]] [--no-mode] [--replay <dump.json> [--replay-secs <n>]]",
        )
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(args.next().ok_or_else(usage)?.into()),
            "--no-mode" => has_mode = false,
            "--auth" => needs_login = true,
            "--auth-start" => {
                needs_login = true;
                login_at_start = true;
            }
            "--login-exit" => {
                login_exit = Some(args.next().and_then(|n| n.parse().ok()).ok_or_else(usage)?)
            }
            "--replay" => {
                let path = args.next().ok_or_else(usage)?;
                let text = std::fs::read_to_string(&path).map_err(|_| usage())?;
                recorded = Some(serde_json::from_str(&text).map_err(|_| usage())?);
            }
            "--replay-secs" => {
                replay_secs = args.next().and_then(|n| n.parse().ok()).ok_or_else(usage)?
            }
            "--list-secs" => {
                list_secs = args
                    .next()
                    .and_then(|n| n.parse::<f64>().ok())
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or_else(usage)?
            }
            _ => return Err(usage()),
        }
    }
    if let Some(code) = login_exit {
        let dir = PathBuf::from(std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| ".".into()));
        let ran = format!(
            "args={} fake_login={}\n",
            std::env::args().skip(1).collect::<Vec<_>>().join(" "),
            std::env::var("FAKE_LOGIN").unwrap_or_else(|_| "-".into())
        );
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("login-ran"))
            .map_err(|_| Error::internal_error())?;
        let _ = log.write_all(ran.as_bytes());
        if code == 0 {
            let _ = std::fs::write(dir.join("logged-in"), "");
        }
        std::process::exit(code);
    }
    let logged_in = || {
        let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| ".".into());
        PathBuf::from(dir).join("logged-in").exists()
    };
    let file = |dir: &PathBuf, id: &SessionId| dir.join(format!("{id}.jsonl"));
    // Whether the session asked for an appended system prompt, and its http MCP server.
    let setup: Arc<Mutex<Setup>> = Arc::default();
    let (on_new, on_load) = (setup.clone(), setup.clone());
    // The directory the session was created in, which a worker acts in.
    let session_cwd: Arc<Mutex<PathBuf>> = Arc::default();
    let cwd_new = session_cwd.clone();
    // One set of config values for the process; sessions here are never concurrent.
    let config = Arc::new(Mutex::new(ConfigState::new(has_mode)));
    let recorded = Arc::new(recorded);
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
                let methods = if needs_login && init.client_capabilities.auth.terminal {
                    vec![
                        AuthMethod::Terminal(
                            AuthMethodTerminal::new("claude-ai-login", "Claude Subscription")
                                .description("Use Claude subscription")
                                .args(vec!["--login-exit".into(), "0".into()]),
                        ),
                        AuthMethod::Terminal(
                            AuthMethodTerminal::new("console-login", "Anthropic Console")
                                .description("Use Anthropic Console (API usage billing)")
                                .args(vec!["--login-exit".into(), "3".into()])
                                .env([("FAKE_LOGIN".to_string(), "console".to_string())].into()),
                        ),
                    ]
                } else {
                    Vec::new()
                };
                responder.respond(
                    InitializeResponse::new(init.protocol_version)
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .mcp_capabilities(McpCapabilities::new().http(true))
                                .session_capabilities(sessions)
                                .load_session(store.is_some()),
                        )
                        .auth_methods(methods),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, _cx| {
                if login_at_start && !logged_in() {
                    return responder.respond_with_error(Error::auth_required());
                }
                *on_new.lock().unwrap() = session_setup(&req.meta, &req.mcp_servers);
                *cwd_new.lock().unwrap() = req.cwd.clone();
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
                if login_at_start && !logged_in() {
                    return responder.respond_with_error(Error::auth_required());
                }
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
            async move |req: ListSessionsRequest, responder, cx: ConnectionTo<Client>| {
                let Some(dir) = list_store.clone() else {
                    return responder.respond_with_error(Error::method_not_found());
                };
                let cwd = req.cwd.unwrap_or_else(|| PathBuf::from("/"));
                // Spawned, so a held answer does not hold the requests behind it.
                cx.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(list_secs)).await;
                    if dir.join("fail-list").exists() {
                        return responder.respond_with_error(Error::internal_error());
                    }
                    responder.respond(ListSessionsResponse::new(kept_sessions(&dir, &cwd)))
                })
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
                let (sources, offered, mcp) = setup.lock().unwrap().clone();
                let session_cwd = session_cwd.clone();
                let store = prompt_store.clone();
                let recorded = recorded.clone();
                if store.is_some() && !open_prompt.lock().unwrap().contains(&req.session_id) {
                    return responder.respond_with_error(Error::resource_not_found(Some(
                        req.session_id.to_string(),
                    )));
                }
                if needs_login {
                    let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| ".".into());
                    if !PathBuf::from(dir).join("logged-in").exists() {
                        return responder.respond_with_error(Error::auth_required());
                    }
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
                    if let Some(dump) = recorded.as_ref() {
                        replay(&worker, &session, dump, replay_secs).await?;
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                    if text.contains("die now") {
                        // Simulate the agent process crashing mid-turn.
                        std::process::exit(3);
                    }
                    if text.contains("Order these ready beads") {
                        // A supervisor's planning turn: the ready beads listed one per line as
                        // `- <id> (...) <title>` come back last first, the models cycling.
                        const MODELS: [&str; 3] = ["sonnet", "haiku", "opus"];
                        let queue: Vec<serde_json::Value> = text
                            .lines()
                            .filter_map(|l| l.strip_prefix("- "))
                            .filter_map(|l| l.split(' ').next())
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .enumerate()
                            .map(
                                |(i, id)| serde_json::json!({ "bead": id, "model": MODELS[i % 3] }),
                            )
                            .collect();
                        say(
                            &worker,
                            &session,
                            &serde_json::Value::Array(queue).to_string(),
                        )?;
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                    if let Some(rest) = text.strip_prefix("Inbox item ") {
                        let item = rest.split(' ').next().unwrap_or_default();
                        let call = if text.contains("[file]") {
                            Some((
                                "file_bead",
                                serde_json::json!({
                                    "item": item,
                                    "title": "filed follow-up",
                                    "body": "found while working the worker's bead",
                                    "labels": ["daemon"],
                                }),
                            ))
                        } else if text.contains("[forward]") {
                            Some((
                                "forward_to_lead",
                                serde_json::json!({ "item": item, "summary": "the worker needs a person's choice" }),
                            ))
                        } else if text.contains("[answer]") {
                            Some((
                                "message_worker",
                                serde_json::json!({ "item": item, "text": "use the blue one" }),
                            ))
                        } else if text.contains("[drop]") {
                            Some((
                                "close_item",
                                serde_json::json!({ "item": item, "note": "nothing to do" }),
                            ))
                        } else {
                            None
                        };
                        let line = match (call, &mcp) {
                            (None, _) => "left it\n".to_string(),
                            (Some(_), None) => "settled error: the session has no http MCP server\n".into(),
                            (Some((tool, args)), Some(server)) => {
                                match call_tool(server, tool, args).await {
                                    Ok(text) => format!("settled: {text}\n"),
                                    Err(e) => format!("settled error: {e}\n"),
                                }
                            }
                        };
                        say(&worker, &session, &line)?;
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                    if let Some(rest) = text.strip_prefix("You are the worker for bead ") {
                        let (bead, title) = rest
                            .lines()
                            .next()
                            .and_then(|l| l.split_once(": "))
                            .unwrap_or_default();
                        // A failed `bd` is reported in the reply, since a test reads what the worker did
                        // from the bead, and the agent stays up.
                        let cwd = session_cwd.lock().unwrap().clone();
                        let once = cwd.join(".stalled-once");
                        if title.contains("stall always")
                            || (title.contains("stall once") && !once.exists())
                        {
                            let _ = std::fs::write(once, "");
                            std::future::pending::<()>().await;
                        }
                        // "ask ..." has the worker ask permission for one tool call first, and report
                        // the option chosen.
                        const ASKS: [&str; 5] = [
                            "ask edit inside",
                            "ask edit outside",
                            "ask run declared",
                            "ask run other",
                            "ask bd own",
                        ];
                        if let Some(what) = ASKS.iter().find(|a| title.contains(**a)) {
                            let inside = cwd.join("src/a.rs");
                            let (kind, label, locations, input) = match *what {
                                "ask edit inside" => (
                                    ToolKind::Edit,
                                    "Edit src/a.rs".to_string(),
                                    vec![ToolCallLocation::new(inside.clone())],
                                    serde_json::json!({ "file_path": inside }),
                                ),
                                "ask edit outside" => (
                                    ToolKind::Edit,
                                    "Edit /tmp/outside-the-worktree.txt".to_string(),
                                    vec![ToolCallLocation::new("/tmp/outside-the-worktree.txt")],
                                    serde_json::json!({ "file_path": "/tmp/outside-the-worktree.txt" }),
                                ),
                                other => {
                                    let command = match other {
                                        "ask run declared" => "cargo test -p wheeld".to_string(),
                                        "ask bd own" => format!("bd show {bead}"),
                                        _ => "rm -rf build".to_string(),
                                    };
                                    (
                                        ToolKind::Execute,
                                        command.clone(),
                                        Vec::new(),
                                        serde_json::json!({ "command": command }),
                                    )
                                }
                            };
                            worker.send_notification(SessionNotification::new(
                                session.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new("call-w", label.clone()).kind(kind),
                                ),
                            ))?;
                            let ask = RequestPermissionRequest::new(
                                session.clone(),
                                ToolCallUpdate::new(
                                    "call-w",
                                    ToolCallUpdateFields::new()
                                        .kind(kind)
                                        .title(label)
                                        .locations(locations)
                                        .raw_input(input),
                                ),
                                vec![
                                    PermissionOption::new(
                                        "allow-once",
                                        "Allow",
                                        PermissionOptionKind::AllowOnce,
                                    ),
                                    PermissionOption::new(
                                        "allow-always",
                                        "Always allow",
                                        PermissionOptionKind::AllowAlways,
                                    ),
                                    PermissionOption::new(
                                        "reject-once",
                                        "Deny",
                                        PermissionOptionKind::RejectOnce,
                                    ),
                                ],
                            );
                            let chosen = match worker.send_request(ask).block_task().await?.outcome {
                                RequestPermissionOutcome::Selected(s) => s.option_id.to_string(),
                                _ => "cancelled".to_string(),
                            };
                            say(&worker, &session, &format!("permission: {chosen}\n"))?;
                        }
                        if title.contains("with sibling") {
                            let beside = cwd.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                            for _ in 0..1800 {
                                if std::fs::read_dir(&beside).map_or(0, |d| d.count()) >= 2 {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            }
                        }
                        // "write <file>" commits <file>, holding the bead's id, before the bead closes.
                        if let Some(file) = title
                            .split_once("write ")
                            .and_then(|(_, f)| f.split_whitespace().next())
                        {
                            let git = |args: &[&str]| {
                                let _ = std::process::Command::new("git")
                                    .args([
                                        "-c",
                                        "user.name=worker",
                                        "-c",
                                        "user.email=worker@example.com",
                                    ])
                                    .args(args)
                                    .current_dir(&cwd)
                                    .output();
                            };
                            let _ = std::fs::write(cwd.join(file), format!("{bead}\n"));
                            git(&["add", file]);
                            git(&["commit", "-q", "-m", &format!("write {file} ({bead})")]);
                        }
                        // "follow-ups" leaves two `follow-up:` lines in the bead's notes: the fake
                        // supervisor files the one marked `[file]` and drops the one marked `[drop]`.
                        if title.contains("follow-ups") {
                            run_bd(
                                &worker,
                                &session,
                                &cwd,
                                &[
                                    "update",
                                    bead,
                                    "--append-notes",
                                    "follow-up: tidy a at a.rs:1 — [file] it is untidy\nfollow-up: tidy b at b.rs:2 — [drop] it is fine",
                                ],
                            )
                            .await?;
                        }
                        if text.contains("Your supervisor answered") || title.contains("close") {
                            run_bd(&worker, &session, &cwd, &["close", bead]).await?;
                        } else if title.contains("block") {
                            run_bd(&worker, &session, &cwd, &["update", bead, "--status", "blocked"])
                                .await?;
                        }
                    }
                    if text.contains("show setup") {
                        let list = |l: &Option<Vec<String>>| {
                            l.as_ref().map_or("-".into(), |l| l.join(","))
                        };
                        let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "-".into());
                        say(
                            &worker,
                            &session,
                            &format!(
                                "setup: sources={} tools={} config_dir={dir}\n",
                                list(&sources),
                                list(&offered)
                            ),
                        )?;
                    }
                    if text.contains("edit while asking") {
                        // A request whose tool line changes after the request arrived: the call
                        // is asked about with `before`, then its content is replaced with `after`.
                        let note = |words: &str| vec![ToolCallContent::from(words)];
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCall(
                                ToolCall::new("call-6", "Edit src/b.rs")
                                    .kind(ToolKind::Edit)
                                    .content(note("before")),
                            ),
                        ))?;
                        let ask = RequestPermissionRequest::new(
                            session.clone(),
                            ToolCallUpdate::new(
                                "call-6",
                                ToolCallUpdateFields::new().title("Edit src/b.rs"),
                            ),
                            vec![PermissionOption::new(
                                "allow-once",
                                "Allow",
                                PermissionOptionKind::AllowOnce,
                            )],
                        );
                        let waiting = worker.send_request(ask);
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                "call-6",
                                ToolCallUpdateFields::new().content(note("after")),
                            )),
                        ))?;
                        waiting.block_task().await?;
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
                        let tools = offered
                            .clone()
                            .unwrap_or_else(|| TOOLS.map(String::from).to_vec());
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
                    if text.contains("then a tool") {
                        // A turn that ends on a tool call after its reply.
                        worker.send_notification(SessionNotification::new(
                            session.clone(),
                            SessionUpdate::ToolCall(
                                ToolCall::new("call-6", "Read src/last.rs")
                                    .kind(ToolKind::Read)
                                    .status(ToolCallStatus::Completed),
                            ),
                        ))?;
                    }
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
