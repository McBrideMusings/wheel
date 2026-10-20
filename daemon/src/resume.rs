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

use wheel_proto::TermAgent;

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

/// Whether `command` runs the Claude Code ACP agent, whose conversations Claude Code's own
/// `--resume` reads. `wheel-fake-agent` stands in for it in the tests.
pub fn runs_claude(command: &str) -> bool {
    // Any word, so `npx -y claude-agent-acp`, `node /path/claude-agent-acp` and `env X=1 ...` count.
    command.split_whitespace().any(|word| {
        matches!(
            Path::new(word).file_name().and_then(|n| n.to_str()),
            Some("claude-agent-acp" | "wheel-fake-agent")
        )
    })
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
fn claude_project_dir(home: &Path, cwd: &str) -> PathBuf {
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

/// The ids of the sessions Claude Code has for `cwd`, the one written to last first.
fn claude_sessions(home: &Path, cwd: &str) -> Vec<String> {
    let dir = claude_project_dir(home, cwd);
    let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
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
            found.push((modified, id.to_string()));
        }
    }
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found.into_iter().map(|(_, id)| id).collect()
}

/// The ids of the sessions Codex started in `cwd` over its last three days of transcripts, the
/// newest first. Codex files its transcripts by date and names the directory in the first line
/// of each.
fn codex_sessions(home: &Path, cwd: &str) -> Vec<String> {
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

/// Which of `sessions` (newest first) a running agent is in when its command line does not say:
/// the one its pane already saved, so two agents in one directory keep one session each, else the
/// newest that no other pane has saved.
fn choose_session(
    sessions: &[String],
    previous: Option<&str>,
    claimed: &HashSet<String>,
) -> Option<String> {
    previous
        .filter(|p| sessions.iter().any(|s| s == p))
        .or_else(|| {
            sessions
                .iter()
                .map(String::as_str)
                .find(|s| !claimed.contains(*s))
        })
        .map(String::from)
}

fn codex_session_in(file: &Path, cwd: &str) -> Option<String> {
    use std::io::BufRead;
    let first = std::io::BufReader::new(std::fs::File::open(file).ok()?)
        .lines()
        .next()?
        .ok()?;
    let meta: serde_json::Value = serde_json::from_str(&first).ok()?;
    let payload = meta.get("payload")?;
    (payload.get("cwd")?.as_str()? == cwd)
        .then(|| payload.get("id")?.as_str().map(String::from))
        .flatten()
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

/// How much of the end of a Claude Code transcript is read to find where its last turn stands.
const TRANSCRIPT_TAIL: u64 = 512 * 1024;

/// Whether Claude Code is in the middle of a turn in `session`, read from the end of its
/// transcript under `home` for `cwd`. A turn is open from a prompt until the reply that ends it:
/// Claude Code writes a `turn_duration` entry when a turn is over, and its last assistant entry
/// of a finished turn has a stop reason other than `tool_use`. A prompt that only ran a local
/// command, an interrupted turn, and a transcript with no turn at all are between turns.
pub fn claude_turn_open(home: &Path, cwd: &str, session: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let path = claude_project_dir(home, cwd).join(format!("{session}.jsonl"));
    let Ok(mut file) = std::fs::File::open(&path) else {
        return false;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL);
    let mut tail = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut tail).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&tail);
    // A tail that starts mid-file starts mid-line; that first piece is not an entry.
    let lines = text.lines().skip(usize::from(start > 0));
    let mut open = false;
    for line in lines {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match entry["type"].as_str() {
            Some("system") if entry["subtype"] == "turn_duration" => open = false,
            Some("assistant") => {
                open = matches!(
                    entry["message"]["stop_reason"].as_str(),
                    None | Some("tool_use")
                )
            }
            Some("user") if entry["isMeta"] != true => open = !between_turns(&entry["message"]),
            _ => {}
        }
    }
    open
}

/// Whether a user entry leaves Claude Code between turns: a local command and its output, or the
/// note an interrupted turn ends with.
fn between_turns(message: &serde_json::Value) -> bool {
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
                    choose_session(&sessions, previous, claimed)
                });
                if let Some(session) = session {
                    sample.agent = Some(TermAgent {
                        name: kind.name().into(),
                        session,
                        home: home.map(|h| h.display().to_string()),
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
        let open = |lines: &[&str]| {
            std::fs::write(project.join(format!("{ID}.jsonl")), lines.join("\n")).unwrap();
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
        assert!(!claude_turn_open(&home, "/tmp/turns", "no-such-session"));
        let _ = std::fs::remove_dir_all(&home);
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
        assert_eq!(sessions, vec![ID.to_string(), older.to_string()]);
        assert!(claude_sessions(&home, "/tmp/other").is_empty());
        let none = HashSet::new();
        assert_eq!(choose_session(&sessions, None, &none).as_deref(), Some(ID));
        // Two agents in one directory keep one session each.
        assert_eq!(
            choose_session(&sessions, Some(older), &none).as_deref(),
            Some(older)
        );
        let taken = HashSet::from([ID.to_string()]);
        assert_eq!(
            choose_session(&sessions, None, &taken).as_deref(),
            Some(older)
        );
        let agent = TermAgent {
            name: "claude".into(),
            session: ID.into(),
            home: Some(home.display().to_string()),
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
        assert_eq!(codex_sessions(&home, "/tmp/p"), vec![ID.to_string()]);
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

#[cfg(test)]
mod runs_claude_tests {
    use super::runs_claude;

    #[test]
    fn finds_the_claude_agent_in_a_launch_command() {
        assert!(runs_claude("claude-agent-acp"));
        assert!(runs_claude("/opt/bin/claude-agent-acp --flag"));
        assert!(runs_claude("npx -y @acp/claude-agent-acp"));
        assert!(runs_claude("env X=1 node /x/claude-agent-acp"));
        assert!(!runs_claude("codex-acp"));
        assert!(!runs_claude(""));
    }
}
