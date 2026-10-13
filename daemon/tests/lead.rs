//! The lead over the socket, driven by the scripted fake agent: a turn streams back with the
//! orchestrator context in place, and a permission request reaches every client, where the first
//! answer resumes the agent and a second answer is refused; the lead changes spaces through the
//! daemon's MCP tools and starts agent panes, whose sessions they own.

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

/// A daemon whose lead and agent panes run `command`, keeping its layout in `state_file`; the handle
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
    // Its instance directory is the one holding the state file, as it is in production.
    let instance = state_file
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    let state = wheeld::AppState::new("/dev/null".into(), TOKEN.into(), instance);
    // The same order as main.rs: the agent command first, because restoring a saved agent pane
    // starts its agent with it; the lead last, because the conversation it resumes is read from
    // the restored state.
    state.set_agent_command(command.into());
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    state.start_lead(command.into(), "/tmp".into(), &wheeld::mcp::url(addr));
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

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_one_turn_and_the_queued_message_still_runs() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    let cancel = |req| ClientMsg::SessionCancel {
        req: Some(req),
        session: "lead".into(),
    };
    // With nothing running or queued, a cancel says so instead of replying ok.
    send(&mut ws, cancel(20)).await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error {
                req: Some(20),
                message,
                ..
            } => {
                assert_eq!(message, "session lead has no turn to cancel");
                break;
            }
            ServerMsg::Ok { req: 20 } => panic!("an idle cancel replied ok"),
            _ => {}
        }
    }

    attach_and_send(&mut ws, "needs permission").await;
    next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: "lead".into(),
            req: None,
            text: "second".into(),
        },
    )
    .await;
    send(&mut ws, cancel(21)).await;
    // The fake agent reports a cancelled permission request, then ends the turn. The reply and the
    // session events come from different tasks, so the reply has no place in the order.
    let mut order = Vec::new();
    let mut replied = false;
    while order.last() != Some(&"turn_end 2".to_string()) || !replied {
        match recv(&mut ws).await {
            ServerMsg::Session { event, .. } => match event {
                SessionEvent::Text { text } if text.contains("permission: cancelled") => {
                    order.push("permission cancelled".into())
                }
                SessionEvent::TurnStart { turn } => order.push(format!("turn_start {turn}")),
                SessionEvent::TurnEnd { turn, .. } => order.push(format!("turn_end {turn}")),
                _ => {}
            },
            ServerMsg::Ok { req: 21 } => replied = true,
            ServerMsg::Error {
                req: Some(21),
                message,
                ..
            } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(
        order,
        [
            "permission cancelled",
            "turn_end 1",
            "turn_start 2",
            "turn_end 2"
        ]
    );
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

/// The session's config options as the daemon last had them, read through an attach.
async fn config_of(ws: &mut Ws, req: u64) -> Vec<wheel_proto::ConfigOption> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: "lead".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached {
            req: Some(got),
            config,
            ..
        } = recv(ws).await
        {
            if got == req {
                return config;
            }
        }
    }
}

async fn send_config_set(ws: &mut Ws, req: u64, config: &str, value: &str) {
    send(
        ws,
        ClientMsg::SessionConfigSet {
            req: Some(req),
            session: "lead".into(),
            config: config.into(),
            value: value.into(),
        },
    )
    .await;
}

/// Set one option and wait for the refusal answering it, which must name `needle`.
async fn config_set_refused(ws: &mut Ws, req: u64, config: &str, value: &str, needle: &str) {
    send_config_set(ws, req, config, value).await;
    loop {
        if let ServerMsg::Error {
            req: Some(got),
            message,
            ..
        } = recv(ws).await
        {
            if got == req {
                assert!(message.contains(needle), "{message}");
                return;
            }
        }
    }
}

fn current(options: &[wheel_proto::ConfigOption], id: &str) -> String {
    let option = options
        .iter()
        .find(|option| option.id == id)
        .unwrap_or_else(|| panic!("no config option {id} in {options:?}"));
    match &option.kind {
        wheel_proto::ConfigKind::Select { current, .. } => current.clone(),
        wheel_proto::ConfigKind::Boolean { current } => current.to_string(),
        wheel_proto::ConfigKind::Unknown => "unknown".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agents_config_options_arrive_and_a_set_moves_one() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;

    let options = config_of(&mut ws, 1).await;
    let mut ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["fast", "mode", "model"]);
    assert_eq!(current(&options, "model"), "default");
    assert_eq!(current(&options, "fast"), "false");

    send_config_set(&mut ws, 2, "model", "sonnet").await;
    // The change reaches every connection as a broadcast, not only the setter's reply.
    let mut broadcast = None;
    let mut acked = false;
    while !acked || broadcast.is_none() {
        match recv(&mut ws).await {
            ServerMsg::Ok { req: 2 } => acked = true,
            ServerMsg::SessionConfig { session, options } if session == "lead" => {
                broadcast = Some(options)
            }
            _ => {}
        }
    }
    assert_eq!(current(&broadcast.unwrap(), "model"), "sonnet");
    assert_eq!(current(&config_of(&mut ws, 3).await, "model"), "sonnet");

    // A value the agent does not offer is refused, and nothing moves.
    config_set_refused(&mut ws, 4, "model", "no-such-model", "no-such-model").await;
    assert_eq!(current(&config_of(&mut ws, 5).await, "model"), "sonnet");

    // An option the agent never named never reaches it.
    config_set_refused(&mut ws, 6, "not-an-option", "x", "not-an-option").await;

    // A boolean takes true or false and nothing else: anything else would read as false and turn
    // the option off while reporting success.
    config_set_refused(&mut ws, 7, "fast", "yes", "not true or false").await;
    assert_eq!(current(&config_of(&mut ws, 8).await, "fast"), "false");
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
    // The space opens only once the person allows it.
    let perm = next_perm(&mut ws).await;
    assert!(perm.title.contains("/tmp"), "{}", perm.title);
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
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

/// Call `create_space`, answering the permission request it raises with allow from a second
/// connection; returns its text and whether it was an error.
async fn open_space(addr: std::net::SocketAddr, cwd: &str) -> (String, bool) {
    let mut ws = connect(&format!("ws://{addr}/ws?token={TOKEN}")).await;
    let allow = async {
        let perm = next_perm(&mut ws).await;
        assert_eq!(perm.session, "lead");
        assert!(perm.title.contains(cwd), "{}", perm.title);
        send(
            &mut ws,
            ClientMsg::PermAnswer {
                req: None,
                id: perm.id,
                choice: "allow".into(),
            },
        )
        .await;
    };
    let (result, ()) = tokio::join!(
        call_tool(addr, "create_space", serde_json::json!({ "cwd": cwd })),
        allow
    );
    result
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
    let (space, _) = open_space(addr, "/tmp").await;
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

/// The lead's artifact tools are the CLI's verbs: it can see what artifacts exist, make one, and
/// show one in a pane — with a pane of its own when it names none, as starting an agent does. A
/// name that is not one plain component, or that no scope has, comes back as a tool error and
/// leaves the layout as it was.
#[tokio::test(flavor = "multi_thread")]
async fn the_artifact_tools_list_make_and_open_one() {
    let base = std::env::temp_dir().join(format!("wheel-lead-artifacts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (_, addr) = start_daemon_at().await;
    let (space, _) = open_space(addr, &base.to_string_lossy()).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();

    // This space has no artifact yet, which is an empty listing rather than an error. Only this
    // space's own two roots are asserted on: the `shared` root is the instance directory, which
    // every test in this binary shares.
    let (text, error) = call_tool(addr, "list_artifacts", serde_json::json!({})).await;
    assert!(!error, "{text}");
    let listing: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mine = |listing: &serde_json::Value| -> Vec<String> {
        listing["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| {
                a["dir"]
                    .as_str()
                    .unwrap()
                    .starts_with(base.to_str().unwrap())
            })
            .map(|a| {
                format!(
                    "{} {}",
                    a["scope"].as_str().unwrap(),
                    a["name"].as_str().unwrap()
                )
            })
            .collect()
    };
    assert!(mine(&listing).is_empty(), "{text}");

    // Making one writes the folder the CLI's verb writes, under the space's own directory.
    let (text, error) =
        call_tool(addr, "new_artifact", serde_json::json!({ "name": "dash" })).await;
    assert!(!error, "{text}");
    let made: wheel_proto::ArtifactInfo = serde_json::from_str(&text).unwrap();
    let dir = base.join(".wheel/artifacts/dash");
    assert_eq!(
        (made.scope, made.name.as_str(), made.dir.as_str()),
        (
            wheel_proto::ArtifactScope::Repo,
            "dash",
            dir.to_string_lossy().as_ref()
        )
    );
    assert!(made.problems.is_empty(), "{made:?}");
    assert!(dir.join("artifact.toml").is_file() && dir.join("index.html").is_file());

    // The same name again is refused rather than overwriting what the artifact holds, and a name
    // that is not one plain component never reaches the filesystem at all.
    let (text, error) =
        call_tool(addr, "new_artifact", serde_json::json!({ "name": "dash" })).await;
    assert!(error && text.contains("already"), "{text}");
    let (text, error) = call_tool(addr, "new_artifact", serde_json::json!({ "name": "a/b" })).await;
    assert!(error && text.contains("is not an artifact name"), "{text}");
    assert!(!base.join(".wheel/artifacts/a").exists());

    // The listing now carries it, and what a bare name means.
    let (text, _) = call_tool(addr, "list_artifacts", serde_json::json!({})).await;
    let listing: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(mine(&listing), ["repo dash"], "{text}");
    assert_eq!(listing["resolves"]["dash"], serde_json::json!("repo"));

    // Opening it with no pane named splits the space's focused pane and shows it on the right.
    let (text, error) =
        call_tool(addr, "open_artifact", serde_json::json!({ "name": "dash" })).await;
    assert!(!error, "{text}");
    let opened: serde_json::Value = serde_json::from_str(&text).unwrap();
    let pane = opened["pane"].as_str().unwrap().to_string();
    assert_ne!(pane, space.focus, "a pane of its own: {text}");
    assert_eq!(
        (
            &opened["shows"]["kind"],
            &opened["shows"]["scope"],
            &opened["shows"]["name"]
        ),
        (
            &serde_json::json!("web"),
            &serde_json::json!("repo"),
            &serde_json::json!("dash")
        )
    );
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let panes = layout.matches("\"kind\"").count();
    assert_eq!(panes, 2, "{layout}");
    assert!(layout.contains(&format!("\"id\": \"{pane}\"")), "{layout}");

    // Named a pane, it replaces what that pane showed and splits nothing.
    let (text, error) = call_tool(
        addr,
        "open_artifact",
        serde_json::json!({ "name": "dash", "scope": "repo", "pane": space.focus }),
    )
    .await;
    assert!(!error, "{text}");
    let opened: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(opened["pane"], serde_json::json!(space.focus));
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let root = &layout["spaces"][0]["root"];
    for side in ["a", "b"] {
        assert_eq!(root[side]["kind"], serde_json::json!("web"), "{root}");
        assert_eq!(root[side]["name"], serde_json::json!("dash"), "{root}");
    }

    // A second space's panes are its own: an artifact opens in the space it is given, in that
    // space's focused pane, and a pane from another space is refused rather than reached into.
    let other = std::env::temp_dir().join(format!("wheel-lead-other-{}", std::process::id()));
    std::fs::create_dir_all(&other).unwrap();
    let (second, _) = open_space(addr, &other.to_string_lossy()).await;
    let second: wheel_proto::SpaceInfo = serde_json::from_str(&second).unwrap();
    let (text, error) = call_tool(
        addr,
        "open_artifact",
        serde_json::json!({ "name": "dash", "space": second.id, "pane": space.focus }),
    )
    .await;
    assert!(
        error && text.contains(&format!("is not in space {}", second.id)),
        "{text}"
    );
    // `dash` is the first space's `repo` artifact, so the second space has no such name: the roots
    // the opener looks in are the space's own.
    let (text, error) = call_tool(
        addr,
        "open_artifact",
        serde_json::json!({ "name": "dash", "space": second.id }),
    )
    .await;
    assert!(error && text.contains("no artifact dash"), "{text}");
    // Its own `repo` artifact, under its own directory — nothing here writes the `shared` root,
    // which every test in this binary would inherit.
    let (text, error) = call_tool(
        addr,
        "new_artifact",
        serde_json::json!({ "name": "other", "space": second.id }),
    )
    .await;
    assert!(!error, "{text}");
    assert!(other.join(".wheel/artifacts/other/index.html").is_file());
    let (text, error) = call_tool(
        addr,
        "open_artifact",
        serde_json::json!({ "name": "other", "space": second.id }),
    )
    .await;
    assert!(!error, "{text}");
    let opened: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_ne!(opened["pane"], serde_json::json!(second.focus));
    let _ = std::fs::remove_dir_all(&other);

    // A name no scope has, and a name that is no name, are tool errors — and neither leaves a
    // pane behind, moves the focus or changes which space is showing, so a model correcting
    // itself finds the layout the person was looking at.
    let (before, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let was: serde_json::Value = serde_json::from_str(&before).unwrap();
    // The second space is the one showing, so the last case — a refused open aimed at the *other*
    // space — is the one that would otherwise leave the person looking at a space they did not
    // ask for: splitting shows the space it splits in.
    assert_eq!(was["active"], serde_json::json!(second.id), "{before}");
    for aimed in [
        serde_json::json!({ "name": "nosuch" }),
        serde_json::json!({ "name": ".." }),
        serde_json::json!({ "name": "a/b" }),
        serde_json::json!({ "name": "" }),
        serde_json::json!({ "name": "nosuch", "space": space.id }),
    ] {
        let (text, error) = call_tool(addr, "open_artifact", aimed.clone()).await;
        assert!(error, "{aimed}: {text}");
        let (now, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
        let now: serde_json::Value = serde_json::from_str(&now).unwrap();
        assert_eq!(now["spaces"], was["spaces"], "{aimed}");
        assert_eq!(now["active"], was["active"], "{aimed}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// Start an agent that the fake agent brings up; returns its session name once it is ready.
async fn start_agent(addr: std::net::SocketAddr, args: serde_json::Value) -> String {
    let (text, error) = call_tool(addr, "start_pane", args).await;
    assert!(!error, "{text}");
    let started: serde_json::Value = serde_json::from_str(&text).unwrap();
    // The first message is queued before the agent is ready, so it is ready or already busy.
    let phase = started["state"]["phase"].as_str().unwrap();
    assert!(matches!(phase, "ready" | "busy"), "{text}");
    started["session"].as_str().unwrap().to_string()
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
async fn the_lead_starts_an_agent_in_an_agent_pane_that_ends_with_it() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 1).await;

    let session = start_agent(
        addr,
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "first task" }),
    )
    .await;
    assert!(session.starts_with('w'), "{session}");
    // With no pane given, the agent runs in a new pane right of the focused one.
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let pane = &layout["spaces"][0]["root"]["b"];
    assert_eq!(
        (&pane["kind"], &pane["session"]),
        (&serde_json::json!("agent"), &serde_json::json!(session))
    );
    // The pane entry carries the session's state, and the listing has no separate map of them.
    assert!(
        matches!(pane["state"]["phase"].as_str(), Some("ready" | "busy")),
        "{layout}"
    );
    assert!(layout.get("workers").is_none(), "{layout}");
    let pane = pane["id"].as_str().unwrap().to_string();

    // The message was the agent's first message.
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
            text: "hello agent".into(),
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
    assert!(text.contains("echo: hello agent"), "{text:?}");
    assert!(
        !text.contains("context: yes"),
        "an agent pane gets no orchestrator context: {text:?}"
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
async fn one_verb_starts_a_pane_and_set_pane_kind_no_longer_starts_agents() {
    let (_, addr) = start_daemon_at().await;
    let listing = mcp_tools(addr).await;
    assert!(listing.contains("\"start_pane\""), "{listing}");
    assert!(!listing.contains("start_worker"), "{listing}");
    assert!(!listing.to_lowercase().contains("worker"), "{listing}");

    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let (text, error) = call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": space.focus, "kind": "agent" }),
    )
    .await;
    assert!(error, "{text}");

    // A terminal starts in the given pane; a message is for an agent only.
    let (text, error) = call_tool(
        addr,
        "start_pane",
        serde_json::json!({ "space": space.id, "pane": space.focus, "kind": "terminal", "message": "x" }),
    )
    .await;
    assert!(error && text.contains("only for an agent"), "{text}");
    let (text, error) = call_tool(
        addr,
        "start_pane",
        serde_json::json!({ "space": space.id, "pane": space.focus, "kind": "terminal" }),
    )
    .await;
    assert!(!error, "{text}");
    let started: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(started, serde_json::json!({ "pane": space.focus }));
}

#[tokio::test(flavor = "multi_thread")]
async fn start_pane_refuses_a_pane_outside_its_space() {
    let (_, addr) = start_daemon_at().await;
    let (a, _) = open_space(addr, "/tmp").await;
    let (b, _) = open_space(addr, "/tmp").await;
    let a: wheel_proto::SpaceInfo = serde_json::from_str(&a).unwrap();
    let b: wheel_proto::SpaceInfo = serde_json::from_str(&b).unwrap();
    let (text, error) = call_tool(
        addr,
        "start_pane",
        serde_json::json!({ "space": a.id, "pane": b.focus, "kind": "agent", "message": "x" }),
    )
    .await;
    assert!(error);
    assert_eq!(text, format!("pane {} is not in space {}", b.focus, a.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_directory_is_gone_gives_up_and_the_lead_hears() {
    let (url, addr) = start_daemon_at().await;
    let dir = std::env::temp_dir().join(format!("wheel-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (space, _) = open_space(addr, dir.to_str().unwrap()).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    std::fs::remove_dir(&dir).unwrap();

    // Five failed starts with backoff 1, 2, 4 and 8 seconds between them.
    let (text, error) = call_tool(
        addr,
        "start_pane",
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "first task" }),
    )
    .await;
    assert!(error, "{text}");
    assert!(
        text.contains("could not start: gave up after 5 tries")
            && text.contains("is not a directory"),
        "{text}"
    );
    let session = text.split_whitespace().nth(1).unwrap();
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let state = &layout["spaces"][0]["root"]["b"]["state"];
    assert_eq!(
        layout["spaces"][0]["root"]["b"]["session"], session,
        "{layout}"
    );
    assert_eq!(state["phase"], "gave_up", "{layout}");
    assert_eq!(state["tries"], 5, "{layout}");
    assert!(
        state["message"]
            .as_str()
            .unwrap()
            .ends_with("is not a directory"),
        "{state}"
    );
    // An agent that gave up takes no messages.
    let mut ws = connect(&url).await;
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(1),
            session: session.into(),
            text: "anyone?".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error {
                req: Some(1),
                message,
                ..
            } => {
                assert!(
                    message.contains(&format!("session {session} is gave_up after 5 tries")),
                    "{message}"
                );
                break;
            }
            ServerMsg::SessionQueued { req: 1, .. } => {
                panic!("an agent that gave up took a message")
            }
            _ => {}
        }
    }
    // The lead's verb says the same, and by pane id too.
    let pane = layout["spaces"][0]["root"]["b"]["id"].as_str().unwrap();
    let (text, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": pane, "text": "anyone?" }),
    )
    .await;
    assert!(error && text.contains("gave_up after 5 tries"), "{text}");
    let (read, error) = call_tool(addr, "read_agent", serde_json::json!({ "agent": pane })).await;
    assert!(!error, "{read}");
    assert!(
        !read.contains("anyone?"),
        "a refused message was queued: {read}"
    );
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
async fn an_agent_asks_under_its_own_name_and_its_pane_closing_withdraws_the_request() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 1).await;
    let agent = start_agent(
        addr,
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "needs permission" }),
    )
    .await;
    attach(&mut ws, &agent, 2).await;

    let perm = next_perm(&mut ws).await;
    assert_eq!(
        (perm.session.as_str(), perm.tool_call.as_deref()),
        (agent.as_str(), Some("call-1"))
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
    // The answer reaches the agent; the lead, attached on the same connection, hears nothing.
    let mut text = String::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { session: s, event } if s == agent => match event {
                SessionEvent::Text { text: chunk } => text.push_str(&chunk),
                SessionEvent::TurnEnd { turn: 1, .. } => break,
                _ => {}
            },
            ServerMsg::Session { session: s, event } => panic!("{s} got {event:?}"),
            _ => {}
        }
    }
    assert!(text.contains("permission: allow-once"), "{text:?}");

    // An agent request and a lead request wait side by side.
    for (session, req) in [(agent.as_str(), 3), ("lead", 4)] {
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
    let [lead_perm, agent_perm] = <[_; 2]>::try_from(waiting).unwrap();
    assert_eq!(
        (lead_perm.session.as_str(), agent_perm.session.as_str()),
        ("lead", agent.as_str())
    );

    // Closing the agent's pane withdraws its request and leaves the lead's.
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
            assert_eq!((id, choice.as_str()), (agent_perm.id.clone(), "withdrawn"));
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
async fn the_lead_reads_a_terminal_pane_and_waits_for_text_in_it() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = open_space(addr, "/tmp").await;
    let pane = serde_json::from_str::<wheel_proto::SpaceInfo>(&space)
        .unwrap()
        .focus;
    // A pane that is not a terminal names its kind.
    for tool in ["read_terminal", "wait_for_text"] {
        let (text, error) =
            call_tool(addr, tool, serde_json::json!({ "pane": pane, "text": "x" })).await;
        assert!(error, "{text}");
        assert_eq!(
            text,
            format!("{pane} is not a terminal pane; its kind is empty")
        );
    }
    call_tool(
        addr,
        "set_pane_kind",
        serde_json::json!({ "pane": pane, "kind": "terminal" }),
    )
    .await;

    // Type as a client would; the answer 42 is computed, so the typed line cannot satisfy the wait.
    let mut ws = connect(&url).await;
    send(&mut ws, ClientMsg::Status { req: Some(1) }).await;
    let term = loop {
        if let ServerMsg::Status { terms, .. } = recv(&mut ws).await {
            break terms[0].id.clone();
        }
    };
    send(
        &mut ws,
        ClientMsg::TermInput {
            req: None,
            id: term,
            data: wheel_proto::encode_bytes(b"echo seen-$((6*7))\r"),
        },
    )
    .await;
    let (text, error) = call_tool(
        addr,
        "wait_for_text",
        serde_json::json!({ "pane": pane, "text": "seen-42", "timeout_secs": 10 }),
    )
    .await;
    assert!(!error, "{text}");
    assert!(text.contains("seen-42"), "{text:?}");

    // Output already on the screen counts, and read shows it too.
    let (text, error) = call_tool(
        addr,
        "wait_for_text",
        serde_json::json!({ "pane": pane, "text": "seen-42", "timeout_secs": 0 }),
    )
    .await;
    assert!(!error, "on-screen output matches at once: {text}");
    let (text, error) = call_tool(
        addr,
        "read_terminal",
        serde_json::json!({ "pane": pane, "timeout_secs": 0.2 }),
    )
    .await;
    assert!(!error && text.contains("seen-42"), "{text:?}");

    // A wait that never matches names the pane.
    let (text, error) = call_tool(
        addr,
        "wait_for_text",
        serde_json::json!({ "pane": pane, "text": "never-printed", "timeout_secs": 0.3 }),
    )
    .await;
    assert!(error);
    assert_eq!(
        text,
        format!("timed out after 0.3s: \"never-printed\" did not appear in {pane}")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_into_a_pane_waits_for_the_person() {
    let (url, addr) = start_daemon_at().await;
    let (space, _) = open_space(addr, "/tmp").await;
    let pane = serde_json::from_str::<wheel_proto::SpaceInfo>(&space)
        .unwrap()
        .focus;
    let args = serde_json::json!({ "pane": pane, "text": "echo typed-$((6*7))", "enter": true });
    let (text, error) = call_tool(addr, "type_into_pane", args.clone()).await;
    assert!(error, "an empty pane takes no input: {text}");
    assert_eq!(
        text,
        format!("{pane} is not a terminal pane; its kind is empty")
    );
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
    let (space, _) = open_space(addr, "/tmp").await;
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

/// The tools `/mcp` lists, names and descriptions, as the text of the reply.
async fn mcp_tools(addr: std::net::SocketAddr) -> String {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let headers = format!("Authorization: Bearer {TOKEN}\r\n");
    mcp_post(addr, &headers, body).await.1
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

/// A daemon with an agent whose first task has finished, then a second daemon restarted on the
/// same state file, after `between` runs; returns the second daemon's url and the agent's name.
async fn restart_with_an_agent(
    command: &str,
    dir: &std::path::Path,
    between: impl FnOnce(),
) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let (url, addr, first_daemon) = start_daemon_with(command, Some(state_file.clone())).await;
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let session = start_agent(
        addr,
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "first task" }),
    )
    .await;
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
    let (url, session) = restart_with_an_agent(&command, &dir.0, || {}).await;

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
        restart_with_an_agent(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {}).await;
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
    let (url, session) = restart_with_an_agent(&command, &dir.0, || {
        std::fs::remove_dir_all(&store).unwrap();
        std::fs::create_dir_all(&store).unwrap();
    })
    .await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &session, |events| !events.is_empty()).await;
    // An agent that can load but no longer has this conversation says so; "this agent cannot
    // resume" belongs to an agent with no load at all, which the sibling test covers.
    assert_eq!(
        events,
        [SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_NOT_REOPENED.into()
        }]
    );
    let text = reply(&mut ws, &session, "hello again").await;
    assert_eq!(text, "echo: hello again");
}

/// A daemon whose lead keeps its conversations in a store, so it can list and reopen them, with
/// the state file beside them; returns its url and that file's path.
async fn lead_with_store(dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let store = dir.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    let state_file = dir.join("spaces.json");
    let (url, _, _) = start_daemon_with(&command, Some(state_file.clone())).await;
    (url, state_file)
}

/// Send the lead `text` and assert on its whole reply: the orchestrator context it starts a
/// conversation with, then the echo.
async fn lead_reply(ws: &mut Ws, text: &str) {
    assert_eq!(
        reply(ws, "lead", text).await,
        format!("context: yes\necho: {text}")
    );
}

/// The conversations the lead's agent holds for its directory.
async fn conversations(ws: &mut Ws, req: u64) -> Vec<wheel_proto::ConversationInfo> {
    send(
        ws,
        ClientMsg::SessionConversationLs {
            req: Some(req),
            session: "lead".into(),
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::Conversations {
                req: r,
                conversations,
                ..
            } if r == req => return conversations,
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("conversation ls: {message}"),
            _ => {}
        }
    }
}

/// Put the lead in a fresh conversation, or in the one `load` names; the agent's id for it, or why
/// it refused.
async fn switch(ws: &mut Ws, req: u64, load: Option<&str>) -> Result<String, String> {
    let msg = match load {
        Some(id) => ClientMsg::SessionConversationResume {
            req: Some(req),
            session: "lead".into(),
            id: id.into(),
        },
        None => ClientMsg::SessionConversationNew {
            req: Some(req),
            session: "lead".into(),
        },
    };
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            ServerMsg::Conversation { req: r, id, .. } if r == req => return Ok(id),
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => return Err(message),
            _ => {}
        }
    }
}

/// The lead's saved conversation id, as the state file holds it.
fn saved_conversation(state_file: &std::path::Path) -> Option<String> {
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_file).unwrap()).unwrap();
    saved["lead"]["acp_session"]
        .as_str()
        .map(ToString::to_string)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_lists_its_conversations_and_a_new_one_resets_every_clients_transcript() {
    let dir = Scratch::new("lead-sessions");
    let (url, state_file) = lead_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 499).await;
    lead_reply(&mut ws, "hello").await;

    // The agent's store holds the conversation the lead is in, and the listing flags it.
    let first = saved_conversation(&state_file).expect("the lead saved its conversation");
    let listed = conversations(&mut ws, 500).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first);
    assert!(listed[0].current, "{listed:?}");
    assert_eq!(listed[0].title.as_deref(), Some("hello"));
    assert_eq!(listed[0].cwd, "/tmp");
    assert!(
        listed[0]
            .updated_at
            .as_deref()
            .is_some_and(|t| t.starts_with("20") && t.contains('T') && t.ends_with('Z')),
        "{listed:?}"
    );

    // A second client is watching the conversation the new one replaces.
    let mut watcher = connect(&url).await;
    let events = attach(&mut watcher, "lead", 501).await;
    assert!(
        events.contains(&SessionEvent::User {
            turn: 1,
            text: "hello".into()
        }),
        "{events:?}"
    );

    let second = switch(&mut ws, 502, None)
        .await
        .expect("a new conversation");
    assert_ne!(second, first);
    assert_eq!(saved_conversation(&state_file), Some(second.clone()));

    // The watcher is sent the new transcript without asking: an empty one.
    loop {
        if let ServerMsg::SessionAttached {
            req: None, events, ..
        } = recv(&mut watcher).await
        {
            assert!(events.is_empty(), "{events:?}");
            break;
        }
    }
    assert!(attach(&mut ws, "lead", 503).await.is_empty());

    // The lead works in the new conversation, and both are listed, newest first.
    lead_reply(&mut ws, "second").await;
    let listed = conversations(&mut ws, 504).await;
    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [second.as_str(), first.as_str()], "{listed:?}");
    assert_eq!(listed.iter().filter(|c| c.current).count(), 1, "{listed:?}");
    assert!(listed[0].current, "{listed:?}");
}

/// A conversation cannot change under a turn: the message was written for the conversation on
/// screen, and the agent is mid-prompt in it. Both verbs refuse until the turn is over.
#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_cannot_change_while_the_lead_is_working() {
    let dir = Scratch::new("lead-busy-switch");
    let (url, state_file) = lead_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 799).await;
    let before = saved_conversation(&state_file);

    // A turn that stops on a permission request keeps the lead busy until it is answered.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(800),
            session: "lead".into(),
            text: "needs permission".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.session, "lead");

    for (req, load) in [(801, None), (802, Some("fake-nothing"))] {
        let refused = switch(&mut ws, req, load)
            .await
            .expect_err("a switch while the lead is working");
        assert!(refused.contains("cancel its turn first"), "{refused}");
    }
    assert_eq!(saved_conversation(&state_file), before);

    // Once the turn is over the same verb works.
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    transcript_when(&mut ws, "lead", |events| {
        events.iter().any(
            |e| matches!(e, SessionEvent::TurnEnd { stop_reason, .. } if stop_reason == "end_turn"),
        )
    })
    .await;
    let opened = switch(&mut ws, 803, None)
        .await
        .expect("a new conversation");
    assert_eq!(saved_conversation(&state_file), Some(opened));
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_a_past_conversation_replays_it_and_an_unknown_id_changes_nothing() {
    let dir = Scratch::new("lead-resume");
    let (url, state_file) = lead_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 599).await;
    lead_reply(&mut ws, "hello").await;
    let first = saved_conversation(&state_file).expect("the lead saved its conversation");
    let second = switch(&mut ws, 600, None)
        .await
        .expect("a new conversation");

    // An id the agent has no conversation for leaves the lead in the one it is in.
    let refused = switch(&mut ws, 601, Some("fake-nothing"))
        .await
        .expect_err("an unknown conversation");
    assert!(refused.contains("fake-nothing"), "{refused}");
    assert_eq!(saved_conversation(&state_file), Some(second.clone()));
    lead_reply(&mut ws, "still here").await;

    // Reopening the first conversation makes the agent's replay of it the transcript.
    assert_eq!(switch(&mut ws, 602, Some(&first)).await, Ok(first.clone()));
    assert_eq!(saved_conversation(&state_file), Some(first.clone()));
    let events = attach(&mut ws, "lead", 603).await;
    let user: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(user, ["hello"], "{events:?}");
    assert!(
        events.contains(&SessionEvent::Text {
            text: "echo: hello".into()
        }),
        "{events:?}"
    );
    // The reopened conversation carries on, and the listing flags it rather than the one left.
    lead_reply(&mut ws, "again").await;
    let listed = conversations(&mut ws, 604).await;
    let current: Vec<&str> = listed
        .iter()
        .filter(|c| c.current)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(current, [first.as_str()], "{listed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_keeps_no_conversations_says_so_rather_than_listing_none() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    // The plain fake agent advertises neither `session/list` nor `loadSession`.
    send(
        &mut ws,
        ClientMsg::SessionConversationLs {
            req: Some(700),
            session: "lead".into(),
        },
    )
    .await;
    let message = loop {
        if let ServerMsg::Error {
            req: Some(700),
            message,
            ..
        } = recv(&mut ws).await
        {
            break message;
        }
    };
    assert!(
        message.contains("cannot list past conversations"),
        "{message}"
    );
    assert_eq!(
        switch(&mut ws, 701, Some("fake-session")).await,
        Err("session lead's agent cannot reopen a past conversation".into())
    );
    // Opening a fresh conversation needs no capability: every agent answers `session/new`.
    assert_eq!(switch(&mut ws, 702, None).await, Ok("fake-session".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn create_space_names_the_directory_and_a_denial_opens_nothing() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let call = tokio::spawn(call_tool(
        addr,
        "create_space",
        serde_json::json!({ "cwd": "/tmp" }),
    ));
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.session, "lead");
    assert!(perm.title.contains("/tmp"), "{}", perm.title);
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "deny".into(),
        },
    )
    .await;
    let (text, error) = call.await.unwrap();
    assert!(error, "{text}");
    assert!(text.contains("denied"), "{text}");
    send(&mut ws, ClientMsg::Status { req: Some(7) }).await;
    loop {
        if let ServerMsg::Status { spaces, .. } = recv(&mut ws).await {
            assert!(spaces.is_empty(), "{spaces:?}");
            break;
        }
    }
    // The socket's own SpaceCreate is a client op, not the lead's, and asks no one.
    send(
        &mut ws,
        ClientMsg::SpaceCreate {
            req: Some(8),
            cwd: "/tmp".into(),
            name: None,
        },
    )
    .await;
    loop {
        if let ServerMsg::Created { req: 8, .. } = recv(&mut ws).await {
            break;
        }
    }
}

/// A space in `/tmp` with an agent pane in it; returns the pane id and the session name. With a
/// `message`, the agent's first turn has finished when this returns.
async fn agent_pane(addr: std::net::SocketAddr, message: Option<&str>) -> (String, String) {
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let mut args = serde_json::json!({ "space": space.id, "kind": "agent" });
    if let Some(message) = message {
        args["message"] = message.into();
    }
    let session = start_agent(addr, args).await;
    let (layout, _) = call_tool(addr, "list_spaces", serde_json::json!({})).await;
    let layout: serde_json::Value = serde_json::from_str(&layout).unwrap();
    let pane = layout["spaces"][0]["root"]["b"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    if message.is_some() {
        let (text, error) = call_tool(
            addr,
            "wait_for_agent",
            serde_json::json!({ "agent": pane, "timeout_secs": 10 }),
        )
        .await;
        assert!(!error, "{text}");
    }
    (pane, session)
}

/// The next permission request `session` raises, skipping others' (the lead's `create_space`).
async fn perm_from(ws: &mut Ws, session: &str) -> wheel_proto::PermInfo {
    loop {
        let perm = next_perm(ws).await;
        if perm.session == session {
            return perm;
        }
    }
}

fn parsed(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_reads_an_agent_pane_by_pane_id_or_session_name() {
    let (_, addr) = start_daemon_at().await;
    let (pane, session) = agent_pane(addr, Some("first task")).await;
    for agent in [&pane, &session] {
        let (text, error) =
            call_tool(addr, "read_agent", serde_json::json!({ "agent": agent })).await;
        assert!(!error, "{text}");
        let read = parsed(&text);
        assert_eq!(
            (read["pane"].as_str(), read["session"].as_str()),
            (Some(pane.as_str()), Some(session.as_str())),
            "{text}"
        );
        assert_eq!(read["state"]["phase"], "ready", "{text}");
        let transcript = read["transcript"].as_str().unwrap();
        assert!(
            transcript.contains("> first task\n") && transcript.contains("echo: first task"),
            "{transcript}"
        );
    }
    let (text, error) = call_tool(addr, "read_agent", serde_json::json!({ "agent": "p99" })).await;
    assert!(error && text == "no session p99", "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_messages_an_agent_pane_and_optionally_waits_for_the_reply() {
    let (_, addr) = start_daemon_at().await;
    let (pane, session) = agent_pane(addr, None).await;
    // By pane id, waiting: the reply comes back with the pane and the session.
    let (text, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": pane, "text": "by pane", "wait_secs": 10 }),
    )
    .await;
    assert!(!error, "{text}");
    let sent = parsed(&text);
    assert_eq!(
        (
            sent["pane"].as_str(),
            sent["session"].as_str(),
            &sent["finished"]
        ),
        (
            Some(pane.as_str()),
            Some(session.as_str()),
            &serde_json::json!(true)
        ),
        "{text}"
    );
    assert!(
        sent["reply"].as_str().unwrap().contains("echo: by pane"),
        "{text}"
    );
    // By session name, not waiting: it returns at once with the turn, and the turn still runs.
    let (text, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": session, "text": "by session" }),
    )
    .await;
    assert!(!error, "{text}");
    let sent = parsed(&text);
    assert_eq!(sent["turn"], 2, "{text}");
    assert!(
        sent.get("reply").is_none() && sent["pane"] == pane.as_str(),
        "{text}"
    );
    let (text, error) = call_tool(
        addr,
        "wait_for_agent",
        serde_json::json!({ "agent": pane, "timeout_secs": 10 }),
    )
    .await;
    assert!(!error, "{text}");
    let (read, _) = call_tool(addr, "read_agent", serde_json::json!({ "agent": pane })).await;
    assert!(read.contains("echo: by session"), "{read}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_waits_for_an_agent_pane_to_be_ready() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let (pane, session) = agent_pane(addr, None).await;
    let (text, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": pane, "text": "needs permission" }),
    )
    .await;
    assert!(!error, "{text}");
    // Busy on a turn that waits for the person: the wait runs out and says what state it is in.
    let (text, error) = call_tool(
        addr,
        "wait_for_agent",
        serde_json::json!({ "agent": pane, "timeout_secs": 0.3 }),
    )
    .await;
    assert!(
        error && text.contains("not ready") && text.contains("busy"),
        "{text}"
    );
    let perm = perm_from(&mut ws, &session).await;
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    for agent in [&session, &pane] {
        let (text, error) = call_tool(
            addr,
            "wait_for_agent",
            serde_json::json!({ "agent": agent, "timeout_secs": 10 }),
        )
        .await;
        assert!(!error, "{text}");
        let waited = parsed(&text);
        assert_eq!(
            (
                waited["pane"].as_str(),
                waited["session"].as_str(),
                waited["state"]["phase"].as_str()
            ),
            (Some(pane.as_str()), Some(session.as_str()), Some("ready")),
            "{text}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_cancels_an_agent_pane_turn_and_an_idle_cancel_is_an_error() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let (pane, session) = agent_pane(addr, None).await;
    let (text, error) = call_tool(addr, "cancel_agent", serde_json::json!({ "agent": pane })).await;
    assert!(
        error && text == format!("session {session} has no turn to cancel"),
        "{text}"
    );

    let (text, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": session, "text": "needs permission" }),
    )
    .await;
    assert!(!error, "{text}");
    perm_from(&mut ws, &session).await;
    let (text, error) = call_tool(
        addr,
        "cancel_agent",
        serde_json::json!({ "agent": session }),
    )
    .await;
    assert!(!error, "{text}");
    let cancelled = parsed(&text);
    assert_eq!(
        (
            cancelled["pane"].as_str(),
            cancelled["session"].as_str(),
            &cancelled["turn"]
        ),
        (
            Some(pane.as_str()),
            Some(session.as_str()),
            &serde_json::json!(1)
        ),
        "{text}"
    );
    let (text, error) = call_tool(
        addr,
        "wait_for_agent",
        serde_json::json!({ "agent": pane, "timeout_secs": 10 }),
    )
    .await;
    assert!(!error, "{text}");
}
