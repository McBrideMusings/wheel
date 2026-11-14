use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use wheel_proto::{decode_bytes, encode_bytes, AgentPermissions, ClientMsg, ContextUse, ServerMsg};

use crate::perms::Perms;
use crate::pty::{Attachment, Term, TermEvent};
use crate::resume::Harness;
use crate::agent::{Restart, Agent, AgentAttachment, AgentConfig};
use crate::shell::{Control, Launch, Seat};
use crate::spaces::{Spaces, StartOptions};

/// What an agent starts from besides its name, pane, agent and directory.
#[derive(Default)]
pub(crate) struct AgentStart {
    /// A conversation to resume.
    pub conversation: Option<String>,
    /// A conversation to copy, and through how many turns.
    pub fork: Option<(String, usize)>,
    /// The agent's configuration directory, when it is not the one `agent.<name>.home` names.
    pub home: Option<String>,
    pub context: Option<ContextUse>,
    pub model: Option<String>,
    pub permissions: AgentPermissions,
}

/// An agent pane's choices while the pane is a terminal: the model and mode (config option id,
/// value) and the permissions it was started with.
#[derive(Clone)]
pub(crate) struct CarriedView {
    pub options: Vec<(String, String)>,
    pub permissions: AgentPermissions,
}

/// Messages queued for one client. When a client stops reading, its terminal forwarders block
/// here, fall behind the broadcast, and resync with a fresh replay instead of growing memory.
const OUTBOX: usize = 256;

/// The most replay bytes one `term_output` frame carries; a longer replay arrives as several
/// frames, so a large scrollback never becomes one WebSocket frame.
const REPLAY_CHUNK: usize = 64 * 1024;

/// Who ends a terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Owner {
    /// A pane shows it; it ends when the pane closes or changes kind, so the pane can still
    /// replay a shell that exited.
    Pane,
    /// Opened with `term_open`; it ends with `term_close`, or once its shell has exited and a
    /// client has been sent the exit. Until then `status` lists it with `alive: false`.
    Client,
}

pub(crate) struct Held {
    pub(crate) term: Arc<Term>,
    pub(crate) owner: Owner,
}

/// The waits around an agent starting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentTiming {
    pub restart: Restart,
    /// How long `start_pane` waits for a new agent to be ready or give up.
    pub start_wait: Duration,
}

impl Default for AgentTiming {
    fn default() -> Self {
        AgentTiming {
            restart: Restart::default(),
            start_wait: Duration::from_secs(60),
        }
    }
}

/// A connection and the columns and rows it asked a terminal to have.
type SizeRequest = (SocketAddr, u16, u16);

/// Which listener a request came in on. A token means different things on each: the local one
/// takes the token file's, the remote one only a paired device's credential (ADR-0015).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Listener {
    Local,
    Remote,
}

/// Who a request was admitted as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Caller {
    /// A client holding the token file.
    Local,
    /// A paired device, by id.
    Device(String),
}

/// What a terminal's agent last said it is doing, and for `needs_you`, what it waits on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Report {
    pub status: wheel_proto::ReportedStatus,
    pub reason: Option<String>,
}

pub struct AppState {
    started: Instant,
    /// The paired devices and the pairing tokens waiting to be used.
    pub(crate) devices: crate::devices::Devices,
    /// The signing token for Apple's push service.
    pub(crate) push: crate::push::Push,
    /// A device id whose pairing was just revoked; its connections close.
    pub(crate) revoked: broadcast::Sender<String>,
    /// Told once, when wheeld receives SIGTERM or SIGINT: every connection sends its client
    /// `stopping` and closes. See [`AppState::announce_stopping`].
    stopping: broadcast::Sender<()>,
    /// One token per open websocket connection; `announce_stopping` waits for them all to drop.
    connections: tokio_util::task::TaskTracker,
    /// The Mac app that owns the windows, once it has attached (ADR-0014).
    pub(crate) mac_apps: crate::mac_windows::MacApps,
    /// What each connected client's windows show, as the clients reported it (ADR-0022).
    pub(crate) views: crate::clients::Views,
    /// The remote listener, once it is bound: set by [`crate::remote::start`].
    pub(crate) remote: Mutex<Option<crate::remote::Remote>>,
    log_path: String,
    token: String,
    pub(crate) next_id: AtomicU64,
    pub(crate) terms: Mutex<HashMap<String, Held>>,
    /// Daemon-wide events (permission requests, every agent's state, layout) every connection
    /// forwards.
    pub(crate) global: broadcast::Sender<ServerMsg>,
    pub(crate) perms: Arc<Perms>,
    /// Every running agent by name: the ones agent panes own.
    pub(crate) agents: Mutex<HashMap<String, Arc<Agent>>>,
    pub(crate) agent_timing: Mutex<AgentTiming>,
    /// The connection that owns orchestrator, if any (`crate::orchestrator`).
    pub(crate) orchestrator_lease: Mutex<Option<SocketAddr>>,
    /// The events each orchestrator has not been told yet, by scope.
    pub(crate) orchestrator_events: Mutex<HashMap<String, std::collections::VecDeque<String>>>,
    /// Panes whose `start_agent` is still running, so a second agent start on one of them is
    /// refused instead of replacing the agent that is coming up. Other kind changes are not held.
    pub(crate) agent_starts: Mutex<HashSet<String>>,
    /// What an agent pane had when it became a terminal, by pane, so the agent view the switch
    /// back starts takes it again. Kept in memory only.
    pub(crate) view_options: Mutex<HashMap<String, CarriedView>>,
    pub(crate) spaces: Mutex<Spaces>,
    /// Top-level keys of the saved file this build does not know, written back on every save.
    pub(crate) saved_extra: Mutex<serde_json::Map<String, serde_json::Value>>,
    /// Directories spaces were made for, newest first, one row per directory, at most
    /// [`crate::spaces::RECENT_LIMIT`]. Saved beside the layout.
    pub(crate) recent: Mutex<Vec<wheel_proto::RecentDir>>,
    /// Where the layout is saved; None keeps it in memory only.
    pub(crate) state_file: Mutex<Option<PathBuf>>,
    /// This instance's own directory, holding its token and saved layout.
    pub(crate) instance_dir: PathBuf,
    /// This machine's `wheel.toml`: its settings and its `machine` artifact entries. `machine`
    /// artifacts' folders live beside it, so every project on this machine can open them.
    pub(crate) machine_file: PathBuf,
    /// Every Claude login's usage, as the usage strip shows it.
    pub(crate) usage: crate::usage::Usage,
    /// Every Codex login's usage; `usage` above holds Claude's.
    pub(crate) codex_usage: crate::usage::Usage,
    /// The program a refresh starts as its warden: `wheeld` itself.
    pub(crate) warden: PathBuf,
    /// One writer at a time for a `wheel.toml`, whichever file: a settings write is a read, a
    /// change and a rename, and two of them interleaved would drop a key.
    pub(crate) settings_write: Mutex<()>,
    /// The watch on the `wheel.toml` files, and what clients were last told each scope resolves to.
    pub(crate) settings_watch: Mutex<Option<crate::settings_watch::Watch>>,
    pub(crate) settings_seen: Mutex<crate::settings_watch::Seen>,
    /// The folders watched for plugin script saves, and the script each plugin pane last sent.
    pub(crate) plugin_watch: Mutex<Option<crate::plugins::Watch>>,
    pub(crate) plugin_seen: Mutex<crate::plugins::Seen>,
    /// One built-in install, update or removal at a time: each reads `installed.json`, changes
    /// files and writes it back.
    pub(crate) builtins_write: Mutex<()>,
    /// Whether a coalesced save of the layout is already scheduled, so a burst of browser
    /// navigations is one write rather than one each.
    pub(crate) save_pending: AtomicBool,
    /// Panes whose conversation is moving between its chat and its terminal, so a second switch
    /// cannot start a second program on the same conversation.
    pub(crate) switching_views: Mutex<std::collections::HashSet<String>>,
    /// How many times the layout has been written, reported by `status`: the one thing a program
    /// can compare to tell a coalesced save from a save per change.
    pub(crate) saves: AtomicU64,
    /// Client messages handled since start, keyed `<client> <op>` (client `mac`, `local` or
    /// `device`), reported by `status`.
    pub(crate) requests: Mutex<std::collections::BTreeMap<String, u64>>,
    /// What each web pane watches — its artifact's folder and its own data directory — keyed by
    /// pane id.
    pub(crate) watchers: Mutex<HashMap<String, Vec<crate::web::Watch>>>,
    /// The state store and pushed data of each web pane whose page or CLI has used them, keyed by
    /// pane id.
    pub(crate) pages: Mutex<HashMap<String, Arc<crate::page::Page>>>,
    /// The refresh runner of each web pane whose artifact declares a command, keyed by pane id.
    pub(crate) refreshers: Mutex<crate::refresh::Refreshers>,
    /// The app process of each web pane whose artifact declares one, keyed by pane id.
    pub(crate) apps: Mutex<crate::app::Apps>,
    /// The MCP session of each artifact entry that names a server.
    pub(crate) mcp: crate::mcp::Sessions,
    /// The `[runner]` watch of each web pane showing the runner built-in, keyed by pane id.
    pub(crate) runner_feeds: Mutex<HashMap<String, crate::runner_feed::Feed>>,
    /// Each space's title-row buttons: the `state` output seen and the list clients were told (ADR-0037).
    pub(crate) title_buttons: Mutex<crate::title_buttons::Buttons>,
    /// The merged streams started since this daemon began, by space and group (ADR-0030).
    pub(crate) streams: crate::streams::Streams,
    /// This state, for callbacks an agent runs later and for a task that saves the layout after
    /// the caller has let go of its lock.
    pub(crate) me: Weak<AppState>,
    /// The connections that offered to draw panes, and the draws waiting on them.
    pub(crate) renderers: crate::render::Renderers,
    /// The size each connection last asked of each terminal, oldest first. A terminal has one
    /// size: the newest request's. When that connection detaches or disconnects, the terminal
    /// returns to the next newest instead of keeping a size nobody is showing.
    pub(crate) term_sizes: Mutex<HashMap<String, Vec<SizeRequest>>>,
    /// Where the `wheel` command finds this daemon, handed to every terminal and agent process.
    /// None for a daemon nothing connects to by address (a test that drives it in process).
    pub(crate) control: Mutex<Option<Control>>,
    /// Why the shell integration is not fully in place, as `set_control` found it.
    pub(crate) shell_problem: Mutex<Option<String>>,
    /// The PATH every agent lookup, terminal, agent and refresh command is given, and where it
    /// came from. Held here rather than in the process environment, because a shell asked again
    /// after startup answers while other threads run, when changing the environment is unsound.
    pub(crate) login_path: Arc<Mutex<wheel_proto::LoginPath>>,
    /// The agent panes' agents a restore kept as saved because their agent could not start,
    /// started once the login shell's PATH arrives.
    pub(crate) kept_agents: Mutex<std::collections::HashSet<String>>,
    /// Conversations a switch is moving an agent into now, by id and the agent's name, so two
    /// switches to one id cannot both pass the check that no other agent is in it.
    pub(crate) switching_into: Mutex<std::collections::HashMap<String, String>>,
    /// The pane and space statuses last sent, so a change is sent once.
    pub(crate) statuses: Mutex<crate::status::StatusBook>,
    /// What the agent in each terminal says it is doing, by terminal id: the last report, kept
    /// until the sampler sees the agent is no longer there.
    pub(crate) reports: Mutex<std::collections::HashMap<String, Report>>,
    /// The context window each terminal's agent says it has, by terminal id; kept and dropped
    /// like `reports`.
    pub(crate) context_reports: Mutex<std::collections::HashMap<String, ContextUse>>,
    /// What each browser pane's page wrote to its console since its last load.
    pub(crate) console: Mutex<crate::console::ConsoleBuffers>,
    /// The conversation Claude Code says it is in, by terminal id: what its `SessionStart` hook
    /// last reported. Read by the sampler, which saves it as the pane's agent.
    pub(crate) conversation_reports: Mutex<std::collections::HashMap<String, String>>,
    /// The usage journal, the apps' log lines, and the shipper that sends them and `wheeld.log`
    /// to the log store.
    pub(crate) logship: crate::logship::Logship,
}

impl AppState {
    /// Tell every connected client that wheeld is stopping and wait until each connection has
    /// written that and closed, for at most two seconds: a client that sees the socket drop right
    /// after `stopping` knows a stop or restart is under way, not a crash.
    pub fn open_connections(&self) -> usize {
        self.connections.len()
    }

    pub async fn announce_stopping(&self) {
        let started = Instant::now();
        // Closed before the broadcast: a connection that opens in between sees `is_closed` and
        // tells its own client, so none misses `stopping`.
        self.connections.close();
        let told = self.stopping.send(()).unwrap_or(0);
        match tokio::time::timeout(Duration::from_secs(2), self.connections.wait()).await {
            Ok(()) => tracing::info!(
                clients = told,
                ms = started.elapsed().as_millis() as u64,
                "told every client wheeld is stopping"
            ),
            Err(_) => tracing::warn!(clients = told, "some clients did not take `stopping` in 2 s; stopping anyway"),
        }
    }

    /// `warden` is the program a refresh starts as its warden ([`crate::refresh::WARDEN_ARG`]):
    /// the `wheeld` binary.
    pub fn new(
        log_path: String,
        token: String,
        instance_dir: PathBuf,
        warden: PathBuf,
    ) -> Arc<AppState> {
        let machine_file = instance_dir.join(crate::settings::FILE);
        Self::with_machine_file(log_path, token, instance_dir, machine_file, warden)
    }

    /// [`AppState::new`] with this machine's `wheel.toml` at `machine_file`. `new` keeps it inside
    /// `instance_dir`, so a test or an embedder never touches the real `~/.wheel/wheel.toml`; the
    /// `wheeld` binary passes `--machine-file`, which defaults to that path.
    pub fn with_machine_file(
        log_path: String,
        token: String,
        instance_dir: PathBuf,
        machine_file: PathBuf,
        warden: PathBuf,
    ) -> Arc<AppState> {
        // The daemon's own directory: its machine settings are written there
        // whether or not anything else made it first.
        if let Err(e) = std::fs::create_dir_all(&instance_dir) {
            tracing::warn!(dir = %instance_dir.display(), error = %e, "cannot create the instance directory");
        }
        if let Some(dir) = machine_file.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!(dir = %dir.display(), error = %e, "cannot create the machine file's directory");
            }
        }
        let (global, _) = broadcast::channel(1024);
        let devices = crate::devices::Devices::load(&instance_dir);
        let (revoked, _) = broadcast::channel(16);
        let (stopping, _) = broadcast::channel(1);
        let logship = crate::logship::Logship::new(&instance_dir, &log_path);
        Arc::new_cyclic(|me| AppState {
            logship,
            started: Instant::now(),
            devices,
            revoked,
            stopping,
            connections: Default::default(),
            push: Default::default(),
            mac_apps: Default::default(),
            views: Default::default(),
            remote: Mutex::new(None),
            log_path,
            token,
            next_id: AtomicU64::new(1),
            terms: Mutex::new(HashMap::new()),
            perms: Arc::new(Perms::new(global.clone())),
            global,
            agents: Mutex::new(HashMap::new()),
            agent_timing: Mutex::new(AgentTiming::default()),
            orchestrator_lease: Mutex::new(None),
            orchestrator_events: Mutex::new(HashMap::new()),
            agent_starts: Mutex::new(HashSet::new()),
            view_options: Mutex::new(HashMap::new()),
            spaces: Mutex::new(Spaces::default()),
            saved_extra: Mutex::new(Default::default()),
            recent: Mutex::new(Vec::new()),
            state_file: Mutex::new(None),
            settings_write: Mutex::new(()),
            settings_watch: Mutex::new(None),
            settings_seen: Mutex::new(Default::default()),
            plugin_watch: Mutex::new(None),
            plugin_seen: Mutex::new(Default::default()),
            builtins_write: Mutex::new(()),
            save_pending: AtomicBool::new(false),
            switching_views: Mutex::new(Default::default()),
            saves: AtomicU64::new(0),
            requests: Mutex::default(),
            instance_dir,
            machine_file,
            usage: crate::usage::Usage::default(),
            codex_usage: crate::usage::Usage::codex(),
            warden,
            watchers: Mutex::new(HashMap::new()),
            pages: Mutex::new(HashMap::new()),
            refreshers: Mutex::new(crate::refresh::Refreshers::new()),
            apps: Mutex::new(crate::app::Apps::new()),
            mcp: Default::default(),
            runner_feeds: Mutex::new(HashMap::new()),
            title_buttons: Mutex::new(crate::title_buttons::Buttons::default()),
            streams: Default::default(),
            me: me.clone(),
            renderers: Default::default(),
            term_sizes: Mutex::new(HashMap::new()),
            control: Mutex::new(None),
            shell_problem: Mutex::new(None),
            login_path: Arc::new(Mutex::new(wheel_proto::LoginPath {
                path: std::env::var("PATH").unwrap_or_default(),
                how: "the PATH wheeld was started with".into(),
                problem: None,
            })),
            kept_agents: Mutex::new(Default::default()),
            switching_into: Mutex::new(Default::default()),
            statuses: Mutex::new(Default::default()),
            reports: Mutex::new(Default::default()),
            context_reports: Mutex::new(Default::default()),
            console: Mutex::new(Default::default()),
            conversation_reports: Mutex::new(Default::default()),
        })
    }

    /// Use `login_path` for everything started from now on, and report it in `status`.
    pub fn set_login_path(&self, login_path: wheel_proto::LoginPath) {
        match &login_path.problem {
            None => tracing::info!(path = %login_path.path, how = %login_path.how, "PATH"),
            Some(why) => {
                tracing::warn!(path = %login_path.path, how = %login_path.how, problem = %why, "PATH")
            }
        }
        *self.login_path.lock().unwrap() = login_path;
    }

    /// The PATH a lookup or a process started now gets.
    pub(crate) fn path(&self) -> String {
        self.login_path.lock().unwrap().path.clone()
    }

    /// Ask `shell` for PATH again every [`crate::login_path::RETRY`] until it gives one or fails
    /// in a way asking again cannot fix, after a first ask ran out of time. Each answer is
    /// reported in `status`, and every client is sent a fresh status once the shell gives one,
    /// since that changes which agents are found.
    pub fn ask_login_path_again(&self, shell: String, own: String) {
        let Some(state) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            for try_ in 2.. {
                tokio::time::sleep(crate::login_path::RETRY).await;
                let (shell, own) = (shell.clone(), own.clone());
                let Ok((login_path, again)) =
                    tokio::task::spawn_blocking(move || crate::login_path::ask(&shell, &own, try_))
                        .await
                else {
                    return;
                };
                let found = login_path.problem.is_none();
                state.set_login_path(login_path);
                if found {
                    state.log_agents();
                    state.start_unstarted_agents();
                    let _ = state.global.send(state.status(None).await);
                }
                if !again {
                    return;
                }
            }
        });
    }

    /// Tell every terminal and agent started from here where to reach this daemon with the `wheel`
    /// command at `cli`, and write the shell integration they start with. Call before restoring a
    /// layout.
    pub fn set_control(&self, url: String, token_file: PathBuf, cli: Option<PathBuf>) {
        let cli = cli.map(|cli| cli.canonicalize().unwrap_or(cli));
        let problem = match crate::shell::install(&self.instance_dir, cli.as_deref()) {
            Err(e) => {
                tracing::warn!(dir = %self.instance_dir.display(), error = %e, "cannot write the shell integration");
                Some(format!("cannot write the shell integration: {e}"))
            }
            Ok(()) if cli.is_none() => {
                tracing::warn!(
                    "no wheel command given with --cli; terminals and agents cannot run it"
                );
                Some(
                    "no wheel command given with --cli, so terminals and agents cannot run it"
                        .to_string(),
                )
            }
            Ok(()) => None,
        };
        *self.shell_problem.lock().unwrap() = problem;
        tracing::info!(%url, token_file = %token_file.display(), cli = ?cli, "wheel control for terminals and agents");
        *self.control.lock().unwrap() = Some(Control { url, token_file });
    }

    /// How to start a terminal for `seat` (its space and pane), running `shell` or the login shell.
    fn term_launch(&self, shell: Option<String>, seat: Option<Seat>) -> Launch {
        match self.control.lock().unwrap().as_ref() {
            Some(control) => crate::shell::launch(
                shell,
                &self.path(),
                std::env::var("SHELL").ok().as_deref(),
                std::env::var("ZDOTDIR").ok().as_deref(),
                control,
                &self.instance_dir,
                seat,
            ),
            None => Launch {
                program: shell,
                ..Default::default()
            },
        }
    }

    /// The environment for the agent in `pane` of `space`: where `wheel` reaches this daemon, and a
    /// `PATH` that finds it. Empty when this daemon has no address to give.
    pub(crate) fn agent_env(&self, pane: &str) -> Vec<(String, String)> {
        self.agent_env_for(Some(pane))
    }

    /// [`AppState::agent_env`] for an agent that may have no pane: with none, `WHEEL_PANE` is not set.
    pub(crate) fn agent_env_for(&self, pane: Option<&str>) -> Vec<(String, String)> {
        let Some(control) = self.control.lock().unwrap().clone() else {
            return Vec::new();
        };
        let mut env = crate::shell::env(&control, &self.instance_dir, pane);
        env.push(("WHEEL_AGENT_PANE".into(), "1".into()));
        env.push((
            "PATH".into(),
            crate::shell::agent_path(&self.instance_dir, &self.path()),
        ));
        env
    }

    /// The artifacts section of the brief for a pane in the space whose directory is `space_cwd`,
    /// and the pane's brief file (`WHEEL_BRIEF_FILE`) written with it. A listing that cannot be
    /// read is said so in the section, not left out.
    pub(crate) fn pane_brief(&self, pane: &str, space_cwd: &str) -> String {
        let section = match self.artifact_ls_in(space_cwd, false) {
            Ok((artifacts, _)) => crate::brief::artifacts_section(&artifacts),
            Err(e) => {
                tracing::warn!(%pane, error = %e, "the brief cannot list the artifacts");
                crate::brief::unreadable_section(&e)
            }
        };
        let all = self.guidance_text(None);
        self.write_brief(
            pane,
            &crate::shell::brief_file(&self.instance_dir, Some(pane)),
            &crate::brief::file_text(&section, &crate::brief::guidance_section(&all, "", "")),
        );
        // A terminal runs whichever agent the person types, so an agent with guidance of its own
        // gets a brief of its own; one without keeps the pane's.
        for agent in crate::settings::agent_names() {
            let own = self.guidance_text(Some(agent));
            let file = crate::shell::agent_brief_file(&self.instance_dir, pane, agent);
            if own.trim().is_empty() {
                let _ = std::fs::remove_file(&file);
                continue;
            }
            let guidance = crate::brief::guidance_section(&all, agent, &own);
            self.write_brief(pane, &file, &crate::brief::file_text(&section, &guidance));
        }
        section
    }

    /// Write one brief file of pane `pane`, and say so.
    fn write_brief(&self, pane: &str, file: &std::path::Path, text: &str) {
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(file, text));
        match written {
            Ok(()) => {
                tracing::info!(%pane, file = %file.display(), bytes = text.len(), "wrote the pane's brief")
            }
            Err(e) => {
                tracing::warn!(%pane, file = %file.display(), error = %e, "cannot write the pane's brief")
            }
        }
    }

    /// The configuration directory the person's login shell gives an agent started in `cwd`, asked
    /// from the primary checkout `cwd` belongs to: a terminal opened there holds it, set by the
    /// person's own startup files, so an agent picks the same account a terminal would. None when
    /// `agent.<name>.home` is set (it decides), when the shell leaves the variable unset, or when it
    /// is the daemon's own. Err when the shell gave no answer, since the agent would otherwise
    /// start in whichever account the default is.
    pub(crate) async fn shell_agent_home(
        &self,
        agent: &str,
        cwd: &str,
    ) -> Result<Option<String>, String> {
        if self.agent_home(agent).is_some() {
            return Ok(None);
        }
        let Some(harness) = Harness::parse(agent) else {
            return Ok(None);
        };
        let asked = crate::checkout::primary(Path::new(cwd));
        let var = harness.home_var();
        let shell = crate::login_path::shell();
        let answer = {
            let (program, dir) = (shell.clone(), asked.clone());
            tokio::task::spawn_blocking(move || {
                crate::login_path::probe_in(&program, &dir, var, crate::login_path::LIMIT)
            })
            .await
            .map_err(|e| format!("asking {shell} for {var} failed: {e}"))?
        }
        .map_err(|f| format!("{shell} gave no {var} for {}: {}", asked.display(), f.why))?;
        tracing::info!(%agent, %var, asked = %asked.display(), home = %answer, "agent home from the login shell");
        let home = (!answer.is_empty()).then(|| PathBuf::from(answer));
        Ok(home
            .filter(|h| Some(h) != harness.daemon_home().as_ref())
            .map(|h| h.display().to_string()))
    }

    /// Start `shell`, else the login shell, on a new PTY and keep it until its owner ends it.
    /// `seat` is the pane the terminal belongs to and the directory of the space holding it.
    pub(crate) async fn open_term(
        &self,
        cwd: String,
        shell: Option<String>,
        cols: u16,
        rows: u16,
        owner: Owner,
        seat: Option<(Seat<'_>, &str)>,
    ) -> Result<Arc<Term>, String> {
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        if let Some((pane, space_cwd)) = seat {
            self.pane_brief(pane, space_cwd);
        }
        let launch = self.term_launch(shell, seat.map(|(pane, _)| pane));
        let spawned = {
            let (id, cwd) = (id.clone(), cwd.clone());
            tokio::task::spawn_blocking(move || {
                Term::spawn(id, cwd, launch, cols.max(1), rows.max(1))
            })
            .await
        };
        match spawned {
            Ok(Ok(term)) => {
                tracing::info!(term = %id, pid = ?term.pid, %cwd, cols, rows, ?owner, "pty spawned");
                let held = Held {
                    term: term.clone(),
                    owner,
                };
                self.terms.lock().unwrap().insert(id, held);
                Ok(term)
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, %cwd, "pty spawn failed");
                Err(format!("spawn failed: {e}"))
            }
            Err(e) => Err(format!("spawn task failed: {e}")),
        }
    }

    /// Replace how long agents take to restart and how long `start_pane` waits for one. Tests
    /// shorten these; the daemon runs on the defaults.
    pub fn set_agent_timing(&self, timing: AgentTiming) {
        *self.agent_timing.lock().unwrap() = timing;
    }

    /// Start agent `name` of the agent called `agent` for `pane`, in `cwd`, with the person's configuration for it (or
    /// `start.home`, else the agent's `agent.<name>.home` setting) and Wheel's brief.
    /// `start.conversation` is one to resume: the one a restored pane saved, or one switched over
    /// from a terminal. One that does not reopen falls through to a new one.
    pub(crate) fn spawn_agent(
        &self,
        name: &str,
        (pane, agent): (&str, &str),
        (cwd, space_cwd): (&str, &str),
        start: AgentStart,
    ) -> Result<(), String> {
        let AgentStart {
            conversation,
            fork,
            home,
            context,
            model,
            permissions,
        } = start;
        let command = self.agent_ready(agent)?.command;
        let harness = Harness::parse(agent).ok_or_else(|| format!("no agent {agent}"))?;
        let config = AgentConfig {
            agent: harness,
            command,
            cwd: cwd.into(),
            home: home.map(PathBuf::from).or_else(|| self.agent_home(agent)),
            brief: Some(crate::brief::text(
                pane,
                name,
                &self.pane_brief(pane, space_cwd),
                &self.guidance_for(agent),
            )),
            env: self.agent_env(pane),
            restored: conversation.is_some(),
            conversation,
            fork,
            context,
            on_saved: Some(Arc::new({
                let me = self.me.clone();
                move || {
                    if let Some(state) = me.upgrade() {
                        state.conversations_changed();
                    }
                }
            })),
            restart: self.agent_timing.lock().unwrap().restart,
            model,
            permissions,
            orchestrator: false,
        };
        let agent = Agent::start(
            name.into(),
            Some(pane.into()),
            config,
            self.perms.clone(),
            self.global.clone(),
        );
        self.agents.lock().unwrap().insert(name.into(), agent);
        Ok(())
    }

    /// Start the orchestrator's agent `name` in `cwd` on `model`, with `brief` as its whole
    /// instructions (`crate::orchestrator`). It belongs to no pane.
    pub(crate) fn spawn_orchestrator_agent(&self, name: &str, cwd: &str, brief: String, model: &str) -> Result<bool, String> {
        let command = self.agent_ready("claude")?.command;
        let config = AgentConfig {
            agent: Harness::Claude,
            command,
            cwd: cwd.into(),
            home: self.agent_home("claude"),
            brief: Some(brief),
            env: {
                let mut env = self.agent_env_for(None);
                env.push(("WHEEL_ORCHESTRATOR".into(), "1".into()));
                env
            },
            restored: false,
            conversation: None,
            fork: None,
            context: None,
            on_saved: None,
            restart: self.agent_timing.lock().unwrap().restart,
            model: Some(model.into()),
            permissions: AgentPermissions::default(),
            orchestrator: true,
        };
        // One lock holds the check and the insert, so two starts never leave a process unowned.
        let mut agents = self.agents.lock().unwrap();
        if agents.contains_key(name) {
            return Ok(false);
        }
        let agent = Agent::start(name.into(), None, config, self.perms.clone(), self.global.clone());
        agents.insert(name.into(), agent);
        Ok(true)
    }

    /// End an agent for good; its agent process is killed.
    pub(crate) fn end_agent(&self, name: &str) {
        let agent = self.agents.lock().unwrap().remove(name);
        if let Some(agent) = agent {
            agent.stop(&self.perms);
        }
    }

    /// [`AppState::end_agent`], returning once the agent's process is gone.
    pub(crate) async fn end_agent_wait(&self, name: &str) {
        let agent = self.agents.lock().unwrap().remove(name);
        if let Some(agent) = agent {
            agent.stop_and_wait(&self.perms).await;
        }
    }

    /// End every agent; their agent processes are killed. A daemon
    /// that stops serving without its process ending (an in-process restart in a test) calls this
    /// first, so its agents do not outlive it.
    pub fn stop_agents(&self) {
        let agents: Vec<_> = self
            .agents
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        for agent in agents {
            agent.stop(&self.perms);
        }
    }

    /// Let a request in, or say what to refuse it with. Browsers always send Origin and native
    /// clients never do, so no web page gets a shell or an agent; everyone else needs the
    /// token. `via` names the route in the log.
    pub(crate) fn admit(
        &self,
        via: &str,
        headers: &HeaderMap,
        token: Option<&str>,
        listener: Listener,
    ) -> Result<Caller, StatusCode> {
        if let Some(origin) = headers.get("origin") {
            tracing::warn!(via, ?origin, "refused: browser origin");
            return Err(StatusCode::FORBIDDEN);
        }
        self.check_token(via, token, listener)
    }

    /// Let a request carrying `token` in, or refuse it with 401. The local listener takes the
    /// token file's token; the remote one takes a paired device's credential and nothing else.
    pub(crate) fn check_token(
        &self,
        via: &str,
        token: Option<&str>,
        listener: Listener,
    ) -> Result<Caller, StatusCode> {
        let admitted = match (listener, token) {
            (_, None) => None,
            (Listener::Local, Some(t)) => {
                // Compare every byte, so the time taken says nothing about how much of a guess matched.
                let matches = t.len() == self.token.len()
                    && t.bytes()
                        .zip(self.token.bytes())
                        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                        == 0;
                matches.then_some(Caller::Local)
            }
            (Listener::Remote, Some(t)) => self.devices.authenticate(t).map(Caller::Device),
        };
        admitted.ok_or_else(|| {
            tracing::warn!(via, ?listener, "refused: missing or wrong token");
            StatusCode::UNAUTHORIZED
        })
    }

    /// Tell every client the paired devices changed.
    pub(crate) fn devices_changed(&self) {
        let _ = self.global.send(ServerMsg::DevicesChanged {
            devices: self.devices.list(),
        });
    }

    pub(crate) fn agent(&self, name: &str) -> Result<Arc<Agent>, String> {
        match self.agents.lock().unwrap().get(name) {
            Some(agent) => Ok(agent.clone()),
            None => Err(format!("no agent {name}")),
        }
    }

    pub(crate) fn term(&self, id: &str) -> Option<Arc<Term>> {
        self.terms.lock().unwrap().get(id).map(|h| h.term.clone())
    }

    /// Size a terminal to what `peer` asked, making it the newest request.
    fn request_size(&self, peer: SocketAddr, id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        let term = self
            .term(id)
            .ok_or_else(|| anyhow::anyhow!("no terminal {id}"))?;
        let mut sizes = self.term_sizes.lock().unwrap();
        let asked = sizes.entry(id.to_string()).or_default();
        asked.retain(|(p, _, _)| *p != peer);
        asked.push((peer, cols, rows));
        term.resize(cols, rows)
    }

    /// Drop `peer`'s size request from one terminal, or from all of them, and give each
    /// terminal the size of the newest request left. A terminal nobody asks for keeps its size.
    fn release_sizes(&self, peer: SocketAddr, only: Option<&str>) {
        let mut sizes = self.term_sizes.lock().unwrap();
        let ids: Vec<String> = sizes
            .iter()
            .filter(|(id, _)| only.is_none_or(|o| o == id.as_str()))
            .filter(|(_, asked)| asked.iter().any(|(p, _, _)| *p == peer))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let asked = sizes.get_mut(&id).expect("listed above");
            asked.retain(|(p, _, _)| *p != peer);
            let Some(&(next, cols, rows)) = asked.last() else {
                sizes.remove(&id);
                continue;
            };
            match self.term(&id) {
                Some(term) => {
                    tracing::info!(%peer, term = %id, cols, rows, restored_to = %next, "resize");
                    if let Err(e) = term.resize(cols, rows) {
                        tracing::warn!(term = %id, error = %e, "restoring size failed");
                    }
                }
                None => {
                    sizes.remove(&id);
                }
            }
        }
    }

    /// Forget a `term_open` terminal whose exit a client has just been sent. Clients already
    /// streaming it hold their own reference and get the exit too; attaching later finds nothing.
    fn exit_delivered(&self, id: &str) {
        let mut terms = self.terms.lock().unwrap();
        if terms.get(id).is_some_and(|h| h.owner == Owner::Client) {
            terms.remove(id);
            tracing::info!(term = %id, "exited terminal dropped");
        }
    }

    /// Hang up a terminal opened with `term_open` and forget it.
    fn term_close(&self, id: &str) -> Result<(), String> {
        let mut terms = self.terms.lock().unwrap();
        match terms.get(id).map(|h| h.owner) {
            None => Err(format!("no terminal {id}")),
            Some(Owner::Pane) => Err(format!(
                "terminal {id} belongs to a pane; close the pane or change its kind instead"
            )),
            Some(Owner::Client) => {
                if let Some(held) = terms.remove(id) {
                    held.term.kill();
                }
                tracing::info!(term = %id, "term closed");
                Ok(())
            }
        }
    }

    async fn status(self: &Arc<Self>, req: Option<u64>) -> ServerMsg {
        let mut terms: Vec<_> = self
            .terms
            .lock()
            .unwrap()
            .values()
            .map(|h| h.term.info())
            .collect();
        terms.sort_by(|a, b| a.id.cmp(&b.id));
        let Spaces { spaces, sessions } = self.listing();
        let dirs: Vec<std::path::PathBuf> = spaces.iter().map(|s| s.cwd.clone().into()).collect();
        // Reading a project folder can stall on a guarded or synced volume.
        let scanner = self.clone();
        let agents = tokio::task::spawn_blocking(move || scanner.agent_status(&dirs))
            .await
            .unwrap_or_default();
        // The reply waits at most 300 ms for a guarded folder that has not answered (host.rs); keep
        // even that off the async threads.
        let host = tokio::task::spawn_blocking(crate::host::access)
            .await
            .unwrap_or_default();
        let requests = self.requests.lock().unwrap().clone();
        ServerMsg::Status {
            req,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: wheel_proto::PROTOCOL_VERSION,
            uptime_secs: self.started.elapsed().as_secs(),
            log_path: self.log_path.clone(),
            terms,
            perms: self.perms.list(),
            spaces,
            sessions,
            layout_saves: self.saves.load(Ordering::SeqCst),
            requests,
            watches: self.watches(),
            refreshes: self.refreshes().await,
            usage_board: self.usage_board(),
            agents,
            shell_integration: self.shell_problem.lock().unwrap().clone(),
            login_path: self.login_path.lock().unwrap().clone(),
            logs: Box::new(self.logship.status(&self.push_url())),
            host: Box::new(host),
            streams: self.streams.statuses(),
        }
    }

    /// The log store's push URL from this machine's settings; empty when none is set.
    fn push_url(&self) -> String {
        self.setting(crate::settings::LOGS_PUSH_URL, None)
            .unwrap_or_default()
    }

    /// Send every log line waiting to the log store, as the daemon's timer does. Blocking.
    pub fn ship_now(&self) {
        let retry = self
            .setting(crate::settings::LOGS_RETRY_S, None)
            .ok()
            .and_then(|s| s.parse().ok())
            .map_or(crate::logship::SHIP_EVERY, Duration::from_secs);
        self.logship.ship(&self.push_url(), retry);
    }
}

/// How often the daemon looks for artifacts whose lifetime is over.
const ARTIFACT_SWEEP: Duration = Duration::from_secs(30);

/// Serve clients on `listener` until the process ends.
pub async fn serve(listener: TcpListener, state: Arc<AppState>) -> anyhow::Result<()> {
    crate::status::follow(state.clone());
    crate::push::follow(state.clone());
    state.sync_settings_watch();
    state.sync_plugin_watch();
    state.builtins_catch_up();
    let shipper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(crate::logship::SHIP_EVERY);
        loop {
            tick.tick().await;
            // A push blocks on the network, so it runs off the async runtime's threads.
            let shipper = shipper.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || shipper.ship_now()).await {
                tracing::error!(error = %e, "the log shipper panicked");
            }
        }
    });
    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ARTIFACT_SWEEP);
        loop {
            tick.tick().await;
            // The sweep reads and deletes files, so it runs off the async runtime's threads.
            let sweeper = sweeper.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || sweeper.artifact_sweep()).await {
                tracing::error!(error = %e, "the artifact sweep panicked");
            }
        }
    });
    axum::serve(
        listener,
        app(state, Listener::Local).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// The routes every listener serves, the local one and the remote one alike, so a token means the
/// same thing on both.
pub(crate) fn app(state: Arc<AppState>, listener: Listener) -> Router {
    Router::new()
        .route("/ws", get(upgrade))
        .with_state(state.clone())
        .merge(crate::web::router(state.clone()))
        .merge(crate::uploads::router(state))
        .layer(axum::Extension(listener))
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::Extension(listener): axum::Extension<Listener>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let via = format!("ws {peer}");
    // A phone with no credential yet presents its pairing token as `pair`, with its own name; the
    // daemon spends the token and sends the credential as the connection's first frame.
    if let Some(pair) = query.get("pair") {
        if listener != Listener::Remote {
            tracing::warn!(via, "refused: a pairing token is for the remote listener");
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if headers.get("origin").is_some() {
            tracing::warn!(via, "refused: browser origin");
            return StatusCode::FORBIDDEN.into_response();
        }
        let name = query.get("name").map(String::as_str).unwrap_or_default();
        return match state.devices.redeem(pair, name) {
            Ok((id, credential)) => {
                tracing::info!(via, device = %id, "paired a device");
                ws.on_upgrade(move |socket| {
                    connection(socket, state, peer, Caller::Device(id), Some(credential))
                })
            }
            Err(why) => {
                tracing::warn!(via, why, "refused: pairing token");
                StatusCode::UNAUTHORIZED.into_response()
            }
        };
    }
    let caller = match state.admit(
        &via,
        &headers,
        query.get("token").map(String::as_str),
        listener,
    ) {
        Ok(caller) => caller,
        Err(status) => return status.into_response(),
    };
    ws.on_upgrade(move |socket| connection(socket, state, peer, caller, None))
}

async fn connection(
    socket: WebSocket,
    state: Arc<AppState>,
    peer: SocketAddr,
    caller: Caller,
    credential: Option<String>,
) {
    // A local connection is mostly a short-lived `wheel` command, thousands a day; the Mac app
    // announces itself with its own "Mac app attached" line, and a paired device is worth a line.
    match &caller {
        Caller::Local => tracing::debug!(%peer, ?caller, "ws connect"),
        Caller::Device(_) => tracing::info!(%peer, ?caller, "ws connect"),
    }
    let _open = state.connections.token();
    let mut stopping = state.stopping.subscribe();
    let mut stopped = false;
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::channel::<ServerMsg>(OUTBOX);
    let device = match &caller {
        Caller::Device(id) => Some(id.clone()),
        Caller::Local => None,
    };
    // Subscribed before the device is counted as connected: a revoke from here on closes this
    // connection, and one that already happened is seen right below.
    let mut revoked = state.revoked.subscribe();
    if let Some(id) = &device {
        if state.devices.name(id).is_none() {
            tracing::warn!(%peer, device = %id, "device revoked before its connection opened; closing it");
            return;
        }
        if let Some(credential) = credential.clone() {
            let _ = out
                .send(ServerMsg::Paired {
                    device: id.clone(),
                    credential,
                })
                .await;
        }
        state.devices.opened(id);
        let _ = state.global.send(ServerMsg::DeviceConnected {
            device: id.clone(),
            name: state.devices.name(id).unwrap_or_default(),
            address: peer.ip().to_string(),
            paired: credential.is_some(),
        });
        state.devices_changed();
    }
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).expect("server messages serialize");
            if sink.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });
    // wheeld began stopping before this connection opened: tell this client too.
    if state.connections.is_closed() {
        let _ = out.send(ServerMsg::Stopping).await;
        stopped = true;
    }
    // Every connection sees permission requests and agent state changes, attached or not.
    let mut global = state.global.subscribe();
    let global_out = out.clone();
    let global_state = state.clone();
    let local_only = device.is_some();
    let global_task = tokio::spawn(async move {
        loop {
            match global.recv().await {
                // Who is paired, and who connected from where, is for the machine's own clients.
                Ok(ServerMsg::DevicesChanged { .. } | ServerMsg::DeviceConnected { .. })
                    if local_only => {}
                Ok(msg) => {
                    if global_out.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(%peer, skipped = n, "client fell behind on daemon events; sending a fresh status");
                    if global_out
                        .send(global_state.status(None).await)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    // One output stream per terminal and agent on this connection; attaching again replaces it.
    let mut forwards: HashMap<String, JoinHandle<()>> = HashMap::new();

    while !stopped {
        let frame = tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(frame)) => frame,
                _ => break,
            },
            // wheeld is stopping: the client is told, then the connection closes below.
            _ = stopping.recv() => {
                let _ = out.send(ServerMsg::Stopping).await;
                stopped = true;
                break;
            }
            // Any revocation: the device is gone from the store when it was this one's.
            _ = revoked.recv(), if device.is_some() => {
                if device.as_deref().is_some_and(|id| state.devices.name(id).is_none()) {
                    tracing::warn!(%peer, ?caller, "device revoked; closing its connection");
                    break;
                }
                continue;
            }
        };
        let Message::Text(text) = frame else { continue };
        // Read once as a value: the journal takes the op, space and pane from it, and the message
        // is read from the same value.
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        match ClientMsg::deserialize(&raw) {
            Ok(msg) => {
                let started = Instant::now();
                let outcome = handle(&state, msg, &out, &mut forwards, peer, &caller).await;
                let device = match &caller {
                    Caller::Device(id) => state.devices.name(id),
                    Caller::Local => None,
                };
                let client = match &caller {
                    Caller::Device(_) => "device",
                    Caller::Local if state.mac_apps.is_app(peer) => "mac",
                    Caller::Local => "local",
                };
                let field = |key: &str| raw.get(key).and_then(|v| v.as_str());
                *state
                    .requests
                    .lock()
                    .unwrap()
                    .entry(format!("{client} {}", field("op").unwrap_or("unknown")))
                    .or_default() += 1;
                state.logship.journal(&crate::logship::Action {
                    op: field("op").unwrap_or("unknown"),
                    client,
                    device: device.as_deref(),
                    // `space_close` alone names its space `id`.
                    space: field("space").or_else(|| {
                        (field("op") == Some("space_close"))
                            .then(|| field("id"))
                            .flatten()
                    }),
                    pane: field("pane"),
                    ok: outcome.as_ref().map(|o| o.is_ok()),
                    ms: started.elapsed().as_millis() as u64,
                    err: outcome
                        .as_ref()
                        .and_then(|o| o.as_ref().err())
                        .map(String::as_str),
                });
            }
            Err(e) => {
                tracing::warn!(%peer, error = %e, "bad client message");
                let _ = out.send(unparsed_reply(&text, &e)).await;
            }
        }
    }

    let mut ended = Vec::new();
    for (_, task) in forwards {
        task.abort();
        ended.push(task);
    }
    state.renderers.gone(peer);
    state.mac_apps.gone(peer);
    let left = state.views.gone(peer);
    state.panes_shown(&left);
    state.release_sizes(peer, None);
    state.orchestrator_gone(peer);
    global_task.abort();
    if stopped {
        // Every sender of `out` must be gone for the writer to see the channel close, so that it
        // writes what is queued (`stopping` first among it) and closes the socket.
        ended.push(global_task);
        for task in ended {
            let _ = task.await;
        }
        drop(out);
        let _ = writer.await;
    } else {
        writer.abort();
    }
    if let Some(id) = &device {
        state.devices.closed(id);
        state.devices_changed();
    }
    match &device {
        Some(_) => tracing::info!(%peer, "ws close"),
        None => tracing::debug!(%peer, "ws close"),
    }
}

async fn handle(
    state: &Arc<AppState>,
    msg: ClientMsg,
    out: &mpsc::Sender<ServerMsg>,
    forwards: &mut HashMap<String, JoinHandle<()>>,
    peer: SocketAddr,
    caller: &Caller,
) -> Option<Result<(), String>> {
    let reply = match msg {
        ClientMsg::PairingIssue { .. }
        | ClientMsg::PairingCancel { .. }
        | ClientMsg::DeviceList { .. }
        | ClientMsg::DeviceRevoke { .. }
        | ClientMsg::WindowOffer {}
        | ClientMsg::WindowAnswer { .. }
        | ClientMsg::Window { .. }
            if *caller != Caller::Local =>
        {
            let req = match &msg {
                ClientMsg::PairingIssue { req }
                | ClientMsg::PairingCancel { req, .. }
                | ClientMsg::DeviceList { req }
                | ClientMsg::DeviceRevoke { req, .. }
                | ClientMsg::Window { req, .. } => *req,
                _ => None,
            };
            tracing::warn!(%peer, ?caller, "refused: pairing and windows are managed from this machine");
            Some(failed(
                req,
                "pairing, devices and windows are managed from the machine wheeld runs on"
                    .to_string(),
            ))
        }
        ClientMsg::PairingIssue { req } => req.map(|req| {
            let (token, expires_ms) = state.devices.issue();
            tracing::info!(%peer, expires_ms, "pairing token issued");
            ServerMsg::Pairing {
                req,
                token,
                expires_ms,
            }
        }),
        ClientMsg::PairingCancel { req, token } => {
            let dropped = state.devices.cancel(&token);
            tracing::info!(%peer, dropped, "pairing token cancelled");
            req.map(|req| ServerMsg::Ok { req })
        }
        ClientMsg::DeviceList { req } => req.map(|req| ServerMsg::Devices {
            req,
            devices: state.devices.list(),
        }),
        ClientMsg::DeviceRevoke { req, id } => {
            if state.devices.revoke(&id) {
                tracing::info!(%peer, device = %id, "device revoked");
                let _ = state.revoked.send(id);
                state.devices_changed();
                req.map(|req| ServerMsg::Ok { req })
            } else {
                Some(failed(req, format!("no paired device {id}")))
            }
        }
        ClientMsg::PushRegister { req, token } => {
            let token = token.trim().to_ascii_lowercase();
            let valid = !token.is_empty()
                && token.len() <= 200
                && token.chars().all(|c| c.is_ascii_hexdigit());
            match (caller, valid) {
                (Caller::Local, _) => Some(failed(
                    req,
                    "a push token belongs to a paired phone; register it from the phone".into(),
                )),
                (_, false) => Some(failed(req, "a push token is hexadecimal".into())),
                (Caller::Device(id), true) => {
                    if state.devices.set_push_token(id, Some(token)) {
                        tracing::info!(%peer, device = %id, "push token registered");
                        req.map(|req| ServerMsg::Ok { req })
                    } else {
                        Some(failed(req, "the daemon could not keep the push token".into()))
                    }
                }
            }
        }
        ClientMsg::PushTest { req, event } => match crate::push::Event::parse(&event) {
            None => Some(failed(
                req,
                format!("{event:?} is not a push event: needs_you, failed or turn_done"),
            )),
            Some(event) => {
                let notice = state.push_test_notice(event);
                let state = state.clone();
                let out = out.clone();
                tokio::spawn(async move {
                    let reply = match state.push_send(&notice).await {
                        Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                        Err(why) => Some(failed(req, why)),
                    };
                    if let Some(reply) = reply {
                        let _ = out.send(reply).await;
                    }
                });
                None
            }
        },
        ClientMsg::Status { req } => Some(state.status(req).await),
        ClientMsg::Log {
            req,
            client,
            inode,
            lines,
        } => {
            let source = match caller {
                Caller::Device(id) => id.as_str(),
                Caller::Local => "local",
            };
            state.logship.app_lines(&client, source, inode, &lines);
            req.map(|req| ServerMsg::Ok { req })
        }
        ClientMsg::RemoteInfo { req } => req.map(|req| crate::remote::info(state, req)),
        ClientMsg::AgentAttach { req, agent } => match state.resolve_agent(&agent) {
            Ok(found) => {
                let found = found.agent;
                // Keyed apart from terminal ids, which share the map, and by the agent's name,
                // so attaching by pane id and by name is one stream.
                let key = format!("agent:{}", found.name);
                if let Some(old) =
                    forwards.insert(key, forward_agent(found, req, out.clone(), peer))
                {
                    old.abort();
                }
                None
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::Usage { req, due_only } => {
            // On a task of its own: a read calls the network.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let Some(req) = req else { return };
                state.usage_read(!due_only).await;
                let reply = ServerMsg::Usage {
                    req,
                    board: state.usage_board(),
                };
                let _ = out.send(reply).await;
            });
            None
        }
        ClientMsg::TaskOutput { req, agent, task } => {
            // A file read of up to 256 KB: on a blocking thread, never in the connection's loop.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let read = tokio::task::spawn_blocking(move || state.agent(&agent)?.task_output(&task)).await;
                let reply = match read {
                    Ok(Ok((text, truncated))) => match req {
                        Some(req) => ServerMsg::TaskOutput { req, text, truncated },
                        None => return,
                    },
                    Ok(Err(message)) => failed(req, message),
                    Err(e) => failed(req, format!("task output read stopped: {e}")),
                };
                let _ = out.send(reply).await;
            });
            None
        }
        ClientMsg::AgentDetach { agent } => {
            let agent = state
                .resolve_agent(&agent)
                .map_or(agent, |a| a.agent.name.clone());
            if let Some(task) = forwards.remove(&format!("agent:{agent}")) {
                task.abort();
                tracing::info!(%peer, %agent, "agent detach");
            }
            None
        }
        ClientMsg::Notify {
            req,
            id,
            level,
            text,
            pane,
        } => {
            if text.trim().is_empty() || text.len() > 4000 || id.is_empty() || id.len() > 200 {
                Some(failed(
                    req,
                    "notify needs a text of 1 to 4000 bytes and an id of 1 to 200 bytes".into(),
                ))
            } else {
                // Sized, never the text: a notice can quote anything.
                tracing::info!(%peer, %id, ?level, bytes = text.len(), ?pane, "toast sent");
                let _ = state.global.send(ServerMsg::Toast {
                    id,
                    level,
                    text,
                    pane,
                });
                req.map(|req| ServerMsg::Ok { req })
            }
        }
        ClientMsg::NotifyClear { req, id } => {
            tracing::info!(%peer, %id, "toast cleared");
            let _ = state.global.send(ServerMsg::ToastClear { id });
            req.map(|req| ServerMsg::Ok { req })
        }
        ClientMsg::AgentNoticeDismiss { req, agent } => {
            match state.resolve_agent(&agent).map(|a| a.agent) {
                Ok(found) => {
                    tracing::info!(%peer, agent = %found.name, "agent notice dismissed");
                    found.set_notice(None);
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::AgentSend { req, agent, text } => {
            // Sized, never the text: a prompt can quote anything.
            tracing::info!(%peer, %agent, chars = text.len(), "agent send");
            match state.resolve_agent(&agent).and_then(|a| {
                let turn = a.agent.send(text)?;
                Ok((a.agent.name.clone(), turn))
            }) {
                Ok((agent, turn)) => {
                    req.map(|req| ServerMsg::AgentQueued { req, agent, turn })
                }
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::AgentWait {
            req,
            agent,
            timeout_secs,
        } => {
            // On a task of its own, so this connection keeps reading while it waits.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let result = if (0.0..=3600.0).contains(&timeout_secs) {
                    state
                        .agent_wait(&agent, std::time::Duration::from_secs_f64(timeout_secs))
                        .await
                } else {
                    Err(format!("timeout_secs is {timeout_secs}; use 0 to 3600"))
                };
                let reply = match result {
                    Ok(w) => req.map(|req| ServerMsg::AgentWaited {
                        req,
                        agent: w.agent,
                        pane: (!w.pane.is_empty()).then_some(w.pane),
                        state: w.state,
                        ready: w.ready,
                    }),
                    Err(message) => Some(failed(req, message)),
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::OrchestratorStart { req, scope } => match state.orchestrator_start(&scope) {
            Ok(agent) => {
                tracing::info!(%peer, %scope, %agent, "orchestrator start");
                req.map(|req| ServerMsg::OrchestratorStarted { req, scope, agent })
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::OrchestratorClaim { req } => {
            let owner = state.orchestrator_claim(peer);
            tracing::info!(%peer, "orchestrator claim");
            req.map(|req| ServerMsg::OrchestratorClaimed { req, owner })
        }
        ClientMsg::OrchestratorRelease { req } => match state.orchestrator_release(peer) {
            Ok(()) => {
                tracing::info!(%peer, "orchestrator release");
                req.map(|req| ServerMsg::Ok { req })
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::OrchestratorEvents { req, scope } => match state.orchestrator_take_events(&scope) {
            Ok(lines) => req.map(|req| ServerMsg::OrchestratorEventLines { req, lines }),
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::OrchestratorStop { req, scope } => match state.orchestrator_stop(&scope) {
            Ok(()) => {
                tracing::info!(%peer, %scope, "orchestrator stop");
                req.map(|req| ServerMsg::Ok { req })
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::AgentCancel { req, agent } => {
            match state
                .resolve_agent(&agent)
                .and_then(|a| a.agent.cancel())
            {
                Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::AgentUnsend { req, agent, turn } => {
            match state
                .resolve_agent(&agent)
                .and_then(|a| a.agent.unsend(turn))
            {
                Ok(text) => {
                    tracing::info!(%peer, %agent, turn, chars = text.len(), "agent unsend");
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => {
                    tracing::warn!(%peer, %agent, turn, %message, "agent unsend failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::AgentSendNow { req, agent, turn } => {
            match state
                .resolve_agent(&agent)
                .and_then(|a| a.agent.send_now(turn))
            {
                Ok(()) => {
                    tracing::info!(%peer, %agent, turn, "agent send now");
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => {
                    tracing::warn!(%peer, %agent, turn, %message, "agent send now failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::AgentClearQueue { req, agent } => {
            match state
                .resolve_agent(&agent)
                .and_then(|a| a.agent.clear_queue())
            {
                Ok(cleared) => {
                    tracing::info!(%peer, %agent, cleared, "agent clear queue");
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => {
                    tracing::warn!(%peer, %agent, %message, "agent clear queue failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::AgentConfigSet {
            req,
            agent,
            config,
            value,
        } => {
            let result = match state.resolve_agent(&agent).map(|a| a.agent) {
                Ok(found) => found.set_config(config.clone(), value.clone()).await,
                Err(message) => Err(message),
            };
            match result {
                Ok(()) => {
                    tracing::info!(%peer, %agent, %config, %value, "agent config set");
                    // The mode is saved with the pane, so a restart keeps it.
                    state.save_layout_soon();
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => {
                    tracing::warn!(%peer, %agent, %config, %value, %message, "agent config set failed");
                    Some(failed(req, message))
                }
            }
        }
        // On tasks of their own, never inline: each waits on the agent, which takes up to a minute
        // to load a conversation, and this connection has to keep reading frames meanwhile.
        ClientMsg::AgentConversationLs { req, agent, limit, before, before_id } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let listed = match state.resolve_agent(&agent) {
                    Ok(found) => found.agent.conversations(limit.map(|n| n as usize), before.map(|at| (at, before_id))).await,
                    Err(message) => Err(message),
                };
                let reply = match listed {
                    Ok((conversations, more)) => req.map(|req| ServerMsg::Conversations {
                        req,
                        agent,
                        conversations,
                        more,
                    }),
                    Err(message) => {
                        tracing::warn!(%peer, %message, "agent conversation ls failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::AgentConversationNew { req, agent } => {
            spawn_switch(state, out, req, agent, None, peer);
            None
        }
        // On a task of its own, like `pane_start`: the fork's agent takes a while to come up.
        ClientMsg::AgentFork { req, agent, turns } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.pane_fork(&agent, turns as usize).await) {
                    (Some(req), Ok(s)) => Some(ServerMsg::Started {
                        req,
                        pane: s.pane,
                        agent: s.agent,
                        state: s.state,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %agent, turns, %message, "agent fork failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::AgentConversationResume { req, agent, id } => {
            spawn_switch(state, out, req, agent, Some(id), peer);
            None
        }
        ClientMsg::PermAnswer { req, id, choice } => {
            match state.perms.answer(&id, &choice, &peer.to_string()) {
                Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::TermOpen {
            req,
            cwd,
            cols,
            rows,
        } => {
            let cwd = cwd.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            match state
                .open_term(cwd, None, cols, rows, Owner::Client, None)
                .await
            {
                Ok(term) => Some(ServerMsg::TermOpened {
                    req,
                    id: term.id.clone(),
                    pid: term.pid,
                }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::SpaceCreate {
            req,
            cwd,
            name,
            focus,
            background,
            pane,
            session,
        } => {
            let made = if focus && background {
                Err("a space with no window cannot be shown; give focus or background, not both"
                    .to_string())
            } else {
                state
                    .space_create(cwd, name, background, pane.as_deref(), session.as_deref())
                    .map(|s| s.id)
            };
            if let (true, Ok(id)) = (focus, &made) {
                let _ = state.focus_requested(Some(id), None);
            }
            created(req, made)
        }
        ClientMsg::SessionNew { req, name } => created(req, state.session_new(&name)),
        ClientMsg::SessionRename { req, session, name } => {
            done(req, state.session_rename(&session, &name))
        }
        ClientMsg::SessionClose { req, session } => done(req, state.session_close(&session)),
        ClientMsg::SessionReorder {
            req,
            session,
            index,
        } => done(req, state.session_reorder(&session, index)),
        ClientMsg::SessionMove {
            req,
            space,
            session,
            index,
        } => done(req, state.session_move(&space, &session, index)),
        ClientMsg::SpaceRecent { req } => req.map(|req| ServerMsg::SpaceRecent {
            req,
            dirs: state.recent.lock().unwrap().clone(),
        }),
        ClientMsg::DirList { req, path } => req.map(|req| {
            let (path, dirs, error) = state.dir_list(&path);
            ServerMsg::DirList {
                req,
                path,
                dirs,
                error,
            }
        }),
        ClientMsg::FilePeek {
            req,
            path,
            cwd,
            line,
            context,
        } => req.map(|req| crate::files::file_peek(req, &path, cwd.as_deref(), line, context)),
        ClientMsg::FileImage { req, path, cwd } => match req {
            // Up to 20 MB is read and encoded: off the async threads.
            Some(req) => tokio::task::spawn_blocking(move || {
                crate::files::file_image(req, &path, cwd.as_deref())
            })
            .await
            .ok(),
            None => None,
        },
        ClientMsg::PaneOpen {
            req,
            pane,
            dir,
            url,
            artifact,
            scope,
            focus,
            site,
        } => {
            let opened = state
                .pane_open(&pane, dir, site, url, artifact, scope)
                .await;
            if let (true, Ok(new)) = (focus, &opened) {
                state.show_pane(new);
            }
            created(req, opened)
        }
        ClientMsg::PaneRun {
            req,
            pane,
            command,
            focus,
            site,
        } => {
            let ran = state.pane_run(&pane, &command, site).await;
            if let (true, Ok(new)) = (focus, &ran) {
                state.show_pane(new);
            }
            created(req, ran)
        }
        ClientMsg::AgentHomes { req, agent, path } => req.map(|req| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            let found =
                crate::homes::agent_homes(&agent, path.as_deref(), std::path::Path::new(&home));
            tracing::info!(%agent, ?path, found = found.as_ref().map(|f| f.0.len()), "agent_homes");
            match found {
                Some((homes, checked)) => ServerMsg::AgentHomes {
                    req,
                    homes,
                    checked,
                    error: None,
                },
                None => ServerMsg::AgentHomes {
                    req,
                    homes: Vec::new(),
                    checked: None,
                    error: Some(format!("{agent:?} is not an agent wheel runs")),
                },
            }
        }),
        ClientMsg::DirRoots { req } => req.map(|req| ServerMsg::DirRoots {
            req,
            roots: state.dir_roots(),
        }),
        ClientMsg::SpaceRename { req, space, name } => done(req, state.space_rename(&space, &name)),
        ClientMsg::SpaceClose { req, id, if_empty } => done(req, state.space_close(&id, if_empty)),
        ClientMsg::Focus { req, space, pane } => done(
            req,
            state.focus_requested(space.as_deref(), pane.as_deref()),
        ),
        ClientMsg::PaneSplit {
            req,
            pane,
            dir,
            focus,
            site,
        } => match state.pane_split(&pane, dir, site) {
            Ok((id, strategy)) => {
                if focus {
                    state.show_pane(&id);
                }
                req.map(|req| ServerMsg::Created {
                req,
                id,
                placed_by: strategy.map(|s| s.name().to_string()),
                })
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::PaneSetSite { req, pane, site } => done(req, state.pane_set_site(&pane, site)),
        ClientMsg::PaneSwap { req, pane, with } => done(req, state.pane_swap(&pane, &with)),
        ClientMsg::PaneMove { req, pane, to } => {
            let focus = matches!(&to, wheel_proto::MoveTarget::NewSpace { focus: true, .. });
            let moved = state.pane_move(&pane, to);
            if focus && moved.is_ok() {
                state.show_pane(&pane);
            }
            done(req, moved)
        }
        ClientMsg::PaneClose { req, pane } => done(req, state.pane_close(&pane)),
        ClientMsg::PaneSetKind {
            req,
            pane,
            kind,
            artifact,
            scope,
            url,
            plugin,
        } => {
            let set = state
                .pane_set_kind(&pane, kind, artifact, scope, url, plugin)
                .await
                .map(|_| ());
            done(req, set)
        }
        // On a task of its own, never inline: an agent takes up to a minute to come up, and this
        // connection has to keep reading frames meanwhile — terminal input, agent attaches and
        // permission answers are on the same socket.
        ClientMsg::PaneStart {
            req,
            pane,
            kind,
            message,
            agent,
            model,
            permission_mode,
            sandbox,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let started = match state.space_holding(&pane) {
                    Ok(space) => {
                        state
                            .pane_start(
                                &space,
                                Some(pane),
                                kind,
                                StartOptions {
                                    message,
                                    agent,
                                    model,
                                    permissions: AgentPermissions {
                                        permission_mode,
                                        sandbox,
                                    },
                                },
                            )
                            .await
                    }
                    Err(e) => Err(e),
                };
                let reply = match (req, started) {
                    (Some(req), Ok(s)) => Some(ServerMsg::Started {
                        req,
                        pane: s.pane,
                        agent: s.agent,
                        state: s.state,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %message, "pane start failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        // On a task of its own: a switch waits for the other view's process to stop.
        ClientMsg::PaneView { req, pane, to } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.pane_view(&pane, to).await) {
                    (Some(req), Ok((view, conversation, switch_ms))) => Some(ServerMsg::PaneView {
                        req,
                        pane,
                        view,
                        conversation,
                        switch_ms,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %pane, ?to, %message, "pane view switch failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::PaneInfo { req, pane } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.pane_info(&pane).await) {
                    (Some(req), Ok((pane, kind, fields, login))) => Some(ServerMsg::PaneInfo {
                        req,
                        pane,
                        kind,
                        fields,
                        login,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %pane, %message, "pane info failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SpaceProcs { req, space } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.space_procs(&space).await) {
                    (Some(req), Ok(procs)) => Some(ServerMsg::Procs { req, space, procs }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %space, %message, "space procs failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ProcStop {
            req,
            space,
            pid,
            force,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.proc_stop(&space, pid, force).await) {
                    (Some(req), Ok(())) => Some(ServerMsg::Ok { req }),
                    (_, Ok(())) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %space, pid, %message, "process stop failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::PaneBrowserUrl { req, pane, url } => match state.pane_browser_url(&pane, &url) {
            Ok(_) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "pane browser url failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::PaneReport {
            req,
            pane,
            status,
            reason,
        } => match state.pane_report(&pane, status, reason) {
            Ok(()) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "pane report failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::PaneReportConversation {
            req,
            pane,
            conversation,
            pid,
        } => match state.pane_report_conversation(&pane, &conversation, pid) {
            Ok(()) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "pane conversation report failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::WorkSet {
            req,
            pane,
            items,
            stage,
            note,
        } => match state.work_set(&pane, items, stage, note) {
            Ok(()) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "work set failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::PaneReportContext { req, pane, context } => {
            match state.pane_report_context(&pane, context) {
                Ok(()) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "pane context report failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::PaneConsole {
            req,
            pane,
            reset,
            entries,
        } => match state.pane_console(&pane, reset, entries) {
            Ok(()) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "pane console report failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::PaneConsoleRead { req, pane, since } => {
            match state.pane_console_read(&pane, since) {
                Ok(entries) => req.map(|req| ServerMsg::Console { req, pane, entries }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "pane console read failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::PaneBrowserOpen { req, pane, text } => {
            match state.pane_browser_open(&pane, &text) {
                Ok(url) => req.map(|req| ServerMsg::Opened { req, pane, url }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "pane browser open failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::View {
            req,
            window,
            session,
            space,
            pane,
        } => {
            // A client that does not name the session gets the one holding its space.
            let session = session.or_else(|| space.as_deref().and_then(|s| state.session_id_holding(s)));
            let view = wheel_proto::WindowView {
                window,
                session,
                space,
                pane,
            };
            // Stored as reported: a view can name a space or pane that closed a moment ago, and
            // `client_ls` leaves out what no longer exists.
            tracing::debug!(%peer, window = %view.window, space = ?view.space, pane = ?view.pane, "view");
            let changed = state
                .views
                .report(peer, state.client_name(peer, caller), view);
            state.panes_shown(&changed);
            done(req, Ok(()))
        }
        ClientMsg::ClientLs { req } => req.map(|req| ServerMsg::Clients {
            req,
            clients: state.client_list(),
        }),
        ClientMsg::WindowOffer {} => {
            state.mac_apps.offer(peer, out.clone());
            None
        }
        ClientMsg::WindowAnswer { id, data, error } => {
            let answer = match error {
                Some(message) => Err(message),
                None => Ok(data.unwrap_or(serde_json::Value::Null)),
            };
            state.mac_apps.answer(peer, id, answer);
            None
        }
        ClientMsg::Window { req, command } => {
            // On a task of its own: the Mac app answers when it has done it.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let reply = match state.window_command(command).await {
                    Ok(data) => req.map(|req| ServerMsg::WindowReply { req, data }),
                    Err(message) => Some(failed(req, message)),
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::RenderOffer {} => {
            let phone = match caller {
                Caller::Device(id) => Some(state.devices.name(id).unwrap_or_else(|| id.clone())),
                Caller::Local => None,
            };
            state.renderers.offer(peer, out.clone(), phone);
            None
        }
        ClientMsg::PaneBrowserMode { req, pane, mode } => {
            let set = state
                .browser_pane(&pane)
                .and_then(|()| state.renderers.set_mode(peer, &pane, &mode));
            match set {
                Ok(()) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "pane browser mode failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::Rendered { id, png } => {
            tracing::info!(%peer, id, base64_chars = png.len(), "rendered");
            state
                .renderers
                .answer(peer, id, crate::render::decode_base64(&png));
            None
        }
        ClientMsg::Recorded {
            id,
            mov,
            frames,
            millis,
        } => {
            tracing::info!(%peer, id, base64_chars = mov.len(), frames, millis, "recorded");
            let answer = crate::render::decode_base64(&mov);
            if answer.is_ok() {
                state
                    .renderers
                    .recorded
                    .lock()
                    .unwrap()
                    .insert(id, (frames, millis));
            }
            if !state.renderers.answer(peer, id, answer) {
                state.renderers.recorded.lock().unwrap().remove(&id);
            }
            None
        }
        ClientMsg::Inspected { id, text } => {
            tracing::info!(%peer, id, chars = text.len(), "inspected");
            state.renderers.answer(peer, id, Ok(text.into_bytes()));
            None
        }
        ClientMsg::PaneInspect {
            req,
            pane,
            action,
            arg,
            path,
            on,
        } => {
            // On a task of its own: a client takes as long as it takes to answer.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let side = match crate::render::Side::parse(on.as_deref()) {
                    Ok(side) => side,
                    Err(message) => {
                        let _ = out.send(failed(req, message)).await;
                        return;
                    }
                };
                if action == "record" {
                    let reply = match record_request(&state, &pane, arg, path.clone(), side).await {
                        Ok((recorded_path, rec)) => req.map(|req| ServerMsg::PaneRecorded {
                            req,
                            pane,
                            path: recorded_path,
                            bytes: rec.bytes,
                            frames: rec.frames,
                            millis: rec.millis,
                        }),
                        Err(message) => {
                            tracing::warn!(%peer, %pane, %message, "pane record failed");
                            Some(failed(req, message))
                        }
                    };
                    if let Some(reply) = reply {
                        let _ = out.send(reply).await;
                    }
                    return;
                }
                let reply = match state.pane_inspect(&pane, &action, arg, path.clone(), side).await {
                    Ok(crate::render::InspectOutcome::Text(text)) => {
                        req.map(|req| ServerMsg::Inspected { req, pane, text })
                    }
                    Ok(crate::render::InspectOutcome::Shot(bytes)) => {
                        req.map(|req| ServerMsg::PaneShot {
                            req,
                            pane,
                            path: path.unwrap_or_default(),
                            bytes,
                        })
                    }
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %action, %message, "pane inspect failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::RenderRefused { id, message } => {
            tracing::info!(%peer, id, %message, "render refused by client");
            state.renderers.answer(peer, id, Err(message));
            None
        }
        ClientMsg::PaneShot {
            req,
            pane,
            path,
            on,
            full,
        } => {
            // On a task of its own: a client takes as long as it takes to draw.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let side = match crate::render::Side::parse(on.as_deref()) {
                    Ok(side) => side,
                    Err(message) => {
                        let _ = out.send(failed(req, message)).await;
                        return;
                    }
                };
                let reply = match state.pane_shot(&pane, &path, side, full).await {
                    Ok(bytes) => req.map(|req| ServerMsg::PaneShot {
                        req,
                        pane,
                        path,
                        bytes,
                    }),
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %message, "pane shot failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SplitRatio { req, split, ratio } => done(req, state.split_ratio(&split, ratio)),
        ClientMsg::ArtifactNew {
            req,
            pane,
            name,
            scope,
            space,
            path,
            ends_with_pane,
            expires_in,
            renderer,
            files,
            reply_to,
            webhook,
            description,
            url,
        } => one_artifact(
            req,
            state.artifact_new(
                &name,
                scope,
                space.as_deref(),
                pane,
                path.as_deref(),
                (ends_with_pane.as_deref(), expires_in),
                crate::artifacts::Post {
                    renderer: renderer.as_deref(),
                    files: &files,
                    reply_to: reply_to.as_deref(),
                    webhook: webhook.as_deref(),
                    description: description.as_deref(),
                    url: url.as_deref(),
                },
            ),
            "artifact new",
        ),
        ClientMsg::ArtifactAdd {
            req,
            path,
            name,
            scope,
            space,
            title,
            description,
            command,
            every,
            pane,
        } => one_artifact(
            req,
            state.artifact_add(
                &path,
                name.as_deref(),
                scope,
                space.as_deref(),
                crate::artifacts::Declared {
                    title,
                    description,
                    command,
                    every,
                    pane,
                    ..Default::default()
                },
            ),
            "artifact add",
        ),
        ClientMsg::ArtifactRemove {
            req,
            name,
            scope,
            space,
        } => one_artifact(
            req,
            state.artifact_remove(&name, scope, space.as_deref()),
            "artifact remove",
        ),
        ClientMsg::ArtifactPromote {
            req,
            name,
            to,
            scope,
            space,
            from,
        } => {
            // Waits on a person, so the connection keeps reading while the request is open. When
            // the connection goes first, dropping the call withdraws the request.
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let result = tokio::select! {
                    result = state.artifact_promote(&name, to, scope, space.as_deref(), from.as_deref()) => result,
                    () = out.closed() => return,
                };
                if let Some(reply) = one_artifact(req, result, "artifact promote") {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::BuiltinLs { req } => builtins_reply(req, state.builtin_ls(), "builtin ls"),
        ClientMsg::BuiltinInstall { req, name } => {
            builtins_reply(req, state.builtin_install(&name), "builtin install")
        }
        ClientMsg::BuiltinUpdate {
            req,
            name,
            on_edited,
        } => match state.builtin_update(&name, on_edited) {
            Ok(crate::builtins::Updated::Edited(edited)) => {
                req.map(|req| ServerMsg::BuiltinEdited { req, name, edited })
            }
            Ok(crate::builtins::Updated::Done(builtins)) => {
                builtins_reply(req, Ok(builtins), "builtin update")
            }
            Err(message) => builtins_reply(req, Err(message), "builtin update"),
        },
        ClientMsg::BuiltinRemove { req, name } => {
            builtins_reply(req, state.builtin_remove(&name), "builtin remove")
        }
        ClientMsg::ArtifactShow {
            req,
            name,
            scope,
            space,
        } => one_artifact(
            req,
            state.artifact_show(&name, scope, space.as_deref()),
            "artifact show",
        ),
        // On a task of its own: connecting starts a process, and a tool call can run long, while
        // this connection has to keep reading frames.
        ClientMsg::ArtifactTools {
            req,
            name,
            scope,
            space,
            pane,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match state.mcp_tools(&name, scope, space.as_deref(), pane.as_deref()).await {
                    Ok(tools) => req.map(|req| ServerMsg::Tools { req, tools }),
                    Err(message) => {
                        tracing::warn!(%name, %message, "artifact tools refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ArtifactRead {
            req,
            name,
            scope,
            space,
            pane,
            uri,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match state.mcp_resource(&name, scope, space.as_deref(), pane.as_deref(), &uri).await {
                    Ok(contents) => req.map(|req| ServerMsg::Resource { req, contents }),
                    Err(message) => {
                        tracing::warn!(%name, %uri, %message, "artifact read refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ArtifactCall {
            req,
            name,
            scope,
            space,
            pane,
            tool,
            arguments,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let called = state
                    .mcp_call(&name, scope, space.as_deref(), pane.as_deref(), &tool, arguments)
                    .await;
                let reply = match called {
                    Ok(result) => req.map(|req| ServerMsg::ToolResult { req, result }),
                    Err(message) => {
                        tracing::warn!(%name, %tool, %message, "artifact call refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::RunnerLs { req, space } => match state.runner_ls(space.as_deref()) {
            Ok(runners) => req.map(|req| ServerMsg::Runners { req, runners }),
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::TitleButtonLs { req, space } => match state.title_buttons(&space) {
            Ok(buttons) => req.map(|req| ServerMsg::TitleButtons {
                req,
                space,
                buttons,
            }),
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::TitleButtonPress { req, space, id } => {
            done(req, state.title_button_press(&space, &id))
        }
        ClientMsg::StreamLs { req, space } => match state.stream_ls(space.as_deref()) {
            Ok(groups) => req.map(|req| ServerMsg::Streams { req, groups }),
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::StreamStart { req, group, space } => {
            match state.stream_start(&group, space.as_deref()).await {
                Ok(stream) => req.map(|req| ServerMsg::StreamStarted { req, stream }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::StreamStop { req, group, space } => {
            done(req, state.stream_stop(&group, space.as_deref()))
        }
        ClientMsg::StreamTail {
            req,
            group,
            space,
            since_ms,
            follow,
        } => match state.stream_find(&group, space.as_deref()) {
            Ok(run) => {
                let since = since_ms.map(|ms| wheel_proto::unix_ms_now().saturating_sub(ms));
                let (lines, rx) = run.tail(since, follow);
                let ended = !follow || rx.is_none();
                let Some(req) = req else { return None };
                let key = format!("stream:{req}");
                let first = ServerMsg::StreamLines { req, lines, ended };
                if let Some(mut rx) = rx {
                    let out = out.clone();
                    let group = group.clone();
                    let task = tokio::spawn(async move {
                        if out.send(first).await.is_err() {
                            return;
                        }
                        loop {
                            let (lines, ended) = match rx.recv().await {
                                Ok(crate::streams::Event::Line(line)) => (vec![line], false),
                                Ok(crate::streams::Event::Ended) | Err(broadcast::error::RecvError::Closed) => {
                                    (Vec::new(), true)
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(%group, skipped = n, "a following tail fell behind and skipped lines");
                                    let note = wheel_proto::StreamLine {
                                        arrived: wheel_proto::unix_ms_now(),
                                        occurred: None,
                                        source: "wheeld".into(),
                                        text: format!("skipped {n} lines: this tail fell behind"),
                                    };
                                    (vec![note], false)
                                }
                            };
                            if out.send(ServerMsg::StreamLines { req, lines, ended }).await.is_err() || ended {
                                return;
                            }
                        }
                    });
                    if let Some(old) = forwards.insert(key, task) {
                        old.abort();
                    }
                    None
                } else {
                    Some(first)
                }
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::Run {
            req,
            name,
            space,
            focus,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let started = match state.settings_space_id(space.as_deref()) {
                    Ok(space) => state.run_command(&space, &name).await,
                    Err(e) => Err(e),
                };
                if let (true, Ok(pane)) = (focus, &started) {
                    state.show_pane(pane);
                }
                let reply = match (req, started) {
                    (Some(req), Ok(pane)) => Some(ServerMsg::Started {
                        req,
                        pane,
                        agent: None,
                        state: None,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%name, %message, "run failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SettingsLs { req, space } => {
            settings_reply(req, state.settings_ls(space.as_deref()), "settings ls")
        }
        ClientMsg::SettingSet {
            req,
            key,
            value,
            space,
            machine,
        } => {
            let written = state.setting_set(&key, &value, space.as_deref(), machine);
            settings_changed(state, req, written, &key, &format!("setting set {key}"))
        }
        ClientMsg::SettingsFileEnsure {
            req,
            space,
            machine,
        } => match state.settings_file_ensure(space.as_deref(), machine) {
            Ok(()) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%message, "settings file ensure failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::GuidanceLs { req, agent } => match state.guidance_ls(agent.as_deref()) {
            Ok((brief, given, files)) => req.map(|req| ServerMsg::Guidance {
                req,
                brief,
                given,
                files,
            }),
            Err(message) => {
                tracing::warn!(%message, "guidance ls failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::GuidancePath { req, agent } => match state.guidance_path(agent.as_deref()) {
            Ok(path) => req.map(|req| ServerMsg::GuidancePath { req, path }),
            Err(message) => {
                tracing::warn!(%message, "guidance path failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::SettingUnset {
            req,
            key,
            space,
            machine,
        } => {
            let removed = state.setting_unset(&key, space.as_deref(), machine);
            settings_changed(state, req, removed, &key, &format!("setting unset {key}"))
        }
        ClientMsg::PluginLs { req, space } => {
            let state = state.clone();
            let listed = tokio::task::spawn_blocking(move || state.plugin_ls(space.as_deref()))
                .await
                .unwrap_or_else(|e| Err(format!("plugin ls failed: {e}")));
            match listed {
                Ok(plugins) => req.map(|req| ServerMsg::Plugins { req, plugins }),
                Err(message) => {
                    tracing::warn!(%peer, %message, "plugin ls failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::PluginSource { req, pane } => {
            let state = state.clone();
            let read = {
                let pane = pane.clone();
                tokio::task::spawn_blocking(move || state.plugin_source(&pane)).await
            };
            match read {
                Ok(Ok((plugin, plugin_pane, chunk, source))) => req.map(|_| ServerMsg::PluginSource {
                    req,
                    pane,
                    plugin,
                    plugin_pane,
                    chunk,
                    source,
                }),
                Ok(Err(message)) => {
                    tracing::warn!(%peer, %pane, %message, "plugin source refused");
                    Some(failed(req, message))
                }
                Err(e) => {
                    tracing::error!(%peer, %pane, error = %e, "plugin source panicked");
                    Some(failed(req, "plugin source failed".into()))
                }
            }
        }
        ClientMsg::ArtifactLs { req, space, pane } => {
            match state.artifact_ls(space.as_deref(), pane) {
                Ok((artifacts, resolves)) => req.map(|req| ServerMsg::Artifacts {
                    req,
                    artifacts,
                    resolves,
                }),
                Err(message) => {
                    tracing::warn!(%peer, %message, "artifact ls failed");
                    Some(failed(req, message))
                }
            }
        }
        // On a task of its own, never inline: a run waits for a person to answer its permission
        // request, and this connection has to keep reading frames meanwhile — the answer may be
        // coming from this very client, and its terminals' input is on the same socket.
        ClientMsg::ArtifactRefresh { req, pane } => {
            let out = out.clone();
            // Started here, in frame order; only the wait for the run is on the task.
            let run = state.refresh_pane(&pane);
            tokio::spawn(async move {
                let result = match run {
                    Ok(wait) => wait.await,
                    Err(message) => Err(message),
                };
                let reply = match result {
                    Ok(refresh) => req.map(|req| ServerMsg::Refresh { req, refresh }),
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %message, "artifact refresh refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ArtifactAppStatus { req, pane } => match state.app_of(&pane) {
            Ok(app) => req.map(|req| ServerMsg::App { req, app: app.info() }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "artifact app status refused");
                Some(failed(req, message))
            }
        },
        ClientMsg::ArtifactAppLogs { req, pane, run, tail } => {
            match state.app_of(&pane).and_then(|app| app.logs(run, tail)) {
                Ok((run, text)) => req.map(|req| ServerMsg::AppLogs { req, run, text }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "artifact app logs refused");
                    Some(failed(req, message))
                }
            }
        }
        // On a task of its own: the request waits for the process to start or stop, and this
        // connection has to keep reading frames meanwhile.
        ClientMsg::ArtifactAppControl { req, pane, action } => {
            let out = out.clone();
            let app = state.app_of(&pane);
            tokio::spawn(async move {
                let result = match app {
                    Ok(app) => app.control(action).await,
                    Err(message) => Err(message),
                };
                let reply = match result {
                    Ok(app) => req.map(|req| ServerMsg::App { req, app }),
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %message, "artifact app control refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ArtifactState {
            req,
            pane,
            key,
            wait_ms,
        } => match state.page(&pane) {
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "artifact state refused");
                Some(failed(req, message))
            }
            Ok(page) => {
                let out = out.clone();
                // On a task of its own: a wait must not stop this connection reading frames.
                tokio::spawn(async move {
                    let (values, timed_out) = match (&key, wait_ms) {
                        (Some(key), Some(ms)) => {
                            page.wait_for(key, Duration::from_millis(ms)).await
                        }
                        _ => (page.values(key.as_deref()), false),
                    };
                    tracing::info!(%peer, %pane, ?key, ?wait_ms, keys = values.len(), timed_out, "artifact state read");
                    if let Some(req) = req {
                        let _ = out
                            .send(ServerMsg::PageState {
                                req,
                                values,
                                timed_out,
                            })
                            .await;
                    }
                });
                None
            }
        },
        ClientMsg::ArtifactData { req, pane, value } => match state.page(&pane) {
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "artifact data refused");
                Some(failed(req, message))
            }
            Ok(_) if value.to_string().len() > crate::page::MAX_VALUE => {
                Some(failed(req, "the value is over 1 MiB".into()))
            }
            Ok(page) => {
                page.push(value);
                req.map(|req| ServerMsg::Ok { req })
            }
        },
        ClientMsg::TermAttach { req, id } => match state.term(&id) {
            Some(term) => {
                if let Some(old) =
                    forwards.insert(id, forward(state.clone(), term, req, out.clone(), peer))
                {
                    old.abort();
                }
                None
            }
            None => term_done(req, &id, Err(format!("no terminal {id}"))),
        },
        ClientMsg::TermClose { req, id } => term_done(req, &id, state.term_close(&id)),
        ClientMsg::TermDetach { id } => {
            if let Some(task) = forwards.remove(&id) {
                task.abort();
                tracing::info!(%peer, term = %id, "detach");
            }
            state.release_sizes(peer, Some(&id));
            None
        }
        ClientMsg::TermInput { req, id, data } => {
            let result = state
                .term(&id)
                .ok_or_else(|| anyhow::anyhow!("no terminal {id}"))
                .and_then(|t| {
                    let bytes = decode_bytes(&data)?;
                    state.term_input_answers_prompt(&id, &bytes);
                    t.write(bytes)
                });
            // Input is logged by size only: it carries whatever the user types, passwords included.
            tracing::debug!(term = %id, bytes = data.len(), "input");
            term_done(req, &id, result.map_err(|e| e.to_string()))
        }
        ClientMsg::TermResize {
            req,
            id,
            cols,
            rows,
        } => {
            let result = state.request_size(peer, &id, cols.max(1), rows.max(1));
            if result.is_ok() {
                tracing::info!(term = %id, cols, rows, "resize");
            }
            term_done(req, &id, result.map_err(|e| e.to_string()))
        }
    };
    // What the journal records: an error reply is a failure, any other reply a success, and no
    // reply here (one sent later by a task of its own) is neither.
    let outcome = match &reply {
        Some(ServerMsg::Error { message, .. }) => Some(Err(message.clone())),
        Some(_) => Some(Ok(())),
        None => None,
    };
    if let Some(reply) = reply {
        let _ = out.send(reply).await;
    }
    outcome
}

/// Move an agent into another conversation — a fresh one, or the past one `load` names — off the
/// connection's own task, and reply with the conversation it ended up in.
fn spawn_switch(
    state: &Arc<AppState>,
    out: &mpsc::Sender<ServerMsg>,
    req: Option<u64>,
    agent: String,
    load: Option<String>,
    peer: SocketAddr,
) {
    let (state, out) = (state.clone(), out.clone());
    tokio::spawn(async move {
        let switched = match state.resolve_agent(&agent) {
            Ok(found) => {
                // Two agents writing one transcript would fork it.
                let claimed = match load.as_deref() {
                    Some(id) => state.claim_conversation(id, &found.agent).map(|()| id),
                    None => Ok(""),
                };
                match claimed {
                    Ok(id) => {
                        let result = found.agent.switch_conversation(load.clone()).await;
                        if !id.is_empty() {
                            state.switching_into.lock().unwrap().remove(id);
                        }
                        result
                    }
                    Err(holder) => Err(format!(
                        "agent {holder} is already in that conversation; resuming it here would fork it"
                    )),
                }
            }
            Err(message) => Err(message),
        };
        let reply = match switched {
            Ok(id) => {
                tracing::info!(%peer, %agent, conversation = %id, resumed = load.is_some(), "agent conversation changed");
                req.map(|req| ServerMsg::Conversation { req, agent, id })
            }
            Err(message) => {
                tracing::warn!(%peer, %agent, resumed = load.is_some(), %message, "agent conversation change failed");
                Some(failed(req, message))
            }
        };
        if let Some(reply) = reply {
            let _ = out.send(reply).await;
        }
    });
}

/// The reply to a request with no result of its own: `ok` when it carried a `req`.
fn done(req: Option<u64>, result: Result<(), String>) -> Option<ServerMsg> {
    match result {
        Ok(()) => req.map(|req| ServerMsg::Ok { req }),
        Err(message) => Some(failed(req, message)),
    }
}

/// The error for a frame that did not parse. A frame carrying a `req` gets the error under that
/// `req`, so the client waiting on it hears at once. An `op` this build has no variant for is
/// named as unsupported; a known op with bad fields keeps serde's message.
fn unparsed_reply(text: &str, error: &serde_json::Error) -> ServerMsg {
    let raw: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let req = raw
        .as_ref()
        .and_then(|v| v.get("req"))
        .and_then(|v| v.as_u64());
    let op = raw
        .as_ref()
        .and_then(|v| v.get("op"))
        .and_then(|v| v.as_str());
    // Parsing the op alone: only an op with no variant fails as "unknown variant"; a known op
    // fails on its missing fields instead.
    let op_unknown = op.is_some_and(|op| {
        serde_json::from_value::<ClientMsg>(serde_json::json!({ "op": op }))
            .is_err_and(|e| e.to_string().starts_with("unknown variant"))
    });
    let message = match op {
        Some(op) if op_unknown => {
            format!(
                "this wheeld doesn't support {op}; it may be older than the client that sent it"
            )
        }
        _ => format!("bad message: {error}"),
    };
    failed(req, message)
}

/// `pane_inspect` `record`: `arg` is the seconds, `path` the movie file. Returns the path written.
async fn record_request(
    state: &AppState,
    pane: &str,
    arg: Option<String>,
    path: Option<String>,
    side: crate::render::Side,
) -> Result<(String, crate::render::Recording), String> {
    let path = path.ok_or("record needs a path")?;
    let seconds = arg
        .as_deref()
        .and_then(|a| a.trim().parse::<u64>().ok())
        .ok_or("record needs a whole number of seconds")?;
    let recording = state.pane_record(pane, seconds, &path, side).await?;
    Ok((path, recording))
}

/// The error reply to a request that is not a terminal op.
fn failed(req: Option<u64>, message: String) -> ServerMsg {
    ServerMsg::Error {
        req,
        term: None,
        message,
    }
}

/// The reply to a terminal op: `ok` when it carried a `req`, or an error naming terminal `id`.
fn term_done(req: Option<u64>, id: &str, result: Result<(), String>) -> Option<ServerMsg> {
    match result {
        Ok(()) => req.map(|req| ServerMsg::Ok { req }),
        Err(message) => {
            // Debug, like input: a client typing into a terminal that is gone fails per keystroke.
            tracing::debug!(term = %id, error = %message, "terminal op failed");
            Some(ServerMsg::Error {
                req,
                term: Some(id.to_string()),
                message,
            })
        }
    }
}

/// The reply to a request about one artifact: the record it read off disk. A record with problems
/// is a success — the problems are what the caller asked for.
fn one_artifact(
    req: Option<u64>,
    result: Result<wheel_proto::ArtifactInfo, String>,
    via: &str,
) -> Option<ServerMsg> {
    match result {
        Ok(artifact) => req.map(|req| ServerMsg::Artifact { req, artifact }),
        Err(message) => {
            tracing::warn!(via, %message, "artifact request refused");
            Some(failed(req, message))
        }
    }
}

/// The reply to a built-in request: every built-in as it now stands.
fn builtins_reply(
    req: Option<u64>,
    result: Result<Vec<wheel_proto::BuiltinInfo>, String>,
    via: &str,
) -> Option<ServerMsg> {
    match result {
        Ok(builtins) => req.map(|req| ServerMsg::Builtins { req, builtins }),
        Err(message) => {
            tracing::warn!(via, %message, "built-in request refused");
            Some(failed(req, message))
        }
    }
}

/// The reply to a settings request: every setting as the daemon resolved it, labelled with the
/// space it resolved them for. That space is what the daemon resolved against, never the echo of
/// the request: a caller that named none is told which space it got.
fn settings_reply(
    req: Option<u64>,
    result: Result<(Option<String>, Vec<wheel_proto::SettingInfo>), String>,
    via: &str,
) -> Option<ServerMsg> {
    match result {
        Ok((space, settings)) => req.map(|req| ServerMsg::Settings {
            req: Some(req),
            space,
            settings,
        }),
        Err(message) => {
            tracing::warn!(via, %message, "settings request refused");
            Some(failed(req, message))
        }
    }
}

/// The reply to a write or a removal of a setting, after every connection hears it, labelled with
/// the space the list was resolved for — a machine-wide change alters what every space falls back
/// to, so a client holding another space's view asks again for that space.
fn settings_changed(
    state: &AppState,
    req: Option<u64>,
    result: Result<(Option<String>, Vec<wheel_proto::SettingInfo>), String>,
    key: &str,
    via: &str,
) -> Option<ServerMsg> {
    if let Ok((space, settings)) = &result {
        state.settings_told(space, settings);
        if key.starts_with("usage.") {
            let _ = state.global.send(ServerMsg::UsageChanged {
                board: state.usage_board(),
            });
        }
        let _ = state.global.send(ServerMsg::Settings {
            req: None,
            space: space.clone(),
            settings: settings.clone(),
        });
    }
    settings_reply(req, result, via)
}

/// The reply to a request that made something: `created` with its id.
fn created(req: Option<u64>, result: Result<String, String>) -> Option<ServerMsg> {
    match result {
        Ok(id) => req.map(|req| ServerMsg::Created {
            req,
            id,
            placed_by: None,
        }),
        Err(message) => Some(failed(req, message)),
    }
}

/// Stream one terminal to one client: replay, then live output. When the client falls far
/// enough behind that output was dropped, start over with a fresh replay.
fn forward(
    state: Arc<AppState>,
    term: Arc<Term>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut req = req;
        let mut resync = false;
        loop {
            let Some(Attachment {
                replay,
                exited,
                mut rx,
            }) = term.attach().await
            else {
                tracing::warn!(%peer, term = %term.id, "attach failed; the terminal's screen is gone");
                let message = format!("terminal {}'s screen is gone", term.id);
                if let Some(error) = term_done(req.take(), &term.id, Err(message)) {
                    let _ = out.send(error).await;
                }
                return;
            };
            tracing::info!(%peer, term = %term.id, replay_bytes = replay.len(), resync, "attach");
            let id = term.id.clone();
            if out
                .send(ServerMsg::TermAttached {
                    req: req.take(),
                    id: id.clone(),
                    replay_bytes: replay.len(),
                })
                .await
                .is_err()
            {
                return;
            }
            for chunk in replay.chunks(REPLAY_CHUNK) {
                if out
                    .send(ServerMsg::TermOutput {
                        id: id.clone(),
                        data: encode_bytes(chunk),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Some(code) = exited {
                exit(&state, &out, id, code).await;
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(TermEvent::Output(bytes)) => {
                        if out
                            .send(ServerMsg::TermOutput {
                                id: id.clone(),
                                data: encode_bytes(&bytes),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(TermEvent::Exited(code)) => {
                        exit(&state, &out, id, code).await;
                        return;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%peer, term = %id, skipped, "client fell behind; resyncing");
                        resync = true;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    })
}

/// Tell one client a terminal exited; once it has the message, the terminal can go.
async fn exit(state: &AppState, out: &mpsc::Sender<ServerMsg>, id: String, code: Option<u32>) {
    let sent = out
        .send(ServerMsg::TermExited {
            id: id.clone(),
            code,
        })
        .await;
    if sent.is_ok() {
        state.exit_delivered(&id);
    }
}

/// Stream an agent's conversation to one client: the transcript so far, then live events. Holds
/// the agent weakly, so the stream ends once the agent has ended and is dropped.
///
/// Three things send the transcript again: attaching, falling far enough behind that events were
/// dropped, and the agent moving to another conversation. A client replaces its whole transcript
/// with whatever arrives, so it never has to tell them apart — and a resend it did not strictly
/// need costs it nothing but the bytes.
fn forward_agent(
    agent: Arc<Agent>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    let name = agent.name.clone();
    let weak = Arc::downgrade(&agent);
    drop(agent);
    tokio::spawn(async move {
        let mut req = req;
        loop {
            // Subscribed before the transcript is read, so a conversation that changes between the
            // two is seen as a change rather than missed.
            let Some((mut switched, attachment)) = weak
                .upgrade()
                .map(|s| (s.conversation_changes(), s.attach()))
            else {
                return;
            };
            let AgentAttachment {
                state,
                events,
                config,
                context,
                mut rx,
            } = attachment;
            tracing::info!(%peer, agent = %name, events = events.len(), config = config.len(), "agent attach");
            if out
                .send(ServerMsg::AgentAttached {
                    req: req.take(),
                    agent: name.clone(),
                    state,
                    events,
                    config,
                    context,
                    now: wheel_proto::unix_ms_now(),
                })
                .await
                .is_err()
            {
                return;
            }
            // Read when it is sent, not with the transcript: a dismissal landing in between has
            // already told this connection it is down, and a copy taken earlier would put it back.
            let commands = weak.upgrade().and_then(|s| s.commands());
            if let Some(commands) = commands {
                let up = ServerMsg::AgentCommands {
                    agent: name.clone(),
                    commands,
                };
                if out.send(up).await.is_err() {
                    return;
                }
            }
            let footer = weak.upgrade().map(|s| s.footer());
            if let Some(footer) = footer {
                let up = ServerMsg::AgentFooter {
                    agent: name.clone(),
                    footer,
                };
                if out.send(up).await.is_err() {
                    return;
                }
            }
            let title = weak.upgrade().and_then(|s| s.title());
            if let Some(title) = title {
                let up = ServerMsg::AgentTitle {
                    agent: name.clone(),
                    title: Some(title),
                };
                if out.send(up).await.is_err() {
                    return;
                }
            }
            let notice = weak.upgrade().and_then(|s| s.notice());
            if let Some(notice) = notice {
                let up = ServerMsg::AgentNotice {
                    agent: name.clone(),
                    notice: Some(notice),
                };
                if out.send(up).await.is_err() {
                    return;
                }
            }
            loop {
                // Biased to the events: an agent that ends drops both channels at once, and the
                // `agent ended` error it queued first must reach the client before the closed
                // watch ends this task.
                let event = tokio::select! {
                    biased;
                    event = rx.recv() => event,
                    changed = switched.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        tracing::info!(%peer, agent = %name, "the agent is in another conversation; resending the transcript");
                        break;
                    }
                };
                match event {
                    Ok(event) => {
                        let msg = ServerMsg::Agent {
                            agent: name.clone(),
                            event,
                        };
                        if out.send(msg).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%peer, agent = %name, skipped, "client fell behind an agent; resending the transcript");
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    })
}
