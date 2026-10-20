//! A scripted ACP agent for tests, so they never call a real model. Over stdio it:
//! - answers every prompt with `echo: <prompt>`;
//! - for a prompt containing "show setup", reports `setup: config_dir=<CLAUDE_CONFIG_DIR>` as this
//!   process was started (`-` when unset), then `brief: <the first line of
//!   _meta.systemPrompt.append>` and `seat: <its line starting "You are the ">`, each `-` when there
//!   is none.
//! - for a prompt containing "permission", announces a tool call, asks permission for it, and
//!   reports `permission: <chosen option id>` (or `permission: cancelled`) before the echo;
//! - for a prompt containing "diff", announces tool call "call-2" editing `src/a.rs` (a text block
//!   and a diff `one\n` -> `two\n`), then completes it with an update whose content is a diff
//!   `one\n` -> `three\n` and a terminal `term-1`;
//! - for a prompt containing "work slowly", works the way a real turn paces itself, so a client can
//!   show work in progress: a thought in three chunks 0.5 s apart, then three tool calls in a row —
//!   "call-3" reading `src/main.rs` (in progress, completed after 1 s), "call-4" searching (completed
//!   after 0.5 s) and "call-5" running `cargo test` (in progress for 6 s, or the number of seconds
//!   after "work slowly", then completed) — before the echo.
//! - for a prompt containing "then a tool", ends the turn on a completed tool call "call-6" (reading
//!   `src/last.rs`) after the echo.
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
//! Every reply ends with a `usage_update` for a 200,000-token window: 10,000 tokens used per turn the
//! process has answered, 190,000 when the prompt contains `context full` and 8,000 when it contains
//! `context compacted`.
//!
//! It offers four config options — `mode`, `model` and `effort` selects and a `fast` boolean — so a test can
//! drive `session/set_config_option` without a real model. Setting one to a value it does not offer
//! fails; every set answers with the whole list, as ACP requires. With `--no-mode` it offers no
//! `mode` option, as an agent with no modes would; a prompt containing "switch mode" sends a
//! `current_mode_update` to `plan`, whether or not it offers the option.
//!
//! Run under the name `claude` (a link named so), it is a stand-in for the Claude Code TUI in a
//! terminal instead: it keeps its session in the store the agent's `--store` would name for that
//! directory, `$CLAUDE_CONFIG_DIR/projects/<cwd, every non-alphanumeric character a ->/<id>.jsonl`,
//! in the same kept-turn lines. `--resume <id>` or `--session-id <id>` names the session; without
//! either it makes up an id. It prints each kept turn as `> <prompt>` and its echo, then
//! `claude-fake <id> ready`, then answers each line read with `echo: <line>` and keeps the turn,
//! until `/exit` or the end of its input.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk,
    CurrentModeUpdate, Diff, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, Meta, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, ResourceLink, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigBoolean, SessionConfigId, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelect, SessionConfigSelectOption, SessionId, SessionInfo,
    SessionListCapabilities, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason, Terminal, TextContent, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error, Result, Stdio};

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

/// Turns this process has answered, for the context use each reply reports.
static TURNS: AtomicU64 = AtomicU64::new(0);

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
/// The values the `effort` select takes; the first is where a session starts.
const EFFORTS: [&str; 3] = ["medium", "low", "high"];

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
            select(
                "effort",
                "Effort",
                SessionConfigOptionCategory::ThoughtLevel,
                EFFORTS,
                &current.effort,
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
    effort: String,
    fast: bool,
}

impl ConfigState {
    fn new(has_mode: bool) -> Self {
        Self {
            has_mode,
            mode: MODES[0].to_string(),
            model: MODELS[0].to_string(),
            effort: EFFORTS[0].to_string(),
            fast: false,
        }
    }

    /// Take a value for one option, or say why it cannot.
    fn set(&mut self, id: &str, value: &SessionConfigOptionValue) -> Result<()> {
        let (chosen, values) = match id {
            "mode" => (&mut self.mode, MODES),
            "model" => (&mut self.model, MODELS),
            "effort" => (&mut self.effort, EFFORTS),
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

/// The text a new or loaded session asked to have appended to its system prompt.
fn session_brief(meta: &Option<Meta>) -> Option<String> {
    meta.as_ref()
        .and_then(|m| m.get("systemPrompt"))
        .and_then(|p| p.get("append"))
        .and_then(|a| a.as_str())
        .map(String::from)
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

/// The Claude Code TUI stand-in: see the module's last paragraph.
fn claude_tui() -> std::io::Result<()> {
    use std::io::BufRead;
    let mut args = std::env::args().skip(1);
    let mut session = None;
    while let Some(arg) = args.next() {
        if arg == "--resume" || arg == "--session-id" {
            session = args.next();
        }
    }
    let session = session.unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!(
            "{:08x}-0000-4000-8000-{:012x}",
            std::process::id(),
            nanos & 0xffff_ffff_ffff
        )
    });
    let home = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude")
        });
    let project: String = std::env::current_dir()?
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = home.join("projects").join(project);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{session}.jsonl"));
    let mut out = std::io::stdout();
    for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
        if let Ok(kept) = serde_json::from_str::<Kept>(line) {
            writeln!(out, "> {}\n{}", kept.prompt, kept.reply)?;
        }
    }
    writeln!(out, "claude-fake {session} ready")?;
    out.flush()?;
    for line in std::io::stdin().lock().lines() {
        let prompt = line?.trim().to_string();
        if prompt == "/exit" {
            break;
        }
        if prompt.is_empty() {
            continue;
        }
        let kept = Kept {
            reply: format!("echo: {prompt}"),
            prompt,
        };
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                writeln!(f, "{}", serde_json::to_string(&kept).unwrap_or_default())
            })?;
        writeln!(out, "{}", kept.reply)?;
        out.flush()?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let invoked = std::env::args().next().unwrap_or_default();
    if std::path::Path::new(&invoked)
        .file_name()
        .and_then(|n| n.to_str())
        == Some("claude")
    {
        return claude_tui().map_err(|_| Error::internal_error());
    }
    let mut args = std::env::args().skip(1);
    let mut store: Option<PathBuf> = None;
    let mut has_mode = true;
    let mut recorded: Option<serde_json::Value> = None;
    let mut replay_secs = f64::INFINITY;
    let mut list_secs = 0.0;
    let usage = || {
        Error::invalid_params().data(
            "usage: wheel-fake-agent [--store <dir> [--list-secs <n>]] [--no-mode] [--replay <dump.json> [--replay-secs <n>]]",
        )
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(args.next().ok_or_else(usage)?.into()),
            "--no-mode" => has_mode = false,
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
    let file = |dir: &PathBuf, id: &SessionId| dir.join(format!("{id}.jsonl"));
    // The text the session asked to have appended to its system prompt.
    let brief: Arc<Mutex<Option<String>>> = Arc::default();
    let (on_new, on_load) = (brief.clone(), brief.clone());
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
                responder.respond(
                    InitializeResponse::new(init.protocol_version).agent_capabilities(
                        AgentCapabilities::new()
                            .session_capabilities(sessions)
                            .load_session(store.is_some()),
                    ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, _cx| {
                *on_new.lock().unwrap() = session_brief(&req.meta);
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
                *on_load.lock().unwrap() = session_brief(&req.meta);
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
                let brief = brief.lock().unwrap().clone();
                let store = prompt_store.clone();
                let recorded = recorded.clone();
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
                    if let Some(dump) = recorded.as_ref() {
                        replay(&worker, &session, dump, replay_secs).await?;
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                    if text.contains("die now") {
                        // Simulate the agent process crashing mid-turn.
                        std::process::exit(3);
                    }
                    if text.contains("show setup") {
                        let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "-".into());
                        let line = |pick: &dyn Fn(&str) -> bool| {
                            brief
                                .as_deref()
                                .and_then(|b| b.lines().find(|l| pick(l)))
                                .unwrap_or("-")
                                .to_string()
                        };
                        let first = line(&|l: &str| !l.is_empty());
                        let seat = line(&|l: &str| l.starts_with("You are the "));
                        say(
                            &worker,
                            &session,
                            &format!("setup: config_dir={dir}\nbrief: {first}\nseat: {seat}\n"),
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
                    let reply = format!("echo: {text}");
                    say(&worker, &session, &reply)?;
                    // Every reply ends with the window's fill: 10,000 tokens more per turn of this
                    // process, or a fixed figure when the prompt names one.
                    let turns = TURNS.fetch_add(1, Ordering::SeqCst) + 1;
                    let used = if text.contains("context full") {
                        190_000
                    } else if text.contains("context compacted") {
                        8_000
                    } else {
                        10_000 * turns
                    };
                    worker.send_notification(SessionNotification::new(
                        session.clone(),
                        SessionUpdate::UsageUpdate(UsageUpdate::new(used, 200_000)),
                    ))?;
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
