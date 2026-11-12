//! The server as the client carries and controls it: DaemonService.swift (the wheeld in the
//! bundle and the launch agent `wheel-server.sh` runs it as), ServerControl.swift (how a restart
//! runs, the restart window and the rollback that follows a wheeld that never answers again) and
//! the words the header and the notices use. A client built for `PROTOCOL` that meets an older
//! wheeld, or one whose running source is not the bundled one, offers Restart Server; it never
//! restarts wheeld on its own (ADR-0020). Every command here blocks, so `view/toasts.rs` runs
//! them on a thread and reads the outcome at the next tick. Stop Server and Start Server from
//! the menu bar item are wheel-ynjo.7's, with the stopped-from-the-menu state.
use std::path::{Path, PathBuf};

/// What the banner, the notice and Settings say while a restart waits.
pub const UPDATED: &str = "The server was updated.";
/// Why Restart Server is not offered for a wheeld the client has no commands for.
pub const UNAVAILABLE: &str = "This server was started by something other than Wheel, so Wheel cannot stop it.";
/// Why a rollback is refused for a wheeld the client did not install.
pub const NO_ROLLBACK: &str = "This server was started by something other than Wheel.";
/// How long a wheeld that said it is stopping has to answer again before a rollback is offered,
/// and how long the person has to cancel that rollback (AppModel.restartWindow, rollbackCountdown).
pub const RESTART_WINDOW_MS: u64 = 10_000;
pub const ROLLBACK_COUNTDOWN_MS: u64 = 10_000;

/// The wheeld this app carries (`apple/Mac/embed-wheeld.sh` puts it in the bundle, `tools/build.py`
/// runs that for this app): which daemon instance it is, where it listens, the source it was built
/// from, and the script that copies it out of the bundle and runs it as its own launch agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub label: String,
    pub instance: String,
    pub listen: String,
    pub name: String,
    /// `wheeld --source-hash` of the bundled server; None for a bundle built before it was written.
    pub bundled_source: Option<String>,
    /// `Resources/wheel-server.sh` beside the plist.
    pub script: PathBuf,
}

impl Service {
    /// The daemon this bundle carries, from `Contents/Resources/WheelDaemon.plist`; None for a
    /// bare cargo binary or a bundle without the embed phase.
    pub fn bundled() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let resources = exe.parent()?.parent()?.join("Resources");
        Self::read(&resources.join("WheelDaemon.plist"))
    }
    /// The service `plist` describes (`Label`, `Instance`, `Listen`, `ServerName`, `SourceHash`),
    /// with `wheel-server.sh` beside it. `plutil` reads it: the script itself needs `plutil` too.
    pub fn read(plist: &Path) -> Option<Self> {
        let out = std::process::Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-"]).arg(plist).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let info: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let text = |key: &str| info.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Service {
            label: text("Label")?,
            instance: text("Instance")?,
            listen: text("Listen")?,
            name: text("ServerName").unwrap_or_else(|| "Wheel Server".into()),
            bundled_source: text("SourceHash"),
            script: plist.with_file_name("wheel-server.sh"),
        })
    }
    pub fn url(&self) -> String {
        format!("ws://{}/ws", self.listen)
    }
    pub fn token_file(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(format!("Library/Application Support/{}/token", self.instance))
    }
    /// Run one verb of the script; its last output line, or why it failed.
    pub fn run(&self, verb: &str) -> Result<String, String> {
        if !self.script.exists() {
            return Err("wheel-server.sh is not in this app".into());
        }
        let out = std::process::Command::new("/bin/bash").arg(&self.script).arg(verb).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&[out.stdout.as_slice(), out.stderr.as_slice()].concat()).trim().to_owned();
        if out.status.success() { Ok(text.lines().last().unwrap_or("").to_owned()) } else { Err(text) }
    }
}

/// How the client stops and starts the wheeld it talks to: the launch agent its own bundled
/// wheeld runs as (LaunchAgentControl), the commands in `WHEEL_SERVER_STOP` and
/// `WHEEL_SERVER_START` for a wheeld the app did not install (CommandControl), or no way at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    LaunchAgent(Service),
    Commands { stop: Option<String>, start: Option<String> },
}

impl Default for Control {
    fn default() -> Self {
        Control::Commands { stop: None, start: None }
    }
}

impl Control {
    /// The control for the daemon an endpoint names: its bundled service when the endpoint came
    /// from the bundle, else the commands in the environment (AppModel.init).
    pub fn for_service(service: Option<Service>) -> Self {
        service.map(Control::LaunchAgent).unwrap_or_else(Self::from_env)
    }
    pub fn from_env() -> Self {
        let read = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Control::Commands { stop: read("WHEEL_SERVER_STOP"), start: read("WHEEL_SERVER_START") }
    }
    pub fn service(&self) -> Option<&Service> {
        match self {
            Control::LaunchAgent(service) => Some(service),
            Control::Commands { .. } => None,
        }
    }
    /// Why the client cannot stop and start this server; None when it can.
    pub fn unavailable(&self) -> Option<&'static str> {
        match self {
            Control::LaunchAgent(_) => None,
            Control::Commands { stop, start } => (stop.is_none() && start.is_none()).then_some(UNAVAILABLE),
        }
    }
    pub fn available(&self) -> bool {
        self.unavailable().is_none()
    }
    /// What `/state` calls this control.
    pub fn kind(&self) -> &'static str {
        match self {
            Control::LaunchAgent(_) => "launch-agent",
            Control::Commands { .. } => "commands",
        }
    }
    /// Run `op`; Ok with the script's last line (empty for a command), Err with why not, in the
    /// Swift client's words.
    pub fn run(&self, op: Op) -> Result<String, String> {
        match self {
            Control::LaunchAgent(service) => service.run(op.verb()),
            Control::Commands { stop, start } => match op {
                Op::Start => run(start.as_deref()),
                Op::Stop => run(stop.as_deref()),
                Op::Restart => run(stop.as_deref()).and_then(|_| run(start.as_deref())),
                Op::RollbackTarget => Err("no earlier version is kept".into()),
                Op::Rollback => Err(NO_ROLLBACK.into()),
            },
        }
    }
}

/// One thing the control does on its thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Copy the bundled server out and load its agent unless one runs (DaemonService.register).
    Start,
    /// Unload and disable the launch agent, so it stays down (Stop Server).
    Stop,
    Restart,
    /// Ask which version a rollback would run.
    RollbackTarget,
    Rollback,
}

impl Op {
    /// The `wheel-server.sh` verb.
    fn verb(self) -> &'static str {
        match self {
            Op::Start => "start",
            Op::Stop => "stop",
            Op::Restart => "restart",
            Op::RollbackTarget => "rollbackable",
            Op::Rollback => "rollback",
        }
    }
}

fn run(command: Option<&str>) -> Result<String, String> {
    let Some(command) = command else { return Err("No command is set for this.".into()) };
    match std::process::Command::new("/bin/sh").arg("-c").arg(command).status() {
        Ok(status) if status.success() => Ok(String::new()),
        Ok(status) => Err(format!("The command exited with {}.", status.code().unwrap_or(-1))),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether a newer server waits for a restart: the bundled source differs from the running one,
/// or the running one speaks an older protocol than this client (AppModel.serverRestartWaiting).
/// `running` is the `status` host's source, None before that wheeld answered a `status` (then the
/// source says nothing yet, as Swift's `serverSources` is nil without a host) and empty for a
/// wheeld that predates the hash.
pub fn restart_waiting(connected: bool, running: Option<&str>, bundled: Option<&str>, server_protocol: Option<u32>, client_protocol: u32) -> bool {
    connected
        && (matches!((running, bundled), (Some(r), Some(b)) if r != b) || server_protocol.is_some_and(|p| p < client_protocol))
}

/// What a dismissed notice was about: the bundled source, or the protocol when there is none
/// (AppModel.serverBannerKey). The same key for the same server keeps the notice closed.
pub fn banner_key(bundled: Option<&str>, server_protocol: Option<u32>) -> String {
    bundled.map(str::to_owned).unwrap_or_else(|| format!("protocol {}", server_protocol.unwrap_or(0)))
}

/// The one log line per connection that says whether the running server is the bundled one
/// (AppModel.logServerSource).
pub fn source_line(label: &str, running: Option<&str>, bundled: Option<&str>, waiting: bool) -> String {
    let running = running.filter(|r| !r.is_empty()).unwrap_or("none reported");
    let bundled = bundled.unwrap_or("none recorded");
    format!("daemon {label}: running source {running}, bundled source {bundled}: {}", if waiting { "different; restart offered" } else { "same; nothing to offer" })
}

/// The link as the header words see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Connecting,
    Connected,
    Unreachable,
}

/// The header's word and dot colour (AppModel.serverStatus). The colour is never the only signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub word: String,
    pub colour: u32,
}

/// What the remote listener reports, for the header word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remote {
    pub problem: bool,
    pub listening: bool,
}

pub const GREY: u32 = 0x8e8e93;
pub const YELLOW: u32 = 0xffd60a;
pub const ORANGE: u32 = 0xff9f0a;
pub const GREEN: u32 = 0x30d158;

/// `stopped`: Stop Server ran from the menu, whatever the link says now.
pub fn status(link: LinkKind, waiting: bool, remote: Option<Remote>, stopped: bool) -> Status {
    if stopped {
        return Status { word: "Stopped".into(), colour: GREY };
    }
    let (word, colour) = match link {
        LinkKind::Connecting => ("Starting…", YELLOW),
        LinkKind::Unreachable => ("Stopped", GREY),
        LinkKind::Connected if waiting => ("Running · restart to update", ORANGE),
        LinkKind::Connected => match remote {
            Some(Remote { problem: true, .. }) => ("Remote listener failed", ORANGE),
            Some(Remote { listening: false, .. }) => ("Running · remote access off", GREEN),
            _ => ("Running", GREEN),
        },
    };
    Status { word: word.into(), colour }
}

/// "1 space", "3 spaces".
pub fn spaces(count: usize) -> String {
    format!("{count} {}", if count == 1 { "space" } else { "spaces" })
}

/// How long wheeld has run, in at most two units of days, hours and minutes, written out
/// (`Duration.formatted(.units(allowed: [.days, .hours, .minutes], width: .wide, maximumUnitCount: 2))`).
pub fn uptime(secs: u64) -> String {
    let units = [(secs / 86_400, "day"), (secs % 86_400 / 3_600, "hour"), (secs % 3_600 / 60, "minute")];
    let parts: Vec<String> = units.into_iter().skip_while(|(n, _)| *n == 0).take(2).filter(|(n, _)| *n > 0)
        .map(|(n, unit)| format!("{n} {unit}{}", if n == 1 { "" } else { "s" })).collect();
    if parts.is_empty() { "0 minutes".into() } else { parts.join(", ") }
}

/// The remote listener as the details line reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listener {
    Failed(String),
    Listening { address: String, port: u16 },
    Off,
}

/// What the lines under the server's header are made from (AppModel.serverDetails).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub stopped: bool,
    /// None while connecting; Some(reason) while unreachable; connected otherwise.
    pub down: Option<Option<String>>,
    pub waiting: bool,
    pub uptime_secs: Option<u64>,
    pub spaces: usize,
    /// None before wheeld answered `remote_info`.
    pub listener: Option<Listener>,
    pub paired: Vec<String>,
}

/// The lines under the server's header while it is open.
pub fn details(f: &Facts) -> Vec<String> {
    if f.stopped {
        return vec![format!("Stopped from this menu. Terminals and agents in {} are not running.", spaces(f.spaces))];
    }
    match &f.down {
        Some(Some(reason)) => return vec![format!("wheeld unreachable: {reason}")],
        Some(None) => return vec!["Connecting to wheeld…".into()],
        None => {}
    }
    let mut lines = vec![];
    if f.waiting {
        lines.push(UPDATED.to_owned());
    }
    if let Some(secs) = f.uptime_secs {
        lines.push(format!("wheeld up {} · {}", uptime(secs), spaces(f.spaces)));
    }
    match &f.listener {
        Some(Listener::Failed(problem)) => lines.push(format!("Remote listener failed: {problem}")),
        Some(Listener::Listening { address, port }) => lines.push(format!("Listening on {address}:{port}")),
        Some(Listener::Off) => lines.push("Remote access is off".into()),
        None => {}
    }
    lines.push(match f.paired.len() {
        0 => "No paired devices".into(),
        n => format!("{n} paired {}: {}", if n == 1 { "device" } else { "devices" }, f.paired.join(", ")),
    });
    lines
}

/// One item of the status item's right-click menu; `id` "-" is a separator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub id: &'static str,
    pub title: &'static str,
    pub key: &'static str,
}

/// The right-click menu (StatusItemMenu.menu): Stop Server… while it runs, which asks first;
/// otherwise Start Server, and Stop Server too while it is unreachable but was not stopped from
/// the menu (a server restarting itself in a loop); then Restart Server and Quit Wheel.
pub fn menu(running: bool, stopped_by_menu: bool) -> Vec<MenuItem> {
    let item = |id, title| MenuItem { id, title, key: "" };
    let mut items = vec![];
    if running {
        items.push(item("stop", "Stop Server…"));
    } else {
        items.push(item("start", "Start Server"));
        if !stopped_by_menu {
            items.push(item("stop", "Stop Server"));
        }
    }
    items.push(item("restart", "Restart Server"));
    items.push(item("-", ""));
    items.push(MenuItem { id: "quit", title: "Quit Wheel", key: "q" });
    items
}

/// What the stop question says (StatusItemMenu.stopServer, ServerDetails' confirm card).
pub fn stop_question(spaces_count: usize) -> (&'static str, String) {
    ("Stop Wheel Server?", format!("Terminals and running agents in {} stop.", spaces(spaces_count)))
}

/// What the "Rolling back" notice counts down to (RollbackOffer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub target: String,
    pub from: u64,
    pub deadline: u64,
}

/// What the view runs on a thread when the watch asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    AskRollbackTarget,
    Rollback(String),
}

/// What happens after wheeld says it is stopping (AppModel's serverStopping, restartOverdue,
/// runRollback and serverAnswered), with the clock passed in so it can be tested: the frame is
/// taken as a restart, and a wheeld that does not answer again within `RESTART_WINDOW_MS` gets
/// the earlier version offered, which runs after `ROLLBACK_COUNTDOWN_MS` unless cancelled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// wheeld said it is stopping and is expected back; the window is not called down meanwhile.
    pub restart_expected: bool,
    restart_due: Option<u64>,
    /// `rollbackable` is being asked on a thread.
    asking_target: bool,
    pub rollback_offer: Option<Offer>,
    pub rolled_back_to: Option<String>,
}

impl Watch {
    /// wheeld sent `stopping`. Returns the log line.
    pub fn stopping(&mut self, now: u64) -> String {
        self.expect_restart(now);
        format!("server: wheeld is stopping; waiting {} s for it to answer again", RESTART_WINDOW_MS / 1000)
    }
    fn expect_restart(&mut self, now: u64) {
        self.restart_expected = true;
        self.restart_due = Some(now + RESTART_WINDOW_MS);
    }
    /// Each tick. A step to run on a thread, with the log lines so far.
    pub fn tick(&mut self, now: u64, connected: bool, control_available: bool) -> (Option<Step>, Vec<String>) {
        let mut log = vec![];
        if let Some(due) = self.restart_due
            && now >= due
        {
            self.restart_due = None;
            if !connected {
                log.push(format!("server: wheeld did not answer again within {} s", RESTART_WINDOW_MS / 1000));
                if control_available {
                    self.asking_target = true;
                    return (Some(Step::AskRollbackTarget), log);
                }
                self.restart_expected = false;
            }
        }
        if let Some(offer) = &self.rollback_offer
            && now >= offer.deadline
        {
            let target = offer.target.clone();
            // The offer ends before the script runs: Cancel cannot stop a rollback that has begun.
            self.rollback_offer = None;
            if connected {
                return (None, log);
            }
            self.expect_restart(now);
            return (Some(Step::Rollback(target)), log);
        }
        (None, log)
    }
    /// `rollbackable` answered (Ok with the version, Err with why there is none). The log line.
    pub fn target_answered(&mut self, now: u64, answer: Result<String, String>, connected: bool) -> Option<String> {
        self.asking_target = false;
        if connected {
            return None;
        }
        self.restart_expected = false;
        match answer {
            Err(why) => Some(format!("server: no earlier version to roll back to ({why})")),
            Ok(target) => {
                self.rollback_offer = Some(Offer { target: target.clone(), from: now, deadline: now + ROLLBACK_COUNTDOWN_MS });
                Some(format!("server: rolling back to {target} in {} s unless cancelled", ROLLBACK_COUNTDOWN_MS / 1000))
            }
        }
    }
    pub fn cancel_rollback(&mut self) {
        self.rollback_offer = None;
    }
    /// `rollback` ended: wheeld runs the earlier version, or the script failed.
    pub fn rollback_ended(&mut self, target: &str, outcome: Result<String, String>) -> String {
        match outcome {
            Ok(_) => {
                self.rolled_back_to = Some(target.to_owned());
                format!("server: rolled back to {target}")
            }
            Err(failure) => {
                self.abandon();
                format!("server: rollback to {target} failed: {failure}")
            }
        }
    }
    /// Nothing is expected back any more: a rollback failed.
    pub fn abandon(&mut self) {
        self.restart_expected = false;
        self.restart_due = None;
        self.rollback_offer = None;
    }
    /// wheeld answers again: whatever was waiting on it is over. True when something was.
    pub fn answered(&mut self) -> bool {
        let was = self.restart_expected || self.rollback_offer.is_some() || self.asking_target;
        self.abandon();
        was
    }
    /// When the next timed step is due, for the view's timer.
    pub fn next_due(&self) -> Option<u64> {
        [self.restart_due, self.rollback_offer.as_ref().map(|o| o.deadline)].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bundle(name: &str, body: &str) -> (PathBuf, Service) {
        let dir = std::env::temp_dir().join(format!("wheel-gpui-server-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let plist = dir.join("WheelDaemon.plist");
        std::fs::write(&plist, r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.piercemakes.wheel.test.daemon</string>
<key>Instance</key><string>wheel-test</string>
<key>Listen</key><string>127.0.0.1:7499</string>
<key>ServerName</key><string>Wheel Test Server</string>
<key>SourceHash</key><string>0123456789abcdef</string>
</dict></plist>"#).unwrap();
        std::fs::write(dir.join("wheel-server.sh"), format!("#!/bin/bash\necho \"$1\" >> \"{}/verbs\"\n{body}\n", dir.display())).unwrap();
        let service = Service::read(&plist).expect("the plist reads");
        (dir, service)
    }

    #[test]
    fn the_bundled_plist_names_the_service() {
        let (dir, service) = fake_bundle("plist", "");
        assert_eq!(service.label, "com.piercemakes.wheel.test.daemon");
        assert_eq!(service.instance, "wheel-test");
        assert_eq!(service.url(), "ws://127.0.0.1:7499/ws");
        assert_eq!(service.bundled_source.as_deref(), Some("0123456789abcdef"));
        assert!(service.token_file().ends_with("Library/Application Support/wheel-test/token"));
        assert_eq!(service.script, dir.join("wheel-server.sh"));
        assert_eq!(Service::read(&dir.join("missing.plist")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_launch_agent_control_runs_the_script_verbs() {
        let (dir, service) = fake_bundle("verbs", r#"case "$1" in rollbackable) echo "no earlier version is kept" >&2; exit 1;; restart) echo "x restarted, running abc";; esac"#);
        let control = Control::for_service(Some(service));
        assert!(control.available());
        assert_eq!(control.kind(), "launch-agent");
        assert_eq!(control.run(Op::Start), Ok(String::new()));
        assert_eq!(control.run(Op::Restart), Ok("x restarted, running abc".to_owned()));
        assert_eq!(control.run(Op::RollbackTarget), Err("no earlier version is kept".to_owned()));
        assert_eq!(std::fs::read_to_string(dir.join("verbs")).unwrap(), "start\nrestart\nrollbackable\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_commands_means_no_restart_offered() {
        assert_eq!(Control::default().unavailable(), Some(UNAVAILABLE));
        assert_eq!(Control::default().kind(), "commands");
        assert!(Control::Commands { stop: Some("true".into()), start: None }.available());
        assert_eq!(Control::default().run(Op::Rollback), Err(NO_ROLLBACK.to_owned()));
    }

    #[test]
    fn restart_runs_stop_then_start_and_reports_the_first_failure() {
        let dir = std::env::temp_dir().join(format!("wheel-gpui-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let both = Control::Commands { stop: Some(format!("touch {}/stop", dir.display())), start: Some(format!("touch {}/start", dir.display())) };
        assert_eq!(both.run(Op::Restart), Ok(String::new()));
        assert!(dir.join("stop").exists() && dir.join("start").exists());
        let failing = Control::Commands { stop: Some("exit 3".into()), start: Some(format!("touch {}/never", dir.display())) };
        assert_eq!(failing.run(Op::Restart).unwrap_err(), "The command exited with 3.");
        assert!(!dir.join("never").exists(), "start ran after a failed stop");
        let half = Control::Commands { stop: None, start: Some("true".into()) };
        assert_eq!(half.run(Op::Restart).unwrap_err(), "No command is set for this.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_waits_when_the_source_or_the_protocol_is_behind() {
        assert!(!restart_waiting(false, Some("a"), Some("b"), Some(1), 2));
        assert!(restart_waiting(true, Some("a"), Some("b"), Some(2), 2));
        assert!(restart_waiting(true, Some(""), Some("b"), Some(2), 2), "a wheeld that reports no source is not the bundled one");
        assert!(!restart_waiting(true, None, Some("b"), Some(2), 2), "before the first status the source says nothing");
        assert!(!restart_waiting(true, Some("b"), Some("b"), Some(2), 2));
        assert!(restart_waiting(true, Some("b"), Some("b"), Some(1), 2));
        assert!(!restart_waiting(true, Some("x"), None, Some(2), 2), "no bundled server: only the protocol counts");
        assert_eq!(banner_key(Some("b"), Some(3)), "b");
        assert_eq!(banner_key(None, Some(3)), "protocol 3");
        assert_eq!(source_line("l", Some(""), None, false), "daemon l: running source none reported, bundled source none recorded: same; nothing to offer");
        assert_eq!(source_line("l", Some("a"), Some("b"), true), "daemon l: running source a, bundled source b: different; restart offered");
    }

    #[test]
    fn the_header_word_follows_the_link() {
        assert_eq!(status(LinkKind::Connecting, false, None, false).word, "Starting…");
        assert_eq!(status(LinkKind::Unreachable, false, None, false).word, "Stopped");
        assert_eq!(status(LinkKind::Connected, true, None, false), Status { word: "Running · restart to update".into(), colour: ORANGE });
        assert_eq!(status(LinkKind::Connected, false, Some(Remote { problem: true, listening: false }), false).word, "Remote listener failed");
        assert_eq!(status(LinkKind::Connected, false, Some(Remote { problem: false, listening: false }), false).word, "Running · remote access off");
        assert_eq!(status(LinkKind::Connected, false, Some(Remote { problem: false, listening: true }), false), Status { word: "Running".into(), colour: GREEN });
        assert_eq!(status(LinkKind::Connected, false, None, false).word, "Running");
        assert_eq!(status(LinkKind::Connected, true, None, true), Status { word: "Stopped".into(), colour: GREY }, "stopped from the menu wins");
    }

    #[test]
    fn the_details_follow_the_link_the_listener_and_the_devices() {
        assert_eq!(uptime(0), "0 minutes");
        assert_eq!(uptime(61), "1 minute");
        assert_eq!(uptime(3_600 * 2 + 60 * 5 + 9), "2 hours, 5 minutes");
        assert_eq!(uptime(86_400 + 3_600 + 60), "1 day, 1 hour");
        assert_eq!(uptime(86_400 * 3 + 60 * 4), "3 days", "a zero middle unit still counts toward two");
        let up = Facts { uptime_secs: Some(125), spaces: 1, listener: Some(Listener::Off), ..Facts::default() };
        assert_eq!(details(&up), ["wheeld up 2 minutes · 1 space", "Remote access is off", "No paired devices"]);
        let busy = Facts { waiting: true, uptime_secs: Some(3_600), spaces: 4, listener: Some(Listener::Listening { address: "100.64.0.1".into(), port: 7421 }), paired: vec!["Phone".into(), "Pad".into()], ..Facts::default() };
        assert_eq!(details(&busy), ["The server was updated.", "wheeld up 1 hour · 4 spaces", "Listening on 100.64.0.1:7421", "2 paired devices: Phone, Pad"]);
        assert_eq!(details(&Facts { listener: Some(Listener::Failed("port taken".into())), paired: vec!["Phone".into()], ..Facts::default() }), ["Remote listener failed: port taken", "1 paired device: Phone"]);
        assert_eq!(details(&Facts { down: Some(Some("refused".into())), ..Facts::default() }), ["wheeld unreachable: refused"]);
        assert_eq!(details(&Facts { down: Some(None), ..Facts::default() }), ["Connecting to wheeld…"]);
        assert_eq!(details(&Facts { stopped: true, spaces: 2, down: Some(Some("refused".into())), ..Facts::default() }), ["Stopped from this menu. Terminals and agents in 2 spaces are not running."]);
    }

    #[test]
    fn the_right_click_menu_follows_whether_the_server_runs() {
        let titles = |items: Vec<MenuItem>| items.iter().map(|i| if i.id == "-" { "-" } else { i.title }).collect::<Vec<_>>();
        assert_eq!(titles(menu(true, false)), ["Stop Server…", "Restart Server", "-", "Quit Wheel"]);
        assert_eq!(titles(menu(false, true)), ["Start Server", "Restart Server", "-", "Quit Wheel"]);
        assert_eq!(titles(menu(false, false)), ["Start Server", "Stop Server", "Restart Server", "-", "Quit Wheel"]);
        assert_eq!(menu(true, false).last().map(|i| i.key), Some("q"));
        assert_eq!(stop_question(1), ("Stop Wheel Server?", "Terminals and running agents in 1 space stop.".to_owned()));
    }

    #[test]
    fn a_stopping_wheeld_is_waited_for_then_rolled_back() {
        let mut w = Watch::default();
        assert!(w.stopping(1_000).starts_with("server: wheeld is stopping; waiting 10 s"));
        assert!(w.restart_expected);
        assert_eq!(w.next_due(), Some(11_000));
        assert_eq!(w.tick(5_000, false, true), (None, vec![]));
        // Back in time: nothing is asked.
        let mut back = w.clone();
        assert!(back.answered());
        assert_eq!(back, Watch::default());
        assert_eq!(back.tick(20_000, true, true), (None, vec![]));
        // Still down at the deadline: the earlier version is asked for, then offered.
        let (step, log) = w.tick(11_000, false, true);
        assert_eq!(step, Some(Step::AskRollbackTarget));
        assert_eq!(log, vec!["server: wheeld did not answer again within 10 s"]);
        assert!(w.restart_expected, "still expected while the target is asked");
        assert_eq!(w.target_answered(11_200, Ok("v1".into()), false).as_deref(), Some("server: rolling back to v1 in 10 s unless cancelled"));
        assert!(!w.restart_expected);
        assert_eq!(w.rollback_offer, Some(Offer { target: "v1".into(), from: 11_200, deadline: 21_200 }));
        assert_eq!(w.next_due(), Some(21_200));
        let (step, _) = w.tick(21_200, false, true);
        assert_eq!(step, Some(Step::Rollback("v1".into())));
        assert_eq!(w.rollback_offer, None);
        assert!(w.restart_expected, "the rolled-back wheeld is waited for in turn");
        assert_eq!(w.rollback_ended("v1", Ok("rolled back".into())), "server: rolled back to v1");
        assert_eq!(w.rolled_back_to.as_deref(), Some("v1"));
        assert!(w.answered());
        assert_eq!(w.rolled_back_to.as_deref(), Some("v1"), "the Rolled back notice stays until closed");
    }

    #[test]
    fn no_earlier_version_a_cancel_and_an_answer_end_the_watch() {
        let mut w = Watch::default();
        w.stopping(0);
        assert_eq!(w.tick(10_000, false, true).0, Some(Step::AskRollbackTarget));
        assert_eq!(w.target_answered(10_000, Err("no earlier version is kept".into()), false).as_deref(), Some("server: no earlier version to roll back to (no earlier version is kept)"));
        assert_eq!(w, Watch::default());
        // No control: the window just ends.
        w.stopping(0);
        assert_eq!(w.tick(10_000, false, false), (None, vec!["server: wheeld did not answer again within 10 s".to_owned()]));
        assert!(!w.restart_expected);
        // Cancel keeps the offer from running.
        w.stopping(0);
        w.tick(10_000, false, true);
        w.target_answered(10_000, Ok("v1".into()), false);
        w.cancel_rollback();
        assert_eq!(w.tick(30_000, false, true), (None, vec![]));
        // A failed rollback abandons the watch.
        assert!(w.rollback_ended("v1", Err("boom".into())).ends_with("failed: boom"));
        assert!(!w.restart_expected && w.rollback_offer.is_none());
        // wheeld answering while the target is asked drops the answer.
        w.stopping(0);
        w.tick(10_000, false, true);
        assert!(w.answered());
        assert_eq!(w.target_answered(10_100, Ok("v1".into()), true), None);
        assert_eq!(w.rollback_offer, None);
    }
}
