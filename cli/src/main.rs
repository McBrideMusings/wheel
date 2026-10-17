//! `wheel`: drive a running `wheeld` from the command line. Every command speaks the same
//! WebSocket protocol the apps use, so anything a client can do, a script can check.
//!
//! Exit codes: 0 success, 2 bad arguments, 3 timed out before `--until` matched,
//! 4 the terminal exited before `--until` matched, 5 could not connect or authenticate,
//! 6 the daemon refused the request, 7 lost the connection after connecting (the request may
//! already have taken effect), 1 anything else.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    decode_bytes, encode_bytes, ArtifactScope, ClientMsg, ConfigKind, Dir, NewPaneKind, ServerMsg,
    SessionEvent, SessionState, SettingInfo, SettingOrigin, StartKind, TimerState, ToolContent,
};

#[derive(Parser)]
#[command(name = "wheel", about = "Drive a running wheeld")]
struct Cli {
    /// Daemon WebSocket URL.
    #[arg(long, global = true, default_value_t = format!("ws://{}/ws", wheel_proto::DEFAULT_LISTEN))]
    url: String,
    /// File holding the daemon's client token. Defaults to ~/Library/Application Support/wheel/token.
    #[arg(long, global = true)]
    token_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the daemon's state as JSON.
    Status,
    /// Terminals.
    #[command(subcommand)]
    Term(TermCommand),
    /// The lead agent: the agent session named `lead`.
    #[command(subcommand)]
    Lead(LeadCommand),
    /// Agent sessions, by the name their agent pane carries (e.g. `w5`).
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Permission requests waiting on a person.
    #[command(subcommand)]
    Perm(PermCommand),
    /// Spaces: one per project, each a split tree of panes.
    #[command(subcommand)]
    Space(SpaceCommand),
    /// Panes inside a space.
    #[command(subcommand)]
    Pane(PaneCommand),
    /// Artifacts: a folder with an artifact.toml manifest that a web pane can show.
    #[command(subcommand)]
    Artifact(ArtifactCommand),
    /// Wheel's own settings, from wheel.toml: a project's over this machine's.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// The lead's inbox: what waits on an agent pane.
    #[command(subcommand)]
    Inbox(InboxCommand),
    /// Supervisors: panes that pursue a goal epic or a backlog focus with their own agent.
    #[command(subcommand)]
    Supervisor(SupervisorCommand),
}

#[derive(Subcommand)]
enum SupervisorCommand {
    /// Start a supervisor in a new pane of a space and print it as JSON. Its agent is woken with
    /// the scope's ready beads; `show` prints the queue once the agent has answered.
    Start {
        /// The epic whose descendants it works.
        #[arg(long)]
        goal: Option<String>,
        /// A bead label; only beads carrying it are in scope. Give a goal, a focus, or both.
        #[arg(long)]
        focus: Option<String>,
        /// The space it runs in. Defaults to the space being shown.
        #[arg(long)]
        space: Option<String>,
        /// How many workers it runs at once. Defaults to 2.
        #[arg(long)]
        workers: Option<u32>,
        /// The share of the usage limit, 1 to 100, it may spend before it holds. Defaults to none.
        #[arg(long)]
        cap: Option<u32>,
    },
    /// Pause a running or held supervisor. Prints it as JSON.
    Pause { id: String },
    /// Resume a paused or held supervisor. Prints it as JSON.
    Resume { id: String },
    /// Stop a supervisor for good: its agent ends and its pane shows it as stopped. Prints it as JSON.
    Stop { id: String },
    /// Set how many workers a supervisor runs at once (1 or more). Prints it as JSON.
    Workers { id: String, workers: u32 },
    /// Set a supervisor's usage cap (1 to 100), or remove it when none is given. Prints it as JSON.
    Cap { id: String, cap: Option<u32> },
    /// Print every supervisor in every space as JSON.
    List,
    /// Print one supervisor as JSON: scope, workers, cap, state, queue, its agent's state, the
    /// workers running now and how many items wait in its inbox.
    Show { id: String },
    /// Print the decisions the supervisor's loop has made as JSON, oldest first: time, worker, bead,
    /// kind and the raw detail.
    Events {
        id: String,
        /// Only the last this many.
        #[arg(long)]
        last: Option<u32>,
    },
    /// Print what waits on the supervisor's agent as JSON, oldest first: a blocked bead, or a
    /// worker the loop gave up on.
    Inbox { id: String },
    /// Print the permission policy that answers the supervisor's workers' requests as JSON: what
    /// it approves, and the project commands it reads from the project's wheel.toml.
    Policy { id: String },
    /// Show a running worker in a pane of its own beside the supervisor's; prints the new pane's
    /// id. Closing that pane leaves the worker running.
    Push {
        id: String,
        /// The worker's session name, as `show` lists it (`worker-<bead>`).
        session: String,
    },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Print every item as JSON, the open ones first: id, source, kind, time, state and detail.
    List,
    /// Close an item: nothing waits on it any more. Prints the item as JSON.
    Close { id: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print every setting, its value, and which wheel.toml that value came from.
    Ls {
        /// The space whose wheel.toml is the project layer; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print one setting's value in force.
    Get {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        #[arg(long)]
        space: Option<String>,
        /// Print the record as JSON rather than the bare value.
        #[arg(long)]
        json: bool,
    },
    /// Write one setting, into a project's wheel.toml or this machine's.
    Set {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        /// The value to write.
        value: String,
        /// Write the wheel.toml of this space's directory; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Write the machine-wide wheel.toml in the instance directory instead.
        #[arg(long)]
        machine: bool,
        /// Print the settings as JSON once written.
        #[arg(long)]
        json: bool,
    },
    /// Wait for the next settings broadcast, the one every connection hears after a write or a
    /// removal; print the space it was resolved for and each setting's value and origin.
    Watch {
        /// Seconds to wait (0 to 3600); exit 3 if no broadcast arrives.
        #[arg(long, default_value_t = 10.0, value_parser = parse_timeout)]
        timeout: f64,
        /// Print the broadcast as JSON: `{"space": ..., "settings": [...]}`.
        #[arg(long)]
        json: bool,
    },
    /// Remove one setting from a project's wheel.toml or this machine's, so the layer below is
    /// in force: this machine's value under a project's, wheel's default under the machine's.
    Unset {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        /// Change the wheel.toml of this space's directory; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Change the machine-wide wheel.toml in the instance directory instead.
        #[arg(long)]
        machine: bool,
        /// Print the settings as JSON once removed.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ArtifactCommand {
    /// Make an artifact — its folder, an artifact.toml and an index.html — and print the folder's
    /// absolute path.
    New {
        /// One plain name, never a path.
        name: String,
        /// Write `pane = true` into the manifest: offer this artifact in a pane picker. Leave it
        /// off for a one-off nobody needs to find in a menu.
        #[arg(long)]
        pane: bool,
        /// Which root to put it under: repo (committed with the project, the default), local (this
        /// checkout only, gitignored) or shared (every project on this machine).
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        /// The space whose directory the repo and local roots sit under; the active space with none.
        #[arg(long)]
        space: Option<String>,
    },
    /// Print every artifact of every scope, with its folder, title and any problems.
    Ls {
        #[arg(long)]
        space: Option<String>,
        /// Only the artifacts whose manifest says `pane = true`: what a pane picker offers.
        #[arg(long)]
        pane: bool,
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run the refresh command of the artifact a web pane shows, now, and print the run. Waits for
    /// a person to answer the permission request the first run of a command raises. Exits 1 when
    /// the run failed — a non-zero exit status, a refusal, or a command that would not start.
    Refresh {
        /// The web pane to refresh for; its data directory is what the command writes.
        pane: String,
        /// Print the runner as JSON.
        #[arg(long)]
        json: bool,
        /// Seconds to wait for the run, a person's answer included (0 to 3600).
        #[arg(long, default_value_t = 120.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the commands a person approved with "always", or forget one so it asks again.
    Trust {
        /// Forget every approval remembered for this artifact name.
        #[arg(long, value_name = "NAME")]
        forget: Option<String>,
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print one artifact's record.
    Show {
        name: String,
        /// Which root it is under. With none, the first of local, repo and shared that has it.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        #[arg(long)]
        space: Option<String>,
        /// Print the record as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SpaceCommand {
    /// Print every space, its pane tree, and the active space as JSON.
    Ls,
    /// Make a space for a directory and print its id.
    New {
        cwd: PathBuf,
        /// Defaults to the directory's name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Close a space and every pane in it.
    Close { id: String },
    /// Make a space the active one.
    Focus { id: String },
}

#[derive(Subcommand)]
enum PaneCommand {
    /// Print every pane, one per line: its space, id, kind, who made it (person or lead) and
    /// whether it has its space's focus.
    Ls {
        /// Print the panes as a JSON array of {space, pane, kind, created_by, focused}.
        #[arg(long)]
        json: bool,
    },
    /// Split a pane and print the new (empty) pane's id.
    Split {
        pane: String,
        /// Side the new pane goes on: left, right, up or down.
        #[arg(value_parser = wheel_proto::from_wire::<Dir>)]
        dir: Dir,
    },
    /// Close a pane; closing a space's last pane closes the space.
    Close { pane: String },
    /// Start a terminal or an agent in a pane, whatever it showed going. Prints the pane and, for
    /// an agent, its session name and state.
    ///
    /// Waits up to a minute for an agent to come up.
    Start {
        pane: String,
        #[arg(value_parser = wheel_proto::from_wire::<StartKind>)]
        kind: StartKind,
        /// For an agent: its first message. The command waits until the agent is ready or has
        /// given up, and fails with the daemon's error when it could not start.
        #[arg(long)]
        message: Option<String>,
    },
    /// Change what a pane shows: empty, terminal, web (an artifact, given by --artifact) or
    /// browser (a page, given by --page).
    Kind {
        pane: String,
        #[arg(value_parser = wheel_proto::from_wire::<NewPaneKind>)]
        kind: NewPaneKind,
        /// The artifact a web pane shows, by name: one plain name, never a path.
        #[arg(long)]
        artifact: Option<String>,
        /// Which root the artifact is under: repo, local or shared. With none, the first of local,
        /// repo and shared that has a folder by that name.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        /// The page a browser pane opens: an http or https URL, a file:/// URL, or about:blank. With
        /// none, the browser.homepage setting as it resolves for the pane's space. Named `page`
        /// rather than `url` because `--url` is this CLI's own flag for the daemon's address.
        #[arg(long)]
        page: Option<String>,
    },
    /// Move a browser pane to a page, as typing it into the pane's address field does; print the
    /// page the text named.
    Page {
        pane: String,
        /// Whatever the address field takes: a URL, `example.com`, a path, or about:blank.
        page: String,
    },
    /// Wait for a web pane's artifact folder or data directory to change; print
    /// `pane_reload <pane> at <unix ms>`.
    WaitReload {
        pane: String,
        /// Seconds to wait (0 to 3600); exit 3 if no change arrives.
        #[arg(long, default_value_t = 10.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Focus a pane, making its space active.
    Focus { pane: String },
    /// Set the share of a split its first pane takes (0.1 to 0.9).
    Ratio { split: String, ratio: f64 },
}

#[derive(Subcommand)]
enum LeadCommand {
    /// Send the lead a message and print its reply as it streams.
    ///
    /// When the turn ends for any reason but a normal finish, stderr gets one line with the raw
    /// protocol token, `[turn ended: max_tokens]`. The token is deliberate: scripts match on it,
    /// where the apps word the same reason for a person.
    Send {
        text: String,
        /// Seconds to wait for the reply to finish (0 to 3600).
        #[arg(long, default_value_t = 300.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the conversation so far.
    Read {
        /// Print the lead's state and its raw events as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop the lead's current turn; queued messages still run. Fails when no turn is running.
    Cancel,
    /// Print as JSON how the lead's agent is configured: its config directory, setting sources,
    /// built-in tools and brief, and whether each path exists; its login (`login.state` is `ok`,
    /// `needed` or `unknown`, beside the preflight's raw reading and the login methods the agent
    /// advertised); and the `claude` the preflight runs.
    Status,
    /// Log the lead in: open a terminal pane running the agent's login method (default
    /// `claude-ai-login`) with the lead's environment, wait for it, and exit with its exit code.
    /// An exit code of 0 sends the lead the messages that waited for the login.
    Login { method: Option<String> },
    /// Print the controls the agent offers — model, permission mode, effort — or set one by
    /// giving its id and a value.
    Config {
        config: Option<String>,
        value: Option<String>,
        /// Print the options as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the conversations the agent holds for the lead's directory, newest first, marking
    /// the one the lead is in.
    Sessions {
        /// Print them as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Put the lead in a fresh conversation and print the agent's id for it. Every client's
    /// transcript empties with it.
    New,
    /// Put the lead back in a past conversation, named by an id `lead sessions` printed.
    Resume { id: String },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Send an agent a message and print its reply as it streams.
    ///
    /// When the turn ends for any reason but a normal finish, stderr gets one line with the raw
    /// protocol token, `[turn ended: max_tokens]`. The token is deliberate: scripts match on it,
    /// where the apps word the same reason for a person.
    Send {
        /// An agent pane's id or its session name.
        session: String,
        text: String,
        /// Seconds to wait for the reply to finish (0 to 3600).
        #[arg(long, default_value_t = 300.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print an agent's conversation so far.
    Read {
        /// An agent pane's id or its session name.
        session: String,
        /// Print the session's state and its raw events as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop a session's current turn, or with none running its next queued message; later ones still
    /// run. Fails when there is neither.
    Cancel {
        /// An agent pane's id or its session name.
        session: String,
    },
    /// Wait until an agent is ready (no turn running or queued) and print its pane, session and
    /// state. Exits 3 when the time runs out first, 6 when the session gave up.
    Wait {
        /// An agent pane's id or its session name.
        session: String,
        /// Seconds to wait (0 to 3600).
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the conversations an agent holds for its directory, newest first, marking the one
    /// its pane is in.
    Sessions {
        /// An agent pane's id or its session name.
        session: String,
        /// Print them as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Put an agent's pane in a fresh conversation and print the agent's id for it. Every
    /// client's transcript empties with it.
    New {
        /// An agent pane's id or its session name.
        session: String,
    },
    /// Put an agent's pane back in a past conversation, named by an id `agent sessions` printed.
    /// Every client's transcript is replaced by that conversation's.
    Resume {
        /// An agent pane's id or its session name.
        session: String,
        id: String,
    },
    /// Print the controls an agent offers, or set one by giving its id and a value.
    Config {
        /// An agent pane's id or its session name.
        session: String,
        config: Option<String>,
        value: Option<String>,
        /// Print the options as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PermCommand {
    /// Print waiting permission requests as JSON.
    List,
    /// Answer one: `allow`, `deny`, or one of its option ids.
    Answer { id: String, choice: String },
    /// Wait for a permission request and print it as JSON (an already-waiting one counts).
    Wait {
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Start a shell and print its id.
    Open {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 32)]
        rows: u16,
    },
    /// Type text into a terminal.
    Send {
        /// A terminal id, or the id of a pane showing a terminal.
        id: String,
        text: String,
        /// Press Return after the text.
        #[arg(long)]
        enter: bool,
    },
    /// Print a terminal's screen as the daemon holds it, then live output until --until appears or --timeout passes.
    Read {
        /// A terminal id, or the id of a pane showing a terminal.
        id: String,
        /// Stop when this text appears; exit 3 if it never does, 4 if the terminal exits first.
        #[arg(long)]
        until: Option<String>,
        /// Skip the screen: only output produced after this command attached counts.
        #[arg(long)]
        new: bool,
        /// Seconds to keep reading live output (0 to 3600).
        #[arg(long, default_value_t = 1.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// End a terminal started with `term open`. A pane's terminal ends with its pane instead.
    Close { id: String },
}

fn parse_timeout(s: &str) -> Result<f64, String> {
    let secs: f64 = s
        .parse()
        .map_err(|_| format!("{s:?} is not a number of seconds"))?;
    if (0.0..=3600.0).contains(&secs) {
        Ok(secs)
    } else {
        Err("timeout must be between 0 and 3600 seconds".into())
    }
}

/// A failure, carrying the exit code a script can branch on.
#[derive(Debug)]
enum Fail {
    Timeout(String),
    Exited(Option<u32>),
    /// The lead's login command ended with this code (None: a signal).
    LoginExit(Option<u32>),
    Connect(String),
    Daemon(String),
    /// The socket closed or failed after connecting.
    Lost(String),
    Other(String),
}

impl Fail {
    fn code(&self) -> u8 {
        match self {
            Fail::Other(_) => 1,
            Fail::Timeout(_) => 3,
            Fail::Exited(_) => 4,
            Fail::LoginExit(code) => code.map_or(1, |c| c.clamp(1, 255) as u8),
            Fail::Connect(_) => 5,
            Fail::Daemon(_) => 6,
            Fail::Lost(_) => 7,
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Timeout(m) | Fail::Connect(m) | Fail::Other(m) => f.write_str(m),
            Fail::Exited(code) => write!(
                f,
                "terminal exited (code {code:?}) before the text appeared"
            ),
            Fail::LoginExit(code) => write!(f, "the login command exited with code {code:?}"),
            Fail::Daemon(m) => write!(f, "daemon: {m}"),
            Fail::Lost(m) => write!(f, "lost the connection to the daemon: {m}"),
        }
    }
}

fn other(e: impl std::fmt::Display) -> Fail {
    Fail::Other(e.to_string())
}

fn lost(e: impl std::fmt::Display) -> Fail {
    Fail::Lost(e.to_string())
}

/// The lead's session name.
const LEAD: &str = "lead";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Conn {
    ws: Ws,
    next_req: u64,
    /// Pushed events that arrived while waiting for a reply, delivered by the next `recv`.
    events: std::collections::VecDeque<ServerMsg>,
}

impl Conn {
    async fn open(url: &str, token_file: Option<PathBuf>) -> Result<Conn, Fail> {
        let path = match token_file {
            Some(path) => path,
            None => wheel_proto::support_dir(wheel_proto::DEFAULT_INSTANCE)
                .map(|dir| dir.join("token"))
                .map_err(|_| Fail::Connect("HOME is not set; pass --token-file".into()))?,
        };
        let token = std::fs::read_to_string(&path)
            .map_err(|e| Fail::Connect(format!("read token from {}: {e}", path.display())))?;
        let (ws, _) = tokio_tungstenite::connect_async(format!("{url}?token={}", token.trim()))
            .await
            .map_err(|e| Fail::Connect(format!("connect to {url}: {e}")))?;
        Ok(Conn {
            ws,
            next_req: 0,
            events: Default::default(),
        })
    }

    fn req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    async fn send(&mut self, msg: &ClientMsg) -> Result<(), Fail> {
        let text = serde_json::to_string(msg).map_err(other)?;
        self.ws.send(Message::Text(text.into())).await.map_err(lost)
    }

    /// The next daemon message, or None if `wait` passes first.
    async fn recv(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        match self.events.pop_front() {
            Some(event) => Ok(Some(event)),
            None => self.recv_frame(wait).await,
        }
    }

    /// Send a request and wait up to ten seconds for the reply carrying its `req` — enough for
    /// anything the daemon answers by itself.
    async fn call(&mut self, msg: ClientMsg, req: u64) -> Result<ServerMsg, Fail> {
        self.call_within(msg, req, Duration::from_secs(10)).await
    }

    /// The same, waiting `wait` for each frame: for a request the daemon cannot answer until a
    /// person has, such as a refresh whose command is not yet approved.
    async fn call_within(
        &mut self,
        msg: ClientMsg,
        req: u64,
        wait: Duration,
    ) -> Result<ServerMsg, Fail> {
        self.send(&msg).await?;
        let mut held = std::collections::VecDeque::new();
        let result = loop {
            let Some(reply) = self.recv_frame(wait).await? else {
                break Err(Fail::Timeout(format!(
                    "no reply from daemon within {:.0}s",
                    wait.as_secs_f64()
                )));
            };
            match &reply {
                ServerMsg::Error {
                    req: Some(r),
                    message,
                    ..
                } if *r == req => break Err(Fail::Daemon(message.clone())),
                ServerMsg::Ok { req: r }
                | ServerMsg::Created { req: r, .. }
                | ServerMsg::Started { req: r, .. }
                | ServerMsg::Opened { req: r, .. }
                | ServerMsg::Artifact { req: r, .. }
                | ServerMsg::Artifacts { req: r, .. }
                | ServerMsg::Refresh { req: r, .. }
                | ServerMsg::Trusted { req: r, .. }
                | ServerMsg::Inbox { req: r, .. }
                | ServerMsg::InboxItem { req: r, .. }
                | ServerMsg::Supervisor { req: r, .. }
                | ServerMsg::Supervisors { req: r, .. }
                | ServerMsg::SupervisorEvents { req: r, .. }
                | ServerMsg::SupervisorInbox { req: r, .. }
                | ServerMsg::SupervisorPolicy { req: r, .. }
                | ServerMsg::Settings { req: Some(r), .. }
                | ServerMsg::Status { req: Some(r), .. }
                | ServerMsg::TermOpened { req: Some(r), .. }
                | ServerMsg::TermAttached { req: Some(r), .. }
                | ServerMsg::SessionAttached { req: Some(r), .. }
                | ServerMsg::SessionQueued { req: r, .. }
                | ServerMsg::SessionWaited { req: r, .. }
                | ServerMsg::LeadLoginInfo { req: r, .. }
                | ServerMsg::LeadLoginFinished { req: r, .. }
                | ServerMsg::Conversations { req: r, .. }
                | ServerMsg::Conversation { req: r, .. }
                    if *r == req =>
                {
                    break Ok(reply)
                }
                _ => held.push_back(reply),
            }
        };
        self.events.extend(held);
        result
    }

    /// The next frame off the socket, skipping the queue `recv` drains first.
    async fn recv_frame(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(lost("it closed the socket")),
                Ok(Some(frame)) => match frame.map_err(lost)? {
                    Message::Text(text) => {
                        return serde_json::from_str(&text).map(Some).map_err(other)
                    }
                    Message::Close(Some(close)) => {
                        return Err(lost(format!("it closed the socket ({})", close.reason)))
                    }
                    _ => {}
                },
            }
        }
    }
}

async fn status(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let reply = conn.call(ClientMsg::Status { req: Some(req) }, req).await?;
    println!("{}", serde_json::to_string_pretty(&reply).map_err(other)?);
    Ok(())
}

async fn lead_status(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Status { lead_config, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    let config =
        lead_config.ok_or_else(|| Fail::Other("this daemon runs without a lead".into()))?;
    // Asking runs the preflight now, so the login it prints is current.
    let req = conn.req();
    let ServerMsg::LeadLoginInfo { login, claude, .. } = conn
        .call_within(
            ClientMsg::LeadLoginStatus { req: Some(req) },
            req,
            Duration::from_secs(30),
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to lead_login_status".into()));
    };
    let mut shown = serde_json::to_value(&config).map_err(other)?;
    shown["login"] = serde_json::to_value(&login).map_err(other)?;
    shown["claude"] = serde_json::to_value(&claude).map_err(other)?;
    println!("{}", serde_json::to_string_pretty(&shown).map_err(other)?);
    Ok(())
}

async fn lead_login(conn: &mut Conn, method: Option<String>) -> Result<(), Fail> {
    let req = conn.req();
    // A login lasts as long as the person takes to answer it in the pane.
    let ServerMsg::LeadLoginFinished {
        method, pane, code, ..
    } = conn
        .call_within(
            ClientMsg::LeadLogin {
                req: Some(req),
                method,
            },
            req,
            Duration::from_secs(3600),
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to lead_login".into()));
    };
    println!(
        "{method} {pane} exit {}",
        code.map_or("-".into(), |c| c.to_string())
    );
    match code {
        Some(0) => Ok(()),
        code => Err(Fail::LoginExit(code)),
    }
}

async fn term_open(conn: &mut Conn, cwd: Option<String>, cols: u16, rows: u16) -> Result<(), Fail> {
    let req = conn.req();
    match conn
        .call(
            ClientMsg::TermOpen {
                req: Some(req),
                cwd,
                cols,
                rows,
            },
            req,
        )
        .await?
    {
        ServerMsg::TermOpened { id, .. } => {
            println!("{id}");
            Ok(())
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// The terminal an id names: a terminal id as is, or the terminal a pane id (`p3`) shows.
async fn terminal_id(conn: &mut Conn, id: String) -> Result<String, Fail> {
    if !id.starts_with('p') {
        return Ok(id);
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    fn find<'a>(node: &'a wheel_proto::Node, pane: &str) -> Option<&'a wheel_proto::PaneKind> {
        match node {
            wheel_proto::Node::Pane { id, kind, .. } => (id == pane).then_some(kind),
            wheel_proto::Node::Split { a, b, .. } => find(a, pane).or_else(|| find(b, pane)),
        }
    }
    match spaces.iter().find_map(|s| find(&s.root, &id)) {
        Some(wheel_proto::PaneKind::Terminal { term, .. }) => Ok(term.clone()),
        Some(_) => Err(Fail::Daemon(format!("{id} is not a terminal pane"))),
        None => Err(Fail::Daemon(format!("no pane {id}"))),
    }
}

async fn term_send(conn: &mut Conn, id: String, text: String, enter: bool) -> Result<(), Fail> {
    let id = terminal_id(conn, id).await?;
    let mut bytes = text.into_bytes();
    if enter {
        bytes.push(b'\r');
    }
    let req = conn.req();
    conn.call(
        ClientMsg::TermInput {
            req: Some(req),
            id,
            data: encode_bytes(&bytes),
        },
        req,
    )
    .await?;
    Ok(())
}

async fn term_read(
    conn: &mut Conn,
    id: String,
    until: Option<String>,
    new: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let id = terminal_id(conn, id).await?;
    let req = conn.req();
    let first = conn
        .call(ClientMsg::TermAttach { req: Some(req), id }, req)
        .await?;
    // Bytes of replay still to arrive; with --new they are dropped rather than printed.
    let mut replay_left = match first {
        ServerMsg::TermAttached { replay_bytes, .. } if new => replay_bytes,
        _ => 0,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    let mut finder = until.as_deref().map(wheel_proto::TextFinder::new);
    let mut out = std::io::stdout();
    loop {
        // Always drain what is already queued, even with --timeout 0.
        let left = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(Duration::from_millis(20));
        match conn.recv(left).await? {
            None => {
                return match &until {
                    Some(n) => Err(Fail::Timeout(format!("timed out before {n:?} appeared"))),
                    None => Ok(()),
                }
            }
            Some(ServerMsg::TermOutput { data, .. }) => {
                let mut bytes = decode_bytes(&data).map_err(other)?;
                let skip = replay_left.min(bytes.len());
                replay_left -= skip;
                bytes.drain(..skip);
                out.write_all(&bytes).map_err(other)?;
                out.flush().map_err(other)?;
                if finder.as_mut().is_some_and(|f| f.feed(&bytes)) {
                    return Ok(());
                }
            }
            Some(ServerMsg::TermAttached { replay_bytes, .. }) => {
                // The daemon dropped output to this connection and is replaying; start over.
                eprintln!("\n[output resynced; replaying {replay_bytes} bytes]");
                if let Some(f) = finder.as_mut() {
                    f.clear();
                }
                replay_left = if new { replay_bytes } else { 0 };
            }
            Some(ServerMsg::TermExited { code, .. }) => {
                eprintln!("\n[terminal exited: {code:?}]");
                return match until {
                    Some(_) => Err(Fail::Exited(code)),
                    None => Ok(()),
                };
            }
            Some(_) => {}
        }
        if tokio::time::Instant::now() >= deadline && until.is_none() {
            return Ok(());
        }
    }
}

fn print_perm_hint(perm: &wheel_proto::PermInfo) {
    eprintln!(
        "\n[waiting on you] {}: {} wants {:?} -- wheel perm answer {} allow|deny",
        perm.id, perm.session, perm.title, perm.id
    );
}

async fn session_send(
    conn: &mut Conn,
    session: String,
    text: String,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    // The daemon resolves a pane id to its session, and events carry the session's name.
    let ServerMsg::SessionAttached { session, .. } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    let req = conn.req();
    let ServerMsg::SessionQueued { turn, .. } = conn
        .call(
            ClientMsg::SessionSend {
                req: Some(req),
                session: session.clone(),
                text,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_send".into()));
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    let mut out = std::io::stdout();
    // Our turn may wait behind others; it starts with the `turn_start` event carrying its id.
    let mut started = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = conn.recv(left).await? else {
            return Err(Fail::Timeout(format!(
                "{session} did not finish within {timeout}s"
            )));
        };
        match msg {
            ServerMsg::Session { session: s, event } if s == session => match event.event {
                SessionEvent::TurnStart { turn: t } if t == turn => started = true,
                SessionEvent::Text { text } if started => {
                    out.write_all(text.as_bytes()).map_err(other)?;
                    out.flush().map_err(other)?;
                }
                SessionEvent::ToolCall { title, status, .. } if started => {
                    eprintln!("\n[tool] {title} ({status})")
                }
                SessionEvent::TurnEnd {
                    turn: t,
                    stop_reason,
                } if t == turn => {
                    println!();
                    if stop_reason != "end_turn" {
                        eprintln!("[turn ended: {stop_reason}]");
                    }
                    return Ok(());
                }
                SessionEvent::Error {
                    turn: Some(t),
                    message,
                } if t == turn => return Err(Fail::Daemon(message)),
                // The agent process stopped. A turn in flight is lost; a queued one runs after the
                // restart, unless the session itself ended (its pane closed), which runs nothing more.
                SessionEvent::Error {
                    turn: None,
                    message,
                } if started || message == wheel_proto::SESSION_ENDED => {
                    return Err(Fail::Daemon(message))
                }
                _ => {}
            },
            ServerMsg::PermRequested { perm } => print_perm_hint(&perm),
            _ => {}
        }
    }
}

/// Wait for a session to be ready; print its pane, session and state.
async fn session_wait(conn: &mut Conn, session: String, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::SessionWaited {
        session,
        pane,
        state,
        ready,
        ..
    } = conn
        .call_within(
            ClientMsg::SessionWait {
                req: Some(req),
                session,
                timeout_secs: timeout,
            },
            req,
            Duration::from_secs_f64(timeout) + Duration::from_secs(10),
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_wait".into()));
    };
    let state = serde_json::to_string(&state).map_err(other)?;
    if !ready {
        return Err(Fail::Timeout(format!(
            "{session} was not ready within {timeout}s: {state}"
        )));
    }
    println!("{} {session} {state}", pane.as_deref().unwrap_or("-"));
    Ok(())
}

/// Print a session's config options, or set one. The agent names them, so there is no fixed list:
/// whatever it offers is what prints and what can be set.
async fn session_config(
    conn: &mut Conn,
    session: String,
    config: Option<String>,
    value: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    if let Some(config) = config {
        // Listing on a half-typed set would hide the typo behind output that looks like a success.
        let Some(value) = value else {
            return Err(Fail::Other(format!(
                "give a value to set {config} to, or no arguments to list the options"
            )));
        };
        return simple(conn, |req| ClientMsg::SessionConfigSet {
            req: Some(req),
            session,
            config,
            value,
        })
        .await;
    }
    let req = conn.req();
    let ServerMsg::SessionAttached { config, state, .. } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session: session.clone(),
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    // While the session is starting or failed, the list is empty or the dead agent's: either way
    // a restart in progress, not what the agent offers.
    match state {
        SessionState::Starting | SessionState::Failed { .. } => {
            return Err(Fail::Other(format!(
                "{session}'s agent is restarting; its config options come back once it is ready"
            )));
        }
        SessionState::GaveUp { tries, message } => {
            return Err(Fail::Other(format!(
                "{session}'s agent gave up after {tries} tries: {message}"
            )));
        }
        SessionState::Ready | SessionState::Busy { .. } => {}
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&config).map_err(other)?);
        return Ok(());
    }
    if config.is_empty() {
        println!("{session} offers no config options");
        return Ok(());
    }
    for option in config {
        match option.kind {
            ConfigKind::Select { current, choices } => {
                println!("{} = {current}  ({})", option.id, option.name);
                for choice in choices {
                    let mark = if choice.value == current { "*" } else { " " };
                    println!("  {mark} {} — {}", choice.value, choice.name);
                }
            }
            ConfigKind::Boolean { current } => {
                println!("{} = {current}  ({})", option.id, option.name);
                println!("    true\n    false");
            }
            ConfigKind::Unknown => {
                println!(
                    "{}  ({}) — this wheel cannot set it",
                    option.id, option.name
                )
            }
        }
    }
    Ok(())
}

/// Print the conversations a session's agent holds for its directory, newest first, marking the
/// one the session is in with `*`.
async fn session_conversations(conn: &mut Conn, session: String, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Conversations { conversations, .. } = conn
        .call_within(
            ClientMsg::SessionConversationLs {
                req: Some(req),
                session: session.clone(),
            },
            req,
            Duration::from_secs(60),
        )
        .await?
    else {
        return Err(Fail::Other(
            "unexpected reply to session_conversation_ls".into(),
        ));
    };
    // The agent's store belongs to the directory, not to wheel: anything else running the same
    // harness there keeps its conversations in the same place. Saying so beats a filter, since
    // nothing in an entry says which program made it. On stderr, so stdout stays the listing —
    // and so a caller reading the JSON is told too.
    if !conversations.is_empty() {
        eprintln!(
            "[these are every conversation the agent holds for that directory, including ones \
             started outside wheel — a terminal Claude Code session in it, say]"
        );
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&conversations).map_err(other)?
        );
        return Ok(());
    }
    if conversations.is_empty() {
        eprintln!("[{session}'s agent holds no conversations for its directory]");
        return Ok(());
    }
    for conversation in &conversations {
        let mark = if conversation.current { "*" } else { " " };
        println!(
            "{mark} {}  {}  {}",
            conversation.id,
            conversation.updated_at.as_deref().unwrap_or("-"),
            conversation.title.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// Put a session in a fresh conversation, or back in the past one `load` names, and print the
/// agent's id for it.
async fn session_switch(
    conn: &mut Conn,
    session: String,
    load: Option<String>,
) -> Result<(), Fail> {
    let req = conn.req();
    let msg = match load {
        Some(id) => ClientMsg::SessionConversationResume {
            req: Some(req),
            session,
            id,
        },
        None => ClientMsg::SessionConversationNew {
            req: Some(req),
            session,
        },
    };
    // A real agent takes up to a minute to load a conversation and replay it.
    let reply = conn.call_within(msg, req, Duration::from_secs(120)).await?;
    let ServerMsg::Conversation { id, .. } = reply else {
        return Err(Fail::Other(format!("unexpected reply {reply:?}")));
    };
    println!("{id}");
    Ok(())
}

async fn session_read(conn: &mut Conn, session: String, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::SessionAttached {
        session,
        state,
        events,
        ..
    } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    if json {
        let all = serde_json::json!({ "state": state, "events": events });
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    // A `user` event arrives when the message is queued; `turn_start` says it began running.
    let waiting = wheel_proto::queued_turns(&events);
    for event in events {
        match event.event {
            SessionEvent::User { turn, text } => {
                let queued = if waiting.contains(&turn) {
                    " (queued)"
                } else {
                    ""
                };
                println!("\n> {text}{queued}")
            }
            SessionEvent::Text { text } => println!("{text}"),
            SessionEvent::Thought { text } => println!("[thought] {text}"),
            SessionEvent::Block { block_kind, name } => {
                println!("[{block_kind}] {}", name.as_deref().unwrap_or("-"))
            }
            SessionEvent::ToolCall {
                title,
                status,
                content,
                ..
            } => {
                println!("[tool] {title} ({status})");
                print_tool_content(&content);
            }
            SessionEvent::ToolUpdate {
                id,
                content: Some(content),
                ..
            } => {
                println!("[tool {id}] content now:");
                print_tool_content(&content);
            }
            SessionEvent::Mode { mode } => println!("[mode] {mode}"),
            SessionEvent::Error { message, .. } => println!("[error] {message}"),
            _ => {}
        }
    }
    eprintln!(
        "[{session}: {}]",
        serde_json::to_string(&state).map_err(other)?
    );
    Ok(())
}

async fn session_cancel(conn: &mut Conn, session: String) -> Result<(), Fail> {
    simple(conn, |req| ClientMsg::SessionCancel {
        req: Some(req),
        session,
    })
    .await
}

/// Text indented under its tool row; a diff by its path and line counts, never its text.
fn print_tool_content(content: &[ToolContent]) {
    for item in content {
        match item {
            ToolContent::Text { text } => {
                for line in text.lines() {
                    println!("  {line}");
                }
            }
            ToolContent::Diff {
                path,
                old_text,
                new_text,
            } => println!(
                "  diff {path} (-{} +{} lines)",
                old_text.as_deref().map_or(0, |t| t.lines().count()),
                new_text.lines().count()
            ),
            ToolContent::Terminal { id } => println!("  terminal {id}"),
            ToolContent::Unknown => println!("  (content this wheel cannot show)"),
        }
    }
}

async fn perm_list(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    println!("{}", serde_json::to_string_pretty(&perms).map_err(other)?);
    Ok(())
}

async fn perm_wait(conn: &mut Conn, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    if let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    {
        if let Some(perm) = perms.into_iter().next() {
            println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
            return Ok(());
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no permission request within {timeout}s"
                )))
            }
            Some(ServerMsg::PermRequested { perm }) => {
                println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

async fn inbox_list(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Inbox { items, .. } = conn
        .call(ClientMsg::InboxLs { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to inbox_ls".into()));
    };
    println!("{}", serde_json::to_string_pretty(&items).map_err(other)?);
    Ok(())
}

async fn supervisor_one(conn: &mut Conn, msg: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Supervisor { supervisor, .. } = conn.call(msg(req), req).await? else {
        return Err(Fail::Other(
            "unexpected reply to a supervisor request".into(),
        ));
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&supervisor).map_err(other)?
    );
    Ok(())
}

async fn supervisor_list(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Supervisors { supervisors, .. } = conn
        .call(ClientMsg::SupervisorLs { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to supervisor_ls".into()));
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&supervisors).map_err(other)?
    );
    Ok(())
}

async fn supervisor(conn: &mut Conn, command: SupervisorCommand) -> Result<(), Fail> {
    match command {
        SupervisorCommand::Start {
            goal,
            focus,
            space,
            workers,
            cap,
        } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorStart {
                req: Some(req),
                space,
                goal,
                focus,
                workers,
                cap,
            })
            .await
        }
        SupervisorCommand::Pause { id } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorPause {
                req: Some(req),
                id,
            })
            .await
        }
        SupervisorCommand::Resume { id } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorResume {
                req: Some(req),
                id,
            })
            .await
        }
        SupervisorCommand::Stop { id } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorStop { req: Some(req), id }).await
        }
        SupervisorCommand::Workers { id, workers } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorWorkers {
                req: Some(req),
                id,
                workers,
            })
            .await
        }
        SupervisorCommand::Cap { id, cap } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorCap {
                req: Some(req),
                id,
                cap,
            })
            .await
        }
        SupervisorCommand::List => supervisor_list(conn).await,
        SupervisorCommand::Show { id } => {
            supervisor_one(conn, |req| ClientMsg::SupervisorShow { req: Some(req), id }).await
        }
        SupervisorCommand::Events { id, last } => {
            let req = conn.req();
            let ServerMsg::SupervisorEvents { events, .. } = conn
                .call(
                    ClientMsg::SupervisorEvents {
                        req: Some(req),
                        id,
                        limit: last,
                    },
                    req,
                )
                .await?
            else {
                return Err(Fail::Other("unexpected reply to supervisor_events".into()));
            };
            println!("{}", serde_json::to_string_pretty(&events).map_err(other)?);
            Ok(())
        }
        SupervisorCommand::Inbox { id } => {
            let req = conn.req();
            let ServerMsg::SupervisorInbox { items, .. } = conn
                .call(ClientMsg::SupervisorInbox { req: Some(req), id }, req)
                .await?
            else {
                return Err(Fail::Other("unexpected reply to supervisor_inbox".into()));
            };
            println!("{}", serde_json::to_string_pretty(&items).map_err(other)?);
            Ok(())
        }
        SupervisorCommand::Push { id, session } => {
            create(conn, |req| ClientMsg::WorkerPush {
                req: Some(req),
                id,
                session,
            })
            .await
        }
        SupervisorCommand::Policy { id } => {
            let req = conn.req();
            let ServerMsg::SupervisorPolicy { policy, .. } = conn
                .call(ClientMsg::SupervisorPolicy { req: Some(req), id }, req)
                .await?
            else {
                return Err(Fail::Other("unexpected reply to supervisor_policy".into()));
            };
            println!("{}", serde_json::to_string_pretty(&policy).map_err(other)?);
            Ok(())
        }
    }
}

async fn inbox_close(conn: &mut Conn, id: String) -> Result<(), Fail> {
    let req = conn.req();
    let msg = ClientMsg::InboxSet {
        req: Some(req),
        id,
        state: wheel_proto::InboxState::Closed,
    };
    let ServerMsg::InboxItem { item, .. } = conn.call(msg, req).await? else {
        return Err(Fail::Other("unexpected reply to inbox_set".into()));
    };
    println!("{}", serde_json::to_string_pretty(&item).map_err(other)?);
    Ok(())
}

async fn wait_reload(conn: &mut Conn, pane: String, timeout: f64) -> Result<(), Fail> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no change in {pane}'s folder within {timeout}s"
                )))
            }
            Some(ServerMsg::PaneReload { pane: changed }) if changed == pane => {
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(other)?
                    .as_millis();
                println!("pane_reload {pane} at {at}");
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

/// Wait for the next settings broadcast (a frame with no `req`, which a reply never is) and print
/// it. The space is the one the daemon resolved the list for, `machine-wide` when it resolved
/// against none, so a script can tell a machine write from a project write.
async fn config_watch(conn: &mut Conn, timeout: f64, json: bool) -> Result<(), Fail> {
    // The connection is up and the broadcast will be heard: a script that writes after this line
    // cannot miss it.
    eprintln!("watching settings");
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no settings broadcast within {timeout}s"
                )))
            }
            Some(ServerMsg::Settings {
                req: None,
                space,
                settings,
            }) => {
                if json {
                    let record = serde_json::json!({ "space": space, "settings": settings });
                    println!("{}", serde_json::to_string_pretty(&record).map_err(other)?);
                    return Ok(());
                }
                println!(
                    "settings space={}",
                    space.as_deref().unwrap_or("machine-wide")
                );
                for s in &settings {
                    let origin = match s.origin {
                        SettingOrigin::Project => "project",
                        SettingOrigin::Machine => "machine",
                        SettingOrigin::Default => "default",
                    };
                    println!("{} = {}   ({origin})", s.key, s.value);
                    if let Some(problem) = &s.problem {
                        println!("  problem: {problem}");
                    }
                }
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

async fn simple(conn: &mut Conn, make: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(make(req), req).await?;
    Ok(())
}

/// Send a request that makes something and print the new id.
async fn create(conn: &mut Conn, make: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    match conn.call(make(req), req).await? {
        ServerMsg::Created { id, .. } => {
            println!("{id}");
            Ok(())
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

async fn space_ls(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Status {
        spaces,
        active,
        watches,
        ..
    } = conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    // Each web pane's node names only its artifact's scope and name; the daemon owns the roots, so
    // the folder it resolved comes back with the watch rather than being recomputed here.
    let layout = serde_json::json!({ "spaces": spaces, "active": active, "web_panes": watches });
    println!("{}", serde_json::to_string_pretty(&layout).map_err(other)?);
    Ok(())
}

/// One artifact for a person to read: what it is, where the daemon resolved it to, what its
/// manifest asks for, and every problem the manifest has — a problem is part of the record, so it
/// prints here as well as in `--json`, never only in the daemon's log.
fn print_artifact(a: &wheel_proto::ArtifactInfo) {
    // The flag is on the name's own line, because what it answers — would a pane picker offer
    // this? — is a property of the artifact rather than a line of its configuration.
    let offered = if a.pane { "  (in the pane picker)" } else { "" };
    println!("{} {} — {}{offered}", a.scope.name(), a.name, a.title);
    println!("  {}", a.dir);
    match (&a.command, a.every) {
        (Some(command), Some(secs)) => println!("  refresh: {command}  (every {secs}s)"),
        (Some(command), None) => println!("  refresh: {command}  (on open and on demand)"),
        (None, Some(secs)) => println!("  every {secs}s"),
        (None, None) => {}
    }
    for problem in &a.problems {
        println!("  problem: {problem}");
    }
}

/// Send a request about one artifact and take the record it replies with.
async fn artifact_of(
    conn: &mut Conn,
    msg: impl FnOnce(u64) -> ClientMsg,
) -> Result<wheel_proto::ArtifactInfo, Fail> {
    let req = conn.req();
    match conn.call(msg(req), req).await? {
        ServerMsg::Artifact { artifact, .. } => Ok(artifact),
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// Make an artifact and print its folder's absolute path, the one thing a script needs next.
async fn artifact_new(
    conn: &mut Conn,
    name: String,
    pane: bool,
    scope: Option<ArtifactScope>,
    space: Option<String>,
) -> Result<(), Fail> {
    let artifact = artifact_of(conn, |req| ClientMsg::ArtifactNew {
        req: Some(req),
        pane,
        name,
        scope,
        space,
    })
    .await?;
    println!("{}", artifact.dir);
    eprintln!(
        "[{} artifact {} — {}]",
        artifact.scope.name(),
        artifact.name,
        artifact.title
    );
    Ok(())
}

/// Ask the daemon for every setting as it resolves for `space`, or write one first.
async fn settings(
    conn: &mut Conn,
    msg: impl FnOnce(u64) -> ClientMsg,
) -> Result<Vec<SettingInfo>, Fail> {
    let req = conn.req();
    match conn.call(msg(req), req).await? {
        ServerMsg::Settings { settings, .. } => Ok(settings),
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// One setting for a person to read: the value in force, and which wheel.toml it came from, so a
/// value that is not the one they expected says where to go and change it.
fn print_setting(s: &SettingInfo) {
    let origin = match s.origin {
        SettingOrigin::Project => "this project's wheel.toml",
        SettingOrigin::Machine => "this machine's wheel.toml",
        SettingOrigin::Default => "wheel's default",
    };
    println!("{} = {}   ({origin})", s.key, s.value);
    println!("  {}", s.help);
    // What removing the project's value would bring back.
    if let (SettingOrigin::Project, Some(machine)) = (s.origin, &s.machine) {
        println!("  this machine's: {machine}");
    }
    if s.origin != SettingOrigin::Default {
        println!("  default: {}", s.default);
    }
    // A file wheel could not use is why the value above is not the one the person wrote, so it is
    // printed beside the value rather than left in the daemon's log.
    if let Some(problem) = &s.problem {
        println!("  problem: {problem}");
    }
}

async fn config_ls(conn: &mut Conn, space: Option<String>, json: bool) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingsLs {
        req: Some(req),
        space,
    })
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    for s in &all {
        print_setting(s);
    }
    Ok(())
}

/// One setting's value on stdout and nothing else, so `$(wheel config get <key>)` is the value.
async fn config_get(
    conn: &mut Conn,
    key: String,
    space: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingsLs {
        req: Some(req),
        space,
    })
    .await?;
    let Some(s) = all.iter().find(|s| s.key == key) else {
        let known: Vec<&str> = all.iter().map(|s| s.key.as_str()).collect();
        return Err(Fail::Daemon(format!(
            "no setting {key}. Wheel's settings are: {}",
            known.join(", ")
        )));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(s).map_err(other)?);
    } else {
        println!("{}", s.value);
    }
    Ok(())
}

async fn config_set(
    conn: &mut Conn,
    key: String,
    value: String,
    space: Option<String>,
    machine: bool,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingSet {
        req: Some(req),
        key: key.clone(),
        value,
        space,
        machine,
    })
    .await?;
    print_changed(&key, &all, json)
}

async fn config_unset(
    conn: &mut Conn,
    key: String,
    space: Option<String>,
    machine: bool,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingUnset {
        req: Some(req),
        key: key.clone(),
        space,
        machine,
    })
    .await?;
    print_changed(&key, &all, json)
}

/// After a write or a removal: the settings as JSON, or the one changed.
fn print_changed(key: &str, all: &[SettingInfo], json: bool) -> Result<(), Fail> {
    if json {
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    // The one that was written, as it now resolves: a project write under a machine value still
    // wins, and a machine write under a project value does not, which this is how you see. The
    // daemon refuses a key it does not have and answers with every key it does, so a reply missing
    // the key it just wrote is a daemon that disagrees with this build about what settings are —
    // which is worth saying, not worth printing the whole list over.
    let Some(s) = all.iter().find(|s| s.key == key) else {
        return Err(Fail::Other(format!(
            "the daemon changed {key} and then reported {} settings without it",
            all.len()
        )));
    };
    print_setting(s);
    Ok(())
}

async fn artifact_ls(
    conn: &mut Conn,
    space: Option<String>,
    pane: bool,
    json: bool,
) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Artifacts {
        artifacts,
        resolves,
        ..
    } = conn
        .call(
            ClientMsg::ArtifactLs {
                req: Some(req),
                space,
                pane,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to artifact_ls".into()));
    };
    if json {
        let all = serde_json::json!({ "artifacts": artifacts, "resolves": resolves });
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    if artifacts.is_empty() {
        if pane {
            eprintln!("[no artifact in any scope asks for the pane picker]");
        } else {
            eprintln!("[no artifacts in any scope]");
        }
        return Ok(());
    }
    for artifact in &artifacts {
        print_artifact(artifact);
    }
    println!("a bare name resolves to:");
    for (name, scope) in &resolves {
        println!("  {name} -> {}", scope.name());
    }
    Ok(())
}

async fn artifact_show(
    conn: &mut Conn,
    name: String,
    scope: Option<ArtifactScope>,
    space: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    let artifact = artifact_of(conn, |req| ClientMsg::ArtifactShow {
        req: Some(req),
        name,
        scope,
        space,
    })
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&artifact).map_err(other)?
        );
    } else {
        print_artifact(&artifact);
    }
    Ok(())
}

/// One pane's refresh runner for a person to read: what it runs where, how many runs it has made,
/// and what the last one did — the exit status and the tail of its error output, so a failing
/// refresh is diagnosable without reproducing it.
fn print_refresh(r: &wheel_proto::RefreshInfo) {
    println!("{} {} {} — {}", r.pane, r.scope.name(), r.name, r.command);
    match r.timer {
        TimerState::Ticking => println!(
            "  on open, on demand and every {}s",
            r.every.unwrap_or_default()
        ),
        TimerState::Untrusted => println!(
            "  on open and on demand; the timer is skipping every tick — this command is not remembered as approved, and a tick never asks. Run `wheel artifact refresh <pane>` and answer \"always\" to resume it"
        ),
        TimerState::Idle => println!(
            "  on open and on demand; the timer is skipping every tick — artifact.toml asks for no command or no interval"
        ),
        TimerState::Stopped => println!(
            "  on open and on demand; this pane's timer is gone until the pane shows the artifact again"
        ),
        // A manifest that gained `every` after the pane opened gets its timer at the next open, as
        // one that gained a `command` does.
        TimerState::Off => match r.every {
            Some(secs) => {
                println!("  on open and on demand; every {secs}s starts when the pane next shows it")
            }
            None => println!("  on open and on demand"),
        },
    }
    println!("  in {}", r.cwd);
    println!("  writes {}", r.data_dir);
    println!("  runs: {}", r.runs);
    let Some(last) = &r.last else {
        println!("  no run has finished yet");
        return;
    };
    match (last.exit, &last.error) {
        (Some(code), _) => println!(
            "  last run at {}: exit {code} in {}ms",
            last.started_ms, last.duration_ms
        ),
        (None, Some(why)) => println!(
            "  last run at {}: did not run — {why} ({}ms)",
            last.started_ms, last.duration_ms
        ),
        (None, None) => println!("  last run at {}: no result", last.started_ms),
    }
    for line in last.stderr_tail.lines() {
        println!("  stderr: {line}");
    }
}

/// Run a pane's refresh now. Exits non-zero when the run failed, so a check reads fresh data or
/// knows it did not.
async fn artifact_refresh(
    conn: &mut Conn,
    pane: String,
    json: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    eprintln!(
        "[refreshing {pane}; a command nobody has approved yet waits here for `wheel perm answer`]"
    );
    let reply = conn
        .call_within(
            ClientMsg::ArtifactRefresh {
                req: Some(req),
                pane: pane.clone(),
            },
            req,
            Duration::from_secs_f64(timeout),
        )
        .await?;
    let ServerMsg::Refresh { refresh, .. } = reply else {
        return Err(Fail::Other("unexpected reply to artifact_refresh".into()));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&refresh).map_err(other)?);
    } else {
        print_refresh(&refresh);
    }
    match &refresh.last {
        Some(run) if run.ok() => Ok(()),
        Some(run) => Err(Fail::Other(format!(
            "{pane}'s refresh failed: {}",
            match (run.exit, &run.error) {
                (Some(code), _) => format!("exit {code}"),
                (None, Some(why)) => why.clone(),
                (None, None) => "no result".into(),
            }
        ))),
        None => Err(Fail::Other(format!("{pane}'s refresh recorded no run"))),
    }
}

/// Print the remembered approvals, or forget one artifact's. The pair is the artifact's folder and
/// the command, so the list says exactly what would run without asking again.
async fn artifact_trust(conn: &mut Conn, forget: Option<String>, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let msg = match forget {
        Some(name) => ClientMsg::ArtifactTrustForget {
            req: Some(req),
            name,
        },
        None => ClientMsg::ArtifactTrust { req: Some(req) },
    };
    let ServerMsg::Trusted { trusted, .. } = conn.call(msg, req).await? else {
        return Err(Fail::Other("unexpected reply to artifact_trust".into()));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&trusted).map_err(other)?);
        return Ok(());
    }
    if trusted.is_empty() {
        eprintln!("[no command is approved; the first run of each asks]");
        return Ok(());
    }
    for t in &trusted {
        println!("{} {} — {}", t.scope.name(), t.name, t.command);
        println!("  {}  (approved at {})", t.dir, t.approved_ms);
    }
    Ok(())
}

/// The daemon resolves nothing against this command's directory, so send it absolute paths.
fn absolute(path: &std::path::Path) -> Result<String, Fail> {
    Ok(std::fs::canonicalize(path)
        .map_err(|e| Fail::Other(format!("{}: {e}", path.display())))?
        .to_string_lossy()
        .into_owned())
}

async fn space_new(conn: &mut Conn, cwd: PathBuf, name: Option<String>) -> Result<(), Fail> {
    let cwd = absolute(&cwd)?;
    create(conn, |req| ClientMsg::SpaceCreate {
        req: Some(req),
        cwd,
        name,
    })
    .await
}

async fn space(conn: &mut Conn, command: SpaceCommand) -> Result<(), Fail> {
    match command {
        SpaceCommand::Ls => space_ls(conn).await,
        SpaceCommand::New { cwd, name } => space_new(conn, cwd, name).await,
        SpaceCommand::Close { id } => {
            simple(conn, |req| ClientMsg::SpaceClose { req: Some(req), id }).await
        }
        SpaceCommand::Focus { id } => {
            simple(conn, |req| ClientMsg::Focus {
                req: Some(req),
                space: Some(id),
                pane: None,
            })
            .await
        }
    }
}

/// Every pane in tab order, with who made it.
async fn pane_ls(conn: &mut Conn, as_json: bool) -> Result<(), Fail> {
    fn walk(node: &wheel_proto::Node, out: &mut Vec<(String, String, wheel_proto::CreatedBy)>) {
        match node {
            wheel_proto::Node::Pane {
                id,
                created_by,
                kind,
            } => {
                let kind = serde_json::to_value(kind)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                out.push((id.clone(), kind, *created_by));
            }
            wheel_proto::Node::Split { a, b, .. } => {
                walk(a, out);
                walk(b, out);
            }
        }
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    let mut rows = Vec::new();
    for space in &spaces {
        let mut panes = Vec::new();
        walk(&space.root, &mut panes);
        for (pane, kind, created_by) in panes {
            let focused = pane == space.focus;
            rows.push(serde_json::json!({
                "space": space.id, "pane": pane, "kind": kind,
                "created_by": created_by, "focused": focused,
            }));
        }
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows).map_err(other)?);
        return Ok(());
    }
    for row in &rows {
        let by = row["created_by"].as_str().unwrap_or_default();
        let focused = if row["focused"] == true {
            "  (focused)"
        } else {
            ""
        };
        println!(
            "{} {} {} by {by}{focused}",
            row["space"].as_str().unwrap_or_default(),
            row["pane"].as_str().unwrap_or_default(),
            row["kind"].as_str().unwrap_or_default(),
        );
    }
    if rows.is_empty() {
        eprintln!("[no panes]");
    }
    Ok(())
}

async fn pane(conn: &mut Conn, command: PaneCommand) -> Result<(), Fail> {
    match command {
        PaneCommand::Ls { json } => pane_ls(conn, json).await,
        PaneCommand::Split { pane, dir } => {
            create(conn, |req| ClientMsg::PaneSplit {
                req: Some(req),
                pane,
                dir,
            })
            .await
        }
        PaneCommand::Close { pane } => {
            simple(conn, |req| ClientMsg::PaneClose {
                req: Some(req),
                pane,
            })
            .await
        }
        PaneCommand::Start {
            pane,
            kind,
            message,
        } => {
            let req = conn.req();
            let msg = ClientMsg::PaneStart {
                req: Some(req),
                pane,
                kind,
                message,
            };
            let ServerMsg::Started {
                pane,
                session,
                state,
                ..
            } = conn.call_within(msg, req, Duration::from_secs(70)).await?
            else {
                return Err(Fail::Other("unexpected reply to pane start".into()));
            };
            let started = serde_json::json!({ "pane": pane, "session": session, "state": state });
            println!("{}", serde_json::to_string_pretty(&started).map_err(other)?);
            Ok(())
        }
        PaneCommand::Kind {
            pane,
            kind,
            artifact,
            scope,
            page,
        } => {
            simple(conn, |req| ClientMsg::PaneSetKind {
                req: Some(req),
                pane,
                kind,
                artifact,
                scope,
                url: page,
            })
            .await
        }
        PaneCommand::Page { pane, page } => {
            let req = conn.req();
            let msg = ClientMsg::PaneBrowserOpen {
                req: Some(req),
                pane,
                text: page,
            };
            match conn.call(msg, req).await? {
                ServerMsg::Opened { url, .. } => {
                    println!("{url}");
                    Ok(())
                }
                other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
        }
        PaneCommand::WaitReload { pane, timeout } => wait_reload(conn, pane, timeout).await,
        PaneCommand::Focus { pane } => {
            simple(conn, |req| ClientMsg::Focus {
                req: Some(req),
                space: None,
                pane: Some(pane),
            })
            .await
        }
        PaneCommand::Ratio { split, ratio } => {
            simple(conn, |req| ClientMsg::SplitRatio {
                req: Some(req),
                split,
                ratio,
            })
            .await
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = async {
        let mut conn = Conn::open(&cli.url, cli.token_file).await?;
        match cli.command {
            Command::Status => status(&mut conn).await,
            Command::Term(TermCommand::Open { cwd, cols, rows }) => {
                term_open(&mut conn, cwd, cols, rows).await
            }
            Command::Term(TermCommand::Send { id, text, enter }) => {
                term_send(&mut conn, id, text, enter).await
            }
            Command::Term(TermCommand::Read {
                id,
                until,
                new,
                timeout,
            }) => term_read(&mut conn, id, until, new, timeout).await,
            Command::Term(TermCommand::Close { id }) => {
                simple(&mut conn, |req| ClientMsg::TermClose { req: Some(req), id }).await
            }
            Command::Lead(LeadCommand::Send { text, timeout }) => {
                session_send(&mut conn, LEAD.into(), text, timeout).await
            }
            Command::Lead(LeadCommand::Read { json }) => {
                session_read(&mut conn, LEAD.into(), json).await
            }
            Command::Lead(LeadCommand::Cancel) => session_cancel(&mut conn, LEAD.into()).await,
            Command::Lead(LeadCommand::Status) => lead_status(&mut conn).await,
            Command::Lead(LeadCommand::Login { method }) => lead_login(&mut conn, method).await,
            Command::Lead(LeadCommand::Config {
                config,
                value,
                json,
            }) => session_config(&mut conn, LEAD.into(), config, value, json).await,
            Command::Lead(LeadCommand::Sessions { json }) => {
                session_conversations(&mut conn, LEAD.into(), json).await
            }
            Command::Lead(LeadCommand::New) => session_switch(&mut conn, LEAD.into(), None).await,
            Command::Lead(LeadCommand::Resume { id }) => {
                session_switch(&mut conn, LEAD.into(), Some(id)).await
            }
            Command::Agent(AgentCommand::Config {
                session,
                config,
                value,
                json,
            }) => session_config(&mut conn, session, config, value, json).await,
            Command::Agent(AgentCommand::Sessions { session, json }) => {
                session_conversations(&mut conn, session, json).await
            }
            Command::Agent(AgentCommand::New { session }) => {
                session_switch(&mut conn, session, None).await
            }
            Command::Agent(AgentCommand::Resume { session, id }) => {
                session_switch(&mut conn, session, Some(id)).await
            }
            Command::Agent(AgentCommand::Send {
                session,
                text,
                timeout,
            }) => session_send(&mut conn, session, text, timeout).await,
            Command::Agent(AgentCommand::Read { session, json }) => {
                session_read(&mut conn, session, json).await
            }
            Command::Agent(AgentCommand::Cancel { session }) => {
                session_cancel(&mut conn, session).await
            }
            Command::Agent(AgentCommand::Wait { session, timeout }) => {
                session_wait(&mut conn, session, timeout).await
            }
            Command::Perm(PermCommand::List) => perm_list(&mut conn).await,
            Command::Perm(PermCommand::Answer { id, choice }) => {
                simple(&mut conn, |req| ClientMsg::PermAnswer {
                    req: Some(req),
                    id,
                    choice,
                })
                .await
            }
            Command::Perm(PermCommand::Wait { timeout }) => perm_wait(&mut conn, timeout).await,
            Command::Inbox(InboxCommand::List) => inbox_list(&mut conn).await,
            Command::Inbox(InboxCommand::Close { id }) => inbox_close(&mut conn, id).await,
            Command::Supervisor(command) => supervisor(&mut conn, command).await,
            Command::Space(command) => space(&mut conn, command).await,
            Command::Pane(command) => pane(&mut conn, command).await,
            Command::Artifact(ArtifactCommand::New {
                name,
                pane,
                scope,
                space,
            }) => artifact_new(&mut conn, name, pane, scope, space).await,
            Command::Artifact(ArtifactCommand::Ls { space, pane, json }) => {
                artifact_ls(&mut conn, space, pane, json).await
            }
            Command::Config(ConfigCommand::Ls { space, json }) => {
                config_ls(&mut conn, space, json).await
            }
            Command::Config(ConfigCommand::Get { key, space, json }) => {
                config_get(&mut conn, key, space, json).await
            }
            Command::Config(ConfigCommand::Set {
                key,
                value,
                space,
                machine,
                json,
            }) => config_set(&mut conn, key, value, space, machine, json).await,
            Command::Config(ConfigCommand::Watch { timeout, json }) => {
                config_watch(&mut conn, timeout, json).await
            }
            Command::Config(ConfigCommand::Unset {
                key,
                space,
                machine,
                json,
            }) => config_unset(&mut conn, key, space, machine, json).await,
            Command::Artifact(ArtifactCommand::Refresh {
                pane,
                json,
                timeout,
            }) => artifact_refresh(&mut conn, pane, json, timeout).await,
            Command::Artifact(ArtifactCommand::Trust { forget, json }) => {
                artifact_trust(&mut conn, forget, json).await
            }
            Command::Artifact(ArtifactCommand::Show {
                name,
                scope,
                space,
                json,
            }) => artifact_show(&mut conn, name, scope, space, json).await,
        }
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => {
            eprintln!("wheel: {fail}");
            ExitCode::from(fail.code())
        }
    }
}
