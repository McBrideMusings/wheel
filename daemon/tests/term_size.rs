//! A terminal has one size: the newest client's request. When that client detaches or
//! disconnects, the terminal returns to the size the client before it asked for.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "test-token";

async fn start_daemon() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wheeld::serve(
        listener,
        wheeld::AppState::new(
            "/dev/null".into(),
            TOKEN.into(),
            std::env::temp_dir(),
            env!("CARGO_BIN_EXE_wheeld").into(),
        ),
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
        let frame = tokio::time::timeout(Duration::from_secs(60), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn size_of(ws: &mut Ws, id: &str) -> (u16, u16) {
    send(ws, ClientMsg::Status { req: Some(99) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(99),
            terms,
            ..
        } = recv(ws).await
        {
            let t = terms.iter().find(|t| t.id == id).expect("terminal listed");
            return (t.cols, t.rows);
        }
    }
}

async fn resize(ws: &mut Ws, id: &str, cols: u16, rows: u16) {
    send(
        ws,
        ClientMsg::TermResize {
            req: Some(50),
            id: id.into(),
            cols,
            rows,
        },
    )
    .await;
    loop {
        if matches!(recv(ws).await, ServerMsg::Ok { req: 50 }) {
            return;
        }
    }
}

/// Poll until the terminal has this size: a disconnect is seen by the daemon a moment later.
async fn settles_at(ws: &mut Ws, id: &str, want: (u16, u16)) {
    for _ in 0..50 {
        if size_of(ws, id).await == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(size_of(ws, id).await, want);
}

#[tokio::test]
async fn a_client_that_leaves_gives_the_terminal_back_its_previous_size() {
    let url = start_daemon().await;
    let mut mac = connect(&url).await;
    send(
        &mut mac,
        ClientMsg::TermOpen {
            req: Some(1),
            cwd: Some("/tmp".into()),
            cols: 100,
            rows: 30,
        },
    )
    .await;
    let ServerMsg::TermOpened { id, .. } = recv(&mut mac).await else {
        panic!("expected term_opened")
    };
    resize(&mut mac, &id, 188, 48).await;
    assert_eq!(size_of(&mut mac, &id).await, (188, 48));

    let mut other = connect(&url).await;
    resize(&mut other, &id, 72, 25).await;
    assert_eq!(size_of(&mut mac, &id).await, (72, 25));

    other.close(None).await.unwrap();
    settles_at(&mut mac, &id, (188, 48)).await;
}

#[tokio::test]
async fn a_client_that_detaches_gives_the_terminal_back_its_previous_size() {
    let url = start_daemon().await;
    let mut mac = connect(&url).await;
    send(
        &mut mac,
        ClientMsg::TermOpen {
            req: Some(1),
            cwd: Some("/tmp".into()),
            cols: 100,
            rows: 30,
        },
    )
    .await;
    let ServerMsg::TermOpened { id, .. } = recv(&mut mac).await else {
        panic!("expected term_opened")
    };
    resize(&mut mac, &id, 188, 48).await;
    let mut phone = connect(&url).await;
    resize(&mut phone, &id, 40, 20).await;
    assert_eq!(size_of(&mut mac, &id).await, (40, 20));

    send(&mut phone, ClientMsg::TermDetach { id: id.clone() }).await;
    settles_at(&mut mac, &id, (188, 48)).await;
}
