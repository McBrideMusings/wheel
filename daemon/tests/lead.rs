//! The lead over the socket, driven by the scripted fake agent: a turn streams back with the
//! orchestrator context in place, and a permission request reaches every client, where the first
//! answer resumes the agent and a second answer is refused; the lead changes spaces through the
//! daemon's MCP tools and starts workers, sessions that agent panes own.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, SessionEvent, SessionState, ToolContent};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "lead-test-token";

async fn start_daemon() -> String {
    start_daemon_at().await.0
}

/// The daemon's WebSocket url and its address.
async fn start_daemon_at() -> (String, std::net::SocketAddr) {
    let (url, addr, _) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    (url, addr)
}

/// A daemon whose lead and workers run `command`, keeping its layout in `state_file`; the handle
/// stops it serving when aborted.
async fn start_daemon_with(
    command: &str,
    state_file: Option<std::path::PathBuf>,
) -> (
    String,
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into());
    // The lead first: restoring a saved agent pane starts its worker with the lead's command.
    state.start_lead(command.into(), "/tmp".into(), &wheeld::mcp::url(addr));
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state));
    (format!("ws://{addr}/ws?token={TOKEN}"), addr, serving)
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
            ServerMsg::Session {
                event: SessionEvent::Text { text: chunk },
                ..
            } => text.push_str(&chunk),
            ServerMsg::Session {
                event: SessionEvent::TurnEnd { stop_reason, .. },
                ..
            } => {
                assert_eq!(stop_reason, "end_turn");
                return text;
            }
            ServerMsg::Session {
                event: SessionEvent::Error { message, .. },
                ..
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
                if lead == Some(SessionState::Ready) {
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
    send(
        &mut ws,
        ClientMsg::SessionAttach {
            req: Some(1),
            session: "lead".into(),
        },
    )
    .await;
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::SessionAttached { req: Some(1), .. }
    ) {}
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: "lead".into(),
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
    send(
        &mut late,
        ClientMsg::SessionAttach {
            req: Some(3),
            session: "lead".into(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::SessionAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    assert_eq!(
        events[..2],
        [
            SessionEvent::User {
                turn: 1,
                text: "hello".into()
            },
            SessionEvent::TurnStart { turn: 1 }
        ]
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, SessionEvent::Text { text } if text.contains("echo: hello"))));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_reaches_every_client_and_the_first_answer_wins() {
    let url = start_daemon().await;
    let mut asker = connect(&url).await;
    let mut other = connect(&url).await;
    wait_ready(&mut asker).await;
    send(
        &mut asker,
        ClientMsg::SessionAttach {
            req: Some(1),
            session: "lead".into(),
        },
    )
    .await;
    while !matches!(recv(&mut asker).await, ServerMsg::SessionAttached { .. }) {}
    send(
        &mut asker,
        ClientMsg::SessionSend {
            session: "lead".into(),
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

    // The request names the tool call the agent announced just before asking. Lead events and
    // permission requests reach a client through separate forwarders, so either can arrive first.
    let (mut announced, mut gated) = (None, None);
    while announced.is_none() || gated.is_none() {
        match recv(&mut asker).await {
            ServerMsg::Session {
                event: SessionEvent::ToolCall { id, .. },
                ..
            } => announced = Some(id),
            ServerMsg::PermRequested { perm } => gated = Some(perm.tool_call),
            _ => {}
        }
    }
    assert_eq!(announced.as_deref(), Some("call-1"));
    assert_eq!(gated.flatten(), announced);
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
                ..
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

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_message_shows_at_once_and_starts_after_the_running_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    // Turn 1 waits on a permission answer, so turn 2 queues behind it.
    attach_and_send(&mut ws, "needs permission").await;
    let perm = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: "lead".into(),
            req: Some(12),
            text: "second".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::Session { event, .. } = recv(&mut ws).await {
            match event {
                SessionEvent::User { turn: 2, text } => {
                    assert_eq!(text, "second");
                    break;
                }
                other => assert!(
                    !matches!(
                        other,
                        SessionEvent::TurnEnd { .. } | SessionEvent::TurnStart { turn: 2 }
                    ),
                    "{other:?} came before the queued message"
                ),
            }
        }
    }

    // A client attaching now sees the queued message, not yet started.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::SessionAttach {
            req: Some(3),
            session: "lead".into(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::SessionAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let kinds: Vec<_> = events
        .iter()
        .filter(|e| !matches!(e, SessionEvent::Text { .. } | SessionEvent::ToolCall { .. }))
        .cloned()
        .collect();
    assert_eq!(
        kinds,
        [
            SessionEvent::User {
                turn: 1,
                text: "needs permission".into()
            },
            SessionEvent::TurnStart { turn: 1 },
            SessionEvent::User {
                turn: 2,
                text: "second".into()
            },
        ]
    );

    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    let mut order = Vec::new();
    while order.last() != Some(&"turn_end 2".to_string()) {
        if let ServerMsg::Session { event, .. } = recv(&mut ws).await {
            match event {
                SessionEvent::TurnStart { turn } => order.push(format!("turn_start {turn}")),
                SessionEvent::TurnEnd { turn, .. } => order.push(format!("turn_end {turn}")),
                _ => {}
            }
        }
    }
    assert_eq!(order, ["turn_end 1", "turn_start 2", "turn_end 2"]);
}

async fn attach_and_send(ws: &mut Ws, text: &str) {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(10),
            session: "lead".into(),
        },
    )
    .await;
    while !matches!(
        recv(ws).await,
        ServerMsg::SessionAttached { req: Some(10), .. }
    ) {}
    send(
        ws,
        ClientMsg::SessionSend {
            session: "lead".into(),
            req: Some(11),
            text: text.into(),
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_carries_its_content_and_an_update_replaces_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach_and_send(&mut ws, "show a diff").await;
    let mut tools = Vec::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session {
                event: event @ (SessionEvent::ToolCall { .. } | SessionEvent::ToolUpdate { .. }),
                ..
            } => tools.push(event),
            ServerMsg::Session {
                event: SessionEvent::TurnEnd { .. },
                ..
            } => break,
            _ => {}
        }
    }
    let diff = |new: &str| ToolContent::Diff {
        path: "src/a.rs".into(),
        old_text: Some("one\n".into()),
        new_text: new.into(),
    };
    let expected = [
        SessionEvent::ToolCall {
            id: "call-2".into(),
            title: "Edit src/a.rs".into(),
            tool_kind: "edit".into(),
            status: "pending".into(),
            content: vec![
                ToolContent::Text {
                    text: "editing a.rs".into(),
                },
                diff("two\n"),
            ],
        },
        SessionEvent::ToolUpdate {
            id: "call-2".into(),
            title: None,
            status: Some("completed".into()),
            content: Some(vec![
                diff("three\n"),
                ToolContent::Terminal {
                    id: "term-1".into(),
                },
            ]),
        },
    ];
    assert_eq!(tools, expected);

    // The stored transcript keeps both, so a client attaching later folds the same content.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::SessionAttach {
            req: Some(3),
            session: "lead".into(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::SessionAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let stored: Vec<_> = events
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                SessionEvent::ToolCall { .. } | SessionEvent::ToolUpdate { .. }
            )
        })
        .collect();
    assert_eq!(stored, expected);
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
    send(
        &mut ws,
        ClientMsg::SessionCancel {
            req: Some(12),
            session: "lead".into(),
        },
    )
    .await;
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
        if let ServerMsg::Session {
            event: SessionEvent::Error { turn: None, .. },
            ..
        } = recv(&mut ws).await
        {
            break;
        }
    }
    wait_ready(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: "lead".into(),
            req: Some(20),
            text: "back again".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: back again"), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_creates_a_space_through_the_mcp_tools() {
    let (url, _) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach_and_send(&mut ws, "make a space for /tmp").await;
    // Every client hears the new layout while the turn is still running.
    let (spaces, active) = loop {
        if let ServerMsg::Spaces { spaces, active } = recv(&mut ws).await {
            break (spaces, active);
        }
    };
    assert_eq!(spaces.len(), 1);
    assert_eq!(
        (spaces[0].cwd.as_str(), spaces[0].name.as_str()),
        ("/tmp", "tmp")
    );
    assert_eq!(active.as_deref(), Some(spaces[0].id.as_str()));
    let text = turn_text(&mut ws).await;
    assert!(
        text.contains(&format!("\"id\": \"{}\"", spaces[0].id)),
        "the tool returned the new space: {text:?}"
    );
}

/// POST one JSON-RPC message to `/mcp` with the given extra header lines; returns the status
/// line and the body.
async fn mcp_post(addr: std::net::SocketAddr, headers: &str, body: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), body.to_string())
}

/// Call one tool with the daemon token; returns its text and whether it was an error.
async fn call_tool(
    addr: std::net::SocketAddr,
    tool: &str,
    args: serde_json::Value,
) -> (String, bool) {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    let (status, body) = mcp_post(
        addr,
        &format!("Authorization: Bearer {TOKEN}\r\n"),
        &body.to_string(),
    )
    .await;
    assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
    let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
    let result = &reply["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_string(),
        result["isError"] == true,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pane_tools_split_start_a_terminal_and_close() {
    let (_, addr) = start_daemon_at().await;
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let (new, error) = call_tool(
        addr,
        "split_pane",
        serde_json::json!({ "pane": space.focus, "dir": "down" }),
    )
    .await;
    assert!(!error, "{new}");
    let (text, error) = call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": new, "kind": "terminal" }),
    )
    .await;
    assert_eq!(
        (text.as_str(), error),
        (format!("{new} is now terminal").as_str(), false)
    );
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let b = &layout["spaces"][0]["root"]["b"];
    assert_eq!(
        (&b["id"], &b["kind"]),
        (&serde_json::json!(new), &serde_json::json!("terminal"))
    );
    let (text, error) = call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": new, "kind": "hologram" }),
    )
    .await;
    assert!(error, "no such kind: {text}");
    // Closing both panes closes the space.
    for pane in [&new, &space.focus] {
        let (text, error) =
            call_tool(addr, "close_pane", serde_json::json!({ "pane": pane })).await;
        assert!(!error, "{text}");
    }
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    assert!(layout.contains("\"spaces\": []"), "{layout}");
}

/// Attach `ws` to `session` and wait for its transcript.
async fn attach(ws: &mut Ws, session: &str, req: u64) -> Vec<SessionEvent> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: session.into(),
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::SessionAttached {
                req: Some(r),
                events,
                ..
            } if r == req => return events,
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("attach {session}: {message}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_starts_a_worker_in_an_agent_pane_that_ends_with_it() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 1).await;

    let (session, error) = call_tool(
        addr,
        "start_worker",
        serde_json::json!({ "space": space.id, "prompt": "first task" }),
    )
    .await;
    assert!(!error, "{session}");
    assert!(session.starts_with('w'), "{session}");
    // With no pane given, the worker runs in a new pane right of the focused one.
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let pane = &layout["spaces"][0]["root"]["b"];
    assert_eq!(
        (&pane["kind"], &pane["session"]),
        (&serde_json::json!("agent"), &serde_json::json!(session))
    );
    let pane = pane["id"].as_str().unwrap().to_string();

    // The prompt was the worker's first message.
    let events = attach(&mut ws, &session, 2).await;
    assert_eq!(
        events.first(),
        Some(&SessionEvent::User {
            turn: 1,
            text: "first task".into()
        })
    );
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(3),
            session: session.clone(),
            text: "hello worker".into(),
        },
    )
    .await;
    let mut text = String::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { session: s, event } if s == session => match event {
                SessionEvent::Text { text: chunk } => text.push_str(&chunk),
                SessionEvent::TurnEnd { turn: 2, .. } => break,
                _ => {}
            },
            ServerMsg::Session { session: s, event } => panic!("{s} got {event:?}"),
            _ => {}
        }
    }
    assert!(text.contains("echo: hello worker"), "{text:?}");
    assert!(
        !text.contains("context: yes"),
        "a worker gets no orchestrator context: {text:?}"
    );

    // Closing the pane ends the session: attached clients hear so, and it is gone.
    let (closed, error) = call_tool(addr, "close_pane", serde_json::json!({ "pane": pane })).await;
    assert!(!error, "{closed}");
    loop {
        if let ServerMsg::Session {
            session: s,
            event:
                SessionEvent::Error {
                    turn: None,
                    message,
                },
        } = recv(&mut ws).await
        {
            assert_eq!(
                (s.as_str(), message.as_str()),
                (session.as_str(), "session ended")
            );
            break;
        }
    }
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(4),
            session: session.clone(),
            text: "anyone?".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error {
                req: Some(4),
                message,
                ..
            } => {
                assert_eq!(message, format!("no session {session}"));
                break;
            }
            ServerMsg::SessionQueued { req: 4, .. } => panic!("an ended session took a message"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn start_worker_refuses_a_pane_outside_its_space() {
    let (_, addr) = start_daemon_at().await;
    let (a, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let (b, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let a: wheel_proto::SpaceInfo = serde_json::from_str(&a).unwrap();
    let b: wheel_proto::SpaceInfo = serde_json::from_str(&b).unwrap();
    let (text, error) = call_tool(
        addr,
        "start_worker",
        serde_json::json!({ "space": a.id, "pane": b.focus, "prompt": "x" }),
    )
    .await;
    assert!(error);
    assert_eq!(text, format!("pane {} is not in space {}", b.focus, a.id));
}

/// Wait for the next permission request any client is shown.
async fn next_perm(ws: &mut Ws) -> wheel_proto::PermInfo {
    loop {
        if let ServerMsg::PermRequested { perm } = recv(ws).await {
            return perm;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_asks_under_its_own_name_and_its_pane_closing_withdraws_the_request() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 1).await;
    let (worker, error) = call_tool(
        addr,
        "start_worker",
        serde_json::json!({ "space": space.id, "prompt": "needs permission" }),
    )
    .await;
    assert!(!error, "{worker}");
    attach(&mut ws, &worker, 2).await;

    let perm = next_perm(&mut ws).await;
    assert_eq!(
        (perm.session.as_str(), perm.tool_call.as_deref()),
        (worker.as_str(), Some("call-1"))
    );
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    // The answer reaches the worker; the lead, attached on the same connection, hears nothing.
    let mut text = String::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { session: s, event } if s == worker => match event {
                SessionEvent::Text { text: chunk } => text.push_str(&chunk),
                SessionEvent::TurnEnd { turn: 1, .. } => break,
                _ => {}
            },
            ServerMsg::Session { session: s, event } => panic!("{s} got {event:?}"),
            _ => {}
        }
    }
    assert!(text.contains("permission: allow-once"), "{text:?}");

    // A worker request and a lead request wait side by side.
    for (session, req) in [(worker.as_str(), 3), ("lead", 4)] {
        send(
            &mut ws,
            ClientMsg::SessionSend {
                req: Some(req),
                session: session.into(),
                text: "needs permission".into(),
            },
        )
        .await;
    }
    let mut waiting = vec![next_perm(&mut ws).await, next_perm(&mut ws).await];
    waiting.sort_by(|a, b| a.session.cmp(&b.session));
    let [lead_perm, worker_perm] = <[_; 2]>::try_from(waiting).unwrap();
    assert_eq!(
        (lead_perm.session.as_str(), worker_perm.session.as_str()),
        ("lead", worker.as_str())
    );

    // Closing the worker's pane withdraws its request and leaves the lead's.
    let layout: serde_json::Value = serde_json::from_str(
        &call_tool(addr, "list_spaces", serde_json::json!({}))
            .await
            .0,
    )
    .unwrap();
    let pane = layout["spaces"][0]["root"]["b"]["id"].as_str().unwrap();
    let (closed, error) = call_tool(addr, "close_pane", serde_json::json!({ "pane": pane })).await;
    assert!(!error, "{closed}");
    loop {
        if let ServerMsg::PermResolved { id, choice } = recv(&mut ws).await {
            assert_eq!((id, choice.as_str()), (worker_perm.id.clone(), "withdrawn"));
            break;
        }
    }
    assert_eq!(perm_list(&mut ws).await, vec![lead_perm]);
}

/// Everything a terminal prints over `ws` for `ms` milliseconds.
async fn output_for(ws: &mut Ws, ms: u64) -> String {
    let mut text = String::new();
    let until = tokio::time::Instant::now() + Duration::from_millis(ms);
    while let Ok(msg) = tokio::time::timeout_at(until, recv(ws)).await {
        if let ServerMsg::TermOutput { data, .. } = msg {
            text.push_str(&String::from_utf8_lossy(
                &wheel_proto::decode_bytes(&data).unwrap(),
            ));
        }
    }
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_into_a_pane_waits_for_the_person() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let pane = serde_json::from_str::<wheel_proto::SpaceInfo>(&space)
        .unwrap()
        .focus;
    let args = serde_json::json!({ "pane": pane, "text": "echo typed-$((6*7))", "enter": true });
    let (text, error) = call_tool(addr, "type_into_pane", args.clone()).await;
    assert!(error, "an empty pane takes no input: {text}");
    assert_eq!(text, format!("{pane} is not a terminal pane"));
    let (text, error) = call_tool(
        addr,
        "type_into_pane",
        serde_json::json!({ "pane": pane, "text": "x".repeat(4097) }),
    )
    .await;
    assert!(error, "{text}");
    assert_eq!(text, "text is 4097 bytes; type at most 4096 per call");

    call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": pane, "kind": "terminal" }),
    )
    .await;
    let mut ws = connect(&url).await;
    send(&mut ws, ClientMsg::Status { req: Some(1) }).await;
    let term = loop {
        if let ServerMsg::Status { terms, .. } = recv(&mut ws).await {
            break terms[0].id.clone();
        }
    };
    send(
        &mut ws,
        ClientMsg::TermAttach {
            req: None,
            id: term.clone(),
        },
    )
    .await;

    for (choice, typed) in [("deny", false), ("allow", true)] {
        let call = tokio::spawn(call_tool(addr, "type_into_pane", args.clone()));
        let perm = next_perm(&mut ws).await;
        assert_eq!(
            perm.tool_call, None,
            "a daemon tool's request gates no ACP call"
        );
        assert_eq!(
            (perm.session.as_str(), perm.title.as_str()),
            (
                "lead",
                format!("Type into {pane}: \"echo typed-$((6*7))\" ⏎").as_str()
            )
        );
        // Unanswered, nothing reaches the shell and the request waits in the list.
        assert!(!output_for(&mut ws, 300).await.contains("typed-"));
        send(&mut ws, ClientMsg::Status { req: Some(2) }).await;
        let perms = loop {
            if let ServerMsg::Status { perms, .. } = recv(&mut ws).await {
                break perms;
            }
        };
        assert_eq!(perms, std::slice::from_ref(&perm));
        send(
            &mut ws,
            ClientMsg::PermAnswer {
                req: None,
                id: perm.id,
                choice: choice.into(),
            },
        )
        .await;
        let (text, error) = call.await.unwrap();
        assert_eq!(error, !typed, "{choice}: {text}");
        assert_eq!(
            output_for(&mut ws, 1000).await.contains("typed-42"),
            typed,
            "{choice}"
        );
    }
}

/// A space with one terminal pane, and a client watching for permission requests.
async fn terminal_pane(url: &str, addr: std::net::SocketAddr) -> (String, Ws) {
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let pane = serde_json::from_str::<wheel_proto::SpaceInfo>(&space)
        .unwrap()
        .focus;
    call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": pane, "kind": "terminal" }),
    )
    .await;
    (pane, connect(url).await)
}

/// The waiting permission requests, as `perm list` shows them.
async fn perm_list(ws: &mut Ws) -> Vec<wheel_proto::PermInfo> {
    send(ws, ClientMsg::Status { req: Some(2) }).await;
    loop {
        if let ServerMsg::Status { perms, .. } = recv(ws).await {
            return perms;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_type_into_pane_request_is_withdrawn_when_its_caller_hangs_up() {
    let (url, addr) = start_daemon_at().await;
    let (pane, mut ws) = terminal_pane(&url, addr).await;
    let call = tokio::spawn(call_tool(
        addr,
        "type_into_pane",
        serde_json::json!({ "pane": pane, "text": "echo late" }),
    ));
    let perm = next_perm(&mut ws).await;
    // The caller gives up and closes its connection.
    call.abort();
    let withdrawn = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let ServerMsg::PermResolved { id, choice } = recv(&mut ws).await {
                return (id, choice);
            }
        }
    })
    .await
    .expect("withdrawn within 1 s");
    assert_eq!(withdrawn, (perm.id.clone(), "withdrawn".to_string()));
    assert_eq!(perm_list(&mut ws).await, []);
    // A late approval finds nothing to approve, so nothing is typed.
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(3),
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error { req: Some(3), .. } => break,
            ServerMsg::Ok { req: 3 } => panic!("a withdrawn request accepted an answer"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_type_into_pane_request_is_withdrawn_with_the_lead_turn() {
    let (url, addr) = start_daemon_at().await;
    let (pane, mut ws) = terminal_pane(&url, addr).await;
    wait_ready(&mut ws).await;
    let call = tokio::spawn(call_tool(
        addr,
        "type_into_pane",
        serde_json::json!({ "pane": pane, "text": "echo late" }),
    ));
    next_perm(&mut ws).await;
    // Cancelling a lead turn withdraws every request the lead session has waiting.
    attach_and_send(&mut ws, "needs permission").await;
    next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::SessionCancel {
            req: Some(12),
            session: "lead".into(),
        },
    )
    .await;
    let (text, error) = call.await.unwrap();
    assert_eq!(
        (text.as_str(), error),
        ("the request was withdrawn before anyone answered", true)
    );
    assert_eq!(perm_list(&mut ws).await, []);
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_refuses_a_request_without_the_daemon_token() {
    let (_, addr) = start_daemon_at().await;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let mut status = Vec::new();
    for headers in ["", "Authorization: Bearer wrong\r\n"] {
        status.push(mcp_post(addr, headers, body).await.0);
    }
    assert_eq!(
        status,
        ["HTTP/1.1 401 Unauthorized", "HTTP/1.1 401 Unauthorized"]
    );
}

/// Attach to `session` until its transcript satisfies `done`; returns that transcript.
async fn transcript_when(
    ws: &mut Ws,
    session: &str,
    done: impl Fn(&[SessionEvent]) -> bool,
) -> Vec<SessionEvent> {
    for req in 100..200 {
        let events = attach(ws, session, req).await;
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{session}'s transcript never got there");
}

/// A daemon with a worker whose first task has finished, then a second daemon restarted on the
/// same state file, after `between` runs; returns the second daemon's url and the worker's name.
async fn restart_with_a_worker(
    command: &str,
    dir: &std::path::Path,
    between: impl FnOnce(),
) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let (url, addr, first_daemon) = start_daemon_with(command, Some(state_file.clone())).await;
    let (space, _) = call_tool(addr, "create_space", serde_json::json!({ "cwd": "/tmp" })).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let (session, error) = call_tool(
        addr,
        "start_worker",
        serde_json::json!({ "space": space.id, "prompt": "first task" }),
    )
    .await;
    assert!(!error, "{session}");
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &session, |events| {
        events.contains(&SessionEvent::TurnEnd {
            turn: 1,
            stop_reason: "end_turn".into(),
        })
    })
    .await;
    // The pane saved the agent's id for the conversation.
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let pane = &saved["spaces"][0]["root"]["b"];
    assert_eq!(pane["session"], serde_json::json!(session));
    assert!(
        pane["acp_session"]
            .as_str()
            .is_some_and(|id| id.starts_with("fake-")),
        "{pane}"
    );
    drop(ws);
    first_daemon.abort();
    between();
    let (url, _, _) = start_daemon_with(command, Some(state_file)).await;
    (url, session)
}

/// Send `text` to `session` and collect its reply until the turn ends.
async fn reply(ws: &mut Ws, session: &str, text: &str) -> String {
    send(
        ws,
        ClientMsg::SessionSend {
            req: Some(300),
            session: session.into(),
            text: text.into(),
        },
    )
    .await;
    let mut turn = None;
    let mut reply = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::SessionQueued {
                req: 300, turn: t, ..
            } => turn = Some(t),
            ServerMsg::Session { session: s, event } if s == session => match event {
                SessionEvent::Text { text } => reply.push_str(&text),
                SessionEvent::TurnEnd { turn: t, .. } if Some(t) == turn => return reply,
                _ => {}
            },
            _ => {}
        }
    }
}

/// A scratch directory removed when the test ends, failed or not.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("wheel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_agent_pane_resumes_its_conversation() {
    let dir = Scratch::new("resume");
    let store = dir.0.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    let (url, session) = restart_with_a_worker(&command, &dir.0, || {}).await;

    // The agent loaded the conversation, and its replay is the transcript, as whole turns.
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &session, |events| events.len() >= 4).await;
    assert_eq!(
        events,
        [
            SessionEvent::User {
                turn: 1,
                text: "first task".into()
            },
            SessionEvent::TurnStart { turn: 1 },
            SessionEvent::Text {
                text: "echo: first task".into()
            },
            SessionEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into()
            },
        ],
        "{events:?}"
    );
    // The conversation carries on, numbered after the replayed turn.
    let text = reply(&mut ws, &session, "after restart").await;
    assert_eq!(text, "echo: after restart");
    let events = attach(&mut ws, &session, 400).await;
    assert!(
        events.contains(&SessionEvent::TurnStart { turn: 2 }),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_agent_pane_that_cannot_resume_says_so_and_starts_afresh() {
    let dir = Scratch::new("no-resume");
    let (url, session) =
        restart_with_a_worker(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {}).await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &session, |events| !events.is_empty()).await;
    assert_eq!(
        events,
        [SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_NOT_RESUMED.into()
        }]
    );
    let text = reply(&mut ws, &session, "hello again").await;
    assert_eq!(text, "echo: hello again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_load_that_fails_leaves_only_the_notice() {
    let dir = Scratch::new("failed-load");
    let store = dir.0.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    // The agent can load sessions, but has lost this one's turns.
    let (url, session) = restart_with_a_worker(&command, &dir.0, || {
        std::fs::remove_dir_all(&store).unwrap();
        std::fs::create_dir_all(&store).unwrap();
    })
    .await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &session, |events| !events.is_empty()).await;
    assert_eq!(
        events,
        [SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_NOT_RESUMED.into()
        }]
    );
    let text = reply(&mut ws, &session, "hello again").await;
    assert_eq!(text, "echo: hello again");
}
