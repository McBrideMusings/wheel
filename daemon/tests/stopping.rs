//! When wheeld stops, every attached client is sent `stopping` and the socket closes after it, so a
//! client can tell a stop or restart from a crash.

mod common;

use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wheel_proto::ServerMsg;

const TOKEN: &str = "stopping-test-token";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Everything the client receives until the socket ends: the messages, then whether it closed.
async fn until_closed(ws: &mut Ws) -> (Vec<ServerMsg>, bool) {
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => seen.push(serde_json::from_str(&text).unwrap()),
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_)) | None) => return (seen, true),
            Err(_) => return (seen, false),
        }
    }
}

#[tokio::test]
async fn every_client_gets_stopping_and_then_the_socket_closes() {
    let dir = std::env::temp_dir().join(format!("wheel-stopping-{}", std::process::id()));
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
    let daemon = common::Daemon::new(serving, state.clone());
    let url = format!("ws://{addr}/ws?token={TOKEN}");
    let (mut first, _) = tokio_tungstenite::connect_async(url.clone()).await.unwrap();
    let (mut second, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Both connections are counted once the server has taken them over from the accept loop.
    let waited = std::time::Instant::now();
    while state.open_connections() < 2 {
        assert!(waited.elapsed() < Duration::from_secs(5), "the connections never registered");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    state.announce_stopping().await;

    for ws in [&mut first, &mut second] {
        let (seen, closed) = until_closed(ws).await;
        assert!(seen.contains(&ServerMsg::Stopping), "messages: {seen:?}");
        assert!(closed, "the socket stayed open after `stopping`");
    }
    // A client that connects after wheeld began stopping is told too.
    let (mut late, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token={TOKEN}"))
        .await
        .unwrap();
    let (seen, closed) = until_closed(&mut late).await;
    assert!(seen.contains(&ServerMsg::Stopping), "late client messages: {seen:?}");
    assert!(closed, "the late client's socket stayed open");
    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
