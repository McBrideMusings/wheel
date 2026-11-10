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
    if let Some(s) = status.filter(|s| !s.success()) {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let tail: String = text.trim().lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
        return Err(format!("{label} failed ({s}): {tail}"));
    }
    if status.is_none() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
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
    fn json_parses_and_an_empty_output_is_null() {
        assert_eq!(json("/bin/echo", "/", &["[1]"]).unwrap(), serde_json::json!([1]));
        assert_eq!(json("/usr/bin/true", "/", &[]).unwrap(), Value::Null);
        assert!(json("/bin/echo", "/", &["nope"]).is_err());
    }
}
