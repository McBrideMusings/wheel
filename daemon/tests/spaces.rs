//! Spaces over the socket: every change reaches every client as a `spaces` event, a terminal pane
//! runs a shell in its space's directory, closing the last pane closes the space, and a restarted
//! daemon gets the same layout back with fresh shells.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{Axis, ClientMsg, Dir, NewPaneKind, Node, PaneKind, ServerMsg, SpaceInfo};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "spaces-test-token";

async fn start_daemon(state_file: Option<PathBuf>) -> (String, Arc<wheeld::AppState>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into());
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    tokio::spawn(wheeld::serve(listener, state.clone()));
    (format!("ws://{addr}/ws?token={TOKEN}"), state)
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
    }
}

fn term_of(space: &SpaceInfo, pane: &str) -> Option<String> {
    fn find(node: &Node, pane: &str) -> Option<Option<String>> {
        match node {
            Node::Pane { id, kind } if id == pane => Some(match kind {
                PaneKind::Terminal { term } => Some(term.clone()),
                PaneKind::Empty => None,
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
        kind: PaneKind::Empty,
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
    drop(first_daemon);

    let (url, _) = start_daemon(Some(state_file)).await;
    let mut ws = connect(&url).await;
    let (after, active_after, terms) = layout(&mut ws).await;
    assert_eq!(active_after, active);
    let Node::Split { id, ratio, a, .. } = &after[0].root else {
        panic!("split restored")
    };
    assert_eq!((id, *ratio), (split, 0.3));
    assert!(matches!(&**a, Node::Pane { id, kind: PaneKind::Empty } if *id == left));
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
}
