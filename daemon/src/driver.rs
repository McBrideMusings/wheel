//! The agent interface: what an agent pane needs from the program behind it, and nothing about
//! which program that is. A [`Launch`] starts one agent process in one conversation. The
//! [`DriverHandle`] it returns takes prompts, cancels, permission answers and option changes; its event
//! stream carries the reply as it streams, tool calls, the permission requests the agent raises,
//! how full its context window is and the end of each turn, and ends with [`DriverEvent::Exited`]
//! when the process does. Dropping the [`DriverHandle`] kills the process and everything it started.
//!
//! Claude Code ([`crate::claude`]), Codex ([`crate::codex`]) and pi ([`crate::pi`]) are the programs
//! behind it.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use wheel_proto::{AgentPermissions, ConfigOption, ContextUse, PermOption, AgentEvent};

use crate::claude::History;
use crate::resume::Harness;

/// Which conversation a process starts in, by the agent's own id for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Conversation {
    /// A fresh conversation under this id.
    New(String),
    /// A past conversation the agent keeps.
    Resume(String),
}

impl Conversation {
    pub fn id(&self) -> &str {
        match self {
            Conversation::New(id) | Conversation::Resume(id) => id,
        }
    }

    /// The same kind of start under the id the agent chose for it.
    pub fn named(&self, id: String) -> Conversation {
        match self {
            Conversation::New(_) => Conversation::New(id),
            Conversation::Resume(_) => Conversation::Resume(id),
        }
    }
}

/// How one agent process starts.
pub struct Launch {
    /// Which program `command` is.
    pub agent: Harness,
    /// The program, and any arguments of its own before the ones the driver adds.
    pub command: String,
    pub cwd: PathBuf,
    /// The agent's configuration directory (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
    /// `PI_CODING_AGENT_DIR`), when it is not the one the daemon's environment names.
    pub config_dir: Option<PathBuf>,
    /// Added to the agent's instructions: how to use Wheel (`crate::brief`).
    pub brief: Option<String>,
    /// Added to the process's environment: where `wheel` reaches the daemon, and the pane.
    pub env: Vec<(String, String)>,
    pub conversation: Conversation,
    /// The model to run, in the agent's own words; None leaves the agent's default.
    pub model: Option<String>,
    /// The permission mode to start in, in the agent's own terms, and Codex's sandbox; a field
    /// left unset leaves the agent's default.
    pub permissions: AgentPermissions,
    /// The agent is the orchestrator: it gets only the `wheel` commands it may run and
    /// no other tool (`crate::orchestrator`).
    pub orchestrator: bool,
}

/// Codex's sandbox values.
pub const CODEX_SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

/// Refuse a permission mode or sandbox `agent` does not have, naming the ones it does.
pub fn check_permissions(
    agent: Harness,
    permission_mode: Option<&str>,
    sandbox: Option<&str>,
) -> Result<(), String> {
    let (modes, sandboxes): (Vec<&str>, Vec<&str>) = match agent {
        Harness::Claude => (crate::claude::start_modes(), Vec::new()),
        Harness::Codex => (crate::codex::policy_names(), CODEX_SANDBOXES.to_vec()),
        Harness::Pi => (Vec::new(), Vec::new()),
    };
    let check = |what: &str, value: Option<&str>, valid: &[&str]| match value {
        Some(_) if valid.is_empty() => Err(format!("{} has no {what}", agent.title())),
        Some(v) if !valid.contains(&v) => Err(format!(
            "{} does not take {what} {v}; it takes {}",
            agent.title(),
            valid.join(", ")
        )),
        _ => Ok(()),
    };
    check("permission mode", permission_mode, &modes)?;
    check("sandbox", sandbox, &sandboxes)
}

/// What an agent process tells its agent.
#[derive(Debug)]
pub enum DriverEvent {
    /// The process is up and in its conversation; `options` are the controls it offers.
    /// `conversation` is the agent's own id for it when the agent chose one, which is the case when
    /// it was not told one to use; `history` is what a resumed conversation held, when the agent
    /// hands that over rather than leaving it in a store wheel reads.
    Ready {
        options: Vec<ConfigOption>,
        conversation: Option<String>,
        history: Option<History>,
    },
    /// Part of the conversation as a client draws it: text, a thought, a tool call or its update,
    /// a plan.
    Transcript(AgentEvent),
    /// The agent asks to run a tool. `request` is the agent's id for the question, which
    /// [`DriverInput::answer`] takes back.
    Permission {
        request: String,
        title: String,
        tool_call: Option<String>,
        options: Vec<PermOption>,
        why: wheel_proto::PermWhy,
    },
    /// How full the conversation's context window is; `None` when the agent has no figure, as pi
    /// has none right after compaction, which clears what was shown.
    Context(Option<ContextUse>),
    /// The agent changed its own mode, leaving plan mode, say.
    Mode(String),
    /// The slash commands the agent accepts, by name, as it lists them when it starts.
    Commands(Vec<String>),
    /// The agent moved its running process into a fresh conversation, as `/clear` does; `id` is
    /// its id for the new one. The transcript so far no longer belongs to the conversation.
    Conversation { id: String },
    /// The agent changed its controls on its own.
    Options(Vec<ConfigOption>),
    /// The agent began a turn no prompt of wheel's started, as Claude Code does when a subagent or
    /// a background task finishes. Its end arrives as `TurnEnd` or `TurnFailed`.
    TurnStart,
    /// The background tasks the agent has running now: commands and other tasks, and subagents. The
    /// agent sends both counts each time they change; it works on after its turn ends while
    /// either is above zero.
    Background { shells: u32, agents: u32 },
    /// The agent refused the message [`DriverInput::steer`] sent as `turn` (no turn was running to
    /// take it, say); it was not taken in and goes back to the queue.
    SteerRefused { turn: u64 },
    /// The agent took in the message [`DriverInput::steer`] sent as `turn`: it joins the turn
    /// running, or starts a turn of its own when that one had ended first.
    Steered { turn: u64 },
    /// The running turn is over; `stop_reason` is `end_turn`, `cancelled` or the agent's own reason.
    TurnEnd { stop_reason: String },
    /// The running turn ended in an error.
    TurnFailed { message: String },
    /// The process ended: `None` when it exited cleanly, else why it stopped.
    Exited { error: Option<String> },
}

/// What an agent sends its agent process.
pub(crate) enum Input {
    Prompt(String),
    /// A message written while a turn runs, for an agent that takes input mid-turn.
    Steer { turn: u64, text: String },
    /// Take back steered messages the agent has not taken in yet.
    Withdraw { turns: Vec<u64> },
    /// Make a steered message the agent has not taken in yet interrupt the running turn.
    SendNow { turn: u64 },
    Cancel,
    /// `choice` is the option id the person picked, or none when the request was withdrawn.
    Answer {
        request: String,
        choice: Option<String>,
    },
    SetOption {
        id: String,
        value: String,
        reply: oneshot::Sender<Result<Vec<ConfigOption>, String>>,
    },
}

/// The sending half of an agent process, cheap to clone into a task that answers for it later.
#[derive(Clone)]
pub struct DriverInput(pub(crate) mpsc::UnboundedSender<Input>);

impl DriverInput {
    fn put(&self, input: Input) -> Result<(), String> {
        self.0
            .send(input)
            .map_err(|_| "the agent process has stopped".to_string())
    }

    /// Start a turn with `text`. The agent runs one turn at a time; the agent sends the next
    /// only once this one has ended.
    pub fn prompt(&self, text: String) -> Result<(), String> {
        self.put(Input::Prompt(text))
    }

    /// Hand the agent `text` while a turn runs, as its terminal does when a message is typed
    /// mid-turn: the agent takes it in after the tool call it is on, into the same turn, and
    /// answers both with one `TurnEnd`. It reports taking it in as `Steered { turn }`. Only for a
    /// handle whose `steers` is true.
    pub fn steer(&self, turn: u64, text: String) -> Result<(), String> {
        self.put(Input::Steer { turn, text })
    }

    /// Take back the steered messages of `turns` the agent has not taken in; a turn it already
    /// took in, or never got, is left as it is.
    pub fn withdraw(&self, turns: Vec<u64>) -> Result<(), String> {
        self.put(Input::Withdraw { turns })
    }

    /// Have the steered message of `turn` run at once, unless the agent already took it in: it
    /// interrupts the running turn, as the agent's terminal does for a message sent with its force
    /// key.
    pub fn send_now(&self, turn: u64) -> Result<(), String> {
        self.put(Input::SendNow { turn })
    }

    /// Stop the running turn; it still ends with a `TurnEnd`.
    pub fn cancel(&self) -> Result<(), String> {
        self.put(Input::Cancel)
    }

    /// Answer permission request `request` with the option the person picked, or withdraw it.
    pub fn answer(&self, request: String, choice: Option<String>) -> Result<(), String> {
        self.put(Input::Answer { request, choice })
    }

    /// Change one option. The answer is the agent's whole list as it stands after the change.
    pub async fn set_option(&self, id: String, value: String) -> Result<Vec<ConfigOption>, String> {
        let (reply, answer) = oneshot::channel();
        self.put(Input::SetOption { id, value, reply })?;
        answer
            .await
            .map_err(|_| "the agent process stopped before it answered".to_string())?
    }
}

/// One running agent process.
pub struct DriverHandle {
    pub input: DriverInput,
    pub events: mpsc::UnboundedReceiver<DriverEvent>,
    /// Whether the agent takes a message while a turn runs ([`DriverInput::steer`]). Codex and pi
    /// run one turn at a time, so a message for them waits in the queue until the turn ends.
    pub steers: bool,
    /// Whether a steered message the agent has not taken in yet can be taken back
    /// ([`DriverInput::withdraw`]). Only then is a message steered the moment it is sent; without
    /// it a message waits in the queue, where Remove and Edit work, until Send now steers it.
    pub withdraws: bool,
    /// Kills the process group when the agent is dropped.
    pub(crate) _group: ProcessGroup,
}

/// Start an agent process.
pub fn launch(launch: Launch) -> Result<DriverHandle, String> {
    match launch.agent {
        Harness::Claude => crate::claude::launch(launch),
        Harness::Codex => crate::codex::launch(launch),
        Harness::Pi => crate::pi::launch(launch),
    }
}

/// An agent process just started: its pipes, and the last line it wrote to stderr.
pub(crate) struct Process {
    pub pid: u32,
    pub child: tokio::process::Child,
    pub stdin: tokio::process::ChildStdin,
    pub stdout: BufReader<tokio::process::ChildStdout>,
    pub last_error: Arc<Mutex<String>>,
}

/// Start `launch.command` with `args` after its own arguments, as the leader of a process group
/// of its own, in `launch.cwd`, with the configuration directory in `home_var` and `launch.env`.
pub(crate) fn spawn(launch: &Launch, args: Vec<String>, home_var: &str) -> Result<Process, String> {
    let mut words = crate::claude::words(&launch.command).into_iter();
    let program = words
        .next()
        .ok_or_else(|| "the agent command is empty".to_string())?;
    let mut command = tokio::process::Command::new(&program);
    command
        .args(words)
        .args(args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    if let Some(dir) = &launch.config_dir {
        command.env(home_var, dir);
    }
    // A pane learns its space from the daemon, never from an inherited variable.
    command.env_remove("WHEEL_SPACE");
    command.env_remove("WHEEL_SPACE_DIR");
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    let pid = child.id().ok_or("the agent exited as it started")?;
    tracing::info!(pid, %program, conversation = ?launch.conversation, cwd = %launch.cwd.display(), config_dir = ?launch.config_dir, "agent process started");
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let last_error = Arc::new(Mutex::new(String::new()));
    let stderr_seen = last_error.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(pid, line = %line, "agent stderr");
            if !line.trim().is_empty() {
                *stderr_seen.lock().unwrap() = line;
            }
        }
    });
    Ok(Process {
        pid,
        child,
        stdin,
        stdout: BufReader::new(stdout),
        last_error,
    })
}

/// Why an agent process ended: `failed` when this side gave up on it, else its exit status with the
/// last line it wrote to stderr. `None` when it exited cleanly.
pub(crate) async fn exit_error(
    mut child: tokio::process::Child,
    failed: Option<String>,
    last_error: &Mutex<String>,
) -> Option<String> {
    // A process this side gave up on may still be running.
    if failed.is_some() {
        let _ = child.start_kill();
    }
    let status = child.wait().await;
    match (failed, status) {
        (Some(e), _) => Some(e),
        (None, Ok(status)) if status.success() => None,
        (None, Ok(status)) => {
            use std::os::unix::process::ExitStatusExt;
            // A shell reports a signal death as 128 plus the signal, and so do agents that catch
            // the signal and exit; the last stderr line says nothing about either.
            let signal = status.signal().or_else(|| {
                status
                    .code()
                    .filter(|code| [129, 130, 131, 134, 137, 139, 141, 143].contains(code))
                    .map(|code| code - 128)
            });
            if let Some(signal) = signal {
                return Some(format!("the agent was killed ({})", signal_name(signal)));
            }
            let said = last_error.lock().unwrap().clone();
            Some(match (status.code(), said.is_empty()) {
                (Some(code), true) => format!("the agent exited with status {code}"),
                (Some(code), false) => format!("the agent exited with status {code}: {said}"),
                (None, _) => "the agent stopped without an exit status".to_string(),
            })
        }
        (None, Err(e)) => Some(format!("waiting for the agent failed: {e}")),
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGQUIT => "SIGQUIT".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGPIPE => "SIGPIPE".into(),
        libc::SIGTERM => "SIGTERM".into(),
        other => format!("signal {other}"),
    }
}

/// A process group, killed when this is dropped: the agent and every tool it started.
pub(crate) struct ProcessGroup(pub(crate) u32);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // The group id is the agent's pid: it is started as the leader of a group of its own.
        unsafe {
            libc::killpg(self.0 as libc::pid_t, libc::SIGKILL);
        }
        tracing::info!(pid = self.0, "agent process group killed");
    }
}

#[cfg(test)]
mod exit_tests {
    use super::*;

    async fn exit_of(script: &str) -> Option<String> {
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", script])
            .spawn()
            .unwrap();
        exit_error(child, None, &Mutex::new("a trust warning".into())).await
    }

    #[tokio::test]
    async fn a_signal_death_is_named_not_blamed_on_the_last_stderr_line() {
        assert_eq!(
            exit_of("kill -TERM $$").await.as_deref(),
            Some("the agent was killed (SIGTERM)")
        );
        assert_eq!(
            exit_of("exit 143").await.as_deref(),
            Some("the agent was killed (SIGTERM)")
        );
        assert_eq!(
            exit_of("exit 3").await.as_deref(),
            Some("the agent exited with status 3: a trust warning")
        );
    }
}

/// The options that answer a question an agent put to the person: one per choice, with its
/// description, id `pick:<index>` and kind `answer`, then Skip, which refuses the question.
pub(crate) fn question_options(choices: impl IntoIterator<Item = (String, Option<String>)>) -> Vec<PermOption> {
    let mut options: Vec<PermOption> = choices
        .into_iter()
        .enumerate()
        .map(|(i, (name, description))| PermOption {
            id: format!("pick:{i}"),
            name,
            kind: "answer".into(),
            description: description.filter(|d| !d.trim().is_empty()),
        })
        .collect();
    options.push(PermOption { id: "deny".into(), name: "Skip".into(), kind: "reject_once".into(), description: None });
    options
}

/// The index of the choice an answer to [`question_options`] picked; none for Skip or a withdrawn
/// request.
pub(crate) fn picked(choice: Option<&str>) -> Option<usize> {
    choice?.strip_prefix("pick:")?.parse().ok()
}

#[cfg(test)]
mod question_tests {
    use super::*;

    #[test]
    fn a_question_offers_each_choice_then_skip_and_a_pick_names_its_index() {
        let options = question_options([("Red".to_string(), Some("Warm".to_string())), ("Blue".to_string(), Some(" ".to_string()))]);
        let offered: Vec<(&str, &str, &str, Option<&str>)> = options
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str(), o.kind.as_str(), o.description.as_deref()))
            .collect();
        assert_eq!(
            offered,
            [
                ("pick:0", "Red", "answer", Some("Warm")),
                ("pick:1", "Blue", "answer", None),
                ("deny", "Skip", "reject_once", None),
            ]
        );
        assert_eq!(picked(Some("pick:1")), Some(1));
        assert_eq!(picked(Some("deny")), None);
        assert_eq!(picked(None), None);
    }
}
