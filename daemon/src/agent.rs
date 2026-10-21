//! The agent interface: what an agent pane needs from the program behind it, and nothing about
//! which program that is. A [`Launch`] starts one agent process in one conversation. The
//! [`Agent`] it returns takes prompts, cancels, permission answers and option changes; its event
//! stream carries the reply as it streams, tool calls, the permission requests the agent raises,
//! how full its context window is and the end of each turn, and ends with [`AgentEvent::Exited`]
//! when the process does. Dropping the [`Agent`] kills the process and everything it started.
//!
//! Claude Code is the first program behind it ([`crate::claude`]).

use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};
use wheel_proto::{ConfigOption, ContextUse, PermOption, SessionEvent};

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
}

/// How one agent process starts.
pub struct Launch {
    /// The program, and any arguments of its own before the ones the driver adds.
    pub command: String,
    pub cwd: PathBuf,
    /// The agent's configuration directory, when it is not the one the daemon's environment names.
    pub config_dir: Option<PathBuf>,
    /// Appended to the agent's system prompt: how to use Wheel (`crate::brief`).
    pub brief: Option<String>,
    /// Added to the process's environment: where `wheel` reaches the daemon, and the pane.
    pub env: Vec<(String, String)>,
    pub conversation: Conversation,
}

/// What an agent process tells its session.
#[derive(Debug)]
pub enum AgentEvent {
    /// The process is up and in its conversation; `options` are the controls it offers.
    Ready { options: Vec<ConfigOption> },
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
    /// How full the conversation's context window is.
    Context(ContextUse),
    /// The agent changed its own mode, leaving plan mode, say.
    Mode(String),
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
    crate::claude::launch(launch)
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
