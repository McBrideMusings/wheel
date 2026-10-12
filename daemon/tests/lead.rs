//! The lead over the socket, driven by the scripted fake agent: a turn streams back with the
//! orchestrator context in place, and a permission request reaches every client, where the first
//! answer resumes the agent and a second answer is refused.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, LeadEvent, LeadState, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "lead-test-token";

async fn start_daemon() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into());
    state.start_lead(wheeld::lead::LeadConfig {
        command: env!("CARGO_BIN_EXE_wheel-fake-agent").into(),
        cwd: "/tmp".into(),
        context: wheeld::lead::ORCHESTRATOR_CONTEXT.into(),
    });
    tokio::spawn(wheeld::serve(listener, state));
    format!("ws://{addr}/ws?token={TOKEN}")
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
        let frame = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Collect the lead's reply text until the turn ends.
async fn turn_text(ws: &mut Ws) -> String {
    let mut text = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::Lead {
                event: LeadEvent::Text { text: chunk },
            } => text.push_str(&chunk),
            ServerMsg::Lead {
                event: LeadEvent::TurnEnd { stop_reason, .. },
            } => {
                assert_eq!(stop_reason, "end_turn");
                return text;
            }
            ServerMsg::Lead {
                event: LeadEvent::Error { message, .. },
            } => panic!("lead error: {message}"),
            _ => {}
        }
    }
}

async fn wait_ready(ws: &mut Ws) {
    for _ in 0..100 {
        send(ws, ClientMsg::Status { req: Some(99) }).await;
        loop {
            if let ServerMsg::Status { lead, .. } = recv(ws).await {
                if lead == Some(LeadState::Ready) {
                    return;
                }
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("lead never became ready");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_streams_back_with_the_orchestrator_context() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    send(&mut ws, ClientMsg::LeadAttach { req: Some(1) }).await;
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::LeadAttached { req: Some(1), .. }
    ) {}
    send(
        &mut ws,
        ClientMsg::LeadSend {
            req: Some(2),
            text: "hello".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(
        text.contains("context: yes"),
        "the session got the orchestrator context: {text:?}"
    );
    assert!(text.contains("echo: hello"), "{text:?}");

    // A client that attaches afterwards gets the whole conversation, chunks merged.
    let mut late = connect(&url).await;
    send(&mut late, ClientMsg::LeadAttach { req: Some(3) }).await;
    let events = loop {
        if let ServerMsg::LeadAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    assert_eq!(
        events.first(),
        Some(&LeadEvent::User {
            turn: 1,
            text: "hello".into()
        })
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, LeadEvent::Text { text } if text.contains("echo: hello"))));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_reaches_every_client_and_the_first_answer_wins() {
    let url = start_daemon().await;
    let mut asker = connect(&url).await;
    let mut other = connect(&url).await;
    wait_ready(&mut asker).await;
    send(&mut asker, ClientMsg::LeadAttach { req: Some(1) }).await;
    while !matches!(recv(&mut asker).await, ServerMsg::LeadAttached { .. }) {}
    send(
        &mut asker,
        ClientMsg::LeadSend {
            req: Some(2),
            text: "needs permission".into(),
        },
    )
    .await;

    // The other client never attached to the lead, and still sees the request.
    let perm = loop {
        if let ServerMsg::PermRequested { perm } = recv(&mut other).await {
            break perm;
        }
    };
    assert_eq!(perm.session, "lead");
    assert_eq!(perm.title, "rm -rf build");
    send(
        &mut other,
        ClientMsg::PermAnswer {
            req: Some(3),
            id: perm.id.clone(),
            choice: "allow".into(),
        },
    )
    .await;
    loop {
        match recv(&mut other).await {
            ServerMsg::Ok { req: 3 } => break,
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }

    let text = turn_text(&mut asker).await;
    assert!(
        text.contains("permission: allow-once"),
        "the agent got the answer: {text:?}"
    );

    send(
        &mut asker,
        ClientMsg::PermAnswer {
            req: Some(4),
            id: perm.id.clone(),
            choice: "deny".into(),
        },
    )
    .await;
    loop {
        match recv(&mut asker).await {
            ServerMsg::Error {
                req: Some(4),
                message,
            } => {
                assert!(
                    message.contains(&format!("no waiting permission {}", perm.id)),
                    "{message}"
                );
                break;
            }
            ServerMsg::Ok { req: 4 } => panic!("a second answer was accepted"),
            _ => {}
        }
    }
}

async fn attach_and_send(ws: &mut Ws, text: &str) {
    send(ws, ClientMsg::LeadAttach { req: Some(10) }).await;
    while !matches!(
        recv(ws).await,
        ServerMsg::LeadAttached { req: Some(10), .. }
    ) {}
    send(
        ws,
        ClientMsg::LeadSend {
            req: Some(11),
            text: text.into(),
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_turn_withdraws_its_permission_request() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach_and_send(&mut ws, "needs permission").await;
    let perm = loop {
        if let ServerMsg::PermRequested { perm } = recv(&mut ws).await {
            break perm;
        }
    };
    send(&mut ws, ClientMsg::LeadCancel { req: Some(12) }).await;
    // Every client hears the request is gone, and the agent is told it was cancelled.
    loop {
        if let ServerMsg::PermResolved { id, choice } = recv(&mut ws).await {
            assert_eq!(id, perm.id);
            assert_eq!(choice, "withdrawn");
            break;
        }
    }
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: cancelled"), "{text:?}");
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(13),
            id: perm.id.clone(),
            choice: "allow".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error { req: Some(13), .. } => break,
            ServerMsg::Ok { req: 13 } => panic!("a withdrawn request accepted an answer"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_agent_is_restarted_and_the_lead_keeps_working() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach_and_send(&mut ws, "die now").await;
    loop {
        if let ServerMsg::Lead {
            event: LeadEvent::Error { turn: None, .. },
        } = recv(&mut ws).await
        {
            break;
        }
    }
    wait_ready(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::LeadSend {
            req: Some(20),
            text: "back again".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: back again"), "{text:?}");
}
