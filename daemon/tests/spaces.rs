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

/// Start a daemon; the handle stops it serving when aborted. Its instance directory is the one
/// holding the state file, as it is in production, so `shared` artifacts land beside the layout.
async fn start_daemon(state_file: Option<PathBuf>) -> (String, JoinHandle<anyhow::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let instance = instance_dir(state_file.as_deref());
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into(), instance);
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state));
    (format!("ws://{addr}/ws?token={TOKEN}"), serving)
}

/// The daemon instance directory for a run keeping its layout at `state_file`.
fn instance_dir(state_file: Option<&std::path::Path>) -> PathBuf {
    state_file
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
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
            ServerMsg::Spaces { .. } | ServerMsg::PaneReload { .. } => {}
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
        artifact: None,
        scope: None,
    }
}

/// What `pane` shows, or None when the tree has no such pane.
fn find_kind<'a>(node: &'a Node, pane: &str) -> Option<&'a PaneKind> {
    match node {
        Node::Pane { id, kind } if id == pane => Some(kind),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_kind(a, pane).or_else(|| find_kind(b, pane)),
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
            artifact: None,
            scope: None,
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
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into(), dir.clone());
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
async fn a_web_pane_serves_its_artifact_and_reloads_on_change() {
    let dir = std::env::temp_dir().join(format!("wheel-web-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A repo artifact, made by hand: the space's own `.wheel/artifacts/<name>/`.
    let site = dir.join(".wheel/artifacts/site");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>one</p>").unwrap();
    let (url, _) = start_daemon(None).await;
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
    };
    assert!(
        matches!(call(&mut ws, web(None)).await, ServerMsg::Error { .. }),
        "web needs an artifact"
    );
    // No scope-and-name input names a folder outside the three roots: anything but one plain name
    // is refused before a folder is computed, and a name nowhere is refused rather than guessed.
    for bad in ["/etc", "..", ".", "../out", "a/b", ".env", "", "nosuch"] {
        match call(&mut ws, web(Some(bad))).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{bad:?} is not an artifact this space has, got {other:?}"),
        }
    }
    // A bare name resolves local, then repo, then shared; only the repo root has `site` so far.
    ok(&mut ws, web(Some("site"))).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Repo && artifact.name() == "site"
    ));
    // The same name in the local root wins, and the pane stores the scope that won, so a project
    // shadows a wider artifact with its own and the saved layout never re-resolves.
    let local = dir.join(".wheel/local/artifacts/site");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("index.html"), "<p>local</p>").unwrap();
    ok(&mut ws, web(Some("site"))).await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Local && artifact.name() == "site"
    ));
    // A pinned scope takes that root whichever others have the name.
    ok(
        &mut ws,
        ClientMsg::PaneSetKind {
            req: Some(11),
            pane: pane.clone(),
            kind: NewPaneKind::Web,
            artifact: Some("site".into()),
            scope: Some(wheel_proto::ArtifactScope::Repo),
        },
    )
    .await;
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, &pane),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Repo && artifact.name() == "site"
    ));

    std::fs::write(site.join(".env"), "SECRET=1").unwrap();
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
    std::fs::write(site.join("index.html"), "<p>two</p>").unwrap();
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

async fn watches(ws: &mut Ws) -> Vec<wheel_proto::WatchInfo> {
    match call(ws, ClientMsg::Status { req: Some(98) }).await {
        ServerMsg::Status { watches, .. } => watches,
        other => panic!("expected status, got {other:?}"),
    }
}

/// Poll `status` until `pane`'s watch is in the state `want` says, for up to 3s.
async fn watch_until(
    ws: &mut Ws,
    pane: &str,
    want: impl Fn(Option<&wheel_proto::WatchInfo>) -> bool,
) {
    for _ in 0..60 {
        let all = watches(ws).await;
        if want(all.iter().find(|w| w.pane == pane)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{pane}'s watch never got there: {:?}", watches(ws).await);
}

/// Wait for `pane_reload` of `pane`; return how long it took.
async fn reloaded(ws: &mut Ws, pane: &str) -> Duration {
    let start = std::time::Instant::now();
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(ws).await {
            if changed == pane {
                return start.elapsed();
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
    // A shared artifact, outside every space, which is the whole point of the scope.
    let viewer = base.join("artifacts/viewer");
    std::fs::create_dir_all(&shown).unwrap();
    std::fs::create_dir_all(&viewer).unwrap();
    let state_file = base.join("spaces.json");
    let web = |id: &str, scope: &str, name: &str| serde_json::json!({"node": "pane", "id": id, "kind": "web", "scope": scope, "name": name});
    let saved = serde_json::json!({
        "spaces": [
            {"id": "s1", "name": "one", "cwd": base, "focus": "p2",
             "root": {"node": "split", "id": "x5", "axis": "horizontal", "ratio": 0.5,
                      "a": web("p2", "repo", "site"), "b": web("p3", "repo", "shown")}},
            {"id": "s4", "name": "two", "cwd": base, "focus": "p6", "root": web("p6", "repo", "shown")},
            // A shared artifact in a space of its own: its folder is under the instance directory.
            {"id": "s7", "name": "three", "cwd": shown, "focus": "p8", "root": web("p8", "shared", "viewer")},
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
    assert_eq!(
        all.iter().map(|w| &w.pane[..]).collect::<Vec<_>>(),
        ["p2", "p3", "p6", "p8"]
    );
    let (spaces, _, _) = layout(&mut ws).await;
    assert!(matches!(
        find_kind(&spaces[0].root, "p2"),
        Some(PaneKind::Web { artifact, .. })
            if artifact.scope() == wheel_proto::ArtifactScope::Repo && artifact.name() == "site"
    ));
    assert_eq!((all[0].watching.clone(), all[0].waiting), (at(&repo), true));
    assert_eq!(all[0].dir, site.to_string_lossy());
    assert_eq!(
        (all[1].watching.clone(), all[1].waiting),
        (at(&shown), false)
    );
    // The shared scope resolves under the instance directory, not under any space.
    assert_eq!(
        (all[3].dir.clone(), all[3].waiting),
        (viewer.to_string_lossy().into_owned(), false)
    );

    // The folder appearing moves the watch onto it and reloads the page.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::create_dir(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>one</p>").unwrap();
    let took = reloaded(&mut events, "p2").await;
    assert!(took < Duration::from_secs(1), "reload took {took:?}");
    watch_until(&mut ws, "p2", |w| {
        w.is_some_and(|w| w.watching == at(&site) && !w.waiting)
    })
    .await;

    // Removed, it waits again; made again, a change inside it reloads.
    std::fs::remove_dir_all(&site).unwrap();
    watch_until(&mut ws, "p2", |w| w.is_some_and(|w| w.waiting)).await;
    std::fs::create_dir(&site).unwrap();
    watch_until(&mut ws, "p2", |w| w.is_some_and(|w| !w.waiting)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await {}
    std::fs::write(site.join("index.html"), "<p>two</p>").unwrap();
    let took = reloaded(&mut events, "p2").await;
    assert!(took < Duration::from_secs(1), "reload took {took:?}");

    // The watch stops when the pane changes kind, closes, or its space closes.
    ok(&mut ws, terminal("p2")).await;
    watch_until(&mut ws, "p2", |w| w.is_none()).await;
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
    watch_until(&mut ws, "p3", |w| w.is_none()).await;
    ok(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(4),
            id: "s4".into(),
        },
    )
    .await;
    ok(
        &mut ws,
        ClientMsg::SpaceClose {
            req: Some(5),
            id: "s7".into(),
        },
    )
    .await;
    assert!(watches(&mut ws).await.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}
