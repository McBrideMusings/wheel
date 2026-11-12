//! The server's control, one per app (ServerControl.swift on AppModel): every window's notice and
//! Settings box, the menu bar's server line and the status item's right-click menu run their
//! Stop, Start and Restart through it, so one command runs at a time and one watch follows a
//! wheeld that said it is stopping. Commands run on a thread; a task on the app reads how each
//! ended and advances the watch's restart window and rollback countdown on time, with or without
//! a window open.
use crate::server::{Control, Op, Service, Step, Watch};
use futures::{StreamExt as _, channel::mpsc};
use gpui::App;
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::{Arc, Mutex}};

/// What a command's thread hands back: the command, the version a rollback ran, and the script's
/// last line or why it failed.
pub struct Outcome {
    pub op: Op,
    pub target: Option<String>,
    pub result: Result<String, String>,
}

pub struct Server {
    pub control: Control,
    /// The command running on its thread now, if one is.
    pub busy: Option<Op>,
    done: Arc<Mutex<Vec<Outcome>>>,
    /// Wakes the app's task: a command ended, or the watch has a new deadline.
    wake: mpsc::UnboundedSender<()>,
    /// Why the last command did not work (AppModel.serverProblem).
    pub problem: Option<String>,
    /// How the last command ended, for `/state`.
    pub service_status: String,
    pub watch: Watch,
    /// Stop Server ran from the menu and Start Server has not since (AppModel.serverStoppedByMenu);
    /// kept across launches, so a stopped server is not started again at launch.
    pub stopped_by_menu: bool,
    /// Stop Server is running: a `stopping` frame now is the stop, not a restart.
    stop_asked: bool,
    /// The Start running came from Start Server, not from the launch's registration.
    menu_start: bool,
    /// The stop question shows in the server line's details (AppModel.serverConfirming).
    pub confirming: bool,
    /// The server line's details are open (AppModel.serverExpanded).
    pub expanded: bool,
    /// Whether wheeld answered last: the watch's deadlines ask this.
    pub connected: bool,
    stopped_file: Option<PathBuf>,
    pub log: Vec<String>,
}

thread_local! {
    static SERVER: RefCell<Option<Rc<RefCell<Server>>>> = const { RefCell::new(None) };
}

fn shared() -> Option<Rc<RefCell<Server>>> {
    SERVER.with(|s| s.borrow().clone())
}

/// Read the app's server control; None before `ensure` made it.
pub fn with<R>(f: impl FnOnce(&Server) -> R) -> Option<R> {
    shared().map(|s| f(&s.borrow()))
}

/// Change the app's server control, and wake its task so a new deadline or command is followed.
pub fn with_mut<R>(f: impl FnOnce(&mut Server) -> R) -> Option<R> {
    let shared = shared()?;
    let mut server = shared.borrow_mut();
    let out = f(&mut server);
    let _ = server.wake.unbounded_send(());
    Some(out)
}

/// Where Stop Server is remembered for `control` across launches (`Prefs` key `serverStopped`).
fn stopped_file(control: &Control) -> PathBuf {
    let name = control.service().map(|s| s.instance.clone()).unwrap_or_else(|| "external".into());
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(format!("Library/Application Support/wheel-gpui/server-stopped-{name}"))
}

/// Make the app's server control for `service` (the bundled wheeld) or the commands in the
/// environment, once per process: the first window or the menu bar makes it. A bundled server
/// is copied out and loaded at launch unless it was stopped from the menu (DaemonService.register).
/// A hidden check remembers nothing across runs.
pub fn ensure(cx: &mut App, service: Option<Service>, hidden: bool) {
    if shared().is_some() {
        return;
    }
    let control = Control::for_service(service);
    let file = (!hidden).then(|| stopped_file(&control));
    let stopped_by_menu = file.as_ref().is_some_and(|f| f.exists());
    let (wake, mut woken) = mpsc::unbounded::<()>();
    let server = Server {
        control, busy: None, done: Arc::default(), wake, problem: None, service_status: "not registered yet".into(), watch: Watch::default(),
        stopped_by_menu, stop_asked: false, menu_start: false, confirming: false, expanded: false, connected: false, stopped_file: file, log: vec![],
    };
    SERVER.with(|s| *s.borrow_mut() = Some(Rc::new(RefCell::new(server))));
    with_mut(|s| {
        if s.control.service().is_some() {
            if s.stopped_by_menu {
                s.note("server was stopped from the menu; not starting it".into());
            } else {
                s.run(Op::Start, None);
            }
        }
    });
    cx.spawn(async move |cx| {
        loop {
            // The watch advances only between commands, so no deadline is waited on while one
            // runs: its end wakes this task, and a deadline passed meanwhile is taken then.
            let due = with(|s| s.busy.is_none().then(|| s.watch.next_due()).flatten()).flatten().map(|due| due.saturating_sub(super::settings::now_ms()).max(1));
            let open = match due {
                Some(ms) => {
                    let timer = cx.background_executor().timer(std::time::Duration::from_millis(ms));
                    match futures::future::select(Box::pin(timer), woken.next()).await {
                        futures::future::Either::Left(_) => true,
                        futures::future::Either::Right((woke, _)) => woke.is_some(),
                    }
                }
                None => woken.next().await.is_some(),
            };
            if !open {
                break;
            }
            cx.update(tick);
        }
    })
    .detach();
}

/// Read finished commands into the model and advance the watch; every window redraws when
/// something changed, so the notices, Settings and the popover follow.
fn tick(cx: &mut App) {
    let Some(shared) = shared() else { return };
    let changed = shared.borrow_mut().tick();
    if changed {
        cx.refresh_windows();
    }
}

impl Server {
    fn note(&mut self, line: String) {
        eprintln!("wheel-gpui: {line}");
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    /// Run `op` on a thread; `target` is the version a rollback runs, for its log line. One
    /// command at a time: every caller checks `busy` first.
    fn run(&mut self, op: Op, target: Option<String>) {
        debug_assert!(self.busy.is_none(), "a server command is already running");
        self.busy = Some(op);
        let (control, done, wake) = (self.control.clone(), self.done.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let result = control.run(op);
            done.lock().unwrap().push(Outcome { op, target, result });
            let _ = wake.unbounded_send(());
        });
    }

    /// Whether Stop, Start and Restart can run now.
    fn ready(&self) -> bool {
        self.control.available() && self.busy.is_none()
    }

    fn set_stopped(&mut self, stopped: bool) {
        self.stopped_by_menu = stopped;
        if let Some(file) = &self.stopped_file {
            if stopped {
                let _ = file.parent().map(std::fs::create_dir_all);
                let _ = std::fs::write(file, "");
            } else {
                let _ = std::fs::remove_file(file);
            }
        }
    }

    fn tick(&mut self) -> bool {
        let now = super::settings::now_ms();
        let done: Vec<Outcome> = self.done.lock().unwrap().drain(..).collect();
        let mut changed = !done.is_empty();
        for outcome in done {
            self.busy = None;
            self.finish(outcome, now);
        }
        if self.busy.is_none() {
            let before = self.watch.clone();
            let (step, log) = self.watch.tick(now, self.connected, self.control.available());
            changed |= self.watch != before;
            for line in log {
                self.note(line);
            }
            match step {
                Some(Step::AskRollbackTarget) => self.run(Op::RollbackTarget, None),
                Some(Step::Rollback(target)) => self.run(Op::Rollback, Some(target)),
                None => {}
            }
        }
        changed
    }

    /// A finished command, read into the model (the Swift completion handlers of DaemonService
    /// and ServerControl).
    fn finish(&mut self, done: Outcome, now: u64) {
        let Outcome { op, target, result } = done;
        let label = self.control.service().map(|s| s.label.clone()).unwrap_or_else(|| "server".into());
        let line = result.as_ref().map(String::as_str).unwrap_or_else(|why| why.as_str()).to_owned();
        match op {
            Op::Start if std::mem::take(&mut self.menu_start) => {
                self.service_status = match &result { Ok(_) => "started from the menu".into(), Err(f) => format!("start failed: {f}") };
                match &result {
                    Ok(_) => {
                        self.set_stopped(false);
                        self.note("server: started from the menu".into());
                    }
                    Err(f) => self.note(format!("server: start failed: {f}")),
                }
                self.problem = result.err();
            }
            Op::Start => {
                self.service_status = match &result {
                    Ok(l) if l.is_empty() => "started".into(),
                    Ok(l) => l.clone(),
                    Err(why) => format!("start failed: {why}"),
                };
                self.note(format!("daemon {label}: {line}"));
                self.problem = result.err();
            }
            Op::Stop => {
                self.stop_asked = false;
                self.service_status = match &result { Ok(_) => "stopped from the menu".into(), Err(f) => format!("stop failed: {f}") };
                match &result {
                    Ok(_) => {
                        self.set_stopped(true);
                        self.note("server: stopped from the menu".into());
                    }
                    Err(f) => self.note(format!("server: stop failed: {f}")),
                }
                self.problem = result.err();
            }
            Op::Restart => {
                self.service_status = match &result { Ok(_) => "restarted from the app".into(), Err(f) => format!("restart failed: {f}") };
                if matches!(&result, Ok(l) if !l.is_empty()) {
                    self.note(format!("daemon {label}: {line}"));
                }
                self.note(match &result { Ok(_) => "server: restarted from the app".into(), Err(f) => format!("server: restart failed: {f}") });
                self.problem = result.err();
            }
            Op::RollbackTarget => {
                self.note(format!("daemon {label}: rollbackable: {}", match &result { Ok(t) => t.clone(), Err(why) => format!("no ({why})") }));
                if let Some(line) = self.watch.target_answered(now, result, self.connected) {
                    self.note(line);
                }
            }
            Op::Rollback => {
                let target = target.unwrap_or_default();
                self.service_status = match &result { Ok(_) => "rolled back from the app".into(), Err(f) => format!("rollback failed: {f}") };
                self.problem = result.as_ref().err().cloned();
                let line = self.watch.rollback_ended(&target, result);
                self.note(line);
            }
        }
    }
}

/// Restart wheeld from what is installed, in one click (ServerControl.swift `restartServer`).
pub fn restart(spaces: usize) {
    with_mut(|s| {
        if s.ready() {
            s.note(format!("server: restart asked for; {spaces} spaces"));
            s.run(Op::Restart, None);
        }
    });
}

/// Stop wheeld and keep it down until Start Server (AppModel.stopServer): nothing is expected
/// back, so its `stopping` is not taken as a restart.
pub fn stop(spaces: usize) {
    with_mut(|s| {
        s.confirming = false;
        if s.ready() {
            s.note(format!("server: stop asked for; {spaces} spaces"));
            s.stop_asked = true;
            s.watch.abandon();
            s.run(Op::Stop, None);
        }
    });
}

/// Start wheeld again after Stop Server (AppModel.startServer).
pub fn start() {
    with_mut(|s| {
        if s.ready() {
            s.note("server: start asked for".into());
            s.menu_start = true;
            s.run(Op::Start, None);
        }
    });
}

/// Ask before stopping: the question shows in the details (AppModel.askToStopServer).
pub fn ask_stop() {
    with_mut(|s| s.confirming = true);
}

pub fn cancel_stop() {
    with_mut(|s| s.confirming = false);
}

pub fn toggle_expanded() {
    with_mut(|s| s.expanded = !s.expanded);
}

/// wheeld sent `stopping`: a stop this app asked for is the stop; anything else is taken as a
/// restart and waited for (AppModel.serverStopping). Every connection reports it; the first counts.
pub fn stopping() {
    with_mut(|s| {
        if s.stop_asked || s.stopped_by_menu {
            if !s.watch.restart_expected {
                s.note("server: wheeld is stopping; a stop was asked for, so it is not expected back".into());
            }
            return;
        }
        if !s.watch.restart_expected {
            let line = s.watch.stopping(super::settings::now_ms());
            s.note(line);
        }
    });
}

/// A connection came up or went down. Up ends whatever the watch waited for.
pub fn link(up: bool) {
    with_mut(|s| {
        s.connected = up;
        if up && s.watch.answered() {
            s.note("server: wheeld answers again".into());
        }
    });
}

pub fn cancel_rollback() {
    with_mut(|s| {
        s.note("server: rollback cancelled".into());
        s.watch.cancel_rollback();
    });
}

pub fn forget_rolled_back() {
    with_mut(|s| s.watch.rolled_back_to = None);
}
