//! `notify` sends a toast to every connection attached now, the sender included, and keeps nothing
//! for one that attaches later.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wheel_proto::{NoticeLevel, ServerMsg};

const TOKEN: &str = "notify-test-token";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_toast(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("a toast arrived within two seconds")
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        let msg: ServerMsg = serde_json::from_str(&text).unwrap();
        if matches!(msg, ServerMsg::Toast { .. } | ServerMsg::ToastClear { .. }) {
            return msg;
        }
    }
}

#[tokio::test]
async fn a_notify_reaches_every_attached_connection_and_a_clear_follows() {
    let dir = std::env::temp_dir().join(format!("wheel-notify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    let daemon = common::Daemon::new(serving, state);
    let url = format!("ws://{addr}/ws?token={TOKEN}");
    let (mut sender, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
    let (mut watcher, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();

    sender
        .send(Message::Text(
            r#"{"op":"notify","req":1,"id":"n1","level":"error","text":"it broke","pane":"p4"}"#.into(),
        ))
        .await
        .unwrap();
    let want = ServerMsg::Toast {
        id: "n1".into(),
        level: NoticeLevel::Error,
        text: "it broke".into(),
        pane: Some("p4".into()),
    };
    assert_eq!(next_toast(&mut watcher).await, want);
    assert_eq!(next_toast(&mut sender).await, want);

    sender
        .send(Message::Text(r#"{"op":"notify_clear","req":2,"id":"n1"}"#.into()))
        .await
        .unwrap();
    assert_eq!(
        next_toast(&mut watcher).await,
        ServerMsg::ToastClear { id: "n1".into() }
    );

    // An empty text is refused under its req, and nothing is broadcast.
    sender
        .send(Message::Text(r#"{"op":"notify","req":3,"id":"n2","level":"notice","text":"  "}"#.into()))
        .await
        .unwrap();
    let refusal = loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), sender.next()).await.unwrap().unwrap().unwrap();
        let Message::Text(text) = frame else { continue };
        if let ServerMsg::Error { req, message, .. } = serde_json::from_str(&text).unwrap() {
            break (req, message);
        }
    };
    assert_eq!(refusal.0, Some(3));
    assert!(refusal.1.contains("1 to 4000 bytes"), "{}", refusal.1);

    // A connection made afterwards is told nothing: the daemon holds no toast.
    let (mut late, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let heard = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let Some(Ok(Message::Text(text))) = late.next().await else { break false };
            if let Ok(ServerMsg::Toast { .. }) = serde_json::from_str(&text) {
                break true;
            }
        }
    })
    .await;
    assert!(!matches!(heard, Ok(true)), "a late connection was sent a toast");
    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
