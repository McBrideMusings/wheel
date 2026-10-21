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
    ClientMsg, ServerMsg, SessionEvent, SessionState, StampedEvent, StartKind, ToolContent,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
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

/// Collect an agent's reply text until the turn ends.
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
            } => panic!("agent error: {message}"),
            _ => {}
        }
    }
}

/// Send a client op carrying `req` and return its reply: `Created`, `Ok` or `Started`. It drops
/// every other message on the way, broadcasts included, so a test that goes on to wait for a
/// broadcast the op causes sends with `send` and reads both in one loop instead.
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
            let pane = spaces.into_iter().find(|s| s.id == id).unwrap().focus;
            return (id, pane);
        }
    }
}

/// Start an agent in `pane` with `message` as its first message; returns its session name once it
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
        },
    )
    .await;
    match started {
        ServerMsg::Started {
            session: Some(session),
            state: Some(SessionState::Ready | SessionState::Busy { .. }),
            ..
        } => session,
        other => panic!("{other:?}"),
    }
}

/// A space in `/tmp` holding one agent pane; returns the pane id and the session name. With a
/// `message`, the agent's first turn has finished when this returns.
async fn agent_pane(url: &str, message: Option<&str>) -> (String, String) {
    let (_, pane) = space(url, "/tmp").await;
    let session = start_agent(url, &pane, message).await;
    if message.is_some() {
        let mut ws = connect(url).await;
        transcript_when(&mut ws, &session, |events| {
            events.contains(&SessionEvent::TurnEnd {
                turn: 1,
                stop_reason: "end_turn".into(),
            })
        })
        .await;
    }
    (pane, session)
}

/// A new space with an agent pane whose agent is ready; returns the session name.
async fn ready_agent(url: &str) -> String {
    agent_pane(url, None).await.1
}

/// The layout as `status` lists it, every agent pane carrying its session's state.
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
        ClientMsg::SessionAttach {
            req: Some(1),
            session: agent.clone(),
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
            session: agent.clone(),
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
    assert!(text.contains(&format!("(session {agent})")), "{text:?}");
    assert!(text.contains("seat: You are the agent in pane"), "{text:?}");

    // A client that attaches afterwards gets the whole conversation, chunks merged.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::SessionAttach {
            req: Some(3),
            session: agent.clone(),
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
    let agent = ready_agent(&url).await;
    send(
        &mut asker,
        ClientMsg::SessionAttach {
            req: Some(1),
            session: agent.clone(),
        },
    )
    .await;
    while !matches!(recv(&mut asker).await, ServerMsg::SessionAttached { .. }) {}
    send(
        &mut asker,
        ClientMsg::SessionSend {
            session: agent.clone(),
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
    assert_eq!(perm.session, agent);
    assert_eq!(perm.title, "rm -rf build");
    // The agent suggested a rule, so the request can also be allowed for good.
    let offered: Vec<&str> = perm.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(offered, ["allow", "allow_always", "deny"]);

    // The request names the tool call the agent announced just before asking. Session events and
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
async fn a_queued_message_shows_at_once_and_starts_after_the_running_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    // Turn 1 waits on a permission answer, so turn 2 queues behind it.
    attach_and_send(&mut ws, &agent, "needs permission").await;
    let perm = next_perm(&mut ws).await;
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: agent.clone(),
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
            session: agent.clone(),
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
    let agent = ready_agent(&url).await;
    let cancel = |req| ClientMsg::SessionCancel {
        req: Some(req),
        session: agent.clone(),
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
                assert_eq!(message, format!("session {agent} has no turn to cancel"));
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
        ClientMsg::SessionSend {
            session: agent.clone(),
            req: None,
            text: "second".into(),
        },
    )
    .await;
    send(&mut ws, cancel(21)).await;
    // The agent stops the turn waiting on the permission request. The reply and the session events
    // come from different tasks, so the reply has no place in the order.
    let mut order = Vec::new();
    let mut replied = false;
    while order.last() != Some(&"turn_end 2 end_turn".to_string()) || !replied {
        match recv(&mut ws).await {
            ServerMsg::Session { event, .. } => match event.event {
                SessionEvent::TurnStart { turn } => order.push(format!("turn_start {turn}")),
                SessionEvent::TurnEnd { turn, stop_reason } => {
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

async fn attach_and_send(ws: &mut Ws, agent: &str, text: &str) {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(10),
            session: agent.into(),
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
            session: agent.into(),
            req: Some(11),
            text: text.into(),
        },
    )
    .await;
}

/// The session's config options as the daemon last had them, read through an attach.
async fn config_of(ws: &mut Ws, agent: &str, req: u64) -> Vec<wheel_proto::ConfigOption> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: agent.into(),
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

async fn send_config_set(ws: &mut Ws, agent: &str, req: u64, config: &str, value: &str) {
    send(
        ws,
        ClientMsg::SessionConfigSet {
            req: Some(req),
            session: agent.into(),
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
            ServerMsg::SessionConfig { session, options } if session == agent => {
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
        ClientMsg::SessionAttach {
            req: Some(req),
            session: agent.into(),
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

#[tokio::test(flavor = "multi_thread")]
async fn a_thought_streams_in_chunks_and_is_stored_whole() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "reflect").await;
    let mut live = Vec::new();
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session {
                event:
                    StampedEvent {
                        event: event @ SessionEvent::Thought { .. },
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
    ];
    assert_eq!(live, expected);

    // The stored transcript merges the thought chunks into one event.
    let mut late = connect(&url).await;
    send(
        &mut late,
        ClientMsg::SessionAttach {
            req: Some(3),
            session: agent.clone(),
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
        .filter(|e| matches!(e, SessionEvent::Thought { .. }))
        .collect();
    assert_eq!(
        stored,
        [SessionEvent::Thought {
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
            content: vec![diff("two\n")],
        },
        SessionEvent::ToolUpdate {
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
        ClientMsg::SessionAttach {
            req: Some(3),
            session: agent.clone(),
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
    let agent = ready_agent(&url).await;
    attach_and_send(&mut ws, &agent, "needs permission").await;
    let perm = loop {
        if let ServerMsg::PermRequested { perm } = recv(&mut ws).await {
            break perm;
        }
    };
    send(
        &mut ws,
        ClientMsg::SessionCancel {
            req: Some(12),
            session: agent.clone(),
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
        if let ServerMsg::Session {
            event:
                StampedEvent {
                    event: SessionEvent::TurnEnd { stop_reason, .. },
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
    let waited = session_wait(&mut ws, &agent, 30.0, 19).await;
    assert!(
        matches!(waited, ServerMsg::SessionWaited { ready: true, .. }),
        "{waited:?}"
    );
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: agent.clone(),
            req: Some(20),
            text: "back again".into(),
        },
    )
    .await;
    let text = turn_text(&mut ws).await;
    assert!(text.contains("echo: back again"), "{text:?}");
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

/// Waits short enough that a test never sleeps through a real backoff; five tries as in
/// production.
fn quick() -> wheeld::AgentTiming {
    let mut timing = wheeld::AgentTiming::default();
    timing.restart.backoff = Duration::from_millis(10);
    timing.restart.backoff_max = Duration::from_millis(40);
    timing
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
    let session = text.split_whitespace().nth(1).unwrap();
    let layout = layout(&url).await;
    let state = &layout["spaces"][0]["root"]["state"];
    assert_eq!(layout["spaces"][0]["root"]["session"], session, "{layout}");
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
    // The refused message was never queued.
    let events = attach(&mut ws, session, 2).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::User { text, .. } if text == "anyone?")),
        "a refused message was queued: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_was_ready_before_it_stopped_starts_its_count_over() {
    let mut timing = quick();
    timing.restart.tries = 2;
    let (url, _, _) =
        start_daemon_timed(env!("CARGO_BIN_EXE_wheel-fake-agent"), None, timing).await;
    let (_, session) = agent_pane(&url, Some("first task")).await;
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

/// An agent program that reads its input and never answers `initialize`, whatever arguments it
/// is given, so its session stays starting.
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
        session: Some(session),
        state: Some(SessionState::Starting),
        ..
    } = reply
    else {
        panic!("{reply:?}")
    };
    assert!(!session.is_empty());
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
        ClientMsg::SessionSend {
            req: Some(5),
            session: agent.clone(),
            text: "needs permission".into(),
        },
    )
    .await;

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
    // The answer reaches the agent; the other agent, attached on the same connection, hears
    // nothing.
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
    assert!(text.contains("permission: allow"), "{text:?}");

    // Two agents' requests wait side by side.
    for (session, req) in [(agent.as_str(), 3), (other.as_str(), 4)] {
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
    let first = next_perm(&mut ws).await;
    let second = next_perm(&mut ws).await;
    let (agent_perm, other_perm) = if first.session == agent {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(
        (agent_perm.session.as_str(), other_perm.session.as_str()),
        (agent.as_str(), other.as_str())
    );

    // Closing the agent's pane withdraws its request and leaves the other's.
    // The withdrawal can reach this connection before the close's own reply, so both are read in
    // one loop; `client_op` would drop the withdrawal on its way to the reply.
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

/// Attach to `session` until a notice is up over it; returns the notice and the transcript that
/// came with the attach.
async fn notice_when_up(ws: &mut Ws, session: &str) -> (String, Vec<SessionEvent>) {
    for req in 100..200 {
        let events = attach(ws, session, req).await;
        let up = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                if let ServerMsg::SessionNotice {
                    session: s,
                    notice: Some(notice),
                } = recv(ws).await
                {
                    if s == session {
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
    panic!("no notice came up over {session}");
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
    let (url, _, first_daemon) = start_daemon_with(command, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let session = start_agent(&url, &pane, Some("first task")).await;
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
    let pane = &saved["spaces"][0]["root"];
    assert_eq!(pane["session"], serde_json::json!(session));
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
    let (url, session) =
        restart_with_an_agent(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {}).await;

    // The agent resumed the conversation, and its transcript is the session's, as whole turns.
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
    // The transcript is read with no times, so none is invented.
    let replayed = attach_stamped(&mut ws, &session, 399).await;
    assert!(
        replayed.len() >= 4 && replayed.iter().all(|e| e.at.is_none()),
        "{replayed:?}"
    );
    // The conversation carries on, numbered after the resumed turn.
    let text = reply(&mut ws, &session, "after restart").await;
    assert_eq!(text, "echo: after restart");
    let events = attach(&mut ws, &session, 400).await;
    assert!(
        events.contains(&SessionEvent::TurnStart { turn: 2 }),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_load_that_fails_leaves_only_the_notice() {
    let dir = Scratch::new("failed-load");
    // The agent's store has lost this conversation.
    let store = dir.0.join("agent-home").join("projects");
    let (url, session) =
        restart_with_an_agent(env!("CARGO_BIN_EXE_wheel-fake-agent"), &dir.0, || {
            std::fs::remove_dir_all(&store).unwrap()
        })
        .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &session).await;
    assert_eq!(notice, wheel_proto::SESSION_NOT_REOPENED);
    assert_eq!(events, [], "the notice is never a transcript line");
    let text = reply(&mut ws, &session, "hello again").await;
    assert_eq!(text, "echo: hello again");
}

/// A daemon with its state file in `dir`, beside the store its agents keep their conversations
/// in, and one agent pane ready; returns its url, that file's path and the agent's session name.
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

/// The conversations `session`'s agent keeps for its directory.
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

/// Put `session` in a fresh conversation, or in the one `load` names; the agent's id for it, or
/// why it refused.
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
    let dir = Scratch::new("agent-store-sessions");
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
        events.contains(&SessionEvent::User {
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
        if let ServerMsg::SessionAttached {
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
        ClientMsg::SessionSend {
            req: Some(800),
            session: agent.clone(),
            text: "needs permission".into(),
        },
    )
    .await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.session, agent);
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
            |e| matches!(e, SessionEvent::TurnEnd { stop_reason, .. } if stop_reason == "end_turn"),
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
    agent_reply(&mut ws, &agent, "again").await;
    let listed = conversations_of(&mut ws, &agent, 604).await;
    let current: Vec<&str> = listed
        .iter()
        .filter(|c| c.current)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(current, [first.as_str()], "{listed:?}");
}

/// The next permission request `session` raises, skipping others'.
async fn perm_from(ws: &mut Ws, session: &str) -> wheel_proto::PermInfo {
    loop {
        let perm = next_perm(ws).await;
        if perm.session == session {
            return perm;
        }
    }
}

/// Send `session_wait` for `target` and return the reply: `session_waited` or an error.
async fn session_wait(ws: &mut Ws, target: &str, secs: f64, req: u64) -> ServerMsg {
    send(
        ws,
        ClientMsg::SessionWait {
            req: Some(req),
            session: target.into(),
            timeout_secs: secs,
        },
    )
    .await;
    loop {
        match recv(ws).await {
            reply @ ServerMsg::SessionWaited { req: r, .. } if r == req => return reply,
            reply @ ServerMsg::Error { req: Some(r), .. } if r == req => return reply,
            _ => {}
        }
    }
}

/// Send `session_cancel` for `target` and return the reply: `ok` or an error.
async fn session_cancel(ws: &mut Ws, target: &str, req: u64) -> ServerMsg {
    send(
        ws,
        ClientMsg::SessionCancel {
            req: Some(req),
            session: target.into(),
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
        ClientMsg::SessionSend {
            req: Some(req),
            session: target.into(),
            text: text.into(),
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_on_an_agent_pane_reports_its_state_by_pane_id_or_session_name() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, session) = agent_pane(&url, None).await;
    queue(&mut ws, &pane, "needs permission", 1).await;
    let perm = perm_from(&mut ws, &session).await;
    // Busy on a turn that waits for the person: the wait runs out and says what state it is in.
    let waited = session_wait(&mut ws, &pane, 0.3, 2).await;
    assert!(
        matches!(
            &waited,
            ServerMsg::SessionWaited {
                ready: false,
                state: SessionState::Busy { .. },
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
    for (target, req) in [(&session, 3), (&pane, 4)] {
        let waited = session_wait(&mut ws, target, 10.0, req).await;
        let ServerMsg::SessionWaited {
            session: s,
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
                session.as_str(),
                Some(pane.as_str()),
                SessionState::Ready,
                true
            )
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_an_agent_panes_turn_and_an_idle_cancel_is_an_error() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let (pane, session) = agent_pane(&url, None).await;
    let refused = session_cancel(&mut ws, &pane, 1).await;
    assert!(
        matches!(&refused, ServerMsg::Error { message, .. } if *message == format!("session {session} has no turn to cancel")),
        "{refused:?}"
    );
    queue(&mut ws, &session, "needs permission", 2).await;
    perm_from(&mut ws, &session).await;
    let cancelled = session_cancel(&mut ws, &session, 3).await;
    assert_eq!(cancelled, ServerMsg::Ok { req: 3 });
    let waited = session_wait(&mut ws, &pane, 10.0, 4).await;
    assert!(
        matches!(waited, ServerMsg::SessionWaited { ready: true, .. }),
        "{waited:?}"
    );
}

/// The data-loss guard: a load that fails must not overwrite the saved id, or one flaky start
/// would lose a conversation the agent still has.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pane_whose_saved_conversation_will_not_load_keeps_the_saved_id_and_says_so() {
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
    assert_eq!(notice, wheel_proto::SESSION_NOT_REOPENED);
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
    assert_eq!(notice, wheel_proto::SESSION_NOT_REOPENED);
    let mut watcher = connect(&url).await;
    send(
        &mut ws,
        ClientMsg::SessionNoticeDismiss {
            req: Some(710),
            session: agent.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionNotice {
            session,
            notice: None,
        } = recv(&mut watcher).await
        {
            assert_eq!(session, agent);
            break;
        }
    }
    attach(&mut ws, &agent, 711).await;
    let again = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if let ServerMsg::SessionNotice {
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
        session: Some(agent),
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
    let dir = Scratch::new("agent-sessions");
    let command = env!("CARGO_BIN_EXE_wheel-fake-agent").to_string();
    let state_file = dir.0.join("spaces.json");
    let (url, _, _) = start_daemon_with(&command, Some(state_file.clone())).await;
    let (_, session) = agent_pane(&url, Some("first task")).await;
    let mut ws = connect(&url).await;
    attach(&mut ws, &session, 899).await;
    let pane_conversation = || saved_conversation(&state_file);
    let first = pane_conversation().expect("the pane saved its conversation");
    let listed = conversations_of(&mut ws, &session, 900).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first);
    assert!(listed[0].current, "{listed:?}");

    let second = switch_of(&mut ws, &session, 901, None)
        .await
        .expect("a new conversation");
    assert_ne!(second, first);
    assert_eq!(pane_conversation(), None);
    assert_eq!(
        reply(&mut ws, &session, "second task").await,
        "echo: second task"
    );
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
    let agent = ready_agent(&url).await;
    let before = wheel_proto::unix_ms_now();
    send(
        &mut ws,
        ClientMsg::SessionAttach {
            req: Some(1),
            session: agent.clone(),
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

/// What `session_attached` says about `session`'s context use.
async fn attached_context(ws: &mut Ws, session: &str, req: u64) -> Option<wheel_proto::ContextUse> {
    send(
        ws,
        ClientMsg::SessionAttach {
            req: Some(req),
            session: session.into(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionAttached {
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
    let (url, session) = restart_with_an_agent(&command, &dir.0, || {}).await;

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
        attached_context(&mut ws, &session, 500).await,
        Some(restored)
    );

    // A compaction shrinks it, and every client hears so.
    let text = reply(&mut ws, &session, "context compacted").await;
    assert_eq!(text, "echo: context compacted");
    let compacted = wheel_proto::ContextUse {
        used: 8_000,
        size: 200_000,
    };
    assert_eq!(compacted.percent(), 4);
    assert_eq!(
        attached_context(&mut ws, &session, 501).await,
        Some(compacted)
    );

    // A new conversation starts empty.
    send(
        &mut ws,
        ClientMsg::SessionConversationNew {
            req: Some(502),
            session: session.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::SessionContext {
            session: s,
            context: None,
        } = recv(&mut ws).await
        {
            if s == session {
                break;
            }
        }
    }
    assert_eq!(attached_context(&mut ws, &session, 503).await, None);
}

/// Ask which view `pane` is in, switching it to `to` first when given: the view and the
/// conversation it shows, or the daemon's refusal.
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
    // session where the fake agent pane's store is: the same store, as the real two share one.
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
            req: Some(399),
            pane: pane.clone(),
            kind: wheel_proto::NewPaneKind::Terminal,
            artifact: None,
            scope: None,
            url: None,
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
    let PaneKind::Agent { session, place, .. } = pane_kind(&mut ws, &pane).await else {
        panic!("the pane is an agent pane")
    };
    // The space's own directory is not repeated; the config directory Claude Code used is kept.
    assert_eq!(
        place.map(|p| *p),
        Some(wheel_proto::AgentPlace {
            cwd: None,
            home: Some(home.to_string_lossy().into())
        })
    );
    let events = transcript_when(&mut ws, &session, |e| e.len() >= 4).await;
    assert_eq!(
        events[0],
        SessionEvent::User {
            turn: 1,
            text: "first from the terminal".into()
        },
        "{events:?}"
    );
    // While a turn runs the pane stays a chat, and says why.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(407),
            session: session.clone(),
            text: "work slowly 1".into(),
        },
    )
    .await;
    let refused = pane_view(&mut ws, 408, &pane, Some(PaneView::Terminal))
        .await
        .unwrap_err();
    assert!(refused.contains("working on a turn"), "{refused}");
    // Its reply carries the slow turn's echo too, which ran first.
    let answered = reply(&mut ws, &session, "second from the chat").await;
    assert!(
        answered.ends_with("echo: second from the chat"),
        "{answered}"
    );

    // Back in a terminal: Claude Code resumes the conversation, the chat's message in it.
    assert_eq!(
        pane_view(&mut ws, 404, &pane, Some(PaneView::Terminal)).await,
        Ok((Some(PaneView::Terminal), Some(ID.into())))
    );
    let (term, agent) = term_of(pane_kind(&mut ws, &pane).await);
    assert_eq!(agent.map(|a| a.session).as_deref(), Some(ID));
    term_attach(&mut ws, &term).await;
    let shown = output_until(&mut ws, &format!("claude-fake {ID} ready"), 30).await;
    assert!(
        shown.contains("> second from the chat") && shown.contains("> first from the terminal"),
        "{shown:?}"
    );
    // The agent session the chat ran in is gone with it.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(405),
            session: session.clone(),
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
    assert!(ended.contains(&session), "{ended}");

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
            req: Some(499),
            pane: pane.clone(),
            kind: wheel_proto::NewPaneKind::Terminal,
            artifact: None,
            scope: None,
            url: None,
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

    // Mid-turn the pane stays a terminal, and says why.
    type_line(&mut ws, &term, "work slowly").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let refused = pane_view(&mut ws, 501, &pane, Some(PaneView::Agent))
        .await
        .unwrap_err();
    assert!(refused.contains("middle of a turn"), "{refused}");
    assert!(
        matches!(pane_kind(&mut ws, &pane).await, PaneKind::Terminal { .. }),
        "a refused switch leaves the terminal"
    );
    output_until(&mut ws, "echo: work slowly", 10).await;

    // As a chat: Codex quits, the app-server loads the thread, and both exchanges are in it.
    assert_eq!(
        pane_view(&mut ws, 502, &pane, Some(PaneView::Agent)).await,
        Ok((Some(PaneView::Agent), Some(thread.clone())))
    );
    let PaneKind::Agent { session, agent, .. } = pane_kind(&mut ws, &pane).await else {
        panic!("the pane is an agent pane")
    };
    assert_eq!(agent, "codex");
    let events = transcript_when(&mut ws, &session, |e| e.len() >= 8).await;
    assert_eq!(
        events[0],
        SessionEvent::User {
            turn: 1,
            text: "first from the terminal".into()
        },
        "{events:?}"
    );
    assert!(
        events.contains(&SessionEvent::Text {
            text: "echo: work slowly".into()
        }),
        "{events:?}"
    );
    // While a turn runs the pane stays a chat, and says why.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(507),
            session: session.clone(),
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
        ClientMsg::SessionCancel {
            req: Some(509),
            session: session.clone(),
        },
    )
    .await;
    loop {
        if let ServerMsg::Session { event, .. } = recv(&mut ws).await {
            if matches!(event.event, SessionEvent::TurnEnd { .. }) {
                break;
            }
        }
    }
    let answered = reply(&mut ws, &session, "second from the chat").await;
    assert!(
        answered.ends_with("echo: second from the chat"),
        "{answered}"
    );

    // Back in a terminal: `codex resume` shows the thread, the chat's message in it.
    assert_eq!(
        pane_view(&mut ws, 504, &pane, Some(PaneView::Terminal)).await,
        Ok((Some(PaneView::Terminal), Some(thread.clone())))
    );
    let (term, agent) = term_of(pane_kind(&mut ws, &pane).await);
    let agent = agent.expect("the terminal runs Codex");
    assert_eq!(
        (agent.name.as_str(), agent.session.as_str()),
        ("codex", thread.as_str())
    );
    term_attach(&mut ws, &term).await;
    let shown = output_until(&mut ws, &format!("codex-fake {thread} ready"), 30).await;
    assert!(
        shown.contains("> second from the chat") && shown.contains("> first from the terminal"),
        "{shown:?}"
    );
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
    state.set_control(format!("ws://{addr}/ws"), dir.join("token"));
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
        req: Some(70),
        pane: false,
        name: name.into(),
        scope: Some(wheel_proto::ArtifactScope::Project),
        space: None,
        path: None,
        ends_with_pane: ends_with_pane.map(String::from),
        expires_in,
    }
}

async fn artifact_reply(ws: &mut Ws, msg: ClientMsg) -> Result<wheel_proto::ArtifactInfo, String> {
    send(ws, msg).await;
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

    let session = artifact_reply(&mut ws, lived("session", Some(&pane), None))
        .await
        .unwrap();
    assert_eq!(session.ends_with_pane.as_deref(), Some(pane.as_str()));
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
    for name in ["session", "timed", "soon", "plain"] {
        assert!(folder(name).is_dir(), "{name}");
    }

    // After its seconds, the expiring one goes and the rest stay.
    tokio::time::sleep(Duration::from_millis(4100)).await;
    daemon.sweep_artifacts();
    assert!(!folder("soon").exists());
    let entries = std::fs::read_to_string(dir.join("wheel.toml")).unwrap();
    assert!(!entries.contains("[artifacts.soon]"), "{entries}");
    for name in ["session", "timed", "plain"] {
        assert!(folder(name).is_dir(), "{name}");
    }

    // Closing the pane ends the session, and with it the artifact that asked to.
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
        if !folder("session").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!folder("session").exists());
    let entries = std::fs::read_to_string(dir.join("wheel.toml")).unwrap();
    assert!(!entries.contains("[artifacts.session]"), "{entries}");
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
        },
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    assert_eq!(layout(&url).await["spaces"][0]["root"]["agent"], "claude");
    // The saved layout holds it, so a restart runs the pane's own agent again.
    let saved = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap()
    };
    assert_eq!(saved()["spaces"][0]["root"]["agent"], "claude");
    first.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (url, _, second) = start_daemon_with(fake, Some(state_file.clone())).await;
    assert_eq!(layout(&url).await["spaces"][0]["root"]["agent"], "claude");
    second.abort();
}

/// A new space with a Codex pane whose agent is ready; returns the session name.
async fn codex_agent(url: &str) -> String {
    let (_, pane) = space(url, "/tmp").await;
    start_agent_as(url, &pane, None, Some("codex")).await
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
    assert_eq!(layout(&url).await["spaces"][0]["root"]["agent"], "codex");
    attach(&mut ws, &agent, 40).await;
    let text = reply(&mut ws, &agent, "show setup").await;
    assert!(text.contains("echo: show setup"), "{text:?}");
    assert!(
        text.contains("codex_home=") && text.contains("/codex-home"),
        "CODEX_HOME is the Codex home, not Claude's: {text:?}"
    );
    assert!(text.contains("policy=untrusted"), "{text:?}");
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
            SessionEvent::Thought { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thought, "pondering the question");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_command_approval_shows_a_card_and_the_answer_resolves_it() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach_and_send(&mut ws, &agent, "permission").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.session, agent);
    assert_eq!(perm.title, "rm -rf build");
    assert_eq!(perm.tool_call.as_deref(), Some("call-1"));
    let offered: Vec<&str> = perm.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(offered, ["allow", "allow_always", "deny"]);
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
        events.iter().any(|e| matches!(e, SessionEvent::ToolUpdate { id, status: Some(s), .. } if id == "call-1" && s == "completed")),
        "{events:?}"
    );

    // A denial reaches Codex as a decline, and its tool call fails.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            session: agent.clone(),
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
async fn a_codex_file_change_approval_names_the_files_and_can_last_the_session() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    attach_and_send(&mut ws, &agent, "edit").await;
    let perm = next_perm(&mut ws).await;
    assert_eq!(perm.title, "Edit src/a.rs");
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
        events.iter().any(|e| matches!(e, SessionEvent::ToolCall { id, tool_kind, content, .. } if id == "call-2" && tool_kind == "edit" && !content.is_empty())),
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
        ClientMsg::SessionCancel {
            req: Some(80),
            session: agent.clone(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { event, .. } => {
                if let SessionEvent::TurnEnd { stop_reason, .. } = event.event {
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
async fn the_approval_policy_is_a_control_sent_with_each_codex_turn() {
    let url = start_daemon().await;
    let mut ws = connect(&url).await;
    let agent = codex_agent(&url).await;
    let options = config_of(&mut ws, &agent, 90).await;
    assert_eq!(current(&options, "mode"), "untrusted");
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
/// its url, the session and the thread id the pane saved. The daemon stops before this returns.
async fn codex_pane_that_answered(dir: &std::path::Path) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let session = start_agent_as(&url, &pane, Some("first task"), Some("codex")).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &session, |events| {
        events.contains(&SessionEvent::TurnEnd {
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
    (session, thread)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_resumes_the_codex_panes_thread() {
    let dir = Scratch::new("codex-resume");
    let (session, thread) = codex_pane_that_answered(&dir.0).await;
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    // The thread's turns are the transcript.
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
    // The next turn runs in the same thread.
    let text = reply(&mut ws, &session, "show setup").await;
    assert!(text.contains(&format!("thread={thread}")), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_thread_that_will_not_reopen_leaves_only_the_notice() {
    let dir = Scratch::new("codex-lost");
    let (session, thread) = codex_pane_that_answered(&dir.0).await;
    std::fs::remove_dir_all(dir.0.join("codex-home").join("sessions")).unwrap();
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &session).await;
    assert_eq!(notice, wheel_proto::SESSION_NOT_REOPENED);
    assert_eq!(events, []);
    // A new thread runs the session, and the saved one is kept for the next start.
    let text = reply(&mut ws, &session, "show setup").await;
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
    assert_eq!(layout(&url).await["spaces"][0]["root"]["agent"], "pi");
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
    assert_eq!(perm.session, agent);
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
        events.iter().any(|e| matches!(e, SessionEvent::ToolUpdate { id, status: Some(s), .. } if id == "call-1" && s == "completed")),
        "{events:?}"
    );
    // `deny` picks the way out, which pi reads as a cancelled dialog.
    send(
        &mut ws,
        ClientMsg::SessionSend {
            req: Some(62),
            session: agent.clone(),
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
        ClientMsg::SessionCancel {
            req: Some(80),
            session: agent.clone(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { event, .. } => match event.event {
                SessionEvent::TurnEnd { stop_reason, .. } => {
                    assert_eq!(stop_reason, "cancelled");
                    break;
                }
                SessionEvent::Error { message, .. } => panic!("{message}"),
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
        ClientMsg::SessionSend {
            req: Some(81),
            session: agent.clone(),
            text: "fail now".into(),
        },
    )
    .await;
    loop {
        match recv(&mut ws).await {
            ServerMsg::Session { event, .. } => match event.event {
                SessionEvent::Error { turn, message } => {
                    assert!(turn.is_some());
                    assert_eq!(message, "429 rate limited");
                    break;
                }
                SessionEvent::TurnEnd { stop_reason, .. } => {
                    panic!("the failed reply ended as {stop_reason}")
                }
                _ => {}
            },
            ServerMsg::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

/// A daemon whose pi pane answered its first task, stopped; returns the session and its id.
async fn pi_pane_that_answered(dir: &std::path::Path) -> (String, String) {
    let state_file = dir.join("spaces.json");
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");
    let (url, _, daemon) = start_daemon_with(fake, Some(state_file.clone())).await;
    let (_, pane) = space(&url, "/tmp").await;
    let session = start_agent_as(&url, &pane, Some("first task"), Some("pi")).await;
    let mut ws = connect(&url).await;
    transcript_when(&mut ws, &session, |events| {
        events.contains(&SessionEvent::TurnEnd {
            turn: 1,
            stop_reason: "end_turn".into(),
        })
    })
    .await;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_file).unwrap()).unwrap();
    let id = saved["spaces"][0]["root"]["conversation"]
        .as_str()
        .expect("the pane saved pi's session id")
        .to_string();
    drop(ws);
    daemon.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;
    (session, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_resumes_the_pi_panes_session() {
    let dir = Scratch::new("pi-resume");
    let (session, id) = pi_pane_that_answered(&dir.0).await;
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
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
    let text = reply(&mut ws, &session, "show setup").await;
    assert!(text.contains(&format!("session={id}")), "{text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pi_session_that_is_gone_leaves_only_the_notice() {
    let dir = Scratch::new("pi-lost");
    let (session, id) = pi_pane_that_answered(&dir.0).await;
    std::fs::remove_dir_all(dir.0.join("pi-home").join("sessions")).unwrap();
    let (url, _, _) = start_daemon_with(
        env!("CARGO_BIN_EXE_wheel-fake-agent"),
        Some(dir.0.join("spaces.json")),
    )
    .await;
    let mut ws = connect(&url).await;
    let (notice, events) = notice_when_up(&mut ws, &session).await;
    assert_eq!(notice, wheel_proto::SESSION_NOT_REOPENED);
    assert_eq!(events, []);
    let text = reply(&mut ws, &session, "show setup").await;
    assert!(!text.contains(&format!("session={id}")), "{text:?}");
}
