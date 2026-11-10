//! Bringing a terminal pane back after a restart: the directory its shell was in, and the coding
//! agent it was running, resumed by that agent's own command. The daemon samples the process in
//! each terminal's foreground, saves what it finds in the pane, and on restore opens the shell
//! there and types the resume command into it.
//!
//! An agent is recognised by the program in the terminal's foreground (Claude Code, Codex or pi), and
//! its conversation id comes from the program's own arguments (`--resume <id>`) or, for a conversation
//! started fresh, from the newest transcript the agent has written for that directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wheel_proto::{ContextUse, TermAgent};

/// The coding agents whose conversations a terminal pane resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

impl Harness {
    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
        }
    }

    /// The program's name as a person says it.
    pub fn title(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Codex => "Codex",
            Harness::Pi => "Pi",
        }
    }

    pub fn parse(name: &str) -> Option<Harness> {
        match name {
            "claude" => Some(Harness::Claude),
            "codex" => Some(Harness::Codex),
            "pi" => Some(Harness::Pi),
            _ => None,
        }
    }

    /// The program and arguments that resume `conversation`.
    pub fn resume_argv(self, conversation: &str) -> Vec<String> {
        match self {
            Harness::Claude => vec!["claude".into(), "--resume".into(), conversation.into()],
            Harness::Codex => vec!["codex".into(), "resume".into(), conversation.into()],
            Harness::Pi => vec!["pi".into(), "--session".into(), conversation.into()],
        }
    }

    /// The environment variable that moves the agent's data directory, and where that is by default.
    pub(crate) fn home_var(self) -> &'static str {
        match self {
            Harness::Claude => "CLAUDE_CONFIG_DIR",
            Harness::Codex => "CODEX_HOME",
            Harness::Pi => "PI_CODING_AGENT_DIR",
        }
    }

    pub fn default_home(self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(match self {
            Harness::Claude => home.join(".claude"),
            Harness::Codex => home.join(".codex"),
            Harness::Pi => home.join(".pi").join("agent"),
        })
    }

    /// The data directory the daemon itself would use for this agent.
    pub fn daemon_home(self) -> Option<PathBuf> {
        std::env::var_os(self.home_var())
            .map(PathBuf::from)
            .or_else(|| self.default_home())
    }
}

/// The name of the program process `pid` runs: its first argument's last path component, without
/// the leading `-` a login shell carries.
pub fn process_name(pid: u32) -> Option<String> {
    let (args, _) = process_args(pid)?;
    let first = args.first()?;
    let name = first.rsplit('/').next()?.trim_start_matches('-');
    (!name.is_empty()).then(|| name.to_string())
}

/// A process's arguments and its environment as name and value.
pub(crate) type ProcessArgs = (Vec<String>, Vec<(String, String)>);

/// What a sample of one terminal found.
#[derive(Debug, Default, PartialEq)]
pub struct Sample {
    pub cwd: Option<String>,
    pub agent: Option<TermAgent>,
}

/// The agent a command line starts, and the conversation id it names, if it names one.
pub fn detect(argv: &[String]) -> Option<(Harness, Option<String>)> {
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
    let agent = Harness::parse(&program)?;
    let mut conversation = None;
    let mut args = argv
        .iter()
        .skip_while(|a| base(a).as_deref() != Some(&program))
        .skip(1);
    while let Some(arg) = args.next() {
        let value = match (agent, arg.as_str()) {
            (Harness::Claude, "--resume" | "-r" | "--session-id") => args.next().cloned(),
            (Harness::Claude, a) if a.starts_with("--resume=") => {
                a.strip_prefix("--resume=").map(String::from)
            }
            (Harness::Claude, a) if a.starts_with("--session-id=") => {
                a.strip_prefix("--session-id=").map(String::from)
            }
            (Harness::Codex, "resume") => args.next().cloned(),
            (Harness::Pi, "--session" | "--session-id") => args.next().cloned(),
            (Harness::Pi, a) if a.starts_with("--session=") => {
                a.strip_prefix("--session=").map(String::from)
            }
            (Harness::Pi, a) if a.starts_with("--session-id=") => {
                a.strip_prefix("--session-id=").map(String::from)
            }
            _ => None,
        };
        if let Some(value) = value.filter(|v| looks_like_session_id(v)) {
            conversation = Some(value);
        }
    }
    Some((agent, conversation))
}

/// A conversation id is a UUID; anything else after `--resume` is a search term or a flag.
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

/// A conversation id and the moment `choose_conversation` dates it by: when its transcript was
/// last written, or for Claude Code when it was created.
type ConversationWritten = (String, std::time::SystemTime);

/// The conversations Claude Code has for `cwd`, the one created last first. A transcript is dated by
/// when its file was created, not last written: another live session in the same directory writes
/// to its file all the time, and that must never make it look like it began after our process.
fn claude_sessions(home: &Path, cwd: &str) -> Vec<ConversationWritten> {
    let dir = claude_project_dir(home, cwd);
    let mut found: Vec<ConversationWritten> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(created) = metadata.created().or_else(|_| metadata.modified()) else {
            continue;
        };
        if looks_like_session_id(id) {
            found.push((id.to_string(), created));
        }
    }
    found.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    found
}

/// The conversations Codex started in `cwd` over its last three days of transcripts, the newest
/// first. Codex files its transcripts by date and names the directory in the first line of each.
fn codex_sessions(home: &Path, cwd: &str) -> Vec<ConversationWritten> {
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

/// Where pi keeps `cwd`'s conversations: the directory `from_env` (`PI_CODING_AGENT_SESSION_DIR`)
/// names, else the `sessionDir` of `settings.json` under pi's home, else `conversations/--<cwd>--`
/// under pi's home, the path without its leading `/` and with every `/`, `\` and `:` as `-`. A
/// leading `~/` is the user's home and a relative directory is relative to `cwd`. A directory
/// chosen by the environment or the settings holds the conversation files itself, for every project;
/// each file's header says whose it is.
fn pi_session_dir(home: &Path, cwd: &str, from_env: Option<&std::ffi::OsStr>) -> PathBuf {
    let chosen = from_env
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let settings = std::fs::read_to_string(home.join("settings.json")).ok()?;
            let settings: serde_json::Value = serde_json::from_str(&settings).ok()?;
            let dir = settings["sessionDir"].as_str().filter(|d| !d.is_empty())?;
            Some(PathBuf::from(dir))
        });
    if let Some(dir) = chosen {
        let dir = dir.to_string_lossy().into_owned();
        let expanded = match dir.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
            None if dir == "~" => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()),
            None => PathBuf::from(&dir),
        };
        return Path::new(cwd).join(expanded);
    }
    let path = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    let name: String = path
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    home.join("sessions").join(format!("--{name}--"))
}

/// The conversations pi keeps for `cwd`: each one's id, its file, and when that was last written. A
/// conversation directory can hold other projects' conversations, and two directories can share a name
/// (`/tmp/a-b` and `/tmp/a/b`), so a conversation is `cwd`'s only when the header on its file's first
/// line names `cwd`. `env_dir` is `PI_CODING_AGENT_SESSION_DIR` as the agent sees it.
fn pi_session_files(
    home: &Path,
    cwd: &str,
    env_dir: Option<&std::ffi::OsStr>,
) -> Vec<(String, PathBuf, std::time::SystemTime)> {
    use std::io::BufRead;
    std::fs::read_dir(pi_session_dir(home, cwd, env_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                return None;
            }
            let first = std::io::BufReader::new(std::fs::File::open(&path).ok()?)
                .lines()
                .next()?
                .ok()?;
            let header: serde_json::Value = serde_json::from_str(&first).ok()?;
            if header["type"] != "session" || header["cwd"].as_str()? != cwd {
                return None;
            }
            let id = header["id"].as_str()?.to_string();
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            Some((id, path, modified))
        })
        .collect()
}

/// The conversations pi has for `cwd`, the one written to last first.
fn pi_sessions(home: &Path, cwd: &str, env_dir: Option<&std::ffi::OsStr>) -> Vec<ConversationWritten> {
    let mut found: Vec<ConversationWritten> = pi_session_files(home, cwd, env_dir)
        .into_iter()
        .filter(|(id, _, _)| looks_like_session_id(id))
        .map(|(id, _, modified)| (id, modified))
        .collect();
    found.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    found
}

/// The most conversations a listing returns: the newest. Listing reads each file's first line and
/// a title, so an unbounded list would cost more with every conversation the machine has had.
const LISTED: usize = 200;

/// How many lines of a conversation file are read looking for its first prompt.
const TITLE_LINES: usize = 400;

/// A prompt as a title: its first non-empty line, cut to 200 characters.
fn title_of(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(200).collect())
}

/// The text of a message's content, whether it is a string or a list of blocks.
fn content_text(content: &serde_json::Value) -> Option<String> {
    match content {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(blocks) => blocks.iter().find_map(|b| b["text"].as_str().map(String::from)),
        _ => None,
    }
}

/// The first prompt in the first `TITLE_LINES` lines of `file` that `prompt` finds, as a title.
fn first_prompt(file: &Path, prompt: impl Fn(&serde_json::Value) -> Option<String>) -> Option<String> {
    use std::io::BufRead;
    let lines = std::io::BufReader::new(std::fs::File::open(file).ok()?).lines();
    lines
        .take(TITLE_LINES)
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .find_map(|entry| prompt(&entry).and_then(|t| title_of(&t)))
}

fn listing(
    id: String,
    cwd: &str,
    title: Option<String>,
    modified: std::time::SystemTime,
) -> wheel_proto::ConversationInfo {
    wheel_proto::ConversationInfo {
        id,
        cwd: cwd.to_string(),
        title,
        updated_at: Some(crate::claude::iso8601(modified)),
        current: false,
    }
}

/// The conversations Codex holds for `cwd`, newest first, titled after their first prompt. Codex
/// files its transcripts by date, naming the directory in the first line of each; one made through
/// a symlinked path names the resolved one, so either spelling matches. The store belongs to the
/// directory, so this lists whatever else ran Codex there too (ADR-0006).
pub(crate) fn codex_conversations(home: &Path, cwd: &str) -> Vec<wheel_proto::ConversationInfo> {
    use std::io::BufRead;
    let resolved = std::fs::canonicalize(cwd).map(|p| p.display().to_string()).unwrap_or_else(|_| cwd.to_string());
    let newest_first = |dir: &Path| {
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        names.sort();
        names.reverse();
        names
    };
    let mut found = Vec::new();
    // Date directories sort newest first, so the walk stops once enough are found.
    'walk: for year in newest_first(&home.join("sessions")) {
        for month in newest_first(&year) {
            for day in newest_first(&month) {
                for file in newest_first(&day) {
                    if found.len() >= LISTED {
                        break 'walk;
                    }
                    let Some(Ok(first)) = std::fs::File::open(&file).ok().and_then(|f| std::io::BufReader::new(f).lines().next()) else {
                        continue;
                    };
                    let Ok(meta) = serde_json::from_str::<serde_json::Value>(&first) else {
                        continue;
                    };
                    let payload = &meta["payload"];
                    let (Some(id), Some(at)) = (payload["id"].as_str(), payload["cwd"].as_str()) else {
                        continue;
                    };
                    if at != cwd && at != resolved {
                        continue;
                    }
                    let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) else {
                        continue;
                    };
                    // The brief and the project's instructions go in as user messages too; the
                    // person's own prompt is the first one that is not a block of markup or of
                    // instructions.
                    let title = first_prompt(&file, |entry| {
                        let p = &entry["payload"];
                        (entry["type"] == "response_item" && p["type"] == "message" && p["role"] == "user")
                            .then(|| content_text(&p["content"]))
                            .flatten()
                            .filter(|t| !t.starts_with('<') && !t.starts_with("# AGENTS.md"))
                    });
                    found.push((modified, listing(id.to_string(), cwd, title, modified)));
                }
            }
        }
    }
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().map(|(_, info)| info).collect()
}

/// The conversations pi holds for `cwd`, newest first, titled after their first prompt.
pub(crate) fn pi_conversations(
    home: &Path,
    cwd: &str,
    env_dir: Option<&std::ffi::OsStr>,
) -> Vec<wheel_proto::ConversationInfo> {
    let mut files = pi_session_files(home, cwd, env_dir);
    files.sort_by_key(|(_, _, at)| std::cmp::Reverse(*at));
    files.truncate(LISTED);
    let mut found: Vec<_> = files
        .into_iter()
        .map(|(id, path, modified)| {
            let title = first_prompt(&path, |entry| {
                let m = &entry["message"];
                (entry["type"] == "message" && m["role"] == "user").then(|| content_text(&m["content"])).flatten()
            });
            (modified, listing(id, cwd, title, modified))
        })
        .collect();
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().map(|(_, info)| info).collect()
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-listing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn write(path: &Path, lines: &[serde_json::Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    #[test]
    fn codex_lists_this_directorys_conversations_titled_by_the_first_real_prompt() {
        let home = scratch("codex");
        let day = home.join("sessions/2026/11/10");
        let meta = |id: &str, cwd: &str| serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd}});
        let user = |text: &str| serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}});
        write(&day.join("rollout-a.jsonl"), &[meta("a", "/work/x"), user("# AGENTS.md instructions\n\nbe nice"), user("<environment_context>"), user("fix the build\nplease")]);
        write(&day.join("rollout-b.jsonl"), &[meta("b", "/work/other"), user("not mine")]);
        write(&day.join("rollout-c.jsonl"), &[meta("c", "/work/x")]);
        let listed = codex_conversations(&home, "/work/x");
        let _ = std::fs::remove_dir_all(&home);
        let mut ids: Vec<_> = listed.iter().map(|c| (c.id.as_str(), c.title.as_deref())).collect();
        ids.sort();
        assert_eq!(ids, vec![("a", Some("fix the build")), ("c", None)]);
        assert!(listed.iter().all(|c| c.cwd == "/work/x" && c.updated_at.is_some() && !c.current));
    }

    #[test]
    fn pi_lists_this_directorys_conversations_titled_by_the_first_user_message() {
        let home = scratch("pi");
        let dir = pi_session_dir(&home, "/work/x", None);
        let header = |id: &str, cwd: &str| serde_json::json!({"type":"session","id":id,"cwd":cwd});
        let message = |role: &str, text: &str| serde_json::json!({"type":"message","message":{"role":role,"content":[{"type":"text","text":text}]}});
        write(&dir.join("1_a.jsonl"), &[header("a", "/work/x"), message("system", "preamble"), message("user", "  list the files  ")]);
        write(&dir.join("2_b.jsonl"), &[header("b", "/work/y"), message("user", "elsewhere")]);
        let listed = pi_conversations(&home, "/work/x", None);
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].id.as_str(), listed[0].title.as_deref()), ("a", Some("list the files")));
    }
}

/// The file pi keeps `conversation` in for `cwd`, if it keeps one.
pub(crate) fn pi_session_file(home: &Path, cwd: &str, conversation: &str) -> Option<PathBuf> {
    pi_session_files(
        home,
        cwd,
        std::env::var_os("PI_CODING_AGENT_SESSION_DIR").as_deref(),
    )
    .into_iter()
    .find(|(id, _, _)| id == conversation)
    .map(|(_, path, _)| path)
}

/// Which of `conversations` a running agent is in when its command line does not say.
///
/// Only a conversation dated since the agent's process `started` can be its own: one dated before
/// that is an earlier run, or another live session, in the same directory. Of those, the one its pane already saved,
/// so two agents in one directory keep one conversation each, else the one dated last (written last, for Codex and pi; created last, for Claude Code) that no other
/// pane has saved. An agent that has written nothing yet (a fresh one, or one at its resume
/// picker) keeps the conversation its pane saved, so a restart still resumes it; with no turn written,
/// it has no turn a switch could cut short.
fn choose_conversation(
    conversations: &[ConversationWritten],
    started: std::time::SystemTime,
    previous: Option<&str>,
    claimed: &HashSet<String>,
) -> Option<String> {
    let mut candidates: Vec<&ConversationWritten> = conversations
        .iter()
        .filter(|(_, written)| *written >= started)
        .collect();
    candidates.sort_by_key(|(_, written)| std::cmp::Reverse(*written));
    let ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
    let exists = |p: &&str| conversations.iter().any(|(id, _)| id == p);
    let chosen = if ids.is_empty() {
        previous.filter(exists)
    } else {
        previous
            .filter(|p| ids.contains(p))
            .or_else(|| ids.iter().copied().find(|s| !claimed.contains(*s)))
    };
    chosen.map(String::from)
}

fn codex_session_in(file: &Path, cwd: &str) -> Option<ConversationWritten> {
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

/// Whether the agent still holds `agent`'s saved conversation: a resume of a conversation it has lost would
/// only print an error.
pub fn conversation_exists(agent: &TermAgent, cwd: &str) -> bool {
    let Some(kind) = Harness::parse(&agent.name) else {
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
        Harness::Claude => claude_project_dir(&home, cwd)
            .join(format!("{}.jsonl", agent.conversation))
            .is_file(),
        Harness::Codex => walk_for(&home.join("sessions"), &agent.conversation),
        Harness::Pi => pi_session_file(&home, cwd, &agent.conversation).is_some(),
    }
}

/// Whether a file under `dir` carries `conversation` in its name (Codex names a transcript
/// `rollout-<time>-<conversation>.jsonl`).
fn walk_for(dir: &Path, conversation: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let path = e.path();
            if path.is_dir() {
                walk_for(&path, conversation)
            } else {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(conversation))
            }
        })
}

/// The line typed into a restored shell to resume `agent`. The agent's data directory is named
/// when the conversation lives somewhere other than the default, so the agent finds it.
pub fn resume_line(agent: &TermAgent) -> Option<String> {
    line(agent)
}

/// The line typed into a new shell to take over `agent`'s conversation from wheel's agent view. The
/// agent must run with the environment the agent view's process had, because Claude Code keeps its
/// login in a Keychain item named after `CLAUDE_CONFIG_DIR`: a different value is a different
/// login. `agent.home` is the directory the agent view was given, if any; otherwise the agent
/// view inherited the daemon's own `CLAUDE_CONFIG_DIR`, and the person's shell setup must not
/// replace it, so the line names the daemon's value, or removes the variable when it has none.
pub fn switch_line(agent: &TermAgent) -> Option<String> {
    let kind = Harness::parse(&agent.name)?;
    let words = kind
        .resume_argv(&agent.conversation)
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
    let kind = Harness::parse(&agent.name)?;
    let words = kind.resume_argv(&agent.conversation);
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

/// The entries in the last `window` bytes of a JSON-lines transcript. A window that starts
/// mid-file starts mid-line; that first piece is dropped.
fn transcript_tail(path: &Path, window: u64) -> Option<Vec<serde_json::Value>> {
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
    Some(entries)
}

/// How much of the end of a Codex rollout is read first.
const TRANSCRIPT_TAIL: u64 = 512 * 1024;

/// The larger windows read when the newest `token_count` event is further back than the first
/// one: a turn that ran many tools writes far more than `TRANSCRIPT_TAIL` after it.
const ROLLOUT_WINDOWS: [u64; 3] = [TRANSCRIPT_TAIL, 16 * 1024 * 1024, u64::MAX];

/// The rollout file of Codex `conversation` under `conversations`, newest day first: Codex names it
/// `rollout-<time>-<conversation>.jsonl` inside a year, month and day directory.
fn codex_rollout(conversations: &Path, conversation: &str) -> Option<PathBuf> {
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
    let suffix = format!("-{conversation}.jsonl");
    for year in newest_first(conversations) {
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

/// How full Codex `conversation`'s context window is: the token count of the last request and the
/// window Codex states, both from the newest `token_count` event in its rollout. None while the
/// rollout has no such event, or an event with no figures yet.
pub fn codex_context(home: &Path, conversation: &str) -> Option<ContextUse> {
    let path = codex_rollout(&home.join("sessions"), conversation)?;
    for window in ROLLOUT_WINDOWS {
        let entries = transcript_tail(&path, window)?;
        let found = entries.iter().rev().find_map(|entry| {
            let payload = &entry["payload"];
            if entry["type"] != "event_msg" || payload["type"] != "token_count" {
                return None;
            }
            let info = &payload["info"];
            Some(ContextUse {
                used: info["last_token_usage"]["total_tokens"].as_u64()?,
                size: info["model_context_window"].as_u64()?,
            })
        });
        if found.is_some() {
            return found;
        }
    }
    None
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

/// The nearest ancestor of `pid` that is Claude Code: a hook runs as its child, through a shell.
pub fn claude_ancestor(pid: u32) -> Option<u32> {
    let mut at = pid;
    for _ in 0..16 {
        at = process_parent(at).filter(|p| *p > 1)?;
        let is_claude = process_args(at)
            .and_then(|(args, _)| detect(&args))
            .is_some_and(|(agent, _)| agent == Harness::Claude);
        if is_claude {
            return Some(at);
        }
    }
    None
}

/// The process that started `pid`.
#[cfg(target_os = "macos")]
fn process_parent(pid: u32) -> Option<u32> {
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
    (got == size).then_some(info.pbi_ppid)
}

/// The process that started `pid`: field 4 of `/proc/<pid>/stat`.
#[cfg(not(target_os = "macos"))]
fn process_parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 2..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// What is running in a terminal whose shell is `shell` and whose foreground process is
/// `foreground`. `previous` is the agent the pane saved last time, and `claimed` the conversations
/// other panes have saved.
pub fn sample(
    shell: Option<u32>,
    foreground: Option<u32>,
    previous: Option<&TermAgent>,
    claimed: &HashSet<String>,
    reported: Option<ContextUse>,
    reported_conversation: Option<&str>,
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
                let pi_sessions_dir = env
                    .iter()
                    .find(|(k, _)| k == "PI_CODING_AGENT_SESSION_DIR")
                    .map(|(_, v)| std::ffi::OsString::from(v))
                    .or_else(|| std::env::var_os("PI_CODING_AGENT_SESSION_DIR"));
                let search = home.clone().or_else(|| kind.daemon_home());
                // `/clear`, `/resume` and `/compact` change a running Claude Code's conversation
                // without restarting it, so neither its command line nor the transcripts written
                // since it started say which one it is in now; its own report does.
                let reported_conversation = reported_conversation
                    .filter(|_| kind == Harness::Claude)
                    .filter(|id| looks_like_session_id(id))
                    .filter(|id| !claimed.contains(*id))
                    .map(String::from);
                let conversation = reported_conversation.or(named).or_else(|| {
                    let (home, cwd) = (search.as_ref()?, cwd.as_deref()?);
                    let conversations = match kind {
                        Harness::Claude => claude_sessions(home, cwd),
                        Harness::Codex => codex_sessions(home, cwd),
                        Harness::Pi => pi_sessions(home, cwd, pi_sessions_dir.as_deref()),
                    };
                    let previous = previous
                        .filter(|p| p.name == kind.name())
                        .map(|p| p.conversation.as_str());
                    let Some(started) = process_started(fg) else {
                        // Once per process: the sampler asks every 2 s.
                        static WARNED: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);
                        if WARNED.lock().unwrap().replace(fg) != Some(fg) {
                            tracing::warn!(pid = fg, agent = kind.name(), "cannot read when the agent's process started; not guessing its conversation");
                        }
                        return None;
                    };
                    choose_conversation(&conversations, started, previous, claimed)
                });
                if let Some(conversation) = conversation {
                    // Codex states its window in its rollout; Claude and pi say theirs through
                    // the hooks their wrappers add.
                    let context = match (kind, search.as_ref()) {
                        (Harness::Codex, Some(home)) => codex_context(home, &conversation),
                        (Harness::Codex, None) => None,
                        _ => reported,
                    };
                    sample.agent = Some(TermAgent {
                        name: kind.name().into(),
                        conversation,
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
pub(crate) fn process_args(pid: u32) -> Option<ProcessArgs> {
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
pub(crate) fn process_args(pid: u32) -> Option<ProcessArgs> {
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
    fn a_command_line_names_its_agent_and_conversation() {
        assert_eq!(
            detect(&argv(&["claude", "--resume", ID])),
            Some((Harness::Claude, Some(ID.into())))
        );
        assert_eq!(
            detect(&argv(&["/opt/bin/claude", "--model", "x"])),
            Some((Harness::Claude, None))
        );
        assert_eq!(
            detect(&argv(&["node", "/usr/lib/bin/codex", "resume", ID])),
            Some((Harness::Codex, Some(ID.into())))
        );
        assert_eq!(
            detect(&argv(&["pi", "--session", ID])),
            Some((Harness::Pi, Some(ID.into())))
        );
        assert_eq!(
            detect(&argv(&["claude", "--resume", "a search"])),
            Some((Harness::Claude, None))
        );
        assert_eq!(detect(&argv(&["vim", "claude"])), None);
        assert_eq!(detect(&argv(&["-zsh"])), None);
    }

    #[test]
    fn resume_commands_name_the_saved_conversation() {
        let agent = |name: &str, home: Option<&str>| TermAgent {
            name: name.into(),
            conversation: ID.into(),
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
        let default = Harness::Claude.default_home().unwrap().display().to_string();
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
    fn a_process_start_time_is_read_from_the_system() {
        let started = process_started(std::process::id()).expect("this process has a start time");
        let age = started.elapsed().expect("it started in the past");
        assert!(age < std::time::Duration::from_secs(3600), "{age:?}");
    }

    #[test]
    fn the_newest_claude_transcript_for_a_directory_is_its_conversation() {
        let home = std::env::temp_dir().join(format!("wheel-resume-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let project = claude_project_dir(&home, "/tmp/my.proj");
        assert_eq!(project, home.join("projects/-tmp-my-proj"));
        std::fs::create_dir_all(&project).unwrap();
        let older = "aaaaaaaa-2222-3333-4444-555555555555";
        std::fs::write(project.join(format!("{older}.jsonl")), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(project.join(format!("{ID}.jsonl")), "{}").unwrap();
        let conversations = claude_sessions(&home, "/tmp/my.proj");
        // The older session is written to again after the newer one was created, as a live session
        // in a shared folder is. A process that started between the two is the newer one's.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(project.join(format!("{older}.jsonl")), "{}\n{}").unwrap();
        let between_creations = conversations[0].1 - std::time::Duration::from_millis(1);
        let rewritten = claude_sessions(&home, "/tmp/my.proj");
        assert_eq!(
            choose_conversation(&rewritten, between_creations, None, &HashSet::new()).as_deref(),
            Some(ID)
        );
        let ids: Vec<&str> = conversations.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec![ID, older]);
        assert!(claude_sessions(&home, "/tmp/other").is_empty());
        let none = HashSet::new();
        let before_both = conversations[1].1 - std::time::Duration::from_secs(1);
        assert_eq!(
            choose_conversation(&conversations, before_both, None, &none).as_deref(),
            Some(ID)
        );
        // Two agents in one directory keep one conversation each.
        assert_eq!(
            choose_conversation(&conversations, before_both, Some(older), &none).as_deref(),
            Some(older)
        );
        let taken = HashSet::from([ID.to_string()]);
        assert_eq!(
            choose_conversation(&conversations, before_both, None, &taken).as_deref(),
            Some(older)
        );
        // A process started after `older` was last written is not in it, even when its pane saved
        // it: that was an earlier run in this directory.
        let between = conversations[1].1 + std::time::Duration::from_millis(1);
        assert_eq!(
            choose_conversation(&conversations, between, Some(older), &none).as_deref(),
            Some(ID)
        );
        // Before the new run has written anything, its pane keeps what it saved, and a pane that
        // saved nothing has no conversation.
        let after_both = conversations[0].1 + std::time::Duration::from_secs(1);
        assert_eq!(
            choose_conversation(&conversations, after_both, Some(older), &none).as_deref(),
            Some(older)
        );
        assert_eq!(choose_conversation(&conversations, after_both, None, &none), None);
        let agent = TermAgent {
            name: "claude".into(),
            conversation: ID.into(),
            home: Some(home.display().to_string()),
            context: None,
        };
        assert!(conversation_exists(&agent, "/tmp/my.proj"));
        let gone = TermAgent {
            conversation: "bbbbbbbb-2222-3333-4444-555555555555".into(),
            ..agent
        };
        assert!(!conversation_exists(&gone, "/tmp/my.proj"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_codex_rollouts_last_token_count_is_the_context_use() {
        let home = std::env::temp_dir().join(format!("wheel-resume-ctx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let day = home.join("sessions/2026/10/22");
        std::fs::create_dir_all(&day).unwrap();
        let count = |last: u64, total: u64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"total_tokens":{last}}},"model_context_window":258400}}}}}}"#
            )
        };
        let empty = r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#;
        let file = day.join(format!("rollout-2026-10-22T10-00-00-{ID}.jsonl"));
        let lines = [
            count(1_000, 1_000),
            count(120_000, 400_000),
            empty.to_string(),
        ];
        std::fs::write(&file, lines.join("\n")).unwrap();
        assert_eq!(
            codex_context(&home, ID),
            Some(ContextUse {
                used: 120_000,
                size: 258_400
            })
        );
        std::fs::write(&file, empty).unwrap();
        assert_eq!(codex_context(&home, ID), None);
        assert_eq!(codex_context(&home, "missing"), None);
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
    fn a_pi_session_is_found_by_directory() {
        let home = std::env::temp_dir().join(format!("wheel-resume-pi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let dir = home.join("sessions/--tmp-p-q--");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("2026-10-21T21-16-53-978Z_{ID}.jsonl"));
        let message = |role: &str, stop: &str| {
            format!(r#"{{"type":"message","message":{{"role":"{role}","stopReason":"{stop}"}}}}"#)
        };
        let header = format!(r#"{{"type":"session","id":"{ID}","cwd":"/tmp/p-q"}}"#);
        let lines = [
            header.clone(),
            message("user", ""),
            message("assistant", "toolUse"),
        ];
        std::fs::write(&file, lines.join("\n")).unwrap();
        let found: Vec<String> = pi_sessions(&home, "/tmp/p-q", None)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(found, vec![ID.to_string()]);
        assert!(pi_sessions(&home, "/tmp/p", None).is_empty());
        // `/tmp/p/q` files its conversations in the same directory, but this one is not its.
        assert!(pi_sessions(&home, "/tmp/p/q", None).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pi_sessions_are_found_where_its_settings_or_environment_say() {
        let home = std::env::temp_dir().join(format!("wheel-resume-pi-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let header = format!(r#"{{"type":"session","id":"{ID}","cwd":"/tmp/p"}}"#);
        std::fs::write(elsewhere.join(format!("t_{ID}.jsonl")), header).unwrap();
        let default = pi_session_dir(&home, "/tmp/p", None);
        assert_eq!(default, home.join("sessions/--tmp-p--"));
        std::fs::write(
            home.join("settings.json"),
            format!(r#"{{"sessionDir":"{}"}}"#, elsewhere.display()),
        )
        .unwrap();
        assert_eq!(pi_session_dir(&home, "/tmp/p", None), elsewhere);
        let env = std::ffi::OsString::from("/env/dir");
        assert_eq!(
            pi_session_dir(&home, "/tmp/p", Some(env.as_os_str())),
            PathBuf::from("/env/dir"),
            "the environment wins over the settings file"
        );
        // through the public lookups, with the settings file in force
        assert_eq!(
            pi_session_file(&home, "/tmp/p", ID),
            Some(elsewhere.join(format!("t_{ID}.jsonl")))
        );
        assert!(pi_session_file(&home, "/tmp/other", ID).is_none());
        std::fs::write(home.join("settings.json"), r#"{"sessionDir":"rel/s"}"#).unwrap();
        assert_eq!(
            pi_session_dir(&home, "/tmp/p", None),
            PathBuf::from("/tmp/p/rel/s"),
            "a relative directory is relative to the project"
        );
        std::fs::write(home.join("settings.json"), r#"{"sessionDir":"~/s"}"#).unwrap();
        let user = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        assert_eq!(pi_session_dir(&home, "/tmp/p", None), user.join("s"));
        std::fs::write(home.join("settings.json"), "{ not json").unwrap();
        assert_eq!(pi_session_dir(&home, "/tmp/p", None), default);
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
