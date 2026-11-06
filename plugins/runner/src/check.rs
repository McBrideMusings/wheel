//! Per-command status checks: the readout `status = { kind = … }` on a `[runner]` entry names.
//! Every check is bounded by [`TIMEOUT`], so a hung host or script delays one readout only.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::StatusCheck;

pub const TIMEOUT: Duration = Duration::from_secs(5);

/// One readout: whether the check succeeded and the text to show.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Reading {
    pub up: bool,
    pub text: String,
}

fn down(text: impl ToString) -> Reading {
    Reading { up: false, text: text.to_string() }
}

pub fn run(check: &StatusCheck, cwd: &Path) -> Reading {
    match check {
        StatusCheck::Http { url } => match get(url) {
            Ok(_) => Reading { up: true, text: "up".into() },
            Err(e) => down(e),
        },
        StatusCheck::Json { url, pull } => match get(url).and_then(|body| pull_path(&body, pull)) {
            Ok(text) => Reading { up: true, text },
            Err(e) => down(e),
        },
        StatusCheck::Command { run } => command(run, cwd),
    }
}

fn get(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if !(200..400).contains(&code) {
        return Err(format!("HTTP {code}"));
    }
    resp.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| e.to_string())
}

/// The value at a dotted path (`data.0.name`) of a JSON document, as text.
fn pull_path(body: &str, path: &str) -> Result<String, String> {
    let doc: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let mut at = &doc;
    for key in path.split('.').filter(|k| !k.is_empty()) {
        at = match at {
            serde_json::Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
            other => other.get(key),
        }
        .ok_or_else(|| format!("no {path} in the answer"))?;
    }
    Ok(match at {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// What a bounded shell command left: its exit status (None when it was killed at the limit) and
/// the first line of its standard output.
pub struct Bounded {
    pub code: Option<i32>,
    pub first_line: String,
    pub timed_out: bool,
}

/// Run `sh -c run` in `cwd` for at most `limit`. Standard output is read on a thread, so a command
/// that fills the pipe or leaves a background child holding it cannot outlast the limit; its whole
/// process group is ended on every exit. Git never asks for credentials.
pub fn bounded(run: &str, cwd: &Path, limit: Duration) -> Result<Bounded, String> {
    let mut child = Command::new("sh").arg("-c").arg(run).current_dir(cwd).env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn().map_err(|e| format!("sh: {e}"))?;
    let pgid = child.id() as i32;
    let mut so = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut first = String::new();
        let mut buf = [0u8; 4096];
        let mut seen = Vec::new();
        while let Ok(n) = so.read(&mut buf) {
            if n == 0 {
                break;
            }
            if first.is_empty() {
                seen.extend_from_slice(&buf[..n]);
                if let Some(end) = seen.iter().position(|b| *b == b'\n').or((seen.len() > 4096).then_some(seen.len())) {
                    first = String::from_utf8_lossy(&seen[..end]).into_owned();
                }
            }
        }
        if first.is_empty() { String::from_utf8_lossy(&seen).lines().next().unwrap_or("").to_string() } else { first }
    });
    let started = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(s)) => break (s.code(), false),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => break (None, true),
        }
    };
    // SAFETY: killpg only signals the group the child leads.
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
    let _ = child.wait();
    let first_line = reader.join().unwrap_or_default();
    Ok(Bounded { code, first_line, timed_out })
}

fn command(run: &str, cwd: &Path) -> Reading {
    match bounded(run, cwd, TIMEOUT) {
        Err(e) => down(e),
        Ok(b) if b.timed_out => down(format!("timed out after {}s", TIMEOUT.as_secs())),
        Ok(b) if b.code == Some(0) => Reading { up: true, text: if b.first_line.is_empty() { "up".into() } else { b.first_line } },
        Ok(b) => down(if b.first_line.is_empty() { format!("exit {}", b.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into())) } else { b.first_line }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_path_reads_objects_and_arrays() {
        assert_eq!(pull_path(r#"{"a":{"b":[{"c":"x"},2]}}"#, "a.b.0.c").unwrap(), "x");
        assert_eq!(pull_path(r#"{"a":{"b":[{"c":"x"},2]}}"#, "a.b.1").unwrap(), "2");
        assert!(pull_path(r#"{"a":1}"#, "a.b").is_err());
        assert!(pull_path("nope", "a").is_err());
    }

    #[test]
    fn a_command_check_ends_at_the_limit_even_when_a_child_holds_the_pipe() {
        let tmp = std::env::temp_dir();
        let t = Instant::now();
        let b = bounded("sleep 30 & echo up", &tmp, Duration::from_secs(2)).unwrap();
        assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
        assert_eq!((b.code, b.first_line.as_str()), (Some(0), "up"));
        let b = bounded("sleep 30", &tmp, Duration::from_millis(300)).unwrap();
        assert!(b.timed_out);
        let b = bounded("yes | head -c 300000; echo end", &tmp, Duration::from_secs(5)).unwrap();
        assert!(!b.timed_out && b.code == Some(0));
    }

    #[test]
    fn a_command_check_reads_exit_and_first_line() {
        let tmp = std::env::temp_dir();
        assert_eq!(run(&StatusCheck::Command { run: "echo v1.2; echo more".into() }, &tmp), Reading { up: true, text: "v1.2".into() });
        assert_eq!(run(&StatusCheck::Command { run: "echo broken; exit 3".into() }, &tmp), Reading { up: false, text: "broken".into() });
        assert_eq!(run(&StatusCheck::Command { run: "exit 4".into() }, &tmp), Reading { up: false, text: "exit 4".into() });
    }
}
