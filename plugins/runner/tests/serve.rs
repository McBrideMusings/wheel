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
    /// The `resources/updated` URIs the server announced, in order.
    announced: Vec<String>,
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
        let mut h = Host { child, stdin, out, next: 1, announced: vec![], dir: root.join("proj").to_string_lossy().into_owned() };
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
            if v["method"] == "notifications/resources/updated" {
                self.announced.push(v["params"]["uri"].as_str().unwrap_or("").to_string());
            }
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
    assert!(ok && asked == json!({ "started": false, "confirm": "Ends every agent.", "command": "echo restarted" }), "{asked}");
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

    let (_, asked) = host.call("run_runner", json!({ "name": "restart" }));
    assert_eq!((asked["started"].clone(), asked["command"].clone()), (json!(false), json!("echo restarted")), "{asked}");
    let (ok, _) = host.call("run_runner", json!({ "name": "restart", "confirmed": "echo restarted" }));
    assert!(ok);

    // A yes answers the command it was asked about: edited since, the entry asks again.
    std::fs::write(
        root.join("proj/wheel.toml"),
        "[runner]\nrestart = { command = \"echo replaced\", confirm = \"Ends every agent.\" }\n",
    ).unwrap();
    let (_, again) = host.call("run_runner", json!({ "name": "restart", "confirmed": "echo restarted" }));
    assert_eq!((again["started"].clone(), again["command"].clone()), (json!(false), json!("echo replaced")), "{again}");
    let (_, bare) = host.call("run_runner", json!({ "name": "restart", "confirmed": "" }));
    assert_eq!(bare["started"], json!(false), "{bare}");
    let (_, started) = host.call("run_runner", json!({ "name": "restart", "confirmed": "echo replaced" }));
    assert_eq!(started["started"], json!(true), "{started}");
    assert!(!host.call("run_runner", json!({ "name": "nope" })).0, "an unknown runner is refused");
    let _ = std::fs::remove_dir_all(&root);
}

/// The page's text parts: every `text` of the page's components, joined.
fn page_text(result: &Value) -> String {
    let messages: Value = serde_json::from_str(result["content"][0]["resource"]["text"].as_str().unwrap_or("[]")).unwrap();
    messages[0]["updateComponents"]["components"].as_array().unwrap().iter()
        .flat_map(|c| {
            let mut v = vec![c["text"].as_str().unwrap_or("").to_string()];
            v.extend(c["modifiers"].as_array().into_iter().flatten().filter_map(|m| m["help"].as_str()).map(String::from));
            v
        })
        .collect::<Vec<_>>().join("\n")
}

fn page_call(host: &mut Host, tool: &str, args: Value) -> (Value, String) {
    let dir = host.dir.clone();
    let r = host.request("tools/call", json!({ "name": tool, "arguments": args }), json!({ "wheel/dir": dir, "wheel/pane": "p1" }));
    let text = page_text(&r["result"]);
    (r["result"].clone(), text)
}

#[test]
fn the_page_lists_commands_asks_before_a_confirm_command_and_follows_a_live_run() {
    let root = std::env::temp_dir().join(format!("wheel-runner-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("proj")).unwrap();
    std::fs::write(root.join("machine.toml"), "[runner]\ntidy = \"echo tidy\"\n").unwrap();
    std::fs::write(
        root.join("proj/wheel.toml"),
        "[runner]\nslow = \"echo first-line; sleep 2; echo second-line\"\nrestart = { command = \"echo restarted\", confirm = \"Ends every agent.\" }\n",
    )
    .unwrap();
    let mut host = Host::start(&root);
    let dir = host.dir.clone();
    let uri = format!("a2ui://runner-native?dir={}", dir.replace('/', "%2F"));

    // The page is a resource: a read answers with the empty surface, and the page tools draw it.
    let read = host.request("resources/read", json!({ "uri": uri }), json!({}));
    assert!(read["result"]["contents"][0]["text"].as_str().unwrap().contains("createSurface"), "{read}");
    assert!(host.request("resources/read", json!({ "uri": "a2ui://other" }), json!({}))["error"].is_object());
    let tools = host.request("tools/list", json!({}), json!({}));
    let ui_tool = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "get_runner").unwrap().clone();
    assert_eq!(ui_tool["_meta"]["ui"]["resourceUri"], "a2ui://runner-native");
    host.request("resources/subscribe", json!({ "uri": uri }), json!({}));

    let (_, page) = page_call(&mut host, "get_runner", json!({}));
    for want in ["Commands", "slow", "restart", "tidy", "Run slow", "Run tidy"] {
        assert!(page.contains(want), "{want} missing from:\n{page}");
    }

    // A confirm command shows its question and starts nothing.
    let (_, page) = page_call(&mut host, "press_run", json!({ "name": "restart" }));
    assert!(page.contains("Ends every agent.") && page.contains("Run restart: Ends every agent."), "{page}");
    assert_eq!(host.call("list_runs", json!({})).1, json!([]));
    let (_, page) = page_call(&mut host, "cancel_ask", json!({}));
    assert!(!page.contains("Ends every agent."), "{page}");

    // A command without one starts at once and its row offers Open; Open fills the pane with its log,
    // and the page is announced as the log grows.
    host.announced.clear();
    let (_, page) = page_call(&mut host, "press_run", json!({ "name": "slow" }));
    assert!(page.contains("running") && page.contains("Open the log of slow"), "{page}");
    let id = host.call("list_runs", json!({})).1[0]["id"].as_str().unwrap().to_string();
    let (_, page) = page_call(&mut host, "open_run", json!({ "path": id }));
    assert!(page.contains("Back to the commands") && page.contains("echo first-line"), "{page}");
    let end = Instant::now() + Duration::from_secs(15);
    let (mut saw_first, mut saw_second) = (false, false);
    while !saw_second {
        assert!(Instant::now() < end, "the log did not follow the run: first {saw_first}");
        if host.announced.is_empty() {
            let mut line = String::new();
            assert!(host.out.read_line(&mut line).unwrap() > 0, "the server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "notifications/resources/updated" {
                host.announced.push(v["params"]["uri"].as_str().unwrap().to_string());
            }
            continue;
        }
        assert_eq!(host.announced.remove(0), uri);
        let (_, page) = page_call(&mut host, "get_runner", json!({}));
        saw_first |= page.contains("first-line");
        saw_second = page.contains("second-line") && page.contains("exit 0");
    }
    assert!(saw_first, "the first line showed before the run ended");

    // The question's Run button starts it; a stop ends a running run.
    let (_, page) = page_call(&mut host, "press_run", json!({ "name": "restart", "confirmed": "echo restarted" }));
    assert!(!page.contains("Ends every agent."), "starting it closes the question: {page}");
    let (_, page) = page_call(&mut host, "close_log", json!({}));
    assert!(page.contains("Open the last log of slow") && !page.contains("Back to the commands"), "{page}");
    let runs = host.call("list_runs", json!({})).1;
    assert!(runs.as_array().unwrap().iter().any(|r| r["name"] == "restart"), "{runs}");
    let _ = std::fs::remove_dir_all(&root);
}
