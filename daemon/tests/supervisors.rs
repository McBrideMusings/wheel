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
    ClientMsg, CreatedBy, InboxKind, LoopEvent, LoopEventKind, Node, PaneKind, QueueEntry,
    ServerMsg, SessionEvent, SessionState, StampedEvent, SupervisorInboxItem, SupervisorInfo,
    SupervisorState, WorkerStage,
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
    // A loop that never looks again within a test, so the queues the other tests read stay as the
    // agent made them; the loop tests set their own.
    state.set_loop_timing(wheeld::loops::LoopTiming {
        tick: Duration::from_secs(3600),
        stall: Duration::from_secs(3600),
    });
    // A restored supervisor's agent is started with its tools, so the route is named first.
    state.set_mcp_url(wheeld::mcp::url(addr));
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
            | reply @ ServerMsg::SupervisorEvents { req: r, .. }
            | reply @ ServerMsg::SupervisorInbox { req: r, .. }
            | reply @ ServerMsg::SupervisorPolicy { req: r, .. }
            | reply @ ServerMsg::Inbox { req: r, .. }
            | reply @ ServerMsg::Created { req: r, .. }
            | reply @ ServerMsg::Ok { req: r }
            | reply @ ServerMsg::SessionWaited { req: r, .. }
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

/// The transcript a client gets on attaching to `session`.
async fn attached_events(ws: &mut Ws, session: &str) -> Vec<SessionEvent> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(790),
            session: session.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached {
            req: Some(790),
            events,
            ..
        } = recv(ws).await
        {
            return events.into_iter().map(|e| e.event).collect();
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

    // The planning turn is the daemon's, not the person's: the chat holds none of it.
    assert_eq!(
        attached_events(&mut ws, &s.session).await,
        Vec::<SessionEvent>::new(),
        "the planning turn must not reach the chat"
    );

    // The agent started from the supervisors' own configuration, not the person's.
    let said = session_reply(&mut ws, &s.session, "hello, show setup").await;
    // A message the person typed stays, with its reply.
    let kinds: Vec<_> = attached_events(&mut ws, &s.session)
        .await
        .into_iter()
        .map(|e| match e {
            SessionEvent::User { text, .. } => format!("user:{text}"),
            SessionEvent::Text { .. } => "text".into(),
            _ => "other".into(),
        })
        .collect();
    assert!(
        kinds.contains(&"user:hello, show setup".to_string())
            && kinds.contains(&"text".to_string()),
        "{kinds:?}"
    );
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

// ---- the loop ----

/// A scratch git repo with one commit and beads (prefix `t`) holding an epic whose children carry
/// `titles`. The fake worker acts on its bead by what the title says. Returns the repo, the epic
/// and the children's ids in the order of `titles`.
fn scratch_work(name: &str, titles: &[&str]) -> (Scratch, String, Vec<String>) {
    let repo = Scratch::new(name);
    run(&repo.0, "git", &["init", "-q", "-b", "main"]);
    run(
        &repo.0,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start",
        ],
    );
    run(
        &repo.0,
        "bd",
        &["init", "--prefix", "t", "--non-interactive", "--quiet"],
    );
    let epic = created(&repo.0, &["The epic", "-t", "epic"]);
    let beads = titles
        .iter()
        .map(|title| created(&repo.0, &[title, "--parent", &epic]))
        .collect();
    (repo, epic, beads)
}

fn fast_loop(daemon: &Running, stall_ms: u64) {
    daemon.state.set_loop_timing(wheeld::loops::LoopTiming {
        tick: Duration::from_millis(200),
        stall: Duration::from_millis(stall_ms),
    });
}

async fn events_of(ws: &mut Ws, id: &str) -> Vec<LoopEvent> {
    match ask(
        ws,
        902,
        ClientMsg::SupervisorEvents {
            req: Some(902),
            id: id.into(),
            limit: None,
        },
    )
    .await
    {
        ServerMsg::SupervisorEvents { events, .. } => events,
        other => panic!("expected events, got {other:?}"),
    }
}

async fn inbox_of(ws: &mut Ws, id: &str) -> Vec<SupervisorInboxItem> {
    match ask(
        ws,
        903,
        ClientMsg::SupervisorInbox {
            req: Some(903),
            id: id.into(),
        },
    )
    .await
    {
        ServerMsg::SupervisorInbox { items, .. } => items,
        other => panic!("expected an inbox, got {other:?}"),
    }
}

/// The events of supervisor `id` once `done` holds for them. Polls while the loop keeps adding
/// events and gives up after a minute with none, so a slow machine (every `bd` call takes a
/// second or more) lengthens the wait rather than running it out.
async fn events_until(
    ws: &mut Ws,
    id: &str,
    done: impl Fn(&[LoopEvent]) -> bool,
) -> Vec<LoopEvent> {
    let mut seen = 0;
    let began = tokio::time::Instant::now();
    let mut quiet_since = began;
    loop {
        let events = events_of(ws, id).await;
        if done(&events) {
            return events;
        }
        if events.len() != seen {
            seen = events.len();
            quiet_since = tokio::time::Instant::now();
        }
        if quiet_since.elapsed() > Duration::from_secs(60)
            || began.elapsed() > Duration::from_secs(20 * 60)
        {
            panic!("the loop never got there; its events: {events:#?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn count(events: &[LoopEvent], kind: LoopEventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

/// The kinds of the events about `bead`, in order.
fn kinds_for(events: &[LoopEvent], bead: &str) -> Vec<LoopEventKind> {
    events
        .iter()
        .filter(|e| e.bead.as_deref() == Some(bead))
        .map(|e| e.kind)
        .collect()
}

/// `kinds_for` without the supervisor's agent being woken for an item and leaving it, which
/// follows an inbox item at once.
fn loop_kinds_for(events: &[LoopEvent], bead: &str) -> Vec<LoopEventKind> {
    let mut kinds = kinds_for(events, bead);
    kinds.retain(|k| !matches!(k, LoopEventKind::Woken | LoopEventKind::Unhandled));
    kinds
}

fn bead_status(repo: &std::path::Path, bead: &str) -> String {
    let out: serde_json::Value =
        serde_json::from_str(&run(repo, "bd", &["show", bead, "--json"])).unwrap();
    out[0]["status"].as_str().unwrap().to_string()
}

/// Start a supervisor on `epic` with `workers` slots and wait for its queue.
async fn start_working(
    daemon: &Running,
    ws: &mut Ws,
    repo: &Scratch,
    epic: &str,
    workers: u32,
) -> SupervisorInfo {
    let space = daemon
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    supervisor(
        ws,
        1,
        ClientMsg::SupervisorStart {
            req: Some(1),
            space: Some(space.id),
            goal: Some(epic.into()),
            focus: None,
            workers: Some(workers),
            cap: None,
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_closes_its_bead_frees_its_slot_and_the_next_queued_bead_starts() {
    let (repo, epic, beads) = scratch_work("close", &["close it one", "close it two"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let started = start_working(&daemon, &mut ws, &repo, &epic, 1).await;
    let id = started.supervisor.id.clone();

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 2).await;

    // One slot: the second bead is claimed only after the first has landed, and each bead goes
    // claimed, started, landed.
    for bead in &beads {
        assert_eq!(
            kinds_for(&events, bead),
            [
                LoopEventKind::Claimed,
                LoopEventKind::Started,
                LoopEventKind::Landed
            ],
            "{events:#?}"
        );
        assert_eq!(bead_status(&repo.0, bead), "closed");
        // Its branch is named by the bead and stays after the landing.
        let branches = run(&repo.0, "git", &["branch", "--list", bead]);
        assert!(branches.contains(bead.as_str()), "{branches}");
    }
    let kinds: Vec<_> = events
        .iter()
        .map(|e| (e.kind, e.bead.clone().unwrap()))
        .collect();
    let (first, second) = (kinds[0].1.clone(), kinds[3].1.clone());
    assert_ne!(first, second);
    assert_eq!(
        kinds[..6].iter().map(|k| k.0).collect::<Vec<_>>(),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Landed,
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Landed
        ]
    );
    let done = show(&mut ws, &id).await;
    assert!(done.running.is_empty(), "{done:?}");
    assert_eq!(done.inbox, 0);
    assert_eq!(done.supervisor.queue, Some(vec![]));
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_leaves_its_bead_open_gets_three_continues_then_an_inbox_item() {
    let (repo, epic, beads) = scratch_work("open", &["stubborn"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 2)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Escalated) == 1).await;
    assert_eq!(
        loop_kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Continued,
            LoopEventKind::Continued,
            LoopEventKind::Continued,
            LoopEventKind::Escalated
        ],
        "{events:#?}"
    );
    let items = inbox_of(&mut ws, &id).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].bead, beads[0]);
    assert_eq!(items[0].kind, LoopEventKind::Escalated);
    assert!(items[0].detail.contains("3 continue messages"), "{items:?}");
    let after = show(&mut ws, &id).await;
    assert_eq!((after.inbox, after.running.len()), (1, 0), "{after:?}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_blocks_its_bead_puts_an_item_in_the_inbox_at_once() {
    let (repo, epic, beads) = scratch_work("blocked", &["block me"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 2)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Blocked) == 1).await;
    assert_eq!(
        loop_kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Blocked
        ],
        "{events:#?}"
    );
    let items = inbox_of(&mut ws, &id).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(
        (items[0].bead.as_str(), items[0].kind),
        (beads[0].as_str(), LoopEventKind::Blocked)
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "blocked");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_worker_is_restarted_after_the_stall_timeout() {
    let (repo, epic, beads) = scratch_work("stall", &["stall once close it"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 4000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    assert_eq!(
        loop_kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Stalled,
            LoopEventKind::Restarted,
            LoopEventKind::Started,
            LoopEventKind::Landed
        ],
        "{events:#?}"
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_stays_silent_is_restarted_three_times_then_escalated() {
    let (repo, epic, beads) = scratch_work("stall-always", &["stall always"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 700);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Escalated) == 1).await;
    assert_eq!(count(&events, LoopEventKind::Stalled), 4, "{events:#?}");
    assert_eq!(count(&events, LoopEventKind::Restarted), 3, "{events:#?}");
    assert_eq!(inbox_of(&mut ws, &id).await.len(), 1);
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_bd_read_stops_refills_and_raises_one_event_and_the_queue_is_kept() {
    let (repo, epic, beads) = scratch_work("bd-down", &["close it one"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let started = start_working(&daemon, &mut ws, &repo, &epic, 1).await;
    let id = started.supervisor.id.clone();
    // Held still while the queue arrives, then bd goes away and the loop is let go.
    supervisor(
        &mut ws,
        2,
        ClientMsg::SupervisorPause {
            req: Some(2),
            id: id.clone(),
        },
    )
    .await;
    let planned = with_queue(&mut ws, &id).await;
    assert_eq!(planned.supervisor.queue.as_ref().map(Vec::len), Some(1));
    std::fs::write(
        repo.0.join("wheel.toml"),
        "[beads]\nbd = \"/nonexistent/wheel-test/bd\"\n",
    )
    .unwrap();
    supervisor(
        &mut ws,
        3,
        ClientMsg::SupervisorResume {
            req: Some(3),
            id: id.clone(),
        },
    )
    .await;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::BdFailed) == 1).await;
    // Several ticks pass with bd still down, and still only one event, no claim, and the queue
    // as it was rather than empty.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let events_later = events_of(&mut ws, &id).await;
    assert_eq!(events_later.len(), events.len(), "{events_later:#?}");
    assert_eq!(count(&events_later, LoopEventKind::Claimed), 0);
    assert_eq!(
        show(&mut ws, &id)
            .await
            .supervisor
            .queue
            .as_ref()
            .map(Vec::len),
        Some(1)
    );

    // bd back: the loop carries on with the bead it kept.
    std::fs::remove_file(repo.0.join("wheel.toml")).unwrap();
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed", "{events:#?}");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_supervisor_ends_its_workers_and_releases_their_beads_without_raising_anything()
{
    let (repo, epic, beads) = scratch_work("stop-workers", &["stall always"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Started) == 1).await;
    assert_eq!(show(&mut ws, &id).await.running.len(), 1);

    supervisor(
        &mut ws,
        4,
        ClientMsg::SupervisorStop {
            req: Some(4),
            id: id.clone(),
        },
    )
    .await;
    for _ in 0..100 {
        if show(&mut ws, &id).await.running.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let after = show(&mut ws, &id).await;
    assert!(after.running.is_empty(), "{after:?}");
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Released) == 1).await;
    assert_eq!(
        loop_kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Released
        ],
        "{events:#?}"
    );
    assert!(
        event_for(&events, &beads[0], LoopEventKind::Released)
            .detail
            .contains("supervisor was stopped"),
        "{events:#?}"
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "open");
    assert!(inbox_of(&mut ws, &id).await.is_empty());
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_whose_worktree_cannot_be_made_is_released_with_an_event_naming_why() {
    let (repo, epic, beads) = scratch_work("worktree-fails", &["stall always"]);
    // The bead's branch is checked out in another worktree, so a second one cannot be made.
    run(&repo.0, "git", &["branch", &beads[0]]);
    let elsewhere = repo.0.join("elsewhere");
    run(
        &repo.0,
        "git",
        &[
            "worktree",
            "add",
            &elsewhere.display().to_string(),
            &beads[0],
        ],
    );
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Released) == 1).await;
    assert_eq!(
        kinds_for(&events, &beads[0]),
        [LoopEventKind::Claimed, LoopEventKind::Released],
        "{events:#?}"
    );
    let released = event_for(&events, &beads[0], LoopEventKind::Released);
    assert!(
        released.detail.contains("cannot make the worktree"),
        "{released:?}"
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "open");
    assert!(show(&mut ws, &id).await.supervisor.working.is_empty());
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_releases_the_beads_whose_workers_died_with_it() {
    let (repo, epic, beads) = scratch_work("restart-releases", &["stall always"]);
    let home = Scratch::new("restart-releases-home");
    let state_file = home.0.join("spaces.json");
    let first = start(Some(state_file.clone())).await;
    fast_loop(&first, 60_000);
    let space = first
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    let mut ws = connect(&first.url).await;
    let id = start_goal(&mut ws, 1, &space.id, &epic).await.supervisor.id;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Started) == 1).await;
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");

    // What a killed daemon leaves on disk: the layout as saved while the worker ran.
    let killed = fresh_dir("restart-releases-killed").join("spaces.json");
    for _ in 0..50 {
        let saved = std::fs::read_to_string(&state_file).unwrap_or_default();
        if saved.contains("\"working\"") {
            std::fs::write(&killed, saved).unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(killed.exists(), "the layout never listed the worker's bead");
    drop(ws);
    first.daemon.abort();

    let second = start(Some(killed)).await;
    let mut ws = connect(&second.url).await;
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Released) == 1).await;
    let released = event_for(&events, &beads[0], LoopEventKind::Released);
    assert!(released.detail.contains("wheeld restarted"), "{released:?}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "open");
    assert!(show(&mut ws, &id).await.supervisor.working.is_empty());
    second.daemon.abort();
}

// ---- landing ----

fn main_commit(repo: &std::path::Path) -> String {
    run(repo, "git", &["rev-parse", "main"]).trim().to_string()
}

fn set_check(repo: &std::path::Path, check: &str) {
    std::fs::write(
        repo.join("wheel.toml"),
        format!("[project]\ncheck = {check:?}\n"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_bead_lands_on_main_after_the_check_and_its_commit_is_in_the_landed_event() {
    let (repo, epic, beads) = scratch_work("land", &["close write a.txt"]);
    // The check sees the merged tree, not the repo the person works in.
    set_check(&repo.0, "test -f a.txt && echo checked");
    let before = main_commit(&repo.0);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    let after = main_commit(&repo.0);
    assert_ne!(before, after);
    let landed = events
        .iter()
        .find(|e| e.kind == LoopEventKind::Landed)
        .unwrap();
    assert!(
        landed
            .detail
            .contains(&format!("landed on main at {after};")),
        "{landed:?}"
    );
    assert!(landed.detail.contains("passed"), "{landed:?}");
    assert_eq!(
        std::fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        format!("{}\n", beads[0])
    );
    assert_eq!(count(&events, LoopEventKind::SentBack), 0);
    // The worktree is gone and the slot is free.
    let mut done = show(&mut ws, &id).await;
    for _ in 0..100 {
        if done.running.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        done = show(&mut ws, &id).await;
    }
    assert!(done.running.is_empty(), "{done:?}");
    assert!(
        !run(&repo.0, "git", &["worktree", "list"]).contains(&format!("/{}", beads[0])),
        "the bead's worktree is still there"
    );
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conflicting_branch_is_sent_back_once_and_the_second_conflict_reaches_the_inbox() {
    // Each waits for the other's worktree, so both branch from the same main whatever the speed.
    let (repo, epic, beads) = scratch_work(
        "conflict",
        &[
            "close write a.txt with sibling",
            "close write a.txt with sibling",
        ],
    );
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 2)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Escalated) == 1).await;
    assert_eq!(count(&events, LoopEventKind::Landed), 1, "{events:#?}");
    let sent = events
        .iter()
        .find(|e| e.kind == LoopEventKind::SentBack)
        .unwrap();
    assert_eq!(count(&events, LoopEventKind::SentBack), 1, "{events:#?}");
    assert!(sent.detail.contains("CONFLICT"), "{sent:?}");
    let loser = sent.bead.clone().unwrap();
    let winner = beads.iter().find(|b| **b != loser).unwrap();
    // The supervisor's agent is woken with the item at once, so its events may follow.
    let mut kinds = kinds_for(&events, &loser);
    kinds.retain(|k| !matches!(k, LoopEventKind::Woken | LoopEventKind::Unhandled));
    assert_eq!(
        kinds[kinds.len() - 2..],
        [LoopEventKind::SentBack, LoopEventKind::Escalated],
        "{events:#?}"
    );
    let items = inbox_of(&mut ws, &id).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(
        (items[0].bead.as_str(), items[0].kind),
        (loser.as_str(), LoopEventKind::Escalated)
    );
    assert!(items[0].detail.contains("CONFLICT"), "{items:?}");
    // Main holds the winner's file and nothing of the loser's.
    assert_eq!(
        std::fs::read_to_string(repo.0.join("a.txt")).unwrap(),
        format!("{winner}\n")
    );
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_red_check_is_sent_back_once_then_escalated_and_main_does_not_move() {
    let (repo, epic, beads) = scratch_work("red", &["close write a.txt"]);
    set_check(&repo.0, "echo boom; exit 1");
    let before = main_commit(&repo.0);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Escalated) == 1).await;
    // The supervisor's agent is woken with the item at once, so more events may follow.
    assert_eq!(
        kinds_for(&events, &beads[0])[..4],
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::SentBack,
            LoopEventKind::Escalated
        ],
        "{events:#?}"
    );
    let sent = events
        .iter()
        .find(|e| e.kind == LoopEventKind::SentBack)
        .unwrap();
    assert!(
        sent.detail.contains("exited 1") && sent.detail.contains("boom"),
        "{sent:?}"
    );
    let items = inbox_of(&mut ws, &id).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].detail.contains("boom"), "{items:?}");
    assert_eq!(main_commit(&repo.0), before);
    assert!(!repo.0.join("a.txt").exists());
    daemon.daemon.abort();
}

// ---- the permission policy ----

/// The lead's inbox, open items first.
async fn lead_inbox(ws: &mut Ws) -> Vec<wheel_proto::InboxItem> {
    match ask(ws, 904, ClientMsg::InboxLs { req: Some(904) }).await {
        ServerMsg::Inbox { items, .. } => items,
        other => panic!("expected the inbox, got {other:?}"),
    }
}

/// The lead's permission items for `session`, all states.
async fn permission_items(ws: &mut Ws, session: &str) -> Vec<wheel_proto::InboxItem> {
    lead_inbox(ws)
        .await
        .into_iter()
        .filter(|i| i.kind == wheel_proto::InboxKind::Permission && i.source.session == session)
        .collect()
}

/// The lead's permission item for `session` once there is one.
async fn permission_item(ws: &mut Ws, session: &str) -> wheel_proto::InboxItem {
    for _ in 0..600 {
        if let Some(item) = permission_items(ws, session).await.into_iter().next() {
            return item;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the lead's inbox never got a permission item for {session}");
}

fn event_for<'a>(events: &'a [LoopEvent], bead: &str, kind: LoopEventKind) -> &'a LoopEvent {
    events
        .iter()
        .find(|e| e.bead.as_deref() == Some(bead) && e.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} event for {bead}: {events:#?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fake_workers_edit_inside_its_worktree_is_approved_by_policy() {
    let (repo, epic, beads) = scratch_work("policy-edit", &["ask edit inside close"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    // The worker closed its bead after its request was answered, so the request did not wait.
    assert_eq!(
        kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::ApprovedByPolicy,
            LoopEventKind::Landed
        ],
        "{events:#?}"
    );
    let approved = event_for(&events, &beads[0], LoopEventKind::ApprovedByPolicy);
    assert_eq!(
        approved.worker.as_deref(),
        Some(format!("worker-{}", beads[0]).as_str())
    );
    assert!(approved.detail.contains("src/a.rs"), "{approved:?}");
    assert!(
        approved.detail.contains("worktrees/") && approved.detail.contains(&beads[0]),
        "{approved:?}"
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    assert!(lead_inbox(&mut ws).await.is_empty());
    assert!(inbox_of(&mut ws, &id).await.is_empty());
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_outside_the_policy_stays_pending_and_waits_in_the_leads_inbox_for_a_person() {
    let (repo, epic, beads) = scratch_work(
        "policy-pending",
        &[
            "ask edit outside close",
            "ask run other close",
            "ask run declared close",
        ],
    );
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 3)
        .await
        .supervisor
        .id;

    // No declared commands here, so a command outside bd and git is not the project's.
    let events = events_until(&mut ws, &id, |e| {
        count(e, LoopEventKind::PendingForPerson) == 3
    })
    .await;
    assert_eq!(
        count(&events, LoopEventKind::ApprovedByPolicy),
        0,
        "{events:#?}"
    );
    let reasons: Vec<_> = beads
        .iter()
        .map(|b| {
            event_for(&events, b, LoopEventKind::PendingForPerson)
                .detail
                .clone()
        })
        .collect();
    assert!(
        reasons[0].contains("/tmp/outside-the-worktree.txt is not inside the worktree"),
        "{reasons:?}"
    );
    assert!(
        reasons[1].contains("rm -rf build") && reasons[1].contains("not one of the 0 declared"),
        "{reasons:?}"
    );
    assert!(reasons[2].contains("cargo test -p wheeld"), "{reasons:?}");

    // Each waits in the lead's inbox as a new permission item from its worker, and nobody has
    // closed any bead.
    let mut perms = Vec::new();
    for bead in &beads {
        let session = format!("worker-{bead}");
        let item = permission_item(&mut ws, &session).await;
        assert_eq!(item.state, wheel_proto::InboxState::New, "{item:?}");
        assert_eq!(item.source.pane, show(&mut ws, &id).await.pane);
        assert!(item.detail.is_some() && item.perm.is_some(), "{item:?}");
        assert_eq!(bead_status(&repo.0, bead), "in_progress");
        perms.push(item);
    }
    assert!(
        inbox_of(&mut ws, &id).await.is_empty(),
        "the supervisor's inbox is for its workers' escalations"
    );

    // A person answers one; its worker goes on and closes its bead, and only its item closes.
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(77),
            id: perms[1].perm.clone().unwrap(),
            choice: "allow".into(),
        },
    )
    .await;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    assert_eq!(bead_status(&repo.0, &beads[1]), "closed");
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");
    for _ in 0..100 {
        if permission_items(&mut ws, &format!("worker-{}", beads[1]))
            .await
            .iter()
            .all(|i| i.state == wheel_proto::InboxState::Closed)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let after = lead_inbox(&mut ws).await;
    let open: Vec<_> = after
        .iter()
        .filter(|i| i.state != wheel_proto::InboxState::Closed)
        .map(|i| i.source.session.clone())
        .collect();
    assert_eq!(open.len(), 2, "{after:?}");
    assert!(!open.contains(&format!("worker-{}", beads[1])), "{after:?}");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_waiting_on_a_persons_answer_is_not_restarted_as_stalled() {
    let (repo, epic, beads) = scratch_work("policy-no-stall", &["ask run other close"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 2500);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| {
        count(e, LoopEventKind::PendingForPerson) == 1
    })
    .await;
    assert_eq!(count(&events, LoopEventKind::Stalled), 0, "{events:#?}");
    let item = permission_item(&mut ws, &format!("worker-{}", beads[0])).await;

    // Wait past the stall timeout several times over: the request stays, the worker runs on.
    tokio::time::sleep(Duration::from_millis(6000)).await;
    let events = events_of(&mut ws, &id).await;
    assert_eq!(count(&events, LoopEventKind::Stalled), 0, "{events:#?}");
    assert_eq!(count(&events, LoopEventKind::Restarted), 0, "{events:#?}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");

    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(78),
            id: item.perm.clone().unwrap(),
            choice: "allow".into(),
        },
    )
    .await;
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    assert_eq!(count(&events, LoopEventKind::Stalled), 0, "{events:#?}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_projects_declared_commands_and_bd_on_the_workers_own_bead_are_approved() {
    let (repo, epic, beads) = scratch_work(
        "policy-declared",
        &[
            "ask run declared close",
            "ask bd own close",
            "ask run other close",
        ],
    );
    std::fs::write(
        repo.0.join("wheel.toml"),
        "[policy]\ncommands = [\"cargo test\", \"cargo clippy\"]\n",
    )
    .unwrap();
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 3)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| {
        count(e, LoopEventKind::ApprovedByPolicy) == 2
            && count(e, LoopEventKind::PendingForPerson) == 1
    })
    .await;
    let declared = event_for(&events, &beads[0], LoopEventKind::ApprovedByPolicy);
    assert!(
        declared.detail.contains("declared command \"cargo test\""),
        "{declared:?}"
    );
    let own = event_for(&events, &beads[1], LoopEventKind::ApprovedByPolicy);
    assert!(
        own.detail.contains(&format!("bd show {}", beads[1])),
        "{own:?}"
    );
    let other = event_for(&events, &beads[2], LoopEventKind::PendingForPerson);
    assert!(
        other.detail.contains("not one of the 2 declared commands"),
        "{other:?}"
    );

    // `wheel supervisor policy` prints what is in force.
    let policy = match ask(
        &mut ws,
        905,
        ClientMsg::SupervisorPolicy {
            req: Some(905),
            id: id.clone(),
        },
    )
    .await
    {
        ServerMsg::SupervisorPolicy { policy, .. } => policy,
        other => panic!("expected a policy, got {other:?}"),
    };
    assert_eq!(policy.supervisor, id);
    assert_eq!(policy.commands, ["cargo test", "cargo clippy"]);
    assert!(policy.commands_file.ends_with("wheel.toml"), "{policy:?}");
    let rules = policy.rules.join("\n");
    for word in [
        "worktree",
        "declared",
        "bd show, update, close",
        "git status",
        "waits for the person",
    ] {
        assert!(rules.contains(word), "{word} missing from {rules}");
    }
    assert!(matches!(
        ask(
            &mut ws,
            906,
            ClientMsg::SupervisorPolicy {
                req: Some(906),
                id: "nope".into()
            }
        )
        .await,
        ServerMsg::Error { .. }
    ));
    daemon.daemon.abort();
}

/// A raw JSON-RPC call on the daemon's MCP server; the reply's `result` or `error`.
async fn mcp_rpc(
    addr: std::net::SocketAddr,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
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
    let (_, body) = raw.split_once("\r\n\r\n").expect("an HTTP reply");
    serde_json::from_str(body.trim()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_call_from_the_lead_or_a_supervisor_cannot_answer_a_request() {
    let (repo, epic, beads) = scratch_work("policy-mcp", &["ask run other close"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;
    let session = format!("worker-{}", beads[0]);
    let item = permission_item(&mut ws, &session).await;
    let perm = item.perm.clone().unwrap();

    // The daemon's MCP server is the only tool surface an agent has, and the lead is the only
    // agent given it. None of its tools has anything to do with a permission request.
    let listed = mcp_rpc(daemon.addr, "tools/list", serde_json::json!({})).await;
    let tools: Vec<(String, String)> = listed["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_string(),
                t["description"].as_str().unwrap_or_default().to_lowercase(),
            )
        })
        .collect();
    assert!(tools.len() > 20, "{tools:?}");
    for (name, description) in &tools {
        for word in ["perm", "approve", "allow", "answer", "grant"] {
            assert!(
                !name.contains(word),
                "tool {name} could answer a permission request"
            );
        }
        assert!(
            !description.contains("answer a permission")
                && !description.contains("approve a permission")
                && !description.contains("allow a permission"),
            "tool {name}: {description}"
        );
    }

    // Asking anyway, under the names a tool for it would have, and with the request's id as every
    // argument it could take, answers nothing.
    for name in [
        "perm_answer",
        "answer_permission",
        "permission_answer",
        "approve_permission",
        "pane_permission",
    ] {
        let reply = mcp_rpc(
            daemon.addr,
            "tools/call",
            serde_json::json!({ "name": name, "arguments": { "id": perm, "perm": perm, "choice": "allow" } }),
        )
        .await;
        let failed = reply["error"].is_object() || reply["result"]["isError"] == true;
        assert!(failed, "{name} was not refused: {reply}");
    }

    // The request still waits: its item is open, the worker has not moved, and the bead is not
    // closed. Only a person's client answering it lets the worker go on.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let items = permission_items(&mut ws, &session).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, wheel_proto::InboxState::New, "{items:?}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "in_progress");
    let events = events_of(&mut ws, &id).await;
    assert_eq!(count(&events, LoopEventKind::Landed), 0, "{events:#?}");
    assert_eq!(
        count(&events, LoopEventKind::ApprovedByPolicy),
        0,
        "{events:#?}"
    );

    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(78),
            id: perm,
            choice: "deny".into(),
        },
    )
    .await;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_beads_item_wakes_the_agent_which_forwards_it_to_the_lead() {
    let (repo, epic, beads) = scratch_work("forward", &["block me [forward]"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let started = start_working(&daemon, &mut ws, &repo, &epic, 1).await;
    let id = started.supervisor.id.clone();

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Forwarded) == 1).await;
    assert_eq!(
        kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Blocked,
            LoopEventKind::Woken,
            LoopEventKind::Forwarded
        ],
        "{events:#?}"
    );
    let woken = events
        .iter()
        .find(|e| e.kind == LoopEventKind::Woken)
        .unwrap();
    assert!(woken.detail.contains("item s1"), "{}", woken.detail);
    // The worker's last turn went to the agent with the item.
    assert!(woken.detail.contains("last turn"), "{}", woken.detail);

    let escalations = lead_inbox(&mut ws).await;
    assert_eq!(escalations.len(), 1, "{escalations:?}");
    let item = &escalations[0];
    assert_eq!(item.kind, InboxKind::Escalation);
    assert_eq!(item.supervisor.as_deref(), Some(id.as_str()));
    assert_eq!(item.source.pane, started.pane);
    assert_eq!(item.source.session, started.supervisor.session);
    assert_eq!(
        item.detail.as_deref(),
        Some("the worker needs a person's choice")
    );
    assert!(inbox_of(&mut ws, &id).await.is_empty());
    assert_eq!(bead_status(&repo.0, &beads[0]), "blocked");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_answers_a_blocked_worker_which_gets_the_message_and_the_item_closes() {
    let (repo, epic, beads) = scratch_work("answer", &["block me [answer]"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    // The worker closes its bead only on receiving the answer, so the landing shows it arrived.
    assert_eq!(
        kinds_for(&events, &beads[0]),
        [
            LoopEventKind::Claimed,
            LoopEventKind::Started,
            LoopEventKind::Blocked,
            LoopEventKind::Woken,
            LoopEventKind::Answered,
            LoopEventKind::Started,
            LoopEventKind::Landed
        ],
        "{events:#?}"
    );
    let answered = events
        .iter()
        .find(|e| e.kind == LoopEventKind::Answered)
        .unwrap();
    assert!(
        answered.detail.contains("use the blue one"),
        "{}",
        answered.detail
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    assert!(inbox_of(&mut ws, &id).await.is_empty());
    assert!(lead_inbox(&mut ws).await.is_empty());
    let done = show(&mut ws, &id).await;
    assert!(done.running.is_empty(), "{done:?}");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_the_agent_drops_leaves_the_inbox_and_one_it_leaves_stays() {
    let (repo, epic, beads) = scratch_work("drop", &["block me [drop]", "block me too"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 2)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| {
        count(e, LoopEventKind::Closed) == 1 && count(e, LoopEventKind::Unhandled) == 1
    })
    .await;
    // The queue ran the second bead first (the fake agent queues last first); name them by title.
    let dropped = beads
        .iter()
        .find(|b| kinds_for(&events, b).contains(&LoopEventKind::Closed))
        .unwrap();
    let left = beads.iter().find(|b| *b != dropped).unwrap();
    assert!(!kinds_for(&events, left).contains(&LoopEventKind::Closed));
    let items = inbox_of(&mut ws, &id).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].bead, *left);
    // The item the agent left is not woken for again.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let later = events_of(&mut ws, &id).await;
    assert_eq!(count(&later, LoopEventKind::Woken), 2, "{later:#?}");
    assert!(lead_inbox(&mut ws).await.is_empty());
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landed_beads_follow_up_lines_become_inbox_items_the_agent_files_or_drops() {
    let (repo, epic, beads) = scratch_work("followups", &["close with follow-ups"]);
    std::fs::write(
        repo.0.join(".beads/labels.toml"),
        "[labels.daemon]\nabout = \"d\"\n",
    )
    .unwrap();
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| {
        count(e, LoopEventKind::Filed) == 1 && count(e, LoopEventKind::Closed) == 1
    })
    .await;
    let queued: Vec<&LoopEvent> = events
        .iter()
        .filter(|e| e.kind == LoopEventKind::FollowUp)
        .collect();
    assert_eq!(queued.len(), 2, "{events:#?}");
    assert!(
        queued[0].detail.contains("tidy a at a.rs:1"),
        "{}",
        queued[0].detail
    );
    assert!(
        queued[1].detail.contains("tidy b at b.rs:2"),
        "{}",
        queued[1].detail
    );
    assert!(inbox_of(&mut ws, &id).await.is_empty());

    let filed = events
        .iter()
        .find(|e| e.kind == LoopEventKind::Filed)
        .unwrap();
    let all: serde_json::Value = serde_json::from_str(&run(
        &repo.0,
        "bd",
        &["list", "--json", "--all", "--limit=0"],
    ))
    .unwrap();
    let new: Vec<&serde_json::Value> = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["title"] == "filed follow-up")
        .collect();
    assert_eq!(new.len(), 1, "{all}");
    let new_id = new[0]["id"].as_str().unwrap();
    assert!(filed.detail.contains(new_id), "{}", filed.detail);
    let shown: serde_json::Value =
        serde_json::from_str(&run(&repo.0, "bd", &["show", new_id, "--json"])).unwrap();
    let edge = shown[0]["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == beads[0].as_str())
        .unwrap_or_else(|| panic!("no edge to {}: {shown}", beads[0]));
    assert_eq!(edge["dependency_type"], "discovered-from", "{edge}");
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_scope_wakes_the_agent_which_queues_the_beads_that_became_ready() {
    let (repo, epic, beads) = scratch_work("dry", &["close first", "close second"]);
    run(&repo.0, "bd", &["dep", "add", &beads[1], &beads[0]]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 2).await;
    // The second bead was not ready when the agent first planned: only the dry wake queued it.
    assert_eq!(count(&events, LoopEventKind::Dry), 1, "{events:#?}");
    let dry_at = events
        .iter()
        .position(|e| e.kind == LoopEventKind::Dry)
        .unwrap();
    let second_claimed = events
        .iter()
        .position(|e| e.kind == LoopEventKind::Claimed && e.bead.as_deref() == Some(&beads[1]))
        .unwrap();
    assert!(dry_at < second_claimed, "{events:#?}");
    assert_eq!(bead_status(&repo.0, &beads[1]), "closed");
    daemon.daemon.abort();
}

/// The kind of pane `pane` is in the daemon's layout.
fn pane_kind(daemon: &Running, pane: &str) -> Option<PaneKind> {
    daemon
        .state
        .spaces()
        .spaces
        .iter()
        .find_map(|s| wheeld::spaces::find_pane(&s.root, pane).cloned())
}

/// The supervisor once its first worker is running.
async fn with_worker(ws: &mut Ws, id: &str) -> SupervisorInfo {
    for _ in 0..600 {
        let info = show(ws, id).await;
        if !info.running.is_empty() {
            return info;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("supervisor {id} never ran a worker");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_in_its_turn_reports_the_working_stage() {
    let (repo, epic, _) = scratch_work("stage", &["stall always"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Started) == 1).await;
    for _ in 0..50 {
        let running = show(&mut ws, &id).await.running;
        if running.first().map(|w| w.stage) == Some(WorkerStage::Working) {
            daemon.daemon.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the worker never reported the working stage");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_pushed_to_a_pane_stays_running_when_that_pane_closes() {
    let (repo, epic, _) = scratch_work("push", &["stall always"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let mut ws = connect(&daemon.url).await;
    let id = start_working(&daemon, &mut ws, &repo, &epic, 1)
        .await
        .supervisor
        .id;
    let running = with_worker(&mut ws, &id).await;
    let session = running.running[0].session.clone();
    assert_eq!(running.running[0].pane, None);

    let refusal = refused(
        &mut ws,
        10,
        ClientMsg::WorkerPush {
            req: Some(10),
            id: id.clone(),
            session: "worker-nobody".into(),
        },
    )
    .await;
    assert!(refusal.contains("not a running worker"), "{refusal}");

    let pane = match ask(
        &mut ws,
        11,
        ClientMsg::WorkerPush {
            req: Some(11),
            id: id.clone(),
            session: session.clone(),
        },
    )
    .await
    {
        ServerMsg::Created { id, .. } => id,
        other => panic!("expected a pane, got {other:?}"),
    };
    assert!(matches!(
        pane_kind(&daemon, &pane),
        Some(PaneKind::Worker { session: s, .. }) if s == session
    ));
    let shown = show(&mut ws, &id).await;
    assert_eq!(shown.running[0].pane.as_deref(), Some(pane.as_str()));

    let twice = refused(
        &mut ws,
        12,
        ClientMsg::WorkerPush {
            req: Some(12),
            id: id.clone(),
            session: session.clone(),
        },
    )
    .await;
    assert!(twice.contains("already in pane"), "{twice}");

    // The pane takes the worker's messages by its own id, as an agent pane does.
    match ask(
        &mut ws,
        14,
        ClientMsg::SessionWait {
            req: Some(14),
            session: pane.clone(),
            timeout_secs: 0.1,
        },
    )
    .await
    {
        ServerMsg::SessionWaited {
            session: named,
            pane: shown,
            ..
        } => assert_eq!((named, shown), (session.clone(), Some(pane.clone()))),
        other => panic!("expected the worker's session, got {other:?}"),
    }

    match ask(
        &mut ws,
        13,
        ClientMsg::PaneClose {
            req: Some(13),
            pane: pane.clone(),
        },
    )
    .await
    {
        ServerMsg::Ok { .. } => {}
        other => panic!("expected ok, got {other:?}"),
    }
    assert!(pane_kind(&daemon, &pane).is_none());
    let after = show(&mut ws, &id).await;
    assert_eq!(after.running.len(), 1, "{after:?}");
    assert_eq!(after.running[0].session, session);
    assert_eq!(after.running[0].pane, None);
    match ask(
        &mut ws,
        15,
        ClientMsg::SessionWait {
            req: Some(15),
            session: session.clone(),
            timeout_secs: 0.1,
        },
    )
    .await
    {
        ServerMsg::SessionWaited { session: named, .. } => assert_eq!(named, session),
        other => panic!("the worker's session is gone: {other:?}"),
    }
    daemon.daemon.abort();
}

/// A usage source the test sets, counting how often it is read.
struct StubUsage {
    next: std::sync::Mutex<Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure>>,
    reads: std::sync::atomic::AtomicUsize,
}

impl StubUsage {
    fn new(session: f64, weekly: f64) -> Arc<StubUsage> {
        Arc::new(StubUsage {
            next: std::sync::Mutex::new(Ok(raw_usage(session, weekly))),
            reads: Default::default(),
        })
    }

    fn set(&self, next: Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure>) {
        *self.next.lock().unwrap() = next;
    }

    fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl wheeld::usage::UsageSource for StubUsage {
    fn read(&self) -> Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.next.lock().unwrap().clone()
    }
}

fn raw_usage(session: f64, weekly: f64) -> wheeld::usage::RawUsage {
    wheeld::usage::RawUsage {
        session_pct: session,
        session_resets_at: Some("2026-10-17T20:00:00Z".into()),
        weekly_pct: weekly,
        weekly_resets_at: Some("2026-10-20T00:00:00Z".into()),
    }
}

/// A daemon reading usage from `usage` every 200 ms.
fn usage_daemon_timing(daemon: &Running, usage: &Arc<StubUsage>, stale_after: Duration) {
    daemon.state.set_usage_source(usage.clone());
    daemon.state.set_usage_timing(wheeld::usage::UsageTiming {
        every: Duration::from_millis(200),
        stale_after,
    });
}

/// Start a supervisor on `epic` with one slot and the usage cap `cap`.
async fn start_capped(
    daemon: &Running,
    ws: &mut Ws,
    repo: &Scratch,
    epic: &str,
    cap: u32,
) -> String {
    let space = daemon
        .state
        .space_create(repo.0.display().to_string(), None, CreatedBy::Person)
        .unwrap();
    supervisor(
        ws,
        1,
        ClientMsg::SupervisorStart {
            req: Some(1),
            space: Some(space.id),
            goal: Some(epic.into()),
            focus: None,
            workers: Some(1),
            cap: Some(cap),
        },
    )
    .await
    .supervisor
    .id
}

fn event_of(events: &[LoopEvent], kind: LoopEventKind) -> &LoopEvent {
    events
        .iter()
        .find(|e| e.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} event in {events:#?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_supervisor_over_its_cap_holds_and_starts_nothing_and_resumes_under_it() {
    let (repo, epic, beads) = scratch_work("capped", &["close it"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let usage = StubUsage::new(91.0, 12.0);
    usage_daemon_timing(&daemon, &usage, Duration::from_secs(3600));
    let mut ws = connect(&daemon.url).await;
    let id = start_capped(&daemon, &mut ws, &repo, &epic, 80).await;
    with_queue(&mut ws, &id).await;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Held) == 1).await;
    let held = event_of(&events, LoopEventKind::Held);
    assert!(
        held.detail
            .contains("5-hour usage 91% is at the cap of 80%"),
        "{held:?}"
    );
    // Several reads and ticks later it has started nothing and held once.
    let before = usage.reads();
    while usage.reads() < before + 3 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let events = events_of(&mut ws, &id).await;
    assert_eq!(count(&events, LoopEventKind::Claimed), 0, "{events:#?}");
    assert_eq!(count(&events, LoopEventKind::Held), 1, "{events:#?}");
    let info = show(&mut ws, &id).await;
    assert_eq!(info.supervisor.state, SupervisorState::Held);
    assert_eq!(info.supervisor.queue.as_ref().map(Vec::len), Some(1));
    assert_eq!(bead_status(&repo.0, &beads[0]), "open");

    usage.set(Ok(raw_usage(40.0, 12.0)));
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;
    let resumed = event_of(&events, LoopEventKind::Resumed);
    assert!(
        resumed.detail.contains("under the cap of 80%"),
        "{resumed:?}"
    );
    assert_eq!(bead_status(&repo.0, &beads[0]), "closed");
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_weekly_reading_at_the_cap_holds_too() {
    let (repo, epic, _) = scratch_work("weekly", &["wait for usage"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let usage = StubUsage::new(5.0, 80.0);
    usage_daemon_timing(&daemon, &usage, Duration::from_secs(3600));
    let mut ws = connect(&daemon.url).await;
    let id = start_capped(&daemon, &mut ws, &repo, &epic, 80).await;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Held) == 1).await;
    let held = event_of(&events, LoopEventKind::Held);
    assert!(
        held.detail
            .contains("weekly usage 80% is at the cap of 80%"),
        "{held:?}"
    );
    assert_eq!(count(&events, LoopEventKind::Claimed), 0);
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_reading_holds_and_the_event_says_why() {
    let (repo, epic, _) = scratch_work("missing", &["wait for usage"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let usage = StubUsage::new(0.0, 0.0);
    usage.set(Err(wheeld::usage::UsageFailure::NotLoggedIn(
        "lead is not logged in".into(),
    )));
    usage_daemon_timing(&daemon, &usage, Duration::from_secs(3600));
    let mut ws = connect(&daemon.url).await;
    let id = start_capped(&daemon, &mut ws, &repo, &epic, 80).await;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Held) == 1).await;
    let held = event_of(&events, LoopEventKind::Held);
    assert_eq!(
        held.detail, "usage is unknown: lead is not logged in",
        "{held:?}"
    );
    assert_eq!(count(&events, LoopEventKind::Claimed), 0);
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_read_keeps_the_last_reading_as_stale_and_holds() {
    let (repo, epic, _) = scratch_work("stale-fail", &["close it"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let usage = StubUsage::new(10.0, 10.0);
    usage_daemon_timing(&daemon, &usage, Duration::from_secs(3600));
    let mut ws = connect(&daemon.url).await;
    let id = start_capped(&daemon, &mut ws, &repo, &epic, 80).await;
    events_until(&mut ws, &id, |e| count(e, LoopEventKind::Landed) == 1).await;

    usage.set(Err(wheeld::usage::UsageFailure::Failed {
        status: Some(500),
        why: "HTTP 500".into(),
        retry_after: None,
    }));
    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Held) == 1).await;
    let held = event_of(&events, LoopEventKind::Held);
    assert!(
        held.detail.contains("usage is stale (HTTP 500)")
            && held.detail.contains("last reading: 5-hour 10%"),
        "{held:?}"
    );
    daemon.daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reading_older_than_the_stale_age_holds_and_the_event_says_how_old() {
    let (repo, epic, _) = scratch_work("stale-age", &["only bead"]);
    let daemon = start(None).await;
    fast_loop(&daemon, 60_000);
    let usage = StubUsage::new(10.0, 10.0);
    usage_daemon_timing(&daemon, &usage, Duration::from_secs(3600));
    // One read, then none for longer than the stale age: the interval is long, the age short.
    daemon.state.set_usage_timing(wheeld::usage::UsageTiming {
        every: Duration::from_secs(3600),
        stale_after: Duration::from_millis(1500),
    });
    let mut ws = connect(&daemon.url).await;
    let id = start_capped(&daemon, &mut ws, &repo, &epic, 80).await;

    let events = events_until(&mut ws, &id, |e| count(e, LoopEventKind::Held) == 1).await;
    let held = event_of(&events, LoopEventKind::Held);
    assert!(
        held.detail.contains("usage is stale (reading is ") && held.detail.contains(" s old)"),
        "{held:?}"
    );
    assert_eq!(usage.reads(), 1);
    daemon.daemon.abort();
}
