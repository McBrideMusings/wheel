//! `pane_shot` over the socket, with scripted clients standing in for the Mac app: the daemon asks
//! the newest client that offered to render, falls through to the next when one does not show the
//! pane, writes the image whole, and says which of its failures it hit.

mod common;

use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "pane-shot-test-token";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image, but it starts like one";

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

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wheel-shot-{name}-{}", std::process::id()));
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

/// A client that offered to render and has been heard by the daemon.
async fn renderer(url: &str) -> Ws {
    let mut ws = connect(url).await;
    send(&mut ws, ClientMsg::RenderOffer {}).await;
    // A request that comes back proves the offer was read before the next client's.
    send(&mut ws, ClientMsg::Status { req: Some(1) }).await;
    loop {
        if matches!(recv(&mut ws).await, ServerMsg::Status { .. }) {
            return ws;
        }
    }
}

async fn shot(ws: &mut Ws, pane: &str, path: &str) {
    send(
        ws,
        ClientMsg::PaneShot {
            req: Some(7),
            pane: pane.into(),
            path: path.into(),
        },
    )
    .await;
}

/// The reply to `pane_shot`: the byte count, or the daemon's error.
async fn shot_reply(ws: &mut Ws) -> Result<u64, String> {
    loop {
        match recv(ws).await {
            ServerMsg::PaneShot { req: 7, bytes, .. } => return Ok(bytes),
            ServerMsg::Error {
                req: Some(7),
                message,
                ..
            } => return Err(message),
            _ => {}
        }
    }
}

/// The next `render_pane` request this client is sent.
async fn render_request(ws: &mut Ws) -> (u64, String) {
    loop {
        if let ServerMsg::RenderPane { id, pane } = recv(ws).await {
            return (id, pane);
        }
    }
}

#[tokio::test]
async fn a_shot_asks_the_newest_renderer_and_writes_the_file_whole() {
    let dir = scratch("ok");
    let (url, daemon) = start(&dir).await;
    let mut cli = connect(&url).await;
    let out = dir.join("pane.png");
    let out_text = out.to_str().unwrap();

    // Nobody renders: the error says so, and nothing is written.
    shot(&mut cli, "p1", out_text).await;
    let message = shot_reply(&mut cli).await.unwrap_err();
    assert!(message.contains("no client is connected"), "{message}");
    assert!(!out.exists());

    // A relative path is refused before any client is asked.
    shot(&mut cli, "p1", "pane.png").await;
    let message = shot_reply(&mut cli).await.unwrap_err();
    assert!(message.contains("not an absolute path"), "{message}");

    // Two renderers: the newer is asked first, says it does not show the pane, and the older one
    // is asked next and answers.
    let mut older = renderer(&url).await;
    let mut newer = renderer(&url).await;
    shot(&mut cli, "p1", out_text).await;
    let (id, pane) = render_request(&mut newer).await;
    assert_eq!(pane, "p1");
    send(
        &mut newer,
        ClientMsg::RenderRefused {
            id,
            message: "not shown".into(),
        },
    )
    .await;
    let (id, pane) = render_request(&mut older).await;
    assert_eq!(pane, "p1");
    send(
        &mut older,
        ClientMsg::Rendered {
            id,
            png: base64::engine::general_purpose::STANDARD.encode(PNG),
        },
    )
    .await;
    assert_eq!(shot_reply(&mut cli).await, Ok(PNG.len() as u64));
    assert_eq!(std::fs::read(&out).unwrap(), PNG);

    // Every client refusing as "not shown" is one message naming the pane; any other refusal is
    // passed on.
    shot(&mut cli, "p999", out_text).await;
    let (id, _) = render_request(&mut newer).await;
    send(
        &mut newer,
        ClientMsg::RenderRefused {
            id,
            message: "not shown".into(),
        },
    )
    .await;
    let (id, _) = render_request(&mut older).await;
    send(
        &mut older,
        ClientMsg::RenderRefused {
            id,
            message: "not shown".into(),
        },
    )
    .await;
    assert_eq!(
        shot_reply(&mut cli).await,
        Err("no connected client shows pane p999".into())
    );
    shot(&mut cli, "p1", out_text).await;
    let (id, _) = render_request(&mut newer).await;
    send(
        &mut newer,
        ClientMsg::RenderRefused {
            id,
            message: "the view has no size".into(),
        },
    )
    .await;
    drop(older);
    let message = shot_reply(&mut cli).await.unwrap_err();
    assert!(message.contains("the view has no size"), "{message}");

    // A client that sends something that is not a PNG writes nothing new.
    let before = std::fs::read(&out).unwrap();
    shot(&mut cli, "p1", out_text).await;
    let (id, _) = render_request(&mut newer).await;
    send(
        &mut newer,
        ClientMsg::Rendered {
            id,
            png: base64::engine::general_purpose::STANDARD.encode(b"GIF89a"),
        },
    )
    .await;
    let message = shot_reply(&mut cli).await.unwrap_err();
    assert!(message.contains("not a PNG"), "{message}");
    assert_eq!(std::fs::read(&out).unwrap(), before);

    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
