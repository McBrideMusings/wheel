//! The lead's inbox over the socket, driven by the scripted fake agent: an agent pane's turn
//! end, permission request and failure add items; an item survives a daemon restart and closes
//! when its pane gets a message or closes; and the lead sees the count of new items in front of
//! the next message a person sends it.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    ClientMsg, InboxItem, InboxKind, InboxState, ServerMsg, SessionEvent, StampedEvent, StartKind,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "inbox-test-token";

/// A daemon on the fake agent keeping its layout, and so its inbox, in `dir`.
async fn start(dir: &std::path::Path) -> (String, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.to_path_buf(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent");
    state.set_orchestration(true);
    state.set_agent_command(command.into());
    state.set_mcp_url(wheeld::mcp::url(addr));
    state.restore_spaces(dir.join("spaces.json")).await.unwrap();
    state.start_lead(command.into(), "/tmp".into());
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        common::Daemon::new(serving, state),
    )
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wheel-inbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
        let frame = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Send `msg` (carrying `req`) and return the first reply for that `req`.
async fn call(ws: &mut Ws, req: u64, msg: ClientMsg) -> Result<ServerMsg, String> {
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => return Err(message),
            reply @ (ServerMsg::Created { req: r, .. }
            | ServerMsg::Started { req: r, .. }
            | ServerMsg::Ok { req: r }
            | ServerMsg::Inbox { req: r, .. }
            | ServerMsg::InboxItem { req: r, .. }
            | ServerMsg::SessionQueued { req: r, .. }
            | ServerMsg::Status { req: Some(r), .. })
                if r == req =>
            {
                return Ok(reply)
            }
            _ => {}
        }
    }
}

async fn inbox(ws: &mut Ws) -> Vec<InboxItem> {
    match call(ws, 500, ClientMsg::InboxLs { req: Some(500) }).await {
        Ok(ServerMsg::Inbox { items, .. }) => items,
        other => panic!("inbox_ls: {other:?}"),
    }
}

/// Poll the inbox until `done` holds for its items; the items then.
async fn inbox_until(
    ws: &mut Ws,
    what: &str,
    done: impl Fn(&[InboxItem]) -> bool,
) -> Vec<InboxItem> {
    for _ in 0..200 {
        let items = inbox(ws).await;
        if done(&items) {
            return items;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the inbox never had {what}: {:?}", inbox(ws).await);
}

/// A space named `name` with an agent pane started on `message`; returns the pane and session.
async fn agent_in_new_space(ws: &mut Ws, name: &str, message: &str) -> (String, String) {
    let Ok(ServerMsg::Created { id: space, .. }) = call(
        ws,
        1,
        ClientMsg::SpaceCreate {
            req: Some(1),
            cwd: "/tmp".into(),
            name: Some(name.into()),
        },
    )
    .await
    else {
        panic!("space_create failed");
    };
    let Ok(ServerMsg::Status { spaces, .. }) =
        call(ws, 2, ClientMsg::Status { req: Some(2) }).await
    else {
        panic!("status failed");
    };
    let pane = spaces.iter().find(|s| s.id == space).unwrap().focus.clone();
    let Ok(ServerMsg::Started { session, .. }) = call(
        ws,
        3,
        ClientMsg::PaneStart {
            req: Some(3),
            pane: pane.clone(),
            kind: StartKind::Agent,
            message: Some(message.into()),
        },
    )
    .await
    else {
        panic!("pane_start failed");
    };
    (pane, session.expect("an agent session"))
}

async fn wait_lead_ready(ws: &mut Ws) {
    for _ in 0..100 {
        if let Ok(ServerMsg::Status { lead, .. }) =
            call(ws, 99, ClientMsg::Status { req: Some(99) }).await
        {
            if lead == Some(wheel_proto::SessionState::Ready) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("lead never became ready");
}

/// The lead's reply to `text`, sent as the person does.
async fn lead_says(ws: &mut Ws, req: u64, text: &str) -> String {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: "lead".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached { req: Some(r), .. } = recv(ws).await {
            if r == req {
                break;
            }
        }
    }
    let queued = call(
        ws,
        req + 1,
        ClientMsg::SessionSend {
            req: Some(req + 1),
            session: "lead".into(),
            text: text.into(),
        },
    )
    .await;
    assert!(
        matches!(queued, Ok(ServerMsg::SessionQueued { .. })),
        "{queued:?}"
    );
    let mut reply = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::Session {
                session,
                event:
                    StampedEvent {
                        event: SessionEvent::Text { text },
                        ..
                    },
            } if session == "lead" => reply.push_str(&text),
            ServerMsg::Session {
                session,
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { .. },
                        ..
                    },
            } if session == "lead" => return reply,
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_end_adds_an_item_that_survives_a_restart() {
    let dir = scratch("restart");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    let (pane, session) = agent_in_new_space(&mut ws, "web", "first task").await;

    let items = inbox_until(&mut ws, "a turn_ended item", |i| !i.is_empty()).await;
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(
        (item.kind, item.state),
        (InboxKind::TurnEnded, InboxState::New)
    );
    assert_eq!(
        (item.source.pane.as_str(), item.source.session.as_str()),
        (pane.as_str(), session.as_str())
    );
    assert_eq!(item.detail.as_deref(), Some("end_turn"));
    // Every client is told the badge count.
    let status = call(&mut ws, 4, ClientMsg::Status { req: Some(4) }).await;
    assert!(
        matches!(status, Ok(ServerMsg::Status { inbox_new: 1, .. })),
        "{status:?}"
    );
    let saved = std::fs::read_to_string(dir.join("inbox.json")).unwrap();
    assert!(saved.contains(&item.id), "{saved}");

    daemon.abort();
    drop(ws);
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    let after = inbox(&mut ws).await;
    assert_eq!(after[..1], items[..], "the item is as it was");
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_message_to_the_pane_closes_its_item() {
    let dir = scratch("message");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    let (_, session) = agent_in_new_space(&mut ws, "web", "first task").await;
    let first = inbox_until(&mut ws, "an item", |i| !i.is_empty()).await[0]
        .id
        .clone();

    let queued = call(
        &mut ws,
        5,
        ClientMsg::SessionSend {
            req: Some(5),
            session,
            text: "again".into(),
        },
    )
    .await;
    assert!(
        matches!(queued, Ok(ServerMsg::SessionQueued { .. })),
        "{queued:?}"
    );
    // The first item closes at once; the second turn's end adds a new one.
    let items = inbox_until(&mut ws, "a second item", |i| i.len() == 2).await;
    let state = |id: &str| items.iter().find(|i| i.id == id).unwrap().state;
    assert_eq!(state(&first), InboxState::Closed, "{items:?}");
    let second = items.iter().find(|i| i.id != first).unwrap();
    assert_eq!(
        (second.kind, second.state),
        (InboxKind::TurnEnded, InboxState::New)
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_sees_the_prefix_only_while_something_is_new() {
    let dir = scratch("prefix");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    wait_lead_ready(&mut ws).await;

    // Nothing new: the lead gets the person's words as typed.
    let plain = lead_says(&mut ws, 10, "hello").await;
    assert!(plain.contains("echo: hello"), "{plain:?}");
    assert!(!plain.contains("[inbox:"), "{plain:?}");

    agent_in_new_space(&mut ws, "web", "first task").await;
    inbox_until(&mut ws, "an item", |i| !i.is_empty()).await;
    let told = lead_says(&mut ws, 20, "what is up").await;
    assert!(
        told.contains("echo: [inbox: 1 new — web 1]\n\nwhat is up"),
        "{told:?}"
    );
    // The lead has been told, so the item is `seen` and the next message carries no prefix.
    let items = inbox(&mut ws).await;
    assert_eq!(items[0].state, InboxState::Seen, "{items:?}");
    let quiet = lead_says(&mut ws, 30, "and now").await;
    assert!(!quiet.contains("[inbox:"), "{quiet:?}");
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_pane_or_its_space_closes_the_items() {
    let dir = scratch("close");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    let (pane, _) = agent_in_new_space(&mut ws, "web", "first task").await;
    inbox_until(&mut ws, "an item", |i| !i.is_empty()).await;

    let closed = call(&mut ws, 6, ClientMsg::PaneClose { req: Some(6), pane }).await;
    assert!(closed.is_ok(), "{closed:?}");
    let items = inbox(&mut ws).await;
    assert!(
        !items.is_empty() && items.iter().all(|i| i.state == InboxState::Closed),
        "{items:?}"
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_adds_an_item_that_closes_when_answered() {
    let dir = scratch("permission");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    agent_in_new_space(&mut ws, "web", "permission please").await;

    let items = inbox_until(&mut ws, "a permission item", |i| {
        i.iter().any(|x| x.kind == InboxKind::Permission)
    })
    .await;
    let item = items
        .iter()
        .find(|i| i.kind == InboxKind::Permission)
        .unwrap();
    assert_eq!(item.state, InboxState::New);
    let perm = item.perm.clone().expect("the request it waits on");
    let answered = call(
        &mut ws,
        7,
        ClientMsg::PermAnswer {
            req: Some(7),
            id: perm,
            choice: "allow".into(),
        },
    )
    .await;
    assert!(answered.is_ok(), "{answered:?}");
    let id = item.id.clone();
    inbox_until(&mut ws, "the permission item closed", |i| {
        i.iter()
            .any(|x| x.id == id && x.state == InboxState::Closed)
    })
    .await;
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_agent_adds_a_failed_item() {
    let dir = scratch("failed");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    let (_, session) = agent_in_new_space(&mut ws, "web", "first task").await;
    inbox_until(&mut ws, "an item", |i| !i.is_empty()).await;
    let queued = call(
        &mut ws,
        8,
        ClientMsg::SessionSend {
            req: Some(8),
            session,
            text: "die now".into(),
        },
    )
    .await;
    assert!(
        matches!(queued, Ok(ServerMsg::SessionQueued { .. })),
        "{queued:?}"
    );
    let items = inbox_until(&mut ws, "a failed item", |i| {
        i.iter().any(|x| x.kind == InboxKind::Failed)
    })
    .await;
    let failed = items.iter().find(|i| i.kind == InboxKind::Failed).unwrap();
    assert_eq!(failed.state, InboxState::New);
    assert!(failed.detail.is_some(), "{failed:?}");
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_an_item_backward_names_the_states_in_protocol_words() {
    let dir = scratch("backward");
    let (url, daemon) = start(&dir).await;
    let mut ws = connect(&url).await;
    agent_in_new_space(&mut ws, "web", "first task").await;
    let id = inbox_until(&mut ws, "an item", |i| !i.is_empty()).await[0]
        .id
        .clone();
    let surfaced = call(
        &mut ws,
        9,
        ClientMsg::InboxSet {
            req: Some(9),
            id: id.clone(),
            state: InboxState::Surfaced,
        },
    )
    .await;
    assert!(
        matches!(surfaced, Ok(ServerMsg::InboxItem { .. })),
        "{surfaced:?}"
    );
    let back = call(
        &mut ws,
        10,
        ClientMsg::InboxSet {
            req: Some(10),
            id: id.clone(),
            state: InboxState::Seen,
        },
    )
    .await;
    assert_eq!(
        back.unwrap_err(),
        format!("inbox item {id} is surfaced and only moves forward; it cannot go back to seen")
    );
    daemon.abort();
}
