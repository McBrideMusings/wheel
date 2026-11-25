//! The servers this app knows (ADR-0057): this Mac's wheeld first, then each enabled paired one,
//! each read through a subscription of its own on the app's one connection to it: whether it is
//! up, its host's name, colour and platform, its sessions, and the space on its orchestrator's
//! folder. The session menu groups by it, New Session on a server asks that server, and Add
//! Server… pairs through it. A window asks it for work through `ask`, since a window's own
//! handlers hold no `App`.

use crate::daemon::{self, Endpoint, Event, Outbox};
use crate::{header, layout, servers, view};
use anyhow::{Result, anyhow};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{App, Task};
use std::{cell::RefCell, time::Duration};
use wheel_proto::{ClientMsg, ServerMsg};

/// How long a New Session asked on another server waits for its `created`.
const NEW_SESSION_WAIT: Duration = Duration::from_secs(15);
/// The `settings_ls` the directory sends on each link's Up: its replies carry the orchestrator's folder.
const SETTINGS_REQ: u64 = 1;

/// One server as the menus draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// `Endpoint::server()`: `local`, or a paired server's id.
    pub key: String,
    pub name: String,
    /// The `host.color` it reported, a macOS system colour's name.
    pub color: Option<String>,
    /// `<os>_<arch>`, such as `darwin_arm64`.
    pub platform: String,
    pub up: bool,
    /// (id, name) of each session, in wheeld's order.
    pub sessions: Vec<(String, String)>,
}

impl Group {
    /// The SF Symbol that marks the server: a Mac or a rack.
    pub fn icon(&self) -> &'static str {
        if self.platform.starts_with("linux") { "server.rack" } else { "desktopcomputer" }
    }
}

/// What a window asks the directory to do.
pub enum Ask {
    /// Show `session` of `server` in `window`, switching the window's connection when it is another server's.
    Switch { window: String, server: String, session: String },
    /// Make a session on `server` and show it in `window`.
    NewSession { window: String, server: String },
    /// Pair with the server `wheel remote pair`'s `code` names; `window` hears how it went.
    Pair { window: String, code: String },
}

struct Entry {
    group: Group,
    endpoint: Endpoint,
    out: Outbox,
    /// The space on the orchestrator's folder and the session holding it, once both are read.
    orchestrator_dir: Option<String>,
    spaces: Vec<layout::Space>,
    layout_sessions: Vec<layout::Session>,
    /// `session_new` requests in flight: req, the window that asked.
    pending: Vec<(u64, String)>,
    next_req: u64,
    _pump: Task<()>,
}

thread_local! {
    static ENTRIES: RefCell<Vec<Entry>> = RefCell::default();
    static ASKS: RefCell<Option<mpsc::UnboundedSender<Ask>>> = RefCell::default();
}

fn log(line: String) {
    eprintln!("wheel-gpui: directory: {line}");
}

/// Start the directory on this Mac's wheeld and every enabled paired one, and the task that runs
/// what windows ask.
pub fn start(cx: &mut App, local: Endpoint) {
    let (asks, mut asked) = mpsc::unbounded();
    ASKS.with(|a| *a.borrow_mut() = Some(asks));
    add(cx, local, "This Mac".into());
    for server in servers::load(&servers::default_file()).into_iter().filter(|s| s.enabled) {
        match servers::endpoint(&server) {
            Ok(endpoint) => add(cx, endpoint, server.name.clone()),
            Err(e) => log(format!("server {} ({}): no endpoint: {e:#}", server.id, server.name)),
        }
    }
    cx.spawn(async move |cx| {
        while let Some(ask) = asked.next().await {
            cx.update(|cx| run(cx, ask));
        }
    })
    .detach();
}

/// Hand `ask` to the directory; it runs once the window's handler returns.
pub fn ask(ask: Ask) {
    ASKS.with(|a| {
        if let Some(asks) = a.borrow().as_ref() {
            let _ = asks.unbounded_send(ask);
        }
    });
}

/// Every server, this Mac's first.
pub fn groups() -> Vec<Group> {
    ENTRIES.with(|e| e.borrow().iter().map(|e| e.group.clone()).collect())
}

/// How many servers the app knows; the menus group by server only when it is more than one.
pub fn count() -> usize {
    ENTRIES.with(|e| e.borrow().len())
}

/// The server, other than `except`, with a space open on its orchestrator's folder, and the
/// session holding that space: where Orchestrator mode finds the one orchestrator (ADR-0054).
pub fn orchestrator_elsewhere(except: &str) -> Option<(String, String)> {
    ENTRIES.with(|e| {
        e.borrow().iter().filter(|e| e.group.key != except && e.group.up).find_map(|e| {
            let dir = e.orchestrator_dir.as_deref()?;
            let space = e.spaces.iter().find(|s| view::same_dir(&s.cwd, dir))?;
            let session = e.layout_sessions.iter().find(|s| s.spaces.contains(&space.id))?;
            Some((e.group.key.clone(), session.id.clone()))
        })
    })
}

/// Subscribe to `endpoint`'s wheeld and list it; a server already listed keeps its entry.
fn add(cx: &mut App, endpoint: Endpoint, name: String) {
    let key = endpoint.server().to_owned();
    if ENTRIES.with(|e| e.borrow().iter().any(|e| e.group.key == key)) {
        log(format!("server {key} is listed already; its connection stays"));
        return;
    }
    let (out, mut incoming) = crate::connection(&endpoint).subscribe();
    let held = key.clone();
    let pump = cx.spawn(async move |cx| {
        while let Some(event) = incoming.next().await {
            cx.update(|cx| on_event(cx, &held, event));
        }
    });
    log(format!("server {key} ({name}): connecting to {}", endpoint.url()));
    let group = Group { key, name, color: None, platform: String::new(), up: false, sessions: vec![] };
    ENTRIES.with(|e| e.borrow_mut().push(Entry { group, endpoint, out, orchestrator_dir: None, spaces: vec![], layout_sessions: vec![], pending: vec![], next_req: 100, _pump: pump }));
    crate::refresh_windows(cx);
}

/// One event from `key`'s wheeld. Windows redraw only when what the menus draw changed.
fn on_event(cx: &mut App, key: &str, event: Event) {
    let mut made: Option<(String, String)> = None;
    let mut failed: Option<(String, String)> = None;
    let changed = ENTRIES.with(|e| {
        let mut entries = e.borrow_mut();
        let Some(entry) = entries.iter_mut().find(|e| e.group.key == key) else { return false };
        let before = (entry.group.clone(), orchestrator_of(entry));
        match event {
            Event::Up(endpoint) => {
                log(format!("server {key}: up"));
                entry.endpoint = endpoint;
                entry.group.up = true;
                let _ = entry.out.send(daemon::frame(&ClientMsg::SettingsLs { req: Some(SETTINGS_REQ), space: None }));
            }
            Event::Down(reason) => {
                if entry.group.up {
                    log(format!("server {key}: down: {reason}"));
                }
                entry.group.up = false;
            }
            Event::Incompatible { server, client } => {
                log(format!("server {key}: at compatibility level {server}, this app at {client}"));
                entry.group.up = false;
            }
            Event::Refused { op } => log(format!("server {key}: refused `{op}`")),
            Event::Message(message) => {
                if let ServerMsg::Status { host, .. } = message.as_ref() {
                    if !host.name.is_empty() {
                        entry.group.name = host.name.clone();
                    }
                    entry.group.color = Some(host.color.clone()).filter(|c| !c.is_empty());
                    entry.group.platform = host.platform.clone();
                }
                if let Some(layout) = layout::parse_layout(&message) {
                    entry.group.sessions = layout.sessions.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
                    entry.layout_sessions = layout.sessions;
                    entry.spaces = layout.spaces;
                }
                match message.as_ref() {
                    ServerMsg::Settings { space: None, settings, .. } => {
                        if let Some(s) = settings.iter().find(|s| s.key == "orchestrator.dir") {
                            entry.orchestrator_dir = Some(view::orchestrator_dir(s));
                        }
                    }
                    ServerMsg::Created { req, id, .. } => {
                        if let Some(i) = entry.pending.iter().position(|(r, _)| r == req) {
                            let (_, window) = entry.pending.remove(i);
                            log(format!("server {key}: made session {id} for window {window}"));
                            made = Some((window, id.clone()));
                        }
                    }
                    ServerMsg::Error { req: Some(req), message, .. } => {
                        if let Some(i) = entry.pending.iter().position(|(r, _)| r == req) {
                            let (_, window) = entry.pending.remove(i);
                            log(format!("server {key}: New Session for window {window} failed: {message}"));
                            failed = Some((window, message.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        before != (entry.group.clone(), orchestrator_of(entry))
    });
    if let Some((window, session)) = made {
        switch(cx, &window, key, &session);
    }
    if let Some((window, message)) = failed {
        tell(cx, &window, format!("New Session on {} failed: {message}", name_of(key)));
    }
    if changed {
        // A server's orchestrator space may have appeared: a window in Orchestrator mode with none moves to it.
        for (_, handle) in crate::window_handles() {
            let _ = handle.update(cx, |client, _, _| client.follow_orchestrator());
        }
        crate::refresh_windows(cx);
    }
}

fn orchestrator_of(entry: &Entry) -> Option<String> {
    let dir = entry.orchestrator_dir.as_deref()?;
    entry.spaces.iter().find(|s| view::same_dir(&s.cwd, dir)).map(|s| s.id.clone())
}

fn name_of(key: &str) -> String {
    ENTRIES.with(|e| e.borrow().iter().find(|e| e.group.key == key).map_or_else(|| key.to_owned(), |e| e.group.name.clone()))
}

fn run(cx: &mut App, ask: Ask) {
    match ask {
        Ask::Switch { window, server, session } => switch(cx, &window, &server, &session),
        Ask::NewSession { window, server } => new_session(cx, window, server),
        Ask::Pair { window, code } => {
            let task = pair(cx, code, device_name());
            cx.spawn(async move |cx| {
                let result = task.await.map(|s| s.name).map_err(|e| format!("{e:#}"));
                cx.update(|cx| {
                    if let Some(handle) = crate::window_handle(&window) {
                        let _ = handle.update(cx, |client, _, cx| {
                            client.paired(result);
                            cx.notify();
                        });
                    }
                });
            })
            .detach();
        }
    }
}

/// Show `session` of `server` in `window`: the window's own server selects it, another server's
/// moves the window's connection there first.
fn switch(cx: &mut App, window: &str, server: &str, session: &str) {
    let Some(handle) = crate::window_handle(window) else {
        log(format!("window {window} closed before session {session} of server {server} could show"));
        return;
    };
    let own = handle.read(cx).map(|c| c.server() == server).unwrap_or(false);
    if own {
        let _ = handle.update(cx, |client, _, cx| {
            client.select_session(session);
            cx.notify();
        });
        return;
    }
    let Some(endpoint) = ENTRIES.with(|e| e.borrow().iter().find(|e| e.group.key == server).map(|e| e.endpoint.clone())) else {
        log(format!("window {window}: no server {server} to switch to"));
        return;
    };
    crate::switch_server(cx, handle, endpoint, session.to_owned());
}

/// New Session on `server` for `window`: the name is the first free one on that server.
fn new_session(cx: &mut App, window: String, server: String) {
    let sent = ENTRIES.with(|e| {
        let mut entries = e.borrow_mut();
        let entry = entries.iter_mut().find(|e| e.group.key == server)?;
        if !entry.group.up {
            return None;
        }
        let taken: Vec<&str> = entry.group.sessions.iter().map(|(_, n)| n.as_str()).collect();
        let name = header::session_name(&taken);
        let req = entry.next_req;
        entry.next_req += 1;
        entry.out.send(daemon::frame(&ClientMsg::SessionNew { req: Some(req), name: name.clone() })).ok()?;
        entry.pending.push((req, window.clone()));
        Some((req, name))
    });
    let Some((req, name)) = sent else {
        log(format!("window {window}: New Session on {server} not sent: the server is not up"));
        return;
    };
    log(format!("server {server}: asked for session {name} (req {req}) for window {window}"));
    cx.spawn(async move |cx| {
        cx.background_executor().timer(NEW_SESSION_WAIT).await;
        let dropped = ENTRIES.with(|e| {
            let mut entries = e.borrow_mut();
            let entry = entries.iter_mut().find(|e| e.group.key == server)?;
            let i = entry.pending.iter().position(|(r, _)| *r == req)?;
            Some(entry.pending.remove(i).1)
        });
        if let Some(window) = dropped {
            log(format!("server {server}: no answer to New Session (req {req}) for window {window} in {}s", NEW_SESSION_WAIT.as_secs()));
            cx.update(|cx| tell(cx, &window, format!("{} did not answer New Session", name_of(&server))));
        }
    })
    .detach();
}

/// Pair with the server `code` names as `device` and list it. A server listed already keeps its
/// connection. The code is never logged.
pub fn pair(cx: &mut App, code: String, device: String) -> Task<Result<servers::Server>> {
    let job = cx.background_executor().spawn(async move { servers::pair(&servers::default_file(), &code, &device) });
    cx.spawn(async move |cx| {
        let server = job.await.inspect_err(|e| log(format!("pairing failed: {e:#}")))?;
        log(format!("paired with server {} ({})", server.id, server.name));
        let endpoint = servers::endpoint(&server).map_err(|e| anyhow!("paired with {}, but its address is unusable: {e:#}", server.name))?;
        let name = server.name.clone();
        cx.update(|cx| add(cx, endpoint, name));
        Ok(server)
    })
}

/// An error a window should see as a notice.
fn tell(cx: &mut App, window: &str, text: String) {
    if let Some(handle) = crate::window_handle(window) {
        let _ = handle.update(cx, |client, _, cx| {
            client.directory_error(text);
            cx.notify();
        });
    }
}

/// The name this Mac pairs as: its host name without `.local`.
fn device_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is writable and its length is passed.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let name = if ok { String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(buf.len())]).into_owned() } else { String::new() };
    let name = name.trim_end_matches(".local").to_owned();
    if name.is_empty() { "Wheel app".to_owned() } else { name }
}
