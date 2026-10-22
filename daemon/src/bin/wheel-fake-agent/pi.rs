//! The pi stand-in: `wheel-fake-agent --mode rpc` speaks the RPC protocol `pi --mode rpc` does,
//! one JSON object per line. `--session-id <id>` starts a session under that id and `--session
//! <file>` reopens the one kept in that file, exiting 1 with `No session found matching` when
//! there is none, as pi does. It keeps each session as pi does: a file at
//! `$PI_CODING_AGENT_DIR/sessions/--<cwd, every / as ->--/<time>_<id>.jsonl` (`~/.pi/agent` when
//! the variable is unset) whose first line is a `session` header naming the id and the directory,
//! then one `message` entry per user and assistant message, written once the first prompt
//! arrives.
//!
//! It answers `get_state` (thinking level `medium`), `get_available_thinking_levels` (`off`,
//! `low`, `medium`, `high`), `set_thinking_level` for those and refuses any other,
//! `get_messages` with the session's messages, and `get_session_stats` with 10,000 tokens per
//! turn the process has run in a 200,000-token window. Each `prompt` is accepted with
//! `disposition: started`, and its reply `echo: <prompt>` streams as `text_delta` updates before
//! `agent_settled`. `abort` stops the running turn, which settles with `aborted: true`. Before
//! the echo, a prompt containing:
//! - "show setup" reports `setup: pi_home=<PI_CODING_AGENT_DIR> session=<id> thinking=<level>`
//!   and `brief: <the first line of --append-system-prompt>` (`-` when unset or none);
//! - "permission" asks, as an extension does, with an `extension_ui_request` `select` titled
//!   `Allow rm -rf build?` offering `Yes` and `No`, then runs `rm -rf build` as tool call "call-1"
//!   when the answer is `Yes` and blocks it otherwise, and reports `permission: <value>` (or
//!   `cancelled`);
//! - "work slowly" runs `sleep 30` as tool call "call-3" for 30 s unless aborted;
//! - "fail now" ends the reply with stop reason `error` and the message `429 rate limited`,
//!   sending no echo.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{oneshot, Notify};

const WINDOW: u64 = 200_000;
const LEVELS: [&str; 4] = ["off", "low", "medium", "high"];

#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn send(&self, line: Value) {
        let mut stdout = self.0.lock().unwrap();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
}

struct State {
    out: Out,
    id: String,
    file: PathBuf,
    cwd: String,
    brief: Option<String>,
    thinking: Mutex<String>,
    messages: Mutex<Vec<Value>>,
    turns: AtomicU64,
    /// The last prompt asked for a compaction, so pi has no token count until the next reply.
    compacted: AtomicBool,
    answers: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    next_dialog: AtomicU64,
    aborted: AtomicBool,
    abort: Notify,
}

impl State {
    /// Add `message` to the session, writing the file with its header the first time.
    fn keep(&self, message: Value) {
        let mut messages = self.messages.lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .unwrap();
        if std::fs::metadata(&self.file).map(|m| m.len()).unwrap_or(0) == 0 {
            let header = json!({"type": "session", "version": 3, "id": self.id, "cwd": self.cwd});
            let _ = writeln!(file, "{header}");
        }
        let _ = writeln!(file, "{}", json!({"type": "message", "message": message}));
        messages.push(message);
    }

    fn event(&self, event: Value) {
        self.out.send(event);
    }

    fn text(&self, text: &str) {
        self.event(json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text}}));
    }
}

fn home() -> PathBuf {
    std::env::var("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".pi/agent")
        })
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

pub async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let (id, file, messages) = match arg_after(&args, "--session") {
        Some(path) => {
            let Ok(text) = std::fs::read_to_string(&path) else {
                eprintln!("No session found matching '{path}'");
                std::process::exit(1);
            };
            let mut lines = text
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok());
            let header = lines.next().unwrap_or_default();
            let messages = lines
                .filter(|e| e["type"] == "message")
                .map(|e| e["message"].clone())
                .collect();
            let id = header["id"].as_str().unwrap_or_default().to_string();
            (id, PathBuf::from(path), messages)
        }
        None => {
            let id = arg_after(&args, "--session-id").unwrap_or_else(|| "fake-pi-session".into());
            let dir = home().join("sessions").join(format!(
                "--{}--",
                cwd.trim_start_matches('/').replace('/', "-")
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join(format!("2026-01-01T00-00-00-000Z_{id}.jsonl"));
            (id, file, Vec::new())
        }
    };
    // A resumed session has already had a turn for every prompt it holds.
    let turns = messages.iter().filter(|m| m["role"] == "user").count() as u64;
    let state = Arc::new(State {
        out: Out(Arc::new(Mutex::new(std::io::stdout()))),
        id,
        file,
        cwd,
        brief: arg_after(&args, "--append-system-prompt"),
        thinking: Mutex::new("medium".into()),
        messages: Mutex::new(messages),
        turns: AtomicU64::new(turns),
        compacted: AtomicBool::new(false),
        answers: Mutex::new(HashMap::new()),
        next_dialog: AtomicU64::new(1),
        aborted: AtomicBool::new(false),
        abort: Notify::new(),
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut running: Option<tokio::task::JoinHandle<()>> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(command) = serde_json::from_str::<Value>(&line) else {
            state.out.send(json!({"type": "response", "command": "parse", "success": false, "error": "Failed to parse command"}));
            continue;
        };
        let id = command["id"].clone();
        let reply = |command: &str, data: Value| json!({"id": id, "type": "response", "command": command, "success": true, "data": data});
        match command["type"].as_str().unwrap_or_default() {
            "get_state" => state.out.send(reply(
                "get_state",
                json!({"sessionId": state.id, "thinkingLevel": *state.thinking.lock().unwrap(), "model": {"id": "fake", "provider": "fake", "contextWindow": WINDOW}, "isStreaming": false}),
            )),
            "get_available_thinking_levels" => state
                .out
                .send(reply("get_available_thinking_levels", json!({"levels": LEVELS}))),
            "get_messages" => state.out.send(reply(
                "get_messages",
                json!({"messages": *state.messages.lock().unwrap()}),
            )),
            "set_thinking_level" => {
                let level = command["level"].as_str().unwrap_or_default();
                if LEVELS.contains(&level) {
                    *state.thinking.lock().unwrap() = level.to_string();
                    state.out.send(json!({"id": id, "type": "response", "command": "set_thinking_level", "success": true}));
                } else {
                    state.out.send(json!({"id": id, "type": "response", "command": "set_thinking_level", "success": false, "error": format!("Unknown thinking level: {level}")}));
                }
            }
            "get_session_stats" => {
                let used = state.turns.load(Ordering::SeqCst) * 10_000;
                let usage = if state.compacted.load(Ordering::SeqCst) {
                    json!({"tokens": null, "contextWindow": WINDOW, "percent": null})
                } else {
                    json!({"tokens": used, "contextWindow": WINDOW, "percent": used * 100 / WINDOW})
                };
                state.out.send(reply(
                    "get_session_stats",
                    json!({"sessionId": state.id, "contextUsage": usage}),
                ));
            }
            "prompt" => {
                let text = command["message"].as_str().unwrap_or_default().to_string();
                state
                    .out
                    .send(reply("prompt", json!({"disposition": "started"})));
                state.aborted.store(false, Ordering::SeqCst);
                running = Some(tokio::spawn(turn(state.clone(), text)));
            }
            "abort" => {
                state.aborted.store(true, Ordering::SeqCst);
                state.abort.notify_waiters();
                if let Some(turn) = running.take() {
                    let _ = turn.await;
                }
                state.out.send(json!({"id": id, "type": "response", "command": "abort", "success": true}));
            }
            "extension_ui_response" => {
                let key = command["id"].as_str().unwrap_or_default().to_string();
                if let Some(answer) = state.answers.lock().unwrap().remove(&key) {
                    let _ = answer.send(command);
                }
            }
            other => state.out.send(json!({"id": id, "type": "response", "command": other, "success": false, "error": format!("Unknown command: {other}")})),
        }
    }
}

/// Ask the client to pick one of `options`, as an extension's `ctx.ui.select` does; the answer is
/// the picked option, or none when the dialog was cancelled.
async fn select(state: &State, title: &str, options: &[&str]) -> Option<String> {
    let id = format!(
        "dialog-{}",
        state.next_dialog.fetch_add(1, Ordering::SeqCst)
    );
    let (tx, rx) = oneshot::channel();
    state.answers.lock().unwrap().insert(id.clone(), tx);
    state.event(json!({"type": "extension_ui_request", "id": id, "method": "select", "title": title, "options": options}));
    let answer = rx.await.ok()?;
    answer["value"].as_str().map(String::from)
}

/// Wait `secs`, or less when the turn is aborted; true when it was.
async fn wait_or_abort(state: &State, secs: u64) -> bool {
    let aborted = state.abort.notified();
    if state.aborted.load(Ordering::SeqCst) {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => false,
        _ = aborted => true,
    }
}

async fn turn(state: Arc<State>, prompt: String) {
    state.turns.fetch_add(1, Ordering::SeqCst);
    state
        .compacted
        .store(prompt.contains("compacted"), Ordering::SeqCst);
    let user = json!({"role": "user", "content": [{"type": "text", "text": prompt}]});
    state.keep(user.clone());
    state.event(json!({"type": "agent_start"}));
    state.event(json!({"type": "turn_start"}));
    state.event(json!({"type": "message_start", "message": user}));
    state.event(json!({"type": "message_end", "message": user}));
    state.event(json!({"type": "message_start", "message": {"role": "assistant", "content": [], "stopReason": "pending"}}));
    let mut said = String::new();
    let mut say = |text: &str| {
        state.text(text);
        said.push_str(text);
    };
    let mut aborted = false;
    if prompt.contains("show setup") {
        let first = |text: Option<&String>| {
            text.and_then(|t| t.lines().next())
                .unwrap_or("-")
                .to_string()
        };
        let pi_home = std::env::var("PI_CODING_AGENT_DIR").unwrap_or_else(|_| "-".into());
        let thinking = state.thinking.lock().unwrap().clone();
        say(&format!(
            "setup: pi_home={pi_home} session={} thinking={thinking}\nbrief: {}\n",
            state.id,
            first(state.brief.as_ref())
        ));
    }
    if prompt.contains("permission") {
        let answer = select(&state, "Allow rm -rf build?", &["Yes", "No"]).await;
        let args = json!({"command": "rm -rf build"});
        state.event(json!({"type": "tool_execution_start", "toolCallId": "call-1", "toolName": "bash", "args": args}));
        let allowed = answer.as_deref() == Some("Yes");
        let result = if allowed {
            "(no output)"
        } else {
            "Blocked by user"
        };
        state.event(json!({"type": "tool_execution_end", "toolCallId": "call-1", "toolName": "bash", "result": {"content": [{"type": "text", "text": result}]}, "isError": !allowed}));
        say(&format!(
            "permission: {}\n",
            answer.as_deref().unwrap_or("cancelled")
        ));
    }
    if prompt.contains("work slowly") {
        let args = json!({"command": "sleep 30"});
        state.event(json!({"type": "tool_execution_start", "toolCallId": "call-3", "toolName": "bash", "args": args}));
        aborted = wait_or_abort(&state, 30).await;
        let text = if aborted { "Command aborted" } else { "" };
        state.event(json!({"type": "tool_execution_end", "toolCallId": "call-3", "toolName": "bash", "result": {"content": [{"type": "text", "text": text}]}, "isError": aborted}));
    }
    let (stop, error) = if aborted {
        ("error", Some("This operation was aborted"))
    } else if prompt.contains("fail now") {
        ("error", Some("429 rate limited"))
    } else {
        say(&format!("echo: {prompt}"));
        ("stop", None)
    };
    let mut reply = json!({"role": "assistant", "content": [{"type": "text", "text": said}], "stopReason": stop});
    if let Some(error) = error {
        reply["errorMessage"] = json!(error);
    }
    state.keep(reply.clone());
    state.event(json!({"type": "message_end", "message": reply}));
    state.event(json!({"type": "turn_end", "message": reply, "toolResults": []}));
    state.event(json!({"type": "agent_end", "messages": [], "willRetry": false}));
    state.event(json!({"type": "agent_settled", "aborted": aborted}));
}
