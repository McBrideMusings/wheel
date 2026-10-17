//! The lead over the socket, driven by the scripted fake agent: a turn streams back with the
//! own Claude configuration in place, and a permission request reaches every client, where the first
//! answer resumes the agent and a second answer is refused; the lead changes spaces through the
//! daemon's MCP tools and starts agent panes, whose sessions they own.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, SessionEvent, SessionState, StampedEvent, ToolContent};

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
) -> (String, std::net::SocketAddr, common::Daemon) {
    start_daemon_timed(command, state_file, wheeld::AgentTiming::default()).await
}

/// A daemon on `command` whose agents restart and start on `timing`'s waits.
async fn start_daemon_timed(
    command: &str,
    state_file: Option<std::path::PathBuf>,
    timing: wheeld::AgentTiming,
) -> (String, std::net::SocketAddr, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Its instance directory is the one holding the state file, as it is in production.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let instance = state_file
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| {
            // A run saving no layout gets an instance directory of its own, so the inbox one
            // daemon keeps there is never another's.
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::env::temp_dir().join(format!("wheel-lead-instance-{}-{n}", std::process::id()))
        });
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance,
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    // The same order as main.rs: the agent command first, because restoring a saved agent pane
    // starts its agent with it; the lead last, because the conversation it resumes is read from
    // the restored state.
    state.set_agent_command(command.into());
    state.set_agent_timing(timing);
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    state.start_lead(
        command.into(),
        "/tmp".into(),
        wheeld::lead::default_dir(),
        &wheeld::mcp::url(addr),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        addr,
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

/// Collect the lead's reply text until the turn ends.
async fn turn_text(ws: &mut Ws) -> String {
    let mut text = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::Text { text: chunk },
                        ..
                    },
                ..
            } => text.push_str(&chunk),
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { stop_reason, .. },
                        ..
                    },
                ..
            } => {
                assert_eq!(stop_reason, "end_turn");
                return text;
            }
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::Error { message, .. },
                        ..
                    },
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
async fn a_turn_streams_back_from_an_agent_started_with_the_leads_own_configuration() {
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
            text: "hello, show setup".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: hello, show setup"), "{text:?}");
    // The new-session metadata carries the one setting source and the three built-in tools, and
    // the agent process was started with the config directory `status` reports.
    send(&mut ws, ClientMsg::Status { req: Some(40) }).await;
    let config_dir = loop {
        if let ServerMsg::Status { lead_config, .. } = recv(&mut ws).await {
            break lead_config.expect("a lead's config").config_dir.path;
        }
    };
    assert!(config_dir.ends_with("/lead-home"), "{config_dir}");
    assert!(
        text.contains(&format!(
            "setup: sources=user tools=Read,Grep,Glob config_dir={config_dir}\n"
        )),
        "{text:?}"
    );

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
        events[..2]
            .iter()
            .map(|e| e.event.clone())
            .collect::<Vec<_>>(),
        [
            SessionEvent::User {
                turn: 1,
                text: "hello, show setup".into()
            },
            SessionEvent::TurnStart { turn: 1 }
        ]
    );
    // The daemon stamps every live event with the time it arrived; turn 1 started after its
    // user message was recorded.
    assert!(events.iter().all(|e| e.at.is_some()), "{events:?}");
    assert!(events[1].at >= events[0].at, "{events:?}");
    assert!(events
        .iter()
        .any(|e| matches!(&e.event, SessionEvent::Text { text } if text.contains("echo: hello"))));
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
                event:
                    StampedEvent {
                        event: SessionEvent::ToolCall { id, .. },
                        ..
                    },
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
            match event.event {
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
        .map(|e| &e.event)
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
            match event.event {
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
            ServerMsg::Session { event, .. } => match event.event {
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

/// Every `mode` event in the transcript the lead holds, read through a fresh attach.
async fn mode_events(url: &str, req: u64) -> Vec<String> {
    let mut ws = connect(url).await;
    send(
        &mut ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: "lead".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached { events, .. } = recv(&mut ws).await {
            return events
                .into_iter()
                .filter_map(|e| match e.event {
                    SessionEvent::Mode { mode } => Some(mode),
                    _ => None,
                })
                .collect();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mode_change_with_no_mode_option_reaches_the_transcript() {
    let command = format!("{} --no-mode", env!("CARGO_BIN_EXE_wheel-fake-agent"));
    let (url, _, serving) = start_daemon_with(&command, None).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    assert!(config_of(&mut ws, 1).await.iter().all(|o| o.id != "mode"));

    attach_and_send(&mut ws, "switch mode").await;
    turn_text(&mut ws).await;
    assert_eq!(mode_events(&url, 3).await, ["plan"]);
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mode_change_folds_into_the_mode_option_and_not_the_transcript() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    assert_eq!(current(&config_of(&mut ws, 1).await, "mode"), "auto");

    attach_and_send(&mut ws, "switch mode").await;
    turn_text(&mut ws).await;
    assert_eq!(current(&config_of(&mut ws, 3).await, "mode"), "plan");
    assert_eq!(mode_events(&url, 4).await, Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thought_chunk_and_a_resource_link_arrive_as_events() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach_and_send(&mut ws, "reflect").await;
    let mut live = Vec::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: event @ (SessionEvent::Thought { .. } | SessionEvent::Block { .. }),
                        ..
                    },
                ..
            } => live.push(event),
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { .. },
                        ..
                    },
                ..
            } => break,
            _ => {}
        }
    }
    let expected = [
        SessionEvent::Thought {
            text: "pondering ".into(),
        },
        SessionEvent::Thought {
            text: "the question".into(),
        },
        SessionEvent::Block {
            block_kind: "resource_link".into(),
            name: Some("notes.md".into()),
        },
    ];
    assert_eq!(live, expected);

    // The stored transcript merges the thought chunks into one event.
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
        .map(|e| e.event)
        .filter(|e| matches!(e, SessionEvent::Thought { .. } | SessionEvent::Block { .. }))
        .collect();
    assert_eq!(
        stored,
        [
            SessionEvent::Thought {
                text: "pondering the question".into()
            },
            expected[2].clone(),
        ]
    );
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
                event:
                    StampedEvent {
                        event:
                            event @ (SessionEvent::ToolCall { .. } | SessionEvent::ToolUpdate { .. }),
                        ..
                    },
                ..
            } => tools.push(event),
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { .. },
                        ..
                    },
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
        .map(|e| e.event)
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
            event:
                StampedEvent {
                    event: SessionEvent::Error { turn: None, .. },
                    ..
                },
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
    attach_stamped(ws, session, req)
        .await
        .into_iter()
        .map(|e| e.event)
        .collect()
}

/// Like `attach`, keeping each event's time stamp.
async fn attach_stamped(ws: &mut Ws, session: &str, req: u64) -> Vec<StampedEvent> {
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
async fn a_missing_lead_folder_is_an_error_in_the_lead_pane_naming_its_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let instance = std::env::temp_dir().join(format!("wheel-lead-missing-{}", std::process::id()));
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance,
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    let missing = std::path::PathBuf::from("/nonexistent/lead");
    state.start_lead(
        env!("CARGO_BIN_EXE_wheel-fake-agent").into(),
        "/tmp".into(),
        missing.clone(),
        &wheeld::mcp::url(addr),
    );
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    let daemon = common::Daemon::new(serving, state);
    let mut ws = connect(&format!("ws://{addr}/ws?token={TOKEN}")).await;
    let events = loop {
        let events = attach(&mut ws, "lead", 1).await;
        if events
            .iter()
            .any(|e| matches!(e, SessionEvent::Error { .. }))
        {
            break events;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let message = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .unwrap();
    assert!(message.contains("/nonexistent/lead/CLAUDE.md"), "{message}");
    daemon.abort();
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
            text: "hello agent, show setup".into(),
        },
    )
    .await;
    let mut text = String::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { session: s, event } if s == session => match event.event {
                SessionEvent::Text { text: chunk } => text.push_str(&chunk),
                SessionEvent::TurnEnd { turn: 2, .. } => break,
                _ => {}
            },
            ServerMsg::Session { session: s, event } => panic!("{s} got {event:?}"),
            _ => {}
        }
    }
    assert!(text.contains("echo: hello agent, show setup"), "{text:?}");
    assert!(
        text.contains("setup: sources=- tools=- config_dir=") && !text.contains("lead-home"),
        "an agent pane keeps the person's configuration: {text:?}"
    );

    // Closing the pane ends the session: attached clients hear so, and it is gone.
    let (closed, error) = call_tool(addr, "close_pane", serde_json::json!({ "pane": pane })).await;
    assert!(!error, "{closed}");
    loop {
        if let ServerMsg::Session {
            session: s,
            event:
                StampedEvent {
                    event:
                        SessionEvent::Error {
                            turn: None,
                            message,
                        },
                    ..
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

/// Waits short enough that a test never sleeps through a real backoff; five tries as in
/// production.
fn quick() -> wheeld::AgentTiming {
    let mut timing = wheeld::AgentTiming::default();
    timing.restart.backoff = Duration::from_millis(10);
    timing.restart.backoff_max = Duration::from_millis(40);
    timing
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_directory_is_gone_gives_up_and_the_lead_hears() {
    let (url, addr, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, quick()).await;
    let dir = std::env::temp_dir().join(format!("wheel-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (space, _) = open_space(addr, dir.to_str().unwrap()).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    std::fs::remove_dir(&dir).unwrap();

    // Five failed starts with the quick backoff between them.
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

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_was_ready_before_it_stopped_starts_its_count_over() {
    let mut timing = quick();
    timing.restart.tries = 2;
    let (url, addr, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, timing).await;
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let session = start_agent(
        addr,
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "first task" }),
    )
    .await;
    let mut ws = connect(&url).await;
    // Three stops in a row, each after the agent was ready: with two tries allowed, a count
    // that did not start over would have given up on the second.
    for round in 0..3u64 {
        send(
            &mut ws,
            ClientMsg::SessionSend {
                req: Some(100 + round),
                session: session.clone(),
                text: "die now".into(),
            },
        )
        .await;
        loop {
            match recv(&mut ws).await {
                ServerMsg::Error {
                    req: Some(r),
                    message,
                    ..
                } if r == 100 + round => panic!("send refused in round {round}: {message}"),
                ServerMsg::SessionStateChanged {
                    session: s,
                    state: SessionState::GaveUp { tries, message },
                    ..
                } if s == session => {
                    panic!("gave up after {tries} tries in round {round}: {message}")
                }
                ServerMsg::SessionStateChanged {
                    session: s,
                    state: SessionState::Failed { .. },
                    ..
                } if s == session => break,
                _ => {}
            }
        }
        loop {
            if let ServerMsg::SessionStateChanged {
                session: s,
                state: SessionState::Ready,
                ..
            } = recv(&mut ws).await
            {
                if s == session {
                    break;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn start_pane_returns_starting_when_the_agent_is_not_ready_in_time() {
    let mut timing = quick();
    timing.start_wait = Duration::from_millis(200);
    // A process that reads its input and never answers the ACP handshake stays starting.
    let (_, addr, _) = start_daemon_timed("sh -c \"cat > /dev/null\"", None, timing).await;
    let (space, _) = open_space(addr, "/tmp").await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let began = std::time::Instant::now();
    let (text, error) = call_tool(
        addr,
        "start_pane",
        serde_json::json!({ "space": space.id, "kind": "agent", "message": "first task" }),
    )
    .await;
    assert!(!error, "{text}");
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "{:?}",
        began.elapsed()
    );
    let started: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(started["state"]["phase"], "starting", "{text}");
    assert!(
        started["session"].as_str().is_some_and(|s| !s.is_empty()),
        "{text}"
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
            ServerMsg::Session { session: s, event } if s == agent => match event.event {
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

/// Everything a terminal prints over `ws` until it contains `needle`, or `secs` seconds pass.
async fn output_until(ws: &mut Ws, needle: &str, secs: u64) -> String {
    let mut text = String::new();
    let until = tokio::time::Instant::now() + Duration::from_secs(secs);
    // `recv` panics after 60s of silence, so wait in slices shorter than that.
    while !text.contains(needle) && tokio::time::Instant::now() < until {
        let slice = until.min(tokio::time::Instant::now() + Duration::from_secs(5));
        if let Ok(ServerMsg::TermOutput { data, .. }) =
            tokio::time::timeout_at(slice, recv(ws)).await
        {
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

/// Send a client op carrying `req` and return its reply: `Created`, `Ok` or `Started`. A client
/// is the person, so nothing it does is gated.
async fn client_op(ws: &mut Ws, req: u64, msg: ClientMsg) -> ServerMsg {
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            reply @ (ServerMsg::Created { req: r, .. }
            | ServerMsg::Ok { req: r }
            | ServerMsg::Started { req: r, .. })
                if r == req =>
            {
                return reply
            }
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("op {req} failed: {message}"),
            _ => {}
        }
    }
}

/// A space the person made through a client, in `/tmp`: its one pane is the person's.
async fn person_space(ws: &mut Ws) -> wheel_proto::SpaceInfo {
    person_space_in(ws, "/tmp").await
}

async fn person_space_in(ws: &mut Ws, cwd: &str) -> wheel_proto::SpaceInfo {
    let ServerMsg::Created { id, .. } = client_op(
        ws,
        81,
        ClientMsg::SpaceCreate {
            req: Some(81),
            cwd: cwd.into(),
            name: None,
        },
    )
    .await
    else {
        unreachable!()
    };
    send(ws, ClientMsg::Status { req: Some(82) }).await;
    loop {
        if let ServerMsg::Status { spaces, .. } = recv(ws).await {
            return spaces.into_iter().find(|s| s.id == id).unwrap();
        }
    }
}

/// Make `pane` a terminal as the person does, through a client.
async fn person_terminal(ws: &mut Ws, pane: &str) {
    client_op(
        ws,
        83,
        ClientMsg::PaneSetKind {
            req: Some(83),
            pane: pane.into(),
            kind: wheel_proto::NewPaneKind::Terminal,
            artifact: None,
            scope: None,
            url: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_into_a_pane_waits_for_the_person() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let pane = person_space(&mut ws).await.focus;
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

    person_terminal(&mut ws, &pane).await;
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
                format!("type_into_pane on your pane {pane}: \"echo typed-$((6*7))\" ⏎").as_str()
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
        // Typed text shows up whenever the shell gets to it; denied text never does, which only
        // a quiet window can show.
        let printed = if typed {
            output_until(&mut ws, "typed-42", 60).await
        } else {
            output_for(&mut ws, 1000).await
        };
        assert_eq!(printed.contains("typed-42"), typed, "{choice}");
    }
}

/// A space with one terminal pane the person made, and a client watching for permission requests.
async fn terminal_pane(url: &str) -> (String, Ws) {
    let mut ws = connect(url).await;
    let pane = person_space(&mut ws).await.focus;
    person_terminal(&mut ws, &pane).await;
    (pane, ws)
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
    let (pane, mut ws) = terminal_pane(&url).await;
    let call = tokio::spawn(call_tool(
        addr,
        "type_into_pane",
        serde_json::json!({ "pane": pane, "text": "echo late" }),
    ));
    let perm = next_perm(&mut ws).await;
    // The caller gives up and closes its connection.
    call.abort();
    let withdrawn = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let ServerMsg::PermResolved { id, choice } = recv(&mut ws).await {
                return (id, choice);
            }
        }
    })
    .await
    .expect("withdrawn");
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
    let (pane, mut ws) = terminal_pane(&url).await;
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

/// Call `tool` and answer the permission request it raises with `choice`, checking that the
/// request is the lead's and names the tool and `pane`; returns the tool's text and whether it was
/// an error.
async fn answered(
    addr: std::net::SocketAddr,
    ws: &mut Ws,
    tool: &str,
    args: serde_json::Value,
    pane: &str,
    choice: &str,
) -> (String, bool) {
    let answer = async {
        let perm = next_perm(ws).await;
        assert_eq!(perm.session, "lead");
        assert!(
            perm.title.starts_with(tool) && perm.title.contains(&format!("your pane {pane}")),
            "{}",
            perm.title
        );
        send(
            ws,
            ClientMsg::PermAnswer {
                req: None,
                id: perm.id,
                choice: choice.into(),
            },
        )
        .await;
    };
    let (result, ()) = tokio::join!(call_tool(addr, tool, args), answer);
    result
}

/// Call `tool` with nobody answering anything: it finishes only if it asked no one.
async fn unasked(
    addr: std::net::SocketAddr,
    ws: &mut Ws,
    tool: &str,
    args: serde_json::Value,
) -> (String, bool) {
    let result = tokio::time::timeout(Duration::from_secs(60), call_tool(addr, tool, args))
        .await
        .unwrap_or_else(|_| panic!("{tool} waited on a request nobody answered"));
    assert_eq!(perm_list(ws).await, [], "{tool} raised a request");
    result
}

/// The layout as `list_spaces` reports it.
async fn listed(addr: std::net::SocketAddr) -> serde_json::Value {
    serde_json::from_str(
        &call_tool(addr, "list_spaces", serde_json::json!({}))
            .await
            .0,
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_asks_before_reading_or_typing_into_the_persons_terminal_but_not_its_own() {
    let (url, addr) = start_daemon_at().await;
    let (person, mut ws) = terminal_pane(&url).await;
    let (text, error) = answered(
        addr,
        &mut ws,
        "read_terminal",
        serde_json::json!({ "pane": person, "timeout_secs": 0.1 }),
        &person,
        "allow",
    )
    .await;
    assert!(!error, "{text}");
    let (text, error) = answered(
        addr,
        &mut ws,
        "wait_for_text",
        serde_json::json!({ "pane": person, "text": "x", "timeout_secs": 0.1 }),
        &person,
        "deny",
    )
    .await;
    assert_eq!(
        (text, error),
        (format!("the person denied wait_for_text on {person}"), true)
    );

    // A terminal the lead made is its own to type into and read.
    let (lead, error) = unasked(
        addr,
        &mut ws,
        "split_pane",
        serde_json::json!({ "pane": person, "dir": "right" }),
    )
    .await;
    assert!(!error, "{lead}");
    let args = serde_json::json!({ "pane": lead, "kind": "terminal" });
    assert!(!unasked(addr, &mut ws, "set_pane_kind", args).await.1);
    let args = serde_json::json!({ "pane": lead, "text": "echo lead-$((6*7))", "enter": true });
    assert!(!unasked(addr, &mut ws, "type_into_pane", args).await.1);
    let args = serde_json::json!({ "pane": lead, "text": "lead-42", "timeout_secs": 10 });
    let (text, error) = unasked(addr, &mut ws, "wait_for_text", args).await;
    assert!(!error && text.contains("lead-42"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_asks_before_working_the_persons_agent_pane_but_not_its_own() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let space = person_space(&mut ws).await;
    let ServerMsg::Started { session, .. } = client_op(
        &mut ws,
        84,
        ClientMsg::PaneStart {
            req: Some(84),
            pane: space.focus.clone(),
            kind: wheel_proto::StartKind::Agent,
            message: None,
        },
    )
    .await
    else {
        unreachable!()
    };
    let session = session.unwrap();
    // Named by its session, the request still names the pane.
    let (text, error) = answered(
        addr,
        &mut ws,
        "read_agent",
        serde_json::json!({ "agent": session }),
        &space.focus,
        "allow",
    )
    .await;
    assert!(!error, "{text}");
    let (text, error) = answered(
        addr,
        &mut ws,
        "message_agent",
        serde_json::json!({ "agent": space.focus, "text": "hello" }),
        &space.focus,
        "deny",
    )
    .await;
    assert_eq!(
        (text, error),
        (
            format!("the person denied message_agent on {}", space.focus),
            true
        )
    );

    let lead = start_agent(
        addr,
        serde_json::json!({ "space": space.id, "kind": "agent" }),
    )
    .await;
    let args = serde_json::json!({ "agent": lead, "text": "hi", "wait_secs": 10 });
    let (text, error) = unasked(addr, &mut ws, "message_agent", args).await;
    assert!(!error && text.contains("echo: hi"), "{text}");
    let (text, error) = unasked(
        addr,
        &mut ws,
        "read_agent",
        serde_json::json!({ "agent": lead }),
    )
    .await;
    assert!(!error && text.contains("> hi"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_request_leaves_the_persons_pane_as_it_was() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let space = person_space(&mut ws).await;
    let pane = space.focus.clone();
    person_terminal(&mut ws, &pane).await;
    let before = listed(addr).await;
    for (tool, args) in [
        ("close_pane", serde_json::json!({ "pane": pane })),
        (
            "set_pane_kind",
            serde_json::json!({ "pane": pane, "kind": "empty" }),
        ),
        (
            "start_pane",
            serde_json::json!({ "space": space.id, "pane": pane, "kind": "terminal" }),
        ),
        (
            "open_artifact",
            serde_json::json!({ "name": "nosuch", "pane": pane }),
        ),
    ] {
        let (text, error) = answered(addr, &mut ws, tool, args, &pane, "deny").await;
        assert_eq!(
            (text, error),
            (format!("the person denied {tool} on {pane}"), true)
        );
        assert_eq!(listed(addr).await, before, "{tool} changed the layout");
    }
    let (text, error) = answered(
        addr,
        &mut ws,
        "close_pane",
        serde_json::json!({ "pane": pane }),
        &pane,
        "allow",
    )
    .await;
    assert!(!error, "{text}");
    assert_eq!(listed(addr).await["spaces"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_space_asks_only_for_the_panes_the_person_made() {
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    let space = person_space(&mut ws).await;
    let args = serde_json::json!({ "pane": space.focus, "dir": "down" });
    let (lead, _) = unasked(addr, &mut ws, "split_pane", args).await;
    let call = tokio::spawn(call_tool(
        addr,
        "close_space",
        serde_json::json!({ "space": space.id }),
    ));
    let perm = next_perm(&mut ws).await;
    assert_eq!(
        perm.title,
        format!("close_space {} on your pane {}", space.id, space.focus)
    );
    assert!(!perm.title.contains(&lead), "{}", perm.title);
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "deny".into(),
        },
    )
    .await;
    assert!(call.await.unwrap().1, "denied");
    assert_eq!(
        listed(addr).await["spaces"][0]["id"],
        serde_json::json!(space.id)
    );

    // A space the lead made holds only its own panes.
    let (own, _) = open_space(addr, "/tmp").await;
    let own: wheel_proto::SpaceInfo = serde_json::from_str(&own).unwrap();
    let args = serde_json::json!({ "space": own.id });
    assert!(!unasked(addr, &mut ws, "close_space", args).await.1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_makes_panes_without_moving_the_persons_focus() {
    let base = std::env::temp_dir().join(format!("wheel-no-focus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (url, addr) = start_daemon_at().await;
    let mut ws = connect(&url).await;
    // The person works in `a` while `b` is showing.
    let a = person_space_in(&mut ws, &base.to_string_lossy()).await;
    let b = person_space(&mut ws).await;
    let view = |layout: &serde_json::Value| {
        (
            layout["active"].clone(),
            layout["spaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["focus"].clone())
                .collect::<Vec<_>>(),
        )
    };
    let before = view(&listed(addr).await);
    assert_eq!(before.0, serde_json::json!(b.id));

    let (split, _) = unasked(
        addr,
        &mut ws,
        "split_pane",
        serde_json::json!({ "pane": a.focus, "dir": "right" }),
    )
    .await;
    let args = serde_json::json!({ "space": a.id, "kind": "terminal" });
    let (started, error) = unasked(addr, &mut ws, "start_pane", args).await;
    assert!(!error, "{started}");
    let args = serde_json::json!({ "name": "dash", "space": a.id });
    assert!(!unasked(addr, &mut ws, "new_artifact", args).await.1);
    let args = serde_json::json!({ "name": "dash", "space": a.id });
    let (opened, error) = unasked(addr, &mut ws, "open_artifact", args).await;
    assert!(!error, "{opened}");
    let layout = listed(addr).await;
    assert_eq!(view(&layout), before, "the person's view moved: {layout}");

    // Every pane says who made it.
    let made: Vec<(String, String)> = {
        fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
            if node["node"] == "pane" {
                out.push((
                    node["id"].as_str().unwrap().into(),
                    node["created_by"].as_str().unwrap().into(),
                ));
            } else {
                walk(&node["a"], out);
                walk(&node["b"], out);
            }
        }
        let mut out = Vec::new();
        walk(&layout["spaces"][0]["root"], &mut out);
        out
    };
    assert_eq!(made.len(), 4, "{made:?}");
    for (pane, by) in &made {
        let want = if *pane == a.focus { "person" } else { "lead" };
        assert_eq!(by, want, "{pane}");
    }

    // Asked to, a split takes the focus and shows its space.
    let args = serde_json::json!({ "pane": split, "dir": "down", "no_focus": false });
    let (focused, _) = unasked(addr, &mut ws, "split_pane", args).await;
    let layout = listed(addr).await;
    assert_eq!(
        (&layout["active"], &layout["spaces"][0]["focus"]),
        (&serde_json::json!(a.id), &serde_json::json!(focused))
    );
    let _ = std::fs::remove_dir_all(&base);
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
            ServerMsg::Session { session: s, event } if s == session => match event.event {
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
    // The agent sends no times with a replay, so none is invented.
    let replayed = attach_stamped(&mut ws, &session, 399).await;
    assert!(
        replayed.len() >= 4 && replayed.iter().all(|e| e.at.is_none()),
        "{replayed:?}"
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

/// Send the lead `text` and assert on its whole reply: the echo.
async fn lead_reply(ws: &mut Ws, text: &str) {
    assert_eq!(reply(ws, "lead", text).await, format!("echo: {text}"));
}

/// The conversations the lead's agent holds for its directory.
async fn conversations(ws: &mut Ws, req: u64) -> Vec<wheel_proto::ConversationInfo> {
    conversations_of(ws, "lead", req).await
}

/// The conversations `session`'s agent holds for its directory.
async fn conversations_of(
    ws: &mut Ws,
    session: &str,
    req: u64,
) -> Vec<wheel_proto::ConversationInfo> {
    send(
        ws,
        ClientMsg::SessionConversationLs {
            req: Some(req),
            session: session.into(),
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
    switch_of(ws, "lead", req, load).await
}

/// Put `session` in a fresh conversation, or in the one `load` names.
async fn switch_of(
    ws: &mut Ws,
    session: &str,
    req: u64,
    load: Option<&str>,
) -> Result<String, String> {
    let msg = match load {
        Some(id) => ClientMsg::SessionConversationResume {
            req: Some(req),
            session: session.into(),
            id: id.into(),
        },
        None => ClientMsg::SessionConversationNew {
            req: Some(req),
            session: session.into(),
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

/// The conversations the fake agent has closed, in order, once they are `want` or five seconds
/// have passed: the close runs beside the switch, so it can land just after the switch's reply.
async fn closed(dir: &std::path::Path, want: &[&str]) -> Vec<String> {
    let read = || -> Vec<String> {
        std::fs::read_to_string(dir.join("store").join("closed"))
            .unwrap_or_default()
            .lines()
            .map(ToString::to_string)
            .collect()
    };
    for _ in 0..100 {
        if read() == want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    read()
}

/// A switch closes the conversation the lead leaves, and only that one: never the one it moves to,
/// never one a refused switch would have left, never the one it reopens while already in it.
#[tokio::test(flavor = "multi_thread")]
async fn a_switch_closes_the_conversation_the_lead_leaves() {
    let dir = Scratch::new("lead-close");
    let (url, state_file) = lead_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 899).await;
    lead_reply(&mut ws, "hello").await;
    let first = saved_conversation(&state_file).expect("the lead saved its conversation");

    let second = switch(&mut ws, 900, None)
        .await
        .expect("a new conversation");
    assert_eq!(closed(&dir.0, &[&first]).await, [first.as_str()]);
    // The fake agent refuses a prompt in a closed conversation, so this works only in the new one.
    lead_reply(&mut ws, "second").await;

    // A refused switch leaves the lead where it was, and closes nothing.
    switch(&mut ws, 901, Some("fake-nothing"))
        .await
        .expect_err("an unknown conversation");
    assert_eq!(
        switch(&mut ws, 902, Some(&second)).await,
        Ok(second.clone())
    );
    // A close is sent before the next prompt, so once that prompt is answered any close is in.
    lead_reply(&mut ws, "still second").await;
    assert_eq!(closed(&dir.0, &[&first]).await, [first.as_str()]);

    // Reopening the closed one closes the one it leaves.
    assert_eq!(switch(&mut ws, 903, Some(&first)).await, Ok(first.clone()));
    assert_eq!(
        closed(&dir.0, &[&first, &second]).await,
        [first.as_str(), second.as_str()]
    );
    lead_reply(&mut ws, "first again").await;
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

/// A lead that kept one conversation, then a second daemon on the same state file after `between`
/// runs; returns the second daemon's url, the state file and the conversation the first one saved.
async fn restart_lead(
    dir: &std::path::Path,
    between: impl FnOnce(),
) -> (String, std::path::PathBuf, String) {
    let store = dir.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    let state_file = dir.join("spaces.json");
    let (url, _, first_daemon) = start_daemon_with(&command, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    attach(&mut ws, "lead", 1000).await;
    lead_reply(&mut ws, "hello").await;
    let saved =
        saved_conversation(&state_file).expect("the lead saved its conversation on first start");
    drop(ws);
    first_daemon.abort();
    between();
    let (url, _, _) = start_daemon_with(&command, Some(state_file.clone())).await;
    (url, state_file, saved)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_lead_resumes_the_conversation_its_state_file_names() {
    let dir = Scratch::new("lead-restart");
    let (url, state_file, saved) = restart_lead(&dir.0, || {}).await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, "lead", |events| {
        events.contains(&SessionEvent::Text {
            text: "echo: hello".into(),
        })
    })
    .await;
    let user: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(user, ["hello"], "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::Error { .. })),
        "{events:?}"
    );
    assert_eq!(saved_conversation(&state_file), Some(saved));
    // The conversation carries on, numbered after the replayed turn.
    wait_ready(&mut ws).await;
    assert!(reply(&mut ws, "lead", "again")
        .await
        .ends_with("echo: again"));
    let events = attach(&mut ws, "lead", 1001).await;
    assert!(
        events.contains(&SessionEvent::TurnStart { turn: 2 }),
        "{events:?}"
    );
}

/// The data-loss guard: a load that fails must not overwrite the saved id, or one flaky start
/// would lose a conversation the agent still has.
#[tokio::test(flavor = "multi_thread")]
async fn a_lead_whose_saved_conversation_will_not_load_keeps_the_saved_id_and_says_so() {
    let dir = Scratch::new("lead-restart-lost");
    let store = dir.0.join("store");
    let (url, state_file, saved) = restart_lead(&dir.0, || {
        std::fs::remove_dir_all(&store).unwrap();
        std::fs::create_dir_all(&store).unwrap();
    })
    .await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, "lead", |events| !events.is_empty()).await;
    assert_eq!(
        events,
        [SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_NOT_REOPENED.into()
        }]
    );
    assert_eq!(saved_conversation(&state_file), Some(saved.clone()));
    // The lead still works, in a new conversation, and the saved id stays untouched.
    wait_ready(&mut ws).await;
    lead_reply(&mut ws, "fresh").await;
    assert_eq!(saved_conversation(&state_file), Some(saved));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_set_while_the_agent_is_starting_is_refused_at_once() {
    // `sleep` runs but never reads or writes, so the lead stays in `Starting`. `cat` would echo
    // the `initialize` request back; the lead answers it "Method not found" and fails.
    let (url, _, serving) = start_daemon_with("/bin/sleep 600", None).await;
    let mut ws = connect(&url).await;
    // The agent never leaves `Starting`, so a reply at all means the refusal did not wait for it.
    config_set_refused(&mut ws, 1, "model", "sonnet", "is still starting").await;
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switch_in_an_agent_pane_saves_the_new_conversation_in_the_pane() {
    let dir = Scratch::new("agent-sessions");
    let store = dir.0.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    let state_file = dir.0.join("spaces.json");
    let (url, addr, _) = start_daemon_with(&command, Some(state_file.clone())).await;
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
    let pane_conversation = || {
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
        saved["spaces"][0]["root"]["b"]["acp_session"]
            .as_str()
            .map(ToString::to_string)
    };
    let first = pane_conversation().expect("the pane saved its conversation");
    let listed = conversations_of(&mut ws, &session, 900).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first);
    assert!(listed[0].current, "{listed:?}");

    let second = switch_of(&mut ws, &session, 901, None)
        .await
        .expect("a new conversation");
    assert_ne!(second, first);
    assert_eq!(pane_conversation(), Some(second));

    let back = switch_of(&mut ws, &session, 902, Some(&first))
        .await
        .expect("the first conversation again");
    assert_eq!(back, first);
    assert_eq!(pane_conversation(), Some(first));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attach_carries_the_daemons_clock() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    let before = wheel_proto::unix_ms_now();
    send(
        &mut ws,
        ClientMsg::SessionAttach {
            req: Some(1),
            session: "lead".into(),
        },
    )
    .await;
    let sent = loop {
        if let ServerMsg::SessionAttached {
            req: Some(1), now, ..
        } = recv(&mut ws).await
        {
            break now;
        }
    };
    let after = wheel_proto::unix_ms_now();
    assert!(
        (before..=after).contains(&sent),
        "{before} <= {sent} <= {after}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn aborting_a_daemon_leaves_none_of_its_agents_running() {
    // Marked by a store of its own, so only this daemon's agents count; the other tests' daemons
    // in this process run theirs concurrently.
    let dir = Scratch::new("abort-agents");
    let store = dir.0.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let command = format!(
        "{} --store {}",
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        store.display()
    );
    let (url, _, daemon) = start_daemon_with(&command, None).await;
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    let marker = store.display().to_string();
    let mine = || {
        let out = std::process::Command::new("pgrep")
            .args(["-f", &marker])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().count()
    };
    assert!(mine() >= 1, "the lead's agent is running");
    drop(ws);
    daemon.abort();
    for _ in 0..50 {
        if mine() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{} agent processes still running after abort", mine());
}

/// Send `text` to the agent and wait for its reply, so each message is its own finished turn.
async fn say(addr: std::net::SocketAddr, agent: &str, text: &str) {
    let (reply, error) = call_tool(
        addr,
        "message_agent",
        serde_json::json!({ "agent": agent, "text": text, "wait_secs": 10 }),
    )
    .await;
    assert!(
        !error && reply.contains(&format!("echo: {text}")),
        "{reply}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_agent_returns_the_last_turn_unless_asked_for_more() {
    let (_, addr) = start_daemon_at().await;
    let (pane, session) = agent_pane(addr, Some("alpha one")).await;
    say(addr, &pane, "beta two").await;
    say(addr, &pane, "gamma three").await;
    let read = |args: serde_json::Value| async move {
        let (text, error) = call_tool(addr, "read_agent", args).await;
        assert!(!error, "{text}");
        parsed(&text)
    };

    // The default is the third turn only.
    let last = read(serde_json::json!({ "agent": pane })).await;
    let transcript = last["transcript"].as_str().unwrap();
    assert!(
        transcript.contains("> gamma three\n") && transcript.contains("echo: gamma three"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("alpha") && !transcript.contains("beta"),
        "{transcript}"
    );
    assert_eq!(
        (&last["turns"], &last["shown"]),
        (&3.into(), &serde_json::json!([3]))
    );

    // Two turns: the last two, by session name too.
    let two = read(serde_json::json!({ "agent": session, "last_turns": 2 })).await;
    let transcript = two["transcript"].as_str().unwrap();
    assert!(
        transcript.contains("echo: beta two") && transcript.contains("echo: gamma three"),
        "{transcript}"
    );
    assert!(!transcript.contains("alpha"), "{transcript}");
    assert_eq!(two["shown"], serde_json::json!([2, 3]));

    // A grep finds the matching message and reply across turns, each tagged with its turn.
    let grep = read(serde_json::json!({ "agent": pane, "grep": "BETA" })).await;
    assert_eq!(
        grep["transcript"], "[turn 2] > beta two\n[turn 2] echo: beta two\n",
        "{grep}"
    );
    assert_eq!(
        (&grep["matches"], &grep["shown"]),
        (&2.into(), &serde_json::json!([2]))
    );
    let nothing = read(serde_json::json!({ "agent": pane, "grep": "no such text" })).await;
    assert_eq!(
        (nothing["transcript"].as_str(), &nothing["matches"]),
        (Some(""), &0.into())
    );

    // All is the whole thing.
    let all = read(serde_json::json!({ "agent": pane, "all": true })).await;
    let transcript = all["transcript"].as_str().unwrap();
    for turn in ["alpha one", "beta two", "gamma three"] {
        assert!(
            transcript.contains(&format!("> {turn}\n"))
                && transcript.contains(&format!("echo: {turn}")),
            "{transcript}"
        );
    }
    assert_eq!(all["shown"], serde_json::json!([1, 2, 3]));

    // Asking for two things at once, or for nothing, is refused.
    for args in [
        serde_json::json!({ "agent": pane, "all": true, "grep": "x" }),
        serde_json::json!({ "agent": pane, "last_turns": 2, "all": true }),
        serde_json::json!({ "agent": pane, "last_turns": 0 }),
        serde_json::json!({ "agent": pane, "grep": "" }),
    ] {
        let (text, error) = call_tool(addr, "read_agent", args.clone()).await;
        assert!(error, "{args}: {text}");
    }
}

/// A scratch git repo with beads set up, prefix `t`.
fn scratch_beads_repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wheel-beads-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for args in [
        &["git", "init", "-q"][..],
        &[
            "bd",
            "init",
            "--prefix",
            "t",
            "--non-interactive",
            "--quiet",
        ][..],
    ] {
        let out = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lead_creates_shows_lists_and_updates_beads_in_a_spaces_repo() {
    let (_, addr) = start_daemon_at().await;
    let repo = scratch_beads_repo("tools");
    let (space, _) = open_space(addr, repo.to_str().unwrap()).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();
    let call = |tool: &'static str, args: serde_json::Value| async move {
        call_tool(addr, tool, args).await
    };

    let (text, error) = call(
        "create_bead",
        serde_json::json!({
            "space": space.id, "title": "-a dash title", "description": "made by the lead",
            "kind": "feature", "priority": 1, "labels": ["lead"],
        }),
    )
    .await;
    assert!(!error, "{text}");
    let created = parsed(&text);
    let id = created["id"].as_str().expect(&text).to_string();

    let (text, error) = call(
        "show_bead",
        serde_json::json!({ "space": space.id, "id": id }),
    )
    .await;
    assert!(!error, "{text}");
    let shown = parsed(&text);
    let shown = shown.as_array().map(|a| a[0].clone()).unwrap_or(shown);
    assert_eq!(
        (
            shown["title"].as_str(),
            shown["description"].as_str(),
            shown["issue_type"].as_str()
        ),
        (
            Some("-a dash title"),
            Some("made by the lead"),
            Some("feature")
        ),
        "{text}"
    );

    let (text, error) = call(
        "update_bead",
        serde_json::json!({ "space": space.id, "id": id, "status": "in_progress", "append_notes": "started" }),
    )
    .await;
    assert!(!error, "{text}");
    let (text, error) = call(
        "list_beads",
        serde_json::json!({ "space": space.id, "status": "in_progress", "labels": ["lead"] }),
    )
    .await;
    assert!(!error && text.contains(&id), "{text}");
    let (text, error) = call(
        "list_beads",
        serde_json::json!({ "space": space.id, "ready": true }),
    )
    .await;
    assert!(
        !error && !text.contains(&id),
        "an in-progress bead is not ready: {text}"
    );
    let (text, error) = call(
        "list_beads",
        serde_json::json!({ "space": space.id, "ready": true, "kind": "feature", "labels": ["lead"], "priority": 1, "limit": 5 }),
    )
    .await;
    assert!(!error, "bd ready takes the shared filters: {text}");
    let (text, error) = call(
        "list_beads",
        serde_json::json!({ "space": space.id, "kind": "bug" }),
    )
    .await;
    assert!(!error && !text.contains(&id), "{text}");

    // Failures are errors that say why, never an empty list.
    for (tool, args) in [
        (
            "show_bead",
            serde_json::json!({ "space": space.id, "id": "t-nosuch" }),
        ),
        (
            "show_bead",
            serde_json::json!({ "space": space.id, "id": "--db=/x" }),
        ),
        (
            "list_beads",
            serde_json::json!({ "space": space.id, "status": "bogus" }),
        ),
        (
            "list_beads",
            serde_json::json!({ "space": space.id, "ready": true, "all": true }),
        ),
        (
            "update_bead",
            serde_json::json!({ "space": space.id, "id": id }),
        ),
        (
            "update_bead",
            serde_json::json!({ "space": space.id, "id": "t-nosuch", "title": "x" }),
        ),
        (
            "create_bead",
            serde_json::json!({ "space": space.id, "title": "  " }),
        ),
        ("list_beads", serde_json::json!({ "space": "s999" })),
    ] {
        let (text, error) = call(tool, args.clone()).await;
        assert!(error && !text.is_empty(), "{tool} {args}: {text}");
    }
    let (text, _) = call(
        "list_beads",
        serde_json::json!({ "space": space.id, "status": "bogus" }),
    )
    .await;
    assert!(text.contains("invalid status"), "bd's own message: {text}");
    let (text, _) = call("list_beads", serde_json::json!({ "space": "s999" })).await;
    assert_eq!(text, "no space s999");

    // A space whose directory has no beads: bd's refusal comes back as the error.
    let bare = std::env::temp_dir().join(format!("wheel-beads-bare-{}", std::process::id()));
    std::fs::create_dir_all(&bare).unwrap();
    let (bare_space, _) = open_space(addr, bare.to_str().unwrap()).await;
    let bare_space: wheel_proto::SpaceInfo = serde_json::from_str(&bare_space).unwrap();
    let (text, error) = call("list_beads", serde_json::json!({ "space": bare_space.id })).await;
    assert!(error && text.contains("bd list"), "{text}");
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&bare);
}

/// The `bd` status reports for the active space.
async fn status_bd(url: &str) -> wheel_proto::HostBinary {
    let mut ws = connect(url).await;
    send(&mut ws, ClientMsg::Status { req: Some(1) }).await;
    loop {
        if let ServerMsg::Status { bd, .. } = recv(&mut ws).await {
            return bd;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bd_setting_decides_which_bd_the_bead_tools_run_and_status_reports_it() {
    let (url, addr) = start_daemon_at().await;
    let repo = scratch_beads_repo("setting");
    let (space, _) = open_space(addr, repo.to_str().unwrap()).await;
    let space: wheel_proto::SpaceInfo = serde_json::from_str(&space).unwrap();

    // A setting naming a file that is not there: status says missing and names the setting, and
    // the tool returns that as its error, while the daemon keeps answering.
    std::fs::write(
        repo.join("wheel.toml"),
        "[beads]\nbd = \"/nonexistent/wheel-test/bd\"\n",
    )
    .unwrap();
    let bd = status_bd(&url).await;
    assert!(!bd.found && bd.path.is_none(), "{bd:?}");
    let problem = bd.problem.clone().unwrap_or_default();
    assert!(
        problem.contains("beads.bd") && problem.contains("/nonexistent/wheel-test/bd"),
        "{bd:?}"
    );
    assert!(
        !bd.why.is_empty() && bd.install.contains("beads.bd"),
        "{bd:?}"
    );
    let (text, error) =
        call_tool(addr, "list_beads", serde_json::json!({ "space": space.id })).await;
    assert!(error && text.contains("beads.bd"), "{text}");

    // A setting naming the real bd: status shows its path and version, and the tool works.
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("bd"))
        .find(|p| p.is_file())
        .expect("bd on the test's PATH");
    std::fs::write(
        repo.join("wheel.toml"),
        format!("[beads]\nbd = \"{}\"\n", real.display()),
    )
    .unwrap();
    let bd = status_bd(&url).await;
    let version = std::process::Command::new(&real)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        bd.found && bd.from_setting && bd.problem.is_none(),
        "{bd:?}"
    );
    assert_eq!(bd.path.as_deref(), real.to_str());
    assert_eq!(
        bd.version.as_deref(),
        Some(String::from_utf8_lossy(&version.stdout).trim())
    );
    let (text, error) =
        call_tool(addr, "list_beads", serde_json::json!({ "space": space.id })).await;
    assert!(!error, "{text}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_agent_counts_a_queued_message_as_pending_not_as_the_last_turn() {
    let (_, addr) = start_daemon_at().await;
    let (pane, _) = agent_pane(addr, Some("first task")).await;
    // The second message waits on a permission, so the third queues behind it.
    for text in ["needs permission", "queued behind"] {
        let (reply, error) = call_tool(
            addr,
            "message_agent",
            serde_json::json!({ "agent": pane, "text": text }),
        )
        .await;
        assert!(!error, "{reply}");
    }
    let (text, error) = call_tool(addr, "read_agent", serde_json::json!({ "agent": pane })).await;
    assert!(!error, "{text}");
    let read = parsed(&text);
    let transcript = read["transcript"].as_str().unwrap();
    assert!(
        transcript.contains("> needs permission\n")
            && transcript.contains("[tool] rm -rf build")
            && transcript.contains("> queued behind (queued)\n"),
        "{transcript}"
    );
    assert!(!transcript.contains("first task"), "{transcript}");
    assert_eq!(
        (&read["turns"], &read["shown"]),
        (&2.into(), &serde_json::json!([2]))
    );
}

// ---- the lead's login ----

/// A scratch project directory whose `wheel.toml` points `claude.path` at `claude`.
fn login_project(name: &str, claude: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wheel-login-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("wheel.toml"),
        format!("[claude]\npath = \"{claude}\"\n"),
    )
    .unwrap();
    dir
}

/// A `claude` that answers `auth status --json` with `says`, and records the config directory it
/// was run with beside itself.
fn fake_claude(name: &str, says: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path =
        std::env::temp_dir().join(format!("wheel-fake-claude-{name}-{}", std::process::id()));
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s' \"$CLAUDE_CONFIG_DIR\" > \"$0.config\"\necho '{says}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Ask for the lead's login, which runs the preflight.
async fn login_info(ws: &mut Ws) -> (wheel_proto::LeadLogin, wheel_proto::HostBinary) {
    send(ws, ClientMsg::LeadLoginStatus { req: Some(70) }).await;
    loop {
        if let ServerMsg::LeadLoginInfo { login, claude, .. } = recv(ws).await {
            return (login, claude);
        }
    }
}

/// Send the lead `text` and wait for the turn to end, returning its error if it failed.
async fn turn_outcome(ws: &mut Ws, text: &str) -> Result<(), String> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(71),
            session: "lead".into(),
        },
    )
    .await;
    while !matches!(recv(ws).await, ServerMsg::SessionAttached { .. }) {}
    send(
        ws,
        ClientMsg::SessionSend {
            req: Some(72),
            session: "lead".into(),
            text: text.into(),
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { .. },
                        ..
                    },
                ..
            } => return Ok(()),
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::Error { message, .. },
                        ..
                    },
                ..
            } => return Err(message),
            _ => {}
        }
    }
}

async fn login_daemon(agent_flags: &str) -> (Ws, String) {
    let command = format!("{} {agent_flags}", env!("CARGO_BIN_EXE_wheel-fake-agent"));
    let (url, _, daemon) = start_daemon_with(&command, None).await;
    // The daemon stops when its handle drops; the test lives as long as its connection.
    std::mem::forget(daemon);
    let mut ws = connect(&url).await;
    wait_ready(&mut ws).await;
    (ws, url)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preflight_says_login_needed_before_any_turn_and_a_login_sends_the_kept_message() {
    let claude = fake_claude("flow", r#"{"loggedIn":false,"authMethod":"none"}"#);
    let project = login_project("flow", claude.to_str().unwrap());
    let (mut ws, _) = login_daemon("--auth").await;
    person_space_in(&mut ws, project.to_str().unwrap()).await;

    // Early warning: nobody has typed, and the lead's empty config directory already reads as
    // logged out.
    let (login, found) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Needed, "{login:?}");
    assert!(!login.agent_required, "{login:?}");
    let preflight = login.preflight.clone().expect("a preflight answer");
    assert!(!preflight.logged_in, "{preflight:?}");
    assert_eq!(preflight.auth_method.as_deref(), Some("none"));
    assert!(found.found, "{found:?}");
    let config_dir = std::fs::read_to_string(format!("{}.config", claude.display())).unwrap();
    assert!(config_dir.ends_with("/lead-home"), "{config_dir}");
    let ids =
        |l: &wheel_proto::LeadLogin| l.methods.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&login), ["claude-ai-login", "console-login"]);

    // The agent's signal: the turn fails with authRequired, and the message is kept.
    let failed = turn_outcome(&mut ws, "hello lead").await;
    assert!(failed.is_err(), "{failed:?}");
    let (login, _) = login_info(&mut ws).await;
    assert!(login.agent_required, "{login:?}");
    assert_eq!(login.state, wheel_proto::LoginState::Needed);
    assert_eq!(login.kept_messages, ["hello lead"]);
    assert_eq!(login.disagreement, None, "both say logged out");

    // A login that fails leaves the login message up with the exit code.
    send(
        &mut ws,
        ClientMsg::LeadLogin {
            req: Some(73),
            method: Some("console-login".into()),
        },
    )
    .await;
    let (pane, code) = loop {
        if let ServerMsg::LeadLoginFinished {
            pane, code, method, ..
        } = recv(&mut ws).await
        {
            assert_eq!(method, "console-login");
            break (pane, code);
        }
    };
    assert_eq!(code, Some(3));
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Needed, "{login:?}");
    assert_eq!(login.last_exit.as_ref().and_then(|e| e.code), Some(3));
    assert_eq!(login.kept_messages, ["hello lead"]);
    // The pane stays open, showing a terminal.
    send(&mut ws, ClientMsg::Status { req: Some(74) }).await;
    let spaces = loop {
        if let ServerMsg::Status { spaces, .. } = recv(&mut ws).await {
            break spaces;
        }
    };
    let shown = spaces.iter().any(|s| {
        fn has(node: &wheel_proto::Node, pane: &str) -> bool {
            match node {
                wheel_proto::Node::Pane { id, kind, .. } => {
                    id == pane && matches!(kind, wheel_proto::PaneKind::Terminal { .. })
                }
                wheel_proto::Node::Split { a, b, .. } => has(a, pane) || has(b, pane),
            }
        }
        has(&s.root, &pane)
    });
    assert!(shown, "pane {pane} is a terminal in the layout");

    // A login that succeeds clears the state and sends the kept message again.
    send(
        &mut ws,
        ClientMsg::LeadLogin {
            req: Some(75),
            method: None,
        },
    )
    .await;
    let mut resent = None;
    let mut finished = None;
    while resent.is_none() || finished.is_none() {
        match recv(&mut ws).await {
            ServerMsg::LeadLoginFinished { code, method, .. } => {
                assert_eq!(method, "claude-ai-login");
                finished = Some(code);
            }
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: SessionEvent::TurnEnd { .. },
                        ..
                    },
                ..
            } => resent = Some(()),
            _ => {}
        }
    }
    assert_eq!(finished, Some(Some(0)));
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Ok, "{login:?}");
    assert!(login.kept_messages.is_empty(), "{login:?}");

    // The panes ran exactly the advertised command and arguments, with the lead's config dir.
    let ran = std::fs::read_to_string(format!("{config_dir}/login-ran")).unwrap();
    assert_eq!(
        ran,
        "args=--auth --login-exit 3 fake_login=console\nargs=--auth --login-exit 0 fake_login=-\n"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logged_in_preflight_with_an_auth_required_turn_shows_the_disagreement() {
    let claude = fake_claude("disagree", r#"{"loggedIn":true,"authMethod":"claude.ai"}"#);
    let project = login_project("disagree", claude.to_str().unwrap());
    let (mut ws, _) = login_daemon("--auth").await;
    person_space_in(&mut ws, project.to_str().unwrap()).await;
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Ok, "{login:?}");
    assert_eq!(login.disagreement, None);

    assert!(turn_outcome(&mut ws, "hi").await.is_err());
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(
        login.state,
        wheel_proto::LoginState::Needed,
        "the agent decides"
    );
    let said = login.disagreement.expect("a disagreement");
    assert!(
        said.contains("authRequired")
            && said.contains("loggedIn: true")
            && said.contains("claude.ai"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logged_out_preflight_with_a_working_turn_shows_the_disagreement() {
    let claude = fake_claude("usable", r#"{"loggedIn":false,"authMethod":"none"}"#);
    let project = login_project("usable", claude.to_str().unwrap());
    let (mut ws, _) = login_daemon("").await;
    person_space_in(&mut ws, project.to_str().unwrap()).await;
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Needed, "{login:?}");

    assert_eq!(turn_outcome(&mut ws, "hi").await, Ok(()));
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(
        login.state,
        wheel_proto::LoginState::Ok,
        "the agent decides: {login:?}"
    );
    let said = login.disagreement.expect("a disagreement");
    assert!(
        said.contains("loggedIn: false") && said.contains("succeeded"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_claude_status_says_missing_and_the_agents_signal_still_works() {
    let project = login_project("missing", "/nonexistent/wheel-test/claude");
    let (mut ws, _) = login_daemon("--auth").await;
    person_space_in(&mut ws, project.to_str().unwrap()).await;
    let (login, claude) = login_info(&mut ws).await;
    assert!(!claude.found && claude.path.is_none(), "{claude:?}");
    let problem = claude.problem.clone().unwrap_or_default();
    assert!(
        problem.contains("claude.path") && problem.contains("/nonexistent/wheel-test/claude"),
        "{claude:?}"
    );
    assert!(
        !claude.why.is_empty() && claude.install.contains("claude.path"),
        "{claude:?}"
    );
    assert!(
        login.preflight.is_none() && login.preflight_problem.is_some(),
        "{login:?}"
    );
    assert_eq!(login.state, wheel_proto::LoginState::Unknown);

    assert!(turn_outcome(&mut ws, "hi").await.is_err());
    let (login, _) = login_info(&mut ws).await;
    assert_eq!(login.state, wheel_proto::LoginState::Needed, "{login:?}");
    assert_eq!(login.methods.len(), 2);
    let _ = std::fs::remove_dir_all(&project);
}
