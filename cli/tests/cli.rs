//! The `wheel` binary against a real daemon. A script relies on the output and the exit code, so
//! both are checked: 0 when the text appears, 3 on timeout, 4 when the shell exits first, 5 when
//! the token is wrong, 6 when the daemon refuses the request, 7 when the connection drops. `wheel
//! usage` also exits 3 when the reading is stale or missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

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
        .args(args)
        .env_remove("WHEEL_SPACE")
        .env_remove("WHEEL_PANE");
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
async fn space_ls_json_is_a_flat_array_of_id_name_cwd_status_and_pane_ids() {
    let d = daemon().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let dir = dir.to_str().unwrap();
    let space = ok(&d, &["space", "new", dir, "--name", "proj"]).await;
    let second = ok(&d, &["pane", "split", &first_pane(&d).await, "right"]).await;

    let rows: serde_json::Value =
        serde_json::from_str(&ok(&d, &["space", "ls", "--json"]).await).unwrap();
    let rows = rows.as_array().expect("a bare array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], space);
    assert_eq!(rows[0]["name"], "proj");
    assert_eq!(rows[0]["cwd"], dir);
    assert!(rows[0]["status"].is_null());
    let panes: Vec<&str> = rows[0]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(panes.len(), 2);
    assert!(panes.contains(&second.as_str()));
}

async fn first_pane(d: &Daemon) -> String {
    layout(d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn space_and_pane_commands_print_ids_and_change_the_layout() {
    let d = daemon().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let dir = dir.to_str().unwrap();

    let space = ok(&d, &["space", "new", dir, "--name", "proj"]).await;
    assert!(space.starts_with('s'), "printed a space id: {space:?}");
    let l = layout(&d).await;
    assert!(l.get("active").is_none(), "wheeld keeps no active space");
    assert_eq!(l["spaces"][0]["name"], "proj");
    assert_eq!(l["spaces"][0]["cwd"], dir);
    let first = l["spaces"][0]["root"]["id"].as_str().unwrap().to_string();
    assert_eq!(l["spaces"][0]["root"]["kind"], "empty");

    let unfocused = ok(&d, &["pane", "split", &first, "down"]).await;
    ok(&d, &["pane", "close", &unfocused]).await;
    let second = ok(&d, &["pane", "split", &first, "right", "--focus"]).await;
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

    let quiet = ok(&d, &["space", "new", dir]).await;
    ok(&d, &["space", "close", &quiet]).await;
    let other = ok(&d, &["space", "new", dir, "--focus"]).await;
    ok(&d, &["pane", "focus", &first]).await;
    ok(&d, &["space", "focus", &other]).await;

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

/// The scripted fake agent, which belongs to the `wheeld` package, so `cargo test -p wheel-cli`
/// does not build it. The first caller of each run builds it with the profile and target
/// directory this test binary was built with, so a test never runs a binary from an earlier build.
fn fake_agent() -> &'static Path {
    static FAKE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FAKE.get_or_init(|| {
        let wheel = Path::new(env!("CARGO_BIN_EXE_wheel"));
        let profile = wheel
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let mut build = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        // This process carries the variables Cargo sets for a crate it runs. Passed on, they reach
        // this build's build scripts, and ring's watches CARGO_MANIFEST_DIR: ring and every crate
        // above it would recompile here, and again in the next plain `cargo build`. With them
        // removed, the build is a no-op once the agent is built; every other variable passes on.
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(set_by_cargo_for_a_crate) {
                build.env_remove(key);
            }
        }
        build.current_dir(env!("CARGO_MANIFEST_DIR")).args([
            "build",
            "-p",
            "wheeld",
            "--bin",
            "wheel-fake-agent",
        ]);
        match profile {
            "debug" => {}
            "release" => {
                build.arg("--release");
            }
            other => {
                build.args(["--profile", other]);
            }
        }
        let status = build.status().expect("run cargo to build wheel-fake-agent");
        assert!(status.success(), "building wheel-fake-agent failed");
        wheel.with_file_name("wheel-fake-agent")
    })
}

/// The variables Cargo sets for a crate it compiles or runs, as the Cargo reference lists them
/// ("Environment variables Cargo sets for crates"), as opposed to ones a person or config set.
fn set_by_cargo_for_a_crate(key: &str) -> bool {
    matches!(
        key,
        "CARGO_MANIFEST_DIR"
            | "CARGO_MANIFEST_PATH"
            | "CARGO_MANIFEST_LINKS"
            | "CARGO_CRATE_NAME"
            | "CARGO_BIN_NAME"
            | "CARGO_PRIMARY_PACKAGE"
            | "CARGO_TARGET_TMPDIR"
            | "CARGO_RUSTC_CURRENT_DIR"
    ) || key.starts_with("CARGO_PKG_")
        || key.starts_with("CARGO_BIN_EXE_")
}

/// A daemon whose agent panes run the scripted fake agent.
async fn daemon_with_agents() -> Daemon {
    daemon_with_agents_reading(None).await
}

/// Like `daemon_with_agents`, reading usage from `usage` when given.
async fn daemon_with_agents_reading(usage: Option<Arc<dyn wheeld::usage::UsageSource>>) -> Daemon {
    let fake = fake_agent();
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
    state
        .setting_set("agent.claude.command", fake.to_str().unwrap(), None, true)
        .unwrap();
    state.set_control(format!("ws://{addr}/ws"), token_file.clone(), None);
    if let Some(usage) = usage {
        state.set_usage_source(usage);
    }
    tokio::spawn(wheeld::serve(listener, state));
    Daemon {
        url: format!("ws://{addr}/ws"),
        token_file,
    }
}

/// An agent pane in a space for the temp directory, with `message` as its first message once it
/// has replied; returns the pane id.
async fn agent_pane(d: &Daemon, message: &str) -> String {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(d, &["space", "new", dir.to_str().unwrap()]).await;
    let pane = layout(d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(d, &["pane", "start", &pane, "agent", "--message", message]).await;
    ok(d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    pane
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
    let agent = started["agent"].as_str().unwrap().to_string();

    // `wait` prints the pane, the agent and the state once the first turn is done.
    let waited = ok(&d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    assert_eq!(waited, format!(r#"{pane} {agent} {{"phase":"ready"}}"#));
    let by_name = ok(&d, &["agent", "wait", &agent, "--timeout", "10"]).await;
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
        .contains(&format!("agent {agent} has no running turn to cancel")));
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_commands_prints_what_the_agent_reported() {
    let d = daemon_with_agents().await;
    let pane = agent_pane(&d, "first task").await;

    let plain = ok(&d, &["agent", "commands", &pane]).await;
    assert_eq!(plain.lines().collect::<Vec<_>>(), ["clear", "compact", "review"]);
    let json: serde_json::Value =
        serde_json::from_str(&ok(&d, &["agent", "commands", &pane, "--json"]).await).unwrap();
    assert_eq!(json, serde_json::json!(["clear", "compact", "review"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_read_prints_a_diff_as_counts_and_json_keeps_its_text() {
    let d = daemon_with_agents().await;
    let pane = agent_pane(&d, "show a diff").await;

    let plain = ok(&d, &["agent", "read", &pane]).await;
    assert!(plain.contains("  diff src/a.rs (-1 +1 lines)"), "{plain}");
    assert!(plain.contains("(completed)"), "{plain}");
    assert!(!plain.contains("(pending)"), "{plain}");

    let json: serde_json::Value =
        serde_json::from_str(&ok(&d, &["agent", "read", &pane, "--json"]).await).unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_terminal_and_a_chat_agent_know_where_wheel_is() {
    let d = daemon_with_agents().await;
    let pane = agent_pane(&d, "show setup").await;
    let seen = ok(&d, &["agent", "read", &pane]).await;
    let wanted = format!("wheel: env=1 space=- pane={pane} url={} token_file=", d.url);
    assert!(seen.contains(&wanted), "{seen}");
    assert!(seen.contains("/shell/bin allowed="), "{seen}");
    assert!(
        seen.contains("allowed=Bash(wheel:*) denied=Bash(wheel perm answer:*)"),
        "{seen}"
    );

    // A terminal pane has the same variables, and the CLI run in it reaches the daemon with no flag.
    ok(&d, &["pane", "kind", &pane, "terminal"]).await;
    let wheel_bin = env!("CARGO_BIN_EXE_wheel");
    ok(
        &d,
        &[
            "term",
            "send",
            &pane,
            &format!(
                "echo seat=${{WHEEL_SPACE-none}}/$WHEEL_PANE/$WHEEL_ENV; {wheel_bin} space ls | head -1"
            ),
            "--enter",
        ],
    )
    .await;
    let until = format!("seat=none/{pane}/1");
    let read = wheel(
        &d,
        &["term", "read", &pane, "--until", &until, "--timeout", "10"],
    )
    .await;
    assert_eq!(
        read.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_send_presses_named_keys() {
    let d = daemon().await;
    let id = open(&d).await;
    ok(
        &d,
        &["term", "send", &id, "echo key-$((6*7))", "--key", "enter"],
    )
    .await;
    let read = wheel(
        &d,
        &["term", "read", &id, "--until", "key-42", "--timeout", "10"],
    )
    .await;
    assert_eq!(read.status.code(), Some(0));
    // Ctrl-C ends a foreground sleep, so the prompt takes the next command.
    ok(&d, &["term", "send", &id, "sleep 30", "--key", "enter"]).await;
    ok(&d, &["term", "send", &id, "", "--key", "ctrl-c"]).await;
    ok(
        &d,
        &["term", "send", &id, "echo after-$((1+1))", "--key", "enter"],
    )
    .await;
    let read = wheel(
        &d,
        &["term", "read", &id, "--until", "after-2", "--timeout", "10"],
    )
    .await;
    assert_eq!(read.status.code(), Some(0));
    let bad = wheel(&d, &["term", "send", &id, "--key", "nonsense"]).await;
    assert_eq!(
        bad.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    let empty = wheel(&d, &["term", "send", &id]).await;
    assert!(!empty.status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn term_send_paste_wraps_text_in_bracketed_paste() {
    let d = daemon().await;
    let id = open(&d).await;
    // With echo off only cat -v prints, so ^[[200~ and ^[[201~ on screen are bytes the program
    // read. The end marker inside the text is dropped, so the paste is one piece.
    ok(&d, &["term", "send", &id, "stty -echo; cat -v", "--enter"]).await;
    ok(
        &d,
        &[
            "term",
            "send",
            &id,
            "--paste",
            "first\x1b[201~\nsecond",
            "--enter",
        ],
    )
    .await;
    let read = wheel(
        &d,
        &[
            "term",
            "read",
            &id,
            "--until",
            "second^[[201~",
            "--timeout",
            "10",
        ],
    )
    .await;
    assert_eq!(
        read.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&read.stdout)
    );
    let screen = String::from_utf8_lossy(&read.stdout);
    assert!(
        screen.contains("^[[200~first\r\nsecond^[[201~")
            || screen.contains("^[[200~first\nsecond^[[201~"),
        "{screen}"
    );
    let bare = wheel(&d, &["term", "send", &id, "--paste", "--enter"]).await;
    assert!(!bare.status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn space_status_prints_what_space_ls_carries() {
    let d = daemon_with_agents().await;
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let pane = layout(&d).await["spaces"][0]["root"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let name = layout(&d).await["spaces"][0]["name"]
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

    let ls = layout(&d).await;
    let needs = serde_json::json!({"kind": "needs_you", "waiting": 1});
    assert_eq!(ls["spaces"][0]["status"], needs, "{ls}");
    assert_eq!(ls["spaces"][0]["root"]["status"], needs, "{ls}");
    let status = ok(&d, &["space", "status"]).await;
    assert_eq!(
        status,
        format!("{name}  needs_you  waiting 1\n  {pane}  agent  needs_you  waiting 1"),
        "{status}"
    );

    let perm = ok(&d, &["perm", "wait", "--timeout", "10"]).await;
    let perm: serde_json::Value = serde_json::from_str(&perm).unwrap();
    ok(
        &d,
        &["perm", "answer", perm["id"].as_str().unwrap(), "allow"],
    )
    .await;
    ok(&d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    let ls = layout(&d).await;
    assert_eq!(
        ls["spaces"][0]["status"],
        serde_json::json!({"kind": "idle"}),
        "{ls}"
    );
    let status = ok(&d, &["space", "status"]).await;
    assert_eq!(
        status,
        format!("{name}  idle\n  {pane}  agent  idle"),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn perm_answer_takes_an_agent_pane_in_place_of_a_request_id() {
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
    let none = wheel(&d, &["perm", "answer", "p999", "allow"]).await;
    assert!(!none.status.success());
    ok(&d, &["perm", "answer", &pane, "allow"]).await;
    ok(&d, &["agent", "wait", &pane, "--timeout", "10"]).await;
    let done = ok(&d, &["agent", "read", &pane]).await;
    assert!(done.contains("permission: allow"), "{done}");
}

#[tokio::test(flavor = "multi_thread")]
async fn skill_prints_the_guide_and_help_points_at_it_without_a_daemon() {
    let skill = Command::new(env!("CARGO_BIN_EXE_wheel"))
        .arg("--skill")
        .output()
        .unwrap();
    assert_eq!(skill.status.code(), Some(0));
    let text = stdout(&skill);
    assert!(
        text.contains("WHEEL_PANE") && text.contains("wheel term send"),
        "{text}"
    );
    let help = Command::new(env!("CARGO_BIN_EXE_wheel"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(stdout(&help).contains("wheel --skill"));
}

#[test]
fn install_links_this_wheel_and_status_reports_it() {
    let dir = std::env::temp_dir().join(format!("wheel-install-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_wheel"))
            .env("PATH", format!("{}:/usr/bin", dir.display()))
            .args(args)
            .output()
            .unwrap()
    };
    let before = run(&["install", "--status", "--dir", dir.to_str().unwrap()]);
    assert!(
        stdout(&before).starts_with("missing:"),
        "{}",
        stdout(&before)
    );
    let done = run(&["install", "--dir", dir.to_str().unwrap()]);
    assert!(stdout(&done).starts_with("installed:"), "{}", stdout(&done));
    let after = run(&["install", "--status", "--dir", dir.to_str().unwrap()]);
    assert!(
        stdout(&after).starts_with("installed:"),
        "{}",
        stdout(&after)
    );
    // A real file already there is left alone.
    let other = std::env::temp_dir().join(format!("wheel-install-other-{}", std::process::id()));
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("wheel"), "mine").unwrap();
    let refused = run(&["install", "--dir", other.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert_eq!(
        std::fs::read_to_string(other.join("wheel")).unwrap(),
        "mine"
    );
    let _ = std::fs::remove_dir_all(&other);
    let target = std::fs::canonicalize(dir.join("wheel")).unwrap();
    assert_eq!(
        target,
        std::fs::canonicalize(env!("CARGO_BIN_EXE_wheel")).unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
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

struct FixedUsage(Result<wheeld::usage::RawUsage, wheeld::usage::UsageFailure>);

impl wheeld::usage::UsageSource for FixedUsage {
    fn read(&self) -> Vec<wheeld::usage::LoginRead> {
        vec![wheeld::usage::LoginRead {
            key: "acct".into(),
            name: ".claude".into(),
            dirs: Vec::new(),
            result: self.0.clone(),
        }]
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_prints_the_reading_and_exits_3_when_it_is_missing() {
    let fresh =
        daemon_with_agents_reading(Some(Arc::new(FixedUsage(Ok(wheeld::usage::RawUsage {
            session_pct: 42.0,
            session_resets_at: Some("2026-10-17T20:00:00Z".into()),
            weekly_pct: 7.5,
            weekly_resets_at: None,
            ..Default::default()
        })))))
        .await;
    let out = ok(&fresh, &["usage"]).await;
    assert!(out.contains("account: .claude"), "{out}");
    assert!(
        out.contains("5-hour: 42% (resets 2026-10-17T20:00:00Z)"),
        "{out}"
    );
    assert!(out.contains("weekly: 7.5% (resets -)"), "{out}");
    assert!(
        out.contains("source: oauth/usage via Claude Code logins"),
        "{out}"
    );
    assert!(out.contains("age: 0 s"), "{out}");
    // The strip's rows: the default pins, and Codex listed but not shown.
    assert!(out.contains("strip: claude .claude session 42%"), "{out}");
    assert!(out.contains("strip: claude .claude weekly 7.5%"), "{out}");
    assert!(out.contains("agent: codex not shown"), "{out}");

    let missing = daemon_with_agents_reading(Some(Arc::new(FixedUsage(Err(
        wheeld::usage::UsageFailure::NotLoggedIn("signed out".into()),
    )))))
    .await;
    let o = wheel(&missing, &["usage"]).await;
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    assert!(
        stdout(&o).starts_with("account: .claude\nmissing: signed out\n"),
        "{o:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_near_names_each_neighbour_with_its_side_kind_and_page() {
    let d = daemon().await;
    let dir = std::env::temp_dir().join(format!("wheel-pane-near-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let space = ok(&d, &["space", "new", dir.to_str().unwrap()]).await;
    let left = first_pane(&d).await;
    let right = ok(&d, &["pane", "split", &left, "right"]).await;
    let below = ok(&d, &["pane", "split", &right, "down"]).await;
    ok(
        &d,
        &[
            "pane",
            "kind",
            &right,
            "browser",
            "--page",
            "https://etv.example.com/",
        ],
    )
    .await;

    let text = ok(&d, &["pane", "near", &left]).await;
    assert_eq!(
        text,
        format!("right {right} browser https://etv.example.com/\nright {below} empty")
    );
    let rows: serde_json::Value =
        serde_json::from_str(&ok(&d, &["pane", "near", &right, "--json"]).await).unwrap();
    let sides: Vec<(&str, &str)> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["side"].as_str().unwrap(), r["pane"].as_str().unwrap()))
        .collect();
    assert_eq!(sides, [("left", left.as_str()), ("down", below.as_str())]);
    ok(&d, &["artifact", "new", "viewer", "--space", &space]).await;
    ok(&d, &["pane", "kind", &below, "web", "--artifact", "viewer"]).await;
    let text = ok(&d, &["pane", "near", &right]).await;
    assert!(
        text.ends_with(&format!("down {below} web artifact viewer (project)")),
        "{text}"
    );
    let missing = wheel(&d, &["pane", "near", "p999"]).await;
    assert_eq!(missing.status.code(), Some(6));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_defaults_to_the_space_holding_wheel_pane_now() {
    let d = daemon().await;
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("wheel-pane-space-{}", std::process::id()));
    let (a, b) = (root.join("a"), root.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let space_a = ok(&d, &["space", "new", a.to_str().unwrap()]).await;
    let pane_a = first_pane(&d).await;
    let space_b = ok(&d, &["space", "new", b.to_str().unwrap()]).await;

    // The pane sits in space a while space b is active; `--space` still wins over the pane.
    let in_pane = |args: Vec<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wheel"));
        cmd.args([
            "--url",
            &d.url,
            "--token-file",
            d.token_file.to_str().unwrap(),
        ])
        .args(args)
        .env("WHEEL_PANE", &pane_a)
        .env_remove("WHEEL_SPACE");
        async move {
            let o = tokio::task::spawn_blocking(move || cmd.output().unwrap())
                .await
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            stdout(&o).trim().to_string()
        }
    };
    let made = in_pane(vec!["artifact", "new", "x"]).await;
    assert!(
        made.starts_with(a.to_str().unwrap()),
        "{made} is under {a:?}"
    );
    let forced = in_pane(vec!["artifact", "new", "y", "--space", &space_b]).await;
    assert!(
        forced.starts_with(b.to_str().unwrap()),
        "{forced} is under {b:?}"
    );
    assert_ne!(space_a, space_b);
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_new_outside_a_pane_without_space_fails_naming_space() {
    let d = daemon().await;
    ok(&d, &["space", "new", &temp_dir_str()]).await;
    let o = wheel(&d, &["artifact", "new", "x"]).await;
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(err.contains("--space"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_ls_prints_a_json_array_of_the_clients_that_reported_a_view() {
    let d = daemon().await;
    let rows: serde_json::Value =
        serde_json::from_str(&ok(&d, &["client", "ls", "--json"]).await).unwrap();
    assert_eq!(rows, serde_json::json!([]), "a command-line call reports no view");
}

/// `wheel session ls --json` as a value.
async fn sessions(d: &Daemon) -> Vec<serde_json::Value> {
    serde_json::from_str::<Vec<serde_json::Value>>(&ok(d, &["session", "ls", "--json"]).await)
        .unwrap()
}

/// The session ids that name `space`; a space is in exactly one, so this has one element.
async fn sessions_holding(d: &Daemon, space: &str) -> Vec<String> {
    sessions(d)
        .await
        .iter()
        .filter(|s| s["spaces"].as_array().unwrap().iter().any(|x| x == space))
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn temp_dir_str() -> String {
    std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn session_new_ls_rename_and_close_change_the_list_and_close_ends_its_spaces() {
    let d = daemon().await;
    let dir = temp_dir_str();
    let work = ok(&d, &["session", "new", "work"]).await;
    assert!(work.starts_with('n'), "printed a session id: {work:?}");
    let listed = ok(&d, &["session", "ls"]).await;
    assert!(listed.contains(&work) && listed.contains("work"), "{listed}");

    ok(&d, &["session", "rename", "work", "play"]).await;
    let all = sessions(&d).await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["name"], "play");
    assert!(!wheel(&d, &["session", "rename", &work, "  "]).await.status.success());

    let space = ok(&d, &["space", "new", &dir, "--session", &work]).await;
    assert_eq!(sessions_holding(&d, &space).await, vec![work.clone()]);

    ok(&d, &["session", "close", &work]).await;
    assert!(sessions(&d).await.is_empty());
    assert_eq!(layout(&d).await["spaces"], serde_json::json!([]));
    assert_eq!(wheel(&d, &["session", "close", &work]).await.status.code(), Some(6));
}

#[tokio::test(flavor = "multi_thread")]
async fn session_move_puts_a_space_in_one_session_and_never_two_or_none() {
    let d = daemon().await;
    let dir = temp_dir_str();
    let a = ok(&d, &["session", "new", "a"]).await;
    let b = ok(&d, &["session", "new", "b"]).await;
    let s1 = ok(&d, &["space", "new", &dir, "--session", &a]).await;
    let s2 = ok(&d, &["space", "new", &dir, "--session", &a]).await;
    assert_eq!(sessions_holding(&d, &s1).await, vec![a.clone()]);

    ok(&d, &["session", "move", &s1, &b]).await;
    assert_eq!(sessions_holding(&d, &s1).await, vec![b.clone()]);
    assert_eq!(sessions_holding(&d, &s2).await, vec![a.clone()]);

    ok(&d, &["session", "move", &s2, &b, "--index", "0"]).await;
    let all = sessions(&d).await;
    let b_row = all.iter().find(|s| s["id"] == b.as_str()).unwrap();
    assert_eq!(b_row["spaces"], serde_json::json!([s2, s1]));
    // Moving into the session it is already in keeps it in exactly one.
    ok(&d, &["session", "move", &s2, &b]).await;
    assert_eq!(sessions_holding(&d, &s2).await, vec![b.clone()]);

    let held: usize = sessions(&d)
        .await
        .iter()
        .map(|s| s["spaces"].as_array().unwrap().len())
        .sum();
    assert_eq!(held, 2, "two spaces, each counted once");
    assert_eq!(wheel(&d, &["session", "move", &s1, "nope"]).await.status.code(), Some(6));

    ok(&d, &["session", "reorder", &b, "0"]).await;
    assert_eq!(sessions(&d).await[0]["id"], b.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_space_made_from_a_pane_joins_that_panes_session() {
    let d = daemon().await;
    let dir = temp_dir_str();
    let a = ok(&d, &["session", "new", "a"]).await;
    let b = ok(&d, &["session", "new", "b"]).await;
    let in_a = ok(&d, &["space", "new", &dir, "--session", &a]).await;
    let in_b = ok(&d, &["space", "new", &dir, "--session", &b]).await;
    let l = layout(&d).await;
    let pane_of = |space: &str| {
        l["spaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == space)
            .unwrap()["root"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (pane_a, pane_b) = (pane_of(&in_a), pane_of(&in_b));

    // The real path: `WHEEL_PANE` in the environment, as a pane's shell has it.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wheel"));
    cmd.args(["--url", &d.url, "--token-file", d.token_file.to_str().unwrap()])
        .args(["space", "new", &dir])
        .env("WHEEL_PANE", &pane_a)
        .env_remove("WHEEL_SPACE");
    let o = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let from_a = stdout(&o).trim().to_string();
    assert_eq!(sessions_holding(&d, &from_a).await, vec![a.clone()]);

    let from_b = ok(&d, &["space", "new", &dir, "--pane", &pane_b]).await;
    assert_eq!(sessions_holding(&d, &from_b).await, vec![b.clone()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_keeps_its_sessions_and_groups_an_old_layout_into_one() {
    let root = std::env::temp_dir().join(format!("wheel-cli-sessions-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("spaces.json");
    let dir = temp_dir_str();

    let start = |state: PathBuf, root: PathBuf| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token_file = root.join(format!("token-{}", addr.port()));
        std::fs::write(&token_file, TOKEN).unwrap();
        let app = wheeld::AppState::new(
            "/dev/null".into(),
            TOKEN.into(),
            root,
            "/usr/bin/false".into(),
        );
        app.restore_spaces(state).await.unwrap();
        let serving = tokio::spawn(wheeld::serve(listener, app));
        (
            Daemon {
                url: format!("ws://{addr}/ws"),
                token_file,
            },
            serving,
        )
    };

    let (d, serving) = start(state.clone(), root.clone()).await;
    let work = ok(&d, &["session", "new", "work"]).await;
    let space = ok(&d, &["space", "new", &dir, "--session", &work]).await;
    let other = ok(&d, &["session", "new", "other"]).await;
    serving.abort();
    let _ = serving.await;

    let (d, serving) = start(state.clone(), root.clone()).await;
    let all = sessions(&d).await;
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0]["id"], work.as_str());
    assert_eq!(all[0]["name"], "work");
    assert_eq!(all[0]["spaces"], serde_json::json!([space]));
    assert_eq!(all[1]["id"], other.as_str());
    serving.abort();
    let _ = serving.await;

    // A layout saved before sessions existed: two spaces, no `sessions` key.
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    saved.as_object_mut().unwrap().remove("sessions");
    let mut second = saved["spaces"][0].clone();
    second["id"] = "s900".into();
    second["root"]["id"] = "p900".into();
    saved["spaces"].as_array_mut().unwrap().push(second);
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();

    let (d, serving) = start(state.clone(), root.clone()).await;
    let all = sessions(&d).await;
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["name"], "Main");
    assert_eq!(all[0]["spaces"], serde_json::json!([space, "s900"]));
    serving.abort();
    let _ = serving.await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_named_session_wins_over_wheel_pane_and_a_stale_pane_is_ignored() {
    let d = daemon().await;
    let dir = temp_dir_str();
    let a = ok(&d, &["session", "new", "a"]).await;
    let b = ok(&d, &["session", "new", "b"]).await;
    let in_a = ok(&d, &["space", "new", &dir, "--session", &a]).await;
    let pane_a = layout(&d).await["spaces"][0]["root"]["id"].as_str().unwrap().to_string();
    let _ = in_a;
    let named = ok(&d, &["space", "new", &dir, "--pane", &pane_a, "--session", &b]).await;
    assert_eq!(sessions_holding(&d, &named).await, vec![b.clone()]);
    let stale = ok(&d, &["space", "new", &dir, "--pane", "p99999"]).await;
    assert_eq!(sessions_holding(&d, &stale).await.len(), 1);
}

struct TwoLogins;

impl wheeld::usage::UsageSource for TwoLogins {
    fn read(&self) -> Vec<wheeld::usage::LoginRead> {
        let login = |name: &str, weekly_pct: f64| wheeld::usage::LoginRead {
            key: name.into(),
            name: name.into(),
            dirs: vec![format!("/logins/{name}").into()],
            result: Ok(wheeld::usage::RawUsage {
                weekly_pct,
                ..Default::default()
            }),
        };
        vec![login(".claude", 12.0), login(".claude-work", 71.0)]
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_info_reports_the_weekly_limit_of_the_panes_login() {
    let d = daemon_with_agents_reading(Some(Arc::new(TwoLogins))).await;
    let pane = agent_pane(&d, "hello").await;
    ok(&d, &["usage"]).await;
    for (dir, weekly) in [("/logins/.claude", "12%"), ("/logins/.claude-work", "71%")] {
        ok(&d, &["config", "set", "agent.claude.home", dir, "--machine"]).await;
        let info = ok(&d, &["pane", "info", &pane]).await;
        assert!(info.contains(&format!("Weekly: {weekly}")), "{info}");
        let json = ok(&d, &["pane", "info", &pane, "--json"]).await;
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["login"]["dir"], dir, "{json}");
        assert_eq!(
            json["login"]["account"]["usage"]["reading"]["weekly_pct"],
            weekly.trim_end_matches('%').parse::<f64>().unwrap(),
            "{json}"
        );
    }
}
