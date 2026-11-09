//! The voice orchestrator (ADR-0002's exception): an agent that talks for a whole space, or for the
//! whole daemon, and is no pane's agent. One runs per scope, started by `voice_start` and ended by
//! `voice_stop`. A client reaches it as it reaches any agent: `agent_attach` for the transcript
//! stream, `agent_send` for what the person said.
//!
//! It runs Claude Code on a fast model with the speech brief below and one tool, Bash, limited to
//! the `wheel` commands in [`ALLOWED`]: it reads state, sends to the pane agents that do the work,
//! stops a turn and answers a permission request. It has no shell and no file tools.

use crate::server::AppState;

/// The `wheel` commands the orchestrator may run; every other command is refused.
pub(crate) const ALLOWED: &[&str] = &[
    "Bash(wheel status:*)",
    "Bash(wheel space ls:*)",
    "Bash(wheel space focus:*)",
    "Bash(wheel pane ls:*)",
    "Bash(wheel pane near:*)",
    "Bash(wheel pane focus:*)",
    "Bash(wheel agent read:*)",
    "Bash(wheel agent send:*)",
    "Bash(wheel agent cancel:*)",
    "Bash(wheel perm list:*)",
    "Bash(wheel perm answer:*)",
];

/// The model the orchestrator runs on, in Claude Code's words.
const MODEL: &str = "haiku";

/// The scope that means the whole daemon.
pub(crate) const DAEMON: &str = "daemon";

/// The agent name of the orchestrator for `scope`.
pub(crate) fn agent_name(scope: &str) -> String {
    format!("voice-{scope}")
}

/// What the orchestrator is told, appended to its system prompt.
pub(crate) fn brief(scope: &str, space_name: Option<&str>) -> String {
    let scope_line = match space_name {
        Some(name) => format!("Your scope is the space {scope} (\"{name}\"): its panes and the agents in them."),
        None => "Your scope is the whole daemon: every space, pane and agent.".to_string(),
    };
    format!(
        "You are the voice orchestrator for Wheel, a Mac app that holds projects as spaces, each a grid \
         of panes with agents in them. The person is talking to you, not typing, and your replies are \
         spoken aloud.\n\
         {scope_line}\n\
         Speak in short sentences. Never use markdown, code, file paths read out letter by letter, or \
         lists. Summarize; never quote a transcript.\n\
         You route and summarize and never do the work. Real work goes to a pane's agent: send it with \
         `wheel agent send <agent> \"<message>\"`. Your only tool is Bash, and it runs only these \
         commands: `wheel status`, `wheel space ls`, `wheel space focus`, `wheel pane ls`, `wheel pane \
         near`, `wheel pane focus`, `wheel agent read`, `wheel agent send`, `wheel agent cancel`, \
         `wheel perm list` and `wheel perm answer`. Anything else is refused. Never edit files or run a \
         shell.\n\
         Before a call that may take a moment, say one short word such as \"checking\". After you act, \
         say in one sentence what you did, for example \"approved the test run in auth\".\n\
         Read an agent's transcript only when asked about that agent."
    )
}

impl AppState {
    /// Start the orchestrator for `scope` (a space id, or `daemon`), or return the one already
    /// running. Returns its agent's name.
    pub(crate) fn voice_start(&self, scope: &str) -> Result<String, String> {
        let name = agent_name(scope);
        if self.agents.lock().unwrap().contains_key(&name) {
            return Ok(name);
        }
        let (cwd, space_name) = if scope == DAEMON {
            (std::env::var("HOME").unwrap_or_else(|_| "/".into()), None)
        } else {
            let spaces = self.spaces.lock().unwrap();
            let space = spaces
                .spaces
                .iter()
                .find(|s| s.id == scope)
                .ok_or_else(|| format!("no space {scope}"))?;
            (space.cwd.clone(), Some(space.name.clone()))
        };
        self.spawn_voice_agent(&name, &cwd, brief(scope, space_name.as_deref()), MODEL)?;
        Ok(name)
    }

    /// End the orchestrator for `scope` and its agent process.
    pub(crate) fn voice_stop(&self, scope: &str) -> Result<(), String> {
        let name = agent_name(scope);
        if !self.agents.lock().unwrap().contains_key(&name) {
            return Err(format!("no voice orchestrator is running for {scope}"));
        }
        self.end_agent(&name);
        Ok(())
    }
}
