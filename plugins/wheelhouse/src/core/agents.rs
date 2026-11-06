//! The agents Wheel runs: what each is doing, and starting a new one.
//!
//! State comes from `wheel agent watch`, which prints every agent pane as one JSON line and
//! prints the list again when the daemon announces a change, so nothing here polls. A view that
//! follows agents has [`Agents`] acquired for as long as a host is subscribed to it, and is told
//! each change through the hub (`View::agents_changed`).

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::watch;

use super::project::Origin;
use super::run;

/// One agent pane.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Agent {
    pub space: String,
    /// The directory of the space that holds the pane.
    pub cwd: String,
    pub pane: String,
    pub agent: String,
    pub harness: String,
    pub model: Option<String>,
    /// `idle`, `working`, `needs_you` (waiting on a permission) or `stopped`.
    pub status: Option<String>,
    /// What the agent declared it is working on (`work_set`).
    #[serde(default)]
    pub work: Option<Work>,
}

/// An agent's declaration: the items, the step and a note.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Work {
    #[serde(default)]
    pub items: Vec<WorkItem>,
    /// `plan`, `edit`, `build`, `verify`, `review` or `gate`.
    pub stage: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WorkItem {
    /// `bead`, `issue` or `pr`.
    pub kind: String,
    pub id: String,
}

/// The rows in `agents` whose space is in project `dir`, or in a worktree of it under
/// `.worktrees/<project name>/` (where `implement` and `swarm` work).
pub fn in_project(agents: &[Agent], dir: &str) -> Vec<Agent> {
    let dir = dir.trim_end_matches('/');
    let name = dir.rsplit('/').next().unwrap_or(dir);
    let worktrees = format!("/.worktrees/{name}/");
    agents
        .iter()
        .filter(|a| {
            let cwd = a.cwd.trim_end_matches('/');
            cwd == dir || (!name.is_empty() && format!("{cwd}/").contains(&worktrees))
        })
        .cloned()
        .collect()
}

/// A line of `wheel agent watch`.
pub fn parse_line(line: &str) -> Result<Vec<Agent>, String> {
    serde_json::from_str(line).map_err(|e| format!("wheel agent watch: {e}"))
}

/// The tool a view lists to let its page start an agent.
pub fn spawn_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "brief": { "type": "string", "description": "The agent's first message" },
            "agent": { "type": "string", "description": "Which agent to run, by name; the agent.default setting when absent" },
            "model": { "type": "string", "description": "The model to run it on" },
        },
        "required": ["brief"],
    });
    Tool::new("spawn_agent", "Start an agent in a new pane beside the asking pane and give it a brief", schema.as_object().unwrap().clone())
}

#[derive(Default)]
struct State {
    agents: Vec<Agent>,
    /// Whether a list has arrived in this run: the first one counts as a change even when empty.
    loaded: bool,
    users: usize,
    /// Changes with every start and stop, so a following thread can tell it is out of date.
    run: u64,
    child: Option<Child>,
}

struct Shared {
    /// The `wheel` command.
    program: String,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

/// The agents the daemon knows, followed while some view needs them.
#[derive(Clone)]
pub struct Agents {
    shared: Arc<Shared>,
}

impl Agents {
    pub fn new(program: &str) -> Self {
        Agents {
            shared: Arc::new(Shared { program: program.into(), state: Mutex::default(), changed: watch::channel(0).0 }),
        }
    }

    /// A view that follows agents is shown: start following if nobody was.
    pub fn acquire(&self) {
        let mut s = self.shared.state.lock().unwrap();
        s.users += 1;
        if s.users == 1 {
            s.run += 1;
            let (run, shared) = (s.run, self.shared.clone());
            std::thread::spawn(move || shared.follow(run));
        }
    }

    /// A view that follows agents is no longer shown: stop following with the last one.
    pub fn release(&self) {
        let mut s = self.shared.state.lock().unwrap();
        s.users = s.users.saturating_sub(1);
        if s.users == 0 {
            s.run += 1;
            if let Some(mut child) = s.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            s.agents.clear();
            s.loaded = false;
        }
    }

    /// The process is ending: stop following, whoever still wants the list.
    pub fn shutdown(&self) {
        self.shared.state.lock().unwrap().users = 1;
        self.release();
    }

    /// Every agent pane, as of the last change.
    pub fn all(&self) -> Vec<Agent> {
        self.shared.state.lock().unwrap().agents.clone()
    }

    /// Wakes on every change of the list.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.shared.changed.subscribe()
    }

    /// Run the `spawn_agent` tool: split the asking pane, start an agent in the new pane and give
    /// it the brief. The new pane takes no focus. The answer is its pane id.
    pub fn spawn(&self, origin: &Origin, args: &Map<String, Value>) -> CallToolResult {
        let text = |k: &str| args.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        let result = match text("brief") {
            None => Err("spawn_agent needs a brief".to_string()),
            Some(brief) => self.start(origin, brief, text("agent"), text("model")),
        };
        match result {
            Ok(pane) => CallToolResult::success(vec![ContentBlock::text(pane)]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }

    fn start(&self, origin: &Origin, brief: &str, agent: Option<&str>, model: Option<&str>) -> Result<String, String> {
        let (program, dir) = (self.shared.program.as_str(), origin.dir.as_str());
        if origin.pane == "default" {
            return Err("spawn_agent needs the asking pane (_meta wheel/pane)".into());
        }
        let out = run::text(program, dir, &["pane", "split", &origin.pane])?;
        let pane = out.split_whitespace().next().ok_or("wheel pane split printed no pane id")?.to_string();
        let mut start = vec!["pane", "start", &pane, "agent"];
        for (flag, value) in [("--agent", agent), ("--model", model)] {
            if let Some(v) = value {
                start.extend([flag, v]);
            }
        }
        // The brief goes as a message the call does not wait out: `pane start --message` and
        // `agent send` both return only when the agent's turn has ended, and `agent send` exits 3
        // when its wait runs out first, the message having been sent.
        let sent = run::text(program, dir, &start)
            .and_then(|_| run::text_or_exit(program, dir, &["agent", "send", &pane, brief, "--timeout", "1"], &[3]));
        match sent {
            Ok(_) => Ok(pane),
            Err(e) => {
                let _ = run::text(program, dir, &["pane", "close", &pane]);
                Err(e)
            }
        }
    }
}

impl Shared {
    /// Store `agents` for run `run`, unless that run is over; announce it when it differs.
    fn set(&self, run: u64, agents: Vec<Agent>) {
        let mut s = self.state.lock().unwrap();
        if s.run == run && (!s.loaded || s.agents != agents) {
            s.agents = agents;
            s.loaded = true;
            drop(s);
            self.changed.send_modify(|n| *n += 1);
        }
    }

    fn current(&self, run: u64) -> bool {
        self.state.lock().unwrap().run == run
    }

    /// Follow `wheel agent watch` until run `run` is over; start it again when it ends.
    fn follow(&self, run: u64) {
        while self.current(run) {
            let spawned = Command::new(&self.program)
                .args(["agent", "watch"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = spawned {
                let out = child.stdout.take();
                {
                    let mut s = self.state.lock().unwrap();
                    if s.run != run {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    s.child = Some(child);
                }
                for line in BufReader::new(out.unwrap()).lines().map_while(Result::ok) {
                    if let Ok(agents) = parse_line(&line) {
                        self.set(run, agents);
                    }
                }
                let mut s = self.state.lock().unwrap();
                if s.run == run {
                    if let Some(mut child) = s.child.take() {
                        let _ = child.wait();
                    }
                    drop(s);
                    // The daemon went away: its agents are not known to be running.
                    self.set(run, vec![]);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(space: &str, cwd: &str, pane: &str, status: &str) -> Value {
        json!({ "space": space, "cwd": cwd, "pane": pane, "agent": "w1", "harness": "claude", "model": "sonnet", "status": status })
    }

    #[test]
    fn a_line_is_parsed_and_a_project_keeps_its_own_rows() {
        let line = json!([row("wheel", "/p/wheel", "p1", "working"), row("other", "/p/other", "p2", "idle")]).to_string();
        let all = parse_line(&line).unwrap();
        assert_eq!(all.len(), 2);
        let mine = in_project(&all, "/p/wheel/");
        assert_eq!(mine.len(), 1);
        assert_eq!((mine[0].pane.as_str(), mine[0].status.as_deref()), ("p1", Some("working")));
        assert!(parse_line("not json").is_err());
    }
}
