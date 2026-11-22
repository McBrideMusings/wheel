//! A restore never waits on a slow login shell for an agent's home: it keeps the agent as saved,
//! wheeld serves at once, and the agent starts when the shell answers. An agent the restore kept
//! survives starts of kept agents that cannot start it. Its own test binary, since it points
//! `SHELL` at a stand-in shell for the whole process.

mod common;

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, StartKind};

const TOKEN: &str = "restore-kept-race-token";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn op(url: &str, msg: ClientMsg, req: u64) -> ServerMsg {
    let mut ws: Ws = tokio_tungstenite::connect_async(url).await.unwrap().0;
    ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
        .await
        .unwrap();
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

/// The home the agent pane `pane` lists in `status`: the shell's route once its agent started.
async fn listed_home(url: &str, pane: &str) -> String {
    let ServerMsg::Status { spaces, .. } = op(url, ClientMsg::Status { req: Some(2) }, 2).await
    else {
        panic!("no status")
    };
    fn walk<'a>(v: &'a serde_json::Value, pane: &str) -> Option<&'a serde_json::Value> {
        if v["node"] == "pane" && v["id"] == pane {
            return Some(v);
        }
        match v {
            serde_json::Value::Object(map) => map.values().find_map(|c| walk(c, pane)),
            serde_json::Value::Array(list) => list.iter().find_map(|c| walk(c, pane)),
            _ => None,
        }
    }
    let listed = serde_json::to_value(&spaces).unwrap();
    walk(&listed, pane).unwrap_or_else(|| panic!("no pane {pane} in {listed}"))["login"]["home"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_agent_survives_a_start_during_the_restore() {
    let dir = std::env::temp_dir().join(format!("wheel-restore-kept-race-{}", std::process::id()));
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
        ClientMsg::PaneStart { from: None,
            req: Some(3),
            pane: pane.clone(),
            kind: StartKind::Agent,
            message: None,
            agent: None,
            model: None,
            permission_mode: None,
            sandbox: None,
            dir: None,
        },
        3,
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    running.abort();

    // A restore with the slow shell keeps the agent as saved rather than wait on the shell, so
    // wheeld serves at once; the agent starts once the shell answers.
    std::fs::write(&slow, "").unwrap();
    let routed = routed.display().to_string();
    let began = std::time::Instant::now();
    let second = state(&dir, fake);
    second
        .restore_spaces(dir.join("spaces.json"))
        .await
        .unwrap();
    let (url, running) = serve(second).await;
    let listed = listed_home(&url, &pane).await;
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "status took {:?}: the restore waited on the slow shell",
        began.elapsed()
    );
    assert_ne!(
        listed, routed,
        "the agent started before the shell answered"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while listed_home(&url, &pane).await != routed {
        assert!(
            std::time::Instant::now() < deadline,
            "the agent never started once the shell answered"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    running.abort();

    // The restore keeps the agent, and the start of kept agents it leaves behind and a second
    // one both find no program for it, so the agent is still kept and starts once its program is
    // there.
    let third = state(&dir, &dir.join("no-such-agent").display().to_string());
    third.restore_spaces(dir.join("spaces.json")).await.unwrap();
    third.start_kept_agents().await;
    std::fs::remove_file(&slow).unwrap();
    third
        .setting_set("agent.claude.command", fake, None, true)
        .unwrap();
    third.start_kept_agents().await;

    let (url, running) = serve(third).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while listed_home(&url, &pane).await != routed {
        assert!(
            std::time::Instant::now() < deadline,
            "the kept agent never started"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    running.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
