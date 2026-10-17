//! Supervisors over the socket, the lead's MCP tools and a restart, driven by the scripted fake
//! agent: a start wakes the agent with the scope's ready beads and stores the queue it returns,
//! pause, resume and stop change the state everywhere, and a supervisor comes back from the saved
//! layout as it was.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    ClientMsg, CreatedBy, Node, PaneKind, QueueEntry, ServerMsg, SessionEvent, SessionState,
    StampedEvent, SupervisorInfo, SupervisorState,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "supervisor-test-token";

struct Running {
    state: Arc<wheeld::AppState>,
    url: String,
    addr: std::net::SocketAddr,
    daemon: common::Daemon,
}

/// A daemon whose agents run the fake agent, keeping its layout in `state_file` when given.
async fn start(state_file: Option<std::path::PathBuf>) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let instance = state_file
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| fresh_dir("instance"));
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance,
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    state.set_agent_command(env!("CARGO_BIN_EXE_wheel-fake-agent").into());
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    Running {
        url: format!("ws://{addr}/ws?token={TOKEN}"),
        addr,
        daemon: common::Daemon::new(serving, state.clone()),
        state,
    }
}

/// An empty directory of its own under the temp directory.
fn fresh_dir(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "wheel-supervisors-{name}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A scratch directory, removed with the value.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        Scratch(fresh_dir(name))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(dir: &std::path::Path, program: &str, args: &[&str]) -> String {
    let out = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("{program} {args:?}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What `bd create --json` printed, as the new bead's id.
fn created(dir: &std::path::Path, args: &[&str]) -> String {
    let mut all = vec!["create"];
    all.extend_from_slice(args);
    all.push("--json");
    let out: serde_json::Value = serde_json::from_str(&run(dir, "bd", &all)).unwrap();
    out["id"]
        .as_str()
        .or_else(|| out[0]["id"].as_str())
        .unwrap()
        .to_string()
}

/// The ids `bd ready` lists for `parent`, in its order.
fn ready_ids(dir: &std::path::Path, parent: &str) -> Vec<String> {
    let out = run(
        dir,
        "bd",
        &["ready", "--json", "--limit=0", "--parent", parent],
    );
    serde_json::from_str::<Vec<serde_json::Value>>(&out)
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect()
}

/// A scratch git repo with beads (prefix `t`) holding an epic with three children, the second
/// blocked by the first, and one bead outside the epic. Returns the repo, the epic and the beads
/// of the epic that are ready, in `bd`'s order.
fn scratch_epic(name: &str) -> (Scratch, String, Vec<String>) {
    let repo = Scratch::new(name);
    run(&repo.0, "git", &["init", "-q"]);
    run(
        &repo.0,
        "bd",
        &["init", "--prefix", "t", "--non-interactive", "--quiet"],
    );
    let epic = created(&repo.0, &["The epic", "-t", "epic"]);
    let first = created(&repo.0, &["First child", "--parent", &epic]);
    let second = created(&repo.0, &["Second child", "--parent", &epic]);
    created(&repo.0, &["Third child", "--parent", &epic]);
    created(&repo.0, &["Outside the epic"]);
    run(&repo.0, "bd", &["dep", "add", &second, &first]);
    let ready = ready_ids(&repo.0, &epic);
    assert_eq!(ready.len(), 2, "the blocked child is not ready: {ready:?}");
    (repo, epic, ready)
}

async fn connect(url: &str) -> Ws {
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

async fn send(ws: &mut Ws, msg: ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Send a request carrying `req` and return the reply that carries it back.
async fn ask(ws: &mut Ws, req: u64, msg: ClientMsg) -> ServerMsg {
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            reply @ ServerMsg::Supervisor { req: r, .. }
            | reply @ ServerMsg::Supervisors { req: r, .. }
            | reply @ ServerMsg::Error { req: Some(r), .. }
            | reply @ ServerMsg::Status { req: Some(r), .. }
                if r == req =>
            {
                return reply
            }
            _ => {}
        }
    }
}

async fn supervisor(ws: &mut Ws, req: u64, msg: ClientMsg) -> SupervisorInfo {
    match ask(ws, req, msg).await {
        ServerMsg::Supervisor { supervisor, .. } => supervisor,
        other => panic!("expected a supervisor, got {other:?}"),
    }
}

async fn refused(ws: &mut Ws, req: u64, msg: ClientMsg) -> String {
    match ask(ws, req, msg).await {
        ServerMsg::Error { message, .. } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn show(ws: &mut Ws, id: &str) -> SupervisorInfo {
    supervisor(
        ws,
        900,
        ClientMsg::SupervisorShow {
            req: Some(900),
            id: id.into(),
        },
    )
    .await
}

async fn list(ws: &mut Ws) -> Vec<SupervisorInfo> {
    match ask(ws, 901, ClientMsg::SupervisorLs { req: Some(901) }).await {
        ServerMsg::Supervisors { supervisors, .. } => supervisors,
        other => panic!("expected supervisors, got {other:?}"),
    }
}

/// The supervisor once its agent has answered with a queue.
async fn with_queue(ws: &mut Ws, id: &str) -> SupervisorInfo {
    for _ in 0..600 {
        let info = show(ws, id).await;
        assert_eq!(info.supervisor.plan_error, None, "{info:?}");
        if info.supervisor.queue.is_some() && info.agent == Some(SessionState::Ready) {
            return info;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("supervisor {id} never got a queue");
}

fn queue_of(ready: &[String]) -> Vec<QueueEntry> {
    let models = ["sonnet", "haiku", "opus"];
    ready
        .iter()
        .rev()
        .enumerate()
        .map(|(i, bead)| QueueEntry {
            bead: bead.clone(),
            model: models[i % 3].into(),
        })
        .collect()
}

async fn start_goal(ws: &mut Ws, req: u64, space: &str, goal: &str) -> SupervisorInfo {
    supervisor(
        ws,
        req,
        ClientMsg::SupervisorStart {
            req: Some(req),
            space: Some(space.into()),
            goal: Some(goal.into()),
            focus: None,
            workers: None,
            cap: None,
        },
    )
    .await
}

/// A tool call on the daemon's MCP server: its text and whether it is an error.
async fn call_tool(
    addr: std::net::SocketAddr,
    tool: &str,
    args: serde_json::Value,
) -> (String, bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    })
    .to_string();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\n\
Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").expect("an HTTP reply");
    assert!(head.starts_with("HTTP/1.1 200"), "{raw}");
    let reply: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_string(),
        result["isError"] == true,
    )
}

/// The supervisor with `id` in a layout broadcast, if the layout holds it.
fn in_layout(spaces: &[wheel_proto::SpaceInfo], id: &str) -> Option<wheel_proto::Supervisor> {
    fn find(node: &Node, id: &str) -> Option<wheel_proto::Supervisor> {
        match node {
            Node::Pane {
                kind: PaneKind::Supervisor { supervisor, .. },
                ..
            } if supervisor.id == id => Some((**supervisor).clone()),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => find(a, id).or_else(|| find(b, id)),
        }
    }
    spaces.iter().find_map(|s| find(&s.root, id))
}

/// Read broadcasts on `ws` until the layout shows supervisor `id` in `state`.
async fn layout_shows(ws: &mut Ws, id: &str, state: SupervisorState) {
    loop {
        if let ServerMsg::Spaces { spaces, .. } = recv(ws).await {
            if in_layout(&spaces, id).is_some_and(|s| s.state == state) {
                return;
            }
        }
    }
}

/// What a message to a supervisor's agent drew back, until the turn ended.
async fn session_reply(ws: &mut Ws, session: &str, text: &str) -> String {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(800),
            session: session.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached { req: Some(800), .. } = recv(ws).await {
            break;
        }
    }
    send(
        ws,
        ClientMsg::SessionSend {
            req: Some(801),
            session: session.into(),
            text: text.into(),
        },
    )
    .await;
    let mut reply = String::new();
    loop {
        if let ServerMsg::Session {
            event: StampedEvent { event, .. },
            ..
        } = recv(ws).await
        {
            match event {
                SessionEvent::Text { text } => reply.push_str(&text),
                SessionEvent::TurnEnd { .. } => return reply,
                SessionEvent::Error { message, .. } => panic!("agent error: {message}"),
                _ => {}
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn starting_a_supervisor_on_an_epic_wakes_the_agent_with_the_ready_beads_and_stores_its_queue(
) {
    let (repo, epic, ready) = scratch_epic("plan");
    let daemon = start(None).await;
    let space = daemon
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    let mut ws = connect(&daemon.url).await;

    let started = supervisor(
        &mut ws,
        1,
        ClientMsg::SupervisorStart {
            req: Some(1),
            space: Some(space.id.clone()),
            goal: Some(epic.clone()),
            focus: None,
            workers: Some(3),
            cap: Some(50),
        },
    )
    .await;
    let s = &started.supervisor;
    assert!(
        s.id.starts_with(&format!("{epic}-u")),
        "goal plus a short id: {}",
        s.id
    );
    assert_eq!(
        (
            s.goal.as_deref(),
            s.focus.as_deref(),
            s.workers,
            s.cap,
            s.state
        ),
        (
            Some(epic.as_str()),
            None,
            3,
            Some(50),
            SupervisorState::Running
        )
    );
    assert_eq!(started.space, space.id);

    // The fake agent queues the beads it was shown last first, a model each, so the queue holds
    // exactly the epic's ready beads (not the blocked child, not the bead outside the epic), in
    // the agent's order rather than `bd`'s.
    let planned = with_queue(&mut ws, &s.id).await;
    assert_eq!(
        planned.supervisor.queue,
        Some(queue_of(&ready)),
        "ready beads: {ready:?}"
    );

    // The agent started from the supervisors' own configuration, not the person's.
    let said = session_reply(&mut ws, &s.session, "hello, show setup").await;
    assert!(
        said.contains("sources=user tools=Read,Grep,Glob config_dir=")
            && said.contains("/supervisor-home\n"),
        "{said:?}"
    );

    // A second goal is a second supervisor in the same space.
    let second = start_goal(&mut ws, 2, &space.id, &epic).await;
    assert_ne!(second.supervisor.id, s.id);
    assert_eq!(list(&mut ws).await.len(), 2);
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_resume_and_stop_change_the_state_everywhere() {
    let (repo, epic, _) = scratch_epic("states");
    let daemon = start(None).await;
    let space = daemon
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    let mut ws = connect(&daemon.url).await;
    let mut watcher = connect(&daemon.url).await;
    let started = start_goal(&mut ws, 1, &space.id, &epic).await;
    let id = started.supervisor.id.clone();
    with_queue(&mut ws, &id).await;

    let step = |req: u64, op: fn(Option<u64>, String) -> ClientMsg| {
        let id = id.clone();
        move || op(Some(req), id.clone())
    };
    let pause = step(10, |req, id| ClientMsg::SupervisorPause { req, id });
    let resume = step(11, |req, id| ClientMsg::SupervisorResume { req, id });
    let stop = step(12, |req, id| ClientMsg::SupervisorStop { req, id });

    // Paused: the reply, every other connection's layout, show, list, status and the lead's tool.
    assert_eq!(
        supervisor(&mut ws, 10, pause()).await.supervisor.state,
        SupervisorState::Paused
    );
    layout_shows(&mut watcher, &id, SupervisorState::Paused).await;
    assert_eq!(
        show(&mut ws, &id).await.supervisor.state,
        SupervisorState::Paused
    );
    assert_eq!(
        list(&mut ws).await[0].supervisor.state,
        SupervisorState::Paused
    );
    let ServerMsg::Status { spaces, .. } =
        ask(&mut ws, 902, ClientMsg::Status { req: Some(902) }).await
    else {
        panic!("status")
    };
    assert_eq!(
        in_layout(&spaces, &id).map(|s| s.state),
        Some(SupervisorState::Paused)
    );
    let (text, error) = call_tool(
        daemon.addr,
        "show_supervisor",
        serde_json::json!({ "id": id }),
    )
    .await;
    assert!(!error, "{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["state"],
        "paused"
    );
    assert!(refused(&mut ws, 10, pause()).await.contains("is paused"));

    // Resumed.
    assert_eq!(
        supervisor(&mut ws, 11, resume()).await.supervisor.state,
        SupervisorState::Running
    );
    layout_shows(&mut watcher, &id, SupervisorState::Running).await;
    assert!(refused(&mut ws, 11, resume()).await.contains("is running"));

    // Workers and cap are checked, and change what show prints.
    let bad = |req: u64, workers: u32| ClientMsg::SupervisorWorkers {
        req: Some(req),
        id: id.clone(),
        workers,
    };
    assert!(refused(&mut ws, 20, bad(20, 0)).await.contains("1 or more"));
    let capped = |req: u64, cap: Option<u32>| ClientMsg::SupervisorCap {
        req: Some(req),
        id: id.clone(),
        cap,
    };
    assert!(refused(&mut ws, 21, capped(21, Some(101)))
        .await
        .contains("1 to 100"));
    assert_eq!(
        supervisor(&mut ws, 22, bad(22, 5)).await.supervisor.workers,
        5
    );
    assert_eq!(
        supervisor(&mut ws, 23, capped(23, Some(80)))
            .await
            .supervisor
            .cap,
        Some(80)
    );
    let shown = show(&mut ws, &id).await.supervisor;
    assert_eq!((shown.workers, shown.cap), (5, Some(80)));
    assert_eq!(
        supervisor(&mut ws, 24, capped(24, None))
            .await
            .supervisor
            .cap,
        None
    );

    // Stopped: the agent is gone, the pane keeps the supervisor, and nothing changes it any more.
    let stopped = supervisor(&mut ws, 12, stop()).await;
    assert_eq!(
        (stopped.supervisor.state, stopped.agent),
        (SupervisorState::Stopped, None)
    );
    layout_shows(&mut watcher, &id, SupervisorState::Stopped).await;
    assert_eq!(show(&mut ws, &id).await.agent, None);
    for (req, msg) in [
        (10, pause()),
        (11, resume()),
        (12, stop()),
        (30, bad(30, 2)),
        (31, capped(31, Some(10))),
    ] {
        let said = refused(&mut ws, req, msg).await;
        assert!(said.contains("stopped"), "{said}");
    }
    assert!(refused(
        &mut ws,
        32,
        ClientMsg::SupervisorShow {
            req: Some(32),
            id: "nope-u1".into()
        }
    )
    .await
    .contains("no supervisor"));

    // The lead's tools make and steer a supervisor of its own, and print what the socket does.
    let (text, error) = call_tool(
        daemon.addr,
        "start_supervisor",
        serde_json::json!({ "space": space.id, "goal": epic, "workers": 1 }),
    )
    .await;
    assert!(!error, "{text}");
    let by_tool: SupervisorInfo = serde_json::from_str(&text).unwrap();
    let lead_id = by_tool.supervisor.id.clone();
    with_queue(&mut ws, &lead_id).await;
    let (text, error) = call_tool(
        daemon.addr,
        "pause_supervisor",
        serde_json::json!({ "id": lead_id }),
    )
    .await;
    assert!(!error, "{text}");
    layout_shows(&mut watcher, &lead_id, SupervisorState::Paused).await;
    let over_socket = show(&mut ws, &lead_id).await;
    let (text, _) = call_tool(
        daemon.addr,
        "show_supervisor",
        serde_json::json!({ "id": lead_id }),
    )
    .await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        serde_json::to_value(&over_socket).unwrap(),
        "the tool and the socket print the same JSON"
    );
    let (text, error) = call_tool(
        daemon.addr,
        "stop_supervisor",
        serde_json::json!({ "id": lead_id }),
    )
    .await;
    assert!(!error, "{text}");
    assert_eq!(
        show(&mut ws, &lead_id).await.supervisor.state,
        SupervisorState::Stopped
    );
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_supervisor_survives_a_daemon_restart_in_its_saved_state() {
    let (repo, epic, _) = scratch_epic("restart");
    let home = Scratch::new("restart-home");
    let state_file = home.0.join("spaces.json");
    let first = start(Some(state_file.clone())).await;
    let space = first
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    let mut ws = connect(&first.url).await;
    let paused_id = start_goal(&mut ws, 1, &space.id, &epic).await.supervisor.id;
    let stopped_id = start_goal(&mut ws, 2, &space.id, &epic).await.supervisor.id;
    with_queue(&mut ws, &paused_id).await;
    with_queue(&mut ws, &stopped_id).await;
    supervisor(
        &mut ws,
        3,
        ClientMsg::SupervisorPause {
            req: Some(3),
            id: paused_id.clone(),
        },
    )
    .await;
    supervisor(
        &mut ws,
        4,
        ClientMsg::SupervisorWorkers {
            req: Some(4),
            id: paused_id.clone(),
            workers: 4,
        },
    )
    .await;
    supervisor(
        &mut ws,
        5,
        ClientMsg::SupervisorCap {
            req: Some(5),
            id: paused_id.clone(),
            cap: Some(40),
        },
    )
    .await;
    supervisor(
        &mut ws,
        6,
        ClientMsg::SupervisorStop {
            req: Some(6),
            id: stopped_id.clone(),
        },
    )
    .await;
    let before = list(&mut ws).await;
    drop(ws);
    first.daemon.abort();

    let second = start(Some(state_file)).await;
    let mut ws = connect(&second.url).await;
    let after = list(&mut ws).await;
    assert_eq!(after.len(), 2, "{after:?}");
    for (was, now) in before.iter().zip(&after) {
        // The agent's conversation id is the agent's own: this fake agent cannot load one.
        let (mut was, mut now) = (was.clone(), now.clone());
        was.supervisor.acp_session = None;
        now.supervisor.acp_session = None;
        was.agent = None;
        let now_agent = now.agent.take();
        assert_eq!(was, now);
        // A paused supervisor's agent runs again under its saved name; a stopped one has none.
        assert_eq!(
            now_agent.is_some(),
            now.supervisor.state != SupervisorState::Stopped,
            "{now:?}"
        );
    }
    let paused = show(&mut ws, &paused_id).await.supervisor;
    assert_eq!(
        (paused.state, paused.workers, paused.cap),
        (SupervisorState::Paused, 4, Some(40))
    );
    assert_eq!(
        show(&mut ws, &stopped_id).await.supervisor.state,
        SupervisorState::Stopped
    );
    second.daemon.abort();
}
