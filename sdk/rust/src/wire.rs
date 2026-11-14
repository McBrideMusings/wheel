//! The backend protocol's methods and their parameter types (`docs/plugin-runtime.md`, "Plugin
//! backends"). `wheeld` uses these as the peer that starts a backend and `Backend` uses them as the
//! backend, so each method is defined once.

use async_lsp::lsp_types::notification::Notification;
use async_lsp::lsp_types::request::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `initialize` (request, wheeld to backend): the backend answers with its feeds and actions.
pub struct Initialize;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InitializeParams {
    #[serde(default)]
    pub wheel_version: String,
    #[serde(default)]
    pub plugin_id: String,
    #[serde(default)]
    pub config: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InitializeResult {
    #[serde(default)]
    pub feeds: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
}

impl Request for Initialize {
    type Params = InitializeParams;
    type Result = InitializeResult;
    const METHOD: &'static str = "initialize";
}

/// `wheel/call` (request, backend to wheeld): `{method, args}` runs a Wheel operation for the
/// backend's plugin if the plugin's `[backend] needs` names it.
pub struct WheelCall;

impl Request for WheelCall {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "wheel/call";
}

/// A feed and its params, the body of `feed/subscribe` and `feed/unsubscribe`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FeedRef {
    pub feed: String,
    #[serde(default)]
    pub params: Value,
}

/// `feed/subscribe` (notification, wheeld to backend): start sending `feed/update` for the feed.
pub struct FeedSubscribe;

impl Notification for FeedSubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/subscribe";
}

/// `feed/unsubscribe` (notification, wheeld to backend): stop sending the feed.
pub struct FeedUnsubscribe;

impl Notification for FeedUnsubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/unsubscribe";
}

/// `action/invoke` (request, wheeld to backend): run an action for a pane. The result is any JSON
/// value.
pub struct ActionInvoke;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ActionInvokeParams {
    pub action: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub caller: Value,
}

impl Request for ActionInvoke {
    type Params = ActionInvokeParams;
    type Result = Value;
    const METHOD: &'static str = "action/invoke";
}

/// `shutdown` (notification, wheeld to backend): exit now.
pub struct Shutdown;

impl Notification for Shutdown {
    type Params = ();
    const METHOD: &'static str = "shutdown";
}

/// `feed/update` (notification, backend to wheeld): `{feed, params, value}` or `{feed, params,
/// patch}`. The params are raw so the receiver can log a malformed update instead of stopping.
pub struct FeedUpdate;

impl Notification for FeedUpdate {
    type Params = Value;
    const METHOD: &'static str = "feed/update";
}

/// `log` (notification, backend to wheeld): `{level, message}`, written to the backend's log.
pub struct LogMessage;

impl Notification for LogMessage {
    type Params = Value;
    const METHOD: &'static str = "log";
}
