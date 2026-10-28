//! wheeld stops cleanly on SIGTERM and SIGINT: it exits 0 and its last log lines reach the file.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A scratch directory removed when the test ends, failed or not.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stops_on(signal: &str) {
    let dir = Scratch(
        std::env::temp_dir().join(format!("wheeld-shutdown-{}-{signal}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let log = dir.0.join("wheeld.log");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_wheeld"))
        .args(["--listen", "127.0.0.1:0"])
        .arg("--log-dir")
        .arg(&dir.0)
        .arg("--support-dir")
        .arg(&dir.0)
        .arg("--machine-file")
        .arg(dir.0.join("wheel.toml"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let started = Instant::now();
    while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("spaces restored")) {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "wheeld never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let sent = Command::new("kill")
        .args([format!("-{signal}"), daemon.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());

    let stopped = Instant::now();
    let status = loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            break status;
        }
        if stopped.elapsed() > Duration::from_secs(60) {
            let _ = daemon.kill();
            panic!("wheeld still running 60s after SIG{signal}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "wheeld exited with {status:?}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains(&format!("support={}", dir.0.display())),
        "wheeld ran in an instance directory other than the scratch one: {text}"
    );
    assert!(
        text.contains(&format!("wheeld stopping signal=SIG{signal}")),
        "log: {text}"
    );
}

#[test]
fn sigterm_stops_the_daemon_and_flushes_its_log() {
    stops_on("TERM");
}

#[test]
fn sigint_stops_the_daemon_and_flushes_its_log() {
    stops_on("INT");
}
