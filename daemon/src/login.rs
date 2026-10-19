//! The lead's login. Two signals reach it. The agent's is authoritative: a lead turn that fails with
//! ACP's `authRequired` means the lead needs a login, and the login methods the agent advertised in
//! `initialize` say how. Wheel's own is an early warning: `claude auth status --json`, run with the
//! lead's `CLAUDE_CONFIG_DIR`, says whether that directory holds a login before anyone types. Both
//! readings are kept, and when they disagree the lead chat says so.
//!
//! A login runs the agent's command with the method's arguments in a terminal pane, where the person
//! answers whatever the login asks. When the command exits 0, the messages whose turns failed for
//! want of a login are sent again.

use std::str::FromStr;
use std::time::Duration;

use agent_client_protocol::AcpAgent;
use tokio::process::Command;
use wheel_proto::{
    AuthMethodInfo, CreatedBy, Dir, Extra, HostBinary, LeadLogin, LoginExit, LoginRun, LoginState,
    PaneKind, Preflight, ServerMsg,
};

use crate::server::{AppState, Launch, Owner};
use crate::settings::CLAUDE;
use crate::spaces::{PANE_COLS, PANE_ROWS};

const WHY: &str = "the lead's login preflight runs `claude auth status` with the lead's own config directory, to warn about a missing login before the lead is asked anything";
const INSTALL: &str = "install Claude Code from https://claude.com/claude-code, or set its absolute path with `wheel config set claude.path <path>`";

/// How long `claude auth status` gets to answer.
const PREFLIGHT_WAIT: Duration = Duration::from_secs(15);

/// The method a login uses when none is named.
pub const DEFAULT_METHOD: &str = "claude-ai-login";

/// What the lead's session tells the login tracker.
pub enum AuthEvent {
    /// The agent's `initialize` answer advertised these terminal login methods.
    Methods(Vec<AuthMethodInfo>),
    /// A turn with this message failed with `authRequired`.
    Required { text: String },
    /// `session/new` or `session/load` failed with `authRequired`: the lead has no conversation
    /// yet, so there is no message to keep.
    StartRequired,
    /// A turn ended without an error.
    TurnOk,
}

/// What the lead's login looks like now. Held under `AppState::login`.
#[derive(Default)]
pub struct Tracker {
    /// The lead's agent command line, which a login runs with the method's arguments added.
    command: Option<String>,
    methods: Vec<AuthMethodInfo>,
    preflight: Option<Preflight>,
    preflight_problem: Option<String>,
    agent_required: bool,
    /// Whether the lead's latest turn succeeded; None before any turn, or after a login.
    last_turn_ok: Option<bool>,
    kept: Vec<String>,
    running: Option<LoginRun>,
    last_exit: Option<LoginExit>,
}

impl Tracker {
    pub fn set_command(&mut self, command: String) {
        self.command = Some(command);
    }

    /// The tracker as a client sees it.
    pub fn view(&self) -> LeadLogin {
        let logged_in = self.preflight.as_ref().map(|p| p.logged_in);
        let state = if self.agent_required {
            LoginState::Needed
        } else {
            match (logged_in, self.last_turn_ok) {
                (Some(true), _) | (_, Some(true)) => LoginState::Ok,
                (Some(false), _) => LoginState::Needed,
                (None, _) => LoginState::Unknown,
            }
        };
        LeadLogin {
            state,
            preflight: self.preflight.clone(),
            preflight_problem: self.preflight_problem.clone(),
            agent_required: self.agent_required,
            methods: self.methods.clone(),
            kept_messages: self.kept.clone(),
            disagreement: self.disagreement(),
            running: self.running.clone(),
            last_exit: self.last_exit.clone(),
        }
    }

    /// Both readings in words, when the agent and the preflight contradict each other.
    fn disagreement(&self) -> Option<String> {
        let preflight = self.preflight.as_ref()?;
        let method = preflight.auth_method.as_deref().unwrap_or("none");
        if self.agent_required && preflight.logged_in {
            return Some(format!(
                "The agent refused a turn with authRequired, but `claude auth status` reports loggedIn: true (authMethod: {method}). The agent's reading decides: the lead needs a login."
            ));
        }
        if !self.agent_required && !preflight.logged_in && self.last_turn_ok == Some(true) {
            return Some(
                "`claude auth status` reports loggedIn: false (authMethod: none), but the lead's last turn succeeded. The agent's reading decides: the lead is usable."
                    .to_string(),
            );
        }
        None
    }

    /// Log both raw readings when they disagree.
    fn log_disagreement(&self) {
        if let Some(why) = self.disagreement() {
            tracing::warn!(
                agent_required = self.agent_required,
                last_turn_ok = ?self.last_turn_ok,
                preflight = %self.preflight.as_ref().map(|p| p.raw.as_str()).unwrap_or("-"),
                %why,
                "lead login: the agent and the preflight disagree"
            );
        }
    }
}

/// Methods in the order a picker shows them: Claude Subscription first, the rest as the agent
/// listed them.
fn order(mut methods: Vec<AuthMethodInfo>) -> Vec<AuthMethodInfo> {
    methods.sort_by_key(|m| m.id != DEFAULT_METHOD);
    methods
}

/// Read the preflight's answer out of what `claude auth status --json` printed.
fn read_preflight(stdout: &str) -> Result<Preflight, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("`claude auth status --json` printed no JSON ({e})"))?;
    let logged_in = value
        .get("loggedIn")
        .and_then(|v| v.as_bool())
        .ok_or("`claude auth status --json` printed no loggedIn field")?;
    Ok(Preflight {
        logged_in,
        auth_method: value
            .get("authMethod")
            .and_then(|v| v.as_str())
            .map(String::from),
        raw: stdout.trim().to_string(),
    })
}

impl AppState {
    /// The `claude` the preflight runs, as `wheel status` reports it.
    pub fn claude_info(&self) -> HostBinary {
        self.host_info(None, CLAUDE, "claude", WHY, INSTALL)
    }

    /// The lead's config home, once the lead exists.
    pub(crate) fn lead_home(&self) -> Option<crate::lead::AgentHome> {
        self.sessions
            .lock()
            .unwrap()
            .get(crate::session::LEAD)
            .and_then(|l| l.home().cloned())
    }

    fn login_view(&self) -> LeadLogin {
        self.login.lock().unwrap().view()
    }

    /// Tell every connection the login changed.
    fn login_changed(&self) {
        let _ = self.global.send(ServerMsg::LeadLoginChanged {
            login: self.login_view(),
        });
    }

    /// The lead's session reported something about its login.
    pub(crate) fn on_auth(&self, event: AuthEvent) {
        {
            let mut tracker = self.login.lock().unwrap();
            match event {
                AuthEvent::Methods(methods) => {
                    tracing::info!(
                        methods = %methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(","),
                        "lead login methods advertised"
                    );
                    tracker.methods = order(methods);
                }
                AuthEvent::Required { text } => {
                    tracing::warn!(
                        chars = text.len(),
                        "lead turn failed with authRequired; message kept"
                    );
                    tracker.agent_required = true;
                    tracker.last_turn_ok = Some(false);
                    tracker.kept.push(text);
                    tracker.log_disagreement();
                }
                AuthEvent::StartRequired => {
                    tracing::warn!("lead start failed with authRequired");
                    tracker.agent_required = true;
                    tracker.log_disagreement();
                }
                AuthEvent::TurnOk => {
                    tracker.last_turn_ok = Some(true);
                    tracker.agent_required = false;
                    // Whoever logged in some other way has moved on from those messages.
                    tracker.kept.clear();
                    tracker.log_disagreement();
                }
            }
        }
        self.login_changed();
    }

    /// Run `claude auth status --json` with the lead's config directory and store what it said.
    /// A missing `claude` or an answer that cannot be read leaves the preflight unset, with the
    /// reason beside it.
    pub async fn lead_preflight(&self) -> LeadLogin {
        let outcome = self.run_preflight().await;
        {
            let mut tracker = self.login.lock().unwrap();
            match outcome {
                Ok(preflight) => {
                    tracing::info!(
                        logged_in = preflight.logged_in,
                        auth_method = ?preflight.auth_method,
                        raw = %preflight.raw,
                        "lead login preflight"
                    );
                    tracker.preflight = Some(preflight);
                    tracker.preflight_problem = None;
                }
                Err(problem) => {
                    tracing::warn!(%problem, "lead login preflight could not run");
                    tracker.preflight = None;
                    tracker.preflight_problem = Some(problem);
                }
            }
            tracker.log_disagreement();
        }
        self.login_changed();
        self.login_view()
    }

    async fn run_preflight(&self) -> Result<Preflight, String> {
        let home = self.lead_home().ok_or("this daemon runs without a lead")?;
        let (claude, _) = self.resolve_host(None, CLAUDE, "claude", INSTALL)?;
        std::fs::create_dir_all(home.dir())
            .map_err(|e| format!("cannot create {}: {e}", home.dir().display()))?;
        let mut command = Command::new(&claude);
        command
            .args(["auth", "status", "--json"])
            .env("CLAUDE_CONFIG_DIR", home.dir())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(PREFLIGHT_WAIT, command.output())
            .await
            .map_err(|_| {
                format!(
                    "`{} auth status --json` gave no answer within {}s",
                    claude.display(),
                    PREFLIGHT_WAIT.as_secs()
                )
            })?
            .map_err(|e| format!("cannot run {}: {e}", claude.display()))?;
        // A logged-out claude may exit non-zero and still print its answer.
        let stdout = String::from_utf8_lossy(&output.stdout);
        read_preflight(&stdout).map_err(|e| {
            format!(
                "{e} (exit {:?}, stderr: {})",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
    }

    /// Log the lead in with `method` (default `claude-ai-login`): open a terminal pane in the
    /// active space running the agent's command with the method's arguments, wait for it to exit,
    /// and on exit 0 send the kept messages again. Returns the method, the pane and the exit code.
    pub async fn lead_login(&self, method: Option<String>) -> Result<LoginExit, String> {
        let method_id = method.unwrap_or_else(|| DEFAULT_METHOD.to_string());
        let home = self.lead_home().ok_or("this daemon runs without a lead")?;
        let (command, chosen) = {
            let mut tracker = self.login.lock().unwrap();
            if let Some(run) = &tracker.running {
                return Err(format!(
                    "a login ({}) is already running in pane {}",
                    run.method, run.pane
                ));
            }
            let command = tracker
                .command
                .clone()
                .ok_or("this daemon runs without a lead")?;
            let chosen = tracker
                .methods
                .iter()
                .find(|m| m.id == method_id)
                .cloned()
                .ok_or_else(|| {
                    if tracker.methods.is_empty() {
                        "the lead's agent has advertised no login methods; it does so once it has started".to_string()
                    } else {
                        format!(
                            "the lead's agent advertised no login method {method_id}; it advertised: {}",
                            tracker.methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ")
                        )
                    }
                })?;
            // Taken under the lock that checked it, so a second login asked for meanwhile is
            // refused; the pane is named once there is one.
            tracker.running = Some(LoginRun {
                method: chosen.id.clone(),
                pane: String::new(),
            });
            (command, chosen)
        };
        let outcome = self.login_in_pane(command, chosen, home).await;
        if outcome.is_err() {
            self.login.lock().unwrap().running = None;
            self.login_changed();
        }
        outcome
    }

    /// The rest of `lead_login`, once it holds the right to run: the pane, the wait, the aftermath.
    async fn login_in_pane(
        &self,
        command: String,
        chosen: AuthMethodInfo,
        home: crate::lead::AgentHome,
    ) -> Result<LoginExit, String> {
        let agent = AcpAgent::from_str(&command)
            .map_err(|e| format!("bad agent command {command:?}: {e}"))?
            .into_config();
        let program = agent.command().display().to_string();
        let mut args = agent.arguments().to_vec();
        args.extend(chosen.args.iter().cloned());
        let mut env: std::collections::BTreeMap<String, String> = agent
            .environment()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env.extend(chosen.env.clone());
        env.insert("CLAUDE_CONFIG_DIR".into(), home.dir().display().to_string());
        std::fs::create_dir_all(home.dir())
            .map_err(|e| format!("cannot create {}: {e}", home.dir().display()))?;

        // With no space open, the login gets one of its own: the person answers its question in
        // that space's only pane.
        let (space, pane) = {
            let spaces = self.spaces();
            match spaces.active {
                Some(id) => {
                    let space = spaces
                        .spaces
                        .into_iter()
                        .find(|s| s.id == id)
                        .ok_or_else(|| format!("no space {id}"))?;
                    let pane =
                        self.pane_split(&space.focus, Dir::Right, CreatedBy::Person, true)?;
                    (space, pane)
                }
                None => {
                    let cwd = std::env::var("HOME").unwrap_or_else(|_| "/".into());
                    let space = self.space_create(cwd, Some("Login".into()), CreatedBy::Person)?;
                    tracing::info!(space = %space.id, "lead login: no space was open, so it got one");
                    let pane = space.focus.clone();
                    (space, pane)
                }
            }
        };
        let term = match self
            .open_launched(
                space.cwd.clone(),
                Launch::Command {
                    program: program.clone(),
                    args: args.clone(),
                    env: env.clone().into_iter().collect(),
                },
                PANE_COLS,
                PANE_ROWS,
                Owner::Pane,
            )
            .await
        {
            Ok(term) => term,
            Err(e) => {
                // Closing a space's last pane closes the space, so a made space goes with it.
                let _ = self.pane_close(&pane);
                return Err(e);
            }
        };
        let shown = PaneKind::Terminal {
            term: term.id.clone(),
            cwd: None,
            agent: None,
            extra: Extra::default(),
        };
        if let Err(e) = self.install(&pane, shown, None) {
            term.kill();
            let _ = self.pane_close(&pane);
            return Err(e);
        }
        tracing::info!(method = %chosen.id, %pane, term = %term.id, %program, args = ?args, config_dir = %home.dir().display(), "lead login started");
        {
            let mut tracker = self.login.lock().unwrap();
            tracker.running = Some(LoginRun {
                method: chosen.id.clone(),
                pane: pane.clone(),
            });
            tracker.last_exit = None;
        }
        self.login_changed();

        let code = match term.attach().await {
            None => None,
            Some(mut attached) => match attached.exited {
                Some(code) => code,
                None => loop {
                    match attached.rx.recv().await {
                        Ok(crate::pty::TermEvent::Exited(code)) => break code,
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break None,
                    }
                },
            },
        };
        tracing::info!(method = %chosen.id, %pane, ?code, "lead login ended");
        let exit = LoginExit {
            method: chosen.id,
            pane,
            code,
        };
        let kept = {
            let mut tracker = self.login.lock().unwrap();
            tracker.running = None;
            tracker.last_exit = Some(exit.clone());
            if code == Some(0) {
                tracker.agent_required = false;
                tracker.last_turn_ok = None;
                std::mem::take(&mut tracker.kept)
            } else {
                Vec::new()
            }
        };
        self.login_changed();
        if code == Some(0) {
            self.lead_preflight().await;
            match self.session(crate::session::LEAD) {
                Ok(lead) => {
                    for text in kept {
                        if let Err(e) = lead.send(text) {
                            tracing::warn!(error = %e, "a kept lead message could not be sent again");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, dropped = kept.len(), "kept lead messages could not be sent again");
                }
            }
        }
        Ok(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preflight_reads_what_claude_printed() {
        let said = r#"{"loggedIn": true, "authMethod": "claude.ai"}"#;
        let p = read_preflight(said).unwrap();
        assert!(p.logged_in);
        assert_eq!(p.auth_method.as_deref(), Some("claude.ai"));
        let out = read_preflight(r#"{"loggedIn": false, "authMethod": "none"}"#).unwrap();
        assert!(!out.logged_in);
        assert!(read_preflight("not json").is_err());
        assert!(read_preflight("{}").is_err());
    }

    #[test]
    fn methods_list_claude_subscription_first() {
        let m = |id: &str| AuthMethodInfo {
            id: id.into(),
            name: id.into(),
            ..AuthMethodInfo::default()
        };
        let ordered = order(vec![m("console-login"), m("claude-ai-login"), m("other")]);
        let ids: Vec<_> = ordered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["claude-ai-login", "console-login", "other"]);
    }
}
