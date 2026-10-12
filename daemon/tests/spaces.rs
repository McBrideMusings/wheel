//! Spaces over the socket: every change reaches every client as a `spaces` event, a terminal pane
//! runs a shell in its space's directory, closing the last pane closes the space, and a restarted
//! daemon gets the same layout back with fresh shells.

use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{Axis, ClientMsg, Dir, NewPaneKind, Node, PaneKind, ServerMsg, SpaceInfo};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "spaces-test-token";

/// Start a daemon; the handle stops it serving when aborted.
async fn start_daemon(state_file: Option<PathBuf>) -> (String, JoinHandle<anyhow::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into());
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state));
    (format!("ws://{addr}/ws?token={TOKEN}"), serving)
}

async fn connect(url: &str) -> Ws {
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
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
            ServerMsg::Spaces { .. } => {}
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
    }
}

fn terminal(pane: &str) -> ClientMsg {
    ClientMsg::PaneSetKind {
        req: Some(2),
        pane: pane.into(),
        kind: NewPaneKind::Terminal,
        dir: None,
    }
}

fn term_of(space: &SpaceInfo, pane: &str) -> Option<String> {
    fn find(node: &Node, pane: &str) -> Option<Option<String>> {
        match node {
            Node::Pane { id, kind } if id == pane => Some(match kind {
                PaneKind::Terminal { term, .. } => Some(term.clone()),
                PaneKind::Empty { .. }
                | PaneKind::Web { .. }
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
async fn changes_reach_every_client_and_the_last_pane_closes_its_space() {
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
    ok(
        &mut ws,
        ClientMsg::PaneClose {
            req: Some(6),
            pane: first,
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
    for _ in 0..100 {
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
            dir: None,
        },
    ] {
        assert!(
            matches!(call(&mut ws, msg.clone()).await, ServerMsg::Error { .. }),
            "{msg:?} is refused"
        );
    }
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
    assert!(matches!(&**a, Node::Pane { id, kind: PaneKind::Empty { .. } } if *id == left));
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
async fn without_a_lead_agent_panes_neither_start_nor_vanish() {
    let dir = std::env::temp_dir().join(format!("wheel-no-lead-agent-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let agent = serde_json::json!({"node": "pane", "id": "p2", "kind": "agent", "session": "w5"});
    let saved = serde_json::json!({
        "spaces": [{"id": "s1", "name": "tmp", "cwd": "/tmp", "focus": "p2", "root": agent}],
        "active": "s1",
        "next_id": 10,
    });
    std::fs::write(&state_file, saved.to_string()).unwrap();

    // A saved agent pane stays as saved, so a later start with a lead runs it again.
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into());
    state.restore_spaces(state_file.clone()).await.unwrap();
    assert_eq!(
        serde_json::to_value(&state.spaces().spaces[0].root).unwrap(),
        agent
    );

    // A worker that cannot start leaves no pane behind.
    let error = state
        .start_worker("s1", None, "task".into())
        .await
        .unwrap_err();
    assert!(error.contains("runs without a lead"), "{error}");
    assert_eq!(
        serde_json::to_value(&state.spaces().spaces[0].root).unwrap(),
        agent
    );
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
async fn a_web_pane_serves_its_folder_and_reloads_on_change() {
    let dir = std::env::temp_dir().join(format!("wheel-web-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<p>one</p>").unwrap();
    let (url, _) = start_daemon(None).await;
    let addr = url
        .trim_start_matches("ws://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    created(&mut ws, new_space("/tmp")).await;
    let (spaces, _, _) = layout(&mut ws).await;
    let pane = spaces[0].focus.clone();
    let web = |dir: Option<String>| ClientMsg::PaneSetKind {
        req: Some(10),
        pane: pane.clone(),
        kind: NewPaneKind::Web,
        dir,
    };
    assert!(
        matches!(call(&mut ws, web(None)).await, ServerMsg::Error { .. }),
        "web needs dir"
    );
    ok(&mut ws, web(Some(dir.to_string_lossy().into_owned()))).await;

    std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
    let page = format!("/artifacts/{pane}/index.html");
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

    let mut watcher = connect(&url).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let written = std::time::Instant::now();
    std::fs::write(dir.join("index.html"), "<p>two</p>").unwrap();
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(&mut watcher).await {
            assert_eq!(changed, pane);
            break;
        }
    }
    assert!(
        written.elapsed() < Duration::from_secs(1),
        "reload took {:?}",
        written.elapsed()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
