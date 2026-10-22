//! `window` commands over the socket, with a scripted client standing in for the Mac app: the
//! daemon stores no windows, forwards each command to the attached app and returns its answer,
//! and says so when no app is attached.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, WindowCommand};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "mac-windows-test-token";

async fn start(dir: &std::path::Path) -> (String, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.to_path_buf(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        common::Daemon::new(serving, state),
    )
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

/// The reply to the command sent as request 7: its data, or the daemon's error.
async fn reply(ws: &mut Ws) -> Result<serde_json::Value, String> {
    loop {
        match recv(ws).await {
            ServerMsg::WindowReply { req: 7, data } => return Ok(data),
            ServerMsg::Error {
                req: Some(7),
                message,
                ..
            } => return Err(message),
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_window_command_reaches_the_attached_mac_app_and_back() {
    let dir = std::env::temp_dir().join(format!("wheel-windows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (url, daemon) = start(&dir).await;
    let mut cli = connect(&url).await;

    send(
        &mut cli,
        ClientMsg::Window {
            req: Some(7),
            command: WindowCommand::Ls,
        },
    )
    .await;
    assert_eq!(reply(&mut cli).await.unwrap_err(), "no Mac app attached");

    // The app attaches; a command arrives as a request and its answer is the reply.
    let mut app = connect(&url).await;
    send(&mut app, ClientMsg::WindowOffer {}).await;
    send(&mut app, ClientMsg::Status { req: Some(1) }).await;
    while !matches!(recv(&mut app).await, ServerMsg::Status { .. }) {}

    send(
        &mut cli,
        ClientMsg::Window {
            req: Some(7),
            command: WindowCommand::New {
                name: Some("Reviews".into()),
                space: Some("plex".into()),
            },
        },
    )
    .await;
    let (id, command) = loop {
        if let ServerMsg::WindowRequest { id, command } = recv(&mut app).await {
            break (id, command);
        }
    };
    assert_eq!(
        command,
        WindowCommand::New {
            name: Some("Reviews".into()),
            space: Some("plex".into())
        }
    );
    send(
        &mut app,
        ClientMsg::WindowAnswer {
            id,
            data: Some(serde_json::json!({"window": "w-1"})),
            error: None,
        },
    )
    .await;
    assert_eq!(
        reply(&mut cli).await.unwrap(),
        serde_json::json!({"window": "w-1"})
    );

    // The app refuses: its words come back as the error.
    send(
        &mut cli,
        ClientMsg::Window {
            req: Some(7),
            command: WindowCommand::Close {
                window: "nope".into(),
            },
        },
    )
    .await;
    let id = loop {
        if let ServerMsg::WindowRequest { id, .. } = recv(&mut app).await {
            break id;
        }
    };
    send(
        &mut app,
        ClientMsg::WindowAnswer {
            id,
            data: None,
            error: Some("no window nope".into()),
        },
    )
    .await;
    assert_eq!(reply(&mut cli).await.unwrap_err(), "no window nope");

    // The app quits: commands fail again.
    drop(app);
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(
        &mut cli,
        ClientMsg::Window {
            req: Some(7),
            command: WindowCommand::Ls,
        },
    )
    .await;
    assert_eq!(reply(&mut cli).await.unwrap_err(), "no Mac app attached");
    drop(daemon);
    let _ = std::fs::remove_dir_all(&dir);
}
