//! Bringing a terminal pane back after a restart: the directory its shell was in, and the coding
//! agent it was running, resumed by that agent's own command. The daemon samples the process in
//! each terminal's foreground, saves what it finds in the pane, and on restore opens the shell
//! there and types the resume command into it.
//!
//! An agent is recognised by the program in the terminal's foreground (Claude Code or Codex), and
//! its session id comes from the program's own arguments (`--resume <id>`) or, for a session
//! started fresh, from the newest transcript the agent has written for that directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wheel_proto::{ContextUse, TermAgent};

/// The coding agents whose sessions a terminal pane resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The program's name as a person says it.
    pub fn title(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    pub fn parse(name: &str) -> Option<Agent> {
        match name {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }

    /// The program and arguments that resume `session`.
    pub fn resume_argv(self, session: &str) -> Vec<String> {
        match self {
            Agent::Claude => vec!["claude".into(), "--resume".into(), session.into()],
            Agent::Codex => vec!["codex".into(), "resume".into(), session.into()],
        }
    }

    /// The environment variable that moves the agent's data directory, and where that is by default.
    fn home_var(self) -> &'static str {
        match self {
            Agent::Claude => "CLAUDE_CONFIG_DIR",
            Agent::Codex => "CODEX_HOME",
        }
    }

    pub fn default_home(self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(match self {
            Agent::Claude => home.join(".claude"),
            Agent::Codex => home.join(".codex"),
        })
    }

    /// The data directory the daemon itself would use for this agent.
    pub fn daemon_home(self) -> Option<PathBuf> {
        std::env::var_os(self.home_var())
            .map(PathBuf::from)
            .or_else(|| self.default_home())
    }
}

/// A process's arguments and its environment as name and value.
type ProcessArgs = (Vec<String>, Vec<(String, String)>);

/// What a sample of one terminal found.
#[derive(Debug, Default, PartialEq)]
pub struct Sample {
    pub cwd: Option<String>,
    pub agent: Option<TermAgent>,
}

/// The agent a command line starts, and the session id it names, if it names one.
pub fn detect(argv: &[String]) -> Option<(Agent, Option<String>)> {
    let base = |arg: &String| {
        Path::new(arg)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    };
    let first = base(argv.first()?)?;
    // A script run by an interpreter: `node /…/bin/claude`.
    let program = if matches!(first.as_str(), "node" | "bun" | "deno") {
        argv.iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .and_then(base)?
    } else {
        first
    };
    let agent = Agent::parse(&program)?;
    let mut session = None;
    let mut args = argv
        .iter()
        .skip_while(|a| base(a).as_deref() != Some(&program))
        .skip(1);
    while let Some(arg) = args.next() {
        let value = match (agent, arg.as_str()) {
            (Agent::Claude, "--resume" | "-r" | "--session-id") => args.next().cloned(),
            (Agent::Claude, a) if a.starts_with("--resume=") => {
                a.strip_prefix("--resume=").map(String::from)
            }
            (Agent::Claude, a) if a.starts_with("--session-id=") => {
                a.strip_prefix("--session-id=").map(String::from)
            }
            (Agent::Codex, "resume") => args.next().cloned(),
            _ => None,
        };
        if let Some(value) = value.filter(|v| looks_like_session_id(v)) {
            session = Some(value);
        }
    }
    Some((agent, session))
}

/// A session id is a UUID; anything else after `--resume` is a search term or a flag.
fn looks_like_session_id(value: &str) -> bool {
    value.len() == 36 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// How Claude Code names a project's directory under `projects/`: every UTF-16 code unit of the
/// path that is not a letter or digit becomes `-`.
pub(crate) fn claude_project_dir(home: &Path, cwd: &str) -> PathBuf {
    let name: String = cwd
        .chars()
        .flat_map(|c| {
            let units = if c.is_ascii_alphanumeric() {
                0
            } else {
                c.len_utf16()
            };
            std::iter::repeat_n('-', units).chain(c.is_ascii_alphanumeric().then_some(c))
        })
        .collect();
    home.join("projects").join(name)
}

/// A session id and when its transcript was last written.
type SessionWritten = (String, std::time::SystemTime);

/// The sessions Claude Code has for `cwd`, the one written to last first.
fn claude_sessions(home: &Path, cwd: &str) -> Vec<SessionWritten> {
    let dir = claude_project_dir(home, cwd);
    let mut found: Vec<SessionWritten> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if looks_like_session_id(id) {
            found.push((id.to_string(), modified));
        }
    }
    found.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    found
}

/// The sessions Codex started in `cwd` over its last three days of transcripts, the newest
/// first. Codex files its transcripts by date and names the directory in the first line of each.
fn codex_sessions(home: &Path, cwd: &str) -> Vec<SessionWritten> {
    let newest_first = |dir: &Path| {
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        names.sort();
        names.reverse();
        names
    };
    let mut found = Vec::new();
    let mut days = 0;
    for year in newest_first(&home.join("sessions")) {
        for month in newest_first(&year) {
            for day in newest_first(&month) {
                days += 1;
                found.extend(
                    newest_first(&day)
                        .iter()
                        .filter_map(|file| codex_session_in(file, cwd)),
                );
                if days >= 3 {
                    return found;
                }
            }
        }
    }
    found
}

/// Which of `sessions` a running agent is in when its command line does not say.
///
/// Only a session written since the agent's process `started` can be its own: one last written
/// before that is an earlier run in the same directory. Of those, the one its pane already saved,
/// so two agents in one directory keep one session each, else the one written last that no other
/// pane has saved. An agent that has written nothing yet (a fresh one, or one at its resume
/// picker) keeps the session its pane saved, so a restart still resumes it; with no turn written,
/// it has no turn a switch could cut short.
fn choose_session(
    sessions: &[SessionWritten],
    started: std::time::SystemTime,
    previous: Option<&str>,
    claimed: &HashSet<String>,
) -> Option<String> {
    let mut candidates: Vec<&SessionWritten> = sessions
        .iter()
        .filter(|(_, written)| *written >= started)
        .collect();
    candidates.sort_by_key(|(_, written)| std::cmp::Reverse(*written));
    let ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
    let exists = |p: &&str| sessions.iter().any(|(id, _)| id == p);
    let chosen = if ids.is_empty() {
        previous.filter(exists)
    } else {
        previous
            .filter(|p| ids.contains(p))
            .or_else(|| ids.iter().copied().find(|s| !claimed.contains(*s)))
    };
    chosen.map(String::from)
}

fn codex_session_in(file: &Path, cwd: &str) -> Option<SessionWritten> {
    use std::io::BufRead;
    let first = std::io::BufReader::new(std::fs::File::open(file).ok()?)
        .lines()
        .next()?
        .ok()?;
    let meta: serde_json::Value = serde_json::from_str(&first).ok()?;
    let payload = meta.get("payload")?;
    if payload.get("cwd")?.as_str()? != cwd {
        return None;
    }
    let written = std::fs::metadata(file).and_then(|m| m.modified()).ok()?;
    Some((payload.get("id")?.as_str()?.to_string(), written))
}

/// Whether the agent still holds `agent`'s saved session: a resume of a session it has lost would
/// only print an error.
pub fn session_exists(agent: &TermAgent, cwd: &str) -> bool {
    let Some(kind) = Agent::parse(&agent.name) else {
        return false;
    };
    let Some(home) = agent
        .home
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| kind.daemon_home())
    else {
        return false;
    };
    match kind {
        Agent::Claude => claude_project_dir(&home, cwd)
            .join(format!("{}.jsonl", agent.session))
            .is_file(),
        Agent::Codex => walk_for(&home.join("sessions"), &agent.session),
    }
}

/// Whether a file under `dir` carries `session` in its name (Codex names a transcript
/// `rollout-<time>-<session>.jsonl`).
fn walk_for(dir: &Path, session: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let path = e.path();
            if path.is_dir() {
                walk_for(&path, session)
            } else {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(session))
            }
        })
}

/// The line typed into a restored shell to resume `agent`. The agent's data directory is named
/// when the session lives somewhere other than the default, so the agent finds it.
pub fn resume_line(agent: &TermAgent) -> Option<String> {
    line(agent)
}

/// The line typed into a new shell to take over `agent`'s session from wheel's agent view. The
/// agent must run with the environment the agent view's process had, because Claude Code keeps its
/// login in a Keychain item named after `CLAUDE_CONFIG_DIR`: a different value is a different
/// login. `agent.home` is the directory the agent view was given, if any; otherwise the agent
/// view inherited the daemon's own `CLAUDE_CONFIG_DIR`, and the person's shell setup must not
/// replace it, so the line names the daemon's value, or removes the variable when it has none.
pub fn switch_line(agent: &TermAgent) -> Option<String> {
    let kind = Agent::parse(&agent.name)?;
    let words = kind
        .resume_argv(&agent.session)
        .iter()
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ");
    let var = kind.home_var();
    let home = agent
        .home
        .clone()
        .or_else(|| std::env::var_os(var).map(|v| v.to_string_lossy().into_owned()));
    Some(match home {
        Some(home) => format!("{var}={} {words}", shell_quote(&home)),
        None => format!("env -u {var} {words}"),
    })
}

fn line(agent: &TermAgent) -> Option<String> {
    let kind = Agent::parse(&agent.name)?;
    let words = kind.resume_argv(&agent.session);
    let mut line = words
        .iter()
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(home) = agent
        .home
        .as_ref()
        .filter(|h| Some(PathBuf::from(h)) != kind.default_home())
    {
        line = format!("{}={} {line}", kind.home_var(), shell_quote(home));
    }
    Some(line)
}

/// How long after its last write a transcript whose final reply has no `turn_duration` is still
/// read as mid-turn: the time Claude Code's stop hooks get to finish and write it.
const STOP_HOOK_GRACE: Duration = Duration::from_secs(10);

/// How much of the end of a Claude Code transcript is read to find where its last turn stands.
const TRANSCRIPT_TAIL: u64 = 512 * 1024;

/// Whether Claude Code is in the middle of a turn in `session`, read from the end of its
/// transcript under `home` for `cwd`. A turn is open from a prompt until the reply that ends it:
/// Claude Code writes a `turn_duration` entry when a turn is over, and its last assistant entry
/// of a finished turn has a stop reason other than `tool_use`. Claude Code writes that reply
/// before its stop hooks run, which can still continue the turn, so a reply with no
/// `turn_duration` after it counts as open for `STOP_HOOK_GRACE` after the file's last write; some
/// Claude Code versions never write `turn_duration`. A prompt that only ran a local command, an
/// interrupted turn, and a transcript with no turn at all are between turns.
pub fn claude_turn_open(home: &Path, cwd: &str, session: &str) -> bool {
    let path = claude_project_dir(home, cwd).join(format!("{session}.jsonl"));
    let Some((entries, written_since)) = transcript_tail(&path, TRANSCRIPT_TAIL) else {
        return false;
    };
    let mut open = false;
    // The last entry that mattered is a reply that ends a turn, with no `turn_duration` after it.
    let mut replied = false;
    for entry in entries {
        match entry["type"].as_str() {
            Some("system") if entry["subtype"] == "turn_duration" => {
                open = false;
                replied = false;
            }
            Some("assistant") => {
                open = matches!(
                    entry["message"]["stop_reason"].as_str(),
                    None | Some("tool_use")
                );
                replied = !open;
            }
            Some("user") if entry["isMeta"] != true => {
                open = !between_turns(&entry["message"]);
                replied = false;
            }
            _ => {}
        }
    }
    open || (replied && written_since.is_some_and(|since| since < STOP_HOOK_GRACE))
}

/// The entries in the last `window` bytes of a JSON-lines transcript, and how long ago the file
/// was last written. A window that starts mid-file starts mid-line; that first piece is dropped.
fn transcript_tail(path: &Path, window: u64) -> Option<(Vec<serde_json::Value>, Option<Duration>)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let start = meta.len().saturating_sub(window);
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(start)).ok()?;
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let entries = text
        .lines()
        .skip(usize::from(start > 0))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let written_since = meta.modified().ok().and_then(|t| t.elapsed().ok());
    Some((entries, written_since))
}

/// Whether `agent` is in the middle of a turn in `session`, the one rule both agents answer to a
/// switch between a terminal and a chat: a turn is open from its start until the entry that ends
/// it, read from the end of the agent's own transcript. `home` is the agent's data directory and
/// `cwd` the directory it runs in.
pub fn turn_open(agent: Agent, home: &Path, cwd: &str, session: &str) -> bool {
    match agent {
        Agent::Claude => claude_turn_open(home, cwd, session),
        Agent::Codex => codex_turn_open(home, session),
    }
}

/// The larger windows read when a Codex rollout's last turn marker is further back than the
/// first one: a turn that ran many tools writes far more than `TRANSCRIPT_TAIL` after its start.
const ROLLOUT_WINDOWS: [u64; 3] = [TRANSCRIPT_TAIL, 16 * 1024 * 1024, u64::MAX];

/// Whether Codex is in the middle of a turn in `session`: the last `task_started`,
/// `task_complete` or `turn_aborted` event in its rollout is a `task_started`. The file's age says
/// nothing: a turn waiting on an approval prompt or a long command writes nothing for minutes.
/// Whether the program that wrote the rollout still runs is the caller's to know; a switch reads
/// this only for the agent it just found in a terminal's foreground. No rollout, and a rollout
/// with no turn, are between turns.
pub fn codex_turn_open(home: &Path, session: &str) -> bool {
    let Some(path) = codex_rollout(&home.join("sessions"), session) else {
        return false;
    };
    for window in ROLLOUT_WINDOWS {
        let Some((entries, _)) = transcript_tail(&path, window) else {
            return false;
        };
        let last = entries.iter().rev().find_map(|entry| {
            let kind = entry["payload"]["type"].as_str()?;
            (entry["type"] == "event_msg")
                .then_some(kind)
                .filter(|k| matches!(*k, "task_started" | "task_complete" | "turn_aborted"))
        });
        if let Some(kind) = last {
            return kind == "task_started";
        }
    }
    false
}

/// The rollout file of Codex `session` under `sessions`, newest day first: Codex names it
/// `rollout-<time>-<session>.jsonl` inside a year, month and day directory.
fn codex_rollout(sessions: &Path, session: &str) -> Option<PathBuf> {
    let newest_first = |dir: &Path| {
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        names.sort();
        names.reverse();
        names
    };
    let suffix = format!("-{session}.jsonl");
    for year in newest_first(sessions) {
        for month in newest_first(&year) {
            for day in newest_first(&month) {
                let found = newest_first(&day).into_iter().find(|file| {
                    file.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                });
                if found.is_some() {
                    return found;
                }
            }
        }
    }
    None
}

/// The context window Claude Code gives a session by default, in tokens.
const CLAUDE_WINDOW: u64 = 200_000;
/// The larger window a session runs with when its model attachment names the 1M context.
const CLAUDE_LARGE_WINDOW: u64 = 1_000_000;

/// The window a `model` attachment entry announces: `CLAUDE_LARGE_WINDOW` when its model id ends
/// in `[1m]` or its marketing name says "1M context", `CLAUDE_WINDOW` otherwise. None for any other
/// entry.
const MODEL_MARK: &[u8] = b"\"type\":\"model\"";

fn attachment_window(entry: &serde_json::Value) -> Option<u64> {
    let attachment = &entry["attachment"];
    if entry["type"] != "attachment"
        || entry["isSidechain"] == true
        || attachment["type"] != "model"
    {
        return None;
    }
    let identity = &attachment["identity"];
    let large = identity["modelId"]
        .as_str()
        .is_some_and(|m| m.ends_with("[1m]"))
        || identity["marketingName"]
            .as_str()
            .is_some_and(|m| m.contains("1M context"));
    Some(if large {
        CLAUDE_LARGE_WINDOW
    } else {
        CLAUDE_WINDOW
    })
}

/// How full `session`'s context window is, read from the usage block of the last assistant
/// entry in its transcript under `home` for `cwd`: the input, cache and output tokens of that
/// request are the conversation as the model last saw it. Claude Code writes no `[1m]` suffix on an
/// assistant entry's model; the window comes from the `model` attachment entry it writes near the
/// transcript's start (and again when the model changes), read from the first and last
/// `TRANSCRIPT_TAIL` bytes only: a model change in the unread middle is not seen. It is
/// `CLAUDE_WINDOW` when no such entry exists. None when the transcript, or an assistant entry with
/// usage in its tail, is missing, and when the conversation outgrows the window the transcript
/// names: the daemon does not guess a window it was not told.
pub fn claude_context(home: &Path, cwd: &str, session: &str) -> Option<ContextUse> {
    use std::io::{Read, Seek, SeekFrom};
    let path = claude_project_dir(home, cwd).join(format!("{session}.jsonl"));
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL);
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(start)).ok()?;
    file.read_to_end(&mut tail).ok()?;
    let mut head = Vec::new();
    if start > 0 {
        file.seek(SeekFrom::Start(0)).ok()?;
        (&mut file)
            .take(TRANSCRIPT_TAIL)
            .read_to_end(&mut head)
            .ok()?;
    }
    let mut window = head
        .split(|b| *b == b'\n')
        .filter(|line| line.windows(MODEL_MARK.len()).any(|w| w == MODEL_MARK))
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .filter_map(|entry| attachment_window(&entry))
        .next_back();
    let text = String::from_utf8_lossy(&tail);
    let lines = text.lines().skip(usize::from(start > 0));
    let mut found: Option<u64> = None;
    for line in lines {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(size) = attachment_window(&entry) {
            window = Some(size);
            continue;
        }
        if entry["type"] != "assistant" || entry["isSidechain"] == true {
            continue;
        }
        let message = &entry["message"];
        if message["model"] == "<synthetic>" {
            continue;
        }
        let usage = &message["usage"];
        if !usage.is_object() {
            continue;
        }
        let part = |key: &str| usage[key].as_u64().unwrap_or(0);
        let used = part("input_tokens")
            + part("cache_creation_input_tokens")
            + part("cache_read_input_tokens")
            + part("output_tokens");
        found = Some(used);
    }
    let used = found?;
    let size = window.unwrap_or(CLAUDE_WINDOW);
    (used <= size).then_some(ContextUse { used, size })
}

/// Whether a user entry leaves Claude Code between turns: a local command and its output, or the
/// note an interrupted turn ends with.
pub(crate) fn between_turns(message: &serde_json::Value) -> bool {
    let text = match &message["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let text = text.trim_start();
    text.starts_with("[Request interrupted by user")
        || text.starts_with("<command-name>")
        || text.starts_with("<local-command-")
}

/// Ask the program in a terminal's foreground to quit: SIGTERM to its process group, as a
/// terminal's own signals reach the whole foreground job.
pub fn stop_foreground(group: u32) -> Result<(), String> {
    let rc = unsafe { libc::killpg(group as libc::pid_t, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!(
            "could not signal process group {group}: {}",
            std::io::Error::last_os_error()
        ))
    }
}

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// What is running in a terminal whose shell is `shell` and whose foreground process is
/// `foreground`. `previous` is the agent the pane saved last time, and `claimed` the sessions
/// other panes have saved.
pub fn sample(
    shell: Option<u32>,
    foreground: Option<u32>,
    previous: Option<&TermAgent>,
    claimed: &HashSet<String>,
) -> Sample {
    let mut sample = Sample::default();
    if let Some(fg) = foreground {
        if let Some((args, env)) = process_args(fg) {
            if let Some((kind, named)) = detect(&args) {
                let cwd = process_cwd(fg);
                let home = env
                    .iter()
                    .find(|(k, _)| k == kind.home_var())
                    .map(|(_, v)| PathBuf::from(v));
                let search = home.clone().or_else(|| kind.daemon_home());
                let session = named.or_else(|| {
                    let (home, cwd) = (search.as_ref()?, cwd.as_deref()?);
                    let sessions = match kind {
                        Agent::Claude => claude_sessions(home, cwd),
                        Agent::Codex => codex_sessions(home, cwd),
                    };
                    let previous = previous
                        .filter(|p| p.name == kind.name())
                        .map(|p| p.session.as_str());
                    let Some(started) = process_started(fg) else {
                        // Once per process: the sampler asks every 2 s.
                        static WARNED: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);
                        if WARNED.lock().unwrap().replace(fg) != Some(fg) {
                            tracing::warn!(pid = fg, agent = kind.name(), "cannot read when the agent's process started; not guessing its session");
                        }
                        return None;
                    };
                    choose_session(&sessions, started, previous, claimed)
                });
                if let Some(session) = session {
                    let context = match (kind, search.as_ref(), cwd.as_deref()) {
                        (Agent::Claude, Some(home), Some(cwd)) => {
                            claude_context(home, cwd, &session)
                        }
                        _ => None,
                    };
                    sample.agent = Some(TermAgent {
                        name: kind.name().into(),
                        session,
                        home: home.map(|h| h.display().to_string()),
                        context,
                    });
                    sample.cwd = cwd;
                }
            }
        }
    }
    if sample.cwd.is_none() {
        sample.cwd = shell.and_then(process_cwd);
    }
    sample
}

/// When process `pid` started.
#[cfg(target_os = "macos")]
fn process_started(pid: u32) -> Option<std::time::SystemTime> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`, which this frame owns.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (got == size).then(|| {
        std::time::UNIX_EPOCH
            + std::time::Duration::new(info.pbi_start_tvsec, info.pbi_start_tvusec as u32 * 1000)
    })
}

/// When process `pid` started: its `starttime` (field 22 of `/proc/<pid>/stat`, in clock ticks
/// since boot) after the boot time `btime` in `/proc/stat`.
#[cfg(not(target_os = "macos"))]
fn process_started(pid: u32) -> Option<std::time::SystemTime> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces; the fields after it are plain.
    let after = &stat[stat.rfind(')')? + 2..];
    let ticks: u64 = after.split_whitespace().nth(19)?.parse().ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // SAFETY: sysconf reads a constant and touches no memory of ours.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = u64::try_from(hz).ok().filter(|h| *h > 0)?;
    Some(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(boot)
            + std::time::Duration::from_millis(ticks * 1000 / hz),
    )
}

#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<String> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if got != size {
        return None;
    }
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            info.pvi_cdir.vip_path.as_ptr() as *const u8,
            std::mem::size_of_val(&info.pvi_cdir.vip_path),
        )
    };
    let end = bytes.iter().position(|b| *b == 0)?;
    let path = String::from_utf8_lossy(&bytes[..end]).into_owned();
    (!path.is_empty()).then_some(path)
}

#[cfg(not(target_os = "macos"))]
pub fn process_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.display().to_string())
}

/// A process's arguments and environment.
#[cfg(target_os = "macos")]
fn process_args(pid: u32) -> Option<ProcessArgs> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

#[cfg(not(target_os = "macos"))]
fn process_args(pid: u32) -> Option<ProcessArgs> {
    let args = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args = args
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let env = env
        .split(|b| *b == 0)
        .filter_map(|e| {
            let e = String::from_utf8_lossy(e);
            let (k, v) = e.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();
    Some((args, env))
}

/// The `KERN_PROCARGS2` layout: `argc`, the executable path, padding NULs, `argc` arguments, then
/// the environment.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_procargs(buf: &[u8]) -> Option<ProcessArgs> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let mut strings = buf.get(4..)?.split(|b| *b == 0);
    // The executable path, then the NULs that pad it.
    strings.next()?;
    let mut rest = strings.skip_while(|s| s.is_empty());
    let text = |s: &[u8]| String::from_utf8_lossy(s).into_owned();
    let args: Vec<String> = rest.by_ref().take(argc).map(text).collect();
    let env = rest
        .filter_map(|s| {
            let s = text(s);
            let (k, v) = s.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();
    (args.len() == argc).then_some((args, env))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_command_line_names_its_agent_and_session() {
        assert_eq!(
            detect(&argv(&["claude", "--resume", ID])),
            Some((Agent::Claude, Some(ID.into())))
        );
        assert_eq!(
            detect(&argv(&["/opt/bin/claude", "--model", "x"])),
            Some((Agent::Claude, None))
        );
        assert_eq!(
            detect(&argv(&["node", "/usr/lib/bin/codex", "resume", ID])),
            Some((Agent::Codex, Some(ID.into())))
        );
        assert_eq!(
            detect(&argv(&["claude", "--resume", "a search"])),
            Some((Agent::Claude, None))
        );
        assert_eq!(detect(&argv(&["vim", "claude"])), None);
        assert_eq!(detect(&argv(&["-zsh"])), None);
    }

    #[test]
    fn resume_commands_name_the_saved_session() {
        let agent = |name: &str, home: Option<&str>| TermAgent {
            name: name.into(),
            session: ID.into(),
            home: home.map(String::from),
            context: None,
        };
        assert_eq!(
            resume_line(&agent("claude", None)).unwrap(),
            format!("claude --resume {ID}")
        );
        assert_eq!(
            resume_line(&agent("codex", None)).unwrap(),
            format!("codex resume {ID}")
        );
        assert_eq!(
            resume_line(&agent("claude", Some("/data/my claude"))).unwrap(),
            format!("CLAUDE_CONFIG_DIR='/data/my claude' claude --resume {ID}")
        );
        assert_eq!(resume_line(&agent("vim", None)), None);
        let default = Agent::Claude.default_home().unwrap().display().to_string();
        assert_eq!(
            resume_line(&agent("claude", Some(&default))).unwrap(),
            format!("claude --resume {ID}")
        );
        assert_eq!(
            switch_line(&agent("claude", Some(&default))).unwrap(),
            format!("CLAUDE_CONFIG_DIR={default} claude --resume {ID}")
        );
        assert_eq!(
            switch_line(&agent("claude", Some("/data/my claude"))).unwrap(),
            format!("CLAUDE_CONFIG_DIR='/data/my claude' claude --resume {ID}")
        );
        if std::env::var_os("CLAUDE_CONFIG_DIR").is_none() {
            assert_eq!(
                switch_line(&agent("claude", None)).unwrap(),
                format!("env -u CLAUDE_CONFIG_DIR claude --resume {ID}")
            );
        }
    }

    #[test]
    fn a_transcript_says_whether_its_last_turn_is_still_running() {
        let home = std::env::temp_dir().join(format!("wheel-resume-turn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let project = claude_project_dir(&home, "/tmp/turns");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join(format!("{ID}.jsonl"));
        let write = |lines: &[&str], age: Duration| {
            std::fs::write(&file, lines.join("\n")).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - age)
                .unwrap();
        };
        let open = |lines: &[&str]| {
            write(lines, Duration::from_secs(60));
            claude_turn_open(&home, "/tmp/turns", ID)
        };
        let prompt = r#"{"type":"user","message":{"role":"user","content":"fix the bug"}}"#;
        let tool_use = r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use"}]}}"#;
        let tool_result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#;
        let streaming =
            r#"{"type":"assistant","message":{"stop_reason":null,"content":[{"type":"text"}]}}"#;
        let reply = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text"}]}}"#;
        let done = r#"{"type":"system","subtype":"turn_duration"}"#;
        let meta = r#"{"type":"user","isMeta":true,"message":{"content":"caveat"}}"#;
        let command =
            r#"{"type":"user","message":{"content":"<command-name>/model</command-name>"}}"#;
        let interrupted = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        assert!(!open(&[]), "no turn yet");
        assert!(open(&[prompt]));
        assert!(open(&[prompt, tool_use]));
        assert!(open(&[prompt, tool_use, tool_result]));
        assert!(open(&[prompt, streaming]));
        assert!(!open(&[prompt, tool_use, tool_result, reply]));
        assert!(!open(&[prompt, streaming, done]));
        assert!(!open(&[prompt, reply, done, meta, command]));
        assert!(!open(&[prompt, tool_use, interrupted]));
        assert!(open(&[prompt, reply, done, prompt]));
        // The reply is written before the stop hooks run; `turn_duration` follows them.
        write(
            &[prompt, tool_use, tool_result, reply],
            Duration::from_secs(1),
        );
        assert!(
            claude_turn_open(&home, "/tmp/turns", ID),
            "stop hooks still running"
        );
        write(&[prompt, reply, done], Duration::from_secs(1));
        assert!(
            !claude_turn_open(&home, "/tmp/turns", ID),
            "turn_duration ends it at once"
        );
        assert!(!claude_turn_open(&home, "/tmp/turns", "no-such-session"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_rollout_says_whether_codexs_last_turn_is_still_running() {
        let home =
            std::env::temp_dir().join(format!("wheel-resume-rollout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/10/21");
        std::fs::create_dir_all(&day).unwrap();
        let file = day.join(format!("rollout-2026-10-21T13-12-46-{ID}.jsonl"));
        let open = |lines: &[&str]| {
            std::fs::write(&file, lines.join("\n")).unwrap();
            codex_turn_open(&home, ID)
        };
        let event = |kind: &str| format!(r#"{{"type":"event_msg","payload":{{"type":"{kind}"}}}}"#);
        let (started, complete, aborted) = (
            event("task_started"),
            event("task_complete"),
            event("turn_aborted"),
        );
        let noise = r#"{"type":"response_item","payload":{"type":"message"}}"#;
        let count = event("token_count");
        assert!(!open(&[]), "no turn yet");
        assert!(open(&[&started]));
        assert!(open(&[&started, noise, &count]));
        assert!(!open(&[&started, noise, &complete, &count]));
        assert!(!open(&[&started, &aborted]));
        assert!(open(&[&started, &complete, &started, noise]));
        // A turn whose output outgrows the first window is still found.
        let big = format!(
            r#"{{"type":"response_item","payload":{{"type":"message","text":"{}"}}}}"#,
            "x".repeat(TRANSCRIPT_TAIL as usize + 1024)
        );
        assert!(open(&[&complete, &started, &big]));
        assert!(!open(&[&started, &complete, &big]));
        assert!(!codex_turn_open(&home, "no-such-session"));
        // A turn quiet for an hour is still open: the running process decides, not the file's age.
        std::fs::write(&file, started.as_bytes()).unwrap();
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert!(codex_turn_open(&home, ID), "a quiet turn is still open");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_process_start_time_is_read_from_the_system() {
        let started = process_started(std::process::id()).expect("this process has a start time");
        let age = started.elapsed().expect("it started in the past");
        assert!(age < std::time::Duration::from_secs(3600), "{age:?}");
    }

    #[test]
    fn the_newest_claude_transcript_for_a_directory_is_its_session() {
        let home = std::env::temp_dir().join(format!("wheel-resume-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let project = claude_project_dir(&home, "/tmp/my.proj");
        assert_eq!(project, home.join("projects/-tmp-my-proj"));
        std::fs::create_dir_all(&project).unwrap();
        let older = "aaaaaaaa-2222-3333-4444-555555555555";
        std::fs::write(project.join(format!("{older}.jsonl")), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(project.join(format!("{ID}.jsonl")), "{}").unwrap();
        let sessions = claude_sessions(&home, "/tmp/my.proj");
        let ids: Vec<&str> = sessions.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec![ID, older]);
        assert!(claude_sessions(&home, "/tmp/other").is_empty());
        let none = HashSet::new();
        let before_both = sessions[1].1 - std::time::Duration::from_secs(1);
        assert_eq!(
            choose_session(&sessions, before_both, None, &none).as_deref(),
            Some(ID)
        );
        // Two agents in one directory keep one session each.
        assert_eq!(
            choose_session(&sessions, before_both, Some(older), &none).as_deref(),
            Some(older)
        );
        let taken = HashSet::from([ID.to_string()]);
        assert_eq!(
            choose_session(&sessions, before_both, None, &taken).as_deref(),
            Some(older)
        );
        // A process started after `older` was last written is not in it, even when its pane saved
        // it: that was an earlier run in this directory.
        let between = sessions[1].1 + std::time::Duration::from_millis(1);
        assert_eq!(
            choose_session(&sessions, between, Some(older), &none).as_deref(),
            Some(ID)
        );
        // Before the new run has written anything, its pane keeps what it saved, and a pane that
        // saved nothing has no session.
        let after_both = sessions[0].1 + std::time::Duration::from_secs(1);
        assert_eq!(
            choose_session(&sessions, after_both, Some(older), &none).as_deref(),
            Some(older)
        );
        assert_eq!(choose_session(&sessions, after_both, None, &none), None);
        let agent = TermAgent {
            name: "claude".into(),
            session: ID.into(),
            home: Some(home.display().to_string()),
            context: None,
        };
        assert!(session_exists(&agent, "/tmp/my.proj"));
        let gone = TermAgent {
            session: "bbbbbbbb-2222-3333-4444-555555555555".into(),
            ..agent
        };
        assert!(!session_exists(&gone, "/tmp/my.proj"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_transcripts_last_assistant_usage_is_the_context_use() {
        let home =
            std::env::temp_dir().join(format!("wheel-resume-context-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let project = claude_project_dir(&home, "/tmp/ctx");
        std::fs::create_dir_all(&project).unwrap();
        let entry = |kind: &str, extra: &str, input: u64, cache: u64, out: u64| {
            format!(
                r#"{{"type":"{kind}",{extra}"message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":{input},"cache_creation_input_tokens":10,"cache_read_input_tokens":{cache},"output_tokens":{out}}}}}}}"#
            )
        };
        let lines = [
            entry("assistant", "", 1, 5_000, 50),
            entry("user", "", 9, 9, 9),
            entry("assistant", "", 2, 73_988, 100),
            entry("assistant", r#""isSidechain":true,"#, 7, 7, 7),
        ];
        std::fs::write(project.join(format!("{ID}.jsonl")), lines.join("\n")).unwrap();
        assert_eq!(
            claude_context(&home, "/tmp/ctx", ID),
            Some(ContextUse {
                used: 74_100,
                size: 200_000
            })
        );
        let big = [entry("assistant", "", 3, 420_000, 97)];
        std::fs::write(project.join(format!("{ID}.jsonl")), big.join("\n")).unwrap();
        assert_eq!(claude_context(&home, "/tmp/ctx", ID), None);
        let attachment = r#"{"type":"attachment","attachment":{"type":"model","identity":{"modelId":"claude-opus-5-5[1m]","marketingName":"Opus 5.5 (1M context)"}}}"#;
        let filler = format!(
            r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
            "x".repeat(600 * 1024)
        );
        let session = [attachment.to_string(), filler, big[0].clone()].join("\n");
        std::fs::write(project.join(format!("{ID}.jsonl")), session).unwrap();
        assert_eq!(
            claude_context(&home, "/tmp/ctx", ID),
            Some(ContextUse {
                used: 420_110,
                size: 1_000_000
            })
        );
        let named = attachment
            .replace("claude-opus-5-5[1m]", "claude-opus-5-5")
            .replace("Opus 5.5 (1M context)", "Opus 5.5");
        let tail_switch = [attachment.to_string(), big[0].clone(), named].join("\n");
        std::fs::write(project.join(format!("{ID}.jsonl")), tail_switch).unwrap();
        assert_eq!(claude_context(&home, "/tmp/ctx", ID), None);
        let marketing = attachment.replace("claude-opus-5-5[1m]", "claude-opus-5-5");
        let session = [marketing, big[0].clone()].join("\n");
        std::fs::write(project.join(format!("{ID}.jsonl")), session).unwrap();
        assert_eq!(
            claude_context(&home, "/tmp/ctx", ID).map(|c| c.size),
            Some(1_000_000)
        );
        std::fs::write(project.join(format!("{ID}.jsonl")), "{}").unwrap();
        assert_eq!(claude_context(&home, "/tmp/ctx", ID), None);
        assert_eq!(claude_context(&home, "/tmp/ctx", "missing"), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_newest_codex_session_started_in_a_directory_is_found() {
        let home = std::env::temp_dir().join(format!("wheel-resume-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/10/19");
        std::fs::create_dir_all(&day).unwrap();
        let line = |cwd: &str| {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{ID}","cwd":"{cwd}"}}}}"#)
        };
        std::fs::write(
            day.join(format!("rollout-2026-10-19T10-00-00-{ID}.jsonl")),
            line("/tmp/p"),
        )
        .unwrap();
        let found: Vec<String> = codex_sessions(&home, "/tmp/p")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(found, vec![ID.to_string()]);
        assert!(codex_sessions(&home, "/tmp/q").is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_process_argument_block_splits_into_arguments_and_environment() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/claude\0\0\0claude\0--x\0HOME=/h\0CLAUDE_CONFIG_DIR=/c\0");
        let (args, env) = parse_procargs(&buf).unwrap();
        assert_eq!(args, vec!["claude", "--x"]);
        assert_eq!(env[1], ("CLAUDE_CONFIG_DIR".into(), "/c".into()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_has_a_directory_and_arguments() {
        let me = std::process::id();
        assert_eq!(
            process_cwd(me).map(|p| std::fs::canonicalize(p).unwrap()),
            Some(std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap())
        );
        let (args, env) = process_args(me).unwrap();
        assert!(!args.is_empty());
        assert!(env.iter().any(|(k, _)| k == "PATH"));
    }
}
