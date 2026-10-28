//! The terminal contract over the socket: replay on reattach, an exited or closed terminal leaves
//! `status`, one stream per attach, and no shell for a client without the token or from a browser.

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
            req: None,
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
    // Output that follows the marker (the next prompt) can arrive before the reply.
    let terms = loop {
        if let ServerMsg::Status { terms, .. } = recv(&mut second).await {
            break terms;
        }
    };
    assert_eq!(terms.len(), 1);
    assert!(terms[0].alive);
    assert_eq!(terms[0].cwd, "/tmp");
}

/// Attach and return the replay: the `replay_bytes` of output that follow `term_attached`.
async fn attach_replay(ws: &mut Ws, id: &str) -> String {
    send(
        ws,
        ClientMsg::TermAttach {
            req: Some(3),
            id: id.into(),
        },
    )
    .await;
    let ServerMsg::TermAttached { replay_bytes, .. } = recv(ws).await else {
        panic!("expected term_attached")
    };
    let mut replay = Vec::new();
    while replay.len() < replay_bytes {
        if let ServerMsg::TermOutput { data, .. } = recv(ws).await {
            replay.extend(decode_bytes(&data).unwrap());
        }
    }
    assert_eq!(replay.len(), replay_bytes);
    String::from_utf8_lossy(&replay).into_owned()
}

#[tokio::test]
async fn a_long_replay_arrives_as_bounded_frames_that_add_up_to_replay_bytes() {
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
    input(
        &mut first,
        &id,
        "seq 1 10000 | sed 's/$/ line-of-scrollback-padding/'; echo done-$((40+2))\r",
    )
    .await;
    read_until(&mut first, "done-42").await;
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
    assert!(replay_bytes > 128 * 1024, "replay is {replay_bytes} bytes");
    let (mut total, mut frames, mut largest) = (0, 0, 0);
    while total < replay_bytes {
        if let ServerMsg::TermOutput { data, .. } = recv(&mut second).await {
            let len = decode_bytes(&data).unwrap().len();
            total += len;
            frames += 1;
            largest = largest.max(len);
        }
    }
    assert_eq!(total, replay_bytes);
    assert!(frames >= 2, "one frame carried {total} bytes");
    assert!(largest <= 64 * 1024, "largest frame was {largest} bytes");
}

#[tokio::test]
async fn reattach_to_a_full_screen_app_redraws_its_screen_over_the_shell_screen() {
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
    // Like vim: shell output, then the alternate screen, cleared, with text drawn at row 3; the
    // sleep keeps the shell from drawing a prompt over it.
    input(
        &mut first,
        &id,
        "echo before-$((1+1)); printf '\\033[?1049h\\033[2J\\033[3;5Hfull-%s' $((40+2)); sleep 5\r",
    )
    .await;
    read_until(&mut first, "full-42").await;
    first.close(None).await.unwrap();

    let mut second = connect(&url).await;
    let replay = attach_replay(&mut second, &id).await;
    // The shell's screen first, then the switch to the alternate screen, then what the app drew,
    // so the shell's output is there again when the app exits.
    let at = |text: &str| {
        replay
            .find(text)
            .unwrap_or_else(|| panic!("no {text:?}: {replay:?}"))
    };
    assert!(at("before-2") < at("\x1b[?1049h"), "{replay:?}");
    assert!(at("\x1b[?1049h") < at("full-42"), "{replay:?}");
    // The cursor is where the app left it, placed last: row 3, after `full-42` at column 5.
    let cursor = replay
        .rfind("\x1b[3;12H")
        .unwrap_or_else(|| panic!("{replay:?}"));
    assert!(at("full-42") < cursor, "{replay:?}");
    assert!(
        !replay[cursor + "\x1b[3;12H".len()..].contains('H'),
        "{replay:?}"
    );
}

/// The terminals `status` lists, by id.
async fn listed(ws: &mut Ws) -> Vec<String> {
    send(ws, ClientMsg::Status { req: Some(90) }).await;
    loop {
        if let ServerMsg::Status { terms, .. } = recv(ws).await {
            return terms.into_iter().map(|t| t.id).collect();
        }
    }
}

/// The reply to the request numbered `req`, skipping terminal output.
async fn reply(ws: &mut Ws, req: u64) -> ServerMsg {
    loop {
        match recv(ws).await {
            ServerMsg::Ok { req: r } if r == req => return ServerMsg::Ok { req: r },
            reply @ ServerMsg::Error { req: Some(r), .. } if r == req => return reply,
            _ => {}
        }
    }
}

#[tokio::test]
async fn an_exited_terminal_is_dropped_once_its_exit_is_delivered() {
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
    input(&mut ws, &id, "exit 7\r").await;
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
    // The exit reaches attached clients before the daemon forgets the terminal, so wait briefly.
    let mut left = listed(&mut ws).await;
    for _ in 0..2400 {
        if left.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        left = listed(&mut ws).await;
    }
    assert_eq!(left, Vec::<String>::new(), "status still lists {id}");

    send(
        &mut ws,
        ClientMsg::TermAttach {
            req: Some(3),
            id: id.clone(),
        },
    )
    .await;
    let ServerMsg::Error { message, .. } = reply(&mut ws, 3).await else {
        panic!("expected an error")
    };
    assert_eq!(message, format!("no terminal {id}"));
}

#[tokio::test]
async fn an_exit_nobody_saw_waits_for_the_next_attach() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let id = open_shell(&mut ws).await;
    input(&mut ws, &id, "exit 7\r").await;
    let mut alive = true;
    for _ in 0..1200 {
        send(&mut ws, ClientMsg::Status { req: None }).await;
        if let ServerMsg::Status { terms, .. } = recv(&mut ws).await {
            assert_eq!(
                terms.len(),
                1,
                "an undelivered exit keeps the terminal listed"
            );
            alive = terms[0].alive;
            if !alive {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive, "the shell exited");

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
    let mut left = listed(&mut ws).await;
    for _ in 0..2400 {
        if left.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        left = listed(&mut ws).await;
    }
    assert_eq!(left, Vec::<String>::new(), "status still lists {id}");
}

#[tokio::test]
async fn term_close_ends_a_terminal_and_status_stops_listing_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let id = open_shell(&mut ws).await;
    let other = open_shell(&mut ws).await;
    let mut both = vec![id.clone(), other.clone()];
    both.sort();
    assert_eq!(listed(&mut ws).await, both);

    send(
        &mut ws,
        ClientMsg::TermClose {
            req: Some(5),
            id: id.clone(),
        },
    )
    .await;
    assert!(matches!(reply(&mut ws, 5).await, ServerMsg::Ok { req: 5 }));
    assert_eq!(listed(&mut ws).await, vec![other.clone()]);

    send(
        &mut ws,
        ClientMsg::TermClose {
            req: Some(6),
            id: id.clone(),
        },
    )
    .await;
    let ServerMsg::Error { message, .. } = reply(&mut ws, 6).await else {
        panic!("closing twice is an error")
    };
    assert_eq!(message, format!("no terminal {id}"));
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

#[tokio::test]
async fn an_error_from_a_terminal_op_names_the_terminal() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    input(&mut ws, "t-missing", "ls\r").await;
    assert_eq!(
        recv(&mut ws).await,
        ServerMsg::Error {
            req: None,
            term: Some("t-missing".into()),
            message: "no terminal t-missing".into(),
        }
    );
}
