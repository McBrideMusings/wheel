//! The `wheel` binary against a real daemon. A script relies on the output and the exit code, so
//! both are checked: 0 when the text appears, 3 on timeout, 4 when the shell exits first, 5 when
//! the token is wrong, 6 when the daemon refuses the request.

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
        wheeld::AppState::new("/dev/null".into(), TOKEN.into()),
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

async fn open(d: &Daemon) -> String {
    let o = wheel(d, &["term", "open", "--cwd", "/tmp"]).await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    stdout(&o).trim().to_string()
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
