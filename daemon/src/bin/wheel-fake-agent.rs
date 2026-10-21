//! A scripted stand-in for Claude Code, so tests never call a real model. Run as
//! `wheel-fake-agent [own flags] --print … --input-format stream-json --output-format stream-json
//! [--session-id <id> | --resume <id>]` it speaks the stream-json wire `claude --print` does, and
//! keeps each conversation where Claude Code would: `$CLAUDE_CONFIG_DIR/projects/<cwd, every
//! non-alphanumeric character a ->/<id>.jsonl` (`~/.claude` when the variable is unset), one
//! transcript line per user and assistant message and a `turn_duration` entry when a turn is over,
//! in Claude Code's own shape. A conversation is
//! written out when its first prompt arrives; `--resume` of an id with no transcript exits 1, as
//! Claude Code does.
//!
//! It answers `initialize` with three models (`default`, `sonnet`, `haiku`) and permission mode
//! `default`, takes `set_permission_mode` (`default`, `acceptEdits`, `plan`, `auto`, `dontAsk`) and
//! `set_model` for those values and refuses any other, and stops the running turn on `interrupt`:
//! the turn ends at its next step with `[Request interrupted by user]`. Each prompt
//! `{"type":"user","message":{"content":…}}` gets the reply `echo: <prompt>`, streamed as text
//! deltas, then a `result` whose usage reports a 200,000-token window: 10,000 tokens used per turn
//! the process has answered, 190,000 when the prompt contains `context full` and 8,000 when it
//! contains `context compacted`. Before the echo, a prompt containing:
//! - "show setup" reports `setup: config_dir=<CLAUDE_CONFIG_DIR>` (`-` when unset), then `brief:
//!   <the first line of --append-system-prompt>` and `seat: <its line starting "You are the ">`,
//!   each `-` when there is none, then `wheel: env=… space=… pane=… url=… token_file=… path=…
//!   allowed=…`: the `WHEEL_*` variables, the first `PATH` entry, and `--allowedTools` and
//!   `--disallowedTools` as `<allowed> denied=<disallowed>`, each `-` when unset;
//! - "permission" calls Bash `rm -rf build` as tool call "call-1", asks permission for it with a
//!   `can_use_tool` request suggesting a rule that allows Bash for the session, and reports
//!   `permission: <allow|deny>`, followed by ` always` when the answer took the rule;
//! - "diff" calls Edit on `src/a.rs` (`one\n` -> `two\n`) as "call-2", whose result is `edited
//!   src/a.rs`;
//! - "reflect" thinks `pondering ` then `the question`;
//! - "switch mode" moves to plan mode on its own (`system/status` with `permissionMode`);
//! - "work slowly" works the way a real turn paces itself: a thought in three deltas 0.5 s apart,
//!   then "call-3" reading `src/main.rs` (done after 1 s), "call-4" grepping `parse_args` (done
//!   after 0.5 s) and "call-5" running `cargo test -p parser` (done after 6 s, or the number of
//!   seconds after "work slowly");
//! - "die now" exits with status 3 mid-turn.
//!
//! After the echo, "then a tool" ends the turn on tool call "call-6" reading `src/last.rs`.
//!
//! With `--replay <dump.json>` every prompt is answered instead by a recorded conversation: the
//! `text`, `tool_call` and `tool_update` events of a `wheel agent read --json` dump from its first
//! turn on, each sent after the gap it had when recorded. `--replay-secs <n>` stops the replay `n`
//! seconds after that first turn started.
//!
//! Given `app-server` it is a stand-in for `codex app-server` instead, and run under the name
//! `codex` for the Codex TUI; see `wheel-fake-agent/codex.rs`.
//!
//! Run under the name `claude` without `--print`, it is a stand-in for the Claude Code TUI in a
//! terminal: `--resume <id>` or `--session-id <id>` names the session, else it makes up an id. It
//! prints each kept turn as `> <prompt>` and its reply, then `claude-fake <id> ready`, then
//! answers each line read with `echo: <line>` and keeps the turn, until `/exit` or the end of its
//! input.

#[path = "wheel-fake-agent/codex.rs"]
mod codex;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot, Notify};

const MODES: [&str; 5] = ["default", "acceptEdits", "plan", "auto", "dontAsk"];
/// Each model's value, the model it resolves to, and its name.
const MODELS: [(&str, &str, &str); 3] = [
    ("default", "fake-default", "Default"),
    ("sonnet", "fake-sonnet", "Sonnet"),
    ("haiku", "fake-haiku", "Haiku"),
];
const WINDOW: u64 = 200_000;

/// The flags `claude --print` takes that carry a value.
const VALUED: [&str; 9] = [
    "--input-format",
    "--output-format",
    "--permission-prompts",
    "--permission-prompt-tool",
    "--session-id",
    "--resume",
    "--append-system-prompt",
    "--allowedTools",
    "--disallowedTools",
];

/// Where Claude Code keeps conversations for the directory this process runs in.
fn store_dir() -> PathBuf {
    let home = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude")
        });
    let cwd = std::env::current_dir().unwrap_or_default();
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join("projects").join(project)
}

fn transcript(id: &str) -> PathBuf {
    store_dir().join(format!("{id}.jsonl"))
}

/// Append one entry to conversation `id`'s transcript, creating it on the first.
fn keep(id: &str, mut entry: Value) {
    entry["sessionId"] = json!(id);
    let path = transcript(id);
    let _ = std::fs::create_dir_all(store_dir());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{entry}");
    }
}

fn user_entry(content: Value) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": content}, "parent_tool_use_id": null})
}

/// The entry Claude Code writes once a turn, stop hooks included, is over.
fn turn_over() -> Value {
    json!({"type": "system", "subtype": "turn_duration"})
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
    let session = session.unwrap_or_else(made_up_id);
    let mut out = std::io::stdout();
    for line in std::fs::read_to_string(transcript(&session))
        .unwrap_or_default()
        .lines()
    {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match (entry["type"].as_str(), &entry["message"]["content"]) {
            (Some("user"), Value::String(prompt)) => writeln!(out, "> {prompt}")?,
            (Some("assistant"), Value::Array(blocks)) => {
                for text in blocks.iter().filter_map(|b| b["text"].as_str()) {
                    writeln!(out, "{text}")?;
                }
            }
            _ => {}
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
        let reply = format!("echo: {prompt}");
        keep(&session, user_entry(json!(prompt)));
        keep(
            &session,
            json!({"type": "assistant", "message": {"role": "assistant", "model": "fake-default", "stop_reason": "end_turn", "content": [{"type": "text", "text": reply}]}}),
        );
        keep(&session, turn_over());
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

/// Writes stdout lines whole, from every task.
#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn send(&self, line: Value) {
        let mut out = self.0.lock().unwrap();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// What the process is set to and has been told.
struct State {
    mode: Mutex<String>,
    model: Mutex<String>,
    /// Permission answers waiting, by the request id this process asked under.
    answers: Mutex<HashMap<String, oneshot::Sender<String>>>,
    interrupted: AtomicBool,
    interrupt: Notify,
}

impl State {
    fn resolved_model(&self) -> &'static str {
        let model = self.model.lock().unwrap().clone();
        MODELS
            .iter()
            .find(|(v, _, _)| *v == model)
            .map_or("fake-default", |(_, r, _)| r)
    }
}

/// Everything one process needs to answer a turn.
struct Turn {
    out: Out,
    state: Arc<State>,
    session: String,
    message: u64,
}

impl Turn {
    /// Wait `ms`; false when the turn was interrupted first.
    async fn pause(&self, ms: u64) -> bool {
        if self.state.interrupted.load(Ordering::SeqCst) {
            return false;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => !self.state.interrupted.load(Ordering::SeqCst),
            _ = self.state.interrupt.notified() => false,
        }
    }

    fn line(&self, mut line: Value) {
        line["session_id"] = json!(self.session);
        line["parent_tool_use_id"] = Value::Null;
        self.out.send(line);
    }

    fn next_message(&mut self) -> String {
        self.message += 1;
        format!("msg_fake_{}_{}", std::process::id(), self.message)
    }

    /// One assistant message: its text streamed as deltas, then the whole message.
    fn say(&mut self, text: &str) {
        let id = self.next_message();
        self.line(json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": id}}}));
        for piece in text.split_inclusive(' ') {
            self.line(json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}}}));
        }
        self.assistant(id, vec![json!({"type": "text", "text": text})], None);
    }

    /// One thought, its deltas `pause_ms` apart.
    async fn think(&mut self, pieces: &[&str], pause_ms: u64) -> bool {
        let id = self.next_message();
        self.line(json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": id}}}));
        for piece in pieces {
            self.line(json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": piece}}}));
            if pause_ms > 0 && !self.pause(pause_ms).await {
                return false;
            }
        }
        self.assistant(
            id,
            vec![json!({"type": "thinking", "thinking": pieces.concat()})],
            None,
        );
        true
    }

    fn assistant(&self, id: String, content: Vec<Value>, used: Option<u64>) {
        let mut message = json!({"id": id, "role": "assistant", "model": self.state.resolved_model(), "content": content});
        if let Some(used) = used {
            message["usage"] = json!({"input_tokens": used - 100, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 100});
            message["stop_reason"] = json!("end_turn");
        }
        let entry = json!({"type": "assistant", "message": message});
        keep(&self.session, entry.clone());
        self.line(entry);
    }

    fn tool(&mut self, id: &str, name: &str, input: Value) {
        let message = self.next_message();
        self.assistant(
            message,
            vec![json!({"type": "tool_use", "id": id, "name": name, "input": input})],
            None,
        );
    }

    fn tool_done(&self, id: &str, result: &str) {
        let entry = user_entry(
            json!([{"type": "tool_result", "tool_use_id": id, "content": result, "is_error": false}]),
        );
        keep(&self.session, entry.clone());
        self.line(entry);
    }

    /// Ask permission for tool call `id`, and wait for the answer: `allow`, `deny`, or none when
    /// the turn was interrupted first.
    async fn ask(&self, id: &str, name: &str, input: Value) -> Option<String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let request = format!("fake-perm-{}", NEXT.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        self.state
            .answers
            .lock()
            .unwrap()
            .insert(request.clone(), tx);
        self.out.send(json!({"type": "control_request", "request_id": request, "request": {"subtype": "can_use_tool", "tool_name": name, "input": input, "tool_use_id": id, "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": name}], "behavior": "allow", "destination": "session"}]}}));
        let answer = tokio::select! {
            answer = rx => answer.ok(),
            _ = self.state.interrupt.notified() => None,
        };
        // An interrupt arriving just before the wait began is seen here instead.
        answer.filter(|_| !self.state.interrupted.load(Ordering::SeqCst))
    }
}

/// Turns this process has answered, for the context use each reply reports.
static TURNS: AtomicU64 = AtomicU64::new(0);

/// The flags this process was started with.
struct Args {
    replay: Option<Value>,
    replay_secs: f64,
    session: String,
    resume: bool,
    brief: Option<String>,
    allowed: Option<String>,
    disallowed: Option<String>,
}

async fn answer(turn: &mut Turn, args: &Args, text: &str) {
    let out_of_time = |turn: &mut Turn| {
        let entry = user_entry(json!([{"type": "text", "text": "[Request interrupted by user]"}]));
        keep(&turn.session, entry.clone());
        turn.line(entry);
    };
    if let Some(dump) = &args.replay {
        if !replay(turn, dump, args.replay_secs).await {
            out_of_time(turn);
        }
        return;
    }
    if text.contains("die now") {
        // Simulate the agent process crashing mid-turn.
        std::process::exit(3);
    }
    if text.contains("show setup") {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "-".into());
        let line = |pick: &dyn Fn(&str) -> bool| {
            args.brief
                .as_deref()
                .and_then(|b| b.lines().find(|l| pick(l)))
                .unwrap_or("-")
                .to_string()
        };
        let first = line(&|l: &str| !l.is_empty());
        let seat = line(&|l: &str| l.starts_with("You are the "));
        let allowed = match (&args.allowed, &args.disallowed) {
            (None, None) => "-".to_string(),
            (a, d) => format!(
                "{} denied={}",
                a.as_deref().unwrap_or_default(),
                d.as_deref().unwrap_or_default()
            ),
        };
        turn.say(&format!(
            "setup: config_dir={}\nbrief: {first}\nseat: {seat}\nwheel: env={} space={} pane={} url={} token_file={} path={} allowed={allowed}\n",
            var("CLAUDE_CONFIG_DIR"),
            var("WHEEL_ENV"),
            var("WHEEL_SPACE"),
            var("WHEEL_PANE"),
            var("WHEEL_URL"),
            var("WHEEL_TOKEN_FILE"),
            std::env::var("PATH").unwrap_or_default().split(':').next().unwrap_or("-"),
        ));
    }
    if text.contains("permission") {
        let input = json!({"command": "rm -rf build"});
        turn.tool("call-1", "Bash", input.clone());
        match turn.ask("call-1", "Bash", input).await {
            Some(behavior) => {
                turn.tool_done("call-1", "removed build");
                turn.say(&format!("permission: {behavior}\n"));
            }
            None => return out_of_time(turn),
        }
    }
    if text.contains("switch mode") {
        *turn.state.mode.lock().unwrap() = "plan".into();
        turn.out.send(json!({"type": "system", "subtype": "status", "status": null, "permissionMode": "plan", "session_id": turn.session}));
    }
    if text.contains("reflect") && !turn.think(&["pondering ", "the question"], 0).await {
        return out_of_time(turn);
    }
    if text.contains("diff") {
        turn.tool(
            "call-2",
            "Edit",
            json!({"file_path": "src/a.rs", "old_string": "one\n", "new_string": "two\n"}),
        );
        turn.tool_done("call-2", "edited src/a.rs");
    }
    if let Some(rest) = text.split("work slowly").nth(1) {
        let secs: u64 = rest
            .split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(6);
        if !turn
            .think(
                &["Looking at ", "the tests that ", "touch the parser."],
                500,
            )
            .await
        {
            return out_of_time(turn);
        }
        turn.tool("call-3", "Read", json!({"file_path": "src/main.rs"}));
        if !turn.pause(1000).await {
            return out_of_time(turn);
        }
        turn.tool_done("call-3", "fn main() {}");
        turn.tool("call-4", "Grep", json!({"pattern": "parse_args"}));
        if !turn.pause(500).await {
            return out_of_time(turn);
        }
        turn.tool_done("call-4", "src/main.rs:1");
        turn.tool(
            "call-5",
            "Bash",
            json!({"command": "cargo test -p parser", "description": "Run the parser's tests"}),
        );
        if !turn.pause(secs * 1000).await {
            return out_of_time(turn);
        }
        turn.tool_done("call-5", "test result: ok");
    }
    let turns = TURNS.fetch_add(1, Ordering::SeqCst) + 1;
    let used = if text.contains("context full") {
        190_000
    } else if text.contains("context compacted") {
        8_000
    } else {
        10_000 * turns
    };
    let reply = format!("echo: {text}");
    let id = turn.next_message();
    turn.line(
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": id}}}),
    );
    turn.line(json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": reply}}}));
    turn.assistant(id, vec![json!({"type": "text", "text": reply})], Some(used));
    if text.contains("then a tool") {
        turn.tool("call-6", "Read", json!({"file_path": "src/last.rs"}));
        turn.tool_done("call-6", "fn last() {}");
    }
}

/// Stream a recorded conversation: the `text`, `tool_call` and `tool_update` events of a
/// `wheel agent read --json` dump from its first turn on, each after the gap it had when
/// recorded, up to `secs` seconds after that turn started. False when interrupted.
async fn replay(turn: &mut Turn, dump: &Value, secs: f64) -> bool {
    let events = dump["events"].as_array().cloned().unwrap_or_default();
    let start = events
        .iter()
        .find(|e| e["kind"] == "turn_start")
        .and_then(|e| e["at"].as_f64())
        .unwrap_or(0.0);
    let mut now = start;
    for e in events
        .iter()
        .filter(|e| e["at"].as_f64().is_some_and(|at| at >= start))
    {
        let at = e["at"].as_f64().unwrap_or(now);
        if (at - start) / 1000.0 > secs {
            break;
        }
        if !turn.pause((at - now).max(0.0) as u64).await {
            return false;
        }
        now = at;
        let id = e["id"].as_str().unwrap_or_default();
        let title = e["title"].as_str().unwrap_or_default();
        match e["kind"].as_str() {
            Some("text") => turn.say(e["text"].as_str().unwrap_or_default()),
            Some("tool_call") => {
                let (name, input) = match e["tool_kind"].as_str() {
                    Some("execute") => ("Bash", json!({"command": title})),
                    Some("read") => (
                        "Read",
                        json!({"file_path": title.strip_prefix("Read ").unwrap_or(title)}),
                    ),
                    Some("edit") => {
                        let diff = e["content"]
                            .as_array()
                            .and_then(|c| c.iter().find(|c| c["type"] == "diff"))
                            .cloned()
                            .unwrap_or_default();
                        (
                            "Edit",
                            json!({"file_path": title.strip_prefix("Edit ").unwrap_or(title), "old_string": diff["old_text"], "new_string": diff["new_text"]}),
                        )
                    }
                    Some("search") => ("Grep", json!({"pattern": title})),
                    _ => (title, json!({})),
                };
                turn.tool(id, name, input);
            }
            Some("tool_update") if matches!(e["status"].as_str(), Some("completed" | "failed")) => {
                let text: Vec<&str> = e["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect();
                turn.tool_done(id, &text.join("\n"));
            }
            _ => {}
        }
    }
    true
}

#[tokio::main]
async fn main() {
    let all: Vec<String> = std::env::args().collect();
    let invoked = all.first().cloned().unwrap_or_default();
    let name = std::path::Path::new(&invoked)
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from);
    let named_claude = name.as_deref() == Some("claude");
    if all.iter().any(|a| a == "app-server") {
        codex::main().await;
        return;
    }
    if name.as_deref() == Some("codex") {
        std::process::exit(i32::from(codex::tui().is_err()));
    }
    if !all.iter().any(|a| a == "--print") {
        if named_claude {
            std::process::exit(i32::from(claude_tui().is_err()));
        }
        eprintln!("usage: wheel-fake-agent [--replay <dump.json> [--replay-secs <n>]] --print … (or run as `claude` for the TUI stand-in)");
        std::process::exit(2);
    }
    let mut args = Args {
        replay: None,
        replay_secs: f64::INFINITY,
        session: String::new(),
        resume: false,
        brief: None,
        allowed: None,
        disallowed: None,
    };
    let mut words = all.into_iter().skip(1);
    while let Some(word) = words.next() {
        let value = if VALUED.contains(&word.as_str()) || word.starts_with("--replay") {
            words.next()
        } else {
            None
        };
        match (word.as_str(), value) {
            ("--replay", Some(path)) => {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                args.replay = serde_json::from_str(&text).ok();
            }
            ("--replay-secs", Some(n)) => args.replay_secs = n.parse().unwrap_or(f64::INFINITY),
            ("--session-id", Some(id)) => args.session = id,
            ("--resume", Some(id)) => {
                args.session = id;
                args.resume = true;
            }
            ("--append-system-prompt", Some(text)) => args.brief = Some(text),
            ("--allowedTools", Some(tools)) => args.allowed = Some(tools),
            ("--disallowedTools", Some(tools)) => args.disallowed = Some(tools),
            _ => {}
        }
    }
    if args.resume && !transcript(&args.session).is_file() {
        eprintln!("No conversation found with session ID: {}", args.session);
        std::process::exit(1);
    }
    if args.session.is_empty() {
        args.session = made_up_id();
    }
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let state = Arc::new(State {
        mode: Mutex::new("default".into()),
        model: Mutex::new("default".into()),
        answers: Mutex::new(HashMap::new()),
        interrupted: AtomicBool::new(false),
        interrupt: Notify::new(),
    });
    let (prompts, mut prompts_rx) = mpsc::unbounded_channel::<String>();
    let reader_out = out.clone();
    let reader_state = state.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match message["type"].as_str() {
                Some("user") => {
                    let content = &message["message"]["content"];
                    let text = match content {
                        Value::String(text) => text.clone(),
                        _ => content
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|b| b["text"].as_str())
                            .collect(),
                    };
                    let _ = prompts.send(text);
                }
                Some("control_request") => {
                    control(&reader_out, &reader_state, &message);
                }
                Some("control_response") => {
                    let response = &message["response"];
                    let id = response["request_id"].as_str().unwrap_or_default();
                    let answer = &response["response"];
                    let mut behavior = answer["behavior"].as_str().unwrap_or("deny").to_string();
                    if answer["updatedPermissions"].is_array() {
                        behavior.push_str(" always");
                    }
                    if let Some(tx) = reader_state.answers.lock().unwrap().remove(id) {
                        let _ = tx.send(behavior);
                    }
                }
                _ => {}
            }
        }
        std::process::exit(0);
    });
    let args = Arc::new(args);
    while let Some(text) = prompts_rx.recv().await {
        state.interrupted.store(false, Ordering::SeqCst);
        let mut turn = Turn {
            out: out.clone(),
            state: state.clone(),
            session: args.session.clone(),
            message: 0,
        };
        out.send(json!({"type": "system", "subtype": "init", "cwd": std::env::current_dir().unwrap_or_default(), "session_id": args.session, "model": state.resolved_model(), "permissionMode": *state.mode.lock().unwrap()}));
        keep(&args.session, user_entry(json!(text)));
        answer(&mut turn, &args, &text).await;
        keep(&args.session, turn_over());
        out.send(json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "stop_reason": "end_turn",
            "session_id": args.session,
            "result": "",
            "modelUsage": {state.resolved_model(): {"contextWindow": WINDOW}},
        }));
    }
}

/// Answer one of the host's control requests.
fn control(out: &Out, state: &State, message: &Value) {
    let id = message["request_id"].clone();
    let request = &message["request"];
    let success = |response: Value| {
        out.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}}));
    };
    let refuse = |error: String| {
        out.send(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": error}}));
    };
    match request["subtype"].as_str() {
        Some("initialize") => {
            let models: Vec<Value> = MODELS
                .iter()
                .map(|(value, resolved, name)| json!({"value": value, "resolvedModel": resolved, "displayName": name, "description": format!("{name} model")}))
                .collect();
            success(
                json!({"models": models, "current_permission_mode": *state.mode.lock().unwrap(), "claude_code_version": "fake"}),
            );
        }
        Some("set_permission_mode") => {
            let mode = request["mode"].as_str().unwrap_or_default();
            if MODES.contains(&mode) {
                *state.mode.lock().unwrap() = mode.into();
                success(json!({"mode": mode}));
            } else {
                refuse(format!("mode does not take {mode}; it takes {MODES:?}"));
            }
        }
        Some("set_model") => {
            let model = request["model"].as_str().unwrap_or_default();
            if MODELS.iter().any(|(v, _, _)| *v == model) {
                *state.model.lock().unwrap() = model.into();
                success(json!({}));
            } else {
                refuse(format!("no model {model}"));
            }
        }
        Some("interrupt") => {
            state.interrupted.store(true, Ordering::SeqCst);
            state.interrupt.notify_waiters();
            success(json!({"still_queued": []}));
        }
        other => refuse(format!("unsupported control request {other:?}")),
    }
}
