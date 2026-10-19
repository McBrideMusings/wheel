//! A frame the daemon cannot parse gets an error under its `req`, so a client waiting on it hears
//! at once instead of timing out.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wheel_proto::ServerMsg;

const TOKEN: &str = "unknown-op-test-token";

async fn error_reply(frame: &str) -> (Option<u64>, String) {
    let dir = std::env::temp_dir().join(format!("wheel-unknown-op-{}", std::process::id()));
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
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
    let reply = loop {
        let frame = tokio::time::timeout(Duration::from_secs(1), ws.next())
            .await
            .expect("daemon replied within a second")
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        if let ServerMsg::Error { req, message, .. } = serde_json::from_str(&text).unwrap() {
            break (req, message);
        }
    };
    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
    reply
}

#[tokio::test]
async fn unknown_op_with_a_req_gets_an_error_naming_the_op() {
    let (req, message) = error_reply(r#"{"op":"from_the_future","req":7}"#).await;
    assert_eq!(req, Some(7));
    assert_eq!(
        message,
        "this wheeld doesn't support from_the_future; it is older than the CLI"
    );
}

#[tokio::test]
async fn malformed_req_gets_a_plain_bad_message_error() {
    let (req, message) = error_reply(r#"{"op":"status","req":"seven"}"#).await;
    assert_eq!(req, None);
    assert!(message.starts_with("bad message:"), "{message}");
}
