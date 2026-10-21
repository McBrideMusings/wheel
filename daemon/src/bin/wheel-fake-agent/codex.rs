//! The Codex stand-in: `wheel-fake-agent app-server` speaks the JSON-RPC `codex app-server` does,
//! one object per line. It answers `initialize`, opens a thread on `thread/start` (the id is made
//! up) and reopens one on `thread/resume`, keeping each thread's turns in
//! `$CODEX_HOME/fake-threads/<id>.jsonl` (`~/.codex` when the variable is unset); resuming an id
//! with no file is an error, as Codex's is. `turn/interrupt` stops the running turn.
//!
//! Each `turn/start` prompt gets the reply `echo: <prompt>`, streamed as `agentMessage` deltas,
//! then `thread/tokenUsage/updated` reporting 10,000 tokens per turn the thread has had in a
//! 200,000-token window. Before the echo, a prompt containing:
//! - "show setup" reports `setup: codex_home=<CODEX_HOME> thread=<id> policy=<approvalPolicy of
//!   the turn>` and `brief: <the first line of developerInstructions>` (`-` when unset or none);
//! - "permission" runs `rm -rf build` as item "call-1", asks `item/commandExecution/requestApproval`
//!   offering `accept`, `acceptForSession` and `cancel`, and reports `permission: <decision>`;
//! - "edit" changes `src/a.rs` as item "call-2", asks `item/fileChange/requestApproval` and
//!   reports `edit: <decision>`;
//! - "reflect" streams the reasoning summary `pondering ` and `the question`;
//! - "work slowly" works for 30 s unless interrupted;
//! - "die now" exits with status 3 mid-turn.

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

#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn send(&self, line: Value) {
        let mut stdout = self.0.lock().unwrap();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({"method": method, "params": params}));
    }
}

struct State {
    out: Out,
    thread: Mutex<String>,
    brief: Mutex<Option<String>>,
    turns: AtomicU64,
    next_server_request: AtomicU64,
    answers: Mutex<HashMap<u64, oneshot::Sender<String>>>,
    interrupted: AtomicBool,
    interrupt: Notify,
}

fn home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".codex"))
}

fn thread_file(id: &str) -> PathBuf {
    home().join("fake-threads").join(format!("{id}.jsonl"))
}

fn keep(id: &str, prompt: &str, reply: &str) {
    let _ = std::fs::create_dir_all(home().join("fake-threads"));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(thread_file(id))
    {
        let _ = writeln!(file, "{}", json!({"prompt": prompt, "reply": reply}));
    }
}

/// The thread's kept turns, as `thread/resume` reports them.
fn turns(id: &str) -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(thread_file(id)).ok()?;
    Some(
        text.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .enumerate()
            .map(|(n, kept)| {
                json!({"id": format!("turn-{n}"), "status": "completed", "items": [
                    {"type": "userMessage", "id": format!("u{n}"), "content": [{"type": "text", "text": kept["prompt"]}]},
                    {"type": "agentMessage", "id": format!("m{n}"), "text": kept["reply"], "phase": "final_answer"},
                ]})
            })
            .collect(),
    )
}

fn made_up_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:08x}-0000-4000-8000-{:012x}",
        std::process::id(),
        nanos & 0xffff_ffff_ffff
    )
}

pub async fn main() {
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let state = Arc::new(State {
        out: out.clone(),
        thread: Mutex::new(String::new()),
        brief: Mutex::new(None),
        turns: AtomicU64::new(0),
        next_server_request: AtomicU64::new(100),
        answers: Mutex::new(HashMap::new()),
        interrupted: AtomicBool::new(false),
        interrupt: Notify::new(),
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message["id"].clone();
        let params = &message["params"];
        match message["method"].as_str() {
            Some("initialize") => out.send(json!({"id": id, "result": {
                "userAgent": "wheel-fake-codex/0",
                "codexHome": home().display().to_string(),
                "platformFamily": "unix",
                "platformOs": "macos",
            }})),
            Some("thread/start") => {
                let thread = made_up_id();
                *state.thread.lock().unwrap() = thread.clone();
                *state.brief.lock().unwrap() =
                    params["developerInstructions"].as_str().map(String::from);
                out.send(json!({"id": id, "result": {
                    "thread": {"id": thread, "turns": []},
                    "model": "fake-codex",
                }}));
            }
            Some("thread/resume") => {
                let thread = params["threadId"].as_str().unwrap_or_default().to_string();
                match turns(&thread) {
                    None => out.send(json!({"id": id, "error": {
                        "code": -32600,
                        "message": format!("no rollout found for thread id {thread}"),
                    }})),
                    Some(turns) => {
                        *state.thread.lock().unwrap() = thread.clone();
                        state.turns.store(turns.len() as u64, Ordering::SeqCst);
                        *state.brief.lock().unwrap() =
                            params["developerInstructions"].as_str().map(String::from);
                        out.send(json!({"id": id, "result": {
                            "thread": {"id": thread, "turns": turns},
                            "model": "fake-codex",
                        }}));
                    }
                }
            }
            Some("turn/start") => {
                state.interrupted.store(false, Ordering::SeqCst);
                let turn = format!("turn-{}", state.turns.load(Ordering::SeqCst));
                out.send(json!({"id": id, "result": {"turn": {"id": turn, "status": "inProgress", "items": []}}}));
                let prompt: String = params["input"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect();
                let policy = params["approvalPolicy"].as_str().unwrap_or("-").to_string();
                let state = state.clone();
                tokio::spawn(async move { run_turn(state, turn, prompt, policy).await });
            }
            Some("turn/interrupt") => {
                state.interrupted.store(true, Ordering::SeqCst);
                state.interrupt.notify_waiters();
                out.send(json!({"id": id, "result": {}}));
            }
            Some(_) => {
                if !id.is_null() {
                    out.send(json!({"id": id, "result": {}}));
                }
            }
            // A response to a request this side made.
            None => {
                if let Some(n) = id.as_u64() {
                    let decision = message["result"]["decision"]
                        .as_str()
                        .unwrap_or("error")
                        .to_string();
                    if let Some(tx) = state.answers.lock().unwrap().remove(&n) {
                        let _ = tx.send(decision);
                    }
                }
            }
        }
    }
}

/// Ask the host to approve `method`, and wait for its decision, or for an interrupt.
async fn ask(state: &State, method: &str, params: Value, decisions: Value) -> Option<String> {
    let n = state.next_server_request.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = oneshot::channel();
    state.answers.lock().unwrap().insert(n, tx);
    let mut params = params;
    params["availableDecisions"] = decisions;
    state
        .out
        .send(json!({"method": method, "id": n, "params": params}));
    let waiting = state.interrupt.notified();
    let decision = tokio::select! {
        decision = rx => decision.ok(),
        _ = waiting => None,
    };
    state.answers.lock().unwrap().remove(&n);
    state
        .out
        .notify("serverRequest/resolved", json!({"requestId": n}));
    decision
}

async fn run_turn(state: Arc<State>, turn: String, prompt: String, policy: String) {
    let out = &state.out;
    let thread = state.thread.lock().unwrap().clone();
    let at = |mut params: Value| {
        params["threadId"] = json!(thread);
        params["turnId"] = json!(turn);
        params
    };
    out.notify(
        "turn/started",
        json!({"threadId": thread, "turn": {"id": turn, "status": "inProgress", "items": []}}),
    );
    let user =
        json!({"type": "userMessage", "id": "user", "content": [{"type": "text", "text": prompt}]});
    out.notify("item/started", at(json!({"item": user})));
    out.notify("item/completed", at(json!({"item": user})));

    let mut reply = String::new();
    let mut say = |text: &str| {
        if !reply.is_empty() {
            reply.push_str(" | ");
        }
        reply.push_str(text);
    };
    if prompt.contains("die now") {
        std::process::exit(3);
    }
    if prompt.contains("show setup") {
        let home = std::env::var("CODEX_HOME").unwrap_or_else(|_| "-".into());
        let brief = state.brief.lock().unwrap().clone();
        let first = brief
            .as_deref()
            .and_then(|b| b.lines().next())
            .unwrap_or("-")
            .to_string();
        say(&format!(
            "setup: codex_home={home} thread={thread} policy={policy}"
        ));
        say(&format!("brief: {first}"));
    }
    if prompt.contains("reflect") {
        let item = json!({"type": "reasoning", "id": "think", "summary": [], "content": []});
        out.notify("item/started", at(json!({"item": item})));
        for delta in ["pondering ", "the question"] {
            out.notify(
                "item/reasoning/summaryTextDelta",
                at(json!({"itemId": "think", "delta": delta, "summaryIndex": 0})),
            );
        }
        out.notify(
            "item/completed",
            at(json!({"item": {"type": "reasoning", "id": "think", "summary": ["pondering the question"], "content": []}})),
        );
    }
    if prompt.contains("permission") {
        let item = json!({"type": "commandExecution", "id": "call-1", "command": "/bin/zsh -lc 'rm -rf build'",
            "cwd": "/tmp", "commandActions": [{"type": "unknown", "command": "rm -rf build"}],
            "status": "inProgress", "aggregatedOutput": null, "exitCode": null});
        out.notify("item/started", at(json!({"item": item})));
        let decision = ask(
            &state,
            "item/commandExecution/requestApproval",
            at(json!({"kind": "command", "itemId": "call-1", "command": "/bin/zsh -lc 'rm -rf build'",
                "cwd": "/tmp", "commandActions": [{"type": "unknown", "command": "rm -rf build"}]})),
            json!(["accept", "acceptForSession", "cancel"]),
        )
        .await;
        let accepted = decision.as_deref().is_some_and(|d| d.starts_with("accept"));
        let mut done = item.clone();
        done["status"] = json!(if accepted { "completed" } else { "declined" });
        if accepted {
            done["aggregatedOutput"] = json!("removed build");
            done["exitCode"] = json!(0);
        }
        out.notify("item/completed", at(json!({"item": done})));
        say(&format!(
            "permission: {}",
            decision.as_deref().unwrap_or("none")
        ));
    }
    if prompt.contains("edit") {
        let item = json!({"type": "fileChange", "id": "call-2", "status": "inProgress", "changes": [
            {"path": "src/a.rs", "kind": {"type": "update", "move_path": null}, "diff": "@@\n-one\n+two\n"}]});
        out.notify("item/started", at(json!({"item": item})));
        let decision = ask(
            &state,
            "item/fileChange/requestApproval",
            at(json!({"itemId": "call-2"})),
            json!(null),
        )
        .await;
        let accepted = decision.as_deref().is_some_and(|d| d.starts_with("accept"));
        let mut done = item.clone();
        done["status"] = json!(if accepted { "completed" } else { "declined" });
        out.notify("item/completed", at(json!({"item": done})));
        say(&format!("edit: {}", decision.as_deref().unwrap_or("none")));
    }
    if prompt.contains("work slowly") {
        for _ in 0..300 {
            if state.interrupted.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let interrupted = state.interrupted.load(Ordering::SeqCst);
    if !interrupted {
        say(&format!("echo: {prompt}"));
        let item =
            json!({"type": "agentMessage", "id": "reply", "text": "", "phase": "final_answer"});
        out.notify("item/started", at(json!({"item": item})));
        for chunk in reply.as_bytes().chunks(8) {
            out.notify(
                "item/agentMessage/delta",
                at(json!({"itemId": "reply", "delta": String::from_utf8_lossy(chunk)})),
            );
        }
        out.notify(
            "item/completed",
            at(json!({"item": {"type": "agentMessage", "id": "reply", "text": reply, "phase": "final_answer"}})),
        );
        keep(&thread, &prompt, &reply);
    }
    let count = state.turns.fetch_add(1, Ordering::SeqCst) + 1;
    out.notify(
        "thread/tokenUsage/updated",
        at(json!({"tokenUsage": {
            "total": {"totalTokens": count * 10_000},
            "last": {"totalTokens": count * 10_000},
            "modelContextWindow": WINDOW,
        }})),
    );
    out.notify(
        "turn/completed",
        json!({"threadId": thread, "turn": {
            "id": turn,
            "status": if interrupted { "interrupted" } else { "completed" },
            "items": [], "error": null,
        }}),
    );
}
