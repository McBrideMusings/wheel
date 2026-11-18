//! A restored agent pane asks the login shell for its harness's home variable unless the pane
//! picked one: the shell's answer is never saved, so a route changed between restarts reaches the
//! restored agent, and a picked value outlives any route. Its own test binary, since it points
//! `SHELL` at a stand-in shell for the whole process.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{ClientMsg, ServerMsg, StartKind};

const TOKEN: &str = "restored-home-token";

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

/// A daemon keeping its layout in `dir/spaces.json`, its Claude Code the fake agent, with no
/// `agent.claude.home` set, so the home comes from the login shell.
async fn daemon(dir: &Path) -> (String, common::Daemon) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.to_path_buf(),
        env!("CARGO_BIN_EXE_wheeld").into(),
    );
    state
        .setting_set("agent.claude.command", env!("CARGO_BIN_EXE_wheel-fake-agent"), None, true)
        .unwrap();
    state.restore_spaces(dir.join("spaces.json")).await.unwrap();
    let serving = tokio::spawn(wheeld::serve(listener, state.clone()));
    (format!("ws://{addr}/ws?token={TOKEN}"), common::Daemon::new(serving, state))
}

/// The home the agent pane `pane` runs with, as `status` lists its login.
async fn running_home(url: &str, pane: &str) -> String {
    let ServerMsg::Status { spaces, .. } = op(url, ClientMsg::Status { req: Some(2) }, 2).await
    else {
        panic!("no status")
    };
    let listed = serde_json::to_value(&spaces).unwrap();
    find_pane(&listed, pane)["login"]["home"]
        .as_str()
        .unwrap_or_else(|| panic!("{pane} lists no login: {listed}"))
        .to_string()
}

/// Wait until the agent pane `pane` runs with `home`: one whose home the login shell gives starts
/// once the shell answers, after the restore.
async fn wait_home(url: &str, pane: &str, home: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while running_home(url, pane).await != home {
        assert!(std::time::Instant::now() < deadline, "{pane} never ran with {home}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn find_pane<'a>(value: &'a serde_json::Value, pane: &str) -> &'a serde_json::Value {
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
    walk(value, pane).unwrap_or_else(|| panic!("no pane {pane} in {value}"))
}

fn saved_pane(dir: &Path, pane: &str) -> serde_json::Value {
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("spaces.json")).unwrap()).unwrap();
    find_pane(&saved, pane).clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_agent_asks_the_shell_for_any_home_it_did_not_pick() {
    let dir = std::env::temp_dir().join(format!("wheel-restored-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The stand-in login shell sets CLAUDE_CONFIG_DIR to whatever `route` holds, as a kennel route
    // in the person's startup files would, and fails once, answering nothing, while `fail` exists.
    let route = dir.join("route");
    let fail = dir.join("fail");
    let shell = dir.join("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nif [ -e {fail} ]; then rm {fail}; exit 1; fi\nexport CLAUDE_CONFIG_DIR=\"$(cat {route})\"\nshift\nexec /bin/sh -c \"$1\"\n",
            fail = fail.display(),
            route = route.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // SAFETY: this binary holds one test, and nothing else reads the environment while it is set.
    unsafe { std::env::set_var("SHELL", &shell) };
    let home = |name: &str| -> PathBuf {
        let h = dir.join(name);
        std::fs::create_dir_all(&h).unwrap();
        h
    };
    let (first, second, picked) = (home("first"), home("second"), home("picked"));

    // 1. A new pane runs with the shell's home, and saves none of it.
    std::fs::write(&route, first.to_str().unwrap()).unwrap();
    let (url, running) = daemon(&dir).await;
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
    let listed = serde_json::to_value(&spaces).unwrap();
    let pane = listed
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
        },
        3,
    )
    .await;
    assert!(matches!(started, ServerMsg::Started { .. }), "{started:?}");
    assert_eq!(running_home(&url, &pane).await, first.display().to_string());
    assert!(saved_pane(&dir, &pane).get("env").is_none(), "{}", saved_pane(&dir, &pane));
    running.abort();

    // 2. The route changes; the restored agent runs with the new home.
    std::fs::write(&route, second.to_str().unwrap()).unwrap();
    let (url, running) = daemon(&dir).await;
    wait_home(&url, &pane, &second.display().to_string()).await;
    running.abort();

    // 3. A picked home outlives any route.
    let path = dir.join("spaces.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    fn pick(v: &mut serde_json::Value, pane: &str, home: &str) {
        if v["node"] == "pane" && v["id"] == pane {
            v["env"] = serde_json::json!({ "CLAUDE_CONFIG_DIR": home });
            return;
        }
        match v {
            serde_json::Value::Object(map) => map.values_mut().for_each(|c| pick(c, pane, home)),
            serde_json::Value::Array(list) => list.iter_mut().for_each(|c| pick(c, pane, home)),
            _ => {}
        }
    }
    pick(&mut saved, &pane, picked.to_str().unwrap());
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let (url, running) = daemon(&dir).await;
    assert_eq!(running_home(&url, &pane).await, picked.display().to_string());
    running.abort();

    // 4. A pane that picked no override (a fork of an agent on its default home) is not moved to
    // the shell's home either.
    let mut saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    pick(&mut saved, &pane, "");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let (url, running) = daemon(&dir).await;
    let home = running_home(&url, &pane).await;
    assert_ne!(home, second.display().to_string(), "the shell's home reached a pane that picked none");
    assert_ne!(home, "", "{home}");
    running.abort();

    // 5. A login shell that gives no answer once leaves the restored agent kept as saved, and it
    // starts by itself once the shell answers again.
    fn unpick(v: &mut serde_json::Value, pane: &str) {
        if v["node"] == "pane" && v["id"] == pane {
            v.as_object_mut().unwrap().remove("env");
            return;
        }
        match v {
            serde_json::Value::Object(map) => map.values_mut().for_each(|c| unpick(c, pane)),
            serde_json::Value::Array(list) => list.iter_mut().for_each(|c| unpick(c, pane)),
            _ => {}
        }
    }
    let mut saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    unpick(&mut saved, &pane);
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    std::fs::write(&fail, "").unwrap();
    let (url, running) = daemon(&dir).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while fail.exists() {
        assert!(std::time::Instant::now() < deadline, "the shell was never asked");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A pane whose agent is not running lists the default home, never the shell's.
    let routed = second.display().to_string();
    assert_ne!(running_home(&url, &pane).await, routed, "an agent started with no answer from the shell");
    wait_home(&url, &pane, &routed).await;
    running.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
