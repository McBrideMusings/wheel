//! The agent interface: what an agent pane needs from the program behind it, and nothing about
//! which program that is. A [`Launch`] starts one agent process in one conversation. The
//! [`Agent`] it returns takes prompts, cancels, permission answers and option changes; its event
//! stream carries the reply as it streams, tool calls, the permission requests the agent raises,
//! how full its context window is and the end of each turn, and ends with [`AgentEvent::Exited`]
//! when the process does. Dropping the [`Agent`] kills the process and everything it started.
//!
//! Claude Code ([`crate::claude`]), Codex ([`crate::codex`]) and pi ([`crate::pi`]) are the programs
//! behind it.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use wheel_proto::{ConfigOption, ContextUse, PermOption, SessionEvent};

use crate::claude::History;
use crate::resume::Agent as Harness;

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
}

/// What an agent process tells its session.
#[derive(Debug)]
pub enum AgentEvent {
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
    Transcript(SessionEvent),
    /// The agent asks to run a tool. `request` is the agent's id for the question, which
    /// [`AgentInput::answer`] takes back.
    Permission {
        request: String,
        title: String,
        tool_call: Option<String>,
        options: Vec<PermOption>,
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
    /// The running turn is over; `stop_reason` is `end_turn`, `cancelled` or the agent's own reason.
    TurnEnd { stop_reason: String },
    /// The running turn ended in an error.
    TurnFailed { message: String },
    /// The process ended: `None` when it exited cleanly, else why it stopped.
    Exited { error: Option<String> },
}

/// What a session sends its agent process.
pub(crate) enum Input {
    Prompt(String),
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
pub struct AgentInput(pub(crate) mpsc::UnboundedSender<Input>);

impl AgentInput {
    fn put(&self, input: Input) -> Result<(), String> {
        self.0
            .send(input)
            .map_err(|_| "the agent process has stopped".to_string())
    }

    /// Start a turn with `text`. The agent runs one turn at a time; the session sends the next
    /// only once this one has ended.
    pub fn prompt(&self, text: String) -> Result<(), String> {
        self.put(Input::Prompt(text))
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
pub struct Agent {
    pub input: AgentInput,
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    /// Kills the process group when the agent is dropped.
    pub(crate) _group: ProcessGroup,
}

/// Start an agent process.
pub fn launch(launch: Launch) -> Result<Agent, String> {
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
