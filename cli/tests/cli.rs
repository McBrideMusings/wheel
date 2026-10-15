//! The `wheel` binary against a real daemon. A script relies on the output and the exit code, so
//! both are checked: 0 when the text appears, 3 on timeout, 4 when the shell exits first, 5 when
//! the token is wrong, 6 when the daemon refuses the request, 7 when the connection drops.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TOKEN: &str = "cli-test-token";

struct Daemon {
    url: String,
    token_file: PathBuf,
}

async fn daemon() -> Daemon {
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
            std::env::temp_dir(),
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
        std::env::temp_dir(),
        "/usr/bin/false".into(),
    );
    state.set_agent_command(fake.to_str().unwrap().into());
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

/// A daemon whose lead runs the scripted fake agent.
async fn daemon_with_lead() -> Daemon {
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
    state.set_agent_command(fake.to_str().unwrap().into());
    state.start_lead(
        fake.to_str().unwrap().into(),
        std::env::temp_dir(),
        &wheeld::mcp::url(addr),
    );
    tokio::spawn(wheeld::serve(listener, state));
    Daemon {
        url: format!("ws://{addr}/ws"),
        token_file,
    }
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
