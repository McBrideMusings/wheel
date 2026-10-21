//! The wire protocol between `wheeld` and its clients: one JSON object per WebSocket text frame.
//!
//! A client connects to `ws://<host>/ws?token=<token>`; the token is the contents of the daemon's
//! token file, readable only by the user. Requests from a browser (anything carrying an `Origin`
//! header) are refused.
//!
//! A client sends [`ClientMsg`]; the daemon answers with [`ServerMsg`]. A request may carry a
//! `req` id, which the daemon copies onto the reply so a client can match them.
//! Terminal bytes travel base64-encoded.

mod finder;
pub use finder::TextFinder;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7420";

/// The message of the `error` event (no `turn`) a session sends last, when it ends for good.
pub const SESSION_ENDED: &str = "session ended";

/// The notice a restored session puts up when its agent did not reopen the saved conversation —
/// it may be deleted, or the reopen may simply have failed. The session goes on in a new conversation, and the
/// saved id is kept rather than replaced, so a load that failed once is tried again next time.
/// The wording does not assert the conversation is gone, because this cannot tell.
pub const SESSION_NOT_REOPENED: &str =
    "session ended: could not reopen that conversation, so this is a new one";

/// The daemon instance a release build runs as. An instance's files live under
/// `~/Library/Application Support/<instance>` (token, saved spaces, `machine` artifacts) and
/// `~/Library/Logs/<instance>`;
/// the dev build runs as `wheel-dev`, so the two never share a token or a layout.
pub const DEFAULT_INSTANCE: &str = "wheel";

/// Where an instance keeps its client token and saved spaces. Fails when `HOME` is unset rather
/// than guessing a relative path.
pub fn support_dir(instance: &str) -> Result<std::path::PathBuf, std::env::VarError> {
    Ok(std::path::PathBuf::from(std::env::var("HOME")?)
        .join("Library/Application Support")
        .join(instance))
}

/// Which registry an artifact belongs to: `project` is the space's own, `machine` is every project
/// on this machine. A project's artifact shadows a machine one of the same name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactScope {
    Project,
    Machine,
}

/// The order a name with no scope is looked for in, so a project can shadow a machine-wide
/// artifact with its own.
pub const SCOPE_ORDER: [ArtifactScope; 2] = [ArtifactScope::Project, ArtifactScope::Machine];

impl ArtifactScope {
    /// The scope's name on the wire and in a message to a person.
    pub fn name(self) -> &'static str {
        match self {
            ArtifactScope::Project => "project",
            ArtifactScope::Machine => "machine",
        }
    }
}

/// Whether `name` is one plain path component, usable as a single directory name under a root the
/// daemon owns: not empty, no separator, not `.` or `..`, and not starting with `.` (a hidden name
/// is never served) or `-` (unusable as a command-line value). `what` names the kind of name in the
/// refusal. An artifact's name and a web pane's own directory name both go through this, so a name
/// refused in one place is refused in the other.
pub fn plain_name(what: &str, name: &str) -> Result<(), String> {
    let mut parts = std::path::Path::new(name).components();
    let one_component = matches!(parts.next(), Some(std::path::Component::Normal(first)) if first.to_str() == Some(name))
        && parts.next().is_none();
    if !one_component || name.starts_with('.') || name.starts_with('-') {
        return Err(format!(
            "{name:?} is not {what}: one plain name, no slash, not . or .., and not starting with . or -"
        ));
    }
    Ok(())
}

/// One artifact as the daemon reads it: the `wheel.toml` entry that defines it, where its page
/// resolved to, and what is wrong with it. `problems` is empty for an artifact with nothing wrong;
/// everything else is reported whether or not there are problems, so a pane on a broken artifact
/// still says what it is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactInfo {
    pub scope: ArtifactScope,
    pub name: String,
    /// The `wheel.toml` holding the entry: the space's for `project`, the instance directory's for
    /// `machine`.
    pub entry: String,
    /// The folder the pane serves from: the real folder, links followed, holding the entry's real
    /// page. The entry's `path` as written when there is nothing there.
    pub dir: String,
    /// The page a pane opens, under `dir`: `index.html` for an entry naming a folder, the file's
    /// own name for one naming a file. None when the entry names nothing that is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// The entry's `title`, or the artifact's name when it declares none.
    pub title: String,
    /// The refresh command the entry declares. The daemon runs it for each pane showing the
    /// artifact — on open and on demand — once a person has approved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// How often, in seconds, the entry asks for a refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u64>,
    /// Whether the entry says `pane = true`: this artifact asks to be offered in a pane picker.
    /// False is a one-off — still openable by name, by a person or an agent, and listed by no
    /// picker. A picker is a filter over these records, so the two registries are the scopes: a
    /// `machine` artifact is every project's on this machine, a `project` one is the project's own.
    #[serde(default)]
    pub pane: bool,
    /// The agent pane whose session ends this artifact: the daemon deletes the artifact when that
    /// pane closes. None for an artifact nothing ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_with_pane: Option<String>,
    /// When the daemon deletes this artifact, in seconds since the Unix epoch. None for one that
    /// never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// What is wrong with this artifact, one line each: no `path`, a path with nothing there, a key
    /// nothing reads, an interval with no command or no positive number, a `pane` that is not a
    /// boolean, a folder with no `index.html`, a reserved `data/` directory.
    #[serde(default)]
    pub problems: Vec<String>,
}

/// One pane's refresh runner: the command it runs for the artifact it shows, and what the last
/// run did. One per pane showing an artifact whose entry declares a command; a pane showing one
/// that declares none has no runner and no row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefreshInfo {
    pub pane: String,
    pub scope: ArtifactScope,
    /// The artifact's name.
    pub name: String,
    /// The folder the command's trust is keyed by: the real folder holding the entry's page.
    pub dir: String,
    /// The space directory the command runs in.
    pub cwd: String,
    /// The pane's data directory, the one place the command is given to write.
    pub data_dir: String,
    /// The command as the entry declared it at the last run, or at open before the first.
    pub command: String,
    /// How often, in seconds, the entry asks for a refresh, and what the pane's timer is doing
    /// about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u64>,
    pub timer: TimerState,
    /// How many runs this runner has finished, a refusal included.
    pub runs: u64,
    /// The last run, or None before the first has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<RunInfo>,
}

/// What a pane's refresh timer would do now — read when this record is made, not remembered from
/// the last tick, so it never describes a tick the entry or the approvals have since outgrown.
///
/// A timer runs only a command the person approved for good: a tick never raises a permission
/// request, so a refresh nobody is watching cannot put a prompt in front of someone who did not ask
/// for one. A tick that cannot run skips instead of ending the timer, so an approval given later,
/// or an entry fixed later, is picked up at the next tick rather than needing the pane reopened.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimerState {
    /// This pane has no timer: the entry declared no `every` when the pane opened it. Adding one
    /// takes effect the next time the pane shows the artifact, as adding a `command` does.
    #[default]
    Off,
    /// The command runs every `every` seconds for as long as the pane shows the artifact.
    Ticking,
    /// Skipping every tick, because the command is not remembered as approved — the person answered
    /// "allow once" or "deny" rather than "always", withdrew the approval, or edited the command.
    /// An on-demand refresh is what asks again; answering "always" there resumes the timer.
    Untrusted,
    /// Skipping every tick, because the entry no longer asks for a run: it declares no `every`
    /// any more, or no `command`. Putting either back resumes the timer at the next tick.
    Idle,
    /// The timer is gone while its pane still shows the artifact, which only a fault inside the
    /// daemon does: nothing is on a timer for this pane until it shows the artifact again.
    Stopped,
}

/// What one refresh run did. A run that a person refused, or that could not start, has no exit
/// status and carries `error` instead, so a failure is never reported as a success with no output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunInfo {
    /// When the run started, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    pub duration_ms: u64,
    /// The command's exit status; None when it never ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// Why there is no exit status: the person refused the command, or the shell could not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The tail of what the command wrote to standard error, however it ended.
    #[serde(default)]
    pub stderr_tail: String,
}

impl RunInfo {
    /// Whether the command ran and exited 0. Anything else — a non-zero status, a refusal, a shell
    /// that would not start — is a failed run.
    pub fn ok(&self) -> bool {
        self.exit == Some(0)
    }
}

/// One remembered approval: a person answered "always" for this command in this artifact's folder,
/// run in this space's directory, so no later run of the same three asks again while the files the
/// command names are as they were. Changing the command in the entry asks afresh.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustInfo {
    /// The artifact's folder: the key, with `cwd` and `command`.
    pub dir: String,
    /// The space's directory the command runs in: a machine artifact opened in another project runs
    /// against other files, so it is approved there on its own. A row with none, written before
    /// approvals named it, matches no space and asks again.
    #[serde(default)]
    pub cwd: String,
    pub command: String,
    /// The scope and name the artifact was approved as, for a person reading the list.
    pub scope: ArtifactScope,
    pub name: String,
    /// When it was approved, in milliseconds since the Unix epoch.
    pub approved_ms: u64,
    /// Every readable file inside the artifact's folder or the space's directory that the command's
    /// text names, as it was when the person approved it. The approval holds only while the same
    /// files are there with the same contents, so an edit, a pull or a branch switch that changes,
    /// adds or removes one asks again. A row with none, like one with no `cwd`, still loads, so the
    /// file stays readable and writable.
    #[serde(default)]
    pub reads: Vec<FileRead>,
}

/// One file a refresh command names, and what was in it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRead {
    pub path: String,
    /// The SHA-256 of the file's contents, in hex.
    pub sha256: String,
}

/// Which artifact a web pane shows: a scope and a name, never a path. Only [`ArtifactRef::new`]
/// makes one, and it is the only place a name is checked, so a reference that exists cannot name
/// anything outside its scope's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    scope: ArtifactScope,
    name: String,
}

impl ArtifactRef {
    /// `name` must be one plain path component, by [`plain_name`]'s rule.
    pub fn new(scope: ArtifactScope, name: &str) -> Result<ArtifactRef, String> {
        plain_name("an artifact name", name)?;
        Ok(ArtifactRef {
            scope,
            name: name.to_string(),
        })
    }

    pub fn scope(&self) -> ArtifactScope {
        self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }
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
    /// transcript so far, then `session` events follow live. `session` is the name an agent pane
    /// carries, or the pane's id. Attaching again replaces the earlier stream.
    SessionAttach {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
    },
    /// Stop streaming a session to this connection. The session keeps running.
    SessionDetach { session: String },
    /// Take down the notice shown over a session (`session_notice`); every client sees it go.
    SessionNoticeDismiss {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
    },
    /// Read every Claude login's usage now, whatever the age of the last reading (a 429's wait
    /// still holds the call back), and reply `usage`.
    Usage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        /// Read only when the last attempt is older than the reading interval, instead of reading
        /// now. A client that refreshes on a timer sets this; a refresh button leaves it off.
        #[serde(default)]
        due_only: bool,
    },
    /// Ask how remote clients reach this daemon. Replies `remote`.
    RemoteInfo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Give a session a message. Replies `session_queued` with the message's turn id; the turn
    /// streams as `session` events carrying that id.
    SessionSend {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        text: String,
    },
    /// Wait until a session is ready, up to `timeout_secs`. Replies `session_waited`, whose
    /// `ready` is false when the time ran out first; an error when the session gave up.
    SessionWait {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        timeout_secs: f64,
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
    /// The directories spaces were made for, newest first. Replies `space_recent`.
    SpaceRecent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
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
    /// Swap the places of two panes in one space. Each keeps running; only the layout changes.
    PaneSwap {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        with: String,
    },
    /// Move a pane to the `dir` side of another pane in its space, splitting the target's area
    /// evenly; the split the pane left collapses. The pane keeps running.
    PaneMove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        target: String,
        dir: Dir,
    },
    /// Close a pane, killing its terminal. Closing a space's last pane leaves the space with one new empty pane; closing a space's only pane when it is already empty changes nothing.
    PaneClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
    },
    /// Change what a pane shows. The old content goes (a terminal is killed, an agent session
    /// ended); `terminal` starts a shell in the space's directory; `web` shows the artifact named by `artifact`
    /// (required for `web` and refused for the others) in `scope`, or, with no `scope`, in the
    /// first of `project` and `machine` that has an entry by that name.
    PaneSetKind {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        kind: NewPaneKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ArtifactScope>,
        /// For `browser` only: the page to open. Without one, the `browser.homepage` setting as it
        /// resolves for the pane's space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Show the Claude Code, Codex or pi conversation a pane holds the other way: `agent` turns a
    /// terminal running the agent into an agent pane on that conversation, `terminal` turns an
    /// agent pane into a terminal running `claude --resume`, `codex resume` or `pi --session` on its
    /// conversation. One process drives a conversation at a time: the other view's process is
    /// stopped before the new one opens it, and a switch while a turn is running is refused. With
    /// no `to`, nothing changes. Replies `pane_view` with the view the pane is in and the
    /// conversation it shows.
    PaneView {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<PaneView>,
    },
    /// Start a terminal or an agent in a pane, in its space's directory; whatever the pane showed
    /// goes. An agent runs the daemon's agent command and takes `message` as its first message;
    /// the reply waits until the agent is ready or has given up, and is an error when it could
    /// not start. Replies `started`.
    PaneStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        kind: StartKind,
        /// For `agent` only: its first message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// For `agent` only: the agent to run, by name; the `agent.default` setting without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// A browser pane is showing `url` now. The client sends this as the person navigates, so the
    /// saved layout follows them and a restored pane comes back on the page they were on; anything
    /// else sending it asks that pane to go there, since the client draws whatever the layout says.
    /// Refused for a pane that is not a browser, and for a URL [`page_allowed`] refuses.
    PaneBrowserUrl {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        url: String,
    },
    /// Send a browser pane to what a person typed into its address field, read by
    /// [`page_typed`]: `example.com`, `~/notes.html` and a full URL all name a page. The pane
    /// moves as for [`ClientMsg::PaneBrowserUrl`], and every client goes there. Replies `opened`
    /// with the page, even when the pane was already on it, so the client that asked goes there
    /// again as a browser does. Refused, with what is wrong with the text, when it names no page
    /// a pane may open.
    PaneBrowserOpen {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        text: String,
    },
    /// Write a PNG of what a client draws for `pane` to `path` on the daemon's machine. The daemon
    /// asks a connected client that offered rendering (`render_offer`) and showed the pane, waits
    /// for its image, writes the file whole, and replies `pane_shot` once it is complete. An error
    /// says why: no client connected that renders, or none shows that pane. `path` must be
    /// absolute.
    PaneShot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        path: String,
    },
    /// This connection can draw any pane it shows as a PNG, so the daemon may send it
    /// `render_pane`. Sent once after connecting; it lasts until the connection closes.
    RenderOffer {},
    /// The answer to `render_pane` `id`: the pane drawn as a PNG, base64.
    Rendered { id: u64, png: String },
    /// The answer to `render_pane` `id` when this client cannot draw the pane: it does not show it,
    /// or the drawing failed. `message` says which.
    RenderRefused { id: u64, message: String },
    /// Set how much of a split its first child takes, from 0.1 to 0.9.
    SplitRatio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        split: String,
        ratio: f64,
    },
    /// Make an artifact: a folder holding a starter `index.html`, and its entry in the scope's
    /// `wheel.toml`. `scope` defaults to `project`. Refused when the scope already has an entry of
    /// that name or the folder is already there. Replies `artifact`.
    ArtifactNew {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        /// Write `pane = true` into the entry: offer this artifact in a pane picker.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pane: bool,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ArtifactScope>,
        /// Whose directory the `project` root sits under; the active space with none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        /// The folder to make, as an absolute path. With none, the scope's default: the space's
        /// `.wheel/artifacts/<name>`, or the instance directory's `artifacts/<name>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Delete the artifact when this agent pane's session ends. Only an artifact in the
        /// scope's default folder can be deleted, so a `path` with this is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ends_with_pane: Option<String>,
        /// Delete the artifact this many seconds after it is made. Refused with a `path`, as
        /// `ends_with_pane` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_in: Option<u64>,
    },
    /// Every one of wheel's own settings, resolved for `space` — the active space with none.
    /// Replies `settings`.
    SettingsLs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// Write one setting. With `machine`, into the machine-wide `wheel.toml`; otherwise into the
    /// `wheel.toml` of `space`'s directory, the active space with none. Replies `settings`, and
    /// every connection is told as well. Refused when the key is not one wheel has, or the value
    /// is not one the key takes.
    SettingSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        key: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        machine: bool,
    },
    /// Remove one setting from one file — the machine-wide `wheel.toml` with `machine`, else the
    /// one of `space`'s directory — so the layer below is in force. Removing a key the file does
    /// not set changes nothing and is not an error. Replies and tells every connection as
    /// `setting_set` does.
    SettingUnset {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        machine: bool,
    },
    /// Every artifact of every scope, each read off disk. With `pane`, only the ones whose
    /// entry says `pane = true` — what a pane picker asks for. Replies `artifacts`.
    ArtifactLs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pane: bool,
    },
    /// Register a page that already exists, wherever it is, as an artifact: write its entry into
    /// the scope's `wheel.toml` and touch nothing in its folder. `path` is absolute and names a
    /// folder holding `index.html` or one HTML file; `name` defaults to the folder's name, or the
    /// file's name without its extension. `scope` defaults to `project`. Refused when the path holds
    /// no page, the scope already has an entry of that name, or `every` comes without `command`.
    /// Replies `artifact`.
    ArtifactAdd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ArtifactScope>,
        /// Whose `wheel.toml` holds a `project` entry; the active space with none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The refresh command; it runs only once a person approves it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        /// Seconds between timed refreshes; needs `command`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        every: Option<u64>,
        /// Offer this artifact in a pane picker.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pane: bool,
    },
    /// Delete one artifact's entry from its `wheel.toml`, leaving every file it names where it is:
    /// in `scope` when given, else the scope a bare name resolves to. Replies `artifact` with the
    /// entry as it was.
    ArtifactRemove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ArtifactScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// One artifact: in `scope` when given, else the first of `project` and `machine` whose
    /// `wheel.toml` has an entry of that name. Replies `artifact`.
    ArtifactShow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<ArtifactScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// Run the refresh command of the artifact web pane `pane` shows, now. Waits for the run —
    /// including for a person to answer the permission request the first run of a command raises —
    /// and replies `refresh` with the runner and that run. Refused when the pane shows no
    /// artifact, or its entry declares no command.
    ArtifactRefresh {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
    },
    /// Read the values the page of web pane `pane` has written to its state store (key to JSON
    /// value): the one `key` names, or all of them. With `wait_ms`, waits up to that long for
    /// `key` to be written before replying; a key already written replies at once. Replies
    /// `page_state`. Refused when `pane` is not a web pane.
    ArtifactState {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// Push one JSON value into the page of web pane `pane`, without reloading it: every open
    /// page receives it, and a page that loads later receives the latest one. Replies `ok`.
    ArtifactData {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        pane: String,
        value: serde_json::Value,
    },
    /// Every remembered approval. Replies `trusted`.
    ArtifactTrust {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Forget every approval remembered for artifact `name`, so the next run of its command asks
    /// again. Replies `trusted` with what is left. Refused when nothing is remembered for it.
    ArtifactTrustForget {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        name: String,
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
    /// List the conversations the session's agent has for the directory it runs in. Replies
    /// `conversations`, newest first, with the one the session is in flagged. The agent's store is
    /// shared with whatever else runs the same harness in that directory — Claude Code in a
    /// terminal, say — so the list holds more than this session's own.
    SessionConversationLs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
    },
    /// Put the session in a fresh conversation. Replies `conversation` with the agent's id for it;
    /// the transcript empties and every attached client is sent the new one as a
    /// `session_attached`. Refused while a turn is running or a message is queued.
    SessionConversationNew {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
    },
    /// Put the session in a past conversation, named by the `id` a `conversations` entry carries.
    /// The agent replays it and that history becomes the transcript, which every attached client
    /// is sent. A load that fails leaves the session in the conversation it was already in.
    SessionConversationResume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        id: String,
    },
}

/// One conversation an agent holds for a directory, as `session/list` reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationInfo {
    /// The agent's own id for it, which `session_conversation_resume` takes.
    pub id: String,
    /// The directory the agent had it in.
    pub cwd: String,
    /// What the agent calls it; None when it named none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When the agent last worked in it, as an ISO 8601 timestamp; None when it gave none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Whether this is the conversation the session is in.
    pub current: bool,
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

/// One of wheel's own settings, as the daemon resolved it: the value in force, where that value
/// came from, and what the key is for. A reader is told the origin because a surprising value is
/// otherwise only diagnosable by opening two files and knowing which wins.
///
/// These are wheel's settings, not an agent's session options — [`ConfigOption`] above is the
/// agent's, named by the agent, and the two never mix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingInfo {
    /// The key, dotted: `browser.homepage`.
    pub key: String,
    /// The value in force for whichever space this list was resolved for.
    pub value: String,
    pub origin: SettingOrigin,
    /// What the value falls back to when neither file sets it.
    pub default: String,
    /// One line a CLI or a settings window can show beside the key.
    pub help: String,
    /// What this machine's `wheel.toml` sets the key to, when it sets a value the key takes —
    /// shown even where a project's value wins over it, so a reader sees what clearing the
    /// project's would bring back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// What the `wheel.toml` of the space this list was resolved for sets the key to, likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// What is wrong with a layer this key could have come from: a file that cannot be read or
    /// parsed, or a value in one that the key does not take and which was therefore ignored. The
    /// value in force is the layer below it, and this is how a reader is told why — otherwise a
    /// setting a person wrote and wheel refused would be invisible outside the daemon's log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// Which of the three layers a setting's value came from. A project's `wheel.toml` wins over the
/// machine-wide one, key by key, and the built-in default is what neither file setting means.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SettingOrigin {
    #[default]
    Default,
    /// The `wheel.toml` beside the token and the saved layout.
    Machine,
    /// The `wheel.toml` at the root of the space this list was resolved for.
    Project,
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
        perms: Vec<PermInfo>,
        spaces: Vec<SpaceInfo>,
        /// The active space's id; None when there are no spaces.
        active: Option<String>,
        /// How many times this daemon has written the layout to its state file. A browser pane's
        /// page is saved a moment after it changes and coalesced with whatever else asks in that
        /// moment, so this is how a reader tells one write per burst of navigation from one write
        /// each — there is nothing else a program could compare.
        #[serde(default)]
        layout_saves: u64,
        /// One per web pane whose folder is watched, sorted by pane.
        #[serde(default)]
        watches: Vec<WatchInfo>,
        /// One per web pane whose artifact declares a refresh command, sorted by pane.
        #[serde(default)]
        refreshes: Vec<RefreshInfo>,
        /// Every coding agent's usage as the strip and the Usage window show it.
        #[serde(default)]
        usage_board: UsageBoard,
        /// Every agent the daemon knows, with whether its program was found, sorted by name.
        #[serde(default)]
        agents: Vec<AgentInfo>,
        /// Why the shell integration (the zsh and bash startup files, the `claude` and `codex`
        /// wrappers, the `wheel` link beside them) is not fully in place; None when it is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_integration: Option<String>,
        /// The PATH wheeld runs with, which agent lookups, terminals and agents use, and where it
        /// came from.
        #[serde(default)]
        login_path: LoginPath,
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
    /// Every coding agent's usage as the strip and the Usage window show it, in reply to `usage`.
    Usage {
        req: u64,
        board: UsageBoard,
    },
    /// How remote clients reach this daemon, in reply to `remote_info`. `listen` is the
    /// `remote.listen` setting the daemon started with, None when remote access is off (the other
    /// fields are then empty, and so are they when `problem` says the listener did not start). `addresses` are the machine's interface addresses and hostname, any
    /// of which a client may dial on `port`; `fingerprint` is the SHA-256 of the daemon's
    /// certificate in DER, lowercase hex, which a client pins instead of trusting an authority.
    Remote {
        req: u64,
        listen: Option<String>,
        addresses: Vec<String>,
        port: u16,
        fingerprint: String,
        /// Why the listener is not running although `listen` is set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem: Option<String>,
    },
    /// The usage board changed: a reading was taken, or the agents or pins settings were written.
    /// Sent to every connection.
    UsageChanged {
        board: UsageBoard,
    },
    /// The message is queued as `turn`; its `user`, `turn_start`, `turn_end` and `error` events
    /// carry this id.
    SessionQueued {
        req: u64,
        session: String,
        turn: u64,
    },
    /// The answer to `session_wait`: the session and its pane, its state, and
    /// whether it was ready.
    SessionWaited {
        req: u64,
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<String>,
        state: SessionState,
        ready: bool,
    },
    /// Replace the whole conversation view of `session` with `events`. Sent on attach, and again
    /// when this connection fell behind and missed events.
    SessionAttached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        session: String,
        state: SessionState,
        events: Vec<StampedEvent>,
        /// The controls the agent offers over this session, empty while it is still starting and
        /// for an agent that offers none.
        #[serde(default)]
        config: Vec<ConfigOption>,
        /// How full the conversation's context window is, as the agent last reported it; unset
        /// while the agent has reported none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
        /// The daemon's clock when it sent this, in milliseconds since the Unix epoch. Event
        /// stamps are on that clock; a client compares it with its own to time a running line.
        now: u64,
    },
    /// The conversations a session's agent holds for its directory, newest first.
    Conversations {
        req: u64,
        session: String,
        conversations: Vec<ConversationInfo>,
    },
    /// The conversation a session is in now, after a `session_conversation_new` or
    /// `session_conversation_resume`: the agent's own id for it.
    Conversation {
        req: u64,
        session: String,
        id: String,
    },
    /// One step of a session's conversation. `text` events are chunks: append each to the last.
    /// A session that ends sends an `error` with no `turn`, then nothing more.
    Session {
        session: String,
        event: StampedEvent,
    },
    /// Sent to every connection whenever a session's state changes. An unsolicited `status` (no
    /// `req`) means this connection missed daemon-wide events: replace the permission list and the
    /// layout with the ones it carries.
    SessionStateChanged {
        session: String,
        state: SessionState,
    },
    /// A browser pane is showing a new page: the one field that changed, rather than every space.
    ///
    /// A person browsing changes this on every page load, every redirect and every in-page
    /// navigation their client reports, so this is the one layout change frequent enough that
    /// resending the whole layout for it would be most of what the daemon says. A client applies
    /// it to the pane it already holds; a client that has no such pane ignores it and will see the
    /// truth in the next `spaces` or `status` it is sent.
    PaneUrl {
        pane: String,
        url: String,
    },
    /// Sent to every connection whenever a session's config options change: when the agent takes a
    /// `session_config_set`, when it changes one itself, and once when a new agent process names
    /// them. Replaces the whole list for that session.
    SessionConfig {
        session: String,
        options: Vec<ConfigOption>,
    },
    /// Sent to every connection whenever a session's context use changes: after a turn, after a
    /// compaction, and unset when the session moves to another conversation. A connection that
    /// attaches is told the current figure in `session_attached`.
    SessionContext {
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
    },
    /// Sent to every connection when a session's notice goes up (`notice` set) or comes down
    /// (unset), and to a connection right after it attaches while one is up. A notice is a
    /// status for the person — a conversation that would not reopen — held in the daemon's
    /// memory only: never part of the transcript, never saved, gone after a daemon restart, a
    /// conversation change or `session_notice_dismiss`.
    SessionNotice {
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
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
    /// One artifact, in reply to `artifact_new`, `artifact_add`, `artifact_remove` or `artifact_show`.
    Artifact {
        req: u64,
        artifact: ArtifactInfo,
    },
    /// A web pane's page state, in reply to `artifact_state`: `values` holds the requested key, or
    /// every key; `timed_out` is true when `wait_ms` ran out before the key was written.
    PageState {
        req: u64,
        values: std::collections::BTreeMap<String, serde_json::Value>,
        timed_out: bool,
    },
    /// Every artifact, in reply to `artifact_ls`, ordered project, machine — the order a bare
    /// name resolves in, so the first record carrying a name is the one that name means.
    /// `resolves` says that outright: the scope a bare name lands in, per name.
    Artifacts {
        req: u64,
        artifacts: Vec<ArtifactInfo>,
        resolves: std::collections::BTreeMap<String, ArtifactScope>,
    },
    /// Wheel's settings as the daemon resolved them, in reply to `settings_ls`, `setting_set` or
    /// `setting_unset`, and sent to every connection after a write or a removal.
    ///
    /// `space` is the space the list was resolved for — the one asked about, or the space being
    /// shown when the asker named none — and `None` only when it was resolved against no space at
    /// all, which is the machine-wide view. A client that holds another space's view asks again
    /// for that space rather than taking this list for it.
    Settings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        settings: Vec<SettingInfo>,
    },
    /// One pane's refresh runner, in reply to `artifact_refresh`: `last` is the run it just made.
    Refresh {
        req: u64,
        refresh: RefreshInfo,
    },
    /// Every remembered approval, in reply to `artifact_trust` or `artifact_trust_forget`, sorted
    /// by folder and then command.
    Trusted {
        req: u64,
        trusted: Vec<TrustInfo>,
    },
    /// The directories spaces were made for, newest first, in reply to `space_recent`.
    SpaceRecent {
        req: u64,
        dirs: Vec<RecentDir>,
    },
    /// A request that made something (a space, a pane) succeeded; `id` names it.
    Created {
        req: u64,
        id: String,
    },
    /// `pane_start` put something in `pane`. For an agent, `session` names its session and
    /// `state` is where it stood when the wait ended: `ready` or `busy`, or `starting` or
    /// `failed` when the wait ran out first.
    Started {
        req: u64,
        pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<SessionState>,
    },
    /// What `pane_view` left `pane` showing: `view` is `agent` or `terminal` (none for any other
    /// kind), and `conversation` the Claude Code session id it shows, when it shows one.
    PaneView {
        req: u64,
        pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<PaneView>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
        /// How long the switch took, in milliseconds. Unset when `to` was not given.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        switch_ms: Option<u64>,
    },
    /// `pane_browser_open` sent `pane` to `url`.
    Opened {
        req: u64,
        pane: String,
        url: String,
    },
    /// `pane_shot` wrote `bytes` bytes of PNG to `path`, which is complete.
    PaneShot {
        req: u64,
        pane: String,
        path: String,
        bytes: u64,
    },
    /// Sent to one connection that sent `render_offer`: draw `pane` as a PNG and answer with
    /// `rendered` or `render_refused`, naming this `id`.
    RenderPane {
        id: u64,
        pane: String,
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

impl ServerMsg {
    /// The `req` of the request this message answers; None for a message the daemon sends
    /// unprompted, and for a reply to a request sent without one. The match names every variant,
    /// so a new one cannot be left out.
    pub fn req(&self) -> Option<u64> {
        match self {
            ServerMsg::Status { req, .. }
            | ServerMsg::TermOpened { req, .. }
            | ServerMsg::TermAttached { req, .. }
            | ServerMsg::SessionAttached { req, .. }
            | ServerMsg::Settings { req, .. }
            | ServerMsg::Error { req, .. } => *req,
            ServerMsg::Usage { req, .. }
            | ServerMsg::Remote { req, .. }
            | ServerMsg::SessionQueued { req, .. }
            | ServerMsg::SessionWaited { req, .. }
            | ServerMsg::Conversations { req, .. }
            | ServerMsg::Conversation { req, .. }
            | ServerMsg::Artifact { req, .. }
            | ServerMsg::Artifacts { req, .. }
            | ServerMsg::Refresh { req, .. }
            | ServerMsg::PageState { req, .. }
            | ServerMsg::Trusted { req, .. }
            | ServerMsg::SpaceRecent { req, .. }
            | ServerMsg::Created { req, .. }
            | ServerMsg::Started { req, .. }
            | ServerMsg::PaneView { req, .. }
            | ServerMsg::Opened { req, .. }
            | ServerMsg::PaneShot { req, .. }
            | ServerMsg::Ok { req } => Some(*req),
            ServerMsg::TermOutput { .. }
            | ServerMsg::TermExited { .. }
            | ServerMsg::UsageChanged { .. }
            | ServerMsg::Session { .. }
            | ServerMsg::SessionStateChanged { .. }
            | ServerMsg::PaneUrl { .. }
            | ServerMsg::SessionConfig { .. }
            | ServerMsg::SessionContext { .. }
            | ServerMsg::SessionNotice { .. }
            | ServerMsg::PermRequested { .. }
            | ServerMsg::PermResolved { .. }
            | ServerMsg::Spaces { .. }
            | ServerMsg::PaneReload { .. }
            | ServerMsg::RenderPane { .. } => None,
        }
    }
}

/// A directory a space was made for, and when (unix ms).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentDir {
    pub dir: String,
    pub opened_ms: u64,
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
    /// The conversation is being handed to another program (the Claude Code TUI): the agent is
    /// stopped, and the session takes no messages.
    Switching,
    /// An agent pane's agent stopped too many times in a row before it was ready; the daemon no longer
    /// starts it, and the session takes no messages. `message` is the last stop's reason.
    GaveUp {
        tries: u32,
        message: String,
    },
}

/// A session event with the moment the daemon recorded it, so a client can say how long a tool
/// has run or a thought took and still say it after a relaunch. On the wire `at` sits beside the
/// event's own fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StampedEvent {
    /// Milliseconds since the Unix epoch. Unset for history an agent replayed when it loaded a
    /// past conversation: the agent sends no times, so none is invented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(flatten)]
    pub event: SessionEvent,
}

/// The current time in milliseconds since the Unix epoch; 0 for a clock before it.
pub fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl StampedEvent {
    /// The event, stamped with the current time.
    pub fn now(event: SessionEvent) -> StampedEvent {
        StampedEvent {
            at: Some(unix_ms_now()),
            event,
        }
    }

    /// The event with no time, for replayed history.
    pub fn unstamped(event: SessionEvent) -> StampedEvent {
        StampedEvent { at: None, event }
    }
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
    /// The agent started on the message queued as `turn`. Every `text`, `tool_call`,
    /// `tool_update` and `plan` after this one up to its `turn_end` or `error` belongs to that
    /// turn; `user` events for later messages and an `error` with no `turn` can arrive between.
    TurnStart {
        turn: u64,
    },
    /// A chunk of the agent's reply.
    Text {
        text: String,
    },
    /// A chunk of the agent's reasoning, merged like `text` in the stored transcript.
    Thought {
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
    /// The agent changed its own mode (`mode` is the new mode's id) and offers no `mode` config
    /// option to show it in; when it does, the change arrives as a `config_options` update instead.
    Mode {
        mode: String,
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
    /// Who is asking: an agent pane's session name, or the pane whose artifact wants to run its refresh
    /// command.
    pub session: String,
    /// What is to be done: the tool call as the agent described it, or the command a refresh would
    /// run, with the artifact it belongs to and the directory it would run in.
    pub title: String,
    pub options: Vec<PermOption>,
    /// The agent's id for the tool call this request gates. A refresh command's request has none.
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

/// One of the two directories a web pane watches. Both send the same `pane_reload`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WatchRole {
    /// The artifact's own folder, whose files the pane serves as read-only static content.
    Artifact,
    /// The pane's own data directory, served under the reserved `data/` prefix of its URL.
    Data,
}

/// One directory a web pane watches, and which artifact the pane shows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchInfo {
    pub pane: String,
    /// Which of the pane's two roots this watch is on.
    pub role: WatchRole,
    pub scope: ArtifactScope,
    /// The artifact's name.
    pub name: String,
    /// The directory this watch is of: the folder the daemon resolved the artifact to, or the
    /// pane's own data directory.
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Whether `value` is a page a pane may point at: an `http` or `https` URL with a host, a `file`
/// URL naming an absolute path on this machine (`file:///…`, or `file://localhost/…`), or
/// `about:blank`. The scheme is matched without case, as schemes are; a host that is only
/// userinfo or only a port is no host. Everything else — `data:`, `javascript:`, a file on
/// another host, a bare word, a URL with whitespace in it — is something a web view cannot load
/// or should never be pointed at, and is refused wherever it arrives: a caller's request, a
/// setting in a `wheel.toml`, or a pane in a saved layout.
pub fn page_allowed(value: &str) -> bool {
    let value = value.trim();
    if value.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if let Some(path) = lower
        .strip_prefix("file://localhost")
        .or_else(|| lower.strip_prefix("file://"))
    {
        return path.starts_with('/') && path.len() > 1;
    }
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `user@host`, `host:port` and `user@host:port` all name a host; `:80`, `user@` and `@` do not.
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    !host.split(':').next().unwrap_or_default().is_empty()
}

/// The page a person means by `text`, typed into a browser pane's address field, or why it is
/// none; `home` is the directory `~` stands for. A path starting with `/` or `~` is that file on
/// this machine. Text with a scheme is that URL as typed, so `mailto:` or `ftp://` is refused
/// rather than read as a host; `localhost:3000` has no scheme, only a port. Anything else is a
/// web address and gets `https://`. Whatever comes out passes [`page_allowed`], so typed text
/// reaches no page a request, a setting or a saved layout could not.
pub fn page_typed(text: &str, home: &str) -> Result<String, String> {
    let text = text.trim();
    let path = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => Some(format!("{home}{rest}")),
        // `~bob/x` is another user's home, which `home` is not.
        Some(_) => return Err(format!("{text:?} isn't a web address or a file path")),
        _ if text.starts_with('/') => Some(text.to_string()),
        _ => None,
    };
    let page = match path {
        Some(path) => format!("file://{}", percent_encode(&path)),
        None => {
            let url = match has_scheme(text) {
                true => text.to_string(),
                false => format!("https://{text}"),
            };
            // Whitespace after the host is part of a path or query, and is sent as a browser
            // sends it; any before is no address at all, which `page_allowed` refuses.
            let host_end = url.find("://").map_or(url.len(), |at| {
                url[at + 3..]
                    .find(['/', '?', '#'])
                    .map_or(url.len(), |i| at + 3 + i)
            });
            let (front, back) = url.split_at(host_end);
            let mut out = front.to_string();
            for c in back.chars() {
                match c.is_whitespace() {
                    true => out.push_str(&percent_encode(c.encode_utf8(&mut [0; 4]))),
                    false => out.push(c),
                }
            }
            out
        }
    };
    match page_allowed(&page) {
        true => Ok(page),
        false => Err(format!("{text:?} isn't a web address or a file path")),
    }
}

/// Whether `text` starts with a URL scheme (`mailto:`, `https:`) rather than a host and a port
/// (`localhost:3000`).
fn has_scheme(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let is_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let port = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let is_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
    is_scheme && !is_port
}

/// `path` with every byte a URL path cannot carry as itself written `%XX`.
fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// How full an agent's context window is, as the agent last reported it: tokens in the
/// conversation out of the window's size. The daemon takes it from the agent's usage updates and
/// clients only show it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextUse {
    pub used: u64,
    pub size: u64,
}

impl ContextUse {
    /// The share of the window in use, 0 to 100 and over, rounded; 0 for a window of no size.
    pub fn percent(&self) -> u64 {
        if self.size == 0 {
            return 0;
        }
        let (used, size) = (u128::from(self.used), u128::from(self.size));
        ((used * 100 + size / 2) / size) as u64
    }
}

/// One reading of an account's Claude usage. Percentages are of the plan's limit, 0 to 100 and over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageReading {
    pub session_pct: f64,
    /// When the 5-hour window resets, as the usage endpoint wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_resets_at: Option<String>,
    pub weekly_pct: f64,
    /// When the 7-day window resets, as the usage endpoint wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_resets_at: Option<String>,
    /// The per-model weekly limits the account has (e.g. Fable), in the order the endpoint
    /// listed them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_limits: Vec<UsageLimit>,
    /// The plan's name, e.g. `Max 20x`; None when the profile could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Whether extra usage (paid use past the plan's limits) is on; None when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_usage: Option<bool>,
    /// The account's email; None when the profile could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// When wheeld read it, in milliseconds since the Unix epoch.
    pub read_at: u64,
}

/// One limit besides the 5-hour and weekly ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// The pin id's second half, e.g. `opus`.
    pub id: String,
    /// What a row calls it, e.g. `Weekly Opus`.
    pub label: String,
    pub pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

fn default_agent() -> String {
    "claude".into()
}

/// The PATH wheeld runs with and hands every terminal and agent it starts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoginPath {
    pub path: String,
    /// Where it came from, as a sentence: the login shell, or the environment wheeld started in.
    pub how: String,
    /// Why the login shell gave no PATH when wheeld asked it for one; None when it gave one or
    /// wheeld did not ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One agent as `status` lists it: its `agent.<name>.command` and `agent.<name>.home` settings and
/// where the program resolves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    /// The command as the setting holds it: a program, with any arguments of its own.
    pub command: String,
    /// The `agent.<name>.home` setting: the configuration directory the agent's panes run with.
    /// None when it is unset and the agent uses its own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// The program's absolute path, when it was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub found: bool,
    /// Why the program was not found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One coding agent as the usage views list it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAgent {
    /// `claude`, `codex`.
    pub id: String,
    pub name: String,
    /// Whether the `usage.agents` setting lists it, so the strip and the window show it.
    pub shown: bool,
    /// Whether wheel can read this agent's usage. When not, `why` says why: not installed, or
    /// installed with no reader.
    pub readable: bool,
    /// `not detected` or `can't read usage` reasons; None when the agent is readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// One entry per account this Mac is logged in to, in name order; empty while `readable` is false.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<UsageAccount>,
}

/// One account of a coding agent and its reading. Several logins of one account are one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageAccount {
    /// Stable while the account stays logged in: the start of the account's id.
    pub id: String,
    /// The config directory the login lives in (`.claude-work`), or the plan and a masked email
    /// when no directory matched.
    pub name: String,
    pub usage: UsageInfo,
}

/// Everything the usage strip and window show: the agents and the pinned rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBoard {
    pub agents: Vec<UsageAgent>,
    /// `<agent>:<limit>` ids from the `usage.pins` setting, in the order the strip shows them.
    pub pins: Vec<String>,
}

/// How far a usage reading can be trusted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    /// Read recently and the last read worked.
    Fresh,
    /// The last read failed or the reading is old: `why` says which. The reading is the last good one.
    Stale,
    /// There is no reading: `why` says why.
    Missing,
}

/// One account's Claude usage and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageInfo {
    pub kind: UsageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<UsageReading>,
    /// Seconds since `reading.read_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
    /// Why the reading is stale or missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Where readings come from.
    pub source: String,
}

/// What a pane shows. Clients render a kind they do not know as an unsupported pane.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneKind {
    /// Not chosen yet: the pane asks what it shows.
    Empty { extra: Extra },
    /// A shell. `cwd` is the directory it was last in and `agent` the coding agent it was running,
    /// both sampled by the daemon and saved so a restart opens the shell there and resumes the agent.
    Terminal {
        term: String,
        cwd: Option<String>,
        agent: Option<Box<TermAgent>>,
        extra: Extra,
    },
    /// The artifact `artifact` names, served at `/artifacts/<pane>/` and reloaded when a file in
    /// its folder changes. The daemon resolves the folder from the scope and the name every time
    /// it needs one, so no path is held here and none crosses the API.
    Web { artifact: ArtifactRef, extra: Extra },
    /// An ordinary browser on `url`: the page the person navigated to, which the client reports
    /// back as they go, so a restored pane comes back where they left it. Nothing about an
    /// artifact applies — no folder, no data directory, no token — because this is the open
    /// internet rather than something a project ships.
    Browser { url: String, extra: Extra },
    /// An agent session, named `session`, that the pane owns: it ends when the pane closes
    /// or changes kind. `agent` names the agent it runs (a key of the `agent.<name>.command`
    /// settings); `conversation` is that agent's own id for the conversation, saved so a
    /// restarted daemon can load it. `state` is the session's state, filled in only in the
    /// layout listing (`status`): never saved, and never in the layout
    /// broadcast, where `session_state_changed` carries it. `place` is where the agent runs when
    /// that is not the space's directory with the daemon's own Claude config directory.
    Agent {
        session: String,
        agent: String,
        conversation: Option<String>,
        state: Option<Box<SessionState>>,
        /// The conversation's context use as last reported, saved with the layout so a restarted
        /// daemon shows it before the agent speaks again.
        context: Option<ContextUse>,
        place: Option<Box<AgentPlace>>,
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

/// Where an agent pane's agent runs, saved flat in the pane as `cwd` and `home`: the directory and
/// the Claude config directory, each set only when it is not the space's directory or the daemon's
/// own. A conversation switched over from a terminal keeps the ones Claude Code ran it with there,
/// since that is where its store is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPlace {
    pub cwd: Option<String>,
    pub home: Option<String>,
}

impl AgentPlace {
    /// The place, or none when neither is set.
    pub fn boxed(cwd: Option<String>, home: Option<String>) -> Option<Box<AgentPlace>> {
        (cwd.is_some() || home.is_some()).then(|| Box::new(AgentPlace { cwd, home }))
    }
}

/// A coding agent running in a terminal pane: which one, the id of its session, and the config
/// directory that session lives under when that is not the agent's default one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermAgent {
    pub name: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// How full the session's context window is, read by the daemon from the agent's own
    /// transcript; unset while it has no figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUse>,
}

/// The fields of a known kind this build does not read (written by a newer one), kept so saving
/// the layout writes them back. Replacing the pane's kind drops them with the old content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra(serde_json::Map<String, serde_json::Value>);

/// A pane object this build cannot read: a `kind` it does not know, or a `web` pane whose artifact
/// it cannot make sense of (a scope from a newer build, a name no reference can hold). Only
/// deserializing makes one, and saving writes it back exactly as it arrived.
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<Box<TermAgent>>,
    },
    Web {
        scope: ArtifactScope,
        name: String,
    },
    Browser {
        url: String,
    },
    Agent {
        session: String,
        /// A pane saved before panes named their agent ran Claude Code.
        #[serde(default = "default_agent")]
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<SessionState>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<String>,
    },
}

impl Serialize for PaneKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (known, Extra(extra)) = match self {
            PaneKind::Empty { extra } => (KnownPaneKind::Empty, extra),
            PaneKind::Terminal {
                term,
                cwd,
                agent,
                extra,
            } => (
                KnownPaneKind::Terminal {
                    term: term.clone(),
                    cwd: cwd.clone(),
                    agent: agent.clone(),
                },
                extra,
            ),
            PaneKind::Web { artifact, extra } => (
                KnownPaneKind::Web {
                    scope: artifact.scope(),
                    name: artifact.name().to_string(),
                },
                extra,
            ),
            PaneKind::Browser { url, extra } => {
                (KnownPaneKind::Browser { url: url.clone() }, extra)
            }
            PaneKind::Agent {
                session,
                agent,
                conversation,
                state,
                context,
                place,
                extra,
            } => (
                KnownPaneKind::Agent {
                    session: session.clone(),
                    agent: agent.clone(),
                    conversation: conversation.clone(),
                    state: state.as_deref().cloned(),
                    context: *context,
                    cwd: place.as_ref().and_then(|p| p.cwd.clone()),
                    home: place.as_ref().and_then(|p| p.home.clone()),
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
            Some("empty" | "terminal" | "web" | "browser" | "agent") => {}
            Some(_) => return Ok(PaneKind::Unknown(UnknownPane(raw))),
            None => return Err(serde::de::Error::custom("pane has no string `kind`")),
        }
        let known: KnownPaneKind =
            match serde_json::from_value(serde_json::Value::Object(raw.clone())) {
                Ok(known) => known,
                // A web pane whose `scope` this build does not know was written by a newer one, and
                // a browser pane with no `url` or a `url` that is not a string is a layout nothing
                // wrote — a hand edit, or a torn file. Keep the pane whole, like any unknown kind,
                // rather than failing the whole layout: one pane nobody here can draw must never
                // cost every space in the file.
                Err(_) if raw["kind"] == "web" || raw["kind"] == "browser" => {
                    return Ok(PaneKind::Unknown(UnknownPane(raw)))
                }
                Err(e) => return Err(serde::de::Error::custom(e)),
            };
        let fields = known_fields(&known).map_err(serde::de::Error::custom)?;
        let extra = Extra(
            raw.iter()
                .filter(|(key, _)| !fields.contains_key(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        Ok(match known {
            KnownPaneKind::Empty => PaneKind::Empty { extra },
            KnownPaneKind::Terminal { term, cwd, agent } => PaneKind::Terminal {
                term,
                cwd,
                agent,
                extra,
            },
            // The same for a name no reference can hold, which only a hand-edited layout has.
            KnownPaneKind::Web { scope, name } => match ArtifactRef::new(scope, &name) {
                Ok(artifact) => PaneKind::Web { artifact, extra },
                Err(_) => PaneKind::Unknown(UnknownPane(raw)),
            },
            // A page this build would refuse to open is not a browser pane it can draw: a layout
            // only reaches here by hand or by damage, and the rule about what a pane may point at
            // holds against the file as well as against a caller.
            KnownPaneKind::Browser { url } => match page_allowed(&url) {
                true => PaneKind::Browser { url, extra },
                false => PaneKind::Unknown(UnknownPane(raw)),
            },
            KnownPaneKind::Agent {
                session,
                agent,
                conversation,
                state,
                context,
                cwd,
                home,
            } => PaneKind::Agent {
                session,
                agent,
                conversation,
                state: state.map(Box::new),
                context,
                place: AgentPlace::boxed(cwd, home),
                extra,
            },
        })
    }
}

/// What `pane_set_kind` turns a pane into.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NewPaneKind {
    Empty,
    Terminal,
    /// A web view of an artifact; needs `artifact`, and takes an optional `scope`.
    Web,
    /// An ordinary browser; takes an optional `url`, and opens the `browser.homepage` setting
    /// without one.
    Browser,
}

/// The two ways a pane shows a Claude Code conversation: wheel's agent chat, or the Claude Code
/// TUI in a terminal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneView {
    Agent,
    Terminal,
}

/// What `pane_start` puts in a pane.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartKind {
    /// A shell in the space's directory.
    Terminal,
    /// An agent session running the daemon's agent command in the space's directory.
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

/// The turns of `user` events still waiting to start: no `turn_start` for them yet. The one
/// rule text transcripts use to mark a message `(queued)`.
pub fn queued_turns(events: &[StampedEvent]) -> std::collections::HashSet<u64> {
    let mut queued = std::collections::HashSet::new();
    for e in events {
        match e.event {
            SessionEvent::User { turn, .. } => {
                queued.insert(turn);
            }
            SessionEvent::TurnStart { turn } => {
                queued.remove(&turn);
            }
            _ => {}
        }
    }
    queued
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_message_is_queued_until_its_turn_starts() {
        let ev = |e| StampedEvent::unstamped(e);
        let events = vec![
            ev(SessionEvent::User {
                turn: 1,
                text: "a".into(),
            }),
            ev(SessionEvent::TurnStart { turn: 1 }),
            ev(SessionEvent::User {
                turn: 2,
                text: "b".into(),
            }),
        ];
        assert_eq!(queued_turns(&events), [2].into_iter().collect());
    }

    #[test]
    fn typed_text_names_a_web_address_a_file_or_nothing() {
        for (typed, page) in [
            ("example.com", "https://example.com"),
            ("  localhost:3000/x ", "https://localhost:3000/x"),
            ("127.0.0.1:8000", "https://127.0.0.1:8000"),
            (
                "http://example.com/a b?q=c\td",
                "http://example.com/a%20b?q=c%09d",
            ),
            ("/tmp/a b#1.html", "file:///tmp/a%20b%231.html"),
            ("~/notes.html", "file:///Users/me/notes.html"),
            ("file://localhost/tmp/a.html", "file://localhost/tmp/a.html"),
            ("ABOUT:BLANK", "ABOUT:BLANK"),
        ] {
            assert_eq!(
                page_typed(typed, "/Users/me").as_deref(),
                Ok(page),
                "{typed:?}"
            );
        }
        for refused in [
            "",
            "rust async",
            "https://exa mple.com",
            "http://",
            "http://:80/",
            "mailto:a@b.com",
            "ftp://example.com",
            "data:text/html,hi",
            "javascript:alert(1)",
            "blob:https://x/y",
            "file:/tmp/a.html",
            "file://otherhost/x",
            "~bob/x",
            "/",
        ] {
            assert!(
                page_typed(refused, "/Users/me").is_err(),
                "{refused:?} is no page"
            );
        }
    }

    #[test]
    fn an_artifact_name_is_one_plain_component() {
        for good in ["beads", "status-dash", "a", "epg_2", "Mixed.Case"] {
            let r = ArtifactRef::new(ArtifactScope::Machine, good).unwrap();
            assert_eq!((r.scope(), r.name()), (ArtifactScope::Machine, good));
        }
        for bad in [
            "",
            ".",
            "..",
            "/",
            "/etc",
            "a/b",
            "a/",
            "../out",
            ".env",
            "-x",
            "sub/../..",
        ] {
            assert!(
                ArtifactRef::new(ArtifactScope::Project, bad).is_err(),
                "{bad:?} is not an artifact name"
            );
        }
    }

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
            content: Some(vec![ToolContent::Diff {
                path: "a.rs".into(),
                old_text: None,
                new_text: "x".into(),
            }]),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"tool_update","id":"c1","content":[{"type":"diff","path":"a.rs","new_text":"x"}]}"#
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
    fn a_stamp_sits_beside_the_event_fields() {
        let stamped = StampedEvent {
            at: Some(1_760_000_000_000),
            event: SessionEvent::TurnStart { turn: 2 },
        };
        let json = serde_json::to_string(&stamped).unwrap();
        assert_eq!(json, r#"{"at":1760000000000,"kind":"turn_start","turn":2}"#);
        assert_eq!(
            serde_json::from_str::<StampedEvent>(&json).unwrap(),
            stamped
        );
        let replayed = StampedEvent::unstamped(SessionEvent::Text { text: "hi".into() });
        assert_eq!(
            serde_json::to_string(&replayed).unwrap(),
            r#"{"kind":"text","text":"hi"}"#
        );
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
        let web = serde_json::json!(
            {"node": "pane", "id": "p5", "kind": "web", "scope": "project", "name": "viewer"}
        );
        let node: Node = serde_json::from_value(web.clone()).unwrap();
        assert!(matches!(
            &node,
            Node::Pane { kind: PaneKind::Web { artifact, .. }, .. }
                if artifact.scope() == ArtifactScope::Project && artifact.name() == "viewer"
        ));
        assert_eq!(serde_json::to_value(&node).unwrap(), web);
        // A saved layout a person edited by hand cannot smuggle a path in as a name, and a scope a
        // newer build wrote is not one either. Both keep the pane as an unknown one and write it
        // back unchanged, rather than failing the layout and taking every space with it.
        let mut unreadable: Vec<serde_json::Value> =
            ["/etc", "../out", ".", "..", ".env", "a/b", ""]
                .iter()
                .map(|bad| {
                    serde_json::json!(
                        {"node": "pane", "id": "p5", "kind": "web", "scope": "project", "name": bad}
                    )
                })
                .collect();
        unreadable.push(serde_json::json!(
            {"node": "pane", "id": "p5", "kind": "web", "scope": "hologram", "name": "viewer"}
        ));
        unreadable.push(serde_json::json!({"node": "pane", "id": "p5", "kind": "web"}));
        for saved in unreadable {
            let node: Node = serde_json::from_value(saved.clone())
                .unwrap_or_else(|e| panic!("{saved} failed the layout: {e}"));
            assert!(
                matches!(&node, Node::Pane { kind: PaneKind::Unknown(pane), .. } if pane.kind() == "web"),
                "{saved} is not a web pane this build can read"
            );
            assert_eq!(serde_json::to_value(&node).unwrap(), saved);
        }
        let agent = serde_json::json!({"node": "pane", "id": "p6", "kind": "agent", "session": "w7", "agent": "codex"});
        let node: Node = serde_json::from_value(agent.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Agent { session, agent, .. }, .. } if session == "w7" && agent == "codex")
        );
        assert_eq!(serde_json::to_value(&node).unwrap(), agent);
        // A pane saved without an agent name ran Claude Code.
        let old = serde_json::json!({"node": "pane", "id": "p6", "kind": "agent", "session": "w7"});
        let node: Node = serde_json::from_value(old).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Agent { agent, .. }, .. } if agent == "claude")
        );
        let saved = serde_json::json!({"node": "pane", "id": "p6", "kind": "agent", "session": "w7", "agent": "claude", "conversation": "abc"});
        let node: Node = serde_json::from_value(saved.clone()).unwrap();
        assert!(
            matches!(&node, Node::Pane { kind: PaneKind::Agent { conversation: Some(id), extra, .. }, .. } if id == "abc" && *extra == Extra::default())
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
