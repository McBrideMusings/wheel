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
use wheel_proto::{
    ArtifactScope, Axis, ClientMsg, Dir, NewPaneKind, Node, PaneKind, ServerMsg, SettingOrigin,
    SpaceInfo, TimerState, WatchInfo, WatchRole,
};

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

/// The daemon instance directory for a run keeping its layout at `state_file`: the directory
/// holding it, as in production, so the run's `shared` artifacts and its panes' data directories
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
            // The broadcasts: every connection gets them whether or not it asked for anything, so
            // a reply can arrive behind one. A test that wants one reads it with `recv` itself.
            ServerMsg::Spaces { .. }
            | ServerMsg::PaneReload { .. }
            | ServerMsg::PermRequested { .. }
            | ServerMsg::PermResolved { .. }
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
    // The layout saves inside `dir`, so `dir` is also the instance directory: the pane data
    // directories this test looks at go with it when it is removed.
    let (url, _) = start_daemon(Some(dir.join("spaces.json"))).await;
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

    // Opening the pane made it a data directory of its own, under the instance directory rather
    // than inside the artifact or anywhere in the project's tree. Nothing in the daemon writes
    // there yet, so a file put there by hand is what the pane serves under `data/`.
    let data = dir.join("panes").join(&pane);
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
    // manifest loader reports as a problem — can never be reached.
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
    // A change in the pane's own data directory reloads the page the same way.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(50), watcher.next()).await {}
    let written = std::time::Instant::now();
    std::fs::write(data.join("rows.json"), "{\"n\":2}").unwrap();
    loop {
        if let ServerMsg::PaneReload { pane: changed } = recv(&mut watcher).await {
            assert_eq!(changed, pane);
            break;
        }
    }
    assert!(
        written.elapsed() < Duration::from_secs(1),
        "a data change reloaded in {:?}",
        written.elapsed()
    );

    // One shared artifact — one folder, reachable from every space — open in two projects has two
    // data directories, and neither pane serves the other's files.
    let viewer = dir.join("artifacts/viewer");
    std::fs::create_dir_all(&viewer).unwrap();
    std::fs::write(viewer.join("index.html"), "<p>viewer</p>").unwrap();
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
                scope: Some(ArtifactScope::Shared),
            },
        )
        .await;
        std::fs::write(dir.join("panes").join(pane).join("rows.json"), body).unwrap();
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
    let gone = dir.join("panes").join(&second);
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
        dir.join("panes").join(&pane).is_dir(),
        "the other pane's data directory went with it"
    );
    std::fs::remove_dir_all(&dir).unwrap();
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
        req: Some(20),
        name: name.into(),
        scope,
        space: None,
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

/// `artifact.toml` in the folder is the artifact's whole definition: the daemon makes the folder,
/// the manifest and the page, reads them back with every problem it finds, and puts nothing about
/// any of it in the saved state file.
#[tokio::test(flavor = "multi_thread")]
async fn artifacts_are_made_listed_and_shown_from_their_manifests() {
    let base = std::env::temp_dir().join(format!("wheel-artifact-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
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
                space: None
            }
        )
        .await,
        ServerMsg::Error { .. }
    ));
    created(&mut ws, new_space(&proj.to_string_lossy())).await;

    // A new artifact: the folder, a manifest titled after it, a page, and nothing wrong with it.
    let made = artifact(&mut ws, make("dash", None)).await;
    assert_eq!(made.scope, ArtifactScope::Repo, "repo is the default scope");
    assert_eq!(
        made.dir,
        proj.join(".wheel/artifacts/dash").to_string_lossy()
    );
    assert_eq!((made.title.as_str(), made.problems.len()), ("dash", 0));
    assert!(
        std::fs::read_to_string(proj.join(".wheel/artifacts/dash/artifact.toml"))
            .unwrap()
            .contains("title = \"dash\"")
    );
    assert!(proj.join(".wheel/artifacts/dash/index.html").is_file());
    // Nothing an artifact holds is overwritten, and a name no reference can hold never reaches disk.
    assert!(matches!(
        call(&mut ws, make("dash", None)).await,
        ServerMsg::Error { .. }
    ));
    for bad in ["a/b", "..", ".env", "", "/etc"] {
        match call(&mut ws, make(bad, Some(ArtifactScope::Repo))).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{bad:?} is not an artifact name, got {other:?}"),
        }
    }

    // A local artifact leaves the one gitignorable prefix behind, once however often it runs.
    let local = artifact(&mut ws, make("dash", Some(ArtifactScope::Local))).await;
    assert_eq!(
        local.dir,
        proj.join(".wheel/local/artifacts/dash").to_string_lossy()
    );
    artifact(&mut ws, make("two", Some(ArtifactScope::Local))).await;
    let ignored = std::fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert_eq!(
        ignored.lines().filter(|l| *l == ".wheel/local/").count(),
        1,
        "{ignored}"
    );
    // A shared artifact's folder is under the instance directory, outside every space.
    let shared = artifact(&mut ws, make("viewer", Some(ArtifactScope::Shared))).await;
    assert_eq!(shared.dir, base.join("artifacts/viewer").to_string_lossy());

    // A manifest's problems are part of the record, so a bad artifact is never only a log line.
    let broken = proj.join(".wheel/artifacts/broken");
    std::fs::create_dir_all(broken.join("data")).unwrap();
    std::fs::write(broken.join("artifact.toml"), "colour = 1\nevery = 2\n").unwrap();
    let shown = artifact(&mut ws, show("broken", None)).await;
    assert_eq!(shown.every, Some(2));
    assert_eq!(shown.title, "broken", "a manifest with no title");
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
            ("local", "dash"),
            ("local", "two"),
            ("repo", "broken"),
            ("repo", "dash"),
            ("shared", "viewer"),
        ]
    );
    assert_eq!(
        resolves
            .iter()
            .map(|(name, scope)| (name.as_str(), scope.name()))
            .collect::<Vec<_>>(),
        [
            ("broken", "repo"),
            ("dash", "local"),
            ("two", "local"),
            ("viewer", "shared"),
        ]
    );

    // A bare name takes the scope that won; a pinned one takes its own root, and refuses when that
    // root has nothing by that name.
    assert_eq!(
        artifact(&mut ws, show("dash", None)).await.scope,
        ArtifactScope::Local
    );
    assert_eq!(
        artifact(&mut ws, show("dash", Some(ArtifactScope::Repo)))
            .await
            .dir,
        made.dir
    );
    for (name, scope) in [("nosuch", None), ("viewer", Some(ArtifactScope::Repo))] {
        match call(&mut ws, show(name, scope)).await {
            ServerMsg::Error { .. } => {}
            other => panic!("{name} in {scope:?} should not resolve, got {other:?}"),
        }
    }

    // The folder and its manifest are the whole definition: the saved state file names no artifact,
    // so a project's artifacts travel with its clone and no rewrite of that file can lose them.
    let saved = std::fs::read_to_string(&state_file).unwrap();
    for name in ["dash", "viewer", "artifact.toml", "title"] {
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
/// 3s.
async fn watch_until(
    ws: &mut Ws,
    pane: &str,
    role: WatchRole,
    want: impl Fn(Option<&WatchInfo>) -> bool,
) {
    for _ in 0..60 {
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

/// How long a reload may take and still count as following the write that caused it. A watch
/// debounces for about 215ms; the rest is room for a loaded machine, since a tighter bound tests
/// how busy the suite is rather than whether the write reached the page.
const PROMPTLY: Duration = Duration::from_secs(5);

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
            // The same shared artifact in two spaces: one folder, two panes, two data directories.
            {"id": "s4", "name": "two", "cwd": base, "focus": "p6", "root": web("p6", "shared", "viewer")},
            // A shared artifact's folder is under the instance directory, outside every space.
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
    // one shared artifact in two spaces and get one directory each.
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
            if artifact.scope() == wheel_proto::ArtifactScope::Repo && artifact.name() == "site"
    ));
    let p2 = row(&all, "p2", WatchRole::Artifact).unwrap();
    assert_eq!((p2.watching, p2.waiting), (at(&repo), true));
    assert_eq!(p2.dir, site.to_string_lossy());
    let p3 = row(&all, "p3", WatchRole::Artifact).unwrap();
    assert_eq!((p3.watching, p3.waiting), (at(&shown), false));
    // The shared scope resolves under the instance directory, not under any space.
    let p8 = row(&all, "p8", WatchRole::Artifact).unwrap();
    assert_eq!(
        (p8.dir, p8.waiting),
        (viewer.to_string_lossy().into_owned(), false)
    );

    // The folder appearing moves the watch onto it and reloads the page.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::create_dir(&site).unwrap();
    std::fs::write(site.join("index.html"), "<p>one</p>").unwrap();
    let took = reloaded(&mut events, "p2").await;
    assert!(took < PROMPTLY, "reload took {took:?}");
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
    let took = reloaded(&mut events, "p2").await;
    assert!(took < PROMPTLY, "reload took {took:?}");

    // A change in the pane's own data directory sends the same reload.
    std::fs::write(data("p2").join("rows.json"), "{\"n\":1}").unwrap();
    let took = reloaded(&mut events, "p2").await;
    assert!(took < PROMPTLY, "a data change took {took:?}");

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
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    // The command writes only where it is given to, named in its environment, and runs in the
    // space's directory — which is what `pwd` here proves.
    let manifest = |command: &str| {
        std::fs::write(
            dash.join("artifact.toml"),
            format!(
                "title = \"Dash\"\ncommand = {}\n",
                toml::Value::from(command)
            ),
        )
        .unwrap()
    };
    manifest("pwd > \"$WHEEL_DATA_DIR/where\" && printf %s \"$WHEEL_PANE\" > \"$WHEEL_DATA_DIR/rows.json\"");
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
    };

    // Opening the pane asks, naming the command, the artifact and the space directory.
    ok(&mut ws, web.clone()).await;
    let title = answer(&mut person, "allow").await;
    assert!(title.contains("repo artifact dash"), "{title}");
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
    assert_eq!(row.scope, ArtifactScope::Repo);
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

    // Editing the command in the manifest asks again, and what runs is what the manifest says now.
    manifest("printf 'boom\\n' >&2; exit 3");
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
    manifest("printf %s refused > \"$WHEEL_DATA_DIR/rows.json\"");
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
    manifest("pwd > \"$WHEEL_DATA_DIR/where\" && printf %s \"$WHEEL_PANE\" > \"$WHEEL_DATA_DIR/rows.json\"");
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

    // An artifact whose manifest declares no command has no runner, and no row to report.
    std::fs::write(dash.join("artifact.toml"), "title = \"Dash\"\n").unwrap();
    ok(&mut ws, web.clone()).await;
    assert!(refreshes(&mut ws).await.is_empty());
    ask_refresh(&mut ws, &pane).await;
    assert!(refresh_reply(&mut ws)
        .await
        .unwrap_err()
        .contains("declares no command"));
    // Nor does a pane showing no artifact at all.
    manifest("true");
    ok(&mut ws, web).await;
    assert_eq!(refreshes(&mut ws).await.len(), 1);
    ok(&mut ws, terminal(&pane)).await;
    assert!(
        refreshes(&mut ws).await.is_empty(),
        "the pane stopped showing it"
    );
    let _ = std::fs::remove_dir_all(&base);
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
        (home, "file:///etc/passwd"),
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
    let clock = base.join(".wheel/artifacts/clock");
    std::fs::create_dir_all(&clock).unwrap();
    std::fs::write(clock.join("index.html"), "<p>clock</p>").unwrap();
    // Each run marks its own start and end in one file, and takes longer than the interval asks
    // for, so two runs overlapping would interleave those marks.
    let command =
        "printf '(' >> \"$WHEEL_DATA_DIR/log\"; sleep 1.2; printf ')' >> \"$WHEEL_DATA_DIR/log\"";
    std::fs::write(
        clock.join("artifact.toml"),
        format!(
            "title = \"Clock\"\nevery = 1\ncommand = {}\n",
            toml::Value::from(command)
        ),
    )
    .unwrap();
    // The pair is remembered before the daemon starts, so the timer has an approved command from
    // its first tick: a tick never asks, and nothing here answers a request.
    let trust = wheel_proto::TrustInfo {
        dir: clock.to_string_lossy().to_string(),
        command: command.to_string(),
        scope: ArtifactScope::Repo,
        name: "clock".into(),
        approved_ms: 1,
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

    // A manifest that stops asking for a timer stops the runs without ending the timer, and asking
    // for one again resumes it — neither needs the pane reopened.
    let every = |secs: Option<u64>| {
        let mut text = format!(
            "title = \"Clock\"\ncommand = {}\n",
            toml::Value::from(command)
        );
        if let Some(secs) = secs {
            text.push_str(&format!("every = {secs}\n"));
        }
        std::fs::write(clock.join("artifact.toml"), text).unwrap();
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
        "a manifest asking for no timer runs nothing: {going:?}"
    );
    every(Some(1));
    ran(&mut ws, &pane, idled + 1).await;

    // A pane opened on a manifest asking for no timer has none, and says so even once the manifest
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
    let dash = base.join(".wheel/artifacts/dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("index.html"), "<p>dash</p>").unwrap();
    std::fs::write(
        dash.join("artifact.toml"),
        "title = \"Dash\"\ncommand = \"printf %s ran > \\\"$WHEEL_DATA_DIR/rows.json\\\"\"\n",
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
        artifact: Some("dash".into()),
        scope: None,
    };
    let empty = ClientMsg::PaneSetKind {
        req: Some(11),
        pane: pane.clone(),
        kind: NewPaneKind::Empty,
        artifact: None,
        scope: None,
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
