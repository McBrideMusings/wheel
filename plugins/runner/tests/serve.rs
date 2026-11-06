//! `wheel-runner serve` over MCP: a host names the project in `_meta` `wheel/dir`, starts a
//! command, closes its page (here: asks again later) and reads the run's status and log back.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Host {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next: u64,
    dir: String,
}

impl Host {
    fn start(root: &std::path::Path) -> Host {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wheel-runner"))
            .arg("serve")
            .env("WHEEL_RUNNER_DIR", root.join("runs"))
            .env("WHEEL_MACHINE_FILE", root.join("machine.toml"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (stdin, out) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
        let mut h = Host { child, stdin, out, next: 1, dir: root.join("proj").to_string_lossy().into_owned() };
        h.request("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }), json!({}));
        h.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        h
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value, meta: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let mut params = params;
        params["_meta"] = meta;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(self.out.read_line(&mut line).unwrap() > 0, "the server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// A tool call for the project, as wheeld sends it; the result's text parsed as JSON.
    fn call(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let dir = self.dir.clone();
        let r = self.request("tools/call", json!({ "name": tool, "arguments": args }), json!({ "wheel/dir": dir, "wheel/pane": "p1" }));
        let result = &r["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or("").to_string();
        (result["isError"] != json!(true), serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_host_starts_a_command_and_reads_its_status_and_log_after_the_page_closed() {
    let root = std::env::temp_dir().join(format!("wheel-runner-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("proj")).unwrap();
    std::fs::write(root.join("machine.toml"), "").unwrap();
    std::fs::write(
        root.join("proj/wheel.toml"),
        "[runner]\nhello = \"echo hello-over-mcp; exit 4\"\nrestart = { command = \"echo restarted\", confirm = \"Ends every agent.\" }\n",
    )
    .unwrap();
    let mut host = Host::start(&root);

    let tools = host.request("tools/list", json!({}), json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["list_runners", "run_runner", "list_runs", "get_log", "stop_run", "get_status"] {
        assert!(names.contains(&want), "{names:?}");
    }

    let (ok, runners) = host.call("list_runners", json!({}));
    assert!(ok);
    let listed: Vec<&str> = runners.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(listed, ["hello", "restart"]);
    assert_eq!(runners[1]["confirm"], "Ends every agent.");

    // A command with confirm asks first and starts nothing.
    let (ok, asked) = host.call("run_runner", json!({ "name": "restart" }));
    assert!(ok && asked == json!({ "started": false, "confirm": "Ends every agent." }), "{asked}");
    assert_eq!(host.call("list_runs", json!({})).1, json!([]));

    let (ok, started) = host.call("run_runner", json!({ "name": "hello" }));
    assert!(ok && started["started"] == json!(true), "{started}");
    let id = started["id"].as_str().unwrap().to_string();

    // A later call (a reopened page) finds the run finished, with its exit code, log and status line.
    let end = Instant::now() + Duration::from_secs(10);
    let run = loop {
        let runs = host.call("list_runs", json!({})).1;
        if runs[0]["state"] == "exited" {
            break runs[0].clone();
        }
        assert!(Instant::now() < end, "the run did not end: {runs}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!((run["exit_code"].clone(), run["id"].clone()), (json!(4), json!(id)));
    let (_, log) = host.call("get_log", json!({ "id": id }));
    assert!(log["text"].as_str().unwrap().contains("hello-over-mcp"), "{log}");
    let next = log["next"].as_u64().unwrap();
    assert_eq!(host.call("get_log", json!({ "id": id, "offset": next })).1["text"], "");
    let (_, status) = host.call("get_status", json!({}));
    assert!(status["line"].as_str().unwrap().starts_with("✗ hello exit 4 in "), "{status}");

    let (ok, _) = host.call("run_runner", json!({ "name": "restart", "confirmed": true }));
    assert!(ok);
    assert!(!host.call("run_runner", json!({ "name": "nope" })).0, "an unknown runner is refused");
    let _ = std::fs::remove_dir_all(&root);
}
