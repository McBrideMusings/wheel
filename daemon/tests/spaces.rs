//! Spaces over the socket: every change reaches every client as a `spaces` event, a terminal pane
//! runs a shell in its space's directory, closing the last pane leaves one empty pane, and a restarted
//! daemon gets the same layout back with fresh shells.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    ArtifactScope, Axis, ClientMsg, Dir, NewPaneKind, Node, PaneKind, ServerMsg, SettingOrigin,
    SpaceInfo, TimerState, WatchInfo, WatchRole,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "spaces-test-token";

/// Start a daemon; the handle stops it serving when aborted. Its instance directory is the one
/// holding the state file, as it is in production, so `machine` artifacts land beside the layout.
async fn start_daemon(state_file: Option<PathBuf>) -> (String, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let instance = instance_dir(state_file.as_deref());
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance,
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        common::Daemon::new(serving, state),
    )
}

/// The daemon instance directory for a run keeping its layout at `state_file`: the directory
/// holding it, as in production, so the run's `machine` artifacts and its panes' data directories
/// land beside the layout. A run saving no layout gets a directory of its own, so two daemons in
/// one test binary never make and remove each other's pane directories.
fn instance_dir(state_file: Option<&std::path::Path>) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    state_file
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::env::temp_dir().join(format!("wheel-instance-{}-{n}", std::process::id()))
        })
}

async fn connect(url: &str) -> Ws {
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

/// How long a frame may take to arrive before the daemon counts as hung. Generous on purpose:
/// every test in this binary runs its own daemon in parallel with the rest, so a tight ceiling
/// here fails whichever test the machine happened to starve rather than finding a defect.
const REPLIES: Duration = Duration::from_secs(60);

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(REPLIES, ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Send a request and return its reply, skipping broadcasts.
async fn call(ws: &mut Ws, msg: ClientMsg) -> ServerMsg {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
    loop {
        match recv(ws).await {
            // The broadcasts: every connection gets them whether or not it asked for anything, so
            // a reply can arrive behind one. A test that wants one reads it with `recv` itself.
            ServerMsg::Spaces { .. }
            | ServerMsg::PaneReload { .. }
            | ServerMsg::PaneUrl { .. }
            | ServerMsg::ArtifactGone { .. }
            | ServerMsg::PermRequested { .. }
            | ServerMsg::PermResolved { .. }
            | ServerMsg::StatusChanged { .. }
            | ServerMsg::Settings { req: None, .. } => {}
            reply => return reply,
        }
    }
}

async fn created(ws: &mut Ws, msg: ClientMsg) -> String {
    match call(ws, msg).await {
        ServerMsg::Created { id, .. } => id,
        other => panic!("expected created, got {other:?}"),
    }
}

async fn ok(ws: &mut Ws, msg: ClientMsg) {
    match call(ws, msg).await {
        ServerMsg::Ok { .. } => {}
        other => panic!("expected ok, got {other:?}"),
    }
}

async fn layout(ws: &mut Ws) -> (Vec<SpaceInfo>, Option<String>, Vec<wheel_proto::TermInfo>) {
    match call(ws, ClientMsg::Status { req: Some(99) }).await {
        ServerMsg::Status {
            spaces,
            active,
            terms,
            ..
        } => (spaces, active, terms),
        other => panic!("expected status, got {other:?}"),
    }
}

fn new_space(cwd: &str) -> ClientMsg {
    ClientMsg::SpaceCreate {
        req: Some(1),
        cwd: cwd.into(),
        name: None,
        background: false,
    }
}

fn terminal(pane: &str) -> ClientMsg {
    ClientMsg::PaneSetKind {
        req: Some(2),
        pane: pane.into(),
        kind: NewPaneKind::Terminal,
        artifact: None,
        scope: None,
        url: None,
    }
}

/// What `pane` shows, or None when the tree has no such pane.
fn find_kind<'a>(node: &'a Node, pane: &str) -> Option<&'a PaneKind> {
    match node {
        Node::Pane { id, kind, .. } if id == pane => Some(kind),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_kind(a, pane).or_else(|| find_kind(b, pane)),
    }
}

fn term_of(space: &SpaceInfo, pane: &str) -> Option<String> {
    fn find(node: &Node, pane: &str) -> Option<Option<String>> {
        match node {
            Node::Pane { id, kind, .. } if id == pane => Some(match kind {
                PaneKind::Terminal { term, .. } => Some(term.clone()),
                PaneKind::Empty { .. }
                | PaneKind::Web { .. }
                | PaneKind::Browser { .. }
                | PaneKind::Agent { .. }
                | PaneKind::Unknown(_) => None,
            }),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => find(a, pane).or_else(|| find(b, pane)),
        }
    }
    find(&space.root, pane).expect("pane in space")
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_reach_every_client_and_the_last_pane_leaves_an_empty_one() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    let mut watcher = connect(&url).await;

    let space = created(&mut ws, new_space("/tmp")).await;
    let ServerMsg::Spaces { spaces, active } = recv(&mut watcher).await else {
        panic!("watcher gets the new layout")
    };
    assert_eq!(active.as_deref(), Some(space.as_str()));
    assert_eq!(
        (spaces[0].name.as_str(), spaces[0].cwd.as_str()),
        ("tmp", "/tmp")
    );
    let Node::Pane {
        id: first,
        kind: PaneKind::Empty { .. },
        ..
    } = spaces[0].root.clone()
    else {
        panic!("a new space holds one empty pane")
    };

    let second = created(
        &mut ws,
        ClientMsg::PaneSplit {
            req: Some(3),
            pane: first.clone(),
            dir: Dir::Down,
            background: false,
        },
    )
    .await;
    ok(&mut ws, terminal(&second)).await;
    let (spaces, _, terms) = layout(&mut ws).await;
    let Node::Split {
        axis: Axis::Vertical,
        a,
        b,
        ..
    } = &spaces[0].root
    else {
        panic!("split down stacks the panes")
    };
    assert!(matches!(&**a, Node::Pane { id, .. } if *id == first));
    assert!(matches!(&**b, Node::Pane { id, .. } if *id == second));
    assert_eq!(spaces[0].focus, second, "the new pane takes focus");
    let term = term_of(&spaces[0], &second).expect("terminal pane");
    let info = terms
        .iter()
        .find(|t| t.id == term)
        .expect("terminal is listed");
    assert!(info.alive);
    assert_eq!(info.cwd, "/tmp");

    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(4),
            pane: second.clone(),
        },
    )
    .await;
    let (spaces, _, terms) = layout(&mut ws).await;
    assert!(
        !terms.iter().any(|t| t.id == term),
        "closing the pane killed its terminal"
    );
    assert!(matches!(&spaces[0].root, Node::Pane { id, .. } if *id == first));
    assert_eq!(spaces[0].focus, first);

    let other = created(&mut ws, new_space("/")).await;
    ok(
        &mut ws,
        ClientMsg::Focus {
            req: Some(5),
            space: Some(space.clone()),
            pane: None,
        },
    )
    .await;
    ok(&mut ws, terminal(&first)).await;
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(6),
            pane: first.clone(),
        },
    )
    .await;
    let (spaces, active, _) = layout(&mut ws).await;
    assert_eq!(
        spaces.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        vec![space.clone(), other.clone()],
        "closing the last pane keeps the space"
    );
    let Node::Pane {
        id: fresh,
        kind: PaneKind::Empty { .. },
        ..
    } = spaces[0].root.clone()
    else {
        panic!("the space holds one empty pane")
    };
    assert_ne!(fresh, first, "the empty pane is a new one");
    assert_eq!(spaces[0].focus, fresh, "the new pane is the focus");
    assert_eq!(active, Some(space.clone()), "the space stays showing");

    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(8),
            pane: fresh.clone(),
        },
    )
    .await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(
        matches!(&spaces[0].root, Node::Pane { id, .. } if *id == fresh),
        "closing an empty pane that is the whole space changes nothing"
    );

    ok(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(7),
            id: space,
            if_empty: false,
        },
    )
    .await;
    let (spaces, active, _) = layout(&mut ws).await;
    assert_eq!(
        spaces.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        vec![other.clone()]
    );
    assert_eq!(active, Some(other), "the neighbour becomes active");
}

/// The Mac app's close of an emptied background space is refused once something started in it,
/// so a worker launched into the space in the meantime keeps running.
#[tokio::test(flavor = "multi_thread")]
async fn a_close_if_empty_spares_a_space_something_started_in() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    let space = created(&mut ws, new_space("/tmp")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    ok(&mut ws, terminal(&pane)).await;
    let close = |if_empty| ClientMsg::SpaceClose {
        req: Some(3),
        id: space.clone(),
        if_empty,
    };
    let refused = call(&mut ws, close(true)).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if message.contains("no longer empty")),
        "{refused:?}"
    );
    let (spaces, _, terms) = layout(&mut ws).await;
    assert_eq!(spaces.len(), 1, "the space is still there");
    assert!(terms.iter().any(|t| t.alive), "its terminal still runs");

    ok(&mut ws, ClientMsg::PaneClose { req: Some(4), pane }).await;
    ok(&mut ws, close(true)).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(spaces.is_empty(), "a space holding one empty pane closes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_keeps_its_exited_terminal_until_the_pane_goes() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space("/tmp")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = wheeld::spaces::first_pane(&spaces[0].root).to_string();
    ok(&mut ws, terminal(&pane)).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let term = term_of(&spaces[0], &pane).expect("terminal pane");

    let refused = call(
        &mut ws,
        ClientMsg::TermClose {
            req: Some(3),
            id: term.clone(),
        },
    )
    .await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if message.contains("belongs to a pane")),
        "term_close refuses a pane's terminal: {refused:?}"
    );

    ok(
        &mut ws,
        ClientMsg::TermInput {
            req: Some(4),
            id: term.clone(),
            data: wheel_proto::encode_bytes(b"exit 3\r"),
        },
    )
    .await;
    let mut alive = true;
    for _ in 0..1200 {
        let (_, _, terms) = layout(&mut ws).await;
        let info = terms
            .iter()
            .find(|t| t.id == term)
            .expect("the pane's terminal stays listed after its shell exits");
        alive = info.alive;
        if !alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive, "the shell exited");

    // The pane replays its shell's exit, and delivering it does not drop the terminal.
    call(
        &mut ws,
        ClientMsg::TermAttach {
            req: Some(6),
            id: term.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::TermExited { code, .. } = recv(&mut ws).await {
            assert_eq!(code, Some(3));
            break;
        }
    }
    let (_, _, terms) = layout(&mut ws).await;
    assert!(terms.iter().any(|t| t.id == term && !t.alive));

    ok(&mut ws, ClientMsg::PaneClose { req: Some(5), pane }).await;
    let (_, _, terms) = layout(&mut ws).await;
    assert!(terms.is_empty(), "closing the pane forgot its terminal");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_refused() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    for msg in [
        new_space("relative/dir"),
        new_space("/no/such/dir/anywhere"),
        ClientMsg::PaneClose {
            req: Some(7),
            pane: "p404".into(),
        },
        ClientMsg::SplitRatio {
            req: Some(8),
            split: "x1".into(),
            ratio: 0.95,
        },
        ClientMsg::PaneSetKind {
            req: Some(9),
            pane: "p404".into(),
            kind: NewPaneKind::Web,
            artifact: None,
            scope: None,
            url: None,
        },
    ] {
        assert!(
            matches!(call(&mut ws, msg.clone()).await, ServerMsg::Error { .. }),
            "{msg:?} is refused"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_directories_list_newest_first_once_each_and_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("wheel-recent-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state_file = dir.join("spaces.json");
    let recent = |msg: ServerMsg| match msg {
        ServerMsg::SpaceRecent { dirs, .. } => dirs.into_iter().map(|d| d.dir).collect::<Vec<_>>(),
        other => panic!("expected space_recent, got {other:?}"),
    };

    let (url, first_daemon) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let ask = ClientMsg::SpaceRecent { req: Some(7) };
    assert!(recent(call(&mut ws, ask.clone()).await).is_empty());
    for cwd in ["/tmp", "/private/tmp", "/tmp"] {
        let id = created(&mut ws, new_space(cwd)).await;
        ok(
            &mut ws,
            ClientMsg::SpaceClose {
                req: Some(8),
                id,
                if_empty: false,
            },
        )
        .await;
    }
    let want = vec!["/tmp".to_string(), "/private/tmp".to_string()];
    assert_eq!(recent(call(&mut ws, ask.clone()).await), want);
    drop(ws);
    first_daemon.abort();

    let (url, _) = start_daemon(Some(state_file)).await;
    let mut ws = connect(&url).await;
    assert_eq!(recent(call(&mut ws, ask).await), want);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_restores_the_layout_with_fresh_shells() {
    let dir = std::env::temp_dir().join(format!("wheel-spaces-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state_file = dir.join("spaces.json");

    let (url, first_daemon) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space("/tmp")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let left = spaces[0].focus.clone();
    let right = created(
        &mut ws,
        ClientMsg::PaneSplit {
            req: Some(3),
            pane: left.clone(),
            dir: Dir::Right,
            background: false,
        },
    )
    .await;
    ok(&mut ws, terminal(&right)).await;
    let (before, active, _) = layout(&mut ws).await;
    let Node::Split { id: split, .. } = &before[0].root else {
        panic!("split")
    };
    ok(
        &mut ws,
        ClientMsg::SplitRatio {
            req: Some(4),
            split: split.clone(),
            ratio: 0.3,
        },
    )
    .await;
    let old_term = term_of(&before[0], &right).unwrap();
    // A pane closed before the restart: its id must never come back.
    let closed = created(
        &mut ws,
        ClientMsg::PaneSplit {
            req: Some(5),
            pane: left.clone(),
            dir: Dir::Down,
            background: false,
        },
    )
    .await;
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(6),
            pane: closed.clone(),
        },
    )
    .await;
    // Stop the first daemon before the second reads the file, so only one ever writes it.
    drop(ws);
    first_daemon.abort();

    let (url, _) = start_daemon(Some(state_file)).await;
    let mut ws = connect(&url).await;
    let (after, active_after, terms) = layout(&mut ws).await;
    assert_eq!(active_after, active);
    let Node::Split { id, ratio, a, .. } = &after[0].root else {
        panic!("split restored")
    };
    assert_eq!((id, *ratio), (split, 0.3));
    assert!(matches!(&**a, Node::Pane { id, kind: PaneKind::Empty { .. }, .. } if *id == left));
    let new_term = term_of(&after[0], &right).expect("still a terminal pane");
    assert_ne!(new_term, old_term, "a new shell, under a new id");
    assert!(terms
        .iter()
        .any(|t| t.id == new_term && t.alive && t.cwd == "/tmp"));

    // Ids come from one counter that survives the restart: everything made after it is
    // numbered above the pane closed before it.
    let num = |id: &str| id[1..].parse::<u64>().unwrap();
    let third = created(
        &mut ws,
        ClientMsg::PaneSplit {
            req: Some(7),
            pane: left.clone(),
            dir: Dir::Down,
            background: false,
        },
    )
    .await;
    for id in [&new_term, &third] {
        assert!(num(id) > num(&closed), "{id} is numbered after {closed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_of_unknown_kind_survives_restore_and_save() {
    let dir = std::env::temp_dir().join(format!("wheel-unknown-kind-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let hologram = serde_json::json!({"node": "pane", "id": "p9", "kind": "hologram", "depth": 3});
    let saved = serde_json::json!({
        "spaces": [{
            "id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p2",
            "root": {"node": "split", "id": "x3", "axis": "horizontal", "ratio": 0.5,
                     "a": {"node": "pane", "id": "p2", "kind": "empty"}, "b": hologram},
        }],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    // The daemon starts, and clients see the pane exactly as it was saved.
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let Node::Split { b, .. } = &spaces[0].root else {
        panic!("split")
    };
    assert_eq!(serde_json::to_value(&**b).unwrap(), hologram);

    // Saving after another change writes it back unchanged.
    ok(&mut ws, terminal("p2")).await;
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert_eq!(on_disk["spaces"][0]["root"]["b"], hologram);

    // It can be turned into a terminal, and closed.
    ok(&mut ws, terminal("p9")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(term_of(&spaces[0], "p9").is_some());
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(3),
            pane: "p9".into(),
        },
    )
    .await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(&spaces[0].root, Node::Pane { id, .. } if id == "p2"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_sweeps_pane_data_directories_no_layout_names() {
    let dir = std::env::temp_dir().join(format!("wheel-sweep-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state_file = dir.join("spaces.json");
    let panes = dir.join("panes");
    // p1 shows an artifact; p7 is left over from a pane the layout no longer has.
    std::fs::create_dir_all(panes.join("p1")).unwrap();
    std::fs::write(panes.join("p1/rows.json"), "[1]").unwrap();
    std::fs::create_dir_all(panes.join("p7/sub")).unwrap();
    std::fs::write(panes.join("p7/sub/rows.json"), "[7]").unwrap();
    let saved = serde_json::json!({
        "spaces": [{
            "id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p1",
            "root": {"node": "pane", "id": "p1", "kind": "web", "scope": "machine", "name": "viewer"},
        }],
        "active": "s1",
        "next_id": 2,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    let (_, serving) = start_daemon(Some(state_file)).await;
    assert!(!panes.join("p7").exists(), "the unnamed directory is swept");
    assert_eq!(
        std::fs::read_to_string(panes.join("p1/rows.json")).unwrap(),
        "[1]",
        "the named pane's directory is intact"
    );
    serving.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_agent_program_agent_panes_neither_start_nor_vanish() {
    let dir = std::env::temp_dir().join(format!("wheel-no-agent-cmd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let agent = serde_json::json!({"node": "pane", "id": "p2", "kind": "agent", "session": "w5", "agent": "claude"});
    let saved = serde_json::json!({
        "spaces": [{"id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p2", "root": agent}],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    // A saved agent pane stays as saved, so a later start with its program found runs it again.
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let missing = dir.join("no-such-claude");
    state
        .setting_set(
            "agent.claude.command",
            missing.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    state.restore_spaces(state_file.clone()).await.unwrap();
    assert_eq!(
        serde_json::to_value(&state.spaces().spaces[0].root).unwrap(),
        agent
    );

    // An agent that cannot start leaves no pane behind.
    let error = state
        .pane_start(
            "s1",
            None,
            wheel_proto::StartKind::Agent,
            Some("task".into()),
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(
        error.contains("agent claude cannot start") && error.contains("no-such-claude"),
        "{error}"
    );
    assert_eq!(
        serde_json::to_value(&state.spaces().spaces[0].root).unwrap(),
        agent
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_that_fails_leaves_the_person_where_they_were() {
    let dir = std::env::temp_dir().join(format!("wheel-failed-start-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let pane = |id: &str| serde_json::json!({"node": "pane", "id": id, "kind": "empty"});
    let saved = serde_json::json!({
        "spaces": [
            {"id": "s1", "name": "one", "cwd": "/tmp", "focus": "p2", "root": pane("p2")},
            {"id": "s2", "name": "two", "cwd": "/tmp", "focus": "p3", "root": pane("p3")},
        ],
        "active": "s2",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    state
        .setting_set(
            "agent.claude.command",
            dir.join("no-such-claude").to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    state.restore_spaces(state_file.clone()).await.unwrap();
    let before = serde_json::to_value(state.spaces()).unwrap();

    // The agent's program is missing, so the agent cannot start. Splitting space one for it showed that
    // space and focused the new pane; the refusal puts the person back in space two.
    state
        .pane_start("s1", None, wheel_proto::StartKind::Agent, None, None, true)
        .await
        .unwrap_err();
    assert_eq!(serde_json::to_value(state.spaces()).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_known_pane_keeps_fields_a_newer_build_added() {
    let dir = std::env::temp_dir().join(format!("wheel-extra-fields-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let saved = serde_json::json!({
        "spaces": [{
            "id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p2",
            "root": {"node": "split", "id": "x3", "axis": "horizontal", "ratio": 0.5,
                     "a": {"node": "pane", "id": "p2", "kind": "empty"},
                     "b": {"node": "pane", "id": "p4", "kind": "terminal", "term": "t1", "extra": 1}},
        }],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    // The daemon gives p4 a new shell and keeps the field it does not know.
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    ok(&mut ws, terminal("p2")).await;
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let p4 = &on_disk["spaces"][0]["root"]["b"];
    assert_eq!(p4["extra"], 1);
    assert_eq!(p4["kind"], "terminal");
    assert_ne!(p4["term"], "t1", "restore opened a new shell");

    // Choosing a new kind replaces the pane's content, unknown fields included.
    ok(&mut ws, terminal("p4")).await;
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert!(on_disk["spaces"][0]["root"]["b"].get("extra").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Make `name` an artifact in the `wheel.toml` in `dir`: an `[artifacts.<name>]` entry naming
/// `path`, with `extra` (TOML lines) beside it — the whole definition, as a person writes it. An
/// entry already there is replaced, and every other line of the file stays.
fn register(dir: &std::path::Path, name: &str, path: &std::path::Path, extra: &str) {
    let file = dir.join("wheel.toml");
    let mut doc: toml::Table = std::fs::read_to_string(&file)
        .map(|text| text.parse().unwrap())
        .unwrap_or_default();
    let entry: toml::Table = format!("path = {:?}\n{extra}", path.display().to_string())
        .parse()
        .unwrap();
    doc.entry("artifacts")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap()
        .insert(name.into(), toml::Value::Table(entry));
    std::fs::write(file, toml::to_string(&doc).unwrap()).unwrap();
}

/// A raw `GET` to the daemon's HTTP side, with extra header lines: the status code and the rest
/// of the response, headers included.
async fn http_get(addr: &str, target: &str, headers: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: {addr}\r\n{headers}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    (response[9..12].parse().unwrap(), response)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_pane_serves_its_artifact_and_reloads_on_change() {
    let dir = std::env::temp_dir().join(format!("wheel-web-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A project artifact, made by hand: a folder, and the entry naming it in the space's own
    // `wheel.toml`.
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>one</p>").unwrap();
    register(&dir, "site", std::path::Path::new("site"), "");
    // The layout saves in `inst` under `dir`, so that is the instance directory: the pane data
    // directories this test looks at go with `dir` when it is removed.
    let inst = dir.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    let (url, _) = start_daemon(Some(inst.join("spaces.json"))).await;
    let addr = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&dir.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let web = |artifact: Option<&str>| ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        artifact: artifact.map(str::to_string),
        scope: None,
        url: None,
    };
    assert!(
        matches!(call(&mut ws, web(None)).await, ServerMsg::Error { .. }),
        "web needs an artifact"
    );
    // No scope-and-name input names a folder outside the two roots: anything but one plain name
    // is refused before a folder is computed, and a name nowhere is refused rather than guessed.
    for bad in ["/etc", "..", ".", "../out", "a/b", ".env", "", "nosuch"] {
        match call(&mut ws, web(Some(bad))).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{bad:?} is not an artifact this space has, got {other:?}"),
        }
    }
    // A bare name resolves project, then machine. The same name in the machine root does not win,
    // and the pane stores the scope that won, so a project shadows a wider artifact with its own
    // and the saved layout never re-resolves.
    let machine = inst.join("artifacts/site");
    std::fs::create_dir_all(&machine).unwrap();
    std::fs::write(machine.join("index.html"), "<p>machine</p>").unwrap();
    register(&inst, "site", &machine, "");
    ok(&mut ws, web(Some("site"))).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Project && artifact.name() == "site"
    ));
    // A pinned scope takes that root whichever others have the name: `machine` over the project
    // `site` a bare name would take, and back.
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(11),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("site".into()),
            scope: Some(wheel_proto::ArtifactScope::Machine),
            url: None,
        },
    )
    .await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Machine && artifact.name() == "site"
    ));
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(11),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("site".into()),
            scope: Some(wheel_proto::ArtifactScope::Project),
            url: None,
        },
    )
    .await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Project && artifact.name() == "site"
    ));

    std::fs::write(site.join(".env"), "SECRET=1").unwrap();
    let page = format!("/artifacts/{pane}/index.html");
    let folder = format!("/artifacts/{pane}/");
    // A token in the URL is traded for a cookie, and the page loads again without it.
    let (status, response) = http_get(&addr, &format!("{page}?token={TOKEN}"), "").await;
    assert_eq!(status, 303);
    assert!(
        response.contains(&format!("location: {page}\r\n")),
        "{response}"
    );
    assert!(response.contains(&format!(
        "set-cookie: wheel_token={TOKEN}; Path=/artifacts/{pane}/; HttpOnly; SameSite=Strict"
    )));
    let cookie = format!("Cookie: wheel_token={TOKEN}\r\n");
    let (status, response) = http_get(&addr, &page, &cookie).await;
    assert_eq!(status, 200);
    assert!(response.ends_with("\r\n\r\n<p>one</p>"), "{response}");
    // The folder's own address is the page, which is what a client opens.
    let (status, response) = http_get(&addr, &folder, &cookie).await;
    assert_eq!(status, 200);
    assert!(response.ends_with("\r\n\r\n<p>one</p>"), "{response}");
    let bearer = format!("Authorization: Bearer {TOKEN}\r\n");
    assert_eq!(http_get(&addr, &page, &bearer).await.0, 200);
    assert_eq!(http_get(&addr, &page, "").await.0, 401, "no token");
    let refusals = [
        (
            format!("/artifacts/{pane}/../../etc/hosts"),
            "no path out of the folder",
        ),
        (format!("/artifacts/{pane}/.env"), "no hidden file"),
    ];
    for (target, why) in refusals {
        assert_eq!(http_get(&addr, &target, &cookie).await.0, 404, "{why}");
    }
    let foreign = format!("{cookie}Origin: http://evil.example\r\n");
    assert_eq!(
        http_get(&addr, &page, &foreign).await.0,
        403,
        "another site's page"
    );

    // Opening the pane made it a data directory of its own, under the instance directory rather
    // than inside the artifact or anywhere in the project's tree. Nothing in the daemon writes
    // there yet, so a file put there by hand is what the pane serves under `data/`.
    let data = inst.join("panes").join(&pane);
    assert!(data.is_dir(), "{} was not made", data.display());
    std::fs::write(data.join("rows.json"), "{\"n\":1}").unwrap();
    std::fs::write(data.join(".env"), "SECRET=1").unwrap();
    let rows = format!("/artifacts/{pane}/data/rows.json");
    let (status, response) = http_get(&addr, &rows, &cookie).await;
    assert_eq!(status, 200);
    assert!(
        response.contains("content-type: application/json"),
        "{response}"
    );
    assert!(response.ends_with("\r\n\r\n{\"n\":1}"), "{response}");
    // The prefix dispatches before any path is resolved, so an artifact's own `data/` — which the
    // entry reader reports as a problem — can never be reached.
    std::fs::create_dir(site.join("data")).unwrap();
    std::fs::write(site.join("data/rows.json"), "the artifact's own").unwrap();
    let (status, response) = http_get(&addr, &rows, &cookie).await;
    assert_eq!(status, 200);
    assert!(
        response.ends_with("\r\n\r\n{\"n\":1}"),
        "the pane's own file, not the artifact's: {response}"
    );
    // The data directory is a root like the artifact's folder, held to the same refusals by the
    // same resolver.
    let refusals = [
        (
            format!("/artifacts/{pane}/data/../../etc/passwd"),
            "no path out of the data directory",
        ),
        (
            format!("/artifacts/{pane}/data//etc/passwd"),
            "no absolute path",
        ),
        (format!("/artifacts/{pane}/data/.env"), "no hidden file"),
    ];
    for (target, why) in refusals {
        assert_eq!(http_get(&addr, &target, &cookie).await.0, 404, "{why}");
    }

    let mut watcher = connect(&url).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(site.join("index.html"), "<p>two</p>").unwrap();
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(&mut watcher).await {
            assert_eq!(changed, pane);
            break;
        }
    }
    // A change in the pane's own data directory reloads the page the same way.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), watcher.next()).await {}
    std::fs::write(data.join("rows.json"), "{\"n\":2}").unwrap();
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(&mut watcher).await {
            assert_eq!(changed, pane);
            break;
        }
    }

    // One machine artifact — one folder, reachable from every space — open in two projects has two
    // data directories, and neither pane serves the other's files.
    let viewer = inst.join("artifacts/viewer");
    std::fs::create_dir_all(&viewer).unwrap();
    std::fs::write(viewer.join("index.html"), "<p>viewer</p>").unwrap();
    register(
        &inst,
        "viewer",
        std::path::Path::new("artifacts/viewer"),
        "",
    );
    let other = dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    created(&mut ws, new_space(&other.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let second = spaces[1].focus.clone();
    for (pane, body) in [
        (&pane, "{\"of\":\"first\"}"),
        (&second, "{\"of\":\"second\"}"),
    ] {
        ok(
            &mut ws,
            ClientMsg::PaneSetKind {
                req: Some(12),
                pane: pane.clone(),
                kind: NewPaneKind::Web,
                artifact: Some("viewer".into()),
                scope: Some(ArtifactScope::Machine),
                url: None,
            },
        )
        .await;
        std::fs::write(inst.join("panes").join(pane).join("rows.json"), body).unwrap();
    }
    for (pane, body) in [(&pane, "first"), (&second, "second")] {
        let (status, response) =
            http_get(&addr, &format!("/artifacts/{pane}/data/rows.json"), &cookie).await;
        assert_eq!(status, 200);
        assert!(
            response.ends_with(&format!("\r\n\r\n{{\"of\":\"{body}\"}}")),
            "{pane} served the other pane's data: {response}"
        );
    }
    // Closing a pane takes its data directory with it, so nothing a refresh wrote is inherited.
    let gone = inst.join("panes").join(&second);
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(13),
            pane: second.clone(),
        },
    )
    .await;
    assert!(!gone.exists(), "{} outlived its pane", gone.display());
    assert!(
        inst.join("panes").join(&pane).is_dir(),
        "the other pane's data directory went with it"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An entry can name a page anywhere on the machine — a folder whose page is `index.html`, or one
/// HTML file — and the pane serves the folder that page really sits in and nothing outside it: not
/// a `..`, not a hidden file, not a link inside the folder pointing out (ADR-0003).
#[tokio::test(flavor = "multi_thread")]
async fn an_entry_serves_a_page_from_anywhere_and_nothing_outside_its_folder() {
    let base = std::env::temp_dir().join(format!("wheel-anywhere-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    // A course kept in a repo of its own, outside the project and outside every Wheel directory.
    let course = base.join("learn/course");
    std::fs::create_dir_all(course.join("lessons")).unwrap();
    std::fs::create_dir_all(course.join(".git")).unwrap();
    std::fs::write(course.join("index.html"), "<p>course</p>").unwrap();
    std::fs::write(course.join("sheet.html"), "<p>sheet</p>").unwrap();
    std::fs::write(course.join("lessons/one.html"), "<p>lesson one</p>").unwrap();
    std::fs::write(course.join(".git/config"), "secret").unwrap();
    std::fs::write(base.join("learn/beside.txt"), "beside").unwrap();
    std::os::unix::fs::symlink(base.join("learn/beside.txt"), course.join("out.txt")).unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // One entry names the folder by a path relative to the project's wheel.toml, one names a file.
    register(&proj, "course", std::path::Path::new("../learn/course"), "");
    register(&proj, "sheet", &course.join("sheet.html"), "");
    let inst = base.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    let (url, _) = start_daemon(Some(inst.join("spaces.json"))).await;
    let addr = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&proj.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let show = |name: &str| ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        artifact: Some(name.into()),
        scope: None,
        url: None,
    };
    let cookie = format!("Cookie: wheel_token={TOKEN}\r\n");
    let get = |path: &str| format!("/artifacts/{pane}/{path}");

    ok(&mut ws, show("course")).await;
    let shown = artifact(
        &mut ws,
        ClientMsg::ArtifactShow {
            req: Some(11),
            name: "course".into(),
            scope: None,
            space: None,
        },
    )
    .await;
    assert_eq!(
        (shown.dir.as_str(), shown.page.as_deref()),
        (
            &*course.canonicalize().unwrap().to_string_lossy(),
            Some("index.html")
        )
    );
    for (path, body) in [
        ("", "<p>course</p>"),
        ("lessons/one.html", "<p>lesson one</p>"),
        ("sheet.html", "<p>sheet</p>"),
    ] {
        let (status, response) = http_get(&addr, &get(path), &cookie).await;
        assert_eq!(status, 200, "{path}: {response}");
        assert!(response.ends_with(body), "{path}: {response}");
    }
    for (path, why) in [
        ("../beside.txt", "no path out of the folder"),
        (
            "lessons/../../beside.txt",
            "no path out through a subfolder",
        ),
        ("out.txt", "no link inside the folder pointing out"),
        (".git/config", "no hidden file"),
    ] {
        assert_eq!(http_get(&addr, &get(path), &cookie).await.0, 404, "{why}");
    }

    // An entry naming one file: the folder's address is that page, and its folder is the root.
    ok(&mut ws, show("sheet")).await;
    let (status, response) = http_get(&addr, &get(""), &cookie).await;
    assert_eq!(status, 200, "{response}");
    assert!(response.ends_with("<p>sheet</p>"), "{response}");
    assert_eq!(
        http_get(&addr, &get("lessons/one.html"), &cookie).await.0,
        200
    );
    // Nothing was written into the course's folder.
    let mut names: Vec<_> = std::fs::read_dir(&course)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [".git", "index.html", "lessons", "out.txt", "sheet.html"]
    );
    std::fs::remove_dir_all(&base).unwrap();
}

/// A page that already exists is registered where it is: `artifact add` writes its entry and
/// nothing in its folder, refuses a path with no page, and `artifact remove` deletes the entry and
/// leaves the files.
#[tokio::test(flavor = "multi_thread")]
async fn an_existing_page_is_registered_and_removed_without_touching_it() {
    let base = std::env::temp_dir().join(format!("wheel-add-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let course = base.join("learn/course");
    std::fs::create_dir_all(&course).unwrap();
    std::fs::write(course.join("index.html"), "<p>course</p>").unwrap();
    std::fs::write(course.join("sheet.html"), "<p>sheet</p>").unwrap();
    std::fs::write(course.join("notes.txt"), "notes").unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(proj.join("site")).unwrap();
    std::fs::write(proj.join("site/index.html"), "<p>site</p>").unwrap();
    let inst = base.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    // The machine file already holds a setting, which an artifact write and removal both keep.
    std::fs::write(
        inst.join("wheel.toml"),
        "[browser]\nhomepage = \"about:blank\"\n",
    )
    .unwrap();
    let (url, _) = start_daemon(Some(inst.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&proj.to_string_lossy())).await;
    let add = |path: &std::path::Path, scope: Option<ArtifactScope>| ClientMsg::ArtifactAdd {
        req: Some(30),
        path: path.display().to_string(),
        name: None,
        scope,
        space: None,
        title: None,
        command: None,
        every: None,
        pane: false,
    };
    let entries = |file: &std::path::Path| -> toml::Table {
        std::fs::read_to_string(file).unwrap().parse().unwrap()
    };
    let listing = |dir: &std::path::Path| {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };

    // A folder outside the project: named after itself, written absolutely, its files untouched.
    let added = artifact(&mut ws, add(&course, None)).await;
    assert_eq!(
        (added.scope, added.name.as_str(), added.dir.as_str()),
        (ArtifactScope::Project, "course", &*course.to_string_lossy())
    );
    assert!(added.problems.is_empty(), "{added:?}");
    let written = entries(&proj.join("wheel.toml"));
    assert_eq!(
        written["artifacts"]["course"]["path"].as_str(),
        Some(&*course.to_string_lossy())
    );
    assert_eq!(listing(&course), ["index.html", "notes.txt", "sheet.html"]);
    // A folder inside the project is written relative to its wheel.toml.
    let site = artifact(&mut ws, add(&proj.join("site"), None)).await;
    assert_eq!(site.name, "site");
    assert_eq!(
        entries(&proj.join("wheel.toml"))["artifacts"]["site"]["path"].as_str(),
        Some("site")
    );

    // Refused, each leaving the file as it was: the same name again, a path with no page, a file
    // that is not HTML, a relative path, and an interval with nothing to run.
    let before = std::fs::read_to_string(proj.join("wheel.toml")).unwrap();
    let declaring =
        |title: Option<&str>, command: Option<&str>, every: Option<u64>| ClientMsg::ArtifactAdd {
            req: Some(33),
            path: course.join("sheet.html").display().to_string(),
            name: None,
            scope: None,
            space: None,
            title: title.map(str::to_string),
            command: command.map(str::to_string),
            every,
            pane: false,
        };
    for (msg, says) in [
        (add(&course, None), "already"),
        (
            declaring(None, None, Some(5)),
            "no command says what to run",
        ),
        (
            declaring(None, Some("true"), Some(0)),
            "not a positive whole number",
        ),
        (
            declaring(None, Some("true"), Some(u64::MAX)),
            "not a positive whole number",
        ),
        (declaring(Some("  "), None, None), "title is empty"),
        (declaring(None, Some(" "), None), "command is empty"),
        (
            add(
                &std::path::PathBuf::from(format!("{} ", course.display())),
                None,
            ),
            "begins or ends with a space",
        ),
        (add(&base.join("missing"), None), "nothing at"),
        (add(&course.join("notes.txt"), None), "is not an HTML file"),
        (
            add(std::path::Path::new("learn/course"), None),
            "not an absolute path",
        ),
        (add(&base.join("learn"), None), "no index.html"),
    ] {
        match call(&mut ws, msg).await {
            ServerMsg::Error { message, .. } => assert!(message.contains(says), "{message}"),
            other => panic!("{says}: expected a refusal, got {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read_to_string(proj.join("wheel.toml")).unwrap(),
        before
    );
    // A refused add touches neither registry.
    assert!(!std::fs::read_to_string(inst.join("wheel.toml"))
        .unwrap()
        .contains("artifacts"));

    // One file, machine-wide, with the rest of its definition: named after the file.
    let sheet = artifact(
        &mut ws,
        ClientMsg::ArtifactAdd {
            req: Some(31),
            path: course.join("sheet.html").display().to_string(),
            name: None,
            scope: Some(ArtifactScope::Machine),
            space: None,
            title: Some("Interview sheet".into()),
            command: Some("true".into()),
            every: Some(30),
            pane: true,
        },
    )
    .await;
    assert_eq!(
        (
            sheet.name.as_str(),
            sheet.title.as_str(),
            sheet.page.as_deref()
        ),
        ("sheet", "Interview sheet", Some("sheet.html"))
    );
    assert_eq!(
        (sheet.command.as_deref(), sheet.every, sheet.pane),
        (Some("true"), Some(30), true)
    );
    let machine = entries(&inst.join("wheel.toml"));
    assert_eq!(machine["browser"]["homepage"].as_str(), Some("about:blank"));
    assert_eq!(
        machine["artifacts"]["sheet"]["every"].as_integer(),
        Some(30)
    );

    // Removing deletes the entry and nothing else: the folder stays, the name no longer resolves,
    // and a table the removal empties goes with it while the setting beside it stays.
    let remove = |name: &str| ClientMsg::ArtifactRemove {
        req: Some(32),
        name: name.into(),
        scope: None,
        space: None,
    };
    // Every connection is told the entry is gone, with the space directory a client needs to name
    // the browser storage the page kept.
    let mut watcher = connect(&url).await;
    let was = artifact(&mut ws, remove("course")).await;
    loop {
        if let ServerMsg::ArtifactGone { scope, name, cwd } = recv(&mut watcher).await {
            assert_eq!(
                (scope, name.as_str(), cwd.as_deref()),
                (
                    ArtifactScope::Project,
                    "course",
                    Some(&*proj.to_string_lossy())
                )
            );
            break;
        }
    }
    assert_eq!(was.dir, course.to_string_lossy());
    assert_eq!(listing(&course), ["index.html", "notes.txt", "sheet.html"]);
    assert!(entries(&proj.join("wheel.toml"))["artifacts"]
        .get("course")
        .is_none());
    assert!(matches!(
        call(&mut ws, show("course", None)).await,
        ServerMsg::Error { .. }
    ));
    let before = std::fs::read_to_string(proj.join("wheel.toml")).unwrap();
    assert!(matches!(
        call(&mut ws, remove("course")).await,
        ServerMsg::Error { .. }
    ));
    // Pinned to a scope that lacks it, the name is refused and the file is left as it was.
    let pinned = ClientMsg::ArtifactRemove {
        req: Some(34),
        name: "site".into(),
        scope: Some(ArtifactScope::Machine),
        space: None,
    };
    assert!(matches!(
        call(&mut ws, pinned).await,
        ServerMsg::Error { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(proj.join("wheel.toml")).unwrap(),
        before
    );
    artifact(&mut ws, remove("sheet")).await;
    let machine = entries(&inst.join("wheel.toml"));
    assert!(machine.get("artifacts").is_none(), "{machine:?}");
    assert_eq!(machine["browser"]["homepage"].as_str(), Some("about:blank"));
    std::fs::remove_dir_all(&base).unwrap();
}

/// The record a request about one artifact replies with.
async fn artifact(ws: &mut Ws, msg: ClientMsg) -> wheel_proto::ArtifactInfo {
    match call(ws, msg).await {
        ServerMsg::Artifact { artifact, .. } => artifact,
        other => panic!("expected artifact, got {other:?}"),
    }
}

fn make(name: &str, scope: Option<ArtifactScope>) -> ClientMsg {
    ClientMsg::ArtifactNew {
        ends_with_pane: None,
        expires_in: None,
        req: Some(20),
        name: name.into(),
        scope,
        space: None,
        path: None,
        pane: false,
    }
}

fn make_for_pane(name: &str, scope: Option<ArtifactScope>) -> ClientMsg {
    ClientMsg::ArtifactNew {
        ends_with_pane: None,
        expires_in: None,
        req: Some(22),
        name: name.into(),
        scope,
        space: None,
        path: None,
        pane: true,
    }
}

fn show(name: &str, scope: Option<ArtifactScope>) -> ClientMsg {
    ClientMsg::ArtifactShow {
        req: Some(21),
        name: name.into(),
        scope,
        space: None,
    }
}

/// `pane = true` in an entry is what offers an artifact in a pane picker, and the registries are
/// the scopes: a flagged `machine` artifact is offered in every space, a flagged `project` one only in
/// its own project, and an artifact with no flag is offered nowhere while staying openable by name.
#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_opts_into_the_pane_picker() {
    let base = std::env::temp_dir().join(format!("wheel-pane-flag-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let (one, two) = (base.join("one"), base.join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let state_file = base.join("spaces.json");
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&one.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let first = spaces[0].id.clone();

    // Made without the flag, an artifact is a one-off: the record says so and no picker list has
    // it, though showing it by name still works.
    let plain = artifact(&mut ws, make("scratch", None)).await;
    assert!(!plain.pane, "{plain:?}");
    let flagged = artifact(&mut ws, make_for_pane("dash", None)).await;
    assert!(flagged.pane, "{flagged:?}");
    // The flag is in the entry it wrote, and a one-off's entry carries the key too, set false, so
    // the next person to open the file can see it exists.
    let written: toml::Table = std::fs::read_to_string(one.join("wheel.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(written["artifacts"]["dash"]["pane"].as_bool(), Some(true));
    assert_eq!(
        written["artifacts"]["scratch"]["pane"].as_bool(),
        Some(false)
    );

    // A one-off is openable by name, which is the whole point of it not being listed: this is
    // asserted on a clean entry, before the bad-value case below touches it.
    let shown = artifact(&mut ws, show("scratch", None)).await;
    assert_eq!(
        (shown.name.as_str(), shown.pane, shown.problems.as_slice()),
        ("scratch", false, &[][..]),
        "{shown:?}"
    );

    // A machine-wide artifact, flagged, and a second project.
    artifact(
        &mut ws,
        make_for_pane("viewer", Some(ArtifactScope::Machine)),
    )
    .await;
    created(&mut ws, new_space(&two.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let second = spaces
        .iter()
        .find(|s| s.cwd.ends_with("two"))
        .expect("the second space")
        .id
        .clone();
    let in_second = ClientMsg::ArtifactNew {
        ends_with_pane: None,
        expires_in: None,
        req: Some(26),
        name: "other".into(),
        scope: Some(ArtifactScope::Project),
        space: Some(second.clone()),
        path: None,
        pane: true,
    };
    artifact(&mut ws, in_second).await;

    // What each space's picker is offered: its own flagged artifacts and the machine-wide ones,
    // never another project's, and never a one-off.
    let offered = |all: Vec<wheel_proto::ArtifactInfo>| {
        all.iter()
            .map(|a| format!("{} {}", a.scope.name(), a.name))
            .collect::<Vec<_>>()
    };
    let (all, _) = artifacts_of(&mut ws, Some(&first), true).await;
    assert_eq!(offered(all), ["project dash", "machine viewer"]);
    let (all, _) = artifacts_of(&mut ws, Some(&second), true).await;
    assert_eq!(offered(all), ["project other", "machine viewer"]);
    // Unfiltered, the one-off is there too, and every record carries the flag.
    let (all, _) = artifacts_of(&mut ws, Some(&first), false).await;
    assert_eq!(
        offered(all.clone()),
        ["project dash", "project scratch", "machine viewer"]
    );
    assert_eq!(
        all.iter().map(|a| a.pane).collect::<Vec<_>>(),
        [true, false, true]
    );

    // A `pane` that is not a boolean is a problem like any other, and the artifact still opens:
    // an entry a person got wrong must not take the artifact away from them.
    register(
        &one,
        "scratch",
        std::path::Path::new(".wheel/artifacts/scratch"),
        "title = \"Scratch\"\npane = \"yes\"\n",
    );
    // Named by space, because the space being shown is the second one by now.
    let in_first = ClientMsg::ArtifactShow {
        req: Some(24),
        name: "scratch".into(),
        scope: None,
        space: Some(first.clone()),
    };
    let shown = artifact(&mut ws, in_first).await;
    assert_eq!(shown.problems, ["pane is not true or false"], "{shown:?}");
    assert!(!shown.pane, "a value it could not read offers nothing");
    let (all, _) = artifacts_of(&mut ws, Some(&first), true).await;
    assert_eq!(offered(all), ["project dash", "machine viewer"]);

    // `resolves` answers what a bare name means whether or not the filter is on: a one-off still
    // shadows a flagged artifact of the same name in a later scope, and a picker has to know.
    let shadowing = ClientMsg::ArtifactNew {
        ends_with_pane: None,
        expires_in: None,
        req: Some(25),
        name: "viewer".into(),
        scope: Some(ArtifactScope::Project),
        space: Some(first.clone()),
        path: None,
        pane: false,
    };
    artifact(&mut ws, shadowing).await;
    let (all, resolves) = artifacts_of(&mut ws, Some(&first), true).await;
    assert_eq!(
        resolves.get("viewer"),
        Some(&ArtifactScope::Project),
        "the unflagged project one is what the bare name means"
    );
    assert_eq!(
        offered(all),
        ["project dash", "machine viewer"],
        "and it is not offered: {resolves:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Every artifact as the daemon lists them for `space`, filtered to the picker's own when `pane`.
async fn artifacts_of(
    ws: &mut Ws,
    space: Option<&str>,
    pane: bool,
) -> (
    Vec<wheel_proto::ArtifactInfo>,
    std::collections::BTreeMap<String, ArtifactScope>,
) {
    let msg = ClientMsg::ArtifactLs {
        req: Some(23),
        space: space.map(str::to_string),
        pane,
    };
    match call(ws, msg).await {
        ServerMsg::Artifacts {
            artifacts,
            resolves,
            ..
        } => (artifacts, resolves),
        other => panic!("expected artifacts, got {other:?}"),
    }
}

/// An artifact's entry in a `wheel.toml` is its whole definition: the daemon makes the folder, the
/// page and the entry, reads them back with every problem it finds, and puts nothing about any of
/// it in the saved state file.
#[tokio::test(flavor = "multi_thread")]
async fn artifacts_are_made_listed_and_shown_from_their_entries() {
    let base = std::env::temp_dir().join(format!("wheel-artifact-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let state_file = base.join("spaces.json");
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;

    // Every scope's root is a space's directory or the instance's, so a run with no space at all
    // is refused rather than resolved against nothing.
    assert!(matches!(
        call(
            &mut ws,
            ClientMsg::ArtifactLs {
                req: Some(19),
                space: None,
                pane: false,
            }
        )
        .await,
        ServerMsg::Error { .. }
    ));
    created(&mut ws, new_space(&proj.to_string_lossy())).await;

    // A new artifact: the folder, a page, an entry titled after it, and nothing wrong with it.
    let made = artifact(&mut ws, make("dash", None)).await;
    assert_eq!(
        made.scope,
        ArtifactScope::Project,
        "project is the default scope"
    );
    assert_eq!(
        made.dir,
        proj.join(".wheel/artifacts/dash").to_string_lossy()
    );
    assert_eq!((made.title.as_str(), made.problems.len()), ("dash", 0));
    assert_eq!(
        (made.entry.as_str(), made.page.as_deref()),
        (
            &*proj.join("wheel.toml").to_string_lossy(),
            Some("index.html")
        )
    );
    // The entry names the folder relative to the file, so a project that commits both works in
    // every clone, and nothing but the page is written beside it.
    let written: toml::Table = std::fs::read_to_string(proj.join("wheel.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        written["artifacts"]["dash"]["path"].as_str(),
        Some(".wheel/artifacts/dash")
    );
    assert_eq!(written["artifacts"]["dash"]["title"].as_str(), Some("dash"));
    assert_eq!(
        std::fs::read_dir(proj.join(".wheel/artifacts/dash"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>(),
        ["index.html"]
    );
    // A folder anywhere: given as an absolute path, it is made there and named absolutely; a
    // relative path is refused, since the daemon's directory is not the caller's.
    let elsewhere = base.join("elsewhere/course");
    let anywhere = artifact(
        &mut ws,
        ClientMsg::ArtifactNew {
            ends_with_pane: None,
            expires_in: None,
            req: Some(27),
            name: "course".into(),
            scope: None,
            space: None,
            path: Some(elsewhere.display().to_string()),
            pane: false,
        },
    )
    .await;
    assert_eq!(anywhere.dir, elsewhere.to_string_lossy());
    assert!(elsewhere.join("index.html").is_file());
    let written: toml::Table = std::fs::read_to_string(proj.join("wheel.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        written["artifacts"]["course"]["path"].as_str(),
        Some(&*elsewhere.to_string_lossy())
    );
    let relative = ClientMsg::ArtifactNew {
        ends_with_pane: None,
        expires_in: None,
        req: Some(28),
        name: "rel".into(),
        scope: None,
        space: None,
        path: Some("rel".into()),
        pane: false,
    };
    assert!(matches!(
        call(&mut ws, relative).await,
        ServerMsg::Error { .. }
    ));
    // A failed new --path leaves no folder it made: `deep/..` is made as a parent, then the claim
    // on the folder itself fails because it resolves to an existing one.
    let fresh = base.join("fresh");
    match call(
        &mut ws,
        ClientMsg::ArtifactNew {
            ends_with_pane: None,
            expires_in: None,
            req: Some(29),
            name: "ghost".into(),
            scope: None,
            space: None,
            path: Some(format!("{}/a/b/..", fresh.display())),
            pane: false,
        },
    )
    .await
    {
        ServerMsg::Error { .. } => {}
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(!fresh.exists(), "{} was left behind", fresh.display());
    // A `..` after a folder not yet made reaches one that exists: it stays.
    let kept = base.join("kept");
    std::fs::create_dir(&kept).unwrap();
    let _ = call(
        &mut ws,
        ClientMsg::ArtifactNew {
            ends_with_pane: None,
            expires_in: None,
            req: Some(30),
            name: "ghost".into(),
            scope: None,
            space: None,
            path: Some(format!("{}/gone/../kept/..", base.display())),
            pane: false,
        },
    )
    .await;
    assert!(kept.is_dir() && !base.join("gone").exists());
    // Nothing an artifact holds is overwritten, and a name no reference can hold never reaches disk.
    assert!(matches!(
        call(&mut ws, make("dash", None)).await,
        ServerMsg::Error { .. }
    ));
    for bad in ["a/b", "..", ".env", "", "/etc"] {
        match call(&mut ws, make(bad, Some(ArtifactScope::Project))).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{bad:?} is not an artifact name, got {other:?}"),
        }
    }

    // A machine artifact's folder is under the instance directory, outside every space.
    let machine = artifact(&mut ws, make("viewer", Some(ArtifactScope::Machine))).await;
    assert_eq!(machine.dir, base.join("artifacts/viewer").to_string_lossy());
    let dash = artifact(&mut ws, make("dash", Some(ArtifactScope::Machine))).await;
    assert_eq!(dash.dir, base.join("artifacts/dash").to_string_lossy());

    // An entry's problems are part of the record, so a bad artifact is never only a log line.
    let broken = proj.join(".wheel/artifacts/broken");
    std::fs::create_dir_all(broken.join("data")).unwrap();
    register(
        &proj,
        "broken",
        std::path::Path::new(".wheel/artifacts/broken"),
        "colour = 1\nevery = 2\n",
    );
    let shown = artifact(&mut ws, show("broken", None)).await;
    assert_eq!(shown.every, Some(2));
    assert_eq!(shown.title, "broken", "an entry with no title");
    for want in [
        "unknown key colour",
        "no command says what to run",
        "no index.html",
        "data/ is reserved",
    ] {
        assert!(
            shown.problems.iter().any(|p| p.contains(want)),
            "{want:?} in {:?}",
            shown.problems
        );
    }

    // Every artifact of every scope, in the order a bare name resolves in.
    let ServerMsg::Artifacts {
        artifacts,
        resolves,
        ..
    } = call(
        &mut ws,
        ClientMsg::ArtifactLs {
            req: Some(22),
            space: None,
            pane: false,
        },
    )
    .await
    else {
        panic!("expected artifacts")
    };
    assert_eq!(
        artifacts
            .iter()
            .map(|a| (a.scope.name(), a.name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("project", "broken"),
            ("project", "course"),
            ("project", "dash"),
            ("machine", "dash"),
            ("machine", "viewer"),
        ]
    );
    assert_eq!(
        resolves
            .iter()
            .map(|(name, scope)| (name.as_str(), scope.name()))
            .collect::<Vec<_>>(),
        [
            ("broken", "project"),
            ("course", "project"),
            ("dash", "project"),
            ("viewer", "machine"),
        ]
    );

    // A bare name takes the scope that won; a pinned one takes its own root, and refuses when that
    // root has nothing by that name.
    assert_eq!(
        artifact(&mut ws, show("dash", None)).await.scope,
        ArtifactScope::Project
    );
    assert_eq!(
        artifact(&mut ws, show("dash", Some(ArtifactScope::Machine)))
            .await
            .dir,
        dash.dir
    );
    for (name, scope) in [("nosuch", None), ("viewer", Some(ArtifactScope::Project))] {
        match call(&mut ws, show(name, scope)).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{name} in {scope:?} should not resolve, got {other:?}"),
        }
    }

    // The entry is the whole definition: the saved state file names no artifact, so no rewrite of
    // that file can lose one.
    let saved = std::fs::read_to_string(&state_file).unwrap();
    for name in ["dash", "viewer", "course", "title"] {
        assert!(!saved.contains(name), "{name} is in {saved}");
    }
    std::fs::remove_dir_all(&base).unwrap();
}

async fn watches(ws: &mut Ws) -> Vec<wheel_proto::WatchInfo> {
    match call(ws, ClientMsg::Status { req: Some(98) }).await {
        ServerMsg::Status { watches, .. } => watches,
        other => panic!("expected status, got {other:?}"),
    }
}

/// `pane`'s watch of the directory `role` names, from a `status` reply.
fn row(all: &[WatchInfo], pane: &str, role: WatchRole) -> Option<WatchInfo> {
    all.iter()
        .find(|w| w.pane == pane && w.role == role)
        .cloned()
}

/// Poll `status` until `pane`'s watch of `role`'s directory is in the state `want` says, for up to
/// 30s.
async fn watch_until(
    ws: &mut Ws,
    pane: &str,
    role: WatchRole,
    want: impl Fn(Option<&WatchInfo>) -> bool,
) {
    for _ in 0..600 {
        let all = watches(ws).await;
        if want(row(&all, pane, role).as_ref()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "{pane}'s {role:?} watch never got there: {:?}",
        watches(ws).await
    );
}

/// Wait for `pane_reload` of `pane`. How long it takes is not asserted: `recv`'s ceiling already
/// fails a reload that never comes, and any tighter bound measures how busy the machine is.
async fn reloaded(ws: &mut Ws, pane: &str) {
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(ws).await {
            if changed == pane {
                return;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_pane_watch_follows_its_folder_and_stops_with_the_pane() {
    let base = std::env::temp_dir().join(format!("wheel-watch-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    // `site` is never made up front: a restored pane naming a missing artifact waits for it.
    let repo = base.join(".wheel/artifacts");
    let site = repo.join("site");
    let shown = repo.join("shown");
    // A machine artifact, outside every space, which is the whole point of the scope.
    let viewer = base.join("artifacts/viewer");
    std::fs::create_dir_all(&shown).unwrap();
    std::fs::create_dir_all(&viewer).unwrap();
    // `base` is both the first space's directory and the instance directory, so one `wheel.toml`
    // holds the project's entries and the machine's.
    register(&base, "site", &site, "");
    register(&base, "shown", &shown, "");
    register(&base, "viewer", &viewer, "");
    let state_file = base.join("spaces.json");
    let web = |id: &str, scope: &str, name: &str| serde_json::json!({"node": "pane", "id": id, "kind": "web", "scope": scope, "name": name});
    let saved = serde_json::json!({
        "spaces": [
            {"id": "s1", "name": "one", "cwd": base, "focus": "p2",
             "root": {"node": "split", "id": "x5", "axis": "horizontal", "ratio": 0.5,
                      "a": web("p2", "project", "site"), "b": web("p3", "project", "shown")}},
            // The same machine artifact in two spaces: one folder, two panes, two data directories.
            {"id": "s4", "name": "two", "cwd": base, "focus": "p6", "root": web("p6", "machine", "viewer")},
            // A machine artifact's folder is under the instance directory, outside every space.
            {"id": "s7", "name": "three", "cwd": shown, "focus": "p8", "root": web("p8", "machine", "viewer")},
        ],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    // Restored with its folder missing, the pane keeps its artifact and waits on the folder above.
    let (url, _) = start_daemon(Some(state_file)).await;
    let mut ws = connect(&url).await;
    let mut events = connect(&url).await;
    let all = watches(&mut ws).await;
    let at = |path: &std::path::Path| Some(path.to_string_lossy().into_owned());
    // Two rows per pane: the artifact's folder, then the pane's own data directory.
    assert_eq!(
        all.iter()
            .map(|w| (&w.pane[..], w.role))
            .collect::<Vec<_>>(),
        [
            ("p2", WatchRole::Artifact),
            ("p2", WatchRole::Data),
            ("p3", WatchRole::Artifact),
            ("p3", WatchRole::Data),
            ("p6", WatchRole::Artifact),
            ("p6", WatchRole::Data),
            ("p8", WatchRole::Artifact),
            ("p8", WatchRole::Data),
        ]
    );
    // Restoring the layout made each pane its data directory, keyed by the pane: p6 and p8 show
    // one machine artifact in two spaces and get one directory each.
    let data = |pane: &str| base.join("panes").join(pane);
    for pane in ["p2", "p3", "p6", "p8"] {
        let dir = data(pane);
        assert!(dir.is_dir(), "{} was not made", dir.display());
        assert_eq!(
            row(&all, pane, WatchRole::Data).unwrap().dir,
            dir.to_string_lossy()
        );
    }
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, "p2"),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Project && artifact.name() == "site"
    ));
    let p2 = row(&all, "p2", WatchRole::Artifact).unwrap();
    assert_eq!((p2.watching, p2.waiting), (at(&repo), true));
    assert_eq!(p2.dir, site.to_string_lossy());
    let p3 = row(&all, "p3", WatchRole::Artifact).unwrap();
    assert_eq!((p3.watching, p3.waiting), (at(&shown), false));
    // The machine scope resolves under the instance directory, not under any space.
    let p8 = row(&all, "p8", WatchRole::Artifact).unwrap();
    assert_eq!(
        (p8.dir, p8.waiting),
        (viewer.to_string_lossy().into_owned(), false)
    );

    // The folder appearing moves the watch onto it and reloads the page.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::create_dir(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>one</p>").unwrap();
    reloaded(&mut events, "p2").await;
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| {
        w.is_some_and(|w| w.watching == at(&site) && !w.waiting)
    })
    .await;

    // Removed, it waits again; made again, a change inside it reloads.
    std::fs::remove_dir_all(&site).unwrap();
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| {
        w.is_some_and(|w| w.waiting)
    })
    .await;
    std::fs::create_dir(&site).unwrap();
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| {
        w.is_some_and(|w| !w.waiting)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await {}
    std::fs::write(site.join("index.html"), "<p>two</p>").unwrap();
    reloaded(&mut events, "p2").await;

    // A change in the pane's own data directory sends the same reload.
    std::fs::write(data("p2").join("rows.json"), "{\"n\":1}").unwrap();
    reloaded(&mut events, "p2").await;

    // Both watches stop and the data directory goes when the pane changes kind, closes, or its
    // space closes.
    ok(&mut ws, terminal("p2")).await;
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| w.is_none()).await;
    watch_until(&mut ws, "p2", WatchRole::Data, |w| w.is_none()).await;
    assert!(
        !data("p2").exists(),
        "{} outlived the artifact the pane showed",
        data("p2").display()
    );
    std::fs::write(data("p2").join("rows.json"), "{\"n\":2}").unwrap_err();
    std::fs::write(site.join("index.html"), "<p>three</p>").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(600), reloaded(&mut events, "p2"))
            .await
            .is_err(),
        "a pane that is no longer web never reloads"
    );
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(3),
            pane: "p3".into(),
        },
    )
    .await;
    watch_until(&mut ws, "p3", WatchRole::Artifact, |w| w.is_none()).await;
    watch_until(&mut ws, "p3", WatchRole::Data, |w| w.is_none()).await;
    assert!(!data("p3").exists());
    ok(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(4),
            id: "s4".into(),
            if_empty: false,
        },
    )
    .await;
    ok(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(5),
            id: "s7".into(),
            if_empty: false,
        },
    )
    .await;
    assert!(watches(&mut ws).await.is_empty());
    // Closing the two spaces took every pane's data directory with it.
    for pane in ["p6", "p8"] {
        assert!(
            !data(pane).exists(),
            "{} outlived its space",
            data(pane).display()
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}

async fn refreshes(ws: &mut Ws) -> Vec<wheel_proto::RefreshInfo> {
    match call(ws, ClientMsg::Status { req: Some(97) }).await {
        ServerMsg::Status { refreshes, .. } => refreshes,
        other => panic!("expected status, got {other:?}"),
    }
}

/// Wait for the next permission request, whoever it is for.
async fn asked(ws: &mut Ws) -> wheel_proto::PermInfo {
    loop {
        if let ServerMsg::PermRequested { perm } = recv(ws).await {
            return perm;
        }
    }
}

/// Answer the one waiting request with `choice`, and say what it asked.
async fn answer(ws: &mut Ws, choice: &str) -> String {
    let perm = asked(ws).await;
    ok(
        ws,
        ClientMsg::PermAnswer {
            req: Some(90),
            id: perm.id.clone(),
            choice: choice.into(),
        },
    )
    .await;
    perm.title
}

/// Poll `status` until `pane`'s runner has made `want` runs, for up to 5s; return its last run.
async fn ran(ws: &mut Ws, pane: &str, want: u64) -> wheel_proto::RunInfo {
    for _ in 0..100 {
        let all = refreshes(ws).await;
        match all.iter().find(|r| r.pane == pane) {
            Some(r) if r.runs >= want => return r.last.clone().expect("a finished run"),
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("{pane} never made {want} runs: {:?}", refreshes(ws).await);
}

/// Send a request without waiting for its reply, for a caller that reads what follows itself:
/// `call` skips the broadcasts, so a test watching for one cannot also use it to send.
async fn send_only(ws: &mut Ws, msg: ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
}

/// Ask for a refresh of `pane` without waiting for the reply, so the caller can answer the
/// permission request it raises on another connection first.
async fn ask_refresh(ws: &mut Ws, pane: &str) {
    send_only(
        ws,
        ClientMsg::ArtifactRefresh {
            req: Some(80),
            pane: pane.into(),
        },
    )
    .await;
}

/// The `refresh` reply to an earlier `ask_refresh`, or the error that came instead.
async fn refresh_reply(ws: &mut Ws) -> Result<wheel_proto::RefreshInfo, String> {
    loop {
        match recv(ws).await {
            ServerMsg::Refresh { refresh, .. } => return Ok(refresh),
            ServerMsg::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
}

async fn trusted(ws: &mut Ws, msg: ClientMsg) -> Vec<wheel_proto::TrustInfo> {
    match call(ws, msg).await {
        ServerMsg::Trusted { trusted, .. } => trusted,
        other => panic!("expected trusted, got {other:?}"),
    }
}

/// A refresh command runs for the pane showing the artifact — on open and on demand — only once a
/// person has approved it, writes into the pane's own data directory, and leaves its exit status
/// and the tail of its error output readable. "Always" is remembered, and forgetting it brings the
/// request back.
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_runs_for_a_pane_once_a_person_approves_it() {
    let base = std::env::temp_dir().join(format!("wheel-refresh-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    // The command writes only where it is given to, named in its environment, and runs in the
    // space's directory — which is what `pwd` here proves.
    let entry = |command: &str| {
        register(
            &base,
            "dash",
            &dash,
            &format!(
                "title = \"Dash\"\ncommand = {}\n",
                toml::Value::from(command)
            ),
        )
    };
    entry("pwd > \"$WHEEL_DATA_DIR/where\" && printf %s \"$WHEEL_PANE\" > \"$WHEEL_DATA_DIR/rows.json\"");
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    // A second connection stands in for the person: the one asking for a refresh waits for its
    // reply, so the answer has to arrive from elsewhere, as it does from a client or `wheel perm`.
    let mut person = connect(&url).await;
    created(&mut ws, new_space(&base.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let data = instance_dir(Some(&base.join("spaces.json")))
        .join("panes")
        .join(&pane);
    let web = ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        artifact: Some("dash".into()),
        scope: None,
        url: None,
    };

    // Opening the pane asks, naming the command, the artifact and the space directory.
    ok(&mut ws, web.clone()).await;
    let title = answer(&mut person, "allow").await;
    assert!(title.contains("project artifact dash"), "{title}");
    assert!(
        title.contains(&base.to_string_lossy().to_string()),
        "{title}"
    );
    assert!(title.contains("WHEEL_DATA_DIR"), "{title}");
    let run = ran(&mut ws, &pane, 1).await;
    assert_eq!((run.exit, run.error.as_deref()), (Some(0), None), "{run:?}");
    assert!(run.ok());
    assert_eq!(
        std::fs::read_to_string(data.join("rows.json")).unwrap(),
        pane,
        "the command wrote the pane's own data directory"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("where"))
            .unwrap()
            .trim()
            .to_string(),
        base.canonicalize().unwrap().to_string_lossy().to_string(),
        "the command ran in the space's directory"
    );
    assert!(
        !dash.join("data").exists(),
        "nothing is written inside the artifact's folder"
    );
    let row = refreshes(&mut ws).await.remove(0);
    assert_eq!(
        (row.pane.as_str(), row.name.as_str()),
        (pane.as_str(), "dash")
    );
    assert_eq!(row.scope, ArtifactScope::Project);
    assert_eq!(
        (row.cwd, row.data_dir),
        (
            base.to_string_lossy().to_string(),
            data.to_string_lossy().to_string()
        )
    );
    assert_eq!(row.every, None);

    // One connection both asks for a refresh and answers the request it raises: the daemon keeps
    // reading this socket while the run waits on a person, so a client with one socket — which is
    // every client — is not deadlocked by its own refresh.
    ask_refresh(&mut ws, &pane).await;
    let perm = asked(&mut ws).await.id;
    send_only(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(91),
            id: perm.clone(),
            choice: "allow".into(),
        },
    )
    .await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(refresh.runs, 2);
    assert!(refresh.last.unwrap().ok());
    // The other connection saw the same request and its answer, so it is not still holding one.
    assert_eq!(resolved(&mut person, &perm).await, "allow");

    // Allowing once is not remembered: the next run asks again. Nothing is trusted yet.
    assert!(trusted(&mut ws, ClientMsg::ArtifactTrust { req: Some(70) })
        .await
        .is_empty());
    std::fs::remove_file(data.join("rows.json")).unwrap();
    ask_refresh(&mut ws, &pane).await;
    answer(&mut person, "always").await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(refresh.runs, 3);
    assert!(refresh.last.unwrap().ok());
    assert!(
        data.join("rows.json").exists(),
        "the on-demand run wrote it"
    );
    let all = trusted(&mut ws, ClientMsg::ArtifactTrust { req: Some(71) }).await;
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(
        (all[0].name.as_str(), all[0].dir.as_str()),
        ("dash", dash.to_string_lossy().as_ref())
    );
    assert!(all[0].command.contains("WHEEL_DATA_DIR"));
    assert!(
        std::fs::read_to_string(base.join("artifact-trust.json"))
            .unwrap()
            .contains("WHEEL_DATA_DIR"),
        "the approval is in the instance directory, not the layout"
    );
    assert!(
        !std::fs::read_to_string(base.join("spaces.json"))
            .unwrap()
            .contains("WHEEL_DATA_DIR"),
        "no command is in the saved layout"
    );

    // A trusted pair runs with no request at all: this one waits for its own reply with nobody
    // answering anything.
    ask_refresh(&mut ws, &pane).await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(refresh.runs, 4);
    assert!(refresh.last.unwrap().ok());

    // Editing the command in the entry asks again, and what runs is what the entry says now.
    entry("printf 'boom\\n' >&2; exit 3");
    ask_refresh(&mut ws, &pane).await;
    answer(&mut person, "allow").await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(refresh.command, "printf 'boom\\n' >&2; exit 3");
    let run = refresh.last.unwrap();
    assert_eq!((run.exit, run.error.as_deref()), (Some(3), None));
    assert_eq!(run.stderr_tail, "boom\n");
    assert!(!run.ok(), "a non-zero exit is a failed run");
    // The pane serves its static files throughout: a failed refresh is not a blank pane.
    let addr = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let (code, body) = http_get(
        &addr,
        &format!("/artifacts/{pane}/index.html?token={TOKEN}"),
        "",
    )
    .await;
    assert_eq!(code, 303, "{body}");

    // A refusal is the run's recorded result, and nothing ran.
    std::fs::remove_file(data.join("rows.json")).unwrap();
    entry("printf %s refused > \"$WHEEL_DATA_DIR/rows.json\"");
    ask_refresh(&mut ws, &pane).await;
    answer(&mut person, "deny").await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    let run = refresh.last.unwrap();
    assert_eq!(run.exit, None);
    assert_eq!(
        run.error.as_deref(),
        Some("the person refused this command")
    );
    assert!(!data.join("rows.json").exists(), "the refused command ran");

    // Forgetting brings the request back for the command that was remembered.
    entry("pwd > \"$WHEEL_DATA_DIR/where\" && printf %s \"$WHEEL_PANE\" > \"$WHEEL_DATA_DIR/rows.json\"");
    ask_refresh(&mut ws, &pane).await;
    let refresh = refresh_reply(&mut ws).await.unwrap();
    assert!(refresh.last.unwrap().ok(), "it is still remembered");
    let left = trusted(
        &mut ws,
        ClientMsg::ArtifactTrustForget {
            req: Some(72),
            name: "dash".into(),
        },
    )
    .await;
    assert!(left.is_empty(), "{left:?}");
    assert!(
        matches!(
            call(
                &mut ws,
                ClientMsg::ArtifactTrustForget {
                    req: Some(73),
                    name: "dash".into()
                }
            )
            .await,
            ServerMsg::Error { .. }
        ),
        "nothing is remembered for it now"
    );
    ask_refresh(&mut ws, &pane).await;
    answer(&mut person, "allow").await;
    assert!(refresh_reply(&mut ws).await.unwrap().last.unwrap().ok());

    // An artifact whose entry declares no command has no runner, and no row to report.
    register(&base, "dash", &dash, "title = \"Dash\"\n");
    ok(&mut ws, web.clone()).await;
    assert!(refreshes(&mut ws).await.is_empty());
    ask_refresh(&mut ws, &pane).await;
    assert!(refresh_reply(&mut ws)
        .await
        .unwrap_err()
        .contains("declares no command"));
    // Nor does a pane showing no artifact at all.
    entry("true");
    ok(&mut ws, web).await;
    assert_eq!(refreshes(&mut ws).await.len(), 1);
    ok(&mut ws, terminal(&pane)).await;
    assert!(
        refreshes(&mut ws).await.is_empty(),
        "the pane stopped showing it"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A browser pane carries the page it is on: it opens the homepage setting or a URL a caller
/// gives, follows the person as the client reports navigation, comes back on that page after a
/// restart, and refuses anything that is not an `http` or `https` URL. None of the artifact
/// machinery applies to it — no data directory, no watch, no refresh.
#[tokio::test(flavor = "multi_thread")]
async fn a_browser_pane_carries_the_page_it_is_on() {
    let base = std::env::temp_dir().join(format!("wheel-browser-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    // The homepage this project sets, which is what a pane with no URL opens.
    std::fs::write(
        project.join("wheel.toml"),
        "[browser]\nhomepage = \"https://home.example/start\"\n",
    )
    .unwrap();
    let state_file = base.join("spaces.json");
    let (url, serving) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&project.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let browser = |url: Option<&str>| ClientMsg::PaneSetKind {
        req: Some(20),
        pane: pane.clone(),
        kind: NewPaneKind::Browser,
        artifact: None,
        scope: None,
        url: url.map(str::to_string),
    };
    // What a client sends as the person navigates.
    let goto = |url: &str| ClientMsg::PaneBrowserUrl {
        req: Some(21),
        pane: pane.clone(),
        url: url.to_string(),
    };

    // With no URL, it opens the homepage as that space resolves it.
    ok(&mut ws, browser(None)).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        kind_of(&spaces[0], &pane),
        Some(PaneKind::Browser {
            url: "https://home.example/start".into(),
            extra: Default::default()
        })
    );
    // None of the artifact machinery is reached: a browser pane is not served by the daemon.
    assert!(
        watches(&mut ws).await.is_empty(),
        "no watch for a browser pane"
    );
    assert!(refreshes(&mut ws).await.is_empty(), "and no refresh");
    assert!(
        !instance_dir(Some(&state_file))
            .join("panes")
            .join(&pane)
            .exists(),
        "no data directory for a browser pane"
    );

    // A URL given at open is used as given, and a client reporting navigation moves the pane.
    ok(&mut ws, browser(Some("https://example.com/one"))).await;
    ok(&mut ws, goto("https://example.com/two?q=1#x")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        kind_of(&spaces[0], &pane),
        Some(PaneKind::Browser {
            url: "https://example.com/two?q=1#x".into(),
            extra: Default::default()
        })
    );

    // A file on this machine is a page too, on both paths.
    ok(&mut ws, browser(Some("file:///tmp/notes/index.html"))).await;
    ok(&mut ws, goto("file:///Users/me/a%20b.html")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        browser_url(&spaces[0], &pane),
        "file:///Users/me/a%20b.html"
    );
    ok(&mut ws, goto("https://example.com/two?q=1#x")).await;

    // Everything that is not a page a pane may open is refused, by both the opening and the
    // navigating path, and the pane stays where it was.
    for bad in [
        "file://otherhost/etc/passwd",
        "data:text/html,<p>x",
        "javascript:alert(1)",
        "example.com",
        "https://",
        "",
    ] {
        assert!(
            matches!(
                call(&mut ws, browser(Some(bad))).await,
                ServerMsg::Error { .. }
            ),
            "opening {bad:?}"
        );
        assert!(
            matches!(call(&mut ws, goto(bad)).await, ServerMsg::Error { .. }),
            "navigating to {bad:?}"
        );
    }
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        browser_url(&spaces[0], &pane),
        "https://example.com/two?q=1#x",
        "a refusal leaves the pane on its page"
    );

    // Text typed into the address field is read by the daemon: it moves the pane, and the reply
    // names the page, even when the pane is already on it, so the asking client goes there again.
    let typed = |text: &str| ClientMsg::PaneBrowserOpen {
        req: Some(22),
        pane: pane.clone(),
        text: text.to_string(),
    };
    for _ in 0..2 {
        let reply = call(&mut ws, typed("example.com/a b")).await;
        assert!(
            matches!(&reply, ServerMsg::Opened { url, .. } if url == "https://example.com/a%20b"),
            "{reply:?}"
        );
    }
    let ServerMsg::Error { message, .. } = call(&mut ws, typed("mailto:a@b.com")).await else {
        panic!("a scheme a pane does not open is refused");
    };
    assert!(message.contains("mailto:a@b.com"), "{message}");
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(browser_url(&spaces[0], &pane), "https://example.com/a%20b");
    ok(&mut ws, goto("https://example.com/two?q=1#x")).await;

    // `url` belongs to a browser pane and nothing else; a pane that is not one cannot navigate.
    let msg = ClientMsg::PaneSetKind {
        req: Some(24),
        pane: pane.clone(),
        kind: NewPaneKind::Terminal,
        artifact: None,
        scope: None,
        url: Some("https://example.com".into()),
    };
    assert!(
        matches!(call(&mut ws, msg).await, ServerMsg::Error { message, .. } if message.contains("url is only for a browser pane")),
        "a terminal takes no url"
    );
    ok(&mut ws, terminal(&pane)).await;
    assert!(
        matches!(call(&mut ws, goto("https://example.com")).await, ServerMsg::Error { message, .. } if message.contains("is not a browser pane")),
        "a terminal pane does not navigate"
    );

    // A pane that was showing an artifact and becomes a browser gives up everything the artifact
    // had: the watch rows, the data directory, the refresh runner. A pane that was never a web
    // pane would pass those assertions without the drop having happened.
    let artifact = base.join("proj/.wheel/artifacts/dash");
    std::fs::create_dir_all(&artifact).unwrap();
    std::fs::write(artifact.join("index.html"), "<p>dash</p>").unwrap();
    register(&base.join("proj"), "dash", &artifact, "title = \"Dash\"\n");
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(26),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("dash".into()),
            scope: None,
            url: None,
        },
    )
    .await;
    let data = instance_dir(Some(&state_file)).join("panes").join(&pane);
    assert!(data.is_dir(), "the web pane has its data directory");
    assert_eq!(watches(&mut ws).await.len(), 2, "and both its watches");
    ok(&mut ws, browser(Some("https://after.example"))).await;
    assert!(
        watches(&mut ws).await.is_empty(),
        "the watches went with the artifact"
    );
    assert!(!data.exists(), "and so did the data directory");

    // Unknown fields on a browser pane are kept, like any pane's: a newer build's extra field
    // survives this build reading the layout and writing it back.
    let saved = std::fs::read_to_string(&state_file).unwrap();
    let doctored = saved.replace(
        "\"kind\": \"browser\"",
        "\"kind\": \"browser\",\n        \"zoom\": 1.5",
    );
    assert_ne!(saved, doctored, "the browser pane is in the file");
    std::fs::write(&state_file, &doctored).unwrap();
    // And a page this build would refuse is a pane it cannot draw, not a pane it draws anyway.
    let hostile = base.join("hostile.json");
    std::fs::write(
        &hostile,
        doctored.replace("https://after.example", "javascript:alert(1)"),
    )
    .unwrap();
    serving.abort();
    let (url, hostile_serving) = start_daemon(Some(hostile.clone())).await;
    let mut hs = connect(&url).await;
    let (spaces, _, _) = layout(&mut hs).await;
    match kind_of(&spaces[0], &pane) {
        Some(PaneKind::Unknown(raw)) => assert_eq!(raw.kind(), "browser"),
        other => panic!("a page it would refuse is not a browser pane it draws: {other:?}"),
    }
    hostile_serving.abort();
    let (url, serving) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(browser_url(&spaces[0], &pane), "https://after.example");
    // Navigating keeps a field this build does not read — it changes the page, not the pane's
    // kind — so a newer build's own field survives this one driving the pane. (Changing the kind
    // drops it, with everything else the old kind held.)
    ok(&mut ws, goto("https://after.example/two")).await;
    // A navigation's save is coalesced (wheel-a8v), so this waits for the write rather than
    // assuming it has happened.
    let after = saved_when(&state_file, "https://after.example/two").await;
    assert!(
        after.contains("zoom"),
        "a field this build does not read is written back: {after}"
    );

    // The page survives a restart: it is in the layout, and nothing has to be started again for it.
    ok(&mut ws, browser(Some("https://kept.example/page"))).await;
    serving.abort();
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        kind_of(&spaces[0], &pane),
        Some(PaneKind::Browser {
            url: "https://kept.example/page".into(),
            extra: Default::default()
        }),
        "a restored browser pane comes back on its page"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A browser pane's page changes as often as a person loads one, so a navigation tells clients
/// the one field rather than the whole layout, and the state file is written once per burst with
/// the last page in it.
#[tokio::test(flavor = "multi_thread")]
async fn browsing_tells_clients_one_field_and_writes_the_file_once_per_burst() {
    let base = std::env::temp_dir().join(format!("wheel-browsing-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let state_file = base.join("spaces.json");
    let (url, serving) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let mut other = connect(&url).await;
    created(&mut ws, new_space(&project.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(30),
            pane: pane.clone(),
            kind: NewPaneKind::Browser,
            artifact: None,
            scope: None,
            url: Some("https://example.com/0".into()),
        },
    )
    .await;
    // The second connection has seen the layout up to here; what follows is only navigation.
    settled(&mut other).await;
    // The daemon counts its own writes, which is what makes "once per burst" an assertion rather
    // than a guess about how fast this machine is.
    let before = saves(&mut ws).await;

    // Twenty pages in quick succession, as a person clicking through a site reports them.
    for i in 1..=20 {
        ok(
            &mut ws,
            ClientMsg::PaneBrowserUrl {
                req: Some(31),
                pane: pane.clone(),
                url: format!("https://example.com/{i}"),
            },
        )
        .await;
    }
    // Each one told the other client the field, and nothing was told the whole layout for it.
    let mut told = Vec::new();
    for _ in 1..=20 {
        match recv(&mut other).await {
            ServerMsg::PaneUrl { pane: which, url } => {
                assert_eq!(which, pane);
                told.push(url);
            }
            ServerMsg::Spaces { .. } => panic!("a navigation sent the whole layout"),
            _ => {}
        }
    }
    assert_eq!(told.len(), 20, "every navigation reached the other client");
    assert_eq!(told.last().unwrap(), "https://example.com/20");

    // Twenty navigations are one write, and the page in the file is the last one reported rather
    // than the one that scheduled the write.
    let saved = saved_when(&state_file, "https://example.com/20").await;
    assert!(!saved.contains("example.com/19"), "{saved}");
    let after = saves(&mut ws).await;
    assert_eq!(
        after - before,
        1,
        "twenty navigations wrote the layout {} times",
        after - before
    );

    // And a restart comes back on it.
    serving.abort();
    let (url, _) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(
        browser_url(&spaces[0], &pane),
        "https://example.com/20",
        "a restored pane comes back on the last page reported"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The state file once it holds `needle`, or a panic saying what it held instead. A browser
/// pane's page is written a moment after it changes, so a reader has to wait for it.
async fn saved_when(path: &std::path::Path, needle: &str) -> String {
    for _ in 0..60 {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.contains(needle) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "{} never held {needle:?}: {}",
        path.display(),
        std::fs::read_to_string(path).unwrap_or_default()
    );
}

/// How many times the daemon has written its layout.
async fn saves(ws: &mut Ws) -> u64 {
    match call(ws, ClientMsg::Status { req: Some(99) }).await {
        ServerMsg::Status { layout_saves, .. } => layout_saves,
        other => panic!("expected status, got {other:?}"),
    }
}

/// Read frames until nothing has arrived for a moment, so a later `recv` sees only what follows.
async fn settled(ws: &mut Ws) {
    while tokio::time::timeout(Duration::from_millis(200), ws.next())
        .await
        .is_ok()
    {}
}

/// What `pane` shows in `space`.
fn kind_of(space: &SpaceInfo, pane: &str) -> Option<PaneKind> {
    fn find(node: &Node, pane: &str) -> Option<PaneKind> {
        match node {
            Node::Pane { id, kind, .. } if id == pane => Some(kind.clone()),
            Node::Pane { .. } => None,
            Node::Split { a, b, .. } => find(a, pane).or_else(|| find(b, pane)),
        }
    }
    find(&space.root, pane)
}

/// The page `pane` is on, for a pane that is a browser pane. Panics for anything else, because a
/// test asking a pane which page it is on has already asserted that it is a browser.
fn browser_url(space: &SpaceInfo, pane: &str) -> String {
    match kind_of(space, pane) {
        Some(PaneKind::Browser { url, .. }) => url,
        other => panic!("{pane} is not a browser pane: {other:?}"),
    }
}

/// Every setting resolves a project's `wheel.toml` over the machine's over wheel's own default,
/// and says which of the three a value came from. A write keeps the rest of the file, reaches
/// every connection, and is refused — without touching the file — for a key wheel does not have
/// or a value the key does not take.
#[tokio::test(flavor = "multi_thread")]
async fn settings_resolve_project_over_machine_over_default() {
    let base = std::env::temp_dir().join(format!("wheel-settings-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let machine_file = instance_dir(Some(&base.join("spaces.json"))).join("wheel.toml");
    let project_file = project.join("wheel.toml");
    let mut ws = connect(&url).await;
    // A second connection: a write reaches every client, not only the one that asked.
    let mut other = connect(&url).await;
    created(&mut ws, new_space(&project.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let space = spaces[0].id.clone();
    let home = "browser.homepage";

    // With neither file written, every value is wheel's own.
    let all = settings_of(&mut ws, None).await;
    let one = all.iter().find(|s| s.key == home).expect("the key exists");
    assert_eq!(
        (one.value.as_str(), one.origin, one.default.as_str()),
        ("about:blank", SettingOrigin::Default, "about:blank")
    );
    assert!(!one.help.is_empty(), "a key carries its own help: {one:?}");

    // A machine-wide write is what a space with no file of its own falls back to.
    let all = set_setting(&mut ws, home, "https://machine.example", None, true)
        .await
        .unwrap();
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://machine.example", SettingOrigin::Machine)
    );
    assert!(machine_file.is_file(), "it wrote the instance directory");
    assert!(
        !project_file.exists(),
        "and not the project: {}",
        project_file.display()
    );
    // The other connection was told, and the list is labelled with the space it was resolved for
    // — the one being shown — rather than with the layer that was written.
    let (space_told, told) = settings_broadcast(&mut other).await;
    assert_eq!(space_told.as_deref(), Some(space.as_str()));
    assert_eq!(
        told.iter().find(|s| s.key == home).unwrap().value,
        "https://machine.example"
    );

    // A project write wins for that space, and leaves the machine value in force for another.
    std::fs::write(&project_file, "# a person's own file\nother = 1\n").unwrap();
    let all = set_setting(
        &mut ws,
        home,
        "https://project.example",
        Some(&space),
        false,
    )
    .await
    .unwrap();
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://project.example", SettingOrigin::Project)
    );
    // Each layer's own value travels with the one in force, so a reader sees what the project's
    // value hides.
    assert_eq!(
        (one.project.as_deref(), one.machine.as_deref()),
        (
            Some("https://project.example"),
            Some("https://machine.example")
        )
    );
    let written = std::fs::read_to_string(&project_file).unwrap();
    assert!(
        written.contains("other = 1"),
        "the rest of the file survives: {written:?}"
    );
    assert!(written.contains("https://project.example"), "{written:?}");
    let (space_told, _) = settings_broadcast(&mut other).await;
    assert_eq!(space_told.as_deref(), Some(space.as_str()));
    // A second space, which has no file of its own, still falls back to the machine value.
    let second = base.join("other");
    std::fs::create_dir_all(&second).unwrap();
    created(&mut ws, new_space(&second.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let elsewhere = spaces
        .iter()
        .find(|s| s.cwd.ends_with("other"))
        .expect("the second space")
        .id
        .clone();
    let all = settings_of(&mut ws, Some(&elsewhere)).await;
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://machine.example", SettingOrigin::Machine)
    );

    // A key wheel does not have, and a value the key does not take, are both refused — and the
    // file they would have been written to is left exactly as it was.
    let before = std::fs::read_to_string(&project_file).unwrap();
    for (key, value) in [
        ("browser.homepge", "https://typo.example"),
        (home, "example.com"),
        (home, "file://otherhost/etc/passwd"),
        (home, "javascript:alert(1)"),
        (home, ""),
    ] {
        let e = set_setting(&mut ws, key, value, Some(&space), false)
            .await
            .expect_err(&format!("{key}={value:?} is refused"));
        assert!(!e.is_empty(), "{key}={value:?}");
        assert_eq!(
            std::fs::read_to_string(&project_file).unwrap(),
            before,
            "{key}={value:?} wrote the file"
        );
    }

    // A write with no space named goes to the space being shown, and says so: the broadcast and
    // the reply both carry the space the list was resolved for, never "no space".
    let all = set_setting(&mut ws, home, "https://noname.example", None, false)
        .await
        .unwrap();
    assert_eq!(
        all.iter().find(|s| s.key == home).unwrap().value,
        "https://noname.example"
    );
    let (space_told, told) = settings_broadcast(&mut other).await;
    assert_eq!(
        space_told.as_deref(),
        Some(elsewhere.as_str()),
        "the second space is the one showing now, so that is what it resolved for"
    );
    assert_eq!(
        told.iter().find(|s| s.key == home).unwrap().origin,
        SettingOrigin::Project,
        "it wrote the showing space's own file: {told:?}"
    );
    assert!(
        base.join("other/wheel.toml").is_file(),
        "the write went to the space being shown"
    );

    // A machine-wide write is told the truth about where it landed: the list comes back resolved
    // for the space in context, so a project value still showing as `project` is visible rather
    // than the machine value being reported as though it had won.
    let all = set_setting(&mut ws, home, "https://second-machine.example", None, true)
        .await
        .unwrap();
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://noname.example", SettingOrigin::Project),
        "the project file of the showing space still wins: {one:?}"
    );
    let (space_told, _) = settings_broadcast(&mut other).await;
    assert_eq!(space_told.as_deref(), Some(elsewhere.as_str()));
    // And the space that has no project value does see the new machine value.
    let all = settings_of(&mut ws, Some(&space)).await;
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(one.value, "https://project.example", "s1 sets its own");

    // A space id nothing has is refused, rather than quietly answered with the machine view.
    let msg = ClientMsg::SettingsLs {
        req: Some(62),
        space: Some("s999".into()),
    };
    assert!(
        matches!(call(&mut ws, msg).await, ServerMsg::Error { message, .. } if message.contains("no space s999")),
        "a mistyped space is refused"
    );

    // A value a person wrote by hand that the key does not take is ignored, and the reader is told
    // why rather than being handed it: the next slice opens this page.
    std::fs::write(
        &project_file,
        "[browser]\nhomepage = \"javascript:alert(1)\"\n",
    )
    .unwrap();
    let all = settings_of(&mut ws, Some(&space)).await;
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://second-machine.example", SettingOrigin::Machine),
        "a refused value falls back: {one:?}"
    );
    let problem = one.problem.as_deref().unwrap_or_default();
    assert!(
        problem.contains("javascript:alert(1)") && problem.contains("wheel.toml"),
        "the reader is told which file and what is wrong: {problem:?}"
    );
    // The same for a value of the wrong type.
    std::fs::write(&project_file, "[browser]\nhomepage = 5\n").unwrap();
    let all = settings_of(&mut ws, Some(&space)).await;
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(one.origin, SettingOrigin::Machine);
    assert!(
        one.problem
            .as_deref()
            .unwrap_or_default()
            .contains("integer"),
        "{one:?}"
    );

    // A key whose parent in the file is not a table is refused rather than replacing what is there.
    std::fs::write(&project_file, "browser = 1\n").unwrap();
    let e = set_setting(&mut ws, home, "https://parent.example", Some(&space), false)
        .await
        .expect_err("browser is not a table");
    assert!(e.contains("not a table"), "{e}");
    assert_eq!(
        std::fs::read_to_string(&project_file).unwrap(),
        "browser = 1\n"
    );

    // A damaged project file refuses the write rather than replacing the keys it could not read,
    // and a reader falls back to the machine value instead of failing.
    std::fs::write(&project_file, "{ not toml").unwrap();
    let e = set_setting(&mut ws, home, "https://after.example", Some(&space), false)
        .await
        .expect_err("a file this cannot parse refuses the write");
    assert!(e.contains("not valid TOML"), "{e}");
    assert_eq!(
        std::fs::read_to_string(&project_file).unwrap(),
        "{ not toml",
        "a file this cannot read is left alone"
    );
    let all = settings_of(&mut ws, Some(&space)).await;
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("https://second-machine.example", SettingOrigin::Machine)
    );

    // A removal from a file this cannot parse is refused the same way, and leaves it alone.
    let e = unset_setting(&mut ws, home, Some(&space), false)
        .await
        .expect_err("a file this cannot parse refuses the removal");
    assert!(e.contains("not valid TOML"), "{e}");
    assert_eq!(
        std::fs::read_to_string(&project_file).unwrap(),
        "{ not toml"
    );

    // Removing the project's value brings the machine's back, keeps the rest of the file, drops
    // the table it emptied, and reaches every connection.
    std::fs::write(
        &project_file,
        "other = 1\n[browser]\nhomepage = \"https://p.example\"\n",
    )
    .unwrap();
    let all = unset_setting(&mut ws, home, Some(&space), false)
        .await
        .unwrap();
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin, one.project.as_deref()),
        (
            "https://second-machine.example",
            SettingOrigin::Machine,
            None
        )
    );
    let left = std::fs::read_to_string(&project_file).unwrap();
    assert!(
        left.contains("other = 1") && !left.contains("browser"),
        "{left:?}"
    );
    loop {
        let (space_told, told) = settings_broadcast(&mut other).await;
        if told
            .iter()
            .find(|s| s.key == home)
            .unwrap()
            .project
            .is_none()
        {
            assert_eq!(space_told.as_deref(), Some(space.as_str()));
            break;
        }
    }
    // Removing it again changes nothing and is no error.
    unset_setting(&mut ws, home, Some(&space), false)
        .await
        .expect("removing a key the file does not set is not an error");
    assert_eq!(std::fs::read_to_string(&project_file).unwrap(), left);
    // Removing the machine's value leaves wheel's default.
    let all = unset_setting(&mut ws, home, Some(&space), true)
        .await
        .unwrap();
    let one = all.iter().find(|s| s.key == home).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin, one.machine.as_deref()),
        ("about:blank", SettingOrigin::Default, None)
    );
    // A machine change naming a space nothing has is refused before this machine's file changes.
    let machine_before = std::fs::read_to_string(&machine_file).unwrap_or_default();
    for e in [
        set_setting(&mut ws, home, "https://ghost.example", Some("s999"), true).await,
        unset_setting(&mut ws, home, Some("s999"), true).await,
    ] {
        assert!(e
            .expect_err("an unknown space is refused")
            .contains("no space s999"));
    }
    assert_eq!(
        std::fs::read_to_string(&machine_file).unwrap_or_default(),
        machine_before
    );
    // An unknown key is refused naming the keys there are.
    let e = unset_setting(&mut ws, "browser.homepge", Some(&space), false)
        .await
        .expect_err("an unknown key is refused");
    assert!(e.contains("browser.homepage"), "{e}");

    // Nothing about settings is in the saved layout.
    let saved = std::fs::read_to_string(base.join("spaces.json")).unwrap();
    assert!(!saved.contains("homepage"), "{saved}");
    let _ = std::fs::remove_dir_all(&base);
}

/// Every setting as the daemon resolves it for `space`.
async fn settings_of(ws: &mut Ws, space: Option<&str>) -> Vec<wheel_proto::SettingInfo> {
    let msg = ClientMsg::SettingsLs {
        req: Some(60),
        space: space.map(str::to_string),
    };
    match call(ws, msg).await {
        ServerMsg::Settings { settings, .. } => settings,
        other => panic!("expected settings, got {other:?}"),
    }
}

/// `terminal.shell` takes only an absolute path to an executable file: a write of anything else is
/// refused with the reason, and a hand-written value that fails the same check is reported on the
/// key while the default stays in force.
#[tokio::test]
async fn terminal_shell_is_accepted_only_as_an_executable_path() {
    let base = std::env::temp_dir().join(format!("wheel-shell-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let machine_file = instance_dir(Some(&base.join("spaces.json"))).join("wheel.toml");
    let mut ws = connect(&url).await;
    let key = "terminal.shell";

    let all = settings_of(&mut ws, None).await;
    let one = all.iter().find(|s| s.key == key).expect("the key exists");
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("", SettingOrigin::Default)
    );

    for (bad, why) in [
        ("zsh", "absolute"),
        ("/nonexistent/shell", "cannot be read"),
        ("/etc", "not a file"),
        ("/etc/hosts", "not executable"),
    ] {
        let e = set_setting(&mut ws, key, bad, None, true)
            .await
            .unwrap_err();
        assert!(e.contains(why), "{bad:?}: {e}");
    }
    assert!(!machine_file.exists(), "a refused value writes nothing");

    let all = set_setting(&mut ws, key, "/bin/sh", None, true)
        .await
        .unwrap();
    let one = all.iter().find(|s| s.key == key).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("/bin/sh", SettingOrigin::Machine)
    );

    std::fs::write(&machine_file, "[terminal]\nshell = \"/etc/hosts\"\n").unwrap();
    let all = settings_of(&mut ws, None).await;
    let one = all.iter().find(|s| s.key == key).unwrap();
    assert_eq!(
        (one.value.as_str(), one.origin),
        ("", SettingOrigin::Default),
        "a hand-written value that fails the check is ignored: {one:?}"
    );
    let problem = one.problem.as_deref().unwrap_or_default();
    assert!(problem.contains("not executable"), "{problem:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// Write one setting; the settings it resolves to afterwards, or the daemon's refusal.
async fn set_setting(
    ws: &mut Ws,
    key: &str,
    value: &str,
    space: Option<&str>,
    machine: bool,
) -> Result<Vec<wheel_proto::SettingInfo>, String> {
    let msg = ClientMsg::SettingSet {
        req: Some(61),
        key: key.to_string(),
        value: value.to_string(),
        space: space.map(str::to_string),
        machine,
    };
    match call(ws, msg).await {
        ServerMsg::Settings { settings, .. } => Ok(settings),
        ServerMsg::Error { message, .. } => Err(message),
        other => panic!("expected settings or an error, got {other:?}"),
    }
}

/// Remove one setting; the settings it resolves to afterwards, or the daemon's refusal.
async fn unset_setting(
    ws: &mut Ws,
    key: &str,
    space: Option<&str>,
    machine: bool,
) -> Result<Vec<wheel_proto::SettingInfo>, String> {
    let msg = ClientMsg::SettingUnset {
        req: Some(63),
        key: key.to_string(),
        space: space.map(str::to_string),
        machine,
    };
    match call(ws, msg).await {
        ServerMsg::Settings { settings, .. } => Ok(settings),
        ServerMsg::Error { message, .. } => Err(message),
        other => panic!("expected settings or an error, got {other:?}"),
    }
}

/// The next settings broadcast: the space it was resolved for, and the settings.
async fn settings_broadcast(ws: &mut Ws) -> (Option<String>, Vec<wheel_proto::SettingInfo>) {
    loop {
        if let ServerMsg::Settings {
            req: None,
            space,
            settings,
        } = recv(ws).await
        {
            return (space, settings);
        }
    }
}

/// `every` runs the refresh on a timer for the pane showing the artifact, through the same runner
/// the open and on-demand paths use: the run count climbs with nobody asking, a run that outlasts
/// its own interval is not overlapped by the next tick, a command that stops being remembered as
/// approved makes every tick skip rather than raising a request nobody expects, and approving it
/// again resumes the timer.
#[tokio::test(flavor = "multi_thread")]
async fn every_runs_the_refresh_on_a_timer() {
    let base = std::env::temp_dir().join(format!("wheel-timer-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let clock = base.join(".wheel/artifacts/clock");
    std::fs::create_dir_all(&clock).unwrap();
    std::fs::write(clock.join("index.html"), "<p>clock</p>").unwrap();
    // Each run marks its own start and end in one file, and takes longer than the interval asks
    // for, so two runs overlapping would interleave those marks.
    let command =
        "printf '(' >> \"$WHEEL_DATA_DIR/log\"; sleep 1.2; printf ')' >> \"$WHEEL_DATA_DIR/log\"";
    register(
        &base,
        "clock",
        &clock,
        &format!(
            "title = \"Clock\"\nevery = 1\ncommand = {}\n",
            toml::Value::from(command)
        ),
    );
    // The pair is remembered before the daemon starts, so the timer has an approved command from
    // its first tick: a tick never asks, and nothing here answers a request.
    let trust = wheel_proto::TrustInfo {
        dir: clock.to_string_lossy().to_string(),
        cwd: base.to_string_lossy().to_string(),
        command: command.to_string(),
        scope: ArtifactScope::Project,
        name: "clock".into(),
        approved_ms: 1,
        reads: Vec::new(),
    };
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("artifact-trust.json"),
        serde_json::json!({ "trusted": [trust] }).to_string(),
    )
    .unwrap();

    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let mut person = connect(&url).await;
    created(&mut ws, new_space(&base.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let data = instance_dir(Some(&base.join("spaces.json")))
        .join("panes")
        .join(&pane);
    let web = ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        artifact: Some("clock".into()),
        scope: None,
        url: None,
    };
    ok(&mut ws, web.clone()).await;

    // Three runs with nobody asking for one: the opening run, then a tick every second.
    let first = ran(&mut ws, &pane, 1).await;
    ran(&mut ws, &pane, 3).await;
    let row = refreshes(&mut ws)
        .await
        .into_iter()
        .find(|r| r.pane == pane)
        .expect("a row for the refreshing pane");
    assert_eq!((row.every, row.timer), (Some(1), TimerState::Ticking));
    assert!(row.runs >= 3, "{row:?}");
    let last = row.last.clone().unwrap();
    assert!(
        last.started_ms > first.started_ms,
        "the last run's time advances: {} then {}",
        first.started_ms,
        last.started_ms
    );
    assert!(
        std::fs::read_to_string(data.join("log"))
            .unwrap()
            .replace("()", "")
            .is_empty(),
        "every run finished before the next began: {:?}",
        std::fs::read_to_string(data.join("log"))
    );
    assert!(
        matches!(
            call(&mut ws, ClientMsg::Status { req: Some(80) }).await,
            ServerMsg::Status { perms, .. } if perms.is_empty()
        ),
        "no tick raised a request"
    );

    // Forgetting the approval does not end the timer: its ticks skip, saying why, and nothing is
    // asked of anybody. A skipped tick is not a run, so the count stays where the runs left it.
    let left = trusted(
        &mut ws,
        ClientMsg::ArtifactTrustForget {
            req: Some(72),
            name: "clock".into(),
        },
    )
    .await;
    assert!(left.is_empty(), "{left:?}");
    let stalled = loop {
        let row = refreshes(&mut ws)
            .await
            .into_iter()
            .find(|r| r.pane == pane)
            .unwrap();
        if row.timer == TimerState::Untrusted {
            break row.runs;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let row = refreshes(&mut ws)
        .await
        .into_iter()
        .find(|r| r.pane == pane)
        .unwrap();
    assert_eq!(
        (row.runs, row.timer),
        (stalled, TimerState::Untrusted),
        "two intervals of skipped ticks ran nothing"
    );

    // Approving it again, at the on-demand refresh that does ask, resumes the timer: the pane never
    // had to be reopened.
    ask_refresh(&mut ws, &pane).await;
    answer(&mut person, "always").await;
    let on_demand = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(
        on_demand.timer,
        TimerState::Ticking,
        "it runs unattended now"
    );
    assert!(on_demand.last.unwrap().ok());
    let going = ran(&mut ws, &pane, on_demand.runs + 1).await;

    // An entry that stops asking for a timer stops the runs without ending the timer, and asking
    // for one again resumes it — neither needs the pane reopened.
    let every = |secs: Option<u64>| {
        let mut text = format!(
            "title = \"Clock\"\ncommand = {}\n",
            toml::Value::from(command)
        );
        if let Some(secs) = secs {
            text.push_str(&format!("every = {secs}\n"));
        }
        register(&base, "clock", &clock, &text);
    };
    every(None);
    let idled = loop {
        let row = refreshes(&mut ws)
            .await
            .into_iter()
            .find(|r| r.pane == pane)
            .unwrap();
        if row.timer == TimerState::Idle {
            break row.runs;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let row = refreshes(&mut ws)
        .await
        .into_iter()
        .find(|r| r.pane == pane)
        .unwrap();
    assert_eq!(
        (row.runs, row.every, row.timer),
        (idled, None, TimerState::Idle),
        "an entry asking for no timer runs nothing: {going:?}"
    );
    every(Some(1));
    ran(&mut ws, &pane, idled + 1).await;

    // A pane opened on an entry asking for no timer has none, and says so even once the entry
    // asks for one: that takes effect when the pane next shows the artifact, as a new `command`
    // does. The record never claims a timer this pane has not got.
    every(None);
    ok(&mut ws, web.clone()).await;
    ran(&mut ws, &pane, 1).await;
    every(Some(1));
    ask_refresh(&mut ws, &pane).await;
    let row = refresh_reply(&mut ws).await.unwrap();
    assert_eq!(
        (row.every, row.timer),
        (Some(1), TimerState::Off),
        "{row:?}"
    );
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let after = refreshes(&mut ws)
        .await
        .into_iter()
        .find(|r| r.pane == pane)
        .unwrap();
    assert_eq!(
        (after.runs, after.timer),
        (row.runs, TimerState::Off),
        "nothing ticks for a pane that opened without an interval"
    );

    // The pane stopping showing the artifact stops the timer with everything else it owns.
    ok(&mut ws, web).await;
    ran(&mut ws, &pane, 2).await;
    ok(&mut ws, terminal(&pane)).await;
    assert!(refreshes(&mut ws).await.is_empty());
    let stopped = std::fs::read_to_string(data.join("log")).unwrap_or_default();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        std::fs::read_to_string(data.join("log")).unwrap_or_default(),
        stopped,
        "no tick ran after the pane changed kind"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Wait for the request `id` to be resolved, and say how.
async fn resolved(ws: &mut Ws, id: &str) -> String {
    loop {
        if let ServerMsg::PermResolved { id: gone, choice } = recv(ws).await {
            if gone == id {
                return choice;
            }
        }
    }
}

/// A run belongs to the pane it refreshes: a pane that stops showing its artifact while a run waits
/// on a person withdraws the request and cancels the run, rather than leaving a command to be
/// approved later and run into a data directory that is gone.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_pane_cancels_a_refresh_waiting_on_a_person() {
    let base = std::env::temp_dir().join(format!("wheel-cancel-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    register(
        &base,
        "dash",
        &dash,
        "title = \"Dash\"\ncommand = \"printf %s ran > \\\"$WHEEL_DATA_DIR/rows.json\\\"\"\n",
    );
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let mut person = connect(&url).await;
    created(&mut ws, new_space(&base.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let data = instance_dir(Some(&base.join("spaces.json")))
        .join("panes")
        .join(&pane);
    let web = ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        artifact: Some("dash".into()),
        scope: None,
        url: None,
    };
    let empty = ClientMsg::PaneSetKind {
        req: Some(11),
        pane: pane.clone(),
        kind: NewPaneKind::Empty,
        artifact: None,
        scope: None,
        url: None,
    };

    // The pane's opening run asks, and nobody answers.
    ok(&mut ws, web.clone()).await;
    let opening = asked(&mut person).await;
    ok(&mut ws, empty.clone()).await;
    assert_eq!(resolved(&mut person, &opening.id).await, "withdrawn");
    assert!(refreshes(&mut ws).await.is_empty());
    assert!(
        !data.exists(),
        "the pane's data directory went with the pane"
    );
    // The request is gone, so nothing can answer it into a run later.
    assert!(matches!(
        call(
            &mut ws,
            ClientMsg::PermAnswer {
                req: Some(12),
                id: opening.id.clone(),
                choice: "allow".into()
            }
        )
        .await,
        ServerMsg::Error { .. }
    ));

    // The same for a run a caller asked for: it is cancelled, and the caller is told so rather
    // than being left waiting on a pane that no longer refreshes.
    ok(&mut ws, web).await;
    let opening = asked(&mut person).await;
    ask_refresh(&mut ws, &pane).await;
    // A reply on the same connection means the daemon has taken the request, which it does in
    // frame order; the other connection's change of kind could otherwise arrive first.
    refreshes(&mut ws).await;
    send_only(&mut person, empty).await;
    assert_eq!(resolved(&mut person, &opening.id).await, "withdrawn");
    let message = refresh_reply(&mut ws).await.unwrap_err();
    assert!(
        message.contains("stopped showing its artifact"),
        "{message}"
    );
    assert!(refreshes(&mut ws).await.is_empty());
    assert!(!data.join("rows.json").exists(), "the command never ran");
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread")]
async fn repointing_an_artifacts_entry_moves_the_open_panes_watch() {
    let base = std::env::temp_dir().join(format!("wheel-repoint-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let old = base.join("old");
    let new = base.join("new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    register(&base, "site", &old, "");
    let state_file = base.join("spaces.json");
    let saved = serde_json::json!({
        "spaces": [{"id": "s1", "name": "one", "cwd": base, "focus": "p2",
                    "root": {"node": "pane", "id": "p2", "kind": "web", "scope": "project", "name": "site"}}],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    let (url, _) = start_daemon(Some(state_file)).await;
    let mut ws = connect(&url).await;
    let mut events = connect(&url).await;
    let at = |path: &std::path::Path| Some(path.to_string_lossy().into_owned());
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| {
        w.is_some_and(|w| w.watching == at(&old))
    })
    .await;

    register(&base, "site", &new, "");
    watch_until(&mut ws, "p2", WatchRole::Artifact, |w| {
        w.is_some_and(|w| w.dir == new.to_string_lossy() && w.watching == at(&new) && !w.waiting)
    })
    .await;
    reloaded(&mut events, "p2").await;

    // The new folder's edits reload the pane; the old folder's no longer do.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await {}
    std::fs::write(old.join("index.html"), "<p>old</p>").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(800), reloaded(&mut events, "p2"))
            .await
            .is_err(),
        "the old folder still reloads the pane"
    );
    std::fs::write(new.join("index.html"), "<p>new</p>").unwrap();
    reloaded(&mut events, "p2").await;
    std::fs::remove_dir_all(&base).unwrap();
}

/// A tree as text: a pane is its id, a horizontal split `(a|b)`, a vertical one `(a/b)`.
fn shape(node: &Node) -> String {
    match node {
        Node::Pane { id, .. } => id.clone(),
        Node::Split { axis, a, b, .. } => {
            let bar = if *axis == Axis::Horizontal { '|' } else { '/' };
            format!("({}{bar}{})", shape(a), shape(b))
        }
    }
}

async fn root_shape(ws: &mut Ws) -> String {
    let (spaces, _, _) = layout(ws).await;
    shape(&spaces[0].root)
}

fn swap_msg(pane: &str, with: &str) -> ClientMsg {
    ClientMsg::PaneSwap {
        req: Some(7),
        pane: pane.into(),
        with: with.into(),
    }
}

fn move_msg(pane: &str, target: &str, dir: Dir) -> ClientMsg {
    ClientMsg::PaneMove {
        req: Some(8),
        pane: pane.into(),
        target: target.into(),
        dir,
    }
}

async fn split(ws: &mut Ws, pane: &str, dir: Dir) -> String {
    created(
        ws,
        ClientMsg::PaneSplit {
            req: Some(3),
            pane: pane.into(),
            dir,
            background: false,
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn panes_swap_and_move_to_every_side_and_keep_their_shells() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    created(&mut ws, new_space("/tmp")).await;
    // p1 | p3 / p4 — p1 on the left, p3 over p4 on the right.
    let p1 = {
        let (spaces, _, _) = layout(&mut ws).await;
        let Node::Pane { id, .. } = &spaces[0].root else {
            panic!("one pane")
        };
        id.clone()
    };
    ok(&mut ws, terminal(&p1)).await;
    let p3 = split(&mut ws, &p1, Dir::Right).await;
    let p4 = split(&mut ws, &p3, Dir::Down).await;
    let term_before = {
        let (spaces, _, _) = layout(&mut ws).await;
        term_of(&spaces[0], &p1).unwrap()
    };
    assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p3}/{p4}))"));

    // Swap: positions exchange, the shell goes with its pane.
    ok(&mut ws, swap_msg(&p1, &p4)).await;
    assert_eq!(root_shape(&mut ws).await, format!("({p4}|({p3}/{p1}))"));
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(term_of(&spaces[0], &p1).unwrap(), term_before);
    ok(&mut ws, swap_msg(&p1, &p4)).await;

    // Move out of a split: p4 leaves (p3/p4), which collapses to p3; p4 lands on each side of p1.
    for (dir, want) in [
        (Dir::Left, format!("(({p4}|{p1})|{p3})")),
        (Dir::Right, format!("(({p1}|{p4})|{p3})")),
        (Dir::Up, format!("(({p4}/{p1})|{p3})")),
        (Dir::Down, format!("(({p1}/{p4})|{p3})")),
    ] {
        ok(&mut ws, move_msg(&p4, &p1, dir)).await;
        assert_eq!(root_shape(&mut ws).await, want, "{dir:?}");
        // Put the tree back: p4 beneath p3.
        ok(&mut ws, move_msg(&p4, &p3, Dir::Down)).await;
        assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p3}/{p4}))"));
    }

    // Onto its own sibling: above p3 reverses them, above it again changes nothing.
    ok(&mut ws, move_msg(&p4, &p3, Dir::Up)).await;
    assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p4}/{p3}))"));
    ok(&mut ws, move_msg(&p4, &p3, Dir::Up)).await;
    assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p4}/{p3}))"));
    ok(&mut ws, move_msg(&p4, &p3, Dir::Down)).await;
    assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p3}/{p4}))"));

    // The shell survived every move.
    let (spaces, _, _) = layout(&mut ws).await;
    assert_eq!(term_of(&spaces[0], &p1).unwrap(), term_before);

    // Refusals leave the tree alone.
    for bad in [
        swap_msg(&p1, &p1),
        swap_msg(&p1, "p99"),
        move_msg(&p1, &p1, Dir::Left),
        move_msg(&p1, "p99", Dir::Left),
        move_msg("p99", &p1, Dir::Left),
    ] {
        assert!(matches!(call(&mut ws, bad).await, ServerMsg::Error { .. }));
    }
    assert_eq!(root_shape(&mut ws).await, format!("({p1}|({p3}/{p4}))"));
}

/// Everything `term` prints, read until `marker` appears or two seconds pass.
async fn term_text(ws: &mut Ws, term: &str, marker: &str) -> String {
    call(
        ws,
        ClientMsg::TermAttach {
            req: Some(60),
            id: term.into(),
        },
    )
    .await;
    let mut text = String::new();
    let read = async {
        loop {
            if let ServerMsg::TermOutput { id, data } = recv(ws).await {
                if id == term {
                    text.push_str(&String::from_utf8_lossy(
                        &wheel_proto::decode_bytes(&data).unwrap(),
                    ));
                    if text.contains(marker) {
                        return;
                    }
                }
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), read).await;
    text
}

const SESSION: &str = "11111111-2222-3333-4444-555555555555";

fn saved_terminals(dir: &std::path::Path, panes: serde_json::Value) -> PathBuf {
    let state_file = dir.join("spaces.json");
    let saved = serde_json::json!({
        "spaces": [{
            "id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p1",
            "root": panes,
        }],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();
    state_file
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_restores_into_its_last_directory_and_resumes_its_agent() {
    let base = std::env::temp_dir().join(format!("wheel-resume-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let work = base.join("work");
    let gone_dir = base.join("moved-away");
    let claude = base.join("claude-home");
    // Claude Code files a session under the directory it ran in, every non-alphanumeric
    // character of it turned into `-`.
    let project: String = work
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    std::fs::create_dir_all(claude.join("projects").join(&project)).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        claude
            .join("projects")
            .join(&project)
            .join(format!("{SESSION}.jsonl")),
        "{}",
    )
    .unwrap();
    let agent = |session: &str| serde_json::json!({"name": "claude", "session": session, "home": claude.to_string_lossy()});
    let pane = |id: &str, cwd: &std::path::Path, agent: serde_json::Value| {
        let mut p = serde_json::json!({"node": "pane", "id": id, "kind": "terminal", "term": "t1", "cwd": cwd.to_string_lossy()});
        if !agent.is_null() {
            p["agent"] = agent;
        }
        p
    };
    let split = |id: &str, a: serde_json::Value, b: serde_json::Value| serde_json::json!({"node": "split", "id": id, "axis": "horizontal", "ratio": 0.5, "a": a, "b": b});
    // p1 plain in a directory, p2 running a live session, p3 a session the agent has lost, p4 the
    // same live session again, p5 a directory that has gone.
    let root = split(
        "x1",
        pane("p1", &work, serde_json::Value::Null),
        split(
            "x2",
            pane("p2", &work, agent(SESSION)),
            split(
                "x3",
                pane("p3", &work, agent("99999999-2222-3333-4444-555555555555")),
                split(
                    "x4",
                    pane("p4", &work, agent(SESSION)),
                    pane("p5", &gone_dir, serde_json::Value::Null),
                ),
            ),
        ),
    );
    let state_file = saved_terminals(&base, root);

    let (url, _daemon) = start_daemon(Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (spaces, _, terms) = layout(&mut ws).await;
    let term = |pane: &str| term_of(&spaces[0], pane).expect("a terminal pane");
    let cwd_of = |pane: &str| {
        let id = term(pane);
        terms.iter().find(|t| t.id == id).unwrap().cwd.clone()
    };
    let work = work.to_string_lossy().to_string();
    for pane in ["p1", "p2", "p3", "p4"] {
        assert_eq!(cwd_of(pane), work, "{pane} opens in its last directory");
    }
    assert_eq!(
        cwd_of("p5"),
        "/tmp",
        "a directory that has gone falls back to the space's"
    );

    let resume = format!("--resume {SESSION}");
    assert!(term_text(&mut ws, &term("p2"), &resume)
        .await
        .contains(&resume));
    for plain in ["p1", "p3", "p4", "p5"] {
        let text = term_text(&mut ws, &term(plain), "--resume").await;
        assert!(
            !text.contains("--resume"),
            "{plain} types no resume command: {text:?}"
        );
    }

    // The pane keeps the agent it resumed and forgets one that could not be.
    let agent_of = |pane: &str| match find_kind(&spaces[0].root, pane) {
        Some(PaneKind::Terminal { agent, .. }) => agent.clone(),
        other => panic!("{pane} is {other:?}"),
    };
    assert_eq!(agent_of("p2").map(|a| a.session), Some(SESSION.to_string()));
    for pane in ["p1", "p3", "p4", "p5"] {
        assert_eq!(agent_of(pane), None, "{pane} resumes nothing");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// The agent in a terminal pane says what it is doing, and the pane and its space follow, the
/// way an agent pane's do. Only a terminal pane takes a report.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_pane_follows_what_its_agent_reports() {
    use wheel_proto::{PaneStatus, ReportedStatus};
    let base = std::env::temp_dir().join(format!("wheel-report-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // A saved agent is kept only while its session is in the agent's store.
    let work = base.join("work");
    let claude = base.join("claude-home");
    let project: String = work
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    std::fs::create_dir_all(claude.join("projects").join(&project)).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        claude
            .join("projects")
            .join(&project)
            .join(format!("{SESSION}.jsonl")),
        "{}",
    )
    .unwrap();
    let root = serde_json::json!({"node": "pane", "id": "p1", "kind": "terminal", "term": "t1",
        "cwd": work.to_string_lossy(),
        "agent": {"name": "claude", "session": SESSION, "home": claude.to_string_lossy()}});
    let state_file = saved_terminals(&base, root);
    let (url, _daemon) = start_daemon(Some(state_file)).await;
    let mut watcher = connect(&url).await;
    let mut ws = connect(&url).await;
    let report = |status| ClientMsg::PaneReport {
        req: Some(1),
        pane: "p1".into(),
        status,
    };
    for (reported, shown) in [
        (ReportedStatus::Working, PaneStatus::Working),
        (
            ReportedStatus::NeedsYou,
            PaneStatus::NeedsYou { waiting: 1 },
        ),
        (ReportedStatus::Idle, PaneStatus::Idle),
    ] {
        ok(&mut ws, report(reported)).await;
        loop {
            if let ServerMsg::StatusChanged {
                pane, space_status, ..
            } = recv(&mut watcher).await
            {
                if pane.as_deref() == Some("p1") {
                    assert_eq!(space_status, Some(shown), "the space follows its pane");
                    break;
                }
            }
        }
        let (spaces, _, _) = layout(&mut ws).await;
        assert_eq!(spaces[0].status, Some(shown));
    }
    let refused = call(
        &mut ws,
        ClientMsg::PaneReport {
            req: Some(2),
            pane: "p999".into(),
            status: ReportedStatus::Idle,
        },
    )
    .await;
    assert!(
        matches!(refused, ServerMsg::Error { ref message, .. } if message == "no pane p999"),
        "{refused:?}"
    );
    let context = wheel_proto::ContextUse {
        used: 261_267,
        size: 1_000_000,
    };
    ok(
        &mut ws,
        ClientMsg::PaneReportContext {
            req: Some(3),
            pane: "p1".into(),
            context,
        },
    )
    .await;
    let from_nowhere = call(
        &mut ws,
        ClientMsg::PaneReportSession {
            req: Some(5),
            pane: "p1".into(),
            session: "66666666-7777-8888-9999-000000000000".into(),
            pid: std::process::id(),
        },
    )
    .await;
    assert!(
        matches!(from_nowhere, ServerMsg::Error { ref message, .. } if message.contains("did not send")),
        "a report from a process under no Claude Code is refused: {from_nowhere:?}"
    );
    let refused = call(
        &mut ws,
        ClientMsg::PaneReportSession {
            req: Some(6),
            pane: "p999".into(),
            session: "66666666-7777-8888-9999-000000000000".into(),
            pid: std::process::id(),
        },
    )
    .await;
    assert!(
        matches!(refused, ServerMsg::Error { ref message, .. } if message == "no pane p999"),
        "{refused:?}"
    );
    let refused = call(
        &mut ws,
        ClientMsg::PaneReportContext {
            req: Some(4),
            pane: "p999".into(),
            context,
        },
    )
    .await;
    assert!(
        matches!(refused, ServerMsg::Error { ref message, .. } if message == "no pane p999"),
        "{refused:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// An edit to a named file that keeps its length and restores its modification time (`touch -r`)
/// leaves its stat as approved, yet the timer stops trusting the command at the next tick.
#[tokio::test(flavor = "multi_thread")]
async fn a_same_length_edit_with_the_mtime_restored_makes_the_timer_untrusted() {
    let base = std::env::temp_dir().join(format!("wheel-touchr-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let script = base.join("pull.sh");
    std::fs::write(&script, "echo aaaa\n").unwrap();
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    register(
        &base,
        "dash",
        &dash,
        "title = \"Dash\"\nevery = 1\ncommand = \"sh pull.sh > \\\"$WHEEL_DATA_DIR/out\\\"\"\n",
    );
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let mut person = connect(&url).await;
    created(&mut ws, new_space(&base.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(10),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("dash".into()),
            scope: None,
            url: None,
        },
    )
    .await;
    answer(&mut person, "always").await;
    ran(&mut ws, &pane, 2).await;
    let timer = |rows: Vec<wheel_proto::RefreshInfo>| {
        rows.into_iter().find(|r| r.pane == pane).unwrap().timer
    };
    assert_eq!(timer(refreshes(&mut ws).await), TimerState::Ticking);

    // Same length, mtime put back with `touch -r` from a copy taken before the edit.
    let reference = base.join("pull.sh.ref");
    std::fs::copy(&script, &reference).unwrap();
    let mtime = std::fs::metadata(&script).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&reference)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    std::fs::write(&script, "echo bbbb\n").unwrap();
    let status = std::process::Command::new("touch")
        .arg("-r")
        .arg(&reference)
        .arg(&script)
        .status()
        .unwrap();
    assert!(status.success());
    let meta = std::fs::metadata(&script).unwrap();
    assert_eq!(meta.len(), 10);
    assert_eq!(meta.modified().unwrap(), mtime, "the stat is as approved");

    let mut untrusted = false;
    for _ in 0..100 {
        if timer(refreshes(&mut ws).await) == TimerState::Untrusted {
            untrusted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        untrusted,
        "a same-length edit that restored the mtime made the timer untrusted"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A command naming a 200 MB file does not slow `status`: the file is hashed once, on a blocking
/// thread, and every later tick and status read costs a `stat` while it is unchanged — and an
/// edit to it still makes the timer untrusted.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_named_file_is_hashed_once_and_status_stays_fast() {
    let base = std::env::temp_dir().join(format!("wheel-bigfile-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let big = base.join("big.log");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(200 * 1024 * 1024)
        .unwrap();
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    register(
        &base,
        "dash",
        &dash,
        "title = \"Dash\"\nevery = 1\ncommand = \"wc -c big.log > \\\"$WHEEL_DATA_DIR/size\\\"\"\n",
    );
    let (url, _) = start_daemon(Some(base.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let mut person = connect(&url).await;
    created(&mut ws, new_space(&base.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(10),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("dash".into()),
            scope: None,
            url: None,
        },
    )
    .await;
    answer(&mut person, "always").await;
    // The opening run, then two ticks: the file was hashed once, at the approval.
    ran(&mut ws, &pane, 3).await;

    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let at = std::time::Instant::now();
        let row = refreshes(&mut ws)
            .await
            .into_iter()
            .find(|r| r.pane == pane)
            .unwrap();
        slowest = slowest.max(at.elapsed());
        assert_eq!(row.timer, TimerState::Ticking, "{row:?}");
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(
        slowest < Duration::from_millis(150),
        "status took {slowest:?} with a 200 MB file named"
    );

    // An edit keeps the length but moves the modification time: the timer stops trusting.
    {
        use std::io::{Seek, Write};
        let mut file = std::fs::OpenOptions::new().write(true).open(&big).unwrap();
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        file.write_all(b"x").unwrap();
    }
    let mut untrusted = false;
    for _ in 0..100 {
        let row = refreshes(&mut ws)
            .await
            .into_iter()
            .find(|r| r.pane == pane)
            .unwrap();
        if row.timer == TimerState::Untrusted {
            untrusted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        untrusted,
        "an edit to the named file made the timer untrusted"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A raw request with a body to the daemon's HTTP side: the status code and the whole response.
async fn http_put(addr: &str, target: &str, headers: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "PUT {target} HTTP/1.1\r\nHost: {addr}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    (response[9..12].parse().unwrap(), response)
}

/// Read a `.wheel/events` stream until `want` has arrived, and return everything received.
async fn read_events(addr: &str, target: &str, headers: &str, want: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {target} HTTP/1.1\r\nHost: {addr}\r\n{headers}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut seen = String::new();
    let mut buf = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !seen.contains(want) {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("{want:?} never arrived on the stream; got {seen:?}"))
            .unwrap();
        assert!(n > 0, "the stream closed before {want:?}; got {seen:?}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    seen
}

/// A page answers its agent: it writes a value to the pane's state store over HTTP, `artifact_state`
/// reads it back (waiting for a key still unwritten), and a value pushed with `artifact_data`
/// reaches the page's event stream — including a stream opened after the push.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_stores_state_and_receives_pushed_data() {
    let dir = std::env::temp_dir().join(format!("wheel-page-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>ask</p>").unwrap();
    register(&dir, "site", std::path::Path::new("site"), "");
    let inst = dir.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    let (url, _) = start_daemon(Some(inst.join("spaces.json"))).await;
    let addr = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    created(&mut ws, new_space(&dir.to_string_lossy())).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(10),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("site".into()),
            scope: None,
            url: None,
        },
    )
    .await;
    let state = |key: Option<&str>, wait_ms: Option<u64>| ClientMsg::ArtifactState {
        req: Some(20),
        pane: pane.clone(),
        key: key.map(str::to_string),
        wait_ms,
    };
    let bearer = format!("Authorization: Bearer {TOKEN}\r\n");
    let put = |key: &str| format!("/artifacts/{pane}/.wheel/state/{key}");

    // Nothing is stored yet: all keys read as empty, and a short wait for one times out.
    match call(&mut ws, state(None, None)).await {
        ServerMsg::PageState { values, .. } => assert!(values.is_empty()),
        other => panic!("expected page_state, got {other:?}"),
    }
    match call(&mut ws, state(Some("answer"), Some(200))).await {
        ServerMsg::PageState {
            values, timed_out, ..
        } => {
            assert!(values.is_empty() && timed_out);
        }
        other => panic!("expected page_state, got {other:?}"),
    }

    // A wait started before the write returns the value the write stores.
    let waiting = {
        let url = url.clone();
        let pane = pane.clone();
        tokio::spawn(async move {
            let mut ws2 = connect(&url).await;
            call(
                &mut ws2,
                ClientMsg::ArtifactState {
                    req: Some(21),
                    pane,
                    key: Some("answer".into()),
                    wait_ms: Some(10_000),
                },
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        http_put(&addr, &put("answer"), "", "{\"a\":1}").await.0,
        401,
        "no token"
    );
    assert_eq!(
        http_put(&addr, &put("answer"), &bearer, "{\"a\":1}")
            .await
            .0,
        204
    );
    match waiting.await.unwrap() {
        ServerMsg::PageState {
            values, timed_out, ..
        } => {
            assert!(!timed_out);
            assert_eq!(values["answer"], serde_json::json!({"a": 1}));
        }
        other => panic!("expected page_state, got {other:?}"),
    }
    // Bad input is refused and stores nothing.
    assert_eq!(
        http_put(&addr, &put("bad"), &bearer, "not json").await.0,
        400
    );
    assert_eq!(http_put(&addr, &put(".hidden"), &bearer, "1").await.0, 400);
    match call(&mut ws, state(None, None)).await {
        ServerMsg::PageState { values, .. } => {
            assert_eq!(values.keys().collect::<Vec<_>>(), ["answer"]);
        }
        other => panic!("expected page_state, got {other:?}"),
    }
    // The store is a hidden file in the pane's data directory: never served.
    assert!(inst.join("panes").join(&pane).join(".state.json").is_file());
    let (status, _) = http_get(
        &addr,
        &format!("/artifacts/{pane}/data/.state.json"),
        &bearer,
    )
    .await;
    assert_eq!(status, 404);

    // A push reaches an open stream, and a stream opened later gets the latest value first.
    let events = format!("/artifacts/{pane}/.wheel/events");
    let open = {
        let (addr, events, bearer) = (addr.clone(), events.clone(), bearer.clone());
        tokio::spawn(async move {
            read_events(&addr, &events, &bearer, "data: {\"q\":\"first\"}").await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ok(
        &mut ws,
        ClientMsg::ArtifactData {
            req: Some(30),
            pane: pane.clone(),
            value: serde_json::json!({"q": "first"}),
        },
    )
    .await;
    open.await.unwrap();
    ok(
        &mut ws,
        ClientMsg::ArtifactData {
            req: Some(31),
            pane: pane.clone(),
            value: serde_json::json!({"q": "latest"}),
        },
    )
    .await;
    let late = read_events(&addr, &events, &bearer, "data: {\"q\":\"latest\"}").await;
    assert!(
        !late.contains("first"),
        "only the latest value is resent: {late}"
    );
    let (status, js) = http_get(
        &addr,
        &format!("/artifacts/{pane}/.wheel/client.js"),
        &bearer,
    )
    .await;
    assert_eq!(status, 200);
    assert!(js.contains("window.wheel"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn dir_list_names_subfolders_marks_open_spaces_and_says_why_it_cannot() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("wheel-dirlist-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for sub in ["beta", "alpha", ".hidden/inner", ".hidden/.deep", "locked"] {
        std::fs::create_dir_all(base.join(sub)).unwrap();
    }
    std::fs::write(base.join("file.txt"), "not a folder").unwrap();
    std::fs::set_permissions(base.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();

    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    let id = created(&mut ws, new_space(&base.join("beta").to_string_lossy())).await;
    let list = |path: String| ClientMsg::DirList { req: Some(5), path };
    let reply = |msg: ServerMsg| match msg {
        ServerMsg::DirList {
            path, dirs, error, ..
        } => (path, dirs, error),
        other => panic!("expected dir_list, got {other:?}"),
    };
    let base_str = base.to_string_lossy().into_owned();

    // Sorted by name, files and the hidden folder left out, the open space's folder marked.
    let (path, dirs, error) = reply(call(&mut ws, list(base_str.clone())).await);
    assert_eq!((path.as_str(), error), (base_str.as_str(), None));
    let rows: Vec<(String, Option<String>)> = dirs.into_iter().map(|d| (d.name, d.space)).collect();
    assert_eq!(
        rows,
        vec![
            ("alpha".to_string(), None),
            ("beta".to_string(), Some(id)),
            ("locked".to_string(), None),
        ]
    );

    // A path that names a hidden folder lists its hidden children too.
    let hidden = format!("{base_str}/.hidden");
    let (_, dirs, _) = reply(call(&mut ws, list(hidden)).await);
    let names: Vec<_> = dirs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, [".deep", "inner"]);

    // Missing folder, a file, and an unreadable folder each get their reason.
    let missing = format!("{base_str}/no/such");
    let (path, dirs, error) = reply(call(&mut ws, list(missing.clone())).await);
    assert_eq!(
        (path, dirs.len(), error),
        (missing.clone(), 0, Some(format!("no folder at {missing}")))
    );
    let file = format!("{base_str}/file.txt");
    let (_, _, error) = reply(call(&mut ws, list(file.clone())).await);
    assert_eq!(error, Some(format!("no folder at {file}")));
    let (_, _, error) = reply(call(&mut ws, list(format!("{base_str}/locked"))).await);
    assert_eq!(error.as_deref(), Some("permission denied"));

    // `~` expands to the daemon user's home.
    let home = std::env::var("HOME").unwrap();
    let (path, _, _) = reply(call(&mut ws, list("~".into())).await);
    assert_eq!(path, home);

    std::fs::set_permissions(base.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread")]
async fn spaces_roots_default_to_home_and_the_parents_of_recent_spaces() {
    let (url, _) = start_daemon(None).await;
    let mut ws = connect(&url).await;
    let roots = |msg: ServerMsg| match msg {
        ServerMsg::DirRoots { roots, .. } => roots,
        other => panic!("expected dir_roots, got {other:?}"),
    };
    async fn get(ws: &mut Ws) -> String {
        let msg = ClientMsg::SettingsLs {
            req: Some(6),
            space: None,
        };
        match call(ws, msg).await {
            ServerMsg::Settings { settings, .. } => {
                settings
                    .into_iter()
                    .find(|s| s.key == "spaces.roots")
                    .unwrap()
                    .value
            }
            other => panic!("expected settings, got {other:?}"),
        }
    }
    let home = std::env::var("HOME").unwrap();
    assert_eq!(get(&mut ws).await, "~");
    assert_eq!(
        roots(call(&mut ws, ClientMsg::DirRoots { req: Some(7) }).await),
        std::slice::from_ref(&home)
    );

    for cwd in ["/tmp", "/private/tmp"] {
        let id = created(&mut ws, new_space(cwd)).await;
        ok(
            &mut ws,
            ClientMsg::SpaceClose {
                req: Some(8),
                id,
                if_empty: false,
            },
        )
        .await;
    }
    assert_eq!(get(&mut ws).await, "~,/private,/");
    assert_eq!(
        roots(call(&mut ws, ClientMsg::DirRoots { req: Some(9) }).await),
        [home, "/private".to_string(), "/".to_string()]
    );
}

/// A project `wheel.toml` that links outside its space is read but never written: `config set`,
/// `artifact new`, `add` and `remove` are refused and the target's bytes stay. A link that stays
/// inside the directory is written through and remains a link, and the machine `wheel.toml`
/// writes through any link.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_wheel_toml_linking_outside_its_space_is_never_written() {
    use std::os::unix::fs::symlink;
    let base = std::env::temp_dir().join(format!("wheel-link-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let page = base.join("page");
    std::fs::create_dir_all(&page).unwrap();
    std::fs::write(page.join("index.html"), "<p>page</p>").unwrap();

    // The outside target holds a setting and an artifact, so reads have something to find.
    let outside = base.join("outside.toml");
    let outside_text = format!(
        "[browser]\nhomepage = \"https://outside.example\"\n[artifacts.kept]\npath = \"{}\"\n",
        page.display()
    );
    std::fs::write(&outside, &outside_text).unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    symlink(&outside, proj.join("wheel.toml")).unwrap();
    let inst = base.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    let elsewhere = base.join("machine-target.toml");
    std::fs::write(&elsewhere, "").unwrap();
    symlink(&elsewhere, inst.join("wheel.toml")).unwrap();

    let (url, daemon) = start_daemon(Some(inst.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let id = created(&mut ws, new_space(&proj.to_string_lossy())).await;
    let unchanged = || assert_eq!(std::fs::read_to_string(&outside).unwrap(), outside_text);
    let refused = |reply: ServerMsg| match reply {
        ServerMsg::Error { message, .. } => {
            assert!(message.contains("outside"), "{message}");
            assert!(
                message.contains("writes only inside the project"),
                "{message}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    };

    refused(
        call(
            &mut ws,
            ClientMsg::SettingSet {
                req: Some(40),
                key: "browser.homepage".into(),
                value: "https://two.example".into(),
                space: Some(id.clone()),
                machine: false,
            },
        )
        .await,
    );
    refused(
        call(
            &mut ws,
            ClientMsg::SettingUnset {
                req: Some(41),
                key: "browser.homepage".into(),
                space: Some(id.clone()),
                machine: false,
            },
        )
        .await,
    );
    refused(call(&mut ws, make("fresh", None)).await);
    refused(
        call(
            &mut ws,
            ClientMsg::ArtifactAdd {
                req: Some(42),
                path: page.display().to_string(),
                name: Some("other".into()),
                scope: None,
                space: None,
                title: None,
                command: None,
                every: None,
                pane: false,
            },
        )
        .await,
    );
    refused(
        call(
            &mut ws,
            ClientMsg::ArtifactRemove {
                req: Some(43),
                name: "kept".into(),
                scope: Some(ArtifactScope::Project),
                space: None,
            },
        )
        .await,
    );
    unchanged();
    assert!(std::fs::symlink_metadata(proj.join("wheel.toml"))
        .unwrap()
        .is_symlink());

    // Reads still follow the link.
    let (_, settings) = daemon.state().settings_ls(Some(&id)).unwrap();
    let homepage = settings
        .iter()
        .find(|s| s.key == "browser.homepage")
        .unwrap();
    assert_eq!(homepage.value, "https://outside.example");
    // The list marks the keys only this machine's file sets, so a client need not know them.
    assert!(!homepage.machine_only);
    let push_url = settings.iter().find(|s| s.key == "logs.push_url").unwrap();
    assert!(push_url.machine_only);
    let shown = artifact(&mut ws, show("kept", Some(ArtifactScope::Project))).await;
    assert_eq!(shown.name, "kept");

    // A link that stays inside the space is written through, and stays a link.
    let proj2 = base.join("proj2");
    std::fs::create_dir_all(proj2.join("config")).unwrap();
    std::fs::write(proj2.join("config/wheel.toml"), "").unwrap();
    symlink("config/wheel.toml", proj2.join("wheel.toml")).unwrap();
    let id2 = created(&mut ws, new_space(&proj2.to_string_lossy())).await;
    daemon
        .state()
        .setting_set("browser.homepage", "https://in.example", Some(&id2), false)
        .unwrap();
    assert!(std::fs::read_to_string(proj2.join("config/wheel.toml"))
        .unwrap()
        .contains("https://in.example"));
    assert!(std::fs::symlink_metadata(proj2.join("wheel.toml"))
        .unwrap()
        .is_symlink());

    // The machine file writes through a link that leaves the instance directory.
    artifact(
        &mut ws,
        ClientMsg::ArtifactAdd {
            req: Some(44),
            path: page.display().to_string(),
            name: Some("wide".into()),
            scope: Some(ArtifactScope::Machine),
            space: None,
            title: None,
            command: None,
            every: None,
            pane: false,
        },
    )
    .await;
    assert!(std::fs::read_to_string(&elsewhere)
        .unwrap()
        .contains("[artifacts.wide]"));
    std::fs::remove_dir_all(&base).unwrap();
}
