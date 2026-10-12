//! The wire protocol between `wheeld` and its clients: one JSON object per WebSocket text frame.
//!
//! A client connects to `ws://<host>/ws?token=<token>`; the token is the contents of the daemon's
//! token file, readable only by the user. Requests from a browser (anything carrying an `Origin`
//! header) are refused.
//!
//! A client sends [`ClientMsg`]; the daemon answers with [`ServerMsg`]. A request may carry a
//! `req` id, which the daemon copies onto the reply so a client can match them.
//! Terminal bytes travel base64-encoded.

use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7420";

/// Where the daemon keeps its client token unless told otherwise: under the user's
/// Application Support. Fails when `HOME` is unset rather than guessing a relative path.
pub fn default_token_path() -> Result<std::path::PathBuf, std::env::VarError> {
    Ok(std::path::PathBuf::from(std::env::var("HOME")?)
        .join("Library/Application Support/wheel/token"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Ask for the daemon's state.
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Start a shell. The daemon replies with `term_opened`; it does not attach the caller.
    TermOpen {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Stream a terminal's output to this connection. Attaching again replaces the earlier stream.
    TermAttach {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
    },
    /// Stop streaming a terminal to this connection. The terminal keeps running.
    TermDetach { id: String },
    /// End a terminal opened with `term_open`: hang up its shell and forget it. A pane's terminal
    /// ends with its pane, so closing one here is refused.
    TermClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
    },
    /// Type into a terminal. With a `req`, the daemon replies `ok` once the input is queued.
    TermInput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        data: String,
    },
    /// Stream the lead's conversation to this connection: `lead_attached` carries the transcript
    /// so far, then `lead` events follow live.
    LeadAttach {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Give the lead a message. Replies `lead_queued` with the message's turn id; the turn
    /// streams as `lead` events carrying that id.
    LeadSend {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        text: String,
    },
    /// Stop the lead's current turn.
    LeadCancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Answer a waiting permission request. `choice` is one of the request's option ids, or
    /// `allow` / `deny` for its first allow-once / reject-once option. The first answer wins.
    PermAnswer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        choice: String,
    },
    /// With a `req`, the daemon replies `ok` once the terminal has the new size.
    TermResize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        cols: u16,
        rows: u16,
    },
    /// Make a space for a project, holding one empty pane, and make it the active space. `cwd`
    /// must be an absolute path to a directory; `name` defaults to its last component.
    /// Replies `created` with the space id.
    SpaceCreate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Close a space and every pane in it; their terminals are killed.
    SpaceClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
    },
    /// Make a space active, or focus a pane (which also makes its space active). Give one.
    Focus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
    },
    /// Split a pane, putting a new empty pane on the `dir` side of it. Replies `created` with the
    /// new pane's id.
    PaneSplit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        dir: Dir,
    },
    /// Close a pane, killing its terminal. Closing the last pane closes its space.
    PaneClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
    },
    /// Change what a pane shows. The old content goes (a terminal is killed); `terminal` starts a
    /// shell in the space's directory; `web` shows the folder `dir` (an absolute path to a
    /// directory, required for `web` and refused for the others).
    PaneSetKind {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        kind: NewPaneKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir: Option<String>,
    },
    /// Set how much of a split its first child takes, from 0.1 to 0.9.
    SplitRatio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        split: String,
        ratio: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ServerMsg {
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        version: String,
        uptime_secs: u64,
        log_path: String,
        terms: Vec<TermInfo>,
        /// None when the daemon runs without a lead.
        lead: Option<LeadState>,
        perms: Vec<PermInfo>,
        spaces: Vec<SpaceInfo>,
        /// The active space's id; None when there are no spaces.
        active: Option<String>,
    },
    TermOpened {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        pid: Option<u32>,
    },
    /// Reset the terminal view, then apply the `replay_bytes` of output that follow as
    /// `term_output`; live output comes after them. The daemon sends this on every attach and
    /// again whenever this connection fell behind and output was dropped. The replay is the
    /// terminal's most recent raw output, up to 1 MiB, not a rendered screen.
    TermAttached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        replay_bytes: usize,
    },
    TermOutput {
        id: String,
        data: String,
    },
    /// The shell exited. Also sent right after the replay when attaching to a terminal that had
    /// already exited.
    TermExited {
        id: String,
        code: Option<u32>,
    },
    /// The message is queued as `turn`; its `user`, `turn_start`, `turn_end` and `error` events
    /// carry this id.
    LeadQueued {
        req: u64,
        turn: u64,
    },
    /// Replace the whole conversation view with `events`. Sent on attach, and again when this
    /// connection fell behind and missed events.
    LeadAttached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        state: LeadState,
        events: Vec<LeadEvent>,
    },
    /// One step of the lead's conversation. `text` events are chunks: append each to the last.
    Lead {
        event: LeadEvent,
    },
    /// Sent to every connection whenever the lead's state changes. An unsolicited `status` (no
    /// `req`) means this connection missed daemon-wide events: replace the lead state and the
    /// permission list with the ones it carries.
    LeadStateChanged {
        state: LeadState,
    },
    /// Sent to every connection: a session is waiting for a person to allow or deny something.
    PermRequested {
        perm: PermInfo,
    },
    /// Sent to every connection once a request is answered, so the others drop it.
    PermResolved {
        id: String,
        choice: String,
    },
    /// Sent to every connection after any change to spaces, panes or focus: replace the whole
    /// layout with this one. An unsolicited `status` carries the same fields.
    Spaces {
        spaces: Vec<SpaceInfo>,
        active: Option<String>,
    },
    /// Sent to every connection: a file under web pane `pane`'s folder changed, so a client showing
    /// it reloads. Changes within 200ms of each other send one.
    PaneReload {
        pane: String,
    },
    /// A request that made something (a space, a pane) succeeded; `id` names it.
    Created {
        req: u64,
        id: String,
    },
    /// A request that has no other reply succeeded.
    Ok {
        req: u64,
    },
    /// A request failed. `term` names the terminal when the request was a terminal op
    /// (`term_attach`, `term_input`, `term_resize`, `term_close`), so a failure with no `req`
    /// still ties to its terminal.
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        term: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LeadState {
    Starting,
    Ready,
    /// Working on a turn; `queued` more messages wait behind it.
    Busy {
        queued: usize,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeadEvent {
    /// A message the person sent, recorded the moment the daemon queues it. It may wait behind a
    /// turn still running; `turn_start` with the same `turn` says when it is taken up.
    User {
        turn: u64,
        text: String,
    },
    /// The lead started on the message queued as `turn`. Every `text`, `tool_call`,
    /// `tool_update` and `plan` after this one up to its `turn_end` or `error` belongs to that
    /// turn; `user` events for later messages and an `error` with no `turn` can arrive between.
    TurnStart {
        turn: u64,
    },
    /// A chunk of the lead's reply.
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        title: String,
        tool_kind: String,
        status: String,
    },
    ToolUpdate {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    Plan {
        entries: Vec<PlanItem>,
    },
    TurnEnd {
        turn: u64,
        stop_reason: String,
    },
    /// A turn failed (`turn` set), or the agent process stopped (`turn` unset; the daemon restarts
    /// it, and the new process does not remember earlier turns).
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanItem {
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermInfo {
    pub id: String,
    /// The session asking: `lead`, or a worker's name.
    pub session: String,
    /// What the session wants to do, as the agent described the tool call.
    pub title: String,
    pub options: Vec<PermOption>,
    /// The ACP tool call this request gates. Requests from the daemon's own MCP tools have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermOption {
    pub id: String,
    pub name: String,
    /// allow_once, allow_always, reject_once or reject_always.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TermInfo {
    pub id: String,
    pub pid: Option<u32>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
    pub buffered_bytes: usize,
}

/// One project's workspace: a directory and a split tree of panes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub root: Node,
    /// The pane that has focus in this space.
    pub focus: String,
}

/// A split tree, after Ghostty's: every leaf is a pane, every inner node splits its area in two.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum Node {
    Pane {
        id: String,
        #[serde(flatten)]
        kind: PaneKind,
    },
    Split {
        id: String,
        axis: Axis,
        /// The share of the split `a` takes.
        ratio: f64,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// `horizontal` puts `a` left of `b`; `vertical` puts `a` above `b`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Which side of a pane a split puts the new pane on.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// What a pane shows. Clients render a kind they do not know as an unsupported pane.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneKind {
    /// Not chosen yet: the pane asks what it shows.
    Empty {
        extra: Extra,
    },
    Terminal {
        term: String,
        extra: Extra,
    },
    /// The HTML in folder `dir`, served at `/artifacts/<pane>/` and reloaded when a file in it
    /// changes.
    Web {
        dir: String,
        extra: Extra,
    },
    /// A kind this build does not know (saved by a newer one), kept whole so saving the
    /// layout writes it back unchanged.
    Unknown(UnknownPane),
}

impl PaneKind {
    /// A new empty pane.
    pub fn empty() -> Self {
        PaneKind::Empty {
            extra: Extra::default(),
        }
    }
}

/// The fields of a known kind this build does not read (written by a newer one), kept so saving
/// the layout writes them back. Replacing the pane's kind drops them with the old content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra(serde_json::Map<String, serde_json::Value>);

/// A pane object whose `kind` names no kind this build knows. Only deserializing makes one,
/// so it never holds a known kind the daemon would then mistake for unknown.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownPane(serde_json::Map<String, serde_json::Value>);

impl UnknownPane {
    /// The kind it was saved as.
    pub fn kind(&self) -> &str {
        self.0["kind"].as_str().unwrap_or_default()
    }
}

/// The kinds this build knows, as they appear on the wire.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum KnownPaneKind {
    Empty,
    Terminal { term: String },
    Web { dir: String },
}

impl Serialize for PaneKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (known, Extra(extra)) = match self {
            PaneKind::Empty { extra } => (KnownPaneKind::Empty, extra),
            PaneKind::Terminal { term, extra } => {
                (KnownPaneKind::Terminal { term: term.clone() }, extra)
            }
            PaneKind::Web { dir, extra } => (KnownPaneKind::Web { dir: dir.clone() }, extra),
            PaneKind::Unknown(UnknownPane(raw)) => return raw.serialize(serializer),
        };
        let mut fields = known_fields(&known).map_err(serde::ser::Error::custom)?;
        fields.extend(extra.clone());
        fields.serialize(serializer)
    }
}

/// A known kind's own wire fields, `kind` included.
fn known_fields(
    known: &KnownPaneKind,
) -> Result<serde_json::Map<String, serde_json::Value>, serde_json::Error> {
    match serde_json::to_value(known)? {
        serde_json::Value::Object(fields) => Ok(fields),
        other => Err(serde::ser::Error::custom(format!(
            "pane kind serialized as {other}, not an object"
        ))),
    }
}

impl<'de> Deserialize<'de> for PaneKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        match raw.get("kind").and_then(|k| k.as_str()) {
            Some("empty" | "terminal" | "web") => {}
            Some(_) => return Ok(PaneKind::Unknown(UnknownPane(raw))),
            None => return Err(serde::de::Error::custom("pane has no string `kind`")),
        }
        let known: KnownPaneKind = serde_json::from_value(serde_json::Value::Object(raw.clone()))
            .map_err(serde::de::Error::custom)?;
        let fields = known_fields(&known).map_err(serde::de::Error::custom)?;
        let extra = Extra(
            raw.into_iter()
                .filter(|(key, _)| !fields.contains_key(key))
                .collect(),
        );
        Ok(match known {
            KnownPaneKind::Empty => PaneKind::Empty { extra },
            KnownPaneKind::Terminal { term } => PaneKind::Terminal { term, extra },
            KnownPaneKind::Web { dir } => PaneKind::Web { dir, extra },
        })
    }
}

/// What `pane_set_kind` turns a pane into.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewPaneKind {
    Empty,
    Terminal,
    /// A web view of a folder; needs `dir`.
    Web,
}

/// Parse a unit enum from its wire name, e.g. `"right"` as [`Dir::Right`].
pub fn from_wire<T: serde::de::DeserializeOwned>(name: &str) -> Result<T, String> {
    serde_json::from_value(serde_json::Value::String(name.to_string()))
        .map_err(|_| format!("{name:?} is not one of the accepted values"))
}

pub fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_bytes(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_use_snake_case_op_tags() {
        let msg = ClientMsg::TermOpen {
            req: Some(3),
            cwd: None,
            cols: 80,
            rows: 24,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"op":"term_open","req":3,"cols":80,"rows":24}"#);
        assert_eq!(serde_json::from_str::<ClientMsg>(&json).unwrap(), msg);
    }

    #[test]
    fn a_pane_kind_this_build_lacks_round_trips_unchanged() {
        let saved = serde_json::json!({"node": "pane", "id": "p9", "kind": "hologram", "depth": 3});
        let node: Node = serde_json::from_value(saved.clone()).unwrap();
        assert!(matches!(
            &node,
            Node::Pane { kind: PaneKind::Unknown(pane), .. } if pane.kind() == "hologram"
        ));
        assert_eq!(serde_json::to_value(&node).unwrap(), saved);

        let terminal =
            serde_json::json!({"node": "pane", "id": "p2", "kind": "terminal", "term": "t3"});
        let node: Node = serde_json::from_value(terminal.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Terminal { term, .. }, .. } if term == "t3")
        );
        assert_eq!(serde_json::to_value(&node).unwrap(), terminal);

        // A known kind keeps the fields a newer build added to it.
        let newer = serde_json::json!(
            {"node": "pane", "id": "p2", "kind": "terminal", "term": "t3", "extra": 1}
        );
        let node: Node = serde_json::from_value(newer.clone()).unwrap();
        assert_eq!(serde_json::to_value(&node).unwrap(), newer);
        let web = serde_json::json!({"node": "pane", "id": "p5", "kind": "web", "dir": "/tmp/w"});
        let node: Node = serde_json::from_value(web.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Web { dir, .. }, .. } if dir == "/tmp/w")
        );
        assert_eq!(serde_json::to_value(&node).unwrap(), web);
        let empty = serde_json::json!({"node": "pane", "id": "p4", "kind": "empty", "hint": "x"});
        let node: Node = serde_json::from_value(empty.clone()).unwrap();
        assert_eq!(serde_json::to_value(&node).unwrap(), empty);
        // A pane with no kind, or a known kind that is malformed, is still an error.
        assert!(
            serde_json::from_value::<Node>(serde_json::json!({"node": "pane", "id": "p2"}))
                .is_err()
        );
        assert!(serde_json::from_value::<Node>(
            serde_json::json!({"node": "pane", "id": "p2", "kind": 7})
        )
        .is_err());
        assert!(serde_json::from_value::<Node>(
            serde_json::json!({"node": "pane", "id": "p2", "kind": "terminal"})
        )
        .is_err());
    }
}
