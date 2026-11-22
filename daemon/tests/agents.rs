//! Agent panes over the socket, driven by the scripted fake agent: a turn streams back with the
//! person's own Claude configuration and Wheel's brief in place, a permission request reaches every
//! client, where the first answer resumes the agent and a second answer is refused, and a pane's
//! conversation survives a restart and moves between its chat and its terminal.

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    ClientMsg, ServerMsg, AgentEvent, AgentState, StampedEvent, StartKind, ToolContent,
};

/// A connection to the daemon. `pending` holds the messages `client_op` read past on its way to
/// its reply; `recv` returns them before anything new off the socket.
struct Ws {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pending: std::collections::VecDeque<ServerMsg>,
}
const TOKEN: &str = "agents-test-token";

async fn start_daemon() -> String {
    start_daemon_at().await.0
}

/// The daemon's WebSocket url and its address.
async fn start_daemon_at() -> (String, std::net::SocketAddr) {
    let (url, addr, _) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    (url, addr)
}

/// A daemon whose agent panes run `command`, keeping its layout in `state_file`; the handle stops
/// it serving when aborted.
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
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::env::temp_dir().join(format!("wheel-agents-instance-{}-{n}", std::process::id()))
        });
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        instance.clone(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    // Every agent gets a home of its own, so nothing from the machine's login reaches a test.
    // A daemon restored from a state file keeps the directories, and the conversations in them.
    let claude_home = instance.join("agent-home");
    let codex_home = instance.join("codex-home");
    let pi_home = instance.join("pi-home");
    for home in [&claude_home, &codex_home, &pi_home] {
        if state_file.is_none() {
            let _ = std::fs::remove_dir_all(home);
        }
        std::fs::create_dir_all(home).unwrap();
    }
    state
        .setting_set(
            "agent.claude.home",
            claude_home.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
    state
        .setting_set("agent.codex.home", codex_home.to_str().unwrap(), None, true)
        .unwrap();
    state
        .setting_set("agent.pi.home", pi_home.to_str().unwrap(), None, true)
        .unwrap();
    // Before the layout is restored, because restoring a saved agent pane starts its agent.
    state
        .setting_set("agent.claude.command", command, None, true)
        .unwrap();
    // The same stand-in answers as Codex when a pane runs it, because Codex starts it with
    // `app-server`.
    state
        .setting_set("agent.codex.command", command, None, true)
        .unwrap();
    // And as pi, which starts it with `--mode rpc`.
    state
        .setting_set("agent.pi.command", command, None, true)
        .unwrap();
    state.set_agent_timing(timing);
    if let Some(path) = state_file {
        state.restore_spaces(path).await.unwrap();
    }
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        addr,
        common::Daemon::new(serving, state),
    )
}

async fn connect(url: &str) -> Ws {
    Ws {
        stream: tokio_tungstenite::connect_async(url).await.unwrap().0,
        pending: Default::default(),
    }
}

async fn send(ws: &mut Ws, msg: ClientMsg) {
    ws.stream
        .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    if let Some(msg) = ws.pending.pop_front() {
        return msg;
    }
    recv_socket(ws).await
}

/// The next message off the socket, ignoring `pending`.
async fn recv_socket(ws: &mut Ws) -> ServerMsg {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), ws.stream.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Collect an agent's reply text until the turn ends.
async fn turn_text(ws: &mut Ws) -> String {
    let mut text = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::Text { text: chunk },
                        ..
                    },
                ..
            } => text.push_str(&chunk),
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::TurnEnd { stop_reason, .. },
                        ..
                    },
                ..
            } => {
                assert_eq!(stop_reason, "end_turn");
                return text;
            }
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::Error { message, .. },
                        ..
                    },
                ..
            } => panic!("agent error: {message}"),
            _ => {}
        }
    }
}

/// Send a client op carrying `req` and return its reply: `Created`, `Ok` or `Started`. Every
/// other message read on the way, broadcasts included, waits in the connection's `pending` queue
/// for its next `recv`, so a test that goes on to wait for a broadcast the op causes still sees it.
async fn client_op(ws: &mut Ws, req: u64, msg: ClientMsg) -> ServerMsg {
    send(ws, msg).await;
    let mut skipped = Vec::new();
    let reply = loop {
        match recv_socket(ws).await {
            reply @ (ServerMsg::Created { req: r, .. }
            | ServerMsg::Ok { req: r }
            | ServerMsg::Started { req: r, .. })
                if r == req =>
            {
                break reply
            }
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("op {req} failed: {message}"),
            other => skipped.push(other),
        }
    };
    // Ahead of anything already queued: these arrived first.
    for msg in skipped.into_iter().rev() {
        ws.pending.push_front(msg);
    }
    reply
}

/// Make a space for `cwd`; returns its id and the one pane it holds.
async fn space(url: &str, cwd: &str) -> (String, String) {
    let mut ws = connect(url).await;
    let created = client_op(
        &mut ws,
        9001,
        ClientMsg::SpaceCreate {
            req: Some(9001),
            cwd: cwd.into(),
            name: None,
            focus: true,
            background: false,
            pane: None,
            session: None,
        },
    )
    .await;
    let ServerMsg::Created { id, .. } = created else {
        panic!("{created:?}")
    };
    send(&mut ws, ClientMsg::Status { req: Some(9002) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(9002),
            spaces,
            ..
        } = recv(&mut ws).await
        {
            let space = spaces.into_iter().find(|s| s.id == id).unwrap();
            let pane = wheeld::spaces::first_pane(&space.root).to_string();
            return (id, pane);
        }
    }
}

/// Start an agent in `pane` with `message` as its first message; returns its agent name once it
/// is ready, or already busy with that message.
async fn start_agent(url: &str, pane: &str, message: Option<&str>) -> String {
    start_agent_as(url, pane, message, None).await
}

/// [`start_agent`] for the agent called `agent`.
async fn start_agent_as(
    url: &str,
    pane: &str,
    message: Option<&str>,
    agent: Option<&str>,
) -> String {
    let mut ws = connect(url).await;
    let started = client_op(
        &mut ws,
        9003,
        ClientMsg::PaneStart {
            req: Some(9003),
            pane: pane.into(),
            kind: StartKind::Agent,
            message: message.map(String::from),
            agent: agent.map(String::from),
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    match started {
        ServerMsg::Started {
            agent: Some(agent),
            state: Some(AgentState::Ready | AgentState::Busy { .. }),
            ..
        } => agent,
        other => panic!("{other:?}"),
    }
}

/// A space in `/tmp` holding one agent pane; returns the pane id and the agent name. With a
/// `message`, the agent's first turn has finished when this returns.
async fn agent_pane(url: &str, message: Option<&str>) -> (String, String) {
    let (_, pane) = space(url, "/tmp").await;
    let agent = start_agent(url, &pane, message).await;
    if message.is_some() {
        let mut ws = connect(url).await;
        transcript_when(&mut ws, &agent, |events| {
            events.contains(&AgentEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into(),
            })
        })
        .await;
    }
    (pane, agent)
}

/// A new space with an agent pane whose agent is ready; returns the agent name.
async fn ready_agent(url: &str) -> String {
    agent_pane(url, None).await.1
}

/// The layout as `status` lists it, every agent pane carrying its agent's state.
async fn layout(url: &str) -> serde_json::Value {
    let mut ws = connect(url).await;
    send(&mut ws, ClientMsg::Status { req: Some(9004) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(9004),
            spaces,
            ..
        } = recv(&mut ws).await
        {
            return serde_json::json!({ "spaces": spaces });
        }
    }
}

/// Ask for an agent in `pane` and return the daemon's reply, `started` or an error, as it came.
async fn pane_start_reply(url: &str, pane: &str, message: &str) -> ServerMsg {
    let mut ws = connect(url).await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9005),
            pane: pane.into(),
            kind: StartKind::Agent,
            message: Some(message.into()),
            agent: None,
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    loop {
        if let reply @ (ServerMsg::Started { req: 9005, .. }
        | ServerMsg::Error {
            req: Some(9005), ..
        }) = recv(&mut ws).await
        {
            return reply;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_streams_back_from_an_agent_pane_with_the_persons_config_and_the_brief() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    send(
        &mut ws,
        ClientMsg::AgentAttach {
            req: Some(1),
            agent: agent.clone(),
        },
    )
    .await;
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::AgentAttached { req: Some(1), .. }
    ) {}
    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(2),
            text: "hello, show setup".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: hello, show setup"), "{text:?}");
    // The agent runs with the Claude config directory set for every agent, and Wheel's brief
    // appended to its system prompt, naming the pane it sits in.
    let setup = text
        .lines()
        .find_map(|l| l.strip_prefix("setup: config_dir="))
        .unwrap_or_else(|| panic!("{text:?}"));
    assert!(setup.ends_with("/agent-home"), "{setup}");
    assert!(
        text.contains("brief: You are running inside Wheel"),
        "{text:?}"
    );
    assert!(text.contains(&format!("(agent {agent})")), "{text:?}");
    assert!(text.contains("seat: You are the agent in pane"), "{text:?}");

    // A client that attaches afterwards gets the whole conversation, chunks merged.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::AgentAttach {
            req: Some(3),
            agent: agent.clone(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::AgentAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    assert_eq!(
        events[..2]
            .iter()
            .map(|e| e.event.clone())
            .collect::<Vec<_>>(),
        [
            AgentEvent::User {
                turn: 1,
                text: "hello, show setup".into()
            },
            AgentEvent::TurnStart { turn: 1 }
        ]
    );
    // The daemon stamps every live event with the time it arrived; turn 1 started after its
    // user message was recorded.
    assert!(events.iter().all(|e| e.at.is_some()), "{events:?}");
    assert!(events[1].at >= events[0].at, "{events:?}");
    assert!(events
        .iter()
        .any(|e| matches!(&e.event, AgentEvent::Text { text } if text.contains("echo: hello"))));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_reaches_every_client_and_the_first_answer_wins() {
    let url = start_daemon().await;
    let mut asker = connect(&url).await;
    let mut other = connect(&url).await;
    let agent = ready_agent(&url).await;
    send(
        &mut asker,
        ClientMsg::AgentAttach {
            req: Some(1),
            agent: agent.clone(),
        },
    )
    .await;
    while !matches!(recv(&mut asker).await, ServerMsg::AgentAttached { .. }) {}
    send(
        &mut asker,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(2),
            text: "needs permission".into(),
        },
    )
    .await;

    // The other client never attached to the agent, and still sees the request.
    let perm = loop {
        if let ServerMsg::PermRequested { perm } = recv(&mut other).await {
            break perm;
        }
    };
    assert_eq!(perm.agent, agent);
    assert_eq!(perm.title, "rm -rf build");
    // The agent suggested a rule, so the request can also be allowed for good.
    let offered: Vec<&str> = perm.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(offered, ["allow", "allow_always", "deny"]);

    // The request names the tool call the agent announced just before asking. Agent events and
    // permission requests reach a client through separate forwarders, so either can arrive first.
    let (mut announced, mut gated) = (None, None);
    while announced.is_none() || gated.is_none() {
        match recv(&mut asker).await {
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::ToolCall { id, .. },
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
        text.contains("permission: allow"),
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
async fn a_question_is_asked_one_at_a_time_and_every_pick_reaches_the_agent() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "ask a question").await;

    let first = next_perm(&mut ws).await;
    assert_eq!(first.title, "Which color?");
    assert_eq!(first.why.reason.as_deref(), Some("Color"));
    let offered: Vec<(&str, &str, Option<&str>)> = first
        .options
        .iter()
        .map(|o| (o.id.as_str(), o.kind.as_str(), o.description.as_deref()))
        .collect();
    assert_eq!(
        offered,
        [
            ("pick:0", "answer", Some("Warm")),
            ("pick:1", "answer", Some("Cool")),
            ("deny", "reject_once", None),
        ]
    );
    send(
        &mut ws,
        ClientMsg::PermAnswer { req: Some(20), id: first.id.clone(), choice: "pick:1".into() },
    )
    .await;
    // The second question is a new request; the tool call is not answered until it has one.
    let second = next_perm(&mut ws).await;
    assert_eq!(second.title, "Which size?");
    assert_ne!(second.id, first.id);
    send(
        &mut ws,
        ClientMsg::PermAnswer { req: Some(21), id: second.id.clone(), choice: "pick:0".into() },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(
        text.contains(r#"question: allow answers={"Which color?":"Blue","Which size?":"Small"}"#),
        "the agent got both answers: {text:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn skipping_a_question_refuses_the_tool_call() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "ask a question").await;
    let first = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::PermAnswer { req: Some(20), id: first.id.clone(), choice: "deny".into() },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("question: deny"), "{text:?}");
    assert!(!text.contains("answers="), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_message_shows_at_once_and_starts_inside_the_running_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    // Turn 1 waits on a permission answer, so turn 2 queues behind it.
    attach_and_send(&mut ws, &agent, "needs permission").await;
    let perm = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(12),
            text: "second".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::Agent { event, .. } = recv(&mut ws).await {
            match event.event {
                AgentEvent::User { turn: 2, text } => {
                    assert_eq!(text, "second");
                    break;
                }
                other => assert!(
                    !matches!(
                        other,
                        AgentEvent::TurnEnd { .. } | AgentEvent::TurnStart { turn: 2 }
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
        ClientMsg::AgentAttach {
            req: Some(3),
            agent: agent.clone(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::AgentAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let kinds: Vec<_> = events
        .iter()
        .map(|e| &e.event)
        .filter(|e| !matches!(e, AgentEvent::Text { .. } | AgentEvent::ToolCall { .. }))
        .cloned()
        .collect();
    assert_eq!(
        kinds,
        [
            AgentEvent::User {
                turn: 1,
                text: "needs permission".into()
            },
            AgentEvent::TurnStart { turn: 1 },
            AgentEvent::User {
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
    // The agent takes the second message into the turn running, so it starts inside turn 1 and
    // both turns end together.
    let mut order = Vec::new();
    while !(order.contains(&"turn_end 1".to_string()) && order.contains(&"turn_end 2".to_string())) {
        if let ServerMsg::Agent { event, .. } = recv(&mut ws).await {
            match event.event {
                AgentEvent::TurnStart { turn } => order.push(format!("turn_start {turn}")),
                AgentEvent::TurnEnd { turn, .. } => order.push(format!("turn_end {turn}")),
                _ => {}
            }
        }
    }
    assert_eq!(order, ["turn_start 2", "turn_end 2", "turn_end 1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_one_turn_and_the_queued_message_still_runs() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    let cancel = |req| ClientMsg::AgentCancel {
        req: Some(req),
        agent: agent.clone(),
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
                assert_eq!(message, format!("agent {agent} has no running turn to cancel"));
                break;
            }
            ServerMsg::Ok { req: 20 } => panic!("an idle cancel replied ok"),
            _ => {}
        }
    }

    attach_and_send(&mut ws, &agent, "needs permission").await;
    next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: None,
            text: "second".into(),
        },
    )
    .await;
    send(&mut ws, cancel(21)).await;
    // The agent stops the turn waiting on the permission request. The reply and the agent events
    // come from different tasks, so the reply has no place in the order.
    let mut order = Vec::new();
    let mut replied = false;
    while order.last() != Some(&"turn_end 2 end_turn".to_string()) || !replied {
        match recv(&mut ws).await {
            ServerMsg::Agent { event, .. } => match event.event {
                AgentEvent::TurnStart { turn } => order.push(format!("turn_start {turn}")),
                AgentEvent::TurnEnd { turn, stop_reason } => {
                    order.push(format!("turn_end {turn} {stop_reason}"))
                }
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
            "turn_end 1 cancelled",
            "turn_start 2",
            "turn_end 2 end_turn"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_cancelled_before_the_agent_takes_it_in_leaves_the_transcript() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "slow start then essay").await;
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::Agent { event: StampedEvent { event: AgentEvent::TurnStart { turn: 1 }, .. }, .. }
    ) {}
    assert!(matches!(agent_cancel(&mut ws, &agent, 30).await, ServerMsg::Ok { .. }));
    // The agent kept nothing of the prompt, so every client is told to drop it, and the turn
    // ends with no `turn_end` of its own.
    loop {
        if let ServerMsg::Agent { event, .. } = recv(&mut ws).await {
            match event.event {
                AgentEvent::Retract { turn: 1 } => break,
                AgentEvent::TurnEnd { turn: 1, stop_reason } => {
                    panic!("the dropped prompt's turn ended as {stop_reason}")
                }
                _ => {}
            }
        }
    }
    send(&mut ws, ClientMsg::AgentSend { agent: agent.clone(), req: None, text: "second".into() }).await;
    while !matches!(
        recv(&mut ws).await,
        ServerMsg::Agent { event: StampedEvent { event: AgentEvent::TurnEnd { turn: 2, .. }, .. }, .. }
    ) {}
    // A client attaching now sees only the message the agent has.
    let mut late = connect(&url).await;
    send(&mut late, ClientMsg::AgentAttach { req: Some(31), agent: agent.clone() }).await;
    let events = loop {
        if let ServerMsg::AgentAttached { req: Some(31), events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let shown: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            AgentEvent::User { turn, text } => Some(format!("{turn} {text}")),
            AgentEvent::TurnStart { turn } => Some(format!("turn_start {turn}")),
            AgentEvent::TurnEnd { turn, .. } => Some(format!("turn_end {turn}")),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["2 second", "turn_start 2", "turn_end 2"]);
}

async fn attach_and_send(ws: &mut Ws, agent: &str, text: &str) {
    send(
        ws,
        ClientMsg::AgentAttach {
            req: Some(10),
            agent: agent.into(),
        },
    )
    .await;
    while !matches!(
        recv(ws).await,
        ServerMsg::AgentAttached { req: Some(10), .. }
    ) {}
    send(
        ws,
        ClientMsg::AgentSend {
            agent: agent.into(),
            req: Some(11),
            text: text.into(),
        },
    )
    .await;
}

/// The agent's config options as the daemon last had them, read through an attach.
async fn config_of(ws: &mut Ws, agent: &str, req: u64) -> Vec<wheel_proto::ConfigOption> {
    send(
        ws,
        ClientMsg::AgentAttach {
            req: Some(req),
            agent: agent.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentAttached {
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

async fn send_config_set(ws: &mut Ws, agent: &str, req: u64, config: &str, value: &str) {
    send(
        ws,
        ClientMsg::AgentConfigSet {
            req: Some(req),
            agent: agent.into(),
            config: config.into(),
            value: value.into(),
        },
    )
    .await;
}

/// Set one option and wait for the refusal answering it, which must name `needle`.
async fn config_set_refused(
    ws: &mut Ws,
    agent: &str,
    req: u64,
    config: &str,
    value: &str,
    needle: &str,
) {
    send_config_set(ws, agent, req, config, value).await;
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
    let agent = ready_agent(&url).await;

    let options = config_of(&mut ws, &agent, 1).await;
    let mut ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["mode", "model"]);
    assert_eq!(current(&options, "model"), "default");

    send_config_set(&mut ws, &agent, 2, "model", "sonnet").await;
    // The change reaches every connection as a broadcast, not only the setter's reply.
    let mut broadcast = None;
    let mut acked = false;
    while !acked || broadcast.is_none() {
        match recv(&mut ws).await {
            ServerMsg::Ok { req: 2 } => acked = true,
            ServerMsg::AgentConfig { agent, options } if agent == agent => {
                broadcast = Some(options)
            }
            _ => {}
        }
    }
    assert_eq!(current(&broadcast.unwrap(), "model"), "sonnet");
    assert_eq!(
        current(&config_of(&mut ws, &agent, 3).await, "model"),
        "sonnet"
    );

    // A value the agent does not offer is refused, and nothing moves.
    config_set_refused(
        &mut ws,
        &agent,
        4,
        "model",
        "no-such-model",
        "no-such-model",
    )
    .await;
    assert_eq!(
        current(&config_of(&mut ws, &agent, 5).await, "model"),
        "sonnet"
    );

    // An option the agent never named never reaches it.
    config_set_refused(&mut ws, &agent, 6, "not-an-option", "x", "not-an-option").await;
}

/// Every `mode` event in `agent`'s transcript, read through a fresh attach.
async fn mode_events(url: &str, agent: &str, req: u64) -> Vec<String> {
    let mut ws = connect(url).await;
    send(
        &mut ws,
        ClientMsg::AgentAttach {
            req: Some(req),
            agent: agent.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentAttached { events, .. } = recv(&mut ws).await {
            return events
                .into_iter()
                .filter_map(|e| match e.event {
                    AgentEvent::Mode { mode } => Some(mode),
                    _ => None,
                })
                .collect();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mode_change_folds_into_the_mode_option_and_not_the_transcript() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    assert_eq!(
        current(&config_of(&mut ws, &agent, 1).await, "mode"),
        "default"
    );

    attach_and_send(&mut ws, &agent, "switch mode").await;
    turn_text(&mut ws).await;
    assert_eq!(
        current(&config_of(&mut ws, &agent, 3).await, "mode"),
        "plan"
    );
    assert_eq!(mode_events(&url, &agent, 4).await, Vec::<String>::new());
}

/// The first footer broadcast for `agent` that counts `shells` and `agents`; earlier ones, such as
/// the one sent when the agent starts, are skipped.
async fn footer_until(ws: &mut Ws, agent: &str, done: impl Fn(&wheel_proto::AgentFooter) -> bool) -> wheel_proto::AgentFooter {
    loop {
        if let ServerMsg::AgentFooter { agent: a, footer } = recv(ws).await {
            if a == agent && done(&footer) {
                return footer;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn running_background_tasks_reach_clients_with_command_call_and_live_output() {
    use wheel_proto::TaskKind;
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;

    attach_and_send(&mut ws, &agent, "start background").await;
    // The ambient monitor in the agent's list is not work, so it is not a task. The command's
    // row has its command, its call and the last lines of the file its result names; the
    // subagent's has its brief and progress.
    let footer = footer_until(&mut ws, &agent, |f| {
        f.tasks.len() == 3
            && f.tasks[0].tail.len() == 3
            && f.tasks[1].tool_uses.is_some()
    })
    .await;
    let shell = &footer.tasks[0];
    assert_eq!((shell.id.as_str(), shell.kind), ("bash-1", TaskKind::Shell));
    assert_eq!(shell.name, "Build the Release app");
    assert_eq!(shell.command.as_deref(), Some("make deploy-client"));
    assert_eq!(shell.call.as_deref(), Some("bg-call-1"));
    assert_eq!(shell.tail, vec!["line 1", "line 2", "line 3"]);
    assert_eq!(shell.output_bytes, 21);
    assert!(shell.started_ms > 0 && shell.last_output_ms.is_some());
    let sub = &footer.tasks[1];
    assert_eq!((sub.kind, sub.name.as_str()), (TaskKind::Agent, "Explore: find callers"));
    assert_eq!(sub.command.as_deref(), Some("Find every caller of AgentFooter"));
    assert_eq!((sub.tool_uses, sub.last_tool.as_deref(), sub.tokens), (Some(2), Some("Bash"), Some(26666)));
    assert_eq!(footer.tasks[2].name, "code-reviewer: review the diff");

    // A write to the output file reaches every client within the second the footer waits.
    let path = shell.output_path.clone().unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut file, b"line 4\n").unwrap();
    drop(file);
    let footer = tokio::time::timeout(Duration::from_secs(5), footer_until(&mut ws, &agent, |f| {
        f.tasks.first().is_some_and(|t| t.tail.last().map(String::as_str) == Some("line 4"))
    }))
    .await
    .expect("the new line reaches the footer");
    assert_eq!(footer.tasks[0].output_bytes, 28);

    // The full output is read on request.
    send(&mut ws, ClientMsg::TaskOutput { req: Some(41), agent: agent.clone(), task: "bash-1".into() }).await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::TaskOutput { req: 41, text, truncated } => {
                assert_eq!(text, "line 1\nline 2\nline 3\nline 4\n");
                assert!(!truncated);
                break;
            }
            ServerMsg::Error { req: Some(41), message, .. } => panic!("task output failed: {message}"),
            _ => {}
        }
    }

    attach_and_send(&mut ws, &agent, "end background").await;
    footer_until(&mut ws, &agent, |f| f.tasks.is_empty()).await;
    send(&mut ws, ClientMsg::TaskOutput { req: Some(42), agent: agent.clone(), task: "bash-1".into() }).await;
    loop {
        if let ServerMsg::Error { req: Some(42), message, .. } = recv(&mut ws).await {
            assert_eq!(message, "no running background task bash-1");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thought_streams_in_chunks_and_is_stored_whole() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "reflect").await;
    let mut live = Vec::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: event @ AgentEvent::Thought { .. },
                        ..
                    },
                ..
            } => live.push(event),
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::TurnEnd { .. },
                        ..
                    },
                ..
            } => break,
            _ => {}
        }
    }
    let expected = [
        AgentEvent::Thought {
            text: "pondering ".into(),
        },
        AgentEvent::Thought {
            text: "the question".into(),
        },
    ];
    assert_eq!(live, expected);

    // The stored transcript merges the thought chunks into one event.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::AgentAttach {
            req: Some(3),
            agent: agent.clone(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::AgentAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let stored: Vec<_> = events
        .into_iter()
        .map(|e| e.event)
        .filter(|e| matches!(e, AgentEvent::Thought { .. }))
        .collect();
    assert_eq!(
        stored,
        [AgentEvent::Thought {
            text: "pondering the question".into()
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_carries_its_diff_and_its_result_keeps_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "show a diff").await;
    let mut tools = Vec::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event:
                            event @ (AgentEvent::ToolCall { .. } | AgentEvent::ToolUpdate { .. }),
                        ..
                    },
                ..
            } => tools.push(event),
            ServerMsg::Agent {
                event:
                    StampedEvent {
                        event: AgentEvent::TurnEnd { .. },
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
        AgentEvent::ToolCall {
            id: "call-2".into(),
            title: "Edit src/a.rs".into(),
            tool_kind: "edit".into(),
            status: "pending".into(),
            content: vec![diff("two\n")],
            detail: None,
        },
        AgentEvent::ToolUpdate {
            id: "call-2".into(),
            title: None,
            status: Some("completed".into()),
            content: None,
        },
    ];
    assert_eq!(tools, expected);

    // The stored transcript keeps both, so a client attaching later folds the same content.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::AgentAttach {
            req: Some(3),
            agent: agent.clone(),
        },
    )
    .await;
    let events = loop {
        if let ServerMsg::AgentAttached { events, .. } = recv(&mut late).await {
            break events;
        }
    };
    let stored: Vec<_> = events
        .into_iter()
        .map(|e| e.event)
        .filter(|e| {
            matches!(
                e,
                AgentEvent::ToolCall { .. } | AgentEvent::ToolUpdate { .. }
            )
        })
        .collect();
    assert_eq!(stored, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_turn_withdraws_its_permission_request() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "needs permission").await;
    let perm = loop {
        if let ServerMsg::PermRequested { perm } = recv(&mut ws).await {
            break perm;
        }
    };
    send(
        &mut ws,
        ClientMsg::AgentCancel {
            req: Some(12),
            agent: agent.clone(),
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
    // The agent was told too: the turn ends as cancelled.
    loop {
        if let ServerMsg::Agent {
            event:
                StampedEvent {
                    event: AgentEvent::TurnEnd { stop_reason, .. },
                    ..
                },
            ..
        } = recv(&mut ws).await
        {
            assert_eq!(stop_reason, "cancelled");
            break;
        }
    }
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
async fn a_crashed_agent_is_restarted_and_keeps_working() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "die now").await;
    loop {
        if let ServerMsg::Agent {
            event:
                StampedEvent {
                    event: AgentEvent::Error { turn: None, .. },
                    ..
                },
            ..
        } = recv(&mut ws).await
        {
            break;
        }
    }
    let waited = agent_wait(&mut ws, &agent, 30.0, 19).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(20),
            text: "back again".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: back again"), "{text:?}");
}

/// Attach `ws` to `agent` and wait for its transcript.
async fn attach(ws: &mut Ws, agent: &str, req: u64) -> Vec<AgentEvent> {
    attach_stamped(ws, agent, req)
        .await
        .into_iter()
        .map(|e| e.event)
        .collect()
}

/// Like `attach`, keeping each event's time stamp.
async fn attach_stamped(ws: &mut Ws, agent: &str, req: u64) -> Vec<StampedEvent> {
    send(
        ws,
        ClientMsg::AgentAttach {
            req: Some(req),
            agent: agent.into(),
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::AgentAttached {
                req: Some(r),
                events,
                ..
            } if r == req => return events,
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("attach {agent}: {message}"),
            _ => {}
        }
    }
}

/// Waits short enough that a test never sleeps through a real backoff; five tries as in
/// production.
fn quick() -> wheeld::AgentTiming {
    let mut timing = wheeld::AgentTiming::default();
    timing.restart.backoff = Duration::from_millis(10);
    timing.restart.backoff_max = Duration::from_millis(40);
    timing
}

/// `pane_start` with `dir`, as the reply came.
async fn pane_start_in(url: &str, pane: &str, dir: &str) -> ServerMsg {
    let mut ws = connect(url).await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9006),
            pane: pane.into(),
            kind: StartKind::Agent,
            message: None,
            agent: None,
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: Some(dir.into()),
        },
    )
    .await;
    loop {
        if let reply @ (ServerMsg::Started { req: 9006, .. }
        | ServerMsg::Error {
            req: Some(9006), ..
        }) = recv(&mut ws).await
        {
            return reply;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_started_with_a_directory_runs_there_and_the_pane_says_so() {
    let url = start_daemon().await;
    let worktree = std::env::temp_dir().join(format!("wheel-dir-{}", std::process::id()));
    std::fs::create_dir_all(&worktree).unwrap();
    let (_, pane) = space(&url, "/tmp").await;

    let relative = pane_start_in(&url, &pane, "some/where").await;
    let ServerMsg::Error { message, .. } = relative else {
        panic!("{relative:?}")
    };
    assert!(message.contains("not an absolute path"), "{message}");
    let missing = pane_start_in(&url, &pane, "/no/such/dir/anywhere").await;
    let ServerMsg::Error { message, .. } = missing else {
        panic!("{missing:?}")
    };
    assert!(message.contains("not an existing directory"), "{message}");

    let started = pane_start_in(&url, &pane, worktree.to_str().unwrap()).await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    let mut ws = connect(&url).await;
    send(
        &mut ws,
        ClientMsg::PaneInfo {
            req: Some(9007),
            pane: pane.clone(),
        },
    )
    .await;
    let fields = loop {
        if let ServerMsg::PaneInfo { fields, .. } = recv(&mut ws).await {
            break fields;
        }
    };
    let directory = fields.iter().find(|f| f.label == "Directory").map(|f| f.value.clone());
    assert_eq!(directory.as_deref(), worktree.to_str());
    std::fs::remove_dir(&worktree).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_directory_is_gone_gives_up_and_says_why() {
    let (url, _, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, quick()).await;
    let dir = std::env::temp_dir().join(format!("wheel-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_, pane) = space(&url, dir.to_str().unwrap()).await;
    std::fs::remove_dir(&dir).unwrap();

    // Five failed starts with the quick backoff between them.
    let reply = pane_start_reply(&url, &pane, "first task").await;
    let ServerMsg::Error { message: text, .. } = reply else {
        panic!("{reply:?}")
    };
    assert!(
        text.contains("could not start: gave up after 5 tries")
            && text.contains("is not a directory"),
        "{text}"
    );
    let agent = text.split_whitespace().nth(1).unwrap();
    let layout = layout(&url).await;
    let state = &layout["spaces"][0]["root"]["state"];
    assert_eq!(layout["spaces"][0]["root"]["agent"], agent, "{layout}");
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
        ClientMsg::AgentSend {
            req: Some(1),
            agent: agent.into(),
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
                    message.contains(&format!("agent {agent} is gave_up after 5 tries")),
                    "{message}"
                );
                break;
            }
            ServerMsg::AgentQueued { req: 1, .. } => {
                panic!("an agent that gave up took a message")
            }
            _ => {}
        }
    }
    // The refused message was never queued.
    let events = attach(&mut ws, agent, 2).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::User { text, .. } if text == "anyone?")),
        "a refused message was queued: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_was_ready_before_it_stopped_starts_its_count_over() {
    let mut timing = quick();
    timing.restart.tries = 2;
    let (url, _, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, timing).await;
    let (_, agent) = agent_pane(&url, Some("first task")).await;
    let mut ws = connect(&url).await;
    // Three stops in a row, each after the agent was ready: with two tries allowed, a count
    // that did not start over would have given up on the second.
    for round in 0..3u64 {
        send(
            &mut ws,
            ClientMsg::AgentSend {
                req: Some(100 + round),
                agent: agent.clone(),
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
                ServerMsg::AgentStateChanged {
                    agent: s,
                    state: AgentState::GaveUp { tries, message },
                    ..
                } if s == agent => {
                    panic!("gave up after {tries} tries in round {round}: {message}")
                }
                ServerMsg::AgentStateChanged {
                    agent: s,
                    state: AgentState::Failed { .. },
                    ..
                } if s == agent => break,
                _ => {}
            }
        }
        loop {
            if let ServerMsg::AgentStateChanged {
                agent: s,
                state: AgentState::Ready,
                ..
            } = recv(&mut ws).await
            {
                if s == agent {
                    break;
                }
            }
        }
    }
}

/// An agent program that reads its input and never answers `initialize`, whatever arguments it
/// is given, so its agent stays starting.
fn never_ready_agent() -> String {
    let path = std::env::temp_dir().join(format!("wheel-never-ready-{}", std::process::id()));
    if !path.exists() {
        std::fs::write(&path, "#!/bin/sh\nexec cat > /dev/null\n").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    path.display().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn start_pane_returns_starting_when_the_agent_is_not_ready_in_time() {
    let mut timing = quick();
    timing.start_wait = Duration::from_millis(200);
    let (url, _, _) = start_daemon_timed(&never_ready_agent(), None, timing).await;
    let (_, pane) = space(&url, "/tmp").await;
    let began = std::time::Instant::now();
    let reply = pane_start_reply(&url, &pane, "first task").await;
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "{:?}",
        began.elapsed()
    );
    let ServerMsg::Started {
        agent: Some(agent),
        state: Some(AgentState::Starting),
        ..
    } = reply
    else {
        panic!("{reply:?}")
    };
    assert!(!agent.is_empty());
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
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let other = ready_agent(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    attach(&mut ws, &other, 1).await;
    attach(&mut ws, &agent, 2).await;
    // Asked only once attached: `attach` drops the daemon-wide messages it reads past, so a
    // request raised before it returned would never reach `next_perm`.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(5),
            agent: agent.clone(),
            text: "needs permission".into(),
        },
    )
    .await;

    let perm = next_perm(&mut ws).await;
    assert_eq!(
        (perm.agent.as_str(), perm.tool_call.as_deref()),
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
    // The answer reaches the agent; the other agent, attached on the same connection, hears
    // nothing.
    let mut text = String::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent { agent: s, event } if s == agent => match event.event {
                AgentEvent::Text { text: chunk } => text.push_str(&chunk),
                AgentEvent::TurnEnd { turn: 1, .. } => break,
                _ => {}
            },
            ServerMsg::Agent { agent: s, event } => panic!("{s} got {event:?}"),
            _ => {}
        }
    }
    assert!(text.contains("permission: allow"), "{text:?}");

    // Two agents' requests wait side by side.
    for (agent, req) in [(agent.as_str(), 3), (other.as_str(), 4)] {
        send(
            &mut ws,
            ClientMsg::AgentSend {
                req: Some(req),
                agent: agent.into(),
                text: "needs permission".into(),
            },
        )
        .await;
    }
    let first = next_perm(&mut ws).await;
    let second = next_perm(&mut ws).await;
    let (agent_perm, other_perm) = if first.agent == agent {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(
        (agent_perm.agent.as_str(), other_perm.agent.as_str()),
        (agent.as_str(), other.as_str())
    );

    // Closing the agent's pane withdraws its request and leaves the other's.
    // The withdrawal can reach this connection before the close's own reply, so both are read in
    // one loop.
    send(&mut ws, ClientMsg::PaneClose { req: Some(6), pane }).await;
    let (mut closed, mut withdrawn) = (false, false);
    while !(closed && withdrawn) {
        match recv(&mut ws).await {
            ServerMsg::PermResolved { id, choice } => {
                assert_eq!((id, choice.as_str()), (agent_perm.id.clone(), "withdrawn"));
                withdrawn = true;
            }
            ServerMsg::Ok { req: 6 } => closed = true,
            ServerMsg::Error {
                req: Some(6),
                message,
                ..
            } => panic!("op 6 failed: {message}"),
            _ => {}
        }
    }
    send(&mut ws, ClientMsg::Status { req: Some(7) }).await;
    let perms = loop {
        if let ServerMsg::Status {
            req: Some(7),
            perms,
            ..
        } = recv(&mut ws).await
        {
            break perms;
        }
    };
    assert_eq!(perms, vec![other_perm]);
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

/// Attach to `agent` until a notice is up over it; returns the notice and the transcript that
/// came with the attach.
async fn notice_when_up(ws: &mut Ws, agent: &str) -> (String, Vec<AgentEvent>) {
    for req in 100..200 {
        let events = attach(ws, agent, req).await;
        let up = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                if let ServerMsg::AgentNotice {
                    agent: s,
                    notice: Some(notice),
                } = recv(ws).await
                {
                    if s == agent {
                        return notice;
                    }
                }
            }
        })
        .await;
        if let Ok(notice) = up {
            return (notice, events);
        }
    }
    panic!("no notice came up over {agent}");
}

/// Attach to `agent` until its transcript satisfies `done`; returns that transcript.
async fn transcript_when(
    ws: &mut Ws,
    agent: &str,
    done: impl Fn(&[AgentEvent]) -> bool,
) -> Vec<AgentEvent> {
    for req in 100..200 {
        let events = attach(ws, agent, req).await;
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{agent}'s transcript never got there");
}

/// A daemon with an agent whose first task has finished, then a second daemon restarted on the
/// same state file, after `between` runs; returns the second daemon's url and the agent's name.
async fn restart_with_an_agent(
    command: &str,
    dir: &std::path::Path,
    between: impl FnOnce(),
) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let (url, _, first_daemon) = start_daemon_with(command, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_agent(&url, &pane, Some("first task")).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &agent, |events| {
        events.contains(&AgentEvent::TurnEnd {
            turn: 1,
            stop_reason: "end_turn".into(),
        })
    })
    .await;
    // The pane saved the agent's id for the conversation.
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let pane = &saved["spaces"][0]["root"];
    assert_eq!(pane["agent"], serde_json::json!(agent));
    assert!(
        pane["conversation"]
            .as_str()
            .is_some_and(|id| id.len() == 36),
        "{pane}"
    );
    drop(ws);
    first_daemon.abort();
    between();
    let (url, _, _) = start_daemon_with(command, Some(state_file)).await;
    (url, agent)
}

/// Send `text` to `agent` and collect its reply until the turn ends.
async fn reply(ws: &mut Ws, agent: &str, text: &str) -> String {
    send(
        ws,
        ClientMsg::AgentSend {
            req: Some(300),
            agent: agent.into(),
            text: text.into(),
        },
    )
    .await;
    let mut turn = None;
    let mut reply = String::new();
    loop {
        match recv(ws).await {
            ServerMsg::AgentQueued {
                req: 300, turn: t, ..
            } => turn = Some(t),
            ServerMsg::Agent { agent: s, event } if s == agent => match event.event {
                AgentEvent::Text { text } => reply.push_str(&text),
                AgentEvent::TurnEnd { turn: t, .. } if Some(t) == turn => return reply,
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
    let (url, agent) =
        restart_with_an_agent(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {}).await;

    // The agent resumed the conversation, and its transcript is the agent's, as whole turns.
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    assert_eq!(
        events,
        [
            AgentEvent::User {
                turn: 1,
                text: "first task".into()
            },
            AgentEvent::TurnStart { turn: 1 },
            AgentEvent::Text {
                text: "echo: first task".into()
            },
            AgentEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into()
            },
        ],
        "{events:?}"
    );
    // The transcript is read with no times, so none is invented.
    let replayed = attach_stamped(&mut ws, &agent, 399).await;
    assert!(
        replayed.len() >= 4 && replayed.iter().all(|e| e.at.is_none()),
        "{replayed:?}"
    );
    // The conversation carries on, numbered after the resumed turn.
    let text = reply(&mut ws, &agent, "after restart").await;
    assert_eq!(text, "echo: after restart");
    let events = attach(&mut ws, &agent, 400).await;
    assert!(
        events.contains(&AgentEvent::TurnStart { turn: 2 }),
        "{events:?}"
    );
}

/// How many of `events` are the message a restore sends an agent whose turn it ended.
fn told_turn_ended(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::User { text, .. } if text.starts_with("wheeld restarted at ")))
        .count()
}

/// The panes the layout file at `state_file` names as in a turn.
fn saved_mid_turn(state_file: &std::path::Path) -> Vec<String> {
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_file).unwrap()).unwrap();
    serde_json::from_value(saved["mid_turn"].clone()).unwrap_or_default()
}

/// Wait until the layout file at `state_file` names exactly `panes` as in a turn.
async fn saved_mid_turn_when(state_file: &std::path::Path, panes: &[&str]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let saved = saved_mid_turn(state_file);
        if saved == panes {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "the layout names {saved:?} in a turn, not {panes:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_in_a_turn_when_wheeld_stops_is_told_once_that_the_turn_ended() {
    let dir = Scratch::new("mid-turn");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let state_file = dir.0.join("spaces.json");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (busy_pane, busy) = agent_pane(&url, Some("first task")).await;
    let (_, idle) = agent_pane(&url, Some("first task")).await;
    // A turn that waits on a permission has not ended; the file says so as soon as it starts,
    // so a wheeld that dies without its stop's save still has it.
    let mut ws = connect(&url).await;
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(2),
            agent: busy.clone(),
            text: "needs permission".into(),
        },
    )
    .await;
    while !matches!(recv(&mut ws).await, ServerMsg::PermRequested { .. }) {}
    saved_mid_turn_when(&state_file, &[&busy_pane]).await;
    drop(ws);
    // As wheeld does on SIGTERM: the stop's save, then the agents end with the process.
    first.abort();
    assert_eq!(saved_mid_turn(&state_file), [busy_pane.clone()]);

    let (url, _, second) = start_daemon_with(fake, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    // The busy agent takes the message as its first new turn, and runs it.
    let events = transcript_when(&mut ws, &busy, |events| {
        told_turn_ended(events) == 1
            && matches!(events.last(), Some(AgentEvent::TurnEnd { .. }))
    })
    .await;
    let told = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::User { text, .. } if text.starts_with("wheeld restarted at ") => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        told.contains(" UTC. The turn you were in ended when it stopped, and every process you ran stopped with it."),
        "{told}"
    );
    // The idle agent resumed its conversation and was told nothing.
    let events = transcript_when(&mut ws, &idle, |events| events.len() >= 4).await;
    assert_eq!(told_turn_ended(&events), 0, "{events:?}");
    // That turn ended, so the next stop names no pane, and the next start tells nobody again.
    saved_mid_turn_when(&state_file, &[]).await;
    drop(ws);
    second.abort();

    let (url, _, _) = start_daemon_with(fake, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &busy, |events| told_turn_ended(events) >= 1).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let events_later = attach(&mut ws, &busy, 900).await;
    assert_eq!(told_turn_ended(&events_later), 1, "{events_later:?}");
    assert_eq!(events_later.len(), events.len(), "{events_later:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_load_that_fails_leaves_only_the_notice() {
    let dir = Scratch::new("failed-load");
    // The agent's store has lost this conversation.
    let store = dir.0.join("agent-home").join("projects");
    let (url, agent) =
        restart_with_an_agent(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {
            std::fs::remove_dir_all(&store).unwrap()
        })
        .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    assert_eq!(events, [], "the notice is never a transcript line");
    let text = reply(&mut ws, &agent, "hello again").await;
    assert_eq!(text, "echo: hello again");
}

/// A daemon with its state file in `dir`, beside the store its agents keep their conversations
/// in, and one agent pane ready; returns its url, that file's path and the agent's program name.
async fn agent_with_store(dir: &std::path::Path) -> (String, std::path::PathBuf, String) {
    let state_file = dir.join("spaces.json");
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(state_file.clone()),
    )
    .await;
    let agent = ready_agent(&url).await;
    (url, state_file, agent)
}

/// Send `agent` `text` and assert on its whole reply: the echo.
async fn agent_reply(ws: &mut Ws, agent: &str, text: &str) {
    assert_eq!(reply(ws, agent, text).await, format!("echo: {text}"));
}

/// The conversations `agent`'s agent keeps for its directory.
async fn conversations_of(
    ws: &mut Ws,
    agent: &str,
    req: u64,
) -> Vec<wheel_proto::ConversationInfo> {
    send(
        ws,
        ClientMsg::AgentConversationLs {
            req: Some(req),
            agent: agent.into(),
            limit: None,
            before: None,
            before_id: None,
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

/// Put `agent` in a fresh conversation, or in the one `load` names; the agent's id for it, or
/// why it refused.
async fn switch_of(
    ws: &mut Ws,
    agent: &str,
    req: u64,
    load: Option<&str>,
) -> Result<String, String> {
    let msg = match load {
        Some(id) => ClientMsg::AgentConversationResume {
            req: Some(req),
            agent: agent.into(),
            id: id.into(),
        },
        None => ClientMsg::AgentConversationNew {
            req: Some(req),
            agent: agent.into(),
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

/// The conversation id the first space's agent pane saved, as the state file holds it.
fn saved_conversation(state_file: &std::path::Path) -> Option<String> {
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_file).unwrap()).unwrap();
    saved["spaces"][0]["root"]["conversation"]
        .as_str()
        .map(ToString::to_string)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_lists_its_conversations_and_a_new_one_resets_every_clients_transcript() {
    let dir = Scratch::new("agent-store-agents");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 499).await;
    agent_reply(&mut ws, &agent, "hello").await;

    // The agent's store holds the conversation the pane is in, and the listing flags it.
    let first = saved_conversation(&state_file).expect("the pane saved its conversation");
    let listed = conversations_of(&mut ws, &agent, 500).await;
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
    let events = attach(&mut watcher, &agent, 501).await;
    assert!(
        events.contains(&AgentEvent::User {
            turn: 1,
            text: "hello".into()
        }),
        "{events:?}"
    );

    let second = switch_of(&mut ws, &agent, 502, None)
        .await
        .expect("a new conversation");
    assert_ne!(second, first);
    // The agent has not written an empty conversation out, so its id is not saved yet.
    assert_eq!(saved_conversation(&state_file), None);

    // The watcher is sent the new transcript without asking: an empty one.
    loop {
        if let ServerMsg::AgentAttached {
            req: None, events, ..
        } = recv(&mut watcher).await
        {
            assert!(events.is_empty(), "{events:?}");
            break;
        }
    }
    assert!(attach(&mut ws, &agent, 503).await.is_empty());

    // The agent works in the new conversation, and both are listed, newest first.
    agent_reply(&mut ws, &agent, "second").await;
    assert_eq!(saved_conversation(&state_file), Some(second.clone()));
    let listed = conversations_of(&mut ws, &agent, 504).await;
    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [second.as_str(), first.as_str()], "{listed:?}");
    assert_eq!(listed.iter().filter(|c| c.current).count(), 1, "{listed:?}");
    assert!(listed[0].current, "{listed:?}");
}

/// `agent new` and `agents` take an agent pane's id as well as an agent name, as every
/// other agent verb does.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_conversation_and_the_listing_accept_an_agent_panes_id() {
    let dir = Scratch::new("agent-store-pane-id");
    let state_file = dir.0.join("spaces.json");
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(state_file.clone()),
    )
    .await;
    let (pane, agent) = agent_pane(&url, None).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 600).await;
    agent_reply(&mut ws, &agent, "hello").await;
    let first = saved_conversation(&state_file).expect("the pane saved its conversation");

    let listed = conversations_of(&mut ws, &pane, 601).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first);

    let second = switch_of(&mut ws, &pane, 602, None)
        .await
        .expect("a new conversation through the pane id");
    assert_ne!(second, first);
}

/// A conversation cannot change under a turn: the message was written for the conversation on
/// screen, and the agent is mid-prompt in it. Both verbs refuse until the turn is over.
#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_cannot_change_while_the_agent_is_working() {
    let dir = Scratch::new("agent-busy-switch");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 799).await;
    assert_eq!(saved_conversation(&state_file), None);

    // A turn that stops on a permission request keeps the agent busy until it is answered.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(800),
            agent: agent.clone(),
            text: "needs permission".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.agent, agent);
    let before = saved_conversation(&state_file);
    assert!(before.is_some(), "the first prompt saves the conversation");

    for (req, load) in [(801, None), (802, Some("fake-nothing"))] {
        let refused = switch_of(&mut ws, &agent, req, load)
            .await
            .expect_err("a switch while the agent is working");
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
    transcript_when(&mut ws, &agent, |events| {
        events.iter().any(
            |e| matches!(e, AgentEvent::TurnEnd { stop_reason, .. } if stop_reason == "end_turn"),
        )
    })
    .await;
    let opened = switch_of(&mut ws, &agent, 803, None)
        .await
        .expect("a new conversation");
    assert_ne!(Some(opened), before);
    assert_eq!(saved_conversation(&state_file), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_a_past_conversation_replays_it_and_an_unknown_id_changes_nothing() {
    let dir = Scratch::new("agent-store-resume");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 599).await;
    agent_reply(&mut ws, &agent, "hello").await;
    let first = saved_conversation(&state_file).expect("the pane saved its conversation");
    let second = switch_of(&mut ws, &agent, 600, None)
        .await
        .expect("a new conversation");

    // An id the agent has no conversation for leaves the pane in the one it is in.
    let refused = switch_of(&mut ws, &agent, 601, Some("fake-nothing"))
        .await
        .expect_err("an unknown conversation");
    assert!(refused.contains("fake-nothing"), "{refused}");
    assert_eq!(saved_conversation(&state_file), None);
    agent_reply(&mut ws, &agent, "still here").await;
    assert_eq!(saved_conversation(&state_file), Some(second.clone()));

    // Reopening the first conversation makes its history the transcript.
    assert_eq!(
        switch_of(&mut ws, &agent, 602, Some(&first)).await,
        Ok(first.clone())
    );
    assert_eq!(saved_conversation(&state_file), Some(first.clone()));
    let events = attach(&mut ws, &agent, 603).await;
    let user: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(user, ["hello"], "{events:?}");
    assert!(
        events.contains(&AgentEvent::Text {
            text: "echo: hello".into()
        }),
        "{events:?}"
    );
    // The reopened conversation carries on, and the listing flags it rather than the one left.
    agent_reply(&mut ws, &agent, "again").await;
    let listed = conversations_of(&mut ws, &agent, 604).await;
    let current: Vec<&str> = listed
        .iter()
        .filter(|c| c.current)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(current, [first.as_str()], "{listed:?}");
}

/// The next permission request `agent` raises, skipping others'.
async fn perm_from(ws: &mut Ws, agent: &str) -> wheel_proto::PermInfo {
    loop {
        let perm = next_perm(ws).await;
        if perm.agent == agent {
            return perm;
        }
    }
}

/// Send `agent_wait` for `target` and return the reply: `agent_waited` or an error.
async fn agent_wait(ws: &mut Ws, target: &str, secs: f64, req: u64) -> ServerMsg {
    send(
        ws,
        ClientMsg::AgentWait {
            req: Some(req),
            agent: target.into(),
            timeout_secs: secs,
        },
    )
    .await;
    loop {
        match recv(ws).await {
            reply @ ServerMsg::AgentWaited { req: r, .. } if r == req => return reply,
            reply @ ServerMsg::Error { req: Some(r), .. } if r == req => return reply,
            _ => {}
        }
    }
}

/// Send `agent_cancel` for `target` and return the reply: `ok` or an error.
async fn agent_cancel(ws: &mut Ws, target: &str, req: u64) -> ServerMsg {
    send(
        ws,
        ClientMsg::AgentCancel {
            req: Some(req),
            agent: target.into(),
        },
    )
    .await;
    loop {
        match recv(ws).await {
            reply @ ServerMsg::Ok { req: r } if r == req => return reply,
            reply @ ServerMsg::Error { req: Some(r), .. } if r == req => return reply,
            _ => {}
        }
    }
}

/// Queue `text` for `target` without waiting for the turn.
async fn queue(ws: &mut Ws, target: &str, text: &str, req: u64) {
    send(
        ws,
        ClientMsg::AgentSend {
            req: Some(req),
            agent: target.into(),
            text: text.into(),
        },
    )
    .await;
}

/// Send `msg` (its `req` is `req`) and return the reply: `ok` or an error.
async fn ok_or_error(ws: &mut Ws, msg: ClientMsg, req: u64) -> ServerMsg {
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            reply @ ServerMsg::Ok { req: r } if r == req => return reply,
            reply @ ServerMsg::Error { req: Some(r), .. } if r == req => return reply,
            _ => {}
        }
    }
}

/// Send `msg` (an unsend of `turn`, its `req` is `req`) and read `ws` until both its reply and the
/// `retract` event of `turn` have arrived, in whichever order; returns the reply.
async fn unsend_and_retract(ws: &mut Ws, msg: ClientMsg, req: u64, turn: u64) -> ServerMsg {
    send(ws, msg).await;
    let (mut reply, mut retracted) = (None, false);
    while reply.is_none() || !retracted {
        match recv(ws).await {
            r @ ServerMsg::Ok { req: x } if x == req => reply = Some(r),
            r @ ServerMsg::Error { req: Some(x), .. } if x == req => return r,
            ServerMsg::Agent {
                event: StampedEvent {
                    event: AgentEvent::Retract { turn: t },
                    ..
                },
                ..
            } if t == turn => retracted = true,
            _ => {}
        }
    }
    reply.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_taken_back_from_the_queue_or_before_the_agent_answers_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    let unsend = |turn, req| ClientMsg::AgentUnsend {
        req: Some(req),
        agent: agent.clone(),
        turn,
    };
    // Sent and taken back before the fake agent's first event: nothing of it stays.
    let events = attach(&mut ws, &agent, 1).await;
    assert!(events.is_empty(), "{events:?}");
    queue(&mut ws, &agent, "hold first 20000 work slowly 30", 2).await;
    let reply = unsend_and_retract(&mut ws, unsend(1, 3), 3, 1).await;
    assert_eq!(reply, ServerMsg::Ok { req: 3 });
    let waited = agent_wait(&mut ws, &pane, 10.0, 4).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
    let events = attach(&mut ws, &agent, 5).await;
    assert!(events.is_empty(), "no turn ran: {events:?}");

    // One running, three queued behind it.
    queue(&mut ws, &agent, "work slowly 30", 6).await;
    loop {
        if let ServerMsg::Agent {
            event: StampedEvent {
                event: AgentEvent::Thought { .. },
                ..
            },
            ..
        } = recv(&mut ws).await
        {
            break;
        }
    }
    for (i, text) in ["one", "two", "three"].iter().enumerate() {
        queue(&mut ws, &agent, text, 10 + i as u64).await;
    }
    // Turns 3, 4 and 5 are queued. Removing the first leaves the other two, in order.
    let reply = ok_or_error(&mut ws, unsend(3, 20), 20).await;
    assert_eq!(reply, ServerMsg::Ok { req: 20 });
    let shown = attach(&mut ws, &agent, 21).await;
    let queued: Vec<u64> = shown
        .iter()
        .filter_map(|e| match e {
            AgentEvent::User { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [2, 4, 5]);
    // The running turn has answered: taking it back is refused.
    let refused = ok_or_error(&mut ws, unsend(2, 22), 22).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if message.contains("no unanswered message")),
        "{refused:?}"
    );
    let clear = ClientMsg::AgentClearQueue {
        req: Some(23),
        agent: agent.clone(),
    };
    assert_eq!(ok_or_error(&mut ws, clear, 23).await, ServerMsg::Ok { req: 23 });
    let shown = attach(&mut ws, &agent, 24).await;
    let users = shown
        .iter()
        .filter(|e| matches!(e, AgentEvent::User { .. }))
        .count();
    assert_eq!(users, 1, "only the running message remains: {shown:?}");
    // Esc stops the running turn only; with nothing running a cancel is an error again.
    let stopped = agent_cancel(&mut ws, &agent, 25).await;
    assert_eq!(stopped, ServerMsg::Ok { req: 25 });
    let waited = agent_wait(&mut ws, &pane, 10.0, 26).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
}

/// Read agent events from `ws` until the turn ends of every turn in `turns` have arrived; returns
/// every event read, in order.
async fn events_until_ended(ws: &mut Ws, turns: &[u64]) -> Vec<AgentEvent> {
    let (mut seen, mut ended) = (Vec::new(), Vec::new());
    while !turns.iter().all(|t| ended.contains(t)) {
        if let ServerMsg::Agent { event, .. } = recv(ws).await {
            if let AgentEvent::TurnEnd { turn, .. } = &event.event {
                ended.push(*turn);
            }
            seen.push(event.event);
        }
    }
    seen
}

/// Wait for the first thought of the turn running, so a message sent next arrives mid-turn.
async fn until_thinking(ws: &mut Ws) {
    loop {
        if let ServerMsg::Agent {
            event: StampedEvent {
                event: AgentEvent::Thought { .. },
                ..
            },
            ..
        } = recv(ws).await
        {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_sent_during_a_turn_goes_to_the_agent_and_joins_that_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    attach(&mut ws, &agent, 1).await;
    queue(&mut ws, &agent, "work slowly 3", 2).await;
    until_thinking(&mut ws).await;
    queue(&mut ws, &agent, "two", 3).await;
    // The agent takes the message in after the step it is on: it starts under its own turn id
    // inside the running turn, and both turns end together.
    let events = events_until_ended(&mut ws, &[1, 2]).await;
    let position = |wanted: &AgentEvent| events.iter().position(|e| e == wanted);
    let start = position(&AgentEvent::TurnStart { turn: 2 }).expect("turn 2 started");
    let reply = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: two")))
        .expect("the agent answered the second message");
    assert!(start < reply, "{events:?}");
    let ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        .count();
    assert_eq!(ends, 2, "{events:?}");
    let waited = agent_wait(&mut ws, &pane, 10.0, 4).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_message_sent_now_interrupts_the_turn_and_runs_first() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    let send_now = |turn, req| ClientMsg::AgentSendNow {
        req: Some(req),
        agent: agent.clone(),
        turn,
    };
    attach(&mut ws, &agent, 1).await;
    // Nothing is queued: refused.
    let refused = ok_or_error(&mut ws, send_now(1, 2), 2).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if message.contains("no queued message")),
        "{refused:?}"
    );
    queue(&mut ws, &agent, "work slowly 3000", 3).await;
    until_thinking(&mut ws).await;
    queue(&mut ws, &agent, "two", 4).await;
    assert_eq!(ok_or_error(&mut ws, send_now(2, 5), 5).await, ServerMsg::Ok { req: 5 });
    let events = events_until_ended(&mut ws, &[1, 2]).await;
    let stopped = events.iter().any(
        |e| matches!(e, AgentEvent::TurnEnd { turn: 1, stop_reason } if stop_reason == "cancelled"),
    );
    assert!(stopped, "turn 1 was interrupted: {events:?}");
    let reply = events
        .iter()
        .any(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: two")));
    assert!(reply, "turn 2 ran: {events:?}");
    let waited = agent_wait(&mut ws, &pane, 10.0, 6).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
}

/// Wait for turn `turn` to start, so a message sent next arrives mid-turn.
async fn until_turn_start(ws: &mut Ws, turn: u64) {
    loop {
        if let ServerMsg::Agent {
            event: StampedEvent {
                event: AgentEvent::TurnStart { turn: t },
                ..
            },
            ..
        } = recv(ws).await
        {
            if t == turn {
                break;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_sent_during_a_pi_turn_joins_it_and_can_be_sent_now() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = pi_agent(&url).await;
    attach(&mut ws, &agent, 1).await;
    // Steered into the running turn: pi takes it in after its tool call and both end together.
    queue(&mut ws, &agent, "work slowly 3", 2).await;
    until_turn_start(&mut ws, 1).await;
    queue(&mut ws, &agent, "two", 3).await;
    let events = events_until_ended(&mut ws, &[1, 2]).await;
    let start = events.iter().position(|e| *e == AgentEvent::TurnStart { turn: 2 });
    let echo = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: two")));
    assert!(start.is_some() && echo.is_some() && start < echo, "{events:?}");
    // Sent now: pi's queue is emptied, the turn aborted, and the message runs as a turn of its own.
    queue(&mut ws, &agent, "work slowly 30", 4).await;
    until_turn_start(&mut ws, 3).await;
    queue(&mut ws, &agent, "three", 5).await;
    let now = ClientMsg::AgentSendNow {
        req: Some(6),
        agent: agent.clone(),
        turn: 4,
    };
    assert_eq!(ok_or_error(&mut ws, now, 6).await, ServerMsg::Ok { req: 6 });
    let events = events_until_ended(&mut ws, &[3, 4]).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::TurnEnd { turn: 3, stop_reason } if stop_reason == "cancelled")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: three"))),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_message_waits_in_the_queue_until_sent_now_and_is_then_steered_into_the_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_agent_as(&url, &pane, None, Some("codex")).await;
    attach(&mut ws, &agent, 1).await;
    queue(&mut ws, &agent, "work slowly", 2).await;
    until_turn_start(&mut ws, 1).await;
    queue(&mut ws, &agent, "two", 3).await;
    // Codex cannot give a steered message back, so this one is not handed over yet: it stays queued
    // where Remove and Edit work.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let shown = attach(&mut ws, &agent, 4).await;
    assert!(
        !shown.contains(&AgentEvent::TurnStart { turn: 2 }),
        "turn 2 waits: {shown:?}"
    );
    let now = ClientMsg::AgentSendNow {
        req: Some(5),
        agent: agent.clone(),
        turn: 2,
    };
    assert_eq!(ok_or_error(&mut ws, now, 5).await, ServerMsg::Ok { req: 5 });
    let events = events_until_ended(&mut ws, &[1, 2]).await;
    assert!(events.contains(&AgentEvent::TurnStart { turn: 2 }), "{events:?}");
    let said: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(said.contains("echo: two"), "turn 1 took the message in: {said:?}");
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::TurnEnd { stop_reason, .. } if stop_reason == "cancelled")),
        "nothing was interrupted: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn send_now_interrupts_a_codex_turn_that_waits_on_a_permission() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach(&mut ws, &agent, 1).await;
    queue(&mut ws, &agent, "permission", 2).await;
    perm_from(&mut ws, &agent).await;
    // The turn is stopped on the request, so a message steered into it would never be read.
    queue(&mut ws, &agent, "two", 3).await;
    let now = ClientMsg::AgentSendNow {
        req: Some(4),
        agent: agent.clone(),
        turn: 2,
    };
    assert_eq!(ok_or_error(&mut ws, now, 4).await, ServerMsg::Ok { req: 4 });
    let events = events_until_ended(&mut ws, &[1, 2]).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::TurnEnd { turn: 1, stop_reason } if stop_reason == "cancelled")),
        "{events:?}"
    );
    let said: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(said.contains("echo: two"), "{said:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_on_an_agent_pane_reports_its_state_by_pane_id_or_agent_name() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    queue(&mut ws, &pane, "needs permission", 1).await;
    let perm = perm_from(&mut ws, &agent).await;
    // Busy on a turn that waits for the person: the wait runs out and says what state it is in.
    let waited = agent_wait(&mut ws, &pane, 0.3, 2).await;
    assert!(
        matches!(
            &waited,
            ServerMsg::AgentWaited {
                ready: false,
                state: AgentState::Busy { .. },
                ..
            }
        ),
        "{waited:?}"
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
    for (target, req) in [(&agent, 3), (&pane, 4)] {
        let waited = agent_wait(&mut ws, target, 10.0, req).await;
        let ServerMsg::AgentWaited {
            agent: s,
            pane: p,
            state,
            ready,
            ..
        } = waited
        else {
            panic!("{waited:?}")
        };
        assert_eq!(
            (s.as_str(), p.as_deref(), state, ready),
            (
                agent.as_str(),
                Some(pane.as_str()),
                AgentState::Ready,
                true
            )
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_an_agent_panes_turn_and_an_idle_cancel_is_an_error() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    let refused = agent_cancel(&mut ws, &pane, 1).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if *message == format!("agent {agent} has no running turn to cancel")),
        "{refused:?}"
    );
    queue(&mut ws, &agent, "needs permission", 2).await;
    perm_from(&mut ws, &agent).await;
    let cancelled = agent_cancel(&mut ws, &agent, 3).await;
    assert_eq!(cancelled, ServerMsg::Ok { req: 3 });
    let waited = agent_wait(&mut ws, &pane, 10.0, 4).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
}

/// A transcript that is not in the store can never load, so the pane takes the new conversation's
/// id instead of trying the dead one on every start.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_whose_saved_conversation_is_not_in_the_store_saves_the_new_one_and_says_so() {
    let dir = Scratch::new("agent-restart-lost");
    // The agents keep their conversations in the instance's Claude config directory.
    let store = dir.0.join("agent-home").join("projects");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let state_file = dir.0.join("spaces.json");
    let (url, agent) = restart_with_an_agent(&command, &dir.0, || {
        std::fs::remove_dir_all(&store).unwrap();
    })
    .await;
    let saved = saved_conversation(&state_file).expect("the pane saved its conversation");
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    assert_eq!(events, [], "the notice is never a transcript line");
    // The agent still works, in a new conversation, and the pane saves that one.
    assert_eq!(reply(&mut ws, &agent, "fresh").await, "echo: fresh");
    let mut now = saved_conversation(&state_file);
    for _ in 0..50 {
        if now.as_ref() != Some(&saved) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        now = saved_conversation(&state_file);
    }
    assert!(
        now.as_ref().is_some_and(|id| id != &saved),
        "the pane still holds the dead id {saved}: {now:?}"
    );
}

/// The data-loss guard: a transcript that is there but cannot be read may read next time, so a
/// load that fails on it must not overwrite the saved id.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_whose_saved_conversation_will_not_read_keeps_the_saved_id_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    fn transcripts(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                transcripts(&path, found);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                found.push(path);
            }
        }
    }
    let dir = Scratch::new("agent-restart-unreadable");
    let store = dir.0.join("agent-home").join("projects");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let state_file = dir.0.join("spaces.json");
    let (url, agent) = restart_with_an_agent(&command, &dir.0, || {
        let mut found = Vec::new();
        transcripts(&store, &mut found);
        assert!(!found.is_empty(), "the agent wrote no transcript");
        for path in found {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
    })
    .await;
    let saved = saved_conversation(&state_file).expect("the pane saved its conversation");
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    assert_eq!(events, [], "the notice is never a transcript line");
    // The agent still works, in a new conversation, and the saved id stays untouched.
    assert_eq!(reply(&mut ws, &agent, "fresh").await, "echo: fresh");
    assert_eq!(saved_conversation(&state_file), Some(saved));
}

/// The notice is a status the person takes down: every client sees it go, and a client attaching
/// afterwards is not shown it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_dismissed_notice_comes_down_for_every_client_and_stays_down() {
    let dir = Scratch::new("agent-notice-dismiss");
    // The agents keep their conversations in the instance's Claude config directory.
    let store = dir.0.join("agent-home").join("projects");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let (url, agent) = restart_with_an_agent(&command, &dir.0, || {
        std::fs::remove_dir_all(&store).unwrap();
    })
    .await;
    let mut ws = connect(&url).await;
    let (notice, _) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    let mut watcher = connect(&url).await;
    send(
        &mut ws,
        ClientMsg::AgentNoticeDismiss {
            req: Some(710),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentNotice {
            agent,
            notice: None,
        } = recv(&mut watcher).await
        {
            assert_eq!(agent, agent);
            break;
        }
    }
    attach(&mut ws, &agent, 711).await;
    let again = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let ServerMsg::AgentNotice {
                notice: Some(notice),
                ..
            } = recv(&mut ws).await
            {
                return notice;
            }
        }
    })
    .await;
    assert!(again.is_err(), "{again:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_set_while_the_agent_is_starting_is_refused_at_once() {
    let mut timing = quick();
    timing.start_wait = Duration::from_millis(200);
    let (url, _, serving) = start_daemon_timed(&never_ready_agent(), None, timing).await;
    let (_, pane) = space(&url, "/tmp").await;
    let reply = pane_start_reply(&url, &pane, "first task").await;
    let ServerMsg::Started {
        agent: Some(agent),
        ..
    } = reply
    else {
        panic!("{reply:?}")
    };
    let mut ws = connect(&url).await;
    // The agent never leaves `Starting`, so a reply at all means the refusal did not wait for it.
    config_set_refused(&mut ws, &agent, 1, "model", "sonnet", "is still starting").await;
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switch_in_an_agent_pane_saves_the_new_conversation_in_the_pane() {
    let dir = Scratch::new("agent-agents");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let state_file = dir.0.join("spaces.json");
    let (url, _, _) = start_daemon_with(&command, Some(state_file.clone())).await;
    let (_, agent) = agent_pane(&url, Some("first task")).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 899).await;
    let pane_conversation = || saved_conversation(&state_file);
    let first = pane_conversation().expect("the pane saved its conversation");
    let listed = conversations_of(&mut ws, &agent, 900).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first);
    assert!(listed[0].current, "{listed:?}");

    let second = switch_of(&mut ws, &agent, 901, None)
        .await
        .expect("a new conversation");
    assert_ne!(second, first);
    assert_eq!(pane_conversation(), None);
    assert_eq!(
        reply(&mut ws, &agent, "second task").await,
        "echo: second task"
    );
    assert_eq!(pane_conversation(), Some(second));

    let back = switch_of(&mut ws, &agent, 902, Some(&first))
        .await
        .expect("the first conversation again");
    assert_eq!(back, first);
    assert_eq!(pane_conversation(), Some(first));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attach_carries_the_daemons_clock() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    let before = wheel_proto::unix_ms_now();
    send(
        &mut ws,
        ClientMsg::AgentAttach {
            req: Some(1),
            agent: agent.clone(),
        },
    )
    .await;
    let sent = loop {
        if let ServerMsg::AgentAttached {
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
    // Marked by an argument of its own, so only this daemon's agents count; the other tests'
    // daemons in this process run theirs concurrently.
    let dir = Scratch::new("abort-agents");
    let marker = dir.0.display().to_string();
    let command = format!(
        "{} --marker {marker}",
        env!("CARGO_BIN_EXE_wheel-fake-agent")
    );
    let (url, _, daemon) = start_daemon_with(&command, None).await;
    let ws = connect(&url).await;
    ready_agent(&url).await;
    let mine = || {
        let out = std::process::Command::new("pgrep")
            .args(["-f", &marker])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().count()
    };
    assert!(mine() >= 1, "the agent is running");
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

/// What `agent_attached` says about `agent`'s context use.
async fn attached_context(ws: &mut Ws, agent: &str, req: u64) -> Option<wheel_proto::ContextUse> {
    send(
        ws,
        ClientMsg::AgentAttach {
            req: Some(req),
            agent: agent.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentAttached {
            req: Some(r),
            context,
            ..
        } = recv(ws).await
        {
            if r == req {
                return context;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_panes_context_use_follows_its_turns_and_survives_a_restart() {
    let dir = Scratch::new("context");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let (url, agent) = restart_with_an_agent(&command, &dir.0, || {}).await;

    // The first turn reported 10,000 of 200,000 tokens; the pane saved it, and the restarted
    // daemon shows it before the agent has spoken.
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.0.join("spaces.json")).unwrap()).unwrap();
    assert_eq!(
        saved["spaces"][0]["root"]["context"],
        serde_json::json!({ "used": 10_000, "size": 200_000 })
    );
    let mut ws = connect(&url).await;
    let restored = wheel_proto::ContextUse {
        used: 10_000,
        size: 200_000,
    };
    assert_eq!(
        attached_context(&mut ws, &agent, 500).await,
        Some(restored)
    );

    // A compaction shrinks it, and every client hears so.
    let text = reply(&mut ws, &agent, "context compacted").await;
    assert_eq!(text, "echo: context compacted");
    let compacted = wheel_proto::ContextUse {
        used: 8_000,
        size: 200_000,
    };
    assert_eq!(compacted.percent(), 4);
    assert_eq!(
        attached_context(&mut ws, &agent, 501).await,
        Some(compacted)
    );

    // A new conversation starts empty.
    send(
        &mut ws,
        ClientMsg::AgentConversationNew {
            req: Some(502),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentContext {
            agent: s,
            context: None,
        } = recv(&mut ws).await
        {
            if s == agent {
                break;
            }
        }
    }
    assert_eq!(attached_context(&mut ws, &agent, 503).await, None);
}

/// Ask which view `pane` is in, switching it to `to` first when given: the view and the
/// conversation it shows, or the daemon's refusal.
/// What a hook of the agent in terminal `pane` reports.
async fn report_status(ws: &mut Ws, req: u64, pane: &str, status: wheel_proto::ReportedStatus) {
    send(
        ws,
        ClientMsg::PaneReport {
            req: Some(req),
            pane: pane.into(),
            status,
            reason: None,
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::Ok { req: r } if r == req => return,
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => panic!("{message}"),
            _ => {}
        }
    }
}

async fn pane_view(
    ws: &mut Ws,
    req: u64,
    pane: &str,
    to: Option<wheel_proto::PaneView>,
) -> Result<(Option<wheel_proto::PaneView>, Option<String>), String> {
    send(
        ws,
        ClientMsg::PaneView {
            req: Some(req),
            pane: pane.into(),
            to,
        },
    )
    .await;
    loop {
        match recv(ws).await {
            ServerMsg::PaneView {
                req: r,
                view,
                conversation,
                ..
            } if r == req => return Ok((view, conversation)),
            ServerMsg::Error {
                req: Some(r),
                message,
                ..
            } if r == req => return Err(message),
            _ => {}
        }
    }
}

/// What `pane` shows: its kind as the layout has it.
async fn pane_kind(ws: &mut Ws, pane: &str) -> wheel_proto::PaneKind {
    send(ws, ClientMsg::Status { req: Some(98) }).await;
    let spaces = loop {
        if let ServerMsg::Status { spaces, .. } = recv(ws).await {
            break spaces;
        }
    };
    spaces
        .iter()
        .flat_map(|s| wheel_proto_panes(&s.root))
        .find(|(id, _)| id == pane)
        .map(|(_, kind)| kind)
        .expect("pane in the layout")
}

fn wheel_proto_panes(node: &wheel_proto::Node) -> Vec<(String, wheel_proto::PaneKind)> {
    match node {
        wheel_proto::Node::Pane { id, kind, .. } => vec![(id.clone(), kind.clone())],
        wheel_proto::Node::Split { a, b, .. } => {
            let mut all = wheel_proto_panes(a);
            all.extend(wheel_proto_panes(b));
            all
        }
    }
}

/// Type `line` into terminal `term`, attaching to it first so its output reaches `ws`.
async fn type_bytes(ws: &mut Ws, term: &str, bytes: &[u8]) {
    send(
        ws,
        ClientMsg::TermInput {
            req: None,
            id: term.into(),
            data: wheel_proto::encode_bytes(bytes),
        },
    )
    .await;
}

async fn type_line(ws: &mut Ws, term: &str, line: &str) {
    send(
        ws,
        ClientMsg::TermInput {
            req: None,
            id: term.into(),
            data: wheel_proto::encode_bytes(format!("{line}\r").as_bytes()),
        },
    )
    .await;
}

async fn term_attach(ws: &mut Ws, term: &str) {
    send(
        ws,
        ClientMsg::TermAttach {
            req: Some(97),
            id: term.into(),
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_shows_a_claude_conversation_as_a_terminal_or_a_chat_one_program_at_a_time() {
    use wheel_proto::{PaneKind, PaneView};
    const ID: &str = "7b07b07b-1111-4222-8333-444455556666";
    let dir = Scratch::new("views");
    // Canonical, as the directory Claude Code reports running in is.
    let root = std::fs::canonicalize(&dir.0).unwrap();
    let work = root.join("work");
    let home = root.join("claude-home");
    let bin = root.join("bin");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // The stand-in for Claude Code in a terminal is the fake agent run as `claude`, keeping its
    // agent where the fake agent pane's store is: the same store, as the real two share one.
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_wheel-fake-agent"), bin.join("claude")).unwrap();
    let project: String = work
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let store = home.join("projects").join(project);
    std::fs::create_dir_all(&store).unwrap();
    let shell = root.join("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nexport PATH={}:$PATH CLAUDE_CONFIG_DIR={} PS1='$ '\nexec /bin/sh -i\n",
            bin.display(),
            home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(
        work.join("wheel.toml"),
        format!("[terminal]\nshell = \"{}\"\n", shell.display()),
    )
    .unwrap();
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent");
    // A daemon with a layout file samples its terminals, as one always does in production.
    let (url, _, _daemon) = start_daemon_with(command, Some(root.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let (_, pane) = space(&url, &work.to_string_lossy()).await;
    client_op(
        &mut ws,
        399,
        ClientMsg::PaneSetKind {
            home: None,
            req: Some(399),
            pane: pane.clone(),
            kind: wheel_proto::NewPaneKind::Terminal,
            artifact: None,
            scope: None,
            url: None,
            plugin: None,
        },
    )
    .await;
    let term_of = |kind: PaneKind| match kind {
        PaneKind::Terminal { term, agent, .. } => (term, agent),
        other => panic!("not a terminal: {other:?}"),
    };
    let (term, _) = term_of(pane_kind(&mut ws, &pane).await);

    // A plain shell has no conversation to show as a chat.
    assert_eq!(
        pane_view(&mut ws, 400, &pane, None).await,
        Ok((Some(PaneView::Terminal), None))
    );
    let refused = pane_view(&mut ws, 401, &pane, Some(PaneView::Agent))
        .await
        .unwrap_err();
    assert!(refused.contains("not running Claude Code"), "{refused}");

    // Claude Code in the terminal takes a message.
    term_attach(&mut ws, &term).await;
    type_line(&mut ws, &term, &format!("claude --session-id {ID}")).await;
    let shown = output_until(&mut ws, &format!("claude-fake {ID} ready"), 30).await;
    assert!(shown.contains("ready"), "{shown:?}");
    type_line(&mut ws, &term, "first from the terminal").await;
    let shown = output_until(&mut ws, "echo: first from the terminal", 10).await;
    assert!(shown.contains("echo: first from"), "{shown:?}");
    // The pane reads back that it shows that conversation once the daemon has seen the agent.
    let mut seen = None;
    for _ in 0..50 {
        seen = pane_view(&mut ws, 402, &pane, None).await.unwrap().1;
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(seen.as_deref(), Some(ID));

    // As a chat: the TUI quits, the agent loads the conversation, and the message is in it.
    assert_eq!(
        pane_view(&mut ws, 403, &pane, Some(PaneView::Agent)).await,
        Ok((Some(PaneView::Agent), Some(ID.into())))
    );
    let PaneKind::Agent { agent, place, .. } = pane_kind(&mut ws, &pane).await else {
        panic!("the pane is an agent pane")
    };
    // The space's own directory is not repeated; the config directory Claude Code used is picked.
    assert_eq!(
        place.map(|p| *p),
        Some(wheel_proto::AgentPlace {
            cwd: None,
            env: [("CLAUDE_CONFIG_DIR".to_string(), home.to_string_lossy().into_owned())].into(),
        })
    );
    let events = transcript_when(&mut ws, &agent, |e| e.len() >= 4).await;
    assert_eq!(
        events[0],
        AgentEvent::User {
            turn: 1,
            text: "first from the terminal".into()
        },
        "{events:?}"
    );
    // While a turn runs the pane stays a chat, and says why.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(407),
            agent: agent.clone(),
            text: "work slowly 1".into(),
        },
    )
    .await;
    let refused = pane_view(&mut ws, 408, &pane, Some(PaneView::Terminal))
        .await
        .unwrap_err();
    assert!(refused.contains("working on a turn"), "{refused}");
    // Its reply carries the slow turn's echo too, which ran first.
    let answered = reply(&mut ws, &agent, "second from the chat").await;
    assert!(
        answered.ends_with("echo: second from the chat"),
        "{answered}"
    );

    // The chat's model and mode are the person's choice, not the agent's defaults.
    for (req, config, value) in [(409, "model", "sonnet"), (410, "mode", "plan")] {
        client_op(
            &mut ws,
            req,
            ClientMsg::AgentConfigSet {
                req: Some(req),
                agent: agent.clone(),
                config: config.into(),
                value: value.into(),
            },
        )
        .await;
    }

    // Back in a terminal: Claude Code resumes the conversation, the chat's message in it.
    assert_eq!(
        pane_view(&mut ws, 404, &pane, Some(PaneView::Terminal)).await,
        Ok((Some(PaneView::Terminal), Some(ID.into())))
    );
    let (term, term_agent) = term_of(pane_kind(&mut ws, &pane).await);
    assert_eq!(term_agent.map(|a| a.conversation).as_deref(), Some(ID));
    term_attach(&mut ws, &term).await;
    let shown = output_until(&mut ws, &format!("claude-fake {ID} ready"), 30).await;
    assert!(
        shown.contains("> second from the chat") && shown.contains("> first from the terminal"),
        "{shown:?}"
    );
    // The agent the chat ran in is gone with it.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(405),
            agent: agent.clone(),
            text: "anyone there".into(),
        },
    )
    .await;
    let ended = loop {
        if let ServerMsg::Error {
            req: Some(405),
            message,
            ..
        } = recv(&mut ws).await
        {
            break message;
        }
    };
    assert!(ended.contains(&agent), "{ended}");

    // Back to the chat: the new agent view has the model and mode the pane had before.
    let mut seen = None;
    for _ in 0..50 {
        seen = pane_view(&mut ws, 411, &pane, None).await.unwrap().1;
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(seen.as_deref(), Some(ID));
    assert_eq!(
        pane_view(&mut ws, 412, &pane, Some(PaneView::Agent)).await,
        Ok((Some(PaneView::Agent), Some(ID.into())))
    );
    let PaneKind::Agent { agent: again, .. } = pane_kind(&mut ws, &pane).await else {
        panic!("the pane is an agent pane")
    };
    let mut options = config_of(&mut ws, &again, 413).await;
    for _ in 0..50 {
        if !options.is_empty()
            && current(&options, "model") == "sonnet"
            && current(&options, "mode") == "plan"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        options = config_of(&mut ws, &again, 414).await;
    }
    assert_eq!(current(&options, "model"), "sonnet", "{options:?}");
    assert_eq!(current(&options, "mode"), "plan", "{options:?}");
    let term = match pane_view(&mut ws, 415, &pane, Some(PaneView::Terminal)).await {
        Ok(_) => term_of(pane_kind(&mut ws, &pane).await).0,
        Err(e) => panic!("{e}"),
    };
    term_attach(&mut ws, &term).await;
    output_until(&mut ws, &format!("claude-fake {ID} ready"), 30).await;

    // Quitting Claude Code leaves a plain terminal with no conversation, however briefly it ran.
    type_line(&mut ws, &term, "/exit").await;
    let mut now = pane_view(&mut ws, 406, &pane, None).await;
    for _ in 0..50 {
        if now == Ok((Some(PaneView::Terminal), None)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        now = pane_view(&mut ws, 406, &pane, None).await;
    }
    assert_eq!(now, Ok((Some(PaneView::Terminal), None)));
    let (still, _) = term_of(pane_kind(&mut ws, &pane).await);
    assert_eq!(still, term, "the same shell carries on");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_shows_a_codex_conversation_as_a_terminal_or_a_chat_one_program_at_a_time() {
    use wheel_proto::{PaneKind, PaneView};
    let dir = Scratch::new("codex-views");
    // Canonical, as the directory Codex reports running in is.
    let root = std::fs::canonicalize(&dir.0).unwrap();
    let work = root.join("work");
    let home = root.join("codex-home");
    let bin = root.join("bin");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // The stand-in for the Codex TUI is the fake agent run as `codex`, over the rollouts the fake
    // `codex app-server` keeps: the same store, as the real two share one.
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_wheel-fake-agent"), bin.join("codex")).unwrap();
    let shell = root.join("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nexport PATH={}:$PATH CODEX_HOME={} PS1='$ '\nexec /bin/sh -i\n",
            bin.display(),
            home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(
        work.join("wheel.toml"),
        format!("[terminal]\nshell = \"{}\"\n", shell.display()),
    )
    .unwrap();
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, _daemon) = start_daemon_with(command, Some(root.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let (_, pane) = space(&url, &work.to_string_lossy()).await;
    client_op(
        &mut ws,
        499,
        ClientMsg::PaneSetKind {
            home: None,
            req: Some(499),
            pane: pane.clone(),
            kind: wheel_proto::NewPaneKind::Terminal,
            artifact: None,
            scope: None,
            url: None,
            plugin: None,
        },
    )
    .await;
    let term_of = |kind: PaneKind| match kind {
        PaneKind::Terminal { term, agent, .. } => (term, agent),
        other => panic!("not a terminal: {other:?}"),
    };
    let (term, _) = term_of(pane_kind(&mut ws, &pane).await);

    // Codex in the terminal takes a message.
    term_attach(&mut ws, &term).await;
    type_line(&mut ws, &term, "codex").await;
    output_until(&mut ws, " ready", 30).await;
    type_line(&mut ws, &term, "first from the terminal").await;
    output_until(&mut ws, "echo: first from the terminal", 10).await;
    // The pane reads back the thread once the daemon has seen the agent.
    let mut seen = None;
    for _ in 0..50 {
        seen = pane_view(&mut ws, 500, &pane, None).await.unwrap().1;
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let thread = seen.expect("the pane shows Codex's thread");

    // The pane switches by what Codex's hooks reported. Nothing reported: allowed (case 6, driven
    // by the first switch in the Claude test); here a working turn and a waiting prompt refuse.
    for (req, status, why) in [
        (
            501,
            wheel_proto::ReportedStatus::Working,
            "in the middle of a turn",
        ),
        (503, wheel_proto::ReportedStatus::NeedsYou, "waiting on you"),
    ] {
        report_status(&mut ws, req, &pane, status).await;
        let refused = pane_view(&mut ws, req + 1, &pane, Some(PaneView::Agent))
            .await
            .unwrap_err();
        assert!(refused.contains(why), "{status:?}: {refused}");
        assert!(
            matches!(pane_kind(&mut ws, &pane).await, PaneKind::Terminal { .. }),
            "a refused switch leaves the terminal"
        );
    }
    type_line(&mut ws, &term, "work slowly").await;
    output_until(&mut ws, "echo: work slowly", 10).await;
    // Esc alone while it works is an interrupt: idle at once, so the switch below is allowed.
    report_status(&mut ws, 505, &pane, wheel_proto::ReportedStatus::Working).await;
    type_bytes(&mut ws, &term, b"\x1b").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // As a chat: Codex quits, the app-server loads the thread, and both exchanges are in it.
    assert_eq!(
        pane_view(&mut ws, 502, &pane, Some(PaneView::Agent)).await,
        Ok((Some(PaneView::Agent), Some(thread.clone())))
    );
    let PaneKind::Agent { agent, harness, .. } = pane_kind(&mut ws, &pane).await else {
        panic!("the pane is an agent pane")
    };
    assert_eq!(harness, "codex");
    let events = transcript_when(&mut ws, &agent, |e| e.len() >= 8).await;
    assert_eq!(
        events[0],
        AgentEvent::User {
            turn: 1,
            text: "first from the terminal".into()
        },
        "{events:?}"
    );
    assert!(
        events.contains(&AgentEvent::Text {
            text: "echo: work slowly".into()
        }),
        "{events:?}"
    );
    // While a turn runs the pane stays a chat, and says why.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(507),
            agent: agent.clone(),
            text: "work slowly".into(),
        },
    )
    .await;
    let refused = pane_view(&mut ws, 508, &pane, Some(PaneView::Terminal))
        .await
        .unwrap_err();
    assert!(refused.contains("working on a turn"), "{refused}");
    send(
        &mut ws,
        ClientMsg::AgentCancel {
            req: Some(509),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::Agent { event, .. } = recv(&mut ws).await {
            if matches!(event.event, AgentEvent::TurnEnd { .. }) {
                break;
            }
        }
    }
    let answered = reply(&mut ws, &agent, "second from the chat").await;
    assert!(
        answered.ends_with("echo: second from the chat"),
        "{answered}"
    );

    codex_config_ok(&mut ws, &agent, 512, "reviewer", "user").await;
    // Back in a terminal: `codex resume` shows the thread, the chat's message in it.
    assert_eq!(
        pane_view(&mut ws, 504, &pane, Some(PaneView::Terminal)).await,
        Ok((Some(PaneView::Terminal), Some(thread.clone())))
    );
    let (term, agent) = term_of(pane_kind(&mut ws, &pane).await);
    let agent = agent.expect("the terminal runs Codex");
    assert_eq!(
        (agent.name.as_str(), agent.conversation.as_str()),
        ("codex", thread.as_str())
    );
    term_attach(&mut ws, &term).await;
    let shown = output_until(&mut ws, &format!("codex-fake {thread} ready"), 30).await;
    assert!(
        shown.contains("> second from the chat") && shown.contains("> first from the terminal"),
        "{shown:?}"
    );
    // Model a native TUI permission change in its own store. Returning to chat must not replay
    // the old chat's reviewer/policy/sandbox over the destination's persisted settings.
    edit_codex_permissions(&home, &thread, |settings| {
        settings["approvalsReviewer"] = serde_json::json!("auto_review");
        settings["approvalPolicy"] = serde_json::json!("never");
        settings["sandbox"] = serde_json::json!({"type": "readOnly", "networkAccess": false});
    });
    pane_view(&mut ws, 513, &pane, Some(PaneView::Agent)).await.unwrap();
    let PaneKind::Agent { agent, .. } = pane_kind(&mut ws, &pane).await else { panic!("agent pane") };
    transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    let options = config_of(&mut ws, &agent, 514).await;
    assert_eq!(current(&options, "reviewer"), "auto_review");
    assert_eq!(current(&options, "mode"), "never");
    assert_eq!(current(&options, "sandbox"), "read-only");
}

/// A daemon that cannot write its shell integration says so in `status`, and one that wrote it
/// with no `wheel` beside it says that; the person sees both in Settings.
#[tokio::test]
async fn status_reports_shell_integration_problem() {
    let dir = std::env::temp_dir().join(format!("wheel-shell-problem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let blocker = dir.join("file");
    std::fs::write(&blocker, "x").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The instance directory sits under a regular file, so no integration file can be written.
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        blocker.join("instance"),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    state.set_control(format!("ws://{addr}/ws"), dir.join("token"), None);
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    let daemon = common::Daemon::new(serving, state);
    let mut ws = connect(&format!("ws://{addr}/ws?token={TOKEN}")).await;
    send(&mut ws, ClientMsg::Status { req: Some(9200) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(9200),
            shell_integration,
            ..
        } = recv(&mut ws).await
        {
            let problem = shell_integration.expect("a problem is reported");
            assert!(
                problem.starts_with("cannot write the shell integration"),
                "{problem}"
            );
            break;
        }
    }
    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// An artifact request in the default folder: `ends_with_pane` and `expires_in` are its lifetime.
fn lived(name: &str, ends_with_pane: Option<&str>, expires_in: Option<u64>) -> ClientMsg {
    ClientMsg::ArtifactNew {
        renderer: None,
        files: vec![],
        reply_to: None,
        webhook: None,
        description: None,
        url: None,
        req: Some(70),
        name: name.into(),
        scope: Some(wheel_proto::ArtifactScope::Project),
        space: None,
        path: None,
        ends_with_pane: ends_with_pane.map(String::from),
        expires_in,
    }
}

async fn artifact_reply(ws: &mut Ws, msg: ClientMsg) -> Result<wheel_proto::ArtifactInfo, String> {
    // The daemon has no active space to default to: aim the request at the first space, unless it
    // names one.
    send(ws, ClientMsg::Status { req: Some(9100) }).await;
    let first = loop {
        if let ServerMsg::Status { req: Some(9100), spaces, .. } = recv(ws).await {
            break spaces.first().map(|s| s.id.clone());
        }
    };
    let mut value = serde_json::to_value(&msg).unwrap();
    if let (Some(first), true) = (first, value.get("space").is_none_or(|s| s.is_null())) {
        value["space"] = first.into();
    }
    send(ws, serde_json::from_value(value).unwrap()).await;
    loop {
        match recv(ws).await {
            ServerMsg::Artifact { artifact, .. } => return Ok(artifact),
            ServerMsg::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
}

#[tokio::test]
async fn an_artifact_lives_until_its_creator_ends_it_and_a_plain_one_never_ends() {
    let (url, _, daemon) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    let dir = std::env::temp_dir().join(format!("wheel-lifetime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_, pane) = space(&url, &dir.to_string_lossy()).await;
    start_agent(&url, &pane, None).await;
    let mut ws = connect(&url).await;
    let folder = |name: &str| dir.join(".wheel/artifacts").join(name);

    let agent = artifact_reply(&mut ws, lived("agent", Some(&pane), None))
        .await
        .unwrap();
    assert_eq!(agent.ends_with_pane.as_deref(), Some(pane.as_str()));
    let timed = artifact_reply(&mut ws, lived("timed", None, Some(3600)))
        .await
        .unwrap();
    let expires_at = timed.expires_at.expect("an expiry");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        (now + 3590..=now + 3610).contains(&expires_at),
        "{expires_at} vs {now}"
    );
    let soon = artifact_reply(&mut ws, lived("soon", None, Some(3)))
        .await
        .unwrap();
    assert!(soon.expires_at.is_some());
    let plain = artifact_reply(&mut ws, lived("plain", None, None))
        .await
        .unwrap();
    assert_eq!((plain.ends_with_pane, plain.expires_at), (None, None));

    // A lifetime is refused where it could never be carried out.
    let mut elsewhere = lived("away", Some(&pane), None);
    if let ClientMsg::ArtifactNew { path, .. } = &mut elsewhere {
        *path = Some(dir.join("away").display().to_string());
    }
    assert!(artifact_reply(&mut ws, elsewhere)
        .await
        .unwrap_err()
        .contains("--path"));
    assert!(artifact_reply(&mut ws, lived("ghost", Some("p999"), None))
        .await
        .unwrap_err()
        .contains("not an agent pane"));
    assert!(!folder("away").exists() && !folder("ghost").exists());

    // Nothing is out of time yet, so a sweep deletes nothing.
    daemon.sweep_artifacts();
    for name in ["agent", "timed", "soon", "plain"] {
        assert!(folder(name).is_dir(), "{name}");
    }

    // After its seconds, the expiring one goes and the rest stay.
    tokio::time::sleep(Duration::from_millis(4100)).await;
    daemon.sweep_artifacts();
    assert!(!folder("soon").exists());
    let entries = std::fs::read_to_string(dir.join("wheel.toml")).unwrap();
    assert!(!entries.contains("[artifacts.soon]"), "{entries}");
    for name in ["agent", "timed", "plain"] {
        assert!(folder(name).is_dir(), "{name}");
    }

    // Closing the pane ends the agent, and with it the artifact that asked to.
    client_op(
        &mut ws,
        71,
        ClientMsg::PaneClose {
            req: Some(71),
            pane,
        },
    )
    .await;
    for _ in 0..100 {
        if !folder("agent").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!folder("agent").exists());
    let entries = std::fs::read_to_string(dir.join("wheel.toml")).unwrap();
    assert!(!entries.contains("[artifacts.agent]"), "{entries}");
    for name in ["timed", "plain"] {
        assert!(folder(name).is_dir(), "{name}");
    }
    daemon.abort();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn an_expiring_artifact_in_a_closed_space_is_swept_after_a_restart() {
    let dir = std::env::temp_dir().join(format!("wheel-lifetime-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state_file = dir.join("instance").join("spaces.json");
    std::fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    let folder = project.join(".wheel/artifacts/short");

    let (url, _, first) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(state_file.clone()),
    )
    .await;
    let (space_id, _) = space(&url, &project.to_string_lossy()).await;
    let mut ws = connect(&url).await;
    artifact_reply(&mut ws, lived("short", None, Some(1)))
        .await
        .unwrap();
    assert!(folder.is_dir());
    client_op(
        &mut ws,
        72,
        ClientMsg::SpaceClose {
            req: Some(72),
            id: space_id,
            if_empty: false,
        },
    )
    .await;
    first.abort();

    // The restarted daemon has no space for the project, so only what the first one wrote down
    // tells it where to look.
    let (_, _, second) =
        start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), Some(state_file)).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    second.sweep_artifacts();
    assert!(!folder.exists(), "{}", folder.display());
    let listed = std::fs::read_to_string(project.join("wheel.toml")).unwrap_or_default();
    assert!(!listed.contains("[artifacts.short]"), "{listed}");
    second.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The agents `status` lists, as JSON.
async fn status_agents(ws: &mut Ws, req: u64) -> serde_json::Value {
    send(ws, ClientMsg::Status { req: Some(req) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(r),
            agents,
            ..
        } = recv(ws).await
        {
            if r == req {
                return serde_json::to_value(agents).unwrap();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_lists_each_agent_and_a_missing_program_refuses_the_start_naming_it() {
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, None).await;
    let (_, pane) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;

    let agents = status_agents(&mut ws, 9300).await;
    let by_name = |name: &str| {
        agents
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed: {agents}"))
            .clone()
    };
    let claude = by_name("claude");
    assert_eq!(claude["command"], fake);
    assert_eq!(claude["found"], true);
    assert_eq!(claude["path"], fake);
    let codex = by_name("codex");
    assert_eq!(codex["command"], fake);
    assert_eq!(codex["found"], true);

    // Pointing claude at a program that is not there reports why, and the start names it.
    send(
        &mut ws,
        ClientMsg::SettingSet {
            req: Some(9301),
            key: "agent.claude.command".into(),
            value: "/no/such/claude".into(),
            space: None,
            machine: true,
        },
    )
    .await;
    loop {
        if matches!(
            recv(&mut ws).await,
            ServerMsg::Settings {
                req: Some(9301),
                ..
            }
        ) {
            break;
        }
    }
    let claude = status_agents(&mut ws, 9302)
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "claude")
        .unwrap()
        .clone();
    assert_eq!(claude["found"], false);
    assert!(claude["path"].is_null(), "{claude}");
    assert!(
        claude["why"].as_str().unwrap().contains("/no/such/claude"),
        "{claude}"
    );
    let before = layout(&url).await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9303),
            pane: pane.clone(),
            kind: StartKind::Agent,
            message: None,
            agent: Some("claude".into()),
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    loop {
        if let ServerMsg::Error {
            req: Some(9303),
            message,
            ..
        } = recv(&mut ws).await
        {
            assert!(
                message.contains("agent claude cannot start")
                    && message.contains("/no/such/claude"),
                "{message}"
            );
            break;
        }
    }
    // A name no agent has is refused too, and neither start changed the layout.
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9304),
            pane,
            kind: StartKind::Agent,
            message: None,
            agent: Some("gemini".into()),
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    loop {
        if let ServerMsg::Error {
            req: Some(9304),
            message,
            ..
        } = recv(&mut ws).await
        {
            assert!(message.contains("no agent gemini"), "{message}");
            break;
        }
    }
    assert_eq!(layout(&url).await, before);
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_names_its_agent_and_keeps_it_across_a_restart() {
    let dir = Scratch::new("agent-pane-names-agent");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    let started = client_op(
        &mut ws,
        9310,
        ClientMsg::PaneStart {
            req: Some(9310),
            pane,
            kind: StartKind::Agent,
            message: None,
            agent: Some("claude".into()),
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    assert_eq!(layout(&url).await["spaces"][0]["root"]["harness"], "claude");
    // The saved layout holds it, so a restart runs the pane's own agent again.
    let saved = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap()
    };
    assert_eq!(saved()["spaces"][0]["root"]["harness"], "claude");
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, second) = start_daemon_with(fake, Some(state_file.clone())).await;
    assert_eq!(layout(&url).await["spaces"][0]["root"]["harness"], "claude");
    second.abort();
}

/// A new space with a Codex pane whose agent is ready; returns the agent name.
async fn codex_agent(url: &str) -> String {
    let (_, pane) = space(url, "/tmp").await;
    start_agent_as(url, &pane, None, Some("codex")).await
}

/// `pane_start` for `agent` with a permission mode and sandbox: the agent, or the daemon's
/// refusal.
async fn start_with_permissions(
    url: &str,
    pane: &str,
    agent: &str,
    permission_mode: Option<&str>,
    sandbox: Option<&str>,
) -> Result<String, String> {
    let mut ws = connect(url).await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9400),
            pane: pane.into(),
            kind: StartKind::Agent,
            message: None,
            agent: Some(agent.into()),
            permission_mode: permission_mode.map(String::from),
            sandbox: sandbox.map(String::from),
            model: None,
            dir: None,
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Started {
                req: 9400,
                agent: Some(agent),
                ..
            } => return Ok(agent),
            ServerMsg::Error {
                req: Some(9400),
                message,
                ..
            } => return Err(message),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_pane_starts_in_the_permission_mode_it_was_asked_for() {
    let url = start_daemon().await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_with_permissions(&url, &pane, "claude", Some("acceptEdits"), None)
        .await
        .unwrap();
    let root = layout(&url).await["spaces"][0]["root"].clone();
    assert_eq!(root["permission_mode"], "acceptEdits", "{root}");
    assert!(root.get("sandbox").is_none(), "{root}");

    // The status reports the mode the agent is in now, not the one it started in.
    let mut ws = connect(&url).await;
    client_op(
        &mut ws,
        9401,
        ClientMsg::AgentConfigSet {
            req: Some(9401),
            agent,
            config: "mode".into(),
            value: "plan".into(),
        },
    )
    .await;
    let root = layout(&url).await["spaces"][0]["root"].clone();
    assert_eq!(root["permission_mode"], "plan", "{root}");

    // An invalid mode fails, names the valid ones, and leaves the pane as it was.
    let (_, other) = space(&url, "/tmp").await;
    let before = layout(&url).await;
    let error = start_with_permissions(&url, &other, "claude", Some("yolo"), None)
        .await
        .unwrap_err();
    assert!(
        error.contains("yolo") && error.contains("acceptEdits") && error.contains("plan"),
        "{error}"
    );
    let error = start_with_permissions(&url, &other, "claude", None, Some("read-only"))
        .await
        .unwrap_err();
    assert!(error.contains("no sandbox"), "{error}");
    assert_eq!(layout(&url).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_agent_restarts_in_the_mode_it_was_in() {
    let url = start_daemon().await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_with_permissions(&url, &pane, "claude", Some("acceptEdits"), None)
        .await
        .unwrap();
    let mut ws = connect(&url).await;
    client_op(
        &mut ws,
        9402,
        ClientMsg::AgentConfigSet {
            req: Some(9402),
            agent: agent.clone(),
            config: "mode".into(),
            value: "plan".into(),
        },
    )
    .await;
    attach_and_send(&mut ws, &agent, "die now").await;
    loop {
        if let ServerMsg::Agent {
            event:
                StampedEvent {
                    event: AgentEvent::Error { turn: None, .. },
                    ..
                },
            ..
        } = recv(&mut ws).await
        {
            break;
        }
    }
    let waited = agent_wait(&mut ws, &agent, 30.0, 9403).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );
    let root = layout(&url).await["spaces"][0]["root"].clone();
    assert_eq!(root["permission_mode"], "plan", "{root}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_pane_starts_with_its_approval_policy_and_sandbox() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_with_permissions(
        &url,
        &pane,
        "codex",
        Some("on-request"),
        Some("workspace-write"),
    )
    .await
    .unwrap();
    attach(&mut ws, &agent, 40).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("policy=on-request"), "{text:?}");
    assert!(text.contains("sandbox: workspace-write"), "{text:?}");
    let root = layout(&url).await["spaces"][0]["root"].clone();
    assert_eq!(root["permission_mode"], "on-request", "{root}");
    assert_eq!(root["sandbox"], "workspace-write", "{root}");

    let (_, other) = space(&url, "/tmp").await;
    let error = start_with_permissions(&url, &other, "codex", Some("plan"), None)
        .await
        .unwrap_err();
    assert!(
        error.contains("on-request") && error.contains("never"),
        "{error}"
    );
    let error = start_with_permissions(&url, &other, "codex", None, Some("everything"))
        .await
        .unwrap_err();
    assert!(error.contains("danger-full-access"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_status_table_scans_agent_skills() {
    let (_, _, daemon) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    let state = daemon.state();
    assert!(state.agent_table().iter().all(|a| a.skills.is_empty()));
    let claude = state
        .agent_status(&[])
        .into_iter()
        .find(|a| a.name == "claude")
        .unwrap();
    assert!(claude.skills.iter().any(|s| s.name == "clear"));
}

#[tokio::test(flavor = "multi_thread")]
async fn each_agent_runs_with_its_own_home_and_an_unset_one_is_no_override() {
    let (url, _, daemon) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    daemon
        .state()
        .setting_unset("agent.codex.home", None, true)
        .unwrap();
    let homes: Vec<_> = daemon
        .state()
        .agent_table()
        .into_iter()
        .map(|a| {
            (
                a.name,
                a.home.map(|h| h.rsplit('/').next().unwrap().to_string()),
            )
        })
        .collect();
    assert_eq!(
        homes,
        [
            ("claude".to_string(), Some("agent-home".to_string())),
            ("codex".to_string(), None),
            ("pi".to_string(), Some("pi-home".to_string())),
        ]
    );
    let mut ws = connect(&url).await;
    let codex = codex_agent(&url).await;
    attach(&mut ws, &codex, 40).await;
    let text = reply(&mut ws, &codex, "show setup").await;
    assert!(
        text.contains("codex_home=- "),
        "no CODEX_HOME override: {text:?}"
    );
    let (_, pane) = space(&url, "/tmp").await;
    let claude = start_agent_as(&url, &pane, None, Some("claude")).await;
    attach(&mut ws, &claude, 40).await;
    let text = reply(&mut ws, &claude, "show setup").await;
    assert!(
        text.contains("config_dir=") && text.contains("/agent-home"),
        "{text:?}"
    );
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_pane_streams_a_turn_with_its_home_and_the_brief() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    assert_eq!(layout(&url).await["spaces"][0]["root"]["harness"], "codex");
    attach(&mut ws, &agent, 40).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("echo: show setup"), "{text:?}");
    assert!(
        text.contains("codex_home=") && text.contains("/codex-home"),
        "CODEX_HOME is the Codex home, not Claude's: {text:?}"
    );
    assert!(text.contains("policy=on-request"), "{text:?}");
    let brief = text.split("brief: ").nth(1).unwrap_or("-");
    assert!(
        !brief.starts_with('-') && !brief.is_empty(),
        "the brief went in: {text:?}"
    );
    // The window it reported fills the pane's context use.
    let status = layout(&url).await;
    assert_eq!(
        status["spaces"][0]["root"]["context"],
        serde_json::json!({"used": 10_000, "size": 200_000}),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_thought_streams_into_the_transcript() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach(&mut ws, &agent, 49).await;
    reply(&mut ws, &agent, "reflect").await;
    let events = attach(&mut ws, &agent, 50).await;
    let thought: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Thought { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thought, "pondering the question");
}

/// A Claude Code request with no suggestion still offers Always allow, with a rule Wheel builds for
/// the command's first word; the agent's reason, decision reason and blocked path reach the card.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_request_carries_why_and_offers_always_allow_without_a_suggestion() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "permission described nosuggest twice").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.title, "rm -rf build");
    let offered: Vec<&str> = perm.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(offered, ["allow", "allow_always", "deny"]);
    assert_eq!(
        perm.why.reason.as_deref(),
        Some("Remove the build output so the next build starts clean")
    );
    assert_eq!(
        perm.why.decision_reason.as_deref(),
        Some("Bash commands need approval in this mode")
    );
    assert_eq!(perm.why.blocked_path.as_deref(), Some("/work/build"));
    assert_eq!(perm.why.always_rule.as_deref(), Some("Bash(rm:*)"));
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(70),
            id: perm.id,
            choice: "allow_always".into(),
        },
    )
    .await;
    // The rule covers the turn's second `rm`, which is never asked.
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: allow always"), "{text:?}");
    assert!(text.contains("permission again: skipped"), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_command_approval_shows_a_card_and_the_answer_resolves_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach_and_send(&mut ws, &agent, "permission").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.agent, agent);
    assert_eq!(perm.title, "rm -rf build");
    assert_eq!(perm.tool_call.as_deref(), Some("call-1"));
    let offered: Vec<&str> = perm.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(offered, ["allow", "allow_always", "deny"]);
    assert_eq!(perm.why.reason.as_deref(), Some("Remove the build output"));
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(60),
            id: perm.id.clone(),
            choice: "allow".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: accept"), "{text:?}");
    let events = attach(&mut ws, &agent, 61).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, status: Some(s), .. } if id == "call-1" && s == "completed")),
        "{events:?}"
    );

    // A denial reaches Codex as a decline, and its tool call fails.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(62),
            text: "permission again".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(63),
            id: perm.id,
            choice: "deny".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: decline"), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_file_change_approval_names_the_files_and_can_last_the_chat_session() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach_and_send(&mut ws, &agent, "edit").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.title, "Edit src/a.rs");
    assert_eq!(perm.options.iter().find(|o| o.id == "allow_always").unwrap().name, "Allow for this chat");
    assert!(perm.why.always_rule.is_none());
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(70),
            id: perm.id,
            choice: "allow_always".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("edit: acceptForSession"), "{text:?}");
    let events = attach(&mut ws, &agent, 71).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolCall { id, tool_kind, content, .. } if id == "call-2" && tool_kind == "edit" && !content.is_empty())),
        "the call carries its diff: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_interrupts_a_codex_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach_and_send(&mut ws, &agent, "work slowly").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(
        &mut ws,
        ClientMsg::AgentCancel {
            req: Some(80),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent { event, .. } => {
                if let AgentEvent::TurnEnd { stop_reason, .. } = event.event {
                    assert_eq!(stop_reason, "cancelled");
                    break;
                }
            }
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    // The agent takes the next turn.
    let text = reply(&mut ws, &agent, "after").await;
    assert_eq!(text, "echo: after");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_approval_policy_is_an_independent_codex_thread_control() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    let options = config_of(&mut ws, &agent, 90).await;
    assert_eq!(current(&options, "mode"), "on-request");
    attach(&mut ws, &agent, 89).await;
    config_set_refused(&mut ws, &agent, 91, "mode", "yolo", "does not take yolo").await;
    send_config_set(&mut ws, &agent, 92, "mode", "never").await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Ok { req: 92 } => break,
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("policy=never"), "{text:?}");
}

/// A daemon on a state file in `dir` with a Codex pane that has answered `first task`; returns
/// its url, the agent and the thread id the pane saved. The daemon stops before this returns.
async fn codex_pane_that_answered(dir: &std::path::Path) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_agent_as(&url, &pane, Some("first task"), Some("codex")).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &agent, |events| {
        events.contains(&AgentEvent::TurnEnd {
            turn: 1,
            stop_reason: "end_turn".into(),
        })
    })
    .await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let thread = saved["spaces"][0]["root"]["conversation"]
        .as_str()
        .expect("the pane saved Codex's thread id")
        .to_string();
    drop(ws);
    daemon.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    (agent, thread)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_resumes_the_codex_panes_thread() {
    let dir = Scratch::new("codex-resume");
    let (agent, thread) = codex_pane_that_answered(&dir.0).await;
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    // The thread's turns are the transcript.
    let events = transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    assert_eq!(
        events,
        [
            AgentEvent::User {
                turn: 1,
                text: "first task".into()
            },
            AgentEvent::TurnStart { turn: 1 },
            AgentEvent::Text {
                text: "echo: first task".into()
            },
            AgentEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into()
            },
        ],
        "{events:?}"
    );
    // The next turn runs in the same thread.
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains(&format!("thread={thread}")), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_thread_that_will_not_reopen_leaves_only_the_notice() {
    let dir = Scratch::new("codex-lost");
    let (agent, thread) = codex_pane_that_answered(&dir.0).await;
    std::fs::remove_dir_all(dir.0.join("codex-home").join("sessions")).unwrap();
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    assert_eq!(events, []);
    // A new thread runs the agent, and the saved one is kept for the next start.
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(!text.contains(&format!("thread={thread}")), "{text:?}");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.0.join("spaces.json")).unwrap()).unwrap();
    assert_eq!(saved["spaces"][0]["root"]["conversation"], thread);
}

async fn pi_agent(url: &str) -> String {
    let (_, pane) = space(url, "/tmp").await;
    start_agent_as(url, &pane, None, Some("pi")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_pane_streams_a_turn_with_its_home_the_brief_and_its_thinking_level() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = pi_agent(&url).await;
    assert_eq!(layout(&url).await["spaces"][0]["root"]["harness"], "pi");
    let options = config_of(&mut ws, &agent, 90).await;
    assert_eq!(current(&options, "thinking"), "medium");
    attach(&mut ws, &agent, 89).await;
    config_set_refused(&mut ws, &agent, 91, "thinking", "max", "does not take max").await;
    send_config_set(&mut ws, &agent, 92, "thinking", "low").await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Ok { req: 92 } => break,
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("echo: show setup"), "{text:?}");
    assert!(
        text.contains("pi_home=") && text.contains("/pi-home"),
        "PI_CODING_AGENT_DIR is pi's home: {text:?}"
    );
    assert!(text.contains("thinking=low"), "{text:?}");
    let brief = text.split("brief: ").nth(1).unwrap_or("-");
    assert!(!brief.starts_with('-'), "the brief went in: {text:?}");
    // The stats pi reported after the turn fill the pane's context use.
    let status = layout(&url).await;
    assert_eq!(
        status["spaces"][0]["root"]["context"],
        serde_json::json!({"used": 10_000, "size": 200_000}),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_extension_dialog_shows_a_card_and_the_answer_reaches_pi() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = pi_agent(&url).await;
    attach_and_send(&mut ws, &agent, "permission").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.agent, agent);
    assert_eq!(perm.title, "Allow rm -rf build?");
    let offered: Vec<&str> = perm.options.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(offered, ["Yes", "No", "Dismiss"]);
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(60),
            id: perm.id.clone(),
            choice: "0".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: Yes"), "{text:?}");
    let events = attach(&mut ws, &agent, 61).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { id, status: Some(s), .. } if id == "call-1" && s == "completed")),
        "{events:?}"
    );
    // `deny` picks the way out, which pi reads as a cancelled dialog.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(62),
            agent: agent.clone(),
            text: "permission".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: Some(63),
            id: perm.id.clone(),
            choice: "deny".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: cancelled"), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_aborts_a_pi_turn_and_a_failed_reply_fails_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = pi_agent(&url).await;
    attach_and_send(&mut ws, &agent, "work slowly").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(
        &mut ws,
        ClientMsg::AgentCancel {
            req: Some(80),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent { event, .. } => match event.event {
                AgentEvent::TurnEnd { stop_reason, .. } => {
                    assert_eq!(stop_reason, "cancelled");
                    break;
                }
                AgentEvent::Error { message, .. } => panic!("{message}"),
                _ => {}
            },
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    let text = reply(&mut ws, &agent, "after").await;
    assert_eq!(text, "echo: after");
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(81),
            agent: agent.clone(),
            text: "fail now".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Agent { event, .. } => match event.event {
                AgentEvent::Error { turn, message } => {
                    assert!(turn.is_some());
                    assert_eq!(message, "429 rate limited");
                    break;
                }
                AgentEvent::TurnEnd { stop_reason, .. } => {
                    panic!("the failed reply ended as {stop_reason}")
                }
                _ => {}
            },
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

/// A daemon whose pi pane answered its first task, stopped; returns the agent and its id.
async fn pi_pane_that_answered(dir: &std::path::Path) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_agent_as(&url, &pane, Some("first task"), Some("pi")).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &agent, |events| {
        events.contains(&AgentEvent::TurnEnd {
            turn: 1,
            stop_reason: "end_turn".into(),
        })
    })
    .await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let id = saved["spaces"][0]["root"]["conversation"]
        .as_str()
        .expect("the pane saved pi's agent id")
        .to_string();
    drop(ws);
    daemon.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    (agent, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_resumes_the_pi_panes_agent() {
    let dir = Scratch::new("pi-resume");
    let (agent, id) = pi_pane_that_answered(&dir.0).await;
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    let events = transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    assert_eq!(
        events,
        [
            AgentEvent::User {
                turn: 1,
                text: "first task".into()
            },
            AgentEvent::TurnStart { turn: 1 },
            AgentEvent::Text {
                text: "echo: first task".into()
            },
            AgentEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into()
            },
        ],
        "{events:?}"
    );
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains(&format!("session={id}")), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_pi_pane_shows_its_context_before_a_new_turn_and_compaction_clears_it() {
    let dir = Scratch::new("pi-context");
    let (agent, _) = pi_pane_that_answered(&dir.0).await;
    // The figure the pane saved is stale; only pi's own stats can say 10,000.
    let state_file = dir.0.join("spaces.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    saved["spaces"][0]["root"]["context"] = serde_json::json!({"used": 1, "size": 200_000});
    std::fs::write(&state_file, saved.to_string()).unwrap();
    let (url, _, _) =
        start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), Some(state_file)).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    assert_eq!(
        attached_context(&mut ws, &agent, 600).await,
        Some(wheel_proto::ContextUse {
            used: 10_000,
            size: 200_000
        })
    );

    // pi has no token count right after a compaction, and the shown figure goes.
    reply(&mut ws, &agent, "context compacted").await;
    assert_eq!(attached_context(&mut ws, &agent, 601).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_session_that_is_gone_leaves_only_the_notice() {
    let dir = Scratch::new("pi-lost");
    let (agent, id) = pi_pane_that_answered(&dir.0).await;
    std::fs::remove_dir_all(dir.0.join("pi-home").join("sessions")).unwrap();
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &agent).await;
    assert_eq!(notice, wheel_proto::AGENT_NOT_REOPENED);
    assert_eq!(events, []);
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(!text.contains(&format!("session={id}")), "{text:?}");
}

/// Read `status_changed` messages from `ws` until one for `pane` carries `want`; every message read
/// on the way is appended to `seen`.
async fn status_until(
    ws: &mut Ws,
    pane: &str,
    want: Option<wheel_proto::PaneStatus>,
    seen: &mut Vec<(
        Option<String>,
        Option<wheel_proto::PaneStatus>,
        Option<wheel_proto::PaneStatus>,
    )>,
) {
    loop {
        if let ServerMsg::StatusChanged {
            pane: changed,
            status,
            space_status,
            ..
        } = recv(ws).await
        {
            let hit = changed.as_deref() == Some(pane) && status == want;
            seen.push((changed, status, space_status));
            if hit {
                return;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_and_its_space_follow_the_agent_through_a_permission_request() {
    use wheel_proto::PaneStatus::{Idle, NeedsYou, Working};
    let url = start_daemon().await;
    let mut watcher = connect(&url).await;
    let (pane, agent) = agent_pane(&url, None).await;
    let mut seen = Vec::new();
    status_until(&mut watcher, &pane, Some(Idle), &mut seen).await;
    seen.clear();

    let mut asker = connect(&url).await;
    send(
        &mut asker,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(2),
            text: "needs permission".into(),
        },
    )
    .await;
    status_until(
        &mut watcher,
        &pane,
        Some(NeedsYou {
            waiting: 1,
            reason: None,
        }),
        &mut seen,
    )
    .await;
    // The listing says the same as the broadcast.
    let listed = layout(&url).await;
    assert_eq!(
        listed["spaces"][0]["root"]["status"],
        serde_json::json!({"kind": "needs_you", "waiting": 1}),
        "{listed}"
    );
    assert_eq!(
        listed["spaces"][0]["status"],
        serde_json::json!({"kind": "needs_you", "waiting": 1}),
        "{listed}"
    );
    let perm = state_perm(&url).await;
    send(
        &mut asker,
        ClientMsg::PermAnswer {
            req: Some(3),
            id: perm,
            choice: "allow".into(),
        },
    )
    .await;
    status_until(&mut watcher, &pane, Some(Working), &mut seen).await;
    // No client shows the pane, so the finished turn is done, not idle.
    status_until(&mut watcher, &pane, Some(wheel_proto::PaneStatus::Done), &mut seen).await;

    // Every message is a change: none repeats the one before, and the space follows its one pane.
    for pair in seen.windows(2) {
        assert_ne!(pair[0], pair[1], "{seen:?}");
    }
    for (_, status, space_status) in &seen {
        assert_eq!(status, space_status, "{seen:?}");
    }
    // Nothing more arrives for a recompute that gives the same value.
    let quiet = tokio::time::timeout(Duration::from_millis(600), async {
        loop {
            if let ServerMsg::StatusChanged { .. } = recv(&mut watcher).await {
                return;
            }
        }
    })
    .await;
    assert!(
        quiet.is_err(),
        "a status changed again with nothing changing"
    );
}

/// The id of the one permission request waiting, from `status`.
async fn state_perm(url: &str) -> String {
    let mut ws = connect(url).await;
    send(&mut ws, ClientMsg::Status { req: Some(9005) }).await;
    loop {
        if let ServerMsg::Status {
            req: Some(9005),
            perms,
            ..
        } = recv(&mut ws).await
        {
            assert_eq!(perms.len(), 1, "{perms:?}");
            return perms[0].id.clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_gives_up_leaves_its_pane_and_space_stopped() {
    let (url, _, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, quick()).await;
    let dir = std::env::temp_dir().join(format!("wheel-stopped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_, pane) = space(&url, dir.to_str().unwrap()).await;
    std::fs::remove_dir(&dir).unwrap();
    let mut watcher = connect(&url).await;
    pane_start_reply(&url, &pane, "first task").await;
    let mut seen = Vec::new();
    status_until(
        &mut watcher,
        &pane,
        Some(wheel_proto::PaneStatus::Stopped),
        &mut seen,
    )
    .await;
    assert_eq!(
        seen.last().unwrap().2,
        Some(wheel_proto::PaneStatus::Stopped)
    );
    let listed = layout(&url).await;
    assert_eq!(
        listed["spaces"][0]["status"],
        serde_json::json!({"kind": "stopped"}),
        "{listed}"
    );
}

/// A broadcast that reaches the connection before an op's `Ok` is still there for the next `recv`.
#[tokio::test]
async fn client_op_keeps_the_broadcasts_it_reads_past() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut server = tokio_tungstenite::accept_async(tcp).await.unwrap();
        server.next().await.unwrap().unwrap();
        for msg in [
            ServerMsg::PermResolved {
                id: "p1".into(),
                choice: "withdrawn".into(),
            },
            ServerMsg::Ok { req: 7 },
        ] {
            let text = serde_json::to_string(&msg).unwrap();
            server.send(Message::Text(text.into())).await.unwrap();
        }
        let _ = server.next().await;
    });
    let mut ws = connect(&format!("ws://{addr}/ws")).await;
    let reply = client_op(&mut ws, 7, ClientMsg::Status { req: Some(7) }).await;
    assert!(matches!(reply, ServerMsg::Ok { req: 7 }), "{reply:?}");
    match recv(&mut ws).await {
        ServerMsg::PermResolved { id, choice } => {
            assert_eq!((id.as_str(), choice.as_str()), ("p1", "withdrawn"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relaunched_agent_resumes_the_conversation_it_was_in() {
    let dir = Scratch::new("relaunch-resumes");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 599).await;
    agent_reply(&mut ws, &agent, "first task").await;
    let first = saved_conversation(&state_file).expect("the pane saved its conversation");

    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(600),
            text: "die now".into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::Agent {
            event:
                StampedEvent {
                    event: AgentEvent::Error { turn: None, .. },
                    ..
                },
            ..
        } = recv(&mut ws).await
        {
            break;
        }
    }
    let waited = agent_wait(&mut ws, &agent, 30.0, 601).await;
    assert!(
        matches!(waited, ServerMsg::AgentWaited { ready: true, .. }),
        "{waited:?}"
    );

    // The relaunched agent is in the same conversation: the saved id is unchanged, the store holds
    // no second conversation, and the transcript holds the first turn once.
    assert_eq!(saved_conversation(&state_file), Some(first.clone()));
    let listed = conversations_of(&mut ws, &agent, 602).await;
    assert_eq!(
        listed.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        [first],
        "{listed:?}"
    );
    let events = attach(&mut ws, &agent, 603).await;
    let firsts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::User { text, .. } if text == "first task"))
        .count();
    assert_eq!(firsts, 1, "{events:?}");
    agent_reply(&mut ws, &agent, "back again").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clear_empties_the_transcript_and_the_pane_saves_the_new_conversation_once_it_has_a_turn()
{
    let dir = Scratch::new("agent-clear");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 700).await;
    agent_reply(&mut ws, &agent, "before the clear").await;
    let first = saved_conversation(&state_file).expect("the pane saved its conversation");

    send(
        &mut ws,
        ClientMsg::AgentSend {
            agent: agent.clone(),
            req: Some(701),
            text: "/clear".into(),
        },
    )
    .await;
    // `agent_wait` only says the agent process is ready, which it is while the `/clear` is
    // still in flight, so poll for the reset itself: an empty transcript and no saved conversation.
    let mut req = 702;
    let mut tries = 0;
    loop {
        let events = attach(&mut ws, &agent, req).await;
        req += 1;
        let emptied = !events
            .iter()
            .any(|e| matches!(e, AgentEvent::User { .. } | AgentEvent::Text { .. }));
        // Nothing is on disk for the new conversation yet, so the pane saves none.
        if emptied && saved_conversation(&state_file).is_none() {
            break;
        }
        tries += 1;
        assert!(
            tries < 300,
            "the clear never emptied the transcript or the saved conversation: {events:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    agent_reply(&mut ws, &agent, "after the clear").await;
    let second = saved_conversation(&state_file).expect("the new conversation is saved");
    assert_ne!(second, first);
    // The saved id is the one the agent wrote its transcript under, so a restart can reopen it.
    let held = conversations_of(&mut ws, &agent, 900).await;
    assert!(
        held.iter().any(|c| c.id == second),
        "the saved conversation {second} is not in the store: {held:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_is_told_which_slash_commands_the_agent_accepts_before_any_message() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach(&mut ws, &agent, 799).await;

    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::AgentAttach {
            req: Some(800),
            agent: agent.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::AgentCommands { agent, commands } = recv(&mut late).await {
            assert_eq!(agent, agent);
            assert_eq!(commands, ["clear", "compact", "review"]);
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_commands_output_shows_as_a_text_row() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach(&mut ws, &agent, 810).await;
    assert_eq!(reply(&mut ws, &agent, "/compact").await, "Compacted");
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_start_runs_the_agent_on_the_model_asked_for_and_status_shows_it() {
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    // A refused model goes through the restart backoff before the start fails (wheel-2fjs);
    // quick() shortens that wait, which this test does not check.
    let (url, _, daemon) = start_daemon_timed(fake, None, quick()).await;
    let (_, pane) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    let started = client_op(
        &mut ws,
        9320,
        ClientMsg::PaneStart {
            req: Some(9320),
            pane: pane.clone(),
            kind: StartKind::Agent,
            message: None,
            agent: Some("claude".into()),
            model: Some("sonnet".into()),
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    let ServerMsg::Started {
        agent: Some(agent),
        ..
    } = started
    else {
        panic!("{started:?}");
    };
    // The process got the flag, and the listing reads the model back from the agent.
    attach(&mut ws, &agent, 40).await;
    let text = reply(&mut ws, &agent, "show model").await;
    assert!(text.contains("model: sonnet"), "{text:?}");
    assert_eq!(layout(&url).await["spaces"][0]["root"]["model"], "sonnet");

    // A model the agent does not offer fails the start with the agent's reason, and the pane that
    // was started with it keeps no agent.
    let (_, other) = space(&url, "/tmp").await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9321),
            pane: other,
            kind: StartKind::Agent,
            message: None,
            agent: Some("claude".into()),
            model: Some("no-such-model".into()),
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    loop {
        if let ServerMsg::Error {
            req: Some(9321),
            message,
            ..
        } = recv(&mut ws).await
        {
            assert!(
                message.contains("does not know the model no-such-model"),
                "{message}"
            );
            break;
        }
    }
    // Codex is asked for its model list first, so an unknown one fails the start too, and a
    // known one shows in the listing.
    let (_, third) = space(&url, "/tmp").await;
    send(
        &mut ws,
        ClientMsg::PaneStart {
            req: Some(9322),
            pane: third.clone(),
            kind: StartKind::Agent,
            message: None,
            agent: Some("codex".into()),
            model: Some("no-such-model".into()),
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    loop {
        if let ServerMsg::Error {
            req: Some(9322),
            message,
            ..
        } = recv(&mut ws).await
        {
            assert!(
                message.contains("does not know the model no-such-model"),
                "{message}"
            );
            break;
        }
    }
    let started = client_op(
        &mut ws,
        9323,
        ClientMsg::PaneStart {
            req: Some(9323),
            pane: third,
            kind: StartKind::Agent,
            message: None,
            agent: Some("codex".into()),
            model: Some("fake-codex-mini".into()),
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    let spaces = layout(&url).await;
    let models: Vec<&str> = spaces["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["root"]["model"].as_str())
        .collect();
    assert!(models.contains(&"fake-codex-mini"), "{models:?}");
    daemon.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_agent_pane_restarts_in_the_approval_policy_a_person_chose() {
    let dir = Scratch::new("codex-pane-keeps-policy");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    let started = client_op(
        &mut ws,
        9340,
        ClientMsg::PaneStart {
            req: Some(9340),
            pane,
            kind: StartKind::Agent,
            message: None,
            agent: Some("codex".into()),
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    let agent = layout(&url).await["spaces"][0]["root"]["agent"]
        .as_str()
        .unwrap()
        .to_string();
    attach(&mut ws, &agent, 41).await;
    send_config_set(&mut ws, &agent, 42, "mode", "never").await;
    let mut saved = serde_json::Value::Null;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        saved = serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
        if saved["spaces"][0]["root"]["permission_mode"] == "never" {
            break;
        }
    }
    assert_eq!(saved["spaces"][0]["root"]["permission_mode"], "never", "{saved}");
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, second) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    let agent = layout(&url).await["spaces"][0]["root"]["agent"]
        .as_str()
        .unwrap()
        .to_string();
    attach(&mut ws, &agent, 43).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("policy=never"), "{text:?}");
    second.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_runs_on_its_model_again_after_a_restart() {
    let dir = Scratch::new("agent-pane-keeps-model");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    let started = client_op(
        &mut ws,
        9330,
        ClientMsg::PaneStart {
            req: Some(9330),
            pane,
            kind: StartKind::Agent,
            message: None,
            agent: Some("claude".into()),
            model: Some("haiku".into()),
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert_eq!(saved["spaces"][0]["root"]["model"], "haiku");
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, second) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    let agent = layout(&url).await["spaces"][0]["root"]["agent"]
        .as_str()
        .unwrap()
        .to_string();
    attach(&mut ws, &agent, 40).await;
    let text = reply(&mut ws, &agent, "show model").await;
    assert!(text.contains("model: haiku"), "{text:?}");
    second.abort();
}

#[tokio::test]
async fn a_text_block_whose_stream_events_fell_short_still_arrives_whole_before_the_tool_call() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "write mid-turn").await;
    turn_text(&mut ws).await;

    let events = attach(&mut connect(&url).await, &agent, 3).await;
    let shape: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(format!("text {}", text.len())),
            AgentEvent::ToolCall { id, .. } => Some(format!("tool {id}")),
            _ => None,
        })
        .collect();
    assert_eq!(shape, ["text 4000", "tool call-7", "text 13"], "{events:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_kind_of_tool_call_arrives_with_its_kind_and_a_described_command_keeps_its_text() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "show tools").await;
    turn_text(&mut ws).await;

    let events = attach(&mut connect(&url).await, &agent, 3).await;
    let calls: Vec<(String, String, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall {
                title,
                tool_kind,
                detail,
                ..
            } => Some((title.clone(), tool_kind.clone(), detail.clone())),
            _ => None,
        })
        .collect();
    let line = |title: &str, kind: &str, detail: Option<&str>| {
        (title.to_string(), kind.to_string(), detail.map(String::from))
    };
    assert_eq!(
        calls,
        [
            line("Read src/model.go", "read", None),
            line("grep addSuggestions in src/model_test.go", "search", None),
            line("Find **/*.go", "search", None),
            line("Edit src/styles.go", "edit", None),
            line("Write src/new.go", "edit", None),
            line("Run the parser's tests", "execute", Some("cargo test -p parser")),
            line("make build", "execute", None),
            line("Fetch https://example.com/doc", "fetch", None),
            line("Skill implement", "skill", None),
            line("Survey the parser", "subagent", None),
            line("mcp__github__list_issues", "mcp", None),
        ],
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolUpdate { status: Some(s), .. } if s == "failed")),
        "the failed command reports failed: {events:?}"
    );
}

#[tokio::test]
async fn a_text_block_rewritten_after_streaming_is_revised_once_not_appended() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "write rewritten").await;
    let mut live = Vec::new();
    loop {
        if let ServerMsg::Agent { event, .. } = recv(&mut ws).await {
            let end = matches!(event.event, AgentEvent::TurnEnd { .. });
            live.push(event.event);
            if end {
                break;
            }
        }
    }
    let revised: Vec<_> = live
        .iter()
        .filter(|e| matches!(e, AgentEvent::TextRevised { .. }))
        .collect();
    assert_eq!(
        revised,
        [&AgentEvent::TextRevised {
            streamed: "See a.rs, then go.".into(),
            text: "See /w/a.rs , then go.".into(),
        }],
        "{live:?}"
    );

    let events = attach(&mut connect(&url).await, &agent, 3).await;
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["See /w/a.rs , then go."], "{events:?}");
}

#[tokio::test]
async fn a_chat_agent_sees_the_spaces_artifacts_in_its_brief_up_to_twenty() {
    let (url, _, _daemon) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    let dir = std::env::temp_dir().join(format!("wheel-brief-artifacts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_, pane) = space(&url, &dir.to_string_lossy()).await;
    let mut ws = connect(&url).await;
    for name in ["alpha", "bravo"] {
        artifact_reply(&mut ws, lived(name, None, None))
            .await
            .unwrap();
    }
    let setup = |agent: String| {
        let url = url.clone();
        async move {
            let mut ws = connect(&url).await;
            send(
                &mut ws,
                ClientMsg::AgentAttach {
                    req: Some(1),
                    agent: agent.clone(),
                },
            )
            .await;
            while !matches!(
                recv(&mut ws).await,
                ServerMsg::AgentAttached { req: Some(1), .. }
            ) {}
            send(
                &mut ws,
                ClientMsg::AgentSend {
                    agent,
                    req: Some(2),
                    text: "show setup".into(),
                },
            )
            .await;
            turn_text(&mut ws).await
        }
    };
    let text = setup(start_agent(&url, &pane, None).await).await;
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("artifacts: "))
        .unwrap_or_else(|| panic!("{text:?}"));
    assert!(line.contains("- alpha (project)"), "{line}");
    assert!(line.contains("- bravo (project)"), "{line}");

    // Thirty artifacts: the section lists twenty and the count.
    for i in 0..28 {
        artifact_reply(&mut ws, lived(&format!("extra{i:02}"), None, None))
            .await
            .unwrap();
    }
    let (_, other) = space(&url, &dir.to_string_lossy()).await;
    let text = setup(start_agent(&url, &other, None).await).await;
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("artifacts: "))
        .unwrap_or_else(|| panic!("{text:?}"));
    assert_eq!(line.matches("(project)").count(), 20, "{line}");
    assert!(line.contains("30 artifacts in all"), "{line}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A raw HTTP request to the daemon with a body: the status code and the response body.
async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &[u8],
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    // A refusal can come before the body is read and close the connection; that is the answer.
    let _ = stream.write_all(body).await;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_put_to_an_agent_pane_is_stored_for_its_agent_and_goes_with_the_pane() {
    let (url, addr) = start_daemon_at().await;
    let (pane, _agent) = agent_pane(&url, None).await;
    let bearer = format!("Authorization: Bearer {TOKEN}\r\n");

    // Stored under the pane's own directory, named after what was sent, the path in the answer.
    let (status, body) = http(addr, "PUT", &format!("/uploads/{pane}/My%20Photo.jpg"), &bearer, b"jpeg bytes").await;
    assert_eq!(status, 201, "{body}");
    let first: serde_json::Value = serde_json::from_str(&body).unwrap();
    let path = std::path::PathBuf::from(first["path"].as_str().unwrap());
    assert!(path.ends_with(format!("uploads/{pane}/1-My_Photo.jpg")), "{path:?}");
    assert_eq!(first["bytes"], 10);
    assert_eq!(std::fs::read(&path).unwrap(), b"jpeg bytes");

    // The same name again takes the next number and leaves the first alone.
    let (status, body) = http(addr, "PUT", &format!("/uploads/{pane}/My%20Photo.jpg"), &bearer, b"other").await;
    assert_eq!(status, 201, "{body}");
    let second: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(second["name"], "2-My_Photo.jpg");
    assert_eq!(std::fs::read(&path).unwrap(), b"jpeg bytes");

    // Two at once never share a name.
    let same = format!("/uploads/{pane}/same.txt");
    let (a, b) = tokio::join!(
        http(addr, "PUT", &same, &bearer, b"a"),
        http(addr, "PUT", &same, &bearer, b"b"),
    );
    assert_eq!((a.0, b.0), (201, 201), "{a:?} {b:?}");
    let (a, b): (serde_json::Value, serde_json::Value) =
        (serde_json::from_str(&a.1).unwrap(), serde_json::from_str(&b.1).unwrap());
    assert_ne!(a["name"], b["name"]);

    // No token, a wrong one, or a browser's Origin is refused; so is a pane that is not an agent's.
    assert_eq!(http(addr, "PUT", &format!("/uploads/{pane}/x"), "", b"x").await.0, 401);
    assert_eq!(http(addr, "PUT", &format!("/uploads/{pane}/x"), "Authorization: Bearer nope\r\n", b"x").await.0, 401);
    let origin = format!("{bearer}Origin: http://{addr}\r\n");
    assert_eq!(http(addr, "PUT", &format!("/uploads/{pane}/x"), &origin, b"x").await.0, 403);
    assert_eq!(http(addr, "PUT", "/uploads/p999/x", &bearer, b"x").await.0, 404);
    assert_eq!(http(addr, "PUT", "/uploads/..%2Fetc/x", &bearer, b"x").await.0, 400);

    // A body declared over the limit is refused before a byte is read.
    let head = &bearer;
    let big = {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "PUT /uploads/{pane}/big.bin HTTP/1.1\r\nHost: {addr}\r\n{head}Content-Length: {}\r\nConnection: close\r\n\r\n",
            wheel_proto::UPLOAD_MAX_BYTES + 1
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response).await;
        response
    };
    assert!(big.starts_with("HTTP/1.1 413"), "{big}");
    assert!(!path.parent().unwrap().join("1-big.bin").exists());

    // DELETE takes one back; a name that leaves the directory is refused.
    let (status, _) = http(addr, "DELETE", &format!("/uploads/{pane}/2-My_Photo.jpg"), &bearer, b"").await;
    assert_eq!(status, 204);
    assert!(!path.parent().unwrap().join("2-My_Photo.jpg").exists());
    assert_eq!(http(addr, "DELETE", &format!("/uploads/{pane}/2-My_Photo.jpg"), &bearer, b"").await.0, 404);
    assert_eq!(http(addr, "DELETE", &format!("/uploads/{pane}/..%2F..%2Ftoken"), &bearer, b"").await.0, 400);

    // Closing the pane removes everything uploaded to it.
    let dir = path.parent().unwrap().to_path_buf();
    let mut ws = connect(&url).await;
    client_op(&mut ws, 72, ClientMsg::PaneClose { req: Some(72), pane: pane.clone() }).await;
    for _ in 0..100 {
        if !dir.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!dir.exists(), "{dir:?} is still there");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_agent_starts_itself_holds_it_busy_until_it_ends() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "then wake").await;
    // The prompt's turn ends, then the agent starts another that waits on a permission answer.
    let perm = next_perm(&mut ws).await;
    let held = layout(&url).await;
    assert_eq!(held["spaces"][0]["root"]["state"]["phase"], "busy", "{held}");
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    // Its result ends it: the agent is ready again.
    loop {
        let now = layout(&url).await;
        if now["spaces"][0]["root"]["state"]["phase"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_a_turn_the_agent_started_itself() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "then wake").await;
    let _perm = next_perm(&mut ws).await;
    send(&mut ws, ClientMsg::AgentCancel { req: Some(77), agent: agent.clone() }).await;
    loop {
        let now = layout(&url).await;
        if now["spaces"][0]["root"]["state"]["phase"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Start an orchestrator of `scope` as a new agent pane, a space one in `space`; its pane and
/// agent, or the error's message.
async fn orchestrator(
    ws: &mut Ws,
    req: u64,
    space: Option<&str>,
    scope: wheel_proto::OrchestratorScope,
) -> Result<(String, String), String> {
    send(ws, ClientMsg::OrchestratorStart { req: Some(req), space: space.map(String::from), scope, turn_limit_secs: None, if_none: false }).await;
    loop {
        match recv(ws).await {
            ServerMsg::OrchestratorStarted { req: r, pane, agent } if r == req => return Ok((pane, agent)),
            ServerMsg::Error { req: Some(r), message, .. } if r == req => return Err(message),
            _ => {}
        }
    }
}

/// The pane `pane` of a layout listing (`layout`), as JSON.
fn pane_in(layout: &serde_json::Value, pane: &str) -> Option<serde_json::Value> {
    match layout {
        serde_json::Value::Object(o) if o.get("id").and_then(|v| v.as_str()) == Some(pane) && o.contains_key("kind") => Some(layout.clone()),
        serde_json::Value::Object(o) => o.values().find_map(|v| pane_in(v, pane)),
        serde_json::Value::Array(a) => a.iter().find_map(|v| pane_in(v, pane)),
        _ => None,
    }
}

/// An orchestrator is an ordinary agent pane (wheel-eg4e.19.4): `orchestrator_start` makes one in
/// the space and names its pane, which the layout marks with its scope; two can share a space,
/// each with its own inbox; it runs with every tool an agent pane has and the orchestrator brief
/// on top of the pane's; `orchestrator_stop` closes its pane.
#[tokio::test(flavor = "multi_thread")]
async fn an_orchestrator_is_an_agent_pane_with_a_scope_and_an_inbox() {
    use wheel_proto::OrchestratorScope::Space;
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (space_id, _) = space(&url, "/tmp").await;

    let refused = orchestrator(&mut ws, 70, Some("s-missing"), Space).await.unwrap_err();
    assert!(refused.contains("no space s-missing"), "{refused:?}");

    // 1. A pane in that space, marked as an orchestrator.
    let (first, agent) = orchestrator(&mut ws, 71, Some(&space_id), Space).await.unwrap();
    let listed = layout(&url).await;
    let space_json = listed["spaces"].as_array().unwrap().iter().find(|s| s["id"] == space_id.as_str()).unwrap().clone();
    let pane = pane_in(&space_json, &first).expect("the orchestrator's pane is in its space");
    assert_eq!((pane["kind"].as_str(), pane["agent"].as_str(), pane["orchestrator"].as_str()), (Some("agent"), Some(agent.as_str()), Some("space")), "{pane}");

    // 2. A second one in the same space runs too, with an inbox of its own.
    let (second, other) = orchestrator(&mut ws, 72, Some(&space_id), Space).await.unwrap();
    assert!(second != first && other != agent);
    assert!(inbox(&mut ws, &first, 73).await.is_empty() && inbox(&mut ws, &second, 74).await.is_empty());

    // 7. Every tool an agent pane has: no whitelist, no empty setting sources, no dontAsk; the
    // orchestrator brief follows the pane's own.
    attach_and_send(&mut ws, &agent, "show orchestrator").await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("tools=-") && text.contains("setting-sources=-") && text.contains("mode=-"), "{text:?}");
    assert!(!text.contains("Bash(wheel agent:*)"), "{text:?}");
    assert!(text.contains(&format!("You are the agent in pane {first}")), "the pane's own brief: {text:?}");
    assert!(text.contains("You are an orchestrator") && text.contains(&format!("wheel inbox ack {first} <ids>")), "{text:?}");
    assert!(text.contains("wheel perm answer <id> allow --confirmed"), "{text:?}");
    assert!(text.contains("allowed=Bash(wheel:*) denied=Bash(wheel perm answer:*)\n"), "a space orchestrator never answers a permission request unasked: {text:?}");
    assert!(text.contains("never wait on an agent's turn") && !text.contains("wait on, cancel"), "the orchestrator dispatches and never waits: {text:?}");

    // Stopping closes the pane; its inbox goes with it.
    client_op(&mut ws, 75, ClientMsg::OrchestratorStop { req: Some(75), pane: first.clone() }).await;
    assert!(pane_in(&layout(&url).await, &first).is_none(), "the pane closed");
    send(&mut ws, ClientMsg::OrchestratorStop { req: Some(76), pane: first.clone() }).await;
    loop {
        if let ServerMsg::Error { req: Some(76), message, .. } = recv(&mut ws).await {
            assert_eq!(message, format!("{first} holds no orchestrator"));
            break;
        }
    }
    assert!(inbox(&mut ws, &second, 77).await.is_empty(), "the other one runs on");
}

/// Closing a space closes its orchestrator's pane, and a start in the closed space is refused.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_space_ends_its_orchestrator() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (space_id, _) = space(&url, "/tmp").await;
    let (pane, _) = orchestrator(&mut ws, 80, Some(&space_id), wheel_proto::OrchestratorScope::Space).await.unwrap();
    client_op(&mut ws, 81, ClientMsg::SpaceClose { req: Some(81), id: space_id.clone(), if_empty: false }).await;
    send(&mut ws, ClientMsg::OrchestratorStop { req: Some(82), pane: pane.clone() }).await;
    loop {
        if let ServerMsg::Error { req: Some(82), message, .. } = recv(&mut ws).await {
            assert!(message.contains("holds no orchestrator"), "{message:?}");
            break;
        }
    }
    let refused = orchestrator(&mut ws, 83, Some(&space_id), wheel_proto::OrchestratorScope::Space).await.unwrap_err();
    assert!(refused.contains(&format!("no space {space_id}")), "{refused:?}");
}

/// The agents whose items wait in the inbox of the orchestrator in `pane`.
async fn inbox_agents(ws: &mut Ws, pane: &str, req: u64) -> Vec<String> {
    inbox(ws, pane, req).await.into_iter().map(|i| i.agent).collect()
}

/// Wait until the inbox of the orchestrator in `pane` holds an item about `agent`.
async fn inbox_gets(ws: &mut Ws, pane: &str, agent: &str, req: u64) {
    for _ in 0..100 {
        if inbox_agents(ws, pane, req).await.iter().any(|a| a == agent) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no item about {agent} reached {pane}");
}

/// Each agent event goes to one orchestrator (wheel-eg4e.19.4): the first space orchestrator
/// started in the agent's space, else the first global one; a space orchestrator's own events go
/// to the first global one, and to nobody when none runs.
#[tokio::test(flavor = "multi_thread")]
async fn each_agent_event_goes_to_the_narrowest_orchestrator() {
    use wheel_proto::OrchestratorScope::{Global, Space};
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (here, here_pane) = space(&url, "/tmp").await;
    let (_, there_pane) = space(&url, "/tmp").await;
    // A long pull, so no inbox message starts a turn in an orchestrator mid-check.
    let (o1, a1) = orchestrator(&mut ws, 200, Some(&here), Space).await.unwrap();
    let (o2, _) = orchestrator(&mut ws, 201, Some(&here), Space).await.unwrap();

    // 6. With no global orchestrator, a space orchestrator's finished turn reaches nobody.
    let mut talk = connect(&url).await;
    attach(&mut talk, &a1, 202).await;
    reply(&mut talk, &a1, "hello").await;

    // 3. A finished turn in a space with orchestrators reaches only the first started there.
    let mine = start_agent(&url, &here_pane, Some("hello")).await;
    inbox_gets(&mut ws, &o1, &mine, 203).await;
    assert_eq!(inbox_agents(&mut ws, &o1, 204).await, [mine.clone()], "o1 holds only this space's agent, not its own turn");
    assert!(inbox_agents(&mut ws, &o2, 205).await.is_empty(), "the second space orchestrator gets nothing");

    // A reload gives o1 a new agent but keeps its inbox and its place ahead of o2.
    let reloaded = client_op(&mut ws, 220, ClientMsg::PaneReload { req: Some(220), pane: o1.clone(), set: Default::default(), unset: Vec::new() }).await;
    assert!(!matches!(reloaded, ServerMsg::Error { .. }), "{reloaded:?}");
    let mut renamed = None;
    for _ in 0..100 {
        renamed = pane_in(&layout(&url).await, &o1).and_then(|p| p["agent"].as_str().map(String::from)).filter(|a| *a != a1);
        if renamed.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let a1 = renamed.expect("the reload gave o1 a new agent");
    let mut talk = connect(&url).await;
    attach(&mut talk, &a1, 227).await;
    assert_eq!(inbox_agents(&mut ws, &o1, 221).await, [mine.clone()], "the inbox outlived the reload");
    let mut child = connect(&url).await;
    attach(&mut child, &mine, 226).await;
    reply(&mut child, &mine, "hello after the reload").await;
    for _ in 0..100 {
        if inbox(&mut ws, &o1, 222).await.first().is_some_and(|i| i.count == 2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(inbox(&mut ws, &o1, 223).await[0].count, 2, "o1 is still first in the space");
    assert!(inbox_agents(&mut ws, &o2, 224).await.is_empty());

    // An orchestrator runs only as a chat.
    send(&mut ws, ClientMsg::PaneView { req: Some(225), pane: o2.clone(), to: Some(wheel_proto::PaneView::Terminal) }).await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Error { req: Some(225), message, .. } => {
                assert_eq!(message, format!("{o2} is an orchestrator, which runs only as a chat"));
                break;
            }
            ServerMsg::PaneView { req: 225, .. } => panic!("the orchestrator pane switched to a terminal"),
            _ => {}
        }
    }

    // 4. The same event in a space with no orchestrator reaches the first global one.
    let (g1, ga1) = orchestrator(&mut ws, 206, None, Global).await.unwrap();
    let (g2, _) = orchestrator(&mut ws, 207, None, Global).await.unwrap();
    let theirs = start_agent(&url, &there_pane, Some("hello")).await;
    inbox_gets(&mut ws, &g1, &theirs, 208).await;
    assert_eq!(inbox_agents(&mut ws, &o1, 209).await, [mine.clone()]);

    // 5. A space orchestrator's own finished turn reaches the global one: it reports up.
    reply(&mut talk, &a1, "hello again").await;
    inbox_gets(&mut ws, &g1, &a1, 210).await;
    assert_eq!(inbox_agents(&mut ws, &o1, 211).await, [mine.clone()], "not its own space's orchestrators");

    // A global orchestrator's own turn reaches nobody, the other global one included.
    let mut up = connect(&url).await;
    attach(&mut up, &ga1, 212).await;
    reply(&mut up, &ga1, "hello").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(inbox_agents(&mut ws, &g2, 213).await.is_empty());
    let g1_agents = inbox_agents(&mut ws, &g1, 214).await;
    assert!(g1_agents.contains(&theirs) && g1_agents.contains(&a1) && !g1_agents.contains(&ga1), "{g1_agents:?}");
}

/// Every space and session as `status` lists them.
async fn spaces_and_sessions(ws: &mut Ws) -> (Vec<wheel_proto::SpaceInfo>, Vec<wheel_proto::SessionInfo>) {
    send(ws, ClientMsg::Status { req: Some(98) }).await;
    loop {
        if let ServerMsg::Status { req: Some(98), spaces, sessions, .. } = recv(ws).await {
            return (spaces, sessions);
        }
    }
}

/// The one Orchestrator space and the session holding it, as `status` lists them.
async fn orchestrator_space(ws: &mut Ws) -> (wheel_proto::SpaceInfo, wheel_proto::SessionInfo) {
    let (spaces, sessions) = spaces_and_sessions(ws).await;
    let mut marked = spaces.into_iter().filter(|s| s.orchestrator);
    let space = marked.next().expect("an Orchestrator space");
    assert!(marked.next().is_none(), "one Orchestrator space");
    let session = sessions.into_iter().find(|n| n.spaces.contains(&space.id)).expect("a session holds it");
    (space, session)
}

/// Two clients that each start the global orchestrator with `if_none` when Orchestrator mode opens
/// start one between them (wheel-eg4e.19.23).
#[tokio::test(flavor = "multi_thread")]
async fn two_if_none_global_starts_make_one_orchestrator() {
    use wheel_proto::OrchestratorScope::Global;
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, _first) = start_daemon_with(fake, None).await;
    let (mut a, mut b) = (connect(&url).await, connect(&url).await);
    // Both sent before either is answered, as two clients opening Orchestrator mode at once.
    let start = |req| ClientMsg::OrchestratorStart { req: Some(req), space: None, scope: Global, turn_limit_secs: None, if_none: true };
    send(&mut a, start(401)).await;
    send(&mut b, start(402)).await;
    async fn started(ws: &mut Ws, req: u64) -> (String, String) {
        loop {
            match recv(ws).await {
                ServerMsg::OrchestratorStarted { req: r, pane, agent } if r == req => return (pane, agent),
                ServerMsg::Error { req: Some(r), message, .. } if r == req => panic!("refused: {message}"),
                _ => {}
            }
        }
    }
    let (pa, aa) = started(&mut a, 401).await;
    let (pb, ab) = started(&mut b, 402).await;
    assert_eq!((pa.as_str(), aa.as_str()), (pb.as_str(), ab.as_str()), "both starts answered with the one orchestrator");
    let (orch, _) = orchestrator_space(&mut a).await;
    fn globals(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::Object(o) => usize::from(o.get("orchestrator").and_then(|x| x.as_str()) == Some("global")) + o.values().map(globals).sum::<usize>(),
            serde_json::Value::Array(a) => a.iter().map(globals).sum(),
            _ => 0,
        }
    }
    assert_eq!(globals(&serde_json::to_value(&orch).unwrap()), 1, "one global orchestrator runs: {orch:?}");
    // A later start with `if_none` finds it; one without starts a second, as before.
    send(&mut a, start(403)).await;
    assert_eq!(started(&mut a, 403).await.0, pa);
    let (second, _) = orchestrator(&mut a, 404, None, Global).await.unwrap();
    assert_ne!(second, pa, "a start without if_none starts another");
}

/// The global orchestrator makes no change itself (wheel-eg4e.21): it starts with every edit and
/// shell command under `permissions.ask`, and wheeld answers each by its rule, so none reaches the
/// person: edits only under its own folder, ~/Knowledge or a space's docs/, and Bash only for a
/// plain `wheel` command. A space orchestrator is started as before.
#[tokio::test(flavor = "multi_thread")]
async fn wheeld_refuses_the_global_orchestrators_own_edits_and_shell() {
    use wheel_proto::OrchestratorScope::{Global, Space};
    let dir = std::env::temp_dir().join(format!("wheel-orchestrator-own-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("project");
    std::fs::create_dir_all(project.join("docs")).unwrap();
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, _daemon) = start_daemon_with(fake, Some(dir.join("spaces.json"))).await;
    let mut ws = connect(&url).await;
    let (project_space, _) = space(&url, project.to_str().unwrap()).await;
    let (_, global) = orchestrator(&mut ws, 500, None, Global).await.unwrap();
    let (orch, _) = orchestrator_space(&mut ws).await;
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let own = std::path::Path::new(&orch.cwd);
    for (tool, arg, allowed) in [
        ("Write", "/tmp/x".to_string(), false),
        ("Write", own.join("notes.md").display().to_string(), true),
        ("Write", home.join("Knowledge/inbox/x.md").display().to_string(), true),
        ("Edit", project.join("docs/adr.md").display().to_string(), true),
        ("Edit", project.join("src/main.rs").display().to_string(), false),
        ("Write", own.join("../escape.md").display().to_string(), false),
        ("Bash", "ls".to_string(), false),
        ("Bash", "wheel status; ls".to_string(), false),
        ("Bash", "wheel status".to_string(), true),
    ] {
        // Nobody answers a request in this test, so each answer is wheeld's own.
        attach_and_send(&mut ws, &global, &format!("guarded {tool} {arg}")).await;
        let text = turn_text(&mut ws).await;
        if allowed {
            assert!(text.contains(&format!("guarded {tool}: allow\n")), "{tool} {arg}: {text:?}");
        } else {
            assert!(text.contains(&format!("guarded {tool}: deny message=wheeld refused this")), "{tool} {arg}: {text:?}");
        }
    }
    // A space orchestrator gets no ask settings: the person's own settings decide, as before.
    let (_, space_agent) = orchestrator(&mut ws, 501, Some(&project_space), Space).await.unwrap();
    attach_and_send(&mut ws, &space_agent, "guarded Write /tmp/x").await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("guarded Write: ran unasked\n"), "{text:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Global orchestrators run in the Orchestrator space (wheel-eg4e.19.9): the first global start
/// makes it, in a session of its own, in its folder setting; it outlives every orchestrator and a
/// restart; and a mirror in it may show a pane of any session, which no other space's may.
#[tokio::test(flavor = "multi_thread")]
async fn global_orchestrators_run_in_a_permanent_orchestrator_space() {
    use wheel_proto::OrchestratorScope::Global;
    let dir = std::env::temp_dir().join(format!("wheel-orchestrator-space-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (main_space, _) = space(&url, "/tmp").await;

    // 1. The first global start makes the space, in a session named Orchestrator, in the folder
    // beside this machine's wheel.toml, and puts the pane in it.
    let (g1, g1_agent) = orchestrator(&mut ws, 300, None, Global).await.unwrap();
    let (orch, session) = orchestrator_space(&mut ws).await;
    // 2. The global orchestrator may run `wheel perm answer`: no tool is denied it.
    attach_and_send(&mut ws, &g1_agent, "show orchestrator").await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("allowed=Bash(wheel:*) denied=-\n"), "{text:?}");
    let folder = dir.join("orchestrator");
    assert_eq!((orch.name.as_str(), orch.cwd.as_str()), ("Orchestrator", folder.to_str().unwrap()));
    assert!(folder.is_dir(), "wheeld made the folder");
    assert_eq!((session.name.as_str(), session.spaces.clone()), ("Orchestrator", vec![orch.id.clone()]));
    assert!(pane_in(&serde_json::to_value(&orch).unwrap(), &g1).is_some(), "{orch:?}");
    // The orchestrator fills the new space: no empty pane beside it (wheel-eg4e.19.19).
    assert!(matches!(&orch.root, wheel_proto::Node::Pane { id, .. } if *id == g1), "{:?}", orch.root);
    // 3. The layout listing marks it.
    let listed = layout(&url).await;
    let marked: Vec<_> = listed["spaces"].as_array().unwrap().iter().filter(|s| s["orchestrator"] == true).collect();
    assert_eq!(marked.len(), 1, "{listed}");
    assert_eq!(marked[0]["id"], orch.id.as_str());

    // The space and its session are permanent and hold nothing else: a space made from an
    // orchestrator's pane joins an ordinary session, and the person cannot move or close either.
    let made = match client_op(
        &mut ws,
        320,
        ClientMsg::SpaceCreate {
            req: Some(320),
            cwd: "/tmp".into(),
            name: None,
            focus: false,
            background: false,
            pane: Some(g1.clone()),
            session: None,
        },
    )
    .await
    {
        ServerMsg::Created { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let (_, sessions) = spaces_and_sessions(&mut ws).await;
    let holder = sessions.iter().find(|n| n.spaces.contains(&made)).unwrap();
    assert_ne!(holder.id, session.id, "a space made from the Orchestrator space joins another session");
    let main_session = holder.id.clone();
    for (req, msg, refusal) in [
        (321, ClientMsg::SessionMove { req: Some(321), space: orch.id.clone(), session: main_session.clone(), index: None }, "is the Orchestrator space"),
        (322, ClientMsg::SessionMove { req: Some(322), space: made.clone(), session: session.id.clone(), index: None }, "holds only the Orchestrator space"),
        (323, ClientMsg::SpaceClose { req: Some(323), id: orch.id.clone(), if_empty: false }, "is the Orchestrator space"),
        (324, ClientMsg::SessionClose { req: Some(324), session: session.id.clone() }, "holds only the Orchestrator space"),
    ] {
        send(&mut ws, msg).await;
        loop {
            match recv(&mut ws).await {
                ServerMsg::Error { req: Some(r), message, .. } if r == req => {
                    assert!(message.contains(refusal), "{req}: {message}");
                    break;
                }
                ServerMsg::Ok { req: r } if r == req => panic!("{req} was not refused"),
                _ => {}
            }
        }
    }

    // A second global start joins the same space; a global start naming a space is refused.
    let (g2, _) = orchestrator(&mut ws, 301, None, Global).await.unwrap();
    let (again, _) = orchestrator_space(&mut ws).await;
    assert_eq!(again.id, orch.id);
    assert!(pane_in(&serde_json::to_value(&again).unwrap(), &g2).is_some(), "{again:?}");
    let refused = orchestrator(&mut ws, 302, Some(&main_space), Global).await.unwrap_err();
    assert!(refused.contains("takes no space"), "{refused}");

    // 5. A mirror in the Orchestrator space may show a pane of another session; 6. a mirror in any
    // other space may not.
    let elsewhere = match client_op(&mut ws, 303, ClientMsg::SessionNew { req: Some(303), name: "Elsewhere".into() }).await {
        ServerMsg::Created { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let there = match client_op(
        &mut ws,
        304,
        ClientMsg::SpaceCreate {
            req: Some(304),
            cwd: "/tmp".into(),
            name: None,
            focus: false,
            background: false,
            pane: None,
            session: Some(elsewhere.clone()),
        },
    )
    .await
    {
        ServerMsg::Created { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let theirs = wheeld::spaces::first_pane(&spaces_now(&mut ws).await.into_iter().find(|s| s.id == there).unwrap().root).to_string();
    set_kind(&mut ws, 305, &theirs, wheel_proto::NewPaneKind::Terminal, None).await;
    let mirror = mirror_reply(&mut ws, 306, &theirs, &orch.id).await.unwrap();
    let refused = mirror_reply(&mut ws, 307, &theirs, &main_space).await.unwrap_err();
    assert!(refused.contains("another session"), "{refused}");

    // 4. Stopping every orchestrator leaves the space, its mirror and its folder; so does a restart.
    for (req, pane) in [(308, &g1), (309, &g2)] {
        client_op(&mut ws, req, ClientMsg::OrchestratorStop { req: Some(req), pane: pane.clone() }).await;
    }
    let (kept, _) = orchestrator_space(&mut ws).await;
    let kept = serde_json::to_value(&kept).unwrap();
    assert!(pane_in(&kept, &mirror).is_some() && pane_in(&kept, &g1).is_none(), "{kept}");
    assert!(folder.is_dir());
    drop(ws);
    first.abort();
    let (url, _, _second) = start_daemon_with(fake, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (restored, session) = orchestrator_space(&mut ws).await;
    assert_eq!((restored.id.as_str(), session.name.as_str()), (orch.id.as_str(), "Orchestrator"));
    assert!(pane_in(&serde_json::to_value(&restored).unwrap(), &mirror).is_some(), "{restored:?}");

    // 5. The mirror closes with its source.
    client_op(&mut ws, 310, ClientMsg::PaneClose { req: Some(310), pane: theirs.clone() }).await;
    let (after, _) = orchestrator_space(&mut ws).await;
    assert!(pane_in(&serde_json::to_value(&after).unwrap(), &mirror).is_none(), "{after:?}");

    // 2. With the folder setting changed, the next global start moves the space there.
    let moved = dir.join("moved");
    send(
        &mut ws,
        ClientMsg::SettingSet {
            req: Some(311),
            key: "orchestrator.global.dir".into(),
            value: moved.to_str().unwrap().into(),
            space: None,
            machine: true,
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Settings { req: Some(311), .. } => break,
            ServerMsg::Error { req: Some(311), message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    orchestrator(&mut ws, 312, None, Global).await.unwrap();
    let (moved_space, _) = orchestrator_space(&mut ws).await;
    assert_eq!((moved_space.id.as_str(), moved_space.cwd.as_str()), (orch.id.as_str(), moved.to_str().unwrap()));
    assert!(moved.is_dir(), "wheeld made the new folder");

    // 6. Elsewhere the old rule holds: a mirror closes when its source's space moves to another
    // session.
    let (_, main_source) = space(&url, "/tmp").await;
    set_kind(&mut ws, 313, &main_source, wheel_proto::NewPaneKind::Terminal, None).await;
    let main_mirror = mirror_reply(&mut ws, 314, &main_source, &main_space).await.unwrap();
    let source_space = spaces_now(&mut ws).await.into_iter().find(|s| pane_in(&serde_json::to_value(s).unwrap(), &main_source).is_some()).unwrap().id;
    client_op(&mut ws, 315, ClientMsg::SessionMove { req: Some(315), space: source_space, session: elsewhere, index: None }).await;
    let all = serde_json::to_value(spaces_now(&mut ws).await).unwrap();
    assert!(pane_in(&all, &main_mirror).is_none(), "{all}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A global orchestrator running when wheeld restarts comes back with the flags and brief the
/// restarted wheeld builds, never ones kept from its first launch.
#[tokio::test]
async fn a_restarted_global_orchestrator_launches_with_the_current_flags_and_brief() {
    use wheel_proto::OrchestratorScope::Global;
    let dir = Scratch::new("orchestrator-relaunch");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let mut ws = connect(&url).await;
    let (pane, agent) = orchestrator(&mut ws, 330, None, Global).await.unwrap();
    attach_and_send(&mut ws, &agent, "say hi").await;
    turn_text(&mut ws).await;
    drop(ws);
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (url, _, _second) = start_daemon_with(fake, Some(state_file.clone())).await;
    let saved = layout(&url).await;
    let restored = pane_in(&saved, &pane).unwrap_or_else(|| panic!("{saved}"));
    assert_eq!(restored["orchestrator"], "global", "{restored}");
    let agent = restored["agent"].as_str().unwrap().to_string();
    let mut ws = connect(&url).await;
    attach_and_send(&mut ws, &agent, "show orchestrator").await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("allowed=Bash(wheel:*) denied=-\n"), "{text:?}");
    assert!(text.contains("You are an orchestrator: an agent pane that dispatches"), "{text:?}");
}

/// Orchestrator is owned by one connection at a time: a claim takes it from the holder, every connection
/// hears each change, a release clears it, a connection that is not the owner cannot release, and
/// a closed connection gives it up.
#[tokio::test(flavor = "multi_thread")]
async fn one_connection_owns_orchestrator_at_a_time() {
    let url = start_daemon().await;
    let mut a = connect(&url).await;
    let mut b = connect(&url).await;

    send(&mut a, ClientMsg::OrchestratorClaim { req: Some(90) }).await;
    let a_id = loop {
        if let ServerMsg::OrchestratorClaimed { req: 90, owner } = recv(&mut a).await {
            break owner;
        }
    };
    let heard = loop {
        if let ServerMsg::OrchestratorLease { owner } = recv(&mut b).await {
            break owner;
        }
    };
    assert_eq!(heard, Some(a_id.clone()), "the other connection hears who owns orchestrator");

    send(&mut b, ClientMsg::OrchestratorRelease { req: Some(91) }).await;
    loop {
        if let ServerMsg::Error { req: Some(91), message, .. } = recv(&mut b).await {
            assert!(message.contains("does not own orchestrator"), "{message:?}");
            break;
        }
    }

    send(&mut b, ClientMsg::OrchestratorClaim { req: Some(92) }).await;
    let b_id = loop {
        if let ServerMsg::OrchestratorClaimed { req: 92, owner } = recv(&mut b).await {
            break owner;
        }
    };
    assert_ne!(a_id, b_id);
    let taken = loop {
        if let ServerMsg::OrchestratorLease { owner } = recv(&mut a).await {
            if owner == Some(b_id.clone()) {
                break owner;
            }
        }
    };
    assert_eq!(taken, Some(b_id));

    // A closed connection gives orchestrator up.
    drop(b);
    let freed = loop {
        if let ServerMsg::OrchestratorLease { owner } = recv(&mut a).await {
            if owner.is_none() {
                break owner;
            }
        }
    };
    assert_eq!(freed, None);

    send(&mut a, ClientMsg::OrchestratorClaim { req: Some(93) }).await;
    loop {
        if let ServerMsg::OrchestratorClaimed { req: 93, .. } = recv(&mut a).await {
            break;
        }
    }
    send(&mut a, ClientMsg::OrchestratorRelease { req: Some(94) }).await;
    loop {
        if let ServerMsg::Ok { req: 94 } = recv(&mut a).await {
            break;
        }
    }
}

/// The inbox of the orchestrator in `pane`.
async fn inbox(ws: &mut Ws, pane: &str, req: u64) -> Vec<wheel_proto::InboxItem> {
    send(ws, ClientMsg::InboxList { req: Some(req), pane: pane.into(), all: false }).await;
    loop {
        if let ServerMsg::InboxItems { req: r, items } = recv(ws).await {
            if r == req {
                return items;
            }
        }
    }
}

/// The orchestrator's transcript as one string, to search.
async fn transcript_text(url: &str, agent: &str) -> String {
    let mut peek = connect(url).await;
    format!("{:?}", attach(&mut peek, agent, 500).await)
}

/// Agent events queue in the orchestrator's inbox (wheel-eg4e.13): a child agent finishing adds an
/// item and sends the orchestrator nothing, mid-turn or idle (wheel-eg4e.19.28); a listing marks
/// the item told; a repeat is the same item; a reply reaches the item's agent and acks the item;
/// another space's agents never get in.
#[tokio::test(flavor = "multi_thread")]
async fn agent_events_queue_in_the_orchestrators_inbox() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (space_id, pane) = space(&url, "/tmp").await;
    let (_, other_pane) = space(&url, "/tmp").await;
    let (orchestrator_pane, orchestrator) =
        orchestrator(&mut ws, 95, Some(&space_id), wheel_proto::OrchestratorScope::Space).await.unwrap();
    // The orchestrator is mid-conversation: a turn that takes 6 s.
    let mut talk = connect(&url).await;
    attach_and_send(&mut talk, &orchestrator, "hold first 6000").await;
    let mine = start_agent(&url, &pane, Some("hello")).await;
    let _theirs = start_agent(&url, &other_pane, Some("hello")).await;

    let mut items = Vec::new();
    for _ in 0..50 {
        items = inbox(&mut ws, &orchestrator_pane, 96).await;
        if !items.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(items.len(), 1, "only this space's agent: {items:?}");
    let item = items[0].clone();
    assert_eq!((item.agent.as_str(), item.kind, item.count), (mine.as_str(), wheel_proto::InboxKind::Finished, 1));
    assert_eq!(item.text, format!("{mine} finished its turn"));
    assert!(!item.told);

    // 1. Mid-turn, nothing reaches the orchestrator.
    let seen = transcript_text(&url, &orchestrator).await;
    assert!(seen.contains("hold first 6000") && !seen.contains("Inbox:"), "{seen}");

    // 2. Its turn over and idle well past the old 15 s pull, it is still sent nothing: it reads the inbox.
    let mut talk_done = connect(&url).await;
    transcript_when(&mut talk_done, &orchestrator, |events| format!("{events:?}").contains("echo: hold first 6000")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let seen = transcript_text(&url, &orchestrator).await;
    assert!(!seen.contains("Inbox:"), "wheeld wrote into the orchestrator's chat: {seen}");
    assert!(inbox(&mut ws, &orchestrator_pane, 97).await[0].told, "the first listing marked the item told");

    // 3. The same event again is the same item.
    send(&mut ws, ClientMsg::AgentSend { req: Some(98), agent: mine.clone(), text: "hello again".into() }).await;
    let mut items = Vec::new();
    for _ in 0..50 {
        items = inbox(&mut ws, &orchestrator_pane, 99).await;
        if items.first().is_some_and(|i| i.count == 2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!((items[0].id, items[0].count), (item.id, 2));

    // 4. A reply goes to the item's agent and acks the item.
    send(&mut ws, ClientMsg::InboxReply { req: Some(100), pane: orchestrator_pane.clone(), id: item.id, text: "reply from the inbox".into() }).await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::AgentQueued { req: 100, agent, .. } => {
                assert_eq!(agent, mine);
                break;
            }
            ServerMsg::Error { req: Some(100), message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    let mut watcher = connect(&url).await;
    transcript_when(&mut watcher, &mine, |events| format!("{events:?}").contains("echo: reply from the inbox")).await;
    let left = inbox(&mut ws, &orchestrator_pane, 101).await;
    assert!(left.iter().all(|i| i.id != item.id), "{left:?}");

    // An ack of an id that names no item is refused.
    send(&mut ws, ClientMsg::InboxAck { req: Some(102), pane: orchestrator_pane.clone(), ids: vec![999] }).await;
    loop {
        if let ServerMsg::Error { req: Some(102), message, .. } = recv(&mut ws).await {
            assert_eq!(message, "no inbox item 999");
            break;
        }
    }
}

/// What `agent_card` answered: Ok, or the error's message.
async fn card_reply(ws: &mut Ws, req: u64, agent: &str, pane: Option<&str>, artifact: Option<&str>) -> Result<(), String> {
    send(ws, ClientMsg::AgentCard { req: Some(req), agent: agent.into(), pane: pane.map(String::from), artifact: artifact.map(String::from) }).await;
    loop {
        match recv(ws).await {
            ServerMsg::Ok { req: r } if r == req => return Ok(()),
            ServerMsg::Error { req: Some(r), message, .. } if r == req => return Err(message),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_card_names_a_pane_or_an_artifact_and_lands_in_the_transcript() {
    let (url, addr, daemon) = start_daemon_with(env!("CARGO_BIN_EXE_wheel-fake-agent"), None).await;
    let dir = std::env::temp_dir().join(format!("wheel-card-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (space_id, pane) = space(&url, &dir.to_string_lossy()).await;
    let agent = start_agent(&url, &pane, None).await;
    let (_, other) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    artifact_reply(&mut ws, lived("board", None, None)).await.unwrap();

    card_reply(&mut ws, 801, &pane, Some(&other), None).await.unwrap();
    card_reply(&mut ws, 802, &agent, None, Some("board")).await.unwrap();
    let events = transcript_when(&mut ws, &agent, |events| events.iter().filter(|e| matches!(e, AgentEvent::Card { .. })).count() == 2).await;
    let cards: Vec<_> = events.into_iter().filter_map(|e| match e { AgentEvent::Card { card } => Some(card), _ => None }).collect();
    assert_eq!(cards[0], wheel_proto::CardRef::Pane { pane: other.clone() });
    let wheel_proto::CardRef::Artifact { id, name, scope, space } = &cards[1] else { panic!("{cards:?}") };
    assert_eq!((name.as_str(), *scope, space.as_deref()), ("board", wheel_proto::ArtifactScope::Project, Some(space_id.as_str())));
    // The card's page is served under its own id, as a web pane's is.
    std::fs::write(dir.join(".wheel/artifacts/board/index.html"), "<p>card page</p>").unwrap();
    let bearer = format!("Authorization: Bearer {TOKEN}\r\n");
    let (status, body) = http(addr, "GET", &format!("/artifacts/{id}/"), &bearer, b"").await;
    assert_eq!((status, body.as_str()), (200, "<p>card page</p>"));
    let (status, _) = http(addr, "GET", &format!("/artifacts/{id}/.wheel/wheel.css"), &bearer, b"").await;
    assert_eq!(status, 200);
    let (status, _) = http(addr, "GET", "/artifacts/k999999/", &bearer, b"").await;
    assert_eq!(status, 404);
    // The card's page is watched as a web pane's is: a change to its files reloads it.
    std::fs::write(dir.join(".wheel/artifacts/board/index.html"), "<p>changed</p>").unwrap();
    let reloaded = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let ServerMsg::PaneReload { pane } = recv(&mut ws).await {
                if pane == *id {
                    break;
                }
            }
        }
    })
    .await;
    assert!(reloaded.is_ok(), "no pane_reload for card page {id}");
    // An agent keeps the pages of its newest 8 artifact cards; the oldest ends.
    for n in 0..8 {
        card_reply(&mut ws, 810 + n, &agent, None, Some("board")).await.unwrap();
    }
    let (status, _) = http(addr, "GET", &format!("/artifacts/{id}/"), &bearer, b"").await;
    assert_eq!(status, 404, "the ninth card ends the first card's page");

    assert_eq!(card_reply(&mut ws, 803, &agent, Some("p999999"), None).await, Err("no pane p999999".into()));
    let missing = card_reply(&mut ws, 804, &agent, None, Some("nowhere")).await.unwrap_err();
    assert!(missing.starts_with("no artifact nowhere"), "{missing}");
    let both = card_reply(&mut ws, 805, &agent, Some(&other), Some("board")).await.unwrap_err();
    assert_eq!(both, "a card names exactly one of a pane and an artifact");
    assert!(card_reply(&mut ws, 806, &agent, None, None).await.is_err());
    daemon.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_keeps_the_complete_resolved_sandbox_and_inherits_the_reviewer() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach(&mut ws, &agent, 1).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("\"networkAccess\":true"), "{text}");
    assert!(text.contains("/tmp/wheel-extra-root"), "{text}");
    assert!(text.contains("\"excludeSlashTmp\":true"), "{text}");
    let options = config_of(&mut ws, &agent, 2).await;
    assert_eq!(current(&options, "reviewer"), "auto_review");
}

/// Set through the public socket and wait for the driver's acknowledgement.
async fn codex_config_ok(ws: &mut Ws, agent: &str, req: u64, config: &str, value: &str) {
    let answer = client_op(ws, req, ClientMsg::AgentConfigSet {
        req: Some(req), agent: agent.into(), config: config.into(), value: value.into(),
    }).await;
    assert!(matches!(answer, ServerMsg::Ok { .. }), "{answer:?}");
}

fn setup_permissions(text: &str) -> serde_json::Value {
    let json = text.split(" | ").find_map(|line| line.strip_prefix("permissions: ")).expect(text);
    serde_json::from_str(json).unwrap()
}

/// Simulate a settings update written by a native Codex process while Wheel is not its owner.
fn edit_codex_permissions(home: &std::path::Path, thread: &str, edit: impl FnOnce(&mut serde_json::Value)) -> serde_json::Value {
    use std::io::Write as _;
    let rollout = std::fs::read_dir(home.join("sessions/2026/01/01")).unwrap().flatten()
        .map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(&format!("-{thread}.jsonl"))).unwrap();
    let text = std::fs::read_to_string(&rollout).unwrap();
    let mut settings = text.lines().rev().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find_map(|e| (e["payload"]["type"] == "permissions").then(|| e["payload"]["settings"].clone())).unwrap();
    edit(&mut settings);
    let entry = serde_json::json!({"type": "event_msg", "payload": {"type": "permissions", "settings": settings}});
    writeln!(std::fs::OpenOptions::new().append(true).open(rollout).unwrap(), "{entry}").unwrap();
    settings
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_reviewer_and_policy_are_independent_and_sandbox_changes_are_resolved() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach(&mut ws, &agent, 1).await;
    config_set_refused(&mut ws, &agent, 2, "reviewer", "guardian_subagent", "does not take").await;
    config_set_refused(&mut ws, &agent, 3, "mode", "untrusted", "does not take").await;
    codex_config_ok(&mut ws, &agent, 4, "reviewer", "user").await;
    let options = config_of(&mut ws, &agent, 5).await;
    assert_eq!(current(&options, "reviewer"), "user");
    assert_eq!(current(&options, "mode"), "on-request");
    codex_config_ok(&mut ws, &agent, 6, "mode", "never").await;
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["approvalsReviewer"], "user");
    assert_eq!(permissions["approvalPolicy"], "never");
    let thread = layout(&url).await["spaces"][0]["root"]["conversation"].as_str().unwrap().to_string();
    codex_config_ok(&mut ws, &agent, 7, "sandbox", "read-only").await;
    // The sandbox selection is durable even when the person switches chats before another turn.
    switch_of(&mut ws, &agent, 90, None).await.unwrap();
    switch_of(&mut ws, &agent, 91, Some(&thread)).await.unwrap();
    let read = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(read["sandbox"], serde_json::json!({"type": "readOnly", "networkAccess": false}));
    codex_config_ok(&mut ws, &agent, 8, "sandbox", "workspace-write").await;
    let expected = serde_json::json!({"type": "workspaceWrite", "networkAccess": true,
        "writableRoots": ["/tmp/wheel-extra-root"], "excludeTmpdirEnvVar": true, "excludeSlashTmp": true});
    for _ in 0..2 {
        let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
        assert_eq!(permissions["sandbox"], expected);
        assert_eq!(permissions["approvalsReviewer"], "user");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_reviewer_survives_resume_process_restart_and_pane_persistence() {
    let dir = Scratch::new("codex-reviewer-persistence");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    let agent = codex_agent(&url).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 1).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert_eq!(setup_permissions(&text)["approvalsReviewer"], "auto_review");
    let thread = saved_conversation(&state_file).unwrap();
    // Change the reviewer without another turn: Codex must save it immediately.
    codex_config_ok(&mut ws, &agent, 2, "reviewer", "user").await;
    switch_of(&mut ws, &agent, 3, None).await.unwrap();
    let options = config_of(&mut ws, &agent, 4).await;
    assert_eq!(current(&options, "reviewer"), "auto_review", "a new chat inherits its default reviewer");
    codex_config_ok(&mut ws, &agent, 8, "mode", "never").await;
    reply(&mut ws, &agent, "show setup").await;
    switch_of(&mut ws, &agent, 5, Some(&thread)).await.unwrap();
    let options = config_of(&mut ws, &agent, 6).await;
    assert_eq!(current(&options, "reviewer"), "user");
    assert_eq!(current(&options, "mode"), "on-request", "resume must not copy another chat's policy");
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert_eq!(saved["spaces"][0]["root"]["approvals_reviewer"], "user");
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stopped: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert_eq!(stopped["spaces"][0]["root"]["approvals_reviewer"], "user", "{stopped}");
    let (url, _, second) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 7).await;
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["approvalsReviewer"], "user");
    assert_eq!(permissions["approvalPolicy"], "on-request");
    second.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_inherits_granular_policy_and_preserves_named_permission_profiles() {
    let dir = Scratch::new("codex-profile-inheritance");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, Some(state_file.clone())).await;
    std::fs::write(dir.0.join("codex-home/config.toml"), r#"
approvals_reviewer = "user"
default_permissions = "restricted"
[approval_policy.granular]
sandbox_approval = true
rules = false
mcp_elicitations = true
request_permissions = false
skill_approval = true
"#).unwrap();
    let agent = codex_agent(&url).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 1).await;
    let options = config_of(&mut ws, &agent, 2).await;
    assert_eq!(current(&options, "reviewer"), "user");
    assert_eq!(current(&options, "permissions"), "restricted");
    assert!(!options.iter().any(|o| o.id == "sandbox"));
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["activePermissionProfile"]["id"], "restricted");
    assert_eq!(permissions["approvalPolicy"]["granular"]["rules"], false);
    assert_eq!(permissions["approvalPolicy"]["granular"]["sandbox_approval"], true);
    let thread = saved_conversation(&state_file).unwrap();
    let config_file = dir.0.join("codex-home/config.toml");
    let config = std::fs::read_to_string(&config_file).unwrap();
    std::fs::write(&config_file, config.replace("default_permissions = \"restricted\"\n", "")).unwrap();
    switch_of(&mut ws, &agent, 4, None).await.unwrap();
    let options = config_of(&mut ws, &agent, 5).await;
    assert!(options.iter().any(|o| o.id == "sandbox"));
    assert!(!options.iter().any(|o| o.id == "permissions"));
    assert!(layout(&url).await["spaces"][0]["root"]["permission_profile"].is_null());
    switch_of(&mut ws, &agent, 6, Some(&thread)).await.unwrap();
    let options = config_of(&mut ws, &agent, 7).await;
    assert_eq!(current(&options, "permissions"), "restricted", "resume retains the destination's profile");
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    assert_eq!(saved["spaces"][0]["root"]["permission_profile"], "restricted");
    assert!(saved["spaces"][0]["root"]["sandbox"].is_null());
    daemon.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, again) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 3).await;
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["activePermissionProfile"]["id"], "restricted");
    assert!(permissions["approvalPolicy"]["granular"].is_object());
    again.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_keeps_a_trust_derived_policy_without_persisting_a_retired_explicit_choice() {
    let dir = Scratch::new("codex-derived-policy");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    std::fs::write(dir.0.join("codex-home/config.toml"), "fake_derived_untrusted = true\n").unwrap();
    let agent = codex_agent(&url).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 1).await;
    let options = config_of(&mut ws, &agent, 2).await;
    assert_eq!(current(&options, "mode"), "untrusted");
    let mode = options.iter().find(|o| o.id == "mode").unwrap();
    if let wheel_proto::ConfigKind::Select { choices, .. } = &mode.kind {
        assert!(!choices.iter().any(|c| c.value == "untrusted"));
    }
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["approvalPolicy"], "untrusted");
    assert!(layout(&url).await["spaces"][0]["root"]["permission_mode"].is_null());
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, second) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 3).await;
    let permissions = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(permissions["approvalPolicy"], "untrusted");
    second.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_resume_uses_complete_backend_settings_instead_of_cached_pane_modes() {
    let dir = Scratch::new("codex-native-settings-win");
    let state_file = dir.0.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, first) = start_daemon_with(fake, Some(state_file.clone())).await;
    std::fs::write(dir.0.join("codex-home/config.toml"), "default_permissions = \"before\"\n").unwrap();
    let agent = codex_agent(&url).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 1).await;
    reply(&mut ws, &agent, "show setup").await;
    let thread = saved_conversation(&state_file).unwrap();
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Another native view changed these settings after Wheel saved its pane. The runtime policy
    // deliberately differs from configured defaults, so replaying a mode string loses its fields.
    let expected = edit_codex_permissions(&dir.0.join("codex-home"), &thread, |settings| {
        settings["approvalPolicy"] = serde_json::json!("never");
        settings["approvalsReviewer"] = serde_json::json!("user");
        settings["activePermissionProfile"] = serde_json::Value::Null;
        settings["sandbox"] = serde_json::json!({"type": "workspaceWrite", "networkAccess": false,
            "writableRoots": ["/tmp/runtime-only-root"], "excludeTmpdirEnvVar": false, "excludeSlashTmp": false});
    });
    let (url, _, second) = start_daemon_with(fake, Some(state_file)).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &agent, |events| events.len() >= 4).await;
    let actual = setup_permissions(&reply(&mut ws, &agent, "show setup").await);
    assert_eq!(actual, expected);
    let pane = layout(&url).await["spaces"][0]["root"].clone();
    assert!(pane["permission_profile"].is_null(), "{pane}");
    assert_eq!(pane["sandbox"], "workspace-write");
    assert_eq!(pane["approvals_reviewer"], "user");
    second.abort();
}

/// Send `pane_reload` `msg` (its `req` is `req`) to a busy agent's pane; returns the turn its
/// reload waits in the queue as.
async fn reload_queued(ws: &mut Ws, msg: ClientMsg, req: u64) -> u64 {
    send(ws, msg).await;
    loop {
        match recv(ws).await {
            ServerMsg::AgentQueued { req: r, turn, .. } if r == req => return turn,
            ServerMsg::Ok { req: r } if r == req => panic!("reload {req} ran at once"),
            ServerMsg::Error { req: Some(r), message, .. } if r == req => panic!("reload {req} failed: {message}"),
            _ => {}
        }
    }
}

/// The agent `pane` names once it no longer names `old`, within 20 s.
async fn agent_after(url: &str, old: &str) -> String {
    for _ in 0..200 {
        let listed = layout(url).await;
        let named = listed["spaces"][0]["root"]["agent"].as_str().unwrap_or_default().to_string();
        if !named.is_empty() && named != old {
            return named;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the pane still names agent {old}");
}

/// A reload asked while a turn runs waits in the agent's queue as a turn of its own: taken back by
/// `agent_unsend` and `agent_clear_queue`, one at a time, and run at once by `agent_send_now`.
/// When its turn comes the agent starts again with the reload's values, and the message queued
/// behind it runs in the new agent; the one queued before it ran in the old.
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_asked_during_a_turn_waits_in_the_queue() {
    let dir = Scratch::new("agent-reload-queued");
    let (url, _state_file, agent) = agent_with_store(&dir.0).await;
    let pane = layout(&url).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 900).await;
    let home = dir.0.join("queued-home").display().to_string();
    let reload = |req: u64, set: &[(&str, &str)], unset: &[&str]| ClientMsg::PaneReload {
        req: Some(req),
        pane: pane.clone(),
        set: set.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        unset: unset.iter().map(|n| n.to_string()).collect(),
    };
    let with_home = |req: u64| reload(req, &[("CLAUDE_CONFIG_DIR", &home)], &[]);

    // Turn 1 waits on a permission; a reload asked now waits behind it, shown in the queue.
    queue(&mut ws, &agent, "needs permission", 901).await;
    let perm = next_perm(&mut ws).await;
    let turn = reload_queued(&mut ws, with_home(902), 902).await;
    let shown = attach(&mut ws, &agent, 903).await;
    let reloads: Vec<_> = shown
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Reload { turn, text } => Some((*turn, text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(reloads.len(), 1, "{shown:?}");
    assert_eq!(reloads[0].0, turn);
    assert!(
        reloads[0].1.starts_with("Reload with CLAUDE_CONFIG_DIR=") && reloads[0].1.ends_with("queued-home"),
        "{reloads:?}"
    );

    // One reload waits at a time; `agent_unsend` takes it back.
    let refused = ok_or_error(&mut ws, with_home(904), 904).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if message.contains("has a reload waiting")),
        "{refused:?}"
    );
    let unsend = ClientMsg::AgentUnsend { req: Some(905), agent: agent.clone(), turn };
    assert_eq!(unsend_and_retract(&mut ws, unsend, 905, turn).await, ServerMsg::Ok { req: 905 });

    // `agent_clear_queue` takes it back with the messages.
    let turn = reload_queued(&mut ws, with_home(906), 906).await;
    let clear = ClientMsg::AgentClearQueue { req: Some(907), agent: agent.clone() };
    assert_eq!(unsend_and_retract(&mut ws, clear, 907, turn).await, ServerMsg::Ok { req: 907 });
    let shown = attach(&mut ws, &agent, 908).await;
    assert!(!shown.iter().any(|e| matches!(e, AgentEvent::Reload { .. })), "{shown:?}");

    // A message before the reload and one after it. The turn ends, the queued message before the
    // reload runs in the old agent, then the pane's agent starts again with the picked home.
    queue(&mut ws, &agent, "show setup before", 909).await;
    reload_queued(&mut ws, with_home(910), 910).await;
    queue(&mut ws, &agent, "show setup after", 911).await;
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    // The message before the reload answers in the old agent, under the home it started with.
    // Its reply streams in chunks: read them until the line naming the home is whole.
    let mut streamed = String::new();
    let before = loop {
        if let ServerMsg::Agent { agent: from, event: StampedEvent { event: AgentEvent::Text { text }, .. }, .. } = recv(&mut ws).await {
            if from == agent {
                streamed.push_str(&text);
            }
        }
        if let Some((_, rest)) = streamed.split_once("setup: config_dir=") {
            if let Some((line, _)) = rest.split_once('\n') {
                break line.to_string();
            }
        }
    };
    assert!(!before.contains("queued-home"), "{before}");
    let reloaded = agent_after(&url, &agent).await;
    let listed = layout(&url).await;
    assert_eq!(listed["spaces"][0]["root"]["env"]["CLAUDE_CONFIG_DIR"], home.as_str(), "{listed}");
    // The message after it runs in the new agent, under the picked home.
    let events = transcript_when(&mut ws, &reloaded, |events| {
        events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("setup: config_dir=")))
    })
    .await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::User { text, .. } if text == "show setup after")),
        "{events:?}"
    );
    let after = events.iter().find_map(|e| match e {
        AgentEvent::Text { text } if text.contains("setup: config_dir=") && text.contains("show setup after") => Some(text.as_str()),
        _ => None,
    });
    assert!(after.is_some_and(|t| t.contains(&format!("config_dir={home}\n"))), "{events:?}");

    // `agent_send_now` runs a queued reload at once: the turn running is interrupted. A message
    // sent before the reload is never lost: an agent that takes messages mid-turn (as Claude Code
    // does) holds it and answers it before the reload, else it goes to the new agent.
    attach(&mut ws, &reloaded, 912).await;
    queue(&mut ws, &reloaded, "needs permission", 913).await;
    next_perm(&mut ws).await;
    queue(&mut ws, &reloaded, "ahead of the reload", 916).await;
    let turn = reload_queued(&mut ws, reload(914, &[], &["CLAUDE_CONFIG_DIR"]), 914).await;
    let now = ClientMsg::AgentSendNow { req: Some(915), agent: reloaded.clone(), turn };
    assert_eq!(ok_or_error(&mut ws, now, 915).await, ServerMsg::Ok { req: 915 });
    let again = agent_after(&url, &reloaded).await;
    assert_ne!(again, agent);
    let listed = layout(&url).await;
    assert!(listed["spaces"][0]["root"].get("env").is_none(), "{listed}");
    let mut old_stream = String::new();
    while let Ok(msg) = tokio::time::timeout(Duration::from_millis(300), recv(&mut ws)).await {
        if let ServerMsg::Agent { agent: from, event: StampedEvent { event: AgentEvent::Text { text }, .. }, .. } = msg {
            if from == reloaded {
                old_stream.push_str(&text);
            }
        }
    }
    if !old_stream.contains("echo: ahead of the reload") {
        transcript_when(&mut ws, &again, |events| {
            events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: ahead of the reload")))
        })
        .await;
    }
}

/// Sent now while a message waits ahead of it, a queued reload takes that message to the new
/// agent: Codex queues a message sent during a turn rather than taking it in, so it is still in the
/// queue when the reload's turn comes.
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_sent_now_takes_the_messages_ahead_of_it_to_the_new_agent() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (_, pane) = space(&url, "/tmp").await;
    let agent = start_agent_as(&url, &pane, None, Some("codex")).await;
    attach(&mut ws, &agent, 1).await;
    queue(&mut ws, &agent, "permission", 2).await;
    perm_from(&mut ws, &agent).await;
    queue(&mut ws, &agent, "ahead", 3).await;
    let reload = ClientMsg::PaneReload { req: Some(4), pane: pane.clone(), set: Default::default(), unset: Vec::new() };
    let turn = reload_queued(&mut ws, reload, 4).await;
    let now = ClientMsg::AgentSendNow { req: Some(5), agent: agent.clone(), turn };
    assert_eq!(ok_or_error(&mut ws, now, 5).await, ServerMsg::Ok { req: 5 });
    let mut again = None;
    for _ in 0..200 {
        let listed = layout(&url).await;
        let named = listed["spaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["root"]["id"] == pane.as_str())
            .and_then(|s| s["root"]["agent"].as_str())
            .map(str::to_string);
        if named.as_deref().is_some_and(|n| n != agent) {
            again = named;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let again = again.expect("the pane names a new agent");
    transcript_when(&mut ws, &again, |events| {
        events.iter().any(|e| matches!(e, AgentEvent::Text { text } if text.contains("echo: ahead")))
    })
    .await;
}

/// `pane_reload` on an agent pane queues while a turn runs; between turns it ends the agent and
/// starts a new one, under a new name, on the same conversation, which carries on.
#[tokio::test(flavor = "multi_thread")]
async fn a_reloaded_agent_pane_carries_on_its_conversation() {
    let dir = Scratch::new("agent-reload");
    let (url, state_file, agent) = agent_with_store(&dir.0).await;
    let pane = layout(&url).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = connect(&url).await;
    attach(&mut ws, &agent, 900).await;
    let reload = |req: u64| ClientMsg::PaneReload {
        req: Some(req),
        pane: pane.clone(),
        set: Default::default(),
        unset: Vec::new(),
    };

    // A turn waiting on a permission keeps the agent busy: the reload waits in the queue, and is
    // taken back.
    send(
        &mut ws,
        ClientMsg::AgentSend {
            req: Some(901),
            agent: agent.clone(),
            text: "needs permission".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    let turn = reload_queued(&mut ws, reload(902), 902).await;
    let unsend = ClientMsg::AgentUnsend { req: Some(910), agent: agent.clone(), turn };
    assert_eq!(unsend_and_retract(&mut ws, unsend, 910, turn).await, ServerMsg::Ok { req: 910 });
    send(
        &mut ws,
        ClientMsg::PermAnswer {
            req: None,
            id: perm.id,
            choice: "allow".into(),
        },
    )
    .await;
    transcript_when(&mut ws, &agent, |events| {
        events.iter().any(
            |e| matches!(e, AgentEvent::TurnEnd { stop_reason, .. } if stop_reason == "end_turn"),
        )
    })
    .await;
    let before = saved_conversation(&state_file);
    assert!(before.is_some(), "the first turn saved the conversation");

    // Between turns a new agent starts on the same conversation, and the pane names it.
    client_op(&mut ws, 903, reload(903)).await;
    let listed = layout(&url).await;
    let reloaded = listed["spaces"][0]["root"]["agent"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(reloaded, agent, "the pane names the new agent");
    attach(&mut ws, &reloaded, 904).await;
    let text = reply(&mut ws, &reloaded, "after reload").await;
    assert_eq!(text, "echo: after reload");
    let events = attach(&mut ws, &reloaded, 905).await;
    assert!(
        events.contains(&AgentEvent::TurnStart { turn: 2 }),
        "the second turn follows the first in one conversation: {events:?}"
    );
    assert_eq!(saved_conversation(&state_file), before);

    // `set` picks values the pane saves, its home among them; `unset` drops them again.
    let home = dir.0.join("picked-home").display().to_string();
    let with_env = |req: u64, set: &[(&str, &str)], unset: &[&str]| ClientMsg::PaneReload {
        req: Some(req),
        pane: pane.clone(),
        set: set.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        unset: unset.iter().map(|n| n.to_string()).collect(),
    };
    client_op(&mut ws, 906, with_env(906, &[("CLAUDE_CONFIG_DIR", &home), ("RELOAD_PICK_TEST", "x")], &[])).await;
    let listed = layout(&url).await;
    let root = &listed["spaces"][0]["root"];
    assert_eq!(root["env"]["CLAUDE_CONFIG_DIR"], home.as_str(), "{root}");
    assert_eq!(root["env"]["RELOAD_PICK_TEST"], "x", "{root}");
    let picked = root["agent"].as_str().unwrap().to_string();
    attach(&mut ws, &picked, 907).await;
    assert_eq!(reply(&mut ws, &picked, "in the new home").await, "echo: in the new home");
    client_op(&mut ws, 908, with_env(908, &[], &["CLAUDE_CONFIG_DIR", "RELOAD_PICK_TEST"])).await;
    let listed = layout(&url).await;
    assert!(listed["spaces"][0]["root"].get("env").is_none(), "{listed}");
}

/// Every space as `status` lists it.
async fn spaces_now(ws: &mut Ws) -> Vec<wheel_proto::SpaceInfo> {
    send(ws, ClientMsg::Status { req: Some(97) }).await;
    loop {
        if let ServerMsg::Status { req: Some(97), spaces, .. } = recv(ws).await {
            return spaces;
        }
    }
}

/// Make a mirror of `source` in `space`; the daemon's reply as it came.
async fn mirror_reply(ws: &mut Ws, req: u64, source: &str, space: &str) -> Result<String, String> {
    send(ws, ClientMsg::PaneMirror { req: Some(req), source: source.into(), space: space.into() }).await;
    loop {
        match recv(ws).await {
            ServerMsg::Created { req: r, id, .. } if r == req => return Ok(id),
            ServerMsg::Error { req: Some(r), message, .. } if r == req => return Err(message),
            _ => {}
        }
    }
}

async fn set_kind(ws: &mut Ws, req: u64, pane: &str, kind: wheel_proto::NewPaneKind, url: Option<&str>) {
    client_op(
        ws,
        req,
        ClientMsg::PaneSetKind {
            home: None,
            req: Some(req),
            pane: pane.into(),
            kind,
            artifact: None,
            scope: None,
            url: url.map(String::from),
            plugin: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mirror_shows_its_terminal_source_takes_input_and_closes_with_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (_, source) = space(&url, "/tmp").await;
    set_kind(&mut ws, 1, &source, wheel_proto::NewPaneKind::Terminal, None).await;
    let wheel_proto::PaneKind::Terminal { term, .. } = pane_kind(&mut ws, &source).await else {
        panic!("not a terminal")
    };
    term_attach(&mut ws, &term).await;
    type_line(&mut ws, &term, "echo before-mirror").await;
    output_until(&mut ws, "before-mirror\r\n", 30).await;

    // Case 1: the mirror goes in another space's lone empty pane and shows the source's screen.
    let (other, empty) = space(&url, "/tmp").await;
    let mirror = mirror_reply(&mut ws, 2, &source, &other).await.unwrap();
    assert_eq!(mirror, empty, "the mirror takes the space's lone empty pane");
    let spaces = spaces_now(&mut ws).await;
    let (shown, kind) = wheel_proto::shown_pane(&spaces, &mirror).unwrap();
    assert_eq!(shown, source);
    let wheel_proto::PaneKind::Terminal { term: through, .. } = kind else { panic!("{kind:?}") };
    assert_eq!(through, &term);
    let mut viewer = connect(&url).await;
    term_attach(&mut viewer, through).await;
    assert!(output_until(&mut viewer, "before-mirror", 30).await.contains("before-mirror"));

    // Case 2 and 3: typed through the mirror, the line reaches the source, and both show its output.
    type_line(&mut viewer, through, "echo typed-in-mirror").await;
    assert!(output_until(&mut ws, "typed-in-mirror\r\n", 30).await.contains("typed-in-mirror"));
    assert!(output_until(&mut viewer, "typed-in-mirror\r\n", 30).await.contains("typed-in-mirror"));

    // Case 7: the listing carries the source and nothing else.
    let layout = layout(&url).await;
    let leaf = layout["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == other.as_str())
        .unwrap()["root"]
        .clone();
    assert_eq!(leaf["kind"], "mirror");
    assert_eq!(leaf["source"], source.as_str());
    let fields: Vec<&str> = leaf.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(fields, ["id", "kind", "node", "source"], "a mirror holds no screen, transcript or page");

    // A mirror of a mirror is refused.
    let refused = mirror_reply(&mut ws, 3, &mirror, &other).await.unwrap_err();
    assert!(refused.contains("is a mirror"), "{refused}");

    // Case 8: closing the source closes the mirror.
    client_op(&mut ws, 4, ClientMsg::PaneClose { req: Some(4), pane: source.clone() }).await;
    let spaces = spaces_now(&mut ws).await;
    assert!(
        spaces.iter().all(|s| !s.site_of(&mirror).is_some()),
        "the mirror closed with its source"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mirror_of_an_agent_streams_its_transcript_and_sends_and_answers_for_it() {
    let url = start_daemon().await;
    let (source, agent) = agent_pane(&url, Some("hello")).await;
    let (other, _) = space(&url, "/tmp").await;
    let mut ws = connect(&url).await;
    let mirror = mirror_reply(&mut ws, 1, &source, &other).await.unwrap();

    // Case 4: attached by the mirror's id, the transcript is the source agent's.
    let events = attach(&mut ws, &mirror, 2).await;
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::TurnEnd { turn: 1, .. })),
        "{events:?}"
    );
    send(&mut ws, ClientMsg::AgentSend { req: Some(5), agent: mirror.clone(), text: "again".into() }).await;
    let text = turn_text(&mut ws).await;
    assert!(!text.is_empty(), "the source agent answered a message sent to its mirror");

    // Case 5: a permission request answered through the mirror answers the source's.
    send(&mut ws, ClientMsg::AgentSend { req: Some(3), agent: mirror.clone(), text: "needs permission".into() }).await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.agent, agent);
    client_op(&mut ws, 4, ClientMsg::PermAnswer { req: Some(4), id: perm.id.clone(), choice: "allow".into() }).await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("permission: allow"), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mirror_of_a_browser_pane_resolves_to_its_page() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (_, source) = space(&url, "/tmp").await;
    set_kind(&mut ws, 1, &source, wheel_proto::NewPaneKind::Browser, Some("https://example.com/")).await;
    let (other, first) = space(&url, "/tmp").await;
    set_kind(&mut ws, 2, &first, wheel_proto::NewPaneKind::Terminal, None).await;
    // Case 6: in a space that holds a pane already, the mirror splits it and shows the same page.
    let mirror = mirror_reply(&mut ws, 3, &source, &other).await.unwrap();
    assert_ne!(mirror, first);
    let spaces = spaces_now(&mut ws).await;
    let (shown, kind) = wheel_proto::shown_pane(&spaces, &mirror).unwrap();
    assert_eq!(shown, source);
    assert!(matches!(kind, wheel_proto::PaneKind::Browser { url, .. } if url == "https://example.com/"), "{kind:?}");
    // A pane cannot mirror itself, even as its space's lone empty pane.
    let (lone_space, lone) = space(&url, "/tmp").await;
    let refused = mirror_reply(&mut ws, 5, &lone, &lone_space).await.unwrap_err();
    assert!(refused.contains("cannot mirror itself"), "{refused}");
    // Closing the source's whole space takes the mirror too.
    let source_space = spaces.iter().find(|s| s.site_of(&source).is_some()).unwrap().id.clone();
    client_op(&mut ws, 4, ClientMsg::SpaceClose { req: Some(4), id: source_space, if_empty: false }).await;
    let spaces = spaces_now(&mut ws).await;
    assert!(spaces.iter().all(|s| !s.site_of(&mirror).is_some()));
    assert!(spaces.iter().any(|s| s.site_of(&first).is_some()), "the space's other pane stays");
}
