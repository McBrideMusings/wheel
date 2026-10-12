//! The terminal contract over the socket: replay on reattach, exit seen after the fact, one stream
//! per attach, and no shell for a client without the token or from a browser.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{decode_bytes, encode_bytes, ClientMsg, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "test-token";

async fn start_daemon() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wheeld::serve(
        listener,
        wheeld::AppState::new("/dev/null".into(), TOKEN.into()),
    ));
    format!("ws://{addr}/ws")
}

async fn connect(base: &str) -> Ws {
    tokio_tungstenite::connect_async(format!("{base}?token={TOKEN}"))
        .await
        .unwrap()
        .0
}

async fn send(ws: &mut Ws, msg: ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
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

/// Read terminal output until `needle` appears; return everything read.
async fn read_until(ws: &mut Ws, needle: &str) -> String {
    let mut seen = String::new();
    while !seen.contains(needle) {
        if let ServerMsg::TermOutput { data, .. } = recv(ws).await {
            seen.push_str(&String::from_utf8_lossy(&decode_bytes(&data).unwrap()));
        }
    }
    seen
}

async fn open_shell(ws: &mut Ws) -> String {
    send(
        ws,
        ClientMsg::TermOpen {
            req: Some(1),
            cwd: Some("/tmp".into()),
            cols: 100,
            rows: 30,
        },
    )
    .await;
    let ServerMsg::TermOpened {
        id, req: Some(1), ..
    } = recv(ws).await
    else {
        panic!("expected term_opened")
    };
    id
}

async fn input(ws: &mut Ws, id: &str, text: &str) {
    send(
        ws,
        ClientMsg::TermInput {
            id: id.into(),
            data: encode_bytes(text.as_bytes()),
        },
    )
    .await;
}

#[tokio::test]
async fn reattach_replays_output_written_before_the_disconnect() {
    let url = start_daemon().await;
    let mut first = connect(&url).await;
    let id = open_shell(&mut first).await;
    send(
        &mut first,
        ClientMsg::TermAttach {
            req: Some(2),
            id: id.clone(),
        },
    )
    .await;
    // The marker only appears in output once the shell has run the arithmetic, never in the echoed input.
    input(&mut first, &id, "echo wheel-$((40+2))-marker\r").await;
    read_until(&mut first, "wheel-42-marker").await;
    first.close(None).await.unwrap();

    let mut second = connect(&url).await;
    send(
        &mut second,
        ClientMsg::TermAttach {
            req: Some(3),
            id: id.clone(),
        },
    )
    .await;
    let ServerMsg::TermAttached { replay_bytes, .. } = recv(&mut second).await else {
        panic!("expected term_attached")
    };
    assert!(replay_bytes > 0, "the daemon kept output for replay");
    let replay = read_until(&mut second, "wheel-42-marker").await;
    assert!(replay.contains("wheel-42-marker"));

    send(&mut second, ClientMsg::Status { req: Some(4) }).await;
    let ServerMsg::Status { terms, .. } = recv(&mut second).await else {
        panic!("expected status")
    };
    assert_eq!(terms.len(), 1);
    assert!(terms[0].alive);
    assert_eq!(terms[0].cwd, "/tmp");
}

#[tokio::test]
async fn attaching_after_the_shell_exited_reports_the_exit() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let id = open_shell(&mut ws).await;
    input(&mut ws, &id, "exit 7\r").await;
    for _ in 0..100 {
        send(&mut ws, ClientMsg::Status { req: None }).await;
        if let ServerMsg::Status { terms, .. } = recv(&mut ws).await {
            if !terms[0].alive {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    send(
        &mut ws,
        ClientMsg::TermAttach {
            req: Some(2),
            id: id.clone(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::TermExited { code, .. } => {
                assert_eq!(code, Some(7));
                break;
            }
            ServerMsg::TermAttached { .. } | ServerMsg::TermOutput { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn attaching_twice_streams_output_once() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let id = open_shell(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::TermAttach {
            req: Some(2),
            id: id.clone(),
        },
    )
    .await;
    send(
        &mut ws,
        ClientMsg::TermAttach {
            req: Some(3),
            id: id.clone(),
        },
    )
    .await;
    // The second attach replaces the first stream, which may end before it sends anything.
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::TermAttached { req: Some(3), .. }
    ) {}
    input(&mut ws, &id, "echo once-$((6*7))-done\r").await;
    let seen = read_until(&mut ws, "once-42-done").await;
    // Wait for any duplicate stream to deliver too, then count.
    let mut tail = seen;
    while let Ok(Some(Ok(Message::Text(text)))) =
        tokio::time::timeout(Duration::from_millis(500), ws.next()).await
    {
        if let Ok(ServerMsg::TermOutput { data, .. }) = serde_json::from_str(&text) {
            tail.push_str(&String::from_utf8_lossy(&decode_bytes(&data).unwrap()));
        }
    }
    assert_eq!(
        tail.matches("once-42-done").count(),
        1,
        "output arrived once: {tail:?}"
    );
}

#[tokio::test]
async fn refuses_clients_without_the_token_or_from_a_browser() {
    let url = start_daemon().await;
    assert!(
        tokio_tungstenite::connect_async(&url).await.is_err(),
        "no token"
    );
    assert!(
        tokio_tungstenite::connect_async(format!("{url}?token=wrong"))
            .await
            .is_err(),
        "wrong token"
    );
    let mut req = format!("{url}?token={TOKEN}")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("origin", "https://example.com".parse().unwrap());
    assert!(
        tokio_tungstenite::connect_async(req).await.is_err(),
        "browser origin"
    );
}
