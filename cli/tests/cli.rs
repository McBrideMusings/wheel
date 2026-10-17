//! The `wheel` binary against a real daemon. A script relies on the output and the exit code, so
//! both are checked: 0 when the text appears, 3 on timeout, 4 when the shell exits first, 5 when
//! the token is wrong, 6 when the daemon refuses the request, 7 when the connection drops. `wheel
//! usage` also exits 3 when the reading is stale or missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "cli-test-token";

struct Daemon {
    url: String,
    token_file: PathBuf,
}

async fn daemon() -> Daemon {
    daemon_in(std::env::temp_dir()).await
}

/// A daemon whose instance directory is `support`, where a machine-wide `wheel.toml` lands.
async fn daemon_in(support: PathBuf) -> Daemon {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "wheel-cli-test-{}-{}",
        std::process::id(),
        addr.port()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let token_file = dir.join("token");
    std::fs::write(&token_file, TOKEN).unwrap();
    tokio::spawn(wheeld::serve(
        listener,
        wheeld::AppState::new(
            "/dev/null".into(),
            TOKEN.into(),
            support,
            "/usr/bin/false".into(),
        ),
    ));
    Daemon {
        url: format!("ws://{addr}/ws"),
        token_file,
    }
}

async fn wheel(d: &Daemon, args: &[&str]) -> Output {
    wheel_with_token(&d.url, &d.token_file, args).await
}

async fn wheel_with_token(url: &str, token_file: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wheel"));
    cmd.args(["--url", url, "--token-file", token_file.to_str().unwrap()])
        .args(args);
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Run a command that must succeed and return its trimmed stdout.
async fn ok(d: &Daemon, args: &[&str]) -> String {
    let o = wheel(d, args).await;
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    stdout(&o).trim().to_string()
}

async fn open(d: &Daemon) -> String {
    ok(d, &["term", "open", "--cwd", "/tmp"]).await
}

#[tokio::test(flavor = "multi_thread")]
async fn open_send_and_read_a_terminal() {
    let d = daemon().await;
    let id = open(&d).await;
    assert!(id.starts_with('t'), "printed a terminal id: {id:?}");
    let sent = wheel(&d, &["term", "send", &id, "echo cli-$((2*21))", "--enter"]).await;
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let read = wheel(
        &d,
        &["term", "read", &id, "--until", "cli-42", "--timeout", "10"],
    )
    .await;
    assert_eq!(
        read.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(stdout(&read).contains("cli-42"));

    let status: serde_json::Value =
        serde_json::from_slice(&wheel(&d, &["status"]).await.stdout).unwrap();
    assert_eq!(status["terms"][0]["id"], id);
    assert_eq!(status["terms"][0]["alive"], true);

    let closed = wheel(&d, &["term", "close", &id]).await;
    assert!(
        closed.status.success(),
        "{}",
        String::from_utf8_lossy(&closed.stderr)
    );
    let status: serde_json::Value =
        serde_json::from_slice(&wheel(&d, &["status"]).await.stdout).unwrap();
    assert_eq!(status["terms"], serde_json::json!([]));
    assert_eq!(
        wheel(&d, &["term", "close", &id]).await.status.code(),
        Some(6)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_new_ignores_output_from_before_it_attached() {
    let d = daemon().await;
    let id = open(&d).await;
    wheel(&d, &["term", "send", &id, "echo old-$((1+1))", "--enter"]).await;
    let first = wheel(
        &d,
        &["term", "read", &id, "--until", "old-2", "--timeout", "10"],
    )
    .await;
    assert_eq!(first.status.code(), Some(0));
    let again = wheel(
        &d,
        &[
            "term",
            "read",
            &id,
            "--new",
            "--until",
            "old-2",
            "--timeout",
            "1",
        ],
    )
    .await;
    assert_eq!(
        again.status.code(),
        Some(3),
        "old output must not satisfy --new: {}",
        stdout(&again)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_codes_tell_a_script_what_went_wrong() {
    let d = daemon().await;
    let id = open(&d).await;

    let timeout = wheel(
        &d,
        &[
            "term",
            "read",
            &id,
            "--until",
            "never-printed",
            "--timeout",
            "0.5",
        ],
    )
    .await;
    assert_eq!(timeout.status.code(), Some(3), "timeout");

    let unknown = wheel(&d, &["term", "send", "t999", "hi"]).await;
    assert_eq!(
        unknown.status.code(),
        Some(6),
        "daemon refused: {}",
        String::from_utf8_lossy(&unknown.stderr)
    );

    let bad_token = std::env::temp_dir().join(format!("wheel-cli-badtoken-{}", std::process::id()));
    std::fs::write(&bad_token, "wrong").unwrap();
    let refused = wheel_with_token(&d.url, &bad_token, &["status"]).await;
    assert_eq!(refused.status.code(), Some(5), "wrong token");

    wheel(&d, &["term", "send", &id, "exit 3", "--enter"]).await;
    let exited = wheel(
        &d,
        &[
            "term",
            "read",
            &id,
            "--until",
            "never-printed",
            "--timeout",
            "10",
        ],
    )
    .await;
    assert_eq!(
        exited.status.code(),
        Some(4),
        "shell exited first: {}",
        String::from_utf8_lossy(&exited.stderr)
    );
}

async fn layout(d: &Daemon) -> serde_json::Value {
    serde_json::from_str(&ok(d, &["space", "ls"]).await).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn space_and_pane_commands_print_ids_and_change_the_layout() {
    let d = daemon().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let dir = dir.to_str().unwrap();

    let space = ok(&d, &["space", "new", dir, "--name", "proj"]).await;
    assert!(space.starts_with('s'), "printed a space id: {space:?}");
    let l = layout(&d).await;
    assert_eq!(l["active"], space);
    assert_eq!(l["spaces"][0]["name"], "proj");
    assert_eq!(l["spaces"][0]["cwd"], dir);
    let first = l["spaces"][0]["root"]["id"].as_str().unwrap().to_string();
    assert_eq!(l["spaces"][0]["root"]["kind"], "empty");

    let second = ok(&d, &["pane", "split", &first, "right"]).await;
    assert!(second.starts_with('p'), "printed a pane id: {second:?}");
    let root = &layout(&d).await["spaces"][0]["root"];
    assert_eq!(root["node"], "split");
    assert_eq!(root["axis"], "horizontal");
    assert_eq!(root["a"]["id"], first);
    assert_eq!(root["b"]["id"], second);
    let split = root["id"].as_str().unwrap().to_string();

    ok(&d, &["pane", "ratio", &split, "0.3"]).await;
    ok(&d, &["pane", "kind", &second, "terminal"]).await;
    let l = layout(&d).await;
    let root = &l["spaces"][0]["root"];
    assert_eq!(root["ratio"], 0.3);
    assert_eq!(root["b"]["kind"], "terminal");
    let term = root["b"]["term"].as_str().unwrap();
    assert!(term.starts_with('t'), "the pane runs a terminal: {term:?}");
    assert_eq!(
        l["spaces"][0]["focus"], second,
        "splitting focuses the new pane"
    );

    let other = ok(&d, &["space", "new", dir]).await;
    assert_eq!(layout(&d).await["active"], other);
    ok(&d, &["pane", "focus", &first]).await;
    let l = layout(&d).await;
    assert_eq!(l["active"], space, "focusing a pane makes its space active");
    assert_eq!(l["spaces"][0]["focus"], first);
    ok(&d, &["space", "focus", &other]).await;
    assert_eq!(layout(&d).await["active"], other);

    ok(&d, &["pane", "close", &second]).await;
    let root = &layout(&d).await["spaces"][0]["root"];
    assert_eq!(root["node"], "pane");
    assert_eq!(root["id"], first);

    ok(&d, &["space", "close", &space]).await;
    let l = layout(&d).await;
    assert_eq!(l["spaces"].as_array().unwrap().len(), 1);
    assert_eq!(l["spaces"][0]["id"], other);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_direction_or_kind_exits_2() {
    let d = daemon().await;
    for args in [
        ["pane", "split", "p1", "sideways"],
        ["pane", "kind", "p1", "hologram"],
    ] {
        let o = wheel(&d, &args).await;
        assert_eq!(
            o.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_id_or_a_ratio_out_of_range_exits_6() {
    let d = daemon().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let pane = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(&d, &["pane", "split", &pane, "down"]).await;
    let split = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for args in [
        vec!["pane", "split", "p999", "right"],
        vec!["pane", "close", "p999"],
        vec!["pane", "kind", "p999", "terminal"],
        vec!["pane", "focus", "p999"],
        vec!["pane", "ratio", "x999", "0.5"],
        vec!["pane", "ratio", &split, "0.05"],
        vec!["pane", "ratio", &split, "0.95"],
        vec!["space", "close", "s999"],
        vec!["space", "focus", "s999"],
    ] {
        let o = wheel(&d, &args).await;
        assert_eq!(
            o.status.code(),
            Some(6),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

/// A WebSocket server that accepts every connection, reads one frame and drops the socket: a
/// daemon that went away mid-command.
async fn vanishing_daemon() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let _ = ws.next().await;
            });
        }
    });
    format!("ws://{addr}/ws")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_exits_7() {
    let url = vanishing_daemon().await;
    let d = daemon().await;
    for args in [
        vec!["status"],
        vec!["term", "read", "t1", "--until", "x", "--timeout", "10"],
    ] {
        let o = wheel_with_token(&url, &d.token_file, &args).await;
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(7), "{args:?}: {stderr}");
        assert!(
            stderr.contains("lost the connection to the daemon"),
            "{stderr}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn term_send_and_read_accept_a_pane_id() {
    let d = daemon().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let dir = dir.to_str().unwrap();
    ok(&d, &["space", "new", dir]).await;
    let pane = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let refused = wheel(&d, &["term", "send", &pane, "x"]).await;
    assert_eq!(refused.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains(&format!("{pane} is not a terminal pane")));

    ok(&d, &["pane", "kind", &pane, "terminal"]).await;
    ok(
        &d,
        &["term", "send", &pane, "echo pane-$((6*7))", "--enter"],
    )
    .await;
    let read = wheel(
        &d,
        &[
            "term",
            "read",
            &pane,
            "--until",
            "pane-42",
            "--timeout",
            "10",
        ],
    )
    .await;
    assert_eq!(
        read.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(stdout(&read).contains("pane-42"));
}

/// A daemon whose agent panes run the scripted fake agent, which sits beside the `wheel` binary
/// once `cargo test --workspace` has built it.
async fn daemon_with_agents() -> Daemon {
    daemon_with_agents_looping(None).await
}

/// Like `daemon_with_agents`, with the supervisors' loops on `timing` when given.
async fn daemon_with_agents_looping(timing: Option<wheeld::loops::LoopTiming>) -> Daemon {
    let fake = Path::new(env!("CARGO_BIN_EXE_wheel")).with_file_name("wheel-fake-agent");
    assert!(
        fake.exists(),
        "build the workspace first: {fake:?} is missing"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "wheel-cli-agent-test-{}-{}",
        std::process::id(),
        addr.port()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let token_file = dir.join("token");
    std::fs::write(&token_file, TOKEN).unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        "/usr/bin/false".into(),
    );
    state.set_agent_command(fake.to_str().unwrap().into());
    if let Some(timing) = timing {
        state.set_loop_timing(timing);
    }
    tokio::spawn(wheeld::serve(listener, state));
    Daemon {
        url: format!("ws://{addr}/ws"),
        token_file,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_commands_accept_a_pane_id_and_wait_for_ready() {
    let d = daemon_with_agents().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let pane = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let started = ok(
        &d,
        &["pane", "start", &pane, "agent", "--message", "first task"],
    )
    .await;
    let started: serde_json::Value = serde_json::from_str(&started).unwrap();
    let session = started["session"].as_str().unwrap().to_string();

    // `wait` prints the pane, the session and the state once the first turn is done.
    let waited = ok(&d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    assert_eq!(waited, format!(r#"{pane} {session} {{"phase":"ready"}}"#));
    let by_name = ok(&d, &["agent", "wait", &session, "--timeout", "10"]).await;
    assert_eq!(by_name, waited);

    let reply = ok(&d, &["agent", "send", &pane, "by pane"]).await;
    assert!(reply.contains("echo: by pane"), "{reply}");
    let read = ok(&d, &["agent", "read", &pane]).await;
    assert!(
        read.contains("> first task") && read.contains("> by pane"),
        "{read}"
    );

    let idle = wheel(&d, &["agent", "cancel", &pane]).await;
    assert_eq!(idle.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&idle.stderr)
        .contains(&format!("session {session} has no turn to cancel")));
}

#[tokio::test(flavor = "multi_thread")]
async fn supervisor_commands_print_the_supervisor_as_json_at_every_state() {
    let d = daemon_with_agents().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let json = |text: String| serde_json::from_str::<serde_json::Value>(&text).unwrap();

    let none = wheel(&d, &["supervisor", "start"]).await;
    assert_eq!(none.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&none.stderr).contains("needs a goal"));

    let started = json(
        ok(
            &d,
            &["supervisor", "start", "--goal", "t-1", "--workers", "3"],
        )
        .await,
    );
    let id = started["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("t-1-u"), "goal plus a short id: {id}");
    assert_eq!(
        (
            started["state"].as_str(),
            started["workers"].as_u64(),
            started["goal"].as_str()
        ),
        (Some("running"), Some(3), Some("t-1"))
    );

    let state = |text: String| json(text)["state"].as_str().unwrap().to_string();
    assert_eq!(state(ok(&d, &["supervisor", "pause", &id]).await), "paused");
    assert_eq!(state(ok(&d, &["supervisor", "show", &id]).await), "paused");
    assert_eq!(
        state(ok(&d, &["supervisor", "resume", &id]).await),
        "running"
    );
    assert_eq!(
        json(ok(&d, &["supervisor", "workers", &id, "5"]).await)["workers"],
        5
    );
    assert_eq!(
        json(ok(&d, &["supervisor", "cap", &id, "70"]).await)["cap"],
        70
    );
    assert_eq!(state(ok(&d, &["supervisor", "stop", &id]).await), "stopped");

    let listed = json(ok(&d, &["supervisor", "list"]).await);
    assert_eq!(listed[0]["id"], id.as_str());
    assert_eq!(listed[0]["state"], "stopped");
    assert_eq!(
        wheel(&d, &["supervisor", "show", "nope-u9"])
            .await
            .status
            .code(),
        Some(6)
    );
    assert_eq!(
        wheel(&d, &["supervisor", "pause", &id]).await.status.code(),
        Some(6)
    );
}

/// Run `program` in `dir` and return its stdout.
fn in_dir(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What `bd create --json` made, as the new bead's id.
fn made_bead(dir: &Path, args: &[&str]) -> String {
    let mut all = vec!["create"];
    all.extend_from_slice(args);
    all.push("--json");
    let out: serde_json::Value = serde_json::from_str(&in_dir(dir, "bd", &all)).unwrap();
    out["id"]
        .as_str()
        .or_else(|| out[0]["id"].as_str())
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn supervisor_events_prints_every_loop_decision_in_order_and_inbox_the_items() {
    let repo = std::env::temp_dir().join(format!("wheel-cli-loop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    in_dir(&repo, "git", &["init", "-q", "-b", "main"]);
    in_dir(
        &repo,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start",
        ],
    );
    in_dir(
        &repo,
        "bd",
        &["init", "--prefix", "t", "--non-interactive", "--quiet"],
    );
    let epic = made_bead(&repo, &["The epic", "-t", "epic"]);
    let titles = ["close it", "block me", "stubborn"];
    let beads: Vec<String> = titles
        .iter()
        .map(|t| made_bead(&repo, &[t, "--parent", &epic]))
        .collect();

    let d = daemon_with_agents_looping(Some(wheeld::loops::LoopTiming {
        tick: Duration::from_millis(200),
        stall: Duration::from_secs(60),
    }))
    .await;
    ok(&d, &["space", "new", repo.to_str().unwrap()]).await;
    let json = |text: String| serde_json::from_str::<serde_json::Value>(&text).unwrap();
    let id = json(
        ok(
            &d,
            &["supervisor", "start", "--goal", &epic, "--workers", "1"],
        )
        .await,
    )["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The three workers go one at a time; wait until each has reached its end.
    let mut events = json(ok(&d, &["supervisor", "events", &id]).await);
    for _ in 0..600 {
        events = json(ok(&d, &["supervisor", "events", &id]).await);
        let kinds: Vec<&str> = events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        // Each of the two inbox items wakes the supervisor's agent, which leaves it.
        if ["landed", "blocked", "escalated"]
            .iter()
            .all(|k| kinds.contains(k))
            && kinds.iter().filter(|k| **k == "unhandled").count() == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let for_bead = |bead: &str| -> Vec<String> {
        events
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["bead"] == bead)
            .map(|e| e["kind"].as_str().unwrap().to_string())
            // The supervisor's agent is woken with each inbox item and here leaves it.
            .filter(|k| k != "woken" && k != "unhandled")
            .collect()
    };
    assert_eq!(
        for_bead(&beads[0]),
        ["claimed", "started", "landed"],
        "{events}"
    );
    assert_eq!(
        for_bead(&beads[1]),
        ["claimed", "started", "blocked"],
        "{events}"
    );
    assert_eq!(
        for_bead(&beads[2]),
        [
            "claimed",
            "started",
            "continued",
            "continued",
            "continued",
            "escalated"
        ],
        "{events}"
    );
    // Oldest first, ids counting up, and --last keeps the end.
    let ids: Vec<u64> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_u64().unwrap())
        .collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    let last = json(ok(&d, &["supervisor", "events", &id, "--last", "2"]).await);
    assert_eq!(last.as_array().unwrap().len(), 2);
    assert_eq!(last[1]["id"].as_u64(), ids.last().copied());

    let inbox = json(ok(&d, &["supervisor", "inbox", &id]).await);
    let mut waiting: Vec<(String, String)> = inbox
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["bead"].as_str().unwrap().to_string(),
                i["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    waiting.sort();
    let mut expected = vec![
        (beads[1].clone(), "blocked".to_string()),
        (beads[2].clone(), "escalated".to_string()),
    ];
    expected.sort();
    assert_eq!(waiting, expected, "{inbox}");
    assert_eq!(json(ok(&d, &["supervisor", "show", &id]).await)["inbox"], 2);
    assert_eq!(
        wheel(&d, &["supervisor", "events", "nope-u9"])
            .await
            .status
            .code(),
        Some(6)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// A daemon whose lead runs the scripted fake agent.
async fn daemon_with_lead() -> Daemon {
    daemon_with_lead_running("").await
}

/// A daemon whose lead runs the fake agent with `flags` after its path.
async fn daemon_with_lead_running(flags: &str) -> Daemon {
    daemon_with_lead_reading(flags, None).await
}

/// A daemon whose lead runs the fake agent, reading usage from `usage` when given.
async fn daemon_with_lead_reading(
    flags: &str,
    usage: Option<Arc<dyn wheeld::usage::UsageSource>>,
) -> Daemon {
    let fake = Path::new(env!("CARGO_BIN_EXE_wheel")).with_file_name("wheel-fake-agent");
    assert!(
        fake.exists(),
        "build the workspace first: {fake:?} is missing"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "wheel-cli-lead-test-{}-{}",
        std::process::id(),
        addr.port()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let token_file = dir.join("token");
    std::fs::write(&token_file, TOKEN).unwrap();
    let state = wheeld::AppState::new(
        "/dev/null".into(),
        TOKEN.into(),
        dir.clone(),
        "/usr/bin/false".into(),
    );
    if let Some(usage) = usage {
        state.set_usage_source(usage);
    }
    let command = format!("{} {flags}", fake.to_str().unwrap());
    state.set_agent_command(command.clone());
    state.start_lead(
        command,
        std::env::temp_dir(),
        wheeld::lead::default_dir(),
        &wheeld::mcp::url(addr),
    );
    tokio::spawn(wheeld::serve(listener, state));
    Daemon {
        url: format!("ws://{addr}/ws"),
        token_file,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lead_status_prints_the_leads_configuration() {
    let d = daemon_with_lead().await;
    let status: serde_json::Value =
        serde_json::from_str(&ok(&d, &["lead", "status"]).await).unwrap();
    let dir = status["config_dir"]["path"].as_str().unwrap();
    assert!(dir.ends_with("/lead-home"), "{status}");
    assert_eq!(status["config_dir"]["exists"], true, "{status}");
    assert_eq!(status["setting_sources"], serde_json::json!(["user"]));
    assert_eq!(status["tools"], serde_json::json!(["Read", "Grep", "Glob"]));
    assert!(
        status["brief"]["path"]
            .as_str()
            .unwrap()
            .ends_with("/lead/CLAUDE.md"),
        "{status}"
    );
    assert_eq!(status["brief"]["exists"], true, "{status}");
    assert_eq!(status["skills"]["exists"], true, "{status}");
}

#[tokio::test(flavor = "multi_thread")]
async fn lead_read_prints_a_diff_as_counts_and_json_keeps_its_text() {
    let d = daemon_with_lead().await;
    ok(&d, &["lead", "send", "show a diff"]).await;

    let plain = ok(&d, &["lead", "read"]).await;
    assert!(plain.contains("  editing a.rs"), "{plain}");
    assert!(plain.contains("  diff src/a.rs (-1 +1 lines)"), "{plain}");
    assert!(plain.contains("  terminal term-1"), "{plain}");

    let json: serde_json::Value =
        serde_json::from_str(&ok(&d, &["lead", "read", "--json"]).await).unwrap();
    assert!(json["state"].is_object(), "{json}");
    let events = json["events"].as_array().unwrap();
    let call = events
        .iter()
        .find(|e| e["kind"] == "tool_call" && e["id"] == "call-2")
        .unwrap_or_else(|| panic!("no call-2 tool call in {json}"));
    let diff = call["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "diff")
        .unwrap();
    assert_eq!(diff["old_text"], "one\n", "{diff}");
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_read_marks_a_message_queued_behind_a_permission_request() {
    let d = daemon_with_agents().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let pane = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &d,
        &[
            "pane",
            "start",
            &pane,
            "agent",
            "--message",
            "permission please",
        ],
    )
    .await;
    ok(&d, &["perm", "wait", "--timeout", "10"]).await;

    // The running turn waits on the person, so "second" queues behind it and `send` times out.
    wheel(&d, &["agent", "send", &pane, "second", "--timeout", "1"]).await;
    let waiting = ok(&d, &["agent", "read", &pane]).await;
    assert!(waiting.contains("> second (queued)"), "{waiting}");
    assert!(
        !waiting.contains("> permission please (queued)"),
        "{waiting}"
    );

    let perm = ok(&d, &["perm", "wait", "--timeout", "10"]).await;
    let perm: serde_json::Value = serde_json::from_str(&perm).unwrap();
    ok(
        &d,
        &["perm", "answer", perm["id"].as_str().unwrap(), "allow"],
    )
    .await;
    ok(&d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    let done = ok(&d, &["agent", "read", &pane]).await;
    assert!(
        done.contains("> second") && !done.contains("(queued)"),
        "{done}"
    );
}

/// `config watch` run in the background; `write` runs once the watcher says it is listening.
async fn watch_during(d: &Daemon, args: &[&str], write: &[&str]) -> Output {
    use std::io::{BufRead, BufReader};
    let mut child = Command::new(env!("CARGO_BIN_EXE_wheel"))
        .args([
            "--url",
            &d.url,
            "--token-file",
            d.token_file.to_str().unwrap(),
        ])
        .args(["config", "watch"])
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "watching settings");
    ok(d, write).await;
    tokio::task::spawn_blocking(move || child.wait_with_output().unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn config_watch_prints_each_kind_of_settings_broadcast_and_times_out_without_one() {
    let support = std::env::temp_dir().join(format!("wheel-cli-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&support);
    let project = support.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let d = daemon_in(support.clone()).await;

    // With no space open, a machine write is resolved against none.
    let machine = watch_during(
        &d,
        &["--timeout", "10"],
        &[
            "config",
            "set",
            "browser.homepage",
            "https://machine.example",
            "--machine",
        ],
    )
    .await;
    assert_eq!(
        machine.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&machine.stderr)
    );
    let text = stdout(&machine);
    assert!(text.starts_with("settings space=machine-wide\n"), "{text}");
    assert!(
        text.contains("browser.homepage = https://machine.example   (machine)"),
        "{text}"
    );

    let space = ok(&d, &["space", "new", project.to_str().unwrap()]).await;
    let project_text = watch_during(
        &d,
        &["--timeout", "10"],
        &[
            "config",
            "set",
            "browser.homepage",
            "https://project.example",
            "--space",
            &space,
        ],
    )
    .await;
    let text = stdout(&project_text);
    assert!(
        text.starts_with(&format!("settings space={space}\n")),
        "{text}"
    );
    assert!(
        text.contains("browser.homepage = https://project.example   (project)"),
        "{text}"
    );

    // A removal is broadcast too: the machine's value is back in force.
    let unset = watch_during(
        &d,
        &["--timeout", "10", "--json"],
        &["config", "unset", "browser.homepage", "--space", &space],
    )
    .await;
    assert_eq!(unset.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_str(&stdout(&unset)).unwrap();
    assert_eq!(json["space"], space.as_str());
    let home = json["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "browser.homepage")
        .unwrap();
    assert_eq!(home["value"], "https://machine.example");
    assert_eq!(home["origin"], "machine");

    let quiet = wheel(&d, &["config", "watch", "--timeout", "0.5"]).await;
    assert_eq!(quiet.status.code(), Some(3), "no broadcast is a timeout");
    let _ = std::fs::remove_dir_all(&support);
}

#[tokio::test(flavor = "multi_thread")]
async fn lead_login_with_no_space_open_makes_a_space_for_its_terminal() {
    let d = daemon_with_lead_running("--auth").await;
    let before: serde_json::Value = serde_json::from_str(&ok(&d, &["space", "ls"]).await).unwrap();
    assert_eq!(before["spaces"].as_array().unwrap().len(), 0, "{before}");
    for _ in 0..100 {
        let status: serde_json::Value =
            serde_json::from_str(&ok(&d, &["lead", "status"]).await).unwrap();
        if !status["login"]["methods"].as_array().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let said = ok(&d, &["lead", "login"]).await;
    assert!(
        said.starts_with("claude-ai-login p") && said.ends_with("exit 0"),
        "{said}"
    );
    let after: serde_json::Value = serde_json::from_str(&ok(&d, &["space", "ls"]).await).unwrap();
    assert_eq!(after["spaces"].as_array().unwrap().len(), 1, "{after}");
    assert_eq!(after["spaces"][0]["name"], "Login", "{after}");
}

#[tokio::test(flavor = "multi_thread")]
async fn lead_login_runs_a_method_in_a_pane_and_exits_with_the_commands_code() {
    let d = daemon_with_lead_running("--auth").await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;

    // The agent advertises its login methods once it has started.
    let mut status = serde_json::Value::Null;
    for _ in 0..100 {
        status = serde_json::from_str(&ok(&d, &["lead", "status"]).await).unwrap();
        if !status["login"]["methods"].as_array().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let ids: Vec<_> = status["login"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["claude-ai-login", "console-login"], "{status}");
    assert!(
        ["ok", "needed", "unknown"].contains(&status["login"]["state"].as_str().unwrap()),
        "{status}"
    );
    assert!(status["claude"]["why"]
        .as_str()
        .unwrap()
        .contains("preflight"));

    // A login that fails exits with its code and says which pane ran it.
    let failed = wheel(&d, &["lead", "login", "console-login"]).await;
    assert_eq!(
        failed.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        stdout(&failed).starts_with("console-login p"),
        "{}",
        stdout(&failed)
    );
    assert!(
        stdout(&failed).trim().ends_with("exit 3"),
        "{}",
        stdout(&failed)
    );

    // The default method is Claude Subscription; its success exits 0.
    let said = ok(&d, &["lead", "login"]).await;
    assert!(
        said.starts_with("claude-ai-login p") && said.ends_with("exit 0"),
        "{said}"
    );
    let status: serde_json::Value =
        serde_json::from_str(&ok(&d, &["lead", "status"]).await).unwrap();
    assert_eq!(status["login"]["last_exit"]["code"], 0, "{status}");

    // A method the agent never advertised is refused.
    let unknown = wheel(&d, &["lead", "login", "nope"]).await;
    assert_eq!(unknown.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("claude-ai-login"));
}

struct FixedUsage(Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure>);

impl wheeld::usage::UsageSource for FixedUsage {
    fn read(&self) -> Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure> {
        self.0.clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_prints_the_reading_and_exits_3_when_it_is_missing() {
    let fresh = daemon_with_lead_reading(
        "",
        Some(Arc::new(FixedUsage(Ok(wheeld::usage::RawUsage {
            session_pct: 42.0,
            session_resets_at: Some("2026-10-17T20:00:00Z".into()),
            weekly_pct: 7.5,
            weekly_resets_at: None,
        })))),
    )
    .await;
    let out = ok(&fresh, &["usage"]).await;
    assert!(
        out.contains("5-hour: 42% (resets 2026-10-17T20:00:00Z)"),
        "{out}"
    );
    assert!(out.contains("weekly: 7.5% (resets -)"), "{out}");
    assert!(out.contains("source: oauth/usage via lead login"), "{out}");
    assert!(out.contains("age: 0 s"), "{out}");
    // `lead status` carries the same reading.
    let status: serde_json::Value =
        serde_json::from_str(&ok(&fresh, &["lead", "status"]).await).unwrap();
    assert_eq!(status["usage"]["kind"], "fresh", "{status}");
    assert_eq!(status["usage"]["reading"]["session_pct"], 42.0, "{status}");

    let missing = daemon_with_lead_reading(
        "",
        Some(Arc::new(FixedUsage(Err(
            wheeld::usage::UsageFailure::NotLoggedIn("lead is not logged in".into()),
        )))),
    )
    .await;
    let o = wheel(&missing, &["usage"]).await;
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    assert_eq!(stdout(&o).trim(), "missing: lead is not logged in");
}
