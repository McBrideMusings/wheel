//! The clock fixture backend (`tools/fixtures/plugins/clock`) on the SDK: feeds `now` and `lines`,
//! actions `echo`, `fail`, `hidden` and `call`. `tools/plugin-backend.sh` and `tools/plugin-call.sh`
//! run it as the clock plugin's backend when `CLOCK_BACKEND=rust`.

use std::time::Duration;

use wheel_plugin_sdk::serde_json::{json, Value};
use wheel_plugin_sdk::{ActionError, Backend};

/// Prints `unsubscribed <feed>` to stderr when its feed task is dropped, which the SDK does at `feed/unsubscribe` and when the connection closes.
struct Unsubscribed(&'static str);

impl Drop for Unsubscribed {
    fn drop(&mut self) {
        eprintln!("unsubscribed {}", self.0);
    }
}

#[tokio::main]
async fn main() {
    eprintln!("clock backend up");
    eprintln!(
        "env WHEEL_URL={} WHEEL_TOKEN_FILE={} wheel={}",
        std::env::var("WHEEL_URL").unwrap_or_else(|_| "None".into()),
        std::env::var("WHEEL_TOKEN_FILE").unwrap_or_else(|_| "None".into()),
        which_wheel().unwrap_or_else(|| "None".into()),
    );
    let mut backend = Backend::new();
    backend.feed("now", |sub| async move {
        let _guard = Unsubscribed("now");
        let mut tick = 0u64;
        loop {
            tick += 1;
            sub.value(json!({ "time": utc_clock(), "tick": tick }));
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    backend.feed("lines", |sub| async move {
        let _guard = Unsubscribed("lines");
        sub.value(json!({ "lines": [] }));
        let mut n = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            n += 1;
            sub.patch(json!([{ "op": "add", "path": "/lines/-", "value": format!("line {n}") }]));
        }
    });
    backend.action("echo", |call| async move { Ok(json!({ "echo": call.args, "caller": call.caller })) });
    backend.action("fail", |_| async { Err(ActionError::new("the clock refuses")) });
    backend.action("hidden", |_| async { Ok(Value::from("hidden ran")) });
    backend.action("call", |call| async move {
        let method = call.args.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = call.args.get("args").cloned().unwrap_or_else(|| json!({}));
        Ok(match call.peer.call(&method, args).await {
            Ok(result) => json!({ "result": result }),
            Err(e) => json!({ "error": { "code": e.code, "message": e.message } }),
        })
    });
    std::process::exit(backend.run().await);
}

fn utc_clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

fn which_wheel() -> Option<String> {
    std::env::var("PATH").ok()?.split(':').map(|d| format!("{d}/wheel")).find(|p| std::path::Path::new(p).is_file())
}
