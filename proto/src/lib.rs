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

/// The message of the `error` event (no `turn`) a session sends last, when it ends for good.
pub const SESSION_ENDED: &str = "session ended";

/// The message of the `error` event (no `turn`) a restored agent pane's session sends when its
/// agent cannot load the saved conversation. The session goes on, fresh.
pub const SESSION_NOT_RESUMED: &str = "session ended: this agent cannot resume";

/// The daemon instance a release build runs as. An instance's files live under
/// `~/Library/Application Support/<instance>` (token, saved spaces) and `~/Library/Logs/<instance>`;
/// the dev build runs as `wheel-dev`, so the two never share a token or a layout.
pub const DEFAULT_INSTANCE: &str = "wheel";

/// Where an instance keeps its client token and saved spaces. Fails when `HOME` is unset rather
/// than guessing a relative path.
pub fn support_dir(instance: &str) -> Result<std::path::PathBuf, std::env::VarError> {
    Ok(std::path::PathBuf::from(std::env::var("HOME")?)
        .join("Library/Application Support")
        .join(instance))
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
    /// Stream an agent session's conversation to this connection: `session_attached` carries the
    /// transcript so far, then `session` events follow live. `session` is `lead` for the lead, or
    /// the name an agent pane carries. Attaching again replaces the earlier stream.
    SessionAttach {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
    },
    /// Stop streaming a session to this connection. The session keeps running.
    SessionDetach { session: String },
    /// Give a session a message. Replies `session_queued` with the message's turn id; the turn
    /// streams as `session` events carrying that id.
    SessionSend {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        text: String,
    },
    /// Stop a session's current turn.
    SessionCancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
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
    /// Change what a pane shows. The old content goes (a terminal is killed, an agent session
    /// ended); `terminal` starts a shell in the space's directory; `agent` starts a worker session
    /// running the lead's agent command there; `web` shows the folder `dir` (an absolute path to a
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
    /// Change one of a session's config options — its model, permission mode or effort level.
    /// `config` is the option's id and `value` one of its choices' `value`s, or `true`/`false`
    /// for a boolean. The whole new list goes to every connection as a `session_config` first,
    /// then this request's `ok` — so the setter has the new values before its own reply, and a
    /// client that redraws on either message lands on the same thing.
    SessionConfigSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        config: String,
        value: String,
    },
}

/// One control an agent offers over its own session: which model it runs, how much it thinks,
/// whether it asks before editing. The agent names the whole set, so wheel draws whatever arrives
/// rather than knowing any option by name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    /// What to label the control.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What the option is for: `mode`, `model`, `thought_level`, `model_config`, or anything else
    /// an agent names. None when the agent gave no category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(flatten)]
    pub kind: ConfigKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigKind {
    /// One value out of `choices`; `current` is one of their `value`s. An agent may group its
    /// choices under headers; wheel flattens them into one list in the order they arrived.
    Select {
        current: String,
        choices: Vec<ConfigChoice>,
    },
    Boolean {
        current: bool,
    },
    /// A kind of control this wheel cannot draw. Listed so it is visible, never set.
    Unknown,
}

/// One value a `select` option can take.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
        /// The lead session's state; None when the daemon runs without a lead.
        lead: Option<SessionState>,
        perms: Vec<PermInfo>,
        spaces: Vec<SpaceInfo>,
        /// The active space's id; None when there are no spaces.
        active: Option<String>,
        /// One per web pane whose folder is watched, sorted by pane.
        #[serde(default)]
        watches: Vec<WatchInfo>,
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
    /// daemon's copy of the screen written out as output: its history, cells, modes and cursor.
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
    SessionQueued {
        req: u64,
        session: String,
        turn: u64,
    },
    /// Replace the whole conversation view of `session` with `events`. Sent on attach, and again
    /// when this connection fell behind and missed events.
    SessionAttached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        state: SessionState,
        events: Vec<SessionEvent>,
        /// The controls the agent offers over this session, empty while it is still starting and
        /// for an agent that offers none.
        #[serde(default)]
        config: Vec<ConfigOption>,
    },
    /// One step of a session's conversation. `text` events are chunks: append each to the last.
    /// A session that ends sends an `error` with no `turn`, then nothing more.
    Session {
        session: String,
        event: SessionEvent,
    },
    /// Sent to every connection whenever a session's state changes. An unsolicited `status` (no
    /// `req`) means this connection missed daemon-wide events: replace the lead state and the
    /// permission list with the ones it carries.
    SessionStateChanged {
        session: String,
        state: SessionState,
    },
    /// Sent to every connection whenever a session's config options change: when the agent takes a
    /// `session_config_set`, when it changes one itself, and once when a new agent process names
    /// them. Replaces the whole list for that session.
    SessionConfig {
        session: String,
        options: Vec<ConfigOption>,
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
pub enum SessionState {
    Starting,
    Ready,
    /// Working on a turn; `queued` more messages wait behind it.
    Busy {
        queued: usize,
    },
    /// The agent stopped; the daemon starts it again.
    Failed {
        message: String,
    },
    /// A worker's agent stopped too many times in a row before it was ready; the daemon no longer
    /// starts it, and the session takes no messages. `message` is the last stop's reason.
    GaveUp {
        tries: u32,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ToolContent>,
    },
    ToolUpdate {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        /// When present, replaces the call's whole content (an empty list clears it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ToolContent>>,
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

/// What a tool call produced or proposes, as the agent reported it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text {
        text: String,
    },
    /// A file change; `old_text` is unset for a new file.
    Diff {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        new_text: String,
    },
    /// A terminal the agent created, by its ACP terminal id.
    Terminal {
        id: String,
    },
    /// A content type this build does not know; a newer daemon may send one.
    #[serde(other)]
    Unknown,
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
}

/// One web pane's folder watch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchInfo {
    pub pane: String,
    /// The folder the pane shows.
    pub dir: String,
    /// The path the watch is on: the folder itself, or while `waiting` the nearest folder above it
    /// that exists. None while the watch could not be set; the daemon retries every second.
    pub watching: Option<String>,
    /// The folder does not exist; the watch moves onto it once it appears.
    pub waiting: bool,
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
    /// The HTML in folder `dir` (the space's directory or a folder inside it), served at
    /// `/artifacts/<pane>/` and reloaded when a file in it changes.
    Web {
        dir: String,
        extra: Extra,
    },
    /// A worker agent session, named `session`, that the pane owns: it ends when the pane closes
    /// or changes kind. `acp_session` is the agent's own id for the conversation, saved so a
    /// restarted daemon can load it.
    Agent {
        session: String,
        acp_session: Option<String>,
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
    Terminal {
        term: String,
    },
    Web {
        dir: String,
    },
    Agent {
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        acp_session: Option<String>,
    },
}

impl Serialize for PaneKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (known, Extra(extra)) = match self {
            PaneKind::Empty { extra } => (KnownPaneKind::Empty, extra),
            PaneKind::Terminal { term, extra } => {
                (KnownPaneKind::Terminal { term: term.clone() }, extra)
            }
            PaneKind::Web { dir, extra } => (KnownPaneKind::Web { dir: dir.clone() }, extra),
            PaneKind::Agent {
                session,
                acp_session,
                extra,
            } => (
                KnownPaneKind::Agent {
                    session: session.clone(),
                    acp_session: acp_session.clone(),
                },
                extra,
            ),
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
            Some("empty" | "terminal" | "web" | "agent") => {}
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
            KnownPaneKind::Agent {
                session,
                acp_session,
            } => PaneKind::Agent {
                session,
                acp_session,
                extra,
            },
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
    /// A worker agent session running the lead's agent command in the space's directory.
    Agent,
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
    fn tool_content_is_tagged_by_type() {
        let update = SessionEvent::ToolUpdate {
            id: "c1".into(),
            title: None,
            status: None,
            content: Some(vec![
                ToolContent::Diff {
                    path: "a.rs".into(),
                    old_text: None,
                    new_text: "x".into(),
                },
                ToolContent::Terminal { id: "t1".into() },
            ]),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"tool_update","id":"c1","content":[{"type":"diff","path":"a.rs","new_text":"x"},{"type":"terminal","id":"t1"}]}"#
        );
        assert_eq!(serde_json::from_str::<SessionEvent>(&json).unwrap(), update);
        // A call with no content leaves the field off.
        let call: SessionEvent = serde_json::from_str(
            r#"{"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"pending"}"#,
        )
        .unwrap();
        assert!(matches!(call, SessionEvent::ToolCall { content, .. } if content.is_empty()));
        let newer: ToolContent = serde_json::from_str(r#"{"type":"hologram","depth":3}"#).unwrap();
        assert_eq!(newer, ToolContent::Unknown);
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
        let agent =
            serde_json::json!({"node": "pane", "id": "p6", "kind": "agent", "session": "w7"});
        let node: Node = serde_json::from_value(agent.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Agent { session, .. }, .. } if session == "w7")
        );
        assert_eq!(serde_json::to_value(&node).unwrap(), agent);
        let saved = serde_json::json!({"node": "pane", "id": "p6", "kind": "agent", "session": "w7", "acp_session": "abc"});
        let node: Node = serde_json::from_value(saved.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Agent { acp_session: Some(id), extra, .. }, .. } if id == "abc" && *extra == Extra::default())
        );
        assert_eq!(serde_json::to_value(&node).unwrap(), saved);
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
