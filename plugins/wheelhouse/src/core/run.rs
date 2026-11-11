//! Running the project's tools in the project's directory: `bd`, `gh`, `git`.

use std::process::Command;

use serde_json::Value;

/// Run `program args` in `dir` and return its standard output.
pub fn text(program: &str, dir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.chars().take(300).collect());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `program args` in `dir`; `Ok` when it exits with 0 or with a code in `also_ok`.
pub fn text_or_exit(program: &str, dir: &str, args: &[&str], also_ok: &[i32]) -> Result<(), String> {
    let out = Command::new(program).args(args).current_dir(dir).output().map_err(|e| format!("{program}: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(c) if also_ok.contains(&c) => Ok(()),
        _ => Err(String::from_utf8_lossy(&out.stderr).chars().take(300).collect()),
    }
}

/// Run `program args` in `dir` and parse its standard output as JSON; an empty output is `null`.
pub fn json(program: &str, dir: &str, args: &[&str]) -> Result<Value, String> {
    let out = text(program, dir, args)?;
    if out.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&out).map_err(|e| format!("{program} {}: not JSON: {e}", args.join(" ")))
}

/// `bd <args> --json --limit 0` in `dir`: the issues it lists.
pub fn bd(dir: &str, args: &[&str]) -> Result<Vec<Value>, String> {
    let mut all = args.to_vec();
    all.extend(["--json", "--limit", "0"]);
    match json("bd", dir, &all)? {
        Value::Array(rows) => Ok(rows),
        Value::Null => Ok(vec![]),
        other => Err(format!("bd {}: expected a list, got {other}", args.join(" "))),
    }
}

/// `gh <args>` in `dir`.
pub fn gh(dir: &str, args: &[&str]) -> Result<String, String> {
    text("gh", dir, args)
}

/// Run a page button's shell command `sh` in `dir` with `env` set (and `WHEEL_PANE` when `pane` names a
/// pane), its output in `log`. A command that fails at once is an error carrying the tail of its output;
/// one still running after ten seconds counts as started and is left to finish.
pub fn button(label: &str, sh: &str, dir: &str, pane: &str, env: &[(&str, &str)], log: &std::path::Path) -> Result<(), String> {
    button_then(label, sh, dir, pane, env, log, |_| {})
}

/// `button`, and `then` gets the last line the command printed once it has exited with 0, or `None` when it
/// failed after the ten seconds: at once when the command was quick (a quick failure is the `Err`, and `then`
/// does not run), else from the thread that waits for it.
pub fn button_then(
    label: &str,
    sh: &str,
    dir: &str,
    pane: &str,
    env: &[(&str, &str)],
    log: &std::path::Path,
    then: impl FnOnce(Option<String>) + Send + 'static,
) -> Result<(), String> {
    let file = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut command = Command::new("/bin/sh");
    if pane != "default" {
        command.env("WHEEL_PANE", pane);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command
        .arg("-c")
        .arg(sh)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(file.try_clone().map_err(|e| e.to_string())?)
        .stderr(file)
        .spawn()
        .map_err(|e| format!("{label}: {e}"))?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() > std::time::Duration::from_secs(10) => break None,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let path = log.to_path_buf();
    let last = move || {
        std::fs::read_to_string(&path).unwrap_or_default().lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
    };
    if let Some(s) = status {
        if !s.success() {
            let text = std::fs::read_to_string(log).unwrap_or_default();
            let tail: String = text.trim().lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
            return Err(format!("{label} failed ({s}): {tail}"));
        }
        then(Some(last()));
        return Ok(());
    }
    std::thread::spawn(move || {
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        then(ok.then(last));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_returned_and_failure_names_the_error() {
        assert_eq!(text("/bin/echo", "/", &["hi"]).unwrap(), "hi\n");
        assert!(text("/bin/ls", "/", &["/no/such/path"]).is_err());
        assert!(text("/no/such/program", "/", &[]).unwrap_err().contains("/no/such/program"));
    }

    #[test]
    fn a_button_hands_the_last_line_it_printed_to_then() {
        let dir = std::env::temp_dir().join(format!("wheel-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        button_then("t", "echo noise; echo p42; echo", "/", "default", &[], &dir.join("t.log"), move |l| tx.send(l).unwrap()).unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("p42"));
        let err = button_then("t", "echo boom >&2; exit 3", "/", "default", &[], &dir.join("e.log"), |_| panic!("then ran")).unwrap_err();
        assert!(err.contains("boom"), "{err}");
        let (tx, rx) = std::sync::mpsc::channel();
        button_then("t", "sleep 11; echo p7", "/", "default", &[], &dir.join("slow.log"), move |l| tx.send(l).unwrap()).unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("p7"), "a command past ten seconds still reports its pane");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn json_parses_and_an_empty_output_is_null() {
        assert_eq!(json("/bin/echo", "/", &["[1]"]).unwrap(), serde_json::json!([1]));
        assert_eq!(json("/usr/bin/true", "/", &[]).unwrap(), Value::Null);
        assert!(json("/bin/echo", "/", &["nope"]).is_err());
    }
}
