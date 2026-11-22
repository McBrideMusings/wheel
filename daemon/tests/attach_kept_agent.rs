//! A client that attaches to an agent pane's agent while a restore still keeps that agent (its
//! home waits on a slow login shell) gets the agent's transcript once the agent starts. Its own
//! test binary, since it points `SHELL` at a stand-in shell for the whole process.

mod common;

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, StartKind};

const TOKEN: &str = "attach-kept-agent-token";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn send(ws: &mut Ws, msg: &ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(msg).unwrap().into()))
        .await
        .unwrap();
}

async fn op(url: &str, msg: ClientMsg, req: u64) -> ServerMsg {
    let mut ws: Ws = tokio_tungstenite::connect_async(url).await.unwrap().0;
    send(&mut ws, &msg).await;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), ws.next())
            .await
            .expect("daemon replied")
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        let reply: serde_json::Value = serde_json::from_str(&text).unwrap();
        if reply["req"] == req {
            return serde_json::from_value(reply).unwrap();
        }
    }
}

fn state(dir: &Path, agent: &str) -> std::sync::Arc<wheeld::AppState> {
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.to_path_buf(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    state
        .setting_set("agent.claude.command", agent, None, true)
        .unwrap();
    state
}

async fn serve(state: std::sync::Arc<wheeld::AppState>) -> (String, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (
        format!("ws://{addr}/ws?token={TOKEN}"),
        common::Daemon::new(serving, state),
    )
}

/// The pane `pane` as `status` lists it.
async fn listed_pane(url: &str, pane: &str) -> serde_json::Value {
    let ServerMsg::Status { spaces, .. } = op(url, ClientMsg::Status { req: Some(2) }, 2).await
    else {
        panic!("no status")
    };
    fn walk(v: &serde_json::Value, pane: &str) -> Option<serde_json::Value> {
        if v["node"] == "pane" && v["id"] == pane {
            return Some(v.clone());
        }
        match v {
            serde_json::Value::Object(map) => map.values().find_map(|c| walk(c, pane)),
            serde_json::Value::Array(list) => list.iter().find_map(|c| walk(c, pane)),
            _ => None,
        }
    }
    let listed = serde_json::to_value(&spaces).unwrap();
    walk(&listed, pane).unwrap_or_else(|| panic!("no pane {pane} in {listed}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attach_to_a_kept_agent_streams_once_it_starts() {
    let dir = std::env::temp_dir().join(format!("wheel-attach-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The stand-in login shell routes CLAUDE_CONFIG_DIR to `routed`, and answers 3 s late while
    // `slow` exists, as a shell busy at login does.
    let routed = dir.join("routed");
    std::fs::create_dir_all(&routed).unwrap();
    let slow = dir.join("slow");
    let shell = dir.join("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nif [ -e {slow} ]; then sleep 3; fi\nexport CLAUDE_CONFIG_DIR={routed}\nshift\nexec /bin/sh -c \"$1\"\n",
            slow = slow.display(),
            routed = routed.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // SAFETY: this binary holds one test, and nothing else reads the environment while it is set.
    unsafe { std::env::set_var("SHELL", &shell) };
    let fake = env!("CARGO_BIN_EXE_wheel-fake-agent");

    // A saved layout with one agent pane.
    let first = state(&dir, fake);
    first.restore_spaces(dir.join("spaces.json")).await.unwrap();
    let (url, running) = serve(first).await;
    let ServerMsg::Created { id: space, .. } = op(
        &url,
        ClientMsg::SpaceCreate {
            req: Some(1),
            cwd: dir.display().to_string(),
            name: None,
            focus: false,
            background: false,
            pane: None,
            session: None,
        },
        1,
    )
    .await
    else {
        panic!("no space")
    };
    let ServerMsg::Status { spaces, .. } = op(&url, ClientMsg::Status { req: Some(2) }, 2).await
    else {
        panic!("no status")
    };
    let pane = serde_json::to_value(&spaces)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == space.as_str())
        .map(|s| s["root"]["id"].as_str().unwrap().to_string())
        .unwrap();
    let started = op(
        &url,
        ClientMsg::PaneStart {
            req: Some(3),
            pane: pane.clone(),
            kind: StartKind::Agent,
            message: None,
            agent: None,
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
            from: None,
        },
        3,
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    running.abort();

    // The restore keeps the agent while the slow shell has not answered. A client connects then,
    // as the Mac app does on a restart, and attaches to the agent the pane names, once.
    std::fs::write(&slow, "").unwrap();
    let second = state(&dir, fake);
    second
        .restore_spaces(dir.join("spaces.json"))
        .await
        .unwrap();
    let (url, running) = serve(second).await;
    let listed = listed_pane(&url, &pane).await;
    let agent = listed["agent"].as_str().unwrap_or_else(|| panic!("no agent in {listed}")).to_string();
    assert_ne!(
        listed["login"]["home"].as_str().unwrap_or_default(),
        routed.display().to_string(),
        "the agent started before the shell answered, so the attach would not race it"
    );
    let mut ws: Ws = tokio_tungstenite::connect_async(&url).await.unwrap().0;
    send(&mut ws, &ClientMsg::AgentAttach { req: None, agent: agent.clone() }).await;

    // The attach streams the agent's transcript once the agent starts.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = vec![];
    loop {
        let frame = match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(frame) => frame.unwrap().unwrap(),
            Err(_) => panic!("no agent_attached for {agent} within 20 s; the connection got {seen:#?}"),
        };
        let Message::Text(text) = frame else { continue };
        let msg: serde_json::Value = serde_json::from_str(&text).unwrap();
        if msg["ev"] == "agent_attached" && msg["agent"] == agent.as_str() {
            break;
        }
        seen.push(msg);
    }
    running.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
