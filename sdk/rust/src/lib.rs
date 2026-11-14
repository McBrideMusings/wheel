//! Wheel's Rust SDK for plugin backends.
//!
//! A backend declares its feeds and actions, publishes each feed's value (the whole value, or an
//! RFC 6902 JSON Patch against the last one) and answers actions:
//!
//! ```no_run
//! use wheel_plugin_sdk::{Backend, serde_json::json};
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut backend = Backend::new();
//!     backend.feed("now", |sub| async move {
//!         loop {
//!             sub.value(json!({ "time": "12:00:00" }));
//!             tokio::time::sleep(std::time::Duration::from_secs(1)).await;
//!         }
//!     });
//!     backend.action("echo", |call| async move { Ok(call.args.clone()) });
//!     backend.run().await;
//! }
//! ```
//!
//! `run` talks to `wheeld` over the unix socket named by `WHEEL_PLUGIN_SOCKET` when it is set,
//! else over standard input and output, with Content-Length framing.

pub mod wire;

use std::collections::HashMap;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_lsp::router::Router;
use async_lsp::{ClientSocket, ErrorCode, MainLoop, ResponseError};
use serde_json::{json, Value};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub use async_lsp::ErrorCode as Code;
pub use serde_json;

use wire::*;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type FeedFn = Arc<dyn Fn(Subscription) -> BoxFuture<()> + Send + Sync>;
type ActionFn = Arc<dyn Fn(Call) -> BoxFuture<Result<Value, ActionError>> + Send + Sync>;

/// An action's failure, sent to `wheeld` as a JSON-RPC error.
#[derive(Debug, Clone)]
pub struct ActionError {
    pub code: i32,
    pub message: String,
}

impl ActionError {
    pub fn new(message: impl Into<String>) -> Self {
        ActionError { code: -32000, message: message.into() }
    }
}

impl<S: Into<String>> From<S> for ActionError {
    fn from(message: S) -> Self {
        ActionError::new(message)
    }
}

/// The error `wheeld` answered a `wheel/call` with.
#[derive(Debug, Clone)]
pub struct CallError {
    pub code: i64,
    pub message: String,
}

/// The connection to `wheeld`: logs and `wheel/call`.
#[derive(Clone)]
pub struct Peer {
    client: ClientSocket,
}

impl Peer {
    /// Write a line to the backend's log; `level` is `debug`, `info`, `warn` or `error`.
    pub fn log(&self, level: &str, message: impl AsRef<str>) {
        let _ = self.client.notify::<LogMessage>(json!({ "level": level, "message": message.as_ref() }));
    }

    /// Run a Wheel operation (`pane.open`, `notify`, `command.run`) if the plugin's `[backend]
    /// needs` names it.
    pub async fn call(&self, method: &str, args: Value) -> Result<Value, CallError> {
        match self.client.request::<WheelCall>(json!({ "method": method, "args": args })).await {
            Ok(value) => Ok(value),
            Err(async_lsp::Error::Response(e)) => Err(CallError { code: e.code_as_i64(), message: e.message }),
            Err(e) => Err(CallError { code: -32603, message: e.to_string() }),
        }
    }
}

trait CodeI64 {
    fn code_as_i64(&self) -> i64;
}

impl CodeI64 for ResponseError {
    fn code_as_i64(&self) -> i64 {
        serde_json::to_value(self.code).ok().and_then(|v| v.as_i64()).unwrap_or(-32603)
    }
}

/// One subscribed (feed, params). Its handler runs as a task until the last pane leaves, when
/// the task is dropped at its next await.
#[derive(Clone)]
pub struct Subscription {
    pub feed: String,
    pub params: Value,
    pub peer: Peer,
}

impl Subscription {
    /// Replace the feed's value.
    pub fn value(&self, value: Value) {
        let _ = self.peer.client.notify::<FeedUpdate>(json!({ "feed": self.feed, "params": self.params, "value": value }));
    }

    /// Change the feed's last value by an RFC 6902 JSON Patch (an array of operations).
    pub fn patch(&self, patch: Value) {
        let _ = self.peer.client.notify::<FeedUpdate>(json!({ "feed": self.feed, "params": self.params, "patch": patch }));
    }
}

/// One `action/invoke`.
#[derive(Clone)]
pub struct Call {
    pub action: String,
    pub args: Value,
    /// `{pane, space, plugin, plugin_pane}`: the pane that invoked it.
    pub caller: Value,
    pub peer: Peer,
}

/// The feeds and actions of one backend.
#[derive(Default)]
pub struct Backend {
    feeds: Vec<(String, FeedFn)>,
    actions: Vec<(String, ActionFn)>,
}

impl Backend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a feed. `handler` runs once per subscribed (feed, params), publishing through the
    /// `Subscription`; it is dropped when the last subscriber leaves.
    pub fn feed<F, Fut>(&mut self, name: &str, handler: F) -> &mut Self
    where
        F: Fn(Subscription) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.feeds.push((name.to_string(), Arc::new(move |s| Box::pin(handler(s)))));
        self
    }

    /// Declare an action.
    pub fn action<F, Fut>(&mut self, name: &str, handler: F) -> &mut Self
    where
        F: Fn(Call) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, ActionError>> + Send + 'static,
    {
        self.actions.push((name.to_string(), Arc::new(move |c| Box::pin(handler(c)))));
        self
    }

    /// Serve `wheeld` until it sends `shutdown` or the connection ends. Returns the process's exit status.
    pub async fn run(self) -> i32 {
        match std::env::var("WHEEL_PLUGIN_SOCKET") {
            Ok(path) if !path.is_empty() => match tokio::net::UnixStream::connect(&path).await {
                Ok(stream) => {
                    let (r, w) = stream.into_split();
                    self.serve(r, w).await
                }
                Err(e) => {
                    eprintln!("backend: cannot connect to {path}: {e}");
                    1
                }
            },
            _ => self.serve(tokio::io::stdin(), tokio::io::stdout()).await,
        }
    }

    /// Serve over any reader and writer.
    pub async fn serve<R, W>(self, read: R, write: W) -> i32
    where
        R: tokio::io::AsyncRead + Send + 'static,
        W: tokio::io::AsyncWrite + Send + 'static,
    {
        let feeds: HashMap<String, FeedFn> = self.feeds.iter().cloned().collect();
        let actions: HashMap<String, ActionFn> = self.actions.iter().cloned().collect();
        let names = InitializeResult {
            feeds: self.feeds.iter().map(|(n, _)| n.clone()).collect(),
            actions: self.actions.iter().map(|(n, _)| n.clone()).collect(),
        };
        let (main_loop, _) = MainLoop::new_server(|client| {
            let state = State { peer: Peer { client }, feeds, actions, running: Mutex::default() };
            let mut router = Router::new(Arc::new(state));
            router
                .request::<Initialize, _>(move |s, p| {
                    s.peer.log("info", format!("initialized for {}", p.plugin_id));
                    let names = names.clone();
                    async move { Ok(names) }
                })
                .notification::<FeedSubscribe>(|s, r| {
                    s.subscribe(r);
                    ControlFlow::Continue(())
                })
                .notification::<FeedUnsubscribe>(|s, r| {
                    s.unsubscribe(r);
                    ControlFlow::Continue(())
                })
                .notification::<Shutdown>(|_, _| ControlFlow::Break(Ok(())))
                .request::<ActionInvoke, _>(|s, p| {
                    let handler = s.actions.get(&p.action).cloned();
                    let peer = s.peer.clone();
                    async move {
                        let Some(handler) = handler else {
                            return Err(ResponseError::new(ErrorCode::METHOD_NOT_FOUND, format!("no action {:?}", p.action)));
                        };
                        let call = Call { action: p.action, args: p.args, caller: p.caller, peer };
                        handler(call).await.map_err(|e| ResponseError::new(ErrorCode::from(e.code), e.message))
                    }
                })
                .unhandled_notification(|_, _| ControlFlow::Continue(()));
            router
        });
        let result = main_loop.run_buffered(read.compat(), write.compat_write()).await;
        if let Err(e) = &result {
            eprintln!("backend: {e}");
        }
        if result.is_ok() { 0 } else { 1 }
    }
}

struct State {
    peer: Peer,
    feeds: HashMap<String, FeedFn>,
    actions: HashMap<String, ActionFn>,
    /// The handler task of each subscribed (feed, params as a string).
    running: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

fn key(feed: &str, params: &Value) -> String {
    format!("{feed}{params}")
}

impl State {
    fn subscribe(&self, r: FeedRef) {
        let Some(handler) = self.feeds.get(&r.feed).cloned() else {
            self.peer.log("warn", format!("subscribe to unknown feed {:?}", r.feed));
            return;
        };
        let k = key(&r.feed, &r.params);
        let task = tokio::spawn(handler(Subscription { feed: r.feed, params: r.params, peer: self.peer.clone() }));
        if let Some(old) = self.running.lock().unwrap().insert(k, task) {
            old.abort();
        }
    }

    fn unsubscribe(&self, r: FeedRef) {
        if let Some(task) = self.running.lock().unwrap().remove(&key(&r.feed, &r.params)) {
            task.abort();
        }
    }
}
