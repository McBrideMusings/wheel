//! Wheel's own settings: one table of keys, resolved per project over machine-wide.
//!
//! Two files hold them — `wheel.toml` at the root of a space's directory, and `wheel.toml` in the
//! instance directory beside the token and the saved layout — and a key set in the project's file
//! wins over the machine's, key by key, with a built-in default behind both. A reader is always
//! told which of the three a value came from, because a homepage that is not the one a person
//! expected is otherwise only diagnosable by opening two files and knowing which one wins.
//!
//! Nothing about settings goes in the saved layout, for the same reason nothing about an artifact
//! does: a daemon that drops unknown top-level keys when it rewrites that file must not be able to
//! lose them. Every read goes to the files themselves, so a value a person changes by hand is in
//! force at the next read rather than at the next restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wheel_proto::{SettingInfo, SettingOrigin};

use crate::server::AppState;

/// The file holding settings, in a project's directory and in the instance directory alike.
pub(crate) const FILE: &str = "wheel.toml";

/// What a key's value has to look like. A key wheel cannot check is not a key: every value a
/// person can write is refused here or accepted knowingly, so no setting reaches the thing that
/// uses it as something that thing has to re-check.
#[derive(Clone, Copy)]
enum Accepts {
    /// A page a browser pane can open: `http://…`, `https://…`, `file:///…` or `about:blank`.
    Page,
    /// An absolute path to an executable file.
    Executable,
    /// Comma-separated entries of letters, digits, `-`, `_` and `:`, or nothing.
    List,
    /// A program to run, with any arguments of its own. Whether it exists is not checked here:
    /// `status` reports that, so a program that is missing is a reading, not a refused write.
    Command,
    /// The name of an agent: one of the `agent.<name>.command` keys.
    Agent,
    /// An absolute directory path, or nothing for the agent's own default.
    Directory,
    /// An address to listen on, `host:port` with a numeric host, or nothing for off.
    Listen,
    /// An `http://` or `https://` URL with a host, or nothing for off.
    PushUrl,
    /// A whole number of seconds, at least 1.
    Seconds,
    /// Comma-separated folders, each `~`, `~/…` or an absolute path; at least one.
    Roots,
    /// Key bindings as Ghostty writes them, comma separated: [`crate::keybind`].
    Keybind,
    /// One of a fixed set of words.
    Choice(&'static [&'static str]),
    /// A whole number between the two bounds, inclusive.
    Int(i64, i64),
    /// A number between the two bounds, inclusive.
    Float(f64, f64),
    /// `true` or `false`.
    Bool,
}

impl Accepts {
    /// The value as it will be stored, or why it is refused. Trimmed, because a trailing space in
    /// a TOML string is invisible in every reader and breaks the thing that uses it.
    ///
    /// Every value goes through here, whoever wrote it: a `set` before it is written, and a value
    /// read out of a file before it is reported, so a page a person typed into `wheel.toml` by
    /// hand cannot reach the thing that opens it unchecked.
    fn take(self, value: &str) -> Result<String, String> {
        match self {
            Accepts::Page => page(value),
            Accepts::Executable => executable(value),
            Accepts::List => list(value),
            Accepts::Command => command(value),
            Accepts::Agent => agent(value),
            Accepts::Directory => directory(value),
            Accepts::Listen => listen(value),
            Accepts::PushUrl => push_url(value),
            Accepts::Seconds => seconds(value),
            Accepts::Roots => roots(value),
            Accepts::Keybind => crate::keybind::take(value),
            Accepts::Choice(words) => choice(value, words),
            Accepts::Int(min, max) => int(value, min, max),
            Accepts::Float(min, max) => float(value, min, max),
            Accepts::Bool => boolean(value),
        }
    }

    /// The kind of control a client draws for the key.
    fn kind(self) -> wheel_proto::SettingKind {
        use wheel_proto::SettingKind;
        match self {
            Accepts::Choice(_) => SettingKind::Choice,
            Accepts::Int(..) => SettingKind::Int,
            Accepts::Float(..) => SettingKind::Float,
            Accepts::Bool => SettingKind::Bool,
            _ => SettingKind::Text,
        }
    }

    /// The words a `Choice` takes, in the order a client lists them; none for any other kind.
    fn choices(self) -> Vec<String> {
        match self {
            Accepts::Choice(words) => words.iter().map(|w| w.to_string()).collect(),
            _ => Vec::new(),
        }
    }

    /// The bounds of a numeric kind.
    fn bounds(self) -> Option<(f64, f64)> {
        match self {
            Accepts::Int(min, max) => Some((min as f64, max as f64)),
            Accepts::Float(min, max) => Some((min, max)),
            _ => None,
        }
    }

    /// The value as a TOML file holds it: a number or a boolean for the kinds that are one, so a
    /// file a person reads shows `size = 17` and not `size = "17"`; a string for the rest.
    fn toml(self, value: &str) -> toml_edit::Value {
        match self {
            Accepts::Int(..) => value.parse::<i64>().map(Into::into).unwrap_or_else(|_| value.into()),
            Accepts::Float(..) => value.parse::<f64>().map(Into::into).unwrap_or_else(|_| value.into()),
            Accepts::Bool => value.parse::<bool>().map(Into::into).unwrap_or_else(|_| value.into()),
            // A choice of widths, such as 560, is a number in the file.
            Accepts::Choice(_) => value.parse::<i64>().map(Into::into).unwrap_or_else(|_| value.into()),
            _ => value.into(),
        }
    }
}

/// One of `words`.
fn choice(value: &str, words: &[&str]) -> Result<String, String> {
    let value = value.trim();
    match words.contains(&value) {
        true => Ok(value.to_string()),
        false => Err(format!("{value:?} is not one of {}", words.join(", "))),
    }
}

/// A whole number from `min` to `max`.
fn int(value: &str, min: i64, max: i64) -> Result<String, String> {
    let value = value.trim();
    match value.parse::<i64>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n.to_string()),
        _ => Err(format!("{value:?} is not a whole number from {min} to {max}")),
    }
}

/// A number from `min` to `max`.
fn float(value: &str, min: f64, max: f64) -> Result<String, String> {
    let value = value.trim();
    match value.parse::<f64>() {
        Ok(n) if n.is_finite() && (min..=max).contains(&n) => Ok(n.to_string()),
        _ => Err(format!("{value:?} is not a number from {min} to {max}")),
    }
}

/// `true` or `false`.
fn boolean(value: &str) -> Result<String, String> {
    match value.trim() {
        v @ ("true" | "false") => Ok(v.to_string()),
        other => Err(format!("{other:?} is not true or false")),
    }
}

/// What a browser pane may point at, and what the `browser.homepage` setting may hold: one rule,
/// [`wheel_proto::page_allowed`], which the saved layout is read against as well, so a pane, a
/// setting and a file cannot disagree about what a page is.
pub(crate) fn page(value: &str) -> Result<String, String> {
    let value = value.trim();
    match wheel_proto::page_allowed(value) {
        true => Ok(value.to_string()),
        false => Err(format!(
            "{value:?} is not a page a pane can open: give an http:// or https:// URL with a host, a file:/// URL, or about:blank"
        )),
    }
}

/// Comma-separated ids, stored without spaces.
fn list(value: &str) -> Result<String, String> {
    let entries: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();
    match entries.iter().find(|e| {
        !e.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
    }) {
        Some(bad) => Err(format!(
            "{bad:?} is not an id: use letters, digits, - _ and :"
        )),
        None => Ok(entries.join(",")),
    }
}

/// A non-empty command line.
fn command(value: &str) -> Result<String, String> {
    let value = value.trim();
    match crate::claude::words(value).is_empty() {
        true => Err("an agent command names a program".into()),
        false => Ok(value.to_string()),
    }
}

/// The names of the agents wheel has, in key order: the middle of every `agent.<name>.command`.
pub(crate) fn agent_names() -> Vec<&'static str> {
    KEYS.iter()
        .filter_map(|k| k.id.strip_prefix("agent.")?.strip_suffix(".command"))
        .collect()
}

/// An absolute path, or empty. Whether it exists is not checked: an agent creates its own home on
/// first start, so a directory that is not there yet is a fresh login, not a refused write.
fn directory(value: &str) -> Result<String, String> {
    let value = value.trim();
    match value.is_empty() || Path::new(value).is_absolute() {
        true => Ok(value.to_string()),
        false => Err(format!("{value:?} is not an absolute path")),
    }
}

/// Folders to browse from, stored without spaces around the commas.
fn roots(value: &str) -> Result<String, String> {
    let entries: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();
    if entries.is_empty() {
        return Err("give at least one folder, or unset the setting for the default".into());
    }
    match entries
        .iter()
        .find(|e| !(**e == "~" || e.starts_with("~/") || Path::new(e).is_absolute()))
    {
        Some(bad) => Err(format!(
            "{bad:?} is not a folder to browse from: use ~, a path under ~/, or an absolute path"
        )),
        None => Ok(entries.join(",")),
    }
}

/// A socket address like `0.0.0.0:7430`, or empty for no remote listener.
fn listen(value: &str) -> Result<String, String> {
    let value = value.trim();
    match value.is_empty() || value.parse::<std::net::SocketAddr>().is_ok() {
        true => Ok(value.to_string()),
        false => Err(format!(
            "{value:?} is not an address to listen on: give an IP address and a port such as 0.0.0.0:7430, or nothing"
        )),
    }
}

/// Where the daemon pushes its log lines: a Loki push endpoint, or empty for not at all.
fn push_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let host = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .map(|rest| rest.split(['/', '?']).next().unwrap_or(""));
    match (value.is_empty(), host) {
        (true, _) => Ok(String::new()),
        (false, Some(h)) if !h.is_empty() => Ok(value.to_string()),
        _ => Err(format!(
            "{value:?} is not a push URL: give an http:// or https:// URL with a host, such as http://10.0.0.5:3100/loki/api/v1/push, or nothing"
        )),
    }
}

/// A whole number of seconds, at least 1.
fn seconds(value: &str) -> Result<String, String> {
    let value = value.trim();
    match value.parse::<u64>() {
        Ok(n) if n >= 1 => Ok(n.to_string()),
        _ => Err(format!(
            "{value:?} is not a number of seconds: give a whole number, 1 or more"
        )),
    }
}

fn agent(value: &str) -> Result<String, String> {
    let value = value.trim();
    match agent_names().contains(&value) {
        true => Ok(value.to_string()),
        false => Err(format!(
            "{value:?} is not an agent: wheel's agents are {}",
            agent_names().join(", ")
        )),
    }
}

/// An absolute path to a file with an execute bit set. Checked where it is written and where it is
/// read, so a hand-written path to a missing or non-executable file is the key's problem rather
/// than a terminal that fails to start.
pub(crate) fn executable(value: &str) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let value = value.trim();
    if !Path::new(value).is_absolute() {
        return Err(format!("{value:?} is not an absolute path"));
    }
    match std::fs::metadata(value) {
        Err(e) => Err(format!("{value:?} cannot be read: {e}")),
        Ok(m) if !m.is_file() => Err(format!("{value:?} is not a file")),
        Ok(m) if m.permissions().mode() & 0o111 == 0 => Err(format!("{value:?} is not executable")),
        Ok(_) => Ok(value.to_string()),
    }
}

/// One setting wheel has.
struct Key {
    id: &'static str,
    accepts: Accepts,
    default: &'static str,
    help: &'static str,
}

/// The key a browser pane opens when nobody gave it a URL.
pub(crate) const HOMEPAGE: &str = "browser.homepage";

/// The key naming the shell a new terminal pane starts.
pub(crate) const SHELL: &str = "terminal.shell";

/// The key naming the agent a new agent pane runs when nobody named one.
pub(crate) const AGENT_DEFAULT: &str = "agent.default";

/// The key naming the address the remote (TLS) listener binds.
pub(crate) const REMOTE_LISTEN: &str = "remote.listen";

/// The key naming the Loki push endpoint the daemon ships its log lines to.
pub(crate) const LOGS_PUSH_URL: &str = "logs.push_url";

/// The key holding the seconds the log shipper waits after a failed push before its first retry.
pub(crate) const LOGS_RETRY_S: &str = "logs.retry_s";

/// The key listing the status ranks that show a dot.
pub(crate) const STATUS_DOTS: &str = "status.dots";

/// The key listing the folders the space browser starts from.
pub(crate) const SPACES_ROOTS: &str = "spaces.roots";

/// The key holding the key bindings over the Mac app's own.
pub(crate) const KEYBIND: &str = "keybind";

/// The keys only this machine's file sets: what the usage strip shows, the programs the agents run,
/// whether the daemon listens for remote clients, which status dots show, where its logs go and
/// which keys do what belong to the machine, not to the space being shown.
fn machine_only(id: &str) -> bool {
    id == KEYBIND
        || id.starts_with("usage.")
        || id.starts_with("agent.")
        || id.starts_with("remote.")
        || id.starts_with("status.")
        || id.starts_with("logs.")
        || id.starts_with("spaces.")
}

/// Every setting wheel has. A key absent here does not exist: a `get` or a `set` naming one is
/// refused, naming the keys that do exist. A key in a `wheel.toml` that is not one of these is a
/// key wheel never reads, which `wheel config ls` cannot report because it reports settings, not
/// file contents — so a typo in a hand-written file is silent, and finding it is reading the file.
const KEYS: [Key; 42] = [
    Key {
        id: HOMEPAGE,
        accepts: Accepts::Page,
        default: "about:blank",
        help: "The page a browser pane opens on when nobody gave it a URL.",
    },
    Key {
        id: SHELL,
        accepts: Accepts::Executable,
        default: "",
        help: "Absolute path of the shell a new terminal pane starts; empty means the login shell. A shell set here starts as a plain shell, not a login shell.",
    },
    Key {
        id: crate::usage::AGENTS_KEY,
        accepts: Accepts::List,
        default: "claude",
        help: "The coding agents the usage strip and the Usage window show, comma separated (claude, codex). An agent wheel cannot read usage for is listed there as not detected or unreadable. Read from this machine's file only.",
    },
    Key {
        id: crate::usage::PINS_KEY,
        accepts: Accepts::List,
        default: "claude:session,claude:weekly",
        help: "The usage rows pinned to the strip, comma separated as agent:limit (claude:session, claude:weekly, claude:opus). Read from this machine's file only.",
    },
    Key {
        id: "agent.claude.command",
        accepts: Accepts::Command,
        default: "claude",
        help: "The program an agent pane running Claude Code starts, with any arguments of its own; found on PATH when it has no directory. Read from this machine's file only.",
    },
    Key {
        id: "agent.codex.command",
        accepts: Accepts::Command,
        default: "codex",
        help: "The program an agent pane running Codex starts, with any arguments of its own; found on PATH when it has no directory. Read from this machine's file only.",
    },
    Key {
        id: "agent.pi.command",
        accepts: Accepts::Command,
        default: "pi",
        help: "The program an agent pane running pi starts, with any arguments of its own (a --model, say); found on PATH when it has no directory. Read from this machine's file only.",
    },
    Key {
        id: "agent.claude.home",
        accepts: Accepts::Directory,
        default: "",
        help: "The directory Claude Code runs with as CLAUDE_CONFIG_DIR, where its login and conversations live; empty means its own default, ~/.claude. A pane that was moved from a terminal keeps the directory it ran with. Read from this machine's file only.",
    },
    Key {
        id: "agent.codex.home",
        accepts: Accepts::Directory,
        default: "",
        help: "The directory Codex runs with as CODEX_HOME, where its login and threads live; empty means its own default, ~/.codex. Read from this machine's file only.",
    },
    Key {
        id: "agent.pi.home",
        accepts: Accepts::Directory,
        default: "",
        help: "The directory pi runs with as PI_CODING_AGENT_DIR, where its settings, logins and sessions live; empty means its own default, ~/.pi/agent. Read from this machine's file only.",
    },
    Key {
        id: AGENT_DEFAULT,
        accepts: Accepts::Agent,
        default: "claude",
        help: "The agent a new agent pane runs when nobody names one (claude, codex, pi). Read from this machine's file only.",
    },
    Key {
        id: REMOTE_LISTEN,
        accepts: Accepts::Listen,
        default: "",
        help: "The address wheeld also listens on for remote clients over TLS with a self-signed certificate, such as 0.0.0.0:7430; empty means no remote access. Read when wheeld starts, from this machine's file only; `wheel remote info` shows the certificate fingerprint a client pins.",
    },
    Key {
        id: STATUS_DOTS,
        accepts: Accepts::List,
        default: "needs_you,stopped,working,idle",
        help: "The status ranks that show a dot beside a pane, a space's sidebar row and its tab, comma separated (needs_you, stopped, working, idle); empty shows no dots. Read from this machine's file only.",
    },
    Key {
        id: LOGS_PUSH_URL,
        accepts: Accepts::PushUrl,
        default: "",
        help: "The Loki push endpoint (…/loki/api/v1/push) the daemon ships its log, its usage journal and the apps' log lines to; empty means they stay on this machine. `wheel status` shows whether they are arriving. Read from this machine's file only.",
    },
    Key {
        id: LOGS_RETRY_S,
        accepts: Accepts::Seconds,
        default: "5",
        help: "The seconds the log shipper waits after a failed push before it tries again, checked every 5 seconds so a value under 5 waits 5; each further failure doubles the wait, up to 300. A store that sends its own Retry-After overrides it. Read from this machine's file only.",
    },
    Key {
        id: SPACES_ROOTS,
        accepts: Accepts::Roots,
        default: "~",
        help: "The folders the new-space browser starts from, comma separated (~, ~/Projects, /srv/code). Unset, it is ~ plus the parent folder of every recent space. Read from this machine's file only.",
    },
    Key {
        id: KEYBIND,
        accepts: Accepts::Keybind,
        default: "",
        help: "Key bindings over Wheel's own, comma separated, written as Ghostty writes them: cmd+shift+d=split-down binds a key, cmd+d=unbind frees one, and clear drops every binding before it. Modifiers are cmd, ctrl, opt and shift; keys are a letter, a digit, f1–f12 or a name (enter, tab, escape, arrow_left, comma, equal, minus, bracket_left). Read from this machine's file only.",
    },
    Key {
        id: "chat.font.family",
        accepts: Accepts::Choice(&["system", "serif", "rounded", "mono"]),
        default: "system",
        help: "The typeface of the chat transcript: system, serif, rounded or mono.",
    },
    Key {
        id: "chat.font.size",
        accepts: Accepts::Int(10, 22),
        default: "13",
        help: "The size of chat text in points, 10 to 22. Tool lines and times follow it, a step or two smaller.",
    },
    Key {
        id: "chat.line_height",
        accepts: Accepts::Float(1.1, 2.0),
        default: "1.45",
        help: "The height of a line of chat text as a multiple of its size, 1.1 to 2.0.",
    },
    Key {
        id: "chat.paragraph_spacing",
        accepts: Accepts::Int(0, 24),
        default: "8",
        help: "The space between paragraphs and blocks of a reply, in points, 0 to 24.",
    },
    Key {
        id: "chat.max_width",
        accepts: Accepts::Choice(&["none", "560", "680", "820"]),
        default: "none",
        help: "The widest a reply and a message may be, in points; none uses the whole pane.",
    },
    Key {
        id: "chat.message.style",
        accepts: Accepts::Choice(&["bubble", "accent", "plain"]),
        default: "accent",
        help: "How your messages are drawn: bubble, accent or plain.",
    },
    Key {
        id: "chat.message.align",
        accepts: Accepts::Choice(&["right", "left"]),
        default: "right",
        help: "Which side your messages sit on: right or left.",
    },
    Key {
        id: "chat.density",
        accepts: Accepts::Choice(&["compact", "comfortable", "spacious"]),
        default: "comfortable",
        help: "The space between turns and between the parts of a turn: compact, comfortable or spacious.",
    },
    Key {
        id: "chat.quote.style",
        accepts: Accepts::Choice(&["bar", "fill", "italic"]),
        default: "bar",
        help: "How a block quote is drawn: bar, fill or italic.",
    },
    Key {
        id: "chat.timestamps",
        accepts: Accepts::Bool,
        default: "false",
        help: "Show the time under each message.",
    },
    Key {
        id: "chat.tools.summary",
        accepts: Accepts::Choice(&["each", "runs", "turn"]),
        default: "turn",
        help: "How tool calls are summarised: each call, each run of calls, or one line per turn.",
    },
    Key {
        id: "chat.thinking",
        accepts: Accepts::Choice(&["live", "collapsed", "hidden"]),
        default: "collapsed",
        help: "How the agent's reasoning shows: live while it streams, collapsed, or hidden.",
    },
    Key {
        id: "chat.edits.card",
        accepts: Accepts::Bool,
        default: "true",
        help: "Show a card of the files a turn edited.",
    },
    Key {
        id: "chat.links.files",
        accepts: Accepts::Choice(&["preview", "pane", "off"]),
        default: "preview",
        help: "What a file path in a reply does when clicked: preview it, open it in a pane, or nothing.",
    },
    Key {
        id: "chat.links.file_label",
        accepts: Accepts::Choice(&["name", "full"]),
        default: "name",
        help: "Show a file link as its name or its full path.",
    },
    Key {
        id: "chat.links.panes",
        accepts: Accepts::Bool,
        default: "true",
        help: "Turn a pane id in a reply into a link that focuses the pane.",
    },
    Key {
        id: "chat.links.web",
        accepts: Accepts::Choice(&["pane", "system"]),
        default: "pane",
        help: "Where a web link in a reply opens: a browser pane, or the system browser.",
    },
    Key {
        id: "chat.images.inline",
        accepts: Accepts::Bool,
        default: "true",
        help: "Show an image in a reply in the transcript.",
    },
    Key {
        id: "chat.images.max_height",
        accepts: Accepts::Int(60, 600),
        default: "220",
        help: "The tallest an inline image may be, in points, 60 to 600.",
    },
    Key {
        id: "chat.session.home",
        accepts: Accepts::Choice(&["row", "header", "transcript", "off"]),
        default: "row",
        help: "Where the agent's home directory shows: row, header, transcript or off.",
    },
    Key {
        id: "code.theme",
        accepts: Accepts::Choice(&["wheel-muted", "xcode", "github", "mono", "solarized"]),
        default: "wheel-muted",
        help: "The colours of code in chat.",
    },
    Key {
        id: "code.font",
        accepts: Accepts::Choice(&["sf-mono", "menlo", "monaco", "courier-new"]),
        default: "sf-mono",
        help: "The typeface of code in chat.",
    },
    Key {
        id: "code.size",
        accepts: Accepts::Int(9, 20),
        default: "12",
        help: "The size of code in chat, in points, 9 to 20.",
    },
    Key {
        id: "code.wrap",
        accepts: Accepts::Choice(&["scroll", "wrap"]),
        default: "scroll",
        help: "Whether a long line of code scrolls sideways or wraps.",
    },
    Key {
        id: "code.line_numbers",
        accepts: Accepts::Bool,
        default: "false",
        help: "Show line numbers beside code blocks.",
    },
];

/// The named key, or an error naming every key there is.
fn key(id: &str) -> Result<&'static Key, String> {
    KEYS.iter().find(|k| k.id == id).ok_or_else(|| {
        format!(
            "no setting {id}. Wheel's settings are: {}",
            KEYS.iter().map(|k| k.id).collect::<Vec<_>>().join(", ")
        )
    })
}

/// What one layer — one `wheel.toml` — contributes: the values it sets that wheel accepts, and
/// what is wrong with the ones it does not, keyed the same way.
#[derive(Default)]
struct Layer {
    sets: BTreeMap<&'static str, String>,
    /// Per key, why this layer does not set it although it tried: a value the key does not take, a
    /// value of the wrong type, or a file that could not be read at all (recorded against every
    /// key, since the file might have set any of them).
    problems: BTreeMap<&'static str, String>,
}

/// The `[runner]` table of one `wheel.toml`: every string value, keyed by its name. A file that is
/// missing, unreadable or not TOML has none, and the last two are logged.
fn runners(path: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml is not valid TOML; it names no runners");
            return Vec::new();
        }
    };
    let Some(runner) = table.get("runner").and_then(|r| r.as_table()) else {
        return Vec::new();
    };
    runner
        .iter()
        .filter_map(|(name, value)| match value.as_str() {
            Some(command) if !command.trim().is_empty() => {
                Some((name.clone(), command.to_string()))
            }
            _ => {
                tracing::warn!(path = %path.display(), %name, kind = value.type_str(), "a [runner] entry is not a non-empty string; it is ignored");
                None
            }
        })
        .collect()
}

/// What one `wheel.toml` sets. A file that is not there sets nothing, which is not a problem. A
/// file that cannot be read or parsed, and a value the key does not take, each set nothing and are
/// reported as a problem on every key they could have set: the layer below is in force, and a
/// person who wrote a value wheel refused has to be told, or the refusal lives only in the log.
///
/// A value is checked here, not only where it is written, because a person edits this file by hand
/// and nothing downstream re-checks what the daemon reports.
fn file(path: &Path) -> Layer {
    let whole = |why: String| Layer {
        sets: BTreeMap::new(),
        problems: KEYS.iter().map(|k| (k.id, why.clone())).collect(),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Layer::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml cannot be read; it sets nothing");
            return whole(format!("{} cannot be read: {e}", path.display()));
        }
    };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml is not valid TOML; it sets nothing");
            return whole(format!("{} is not valid TOML: {e}", path.display()));
        }
    };
    let mut layer = Layer::default();
    for k in KEYS {
        let Some(value) = at(&table, k.id) else {
            continue;
        };
        let written = match (k.accepts, value) {
            (_, toml::Value::String(text)) => Some(text.clone()),
            (Accepts::Int(..), toml::Value::Integer(n)) => Some(n.to_string()),
            (Accepts::Float(..), toml::Value::Float(n)) => Some(n.to_string()),
            (Accepts::Float(..), toml::Value::Integer(n)) => Some(n.to_string()),
            (Accepts::Bool, toml::Value::Boolean(b)) => Some(b.to_string()),
            (Accepts::Choice(_), toml::Value::Integer(n)) => Some(n.to_string()),
            _ => None,
        };
        let Some(text) = written.as_deref() else {
            tracing::warn!(path = %path.display(), key = k.id, kind = value.type_str(), "a wheel.toml sets a key to something of the wrong type; it is ignored");
            layer.problems.insert(
                k.id,
                format!(
                    "{} sets {} to a {}, which that key does not take; it is ignored",
                    path.display(),
                    k.id,
                    value.type_str()
                ),
            );
            continue;
        };
        match k.accepts.take(text) {
            Ok(value) => {
                layer.sets.insert(k.id, value);
            }
            Err(why) => {
                tracing::warn!(path = %path.display(), key = k.id, %why, "a wheel.toml sets a key to a value it does not take; it is ignored");
                layer
                    .problems
                    .insert(k.id, format!("{}: {why}", path.display()));
            }
        }
    }
    layer
}

/// The value a dotted key names in `table`, walking one table per segment.
fn at<'a>(table: &'a toml::Table, id: &str) -> Option<&'a toml::Value> {
    let Some((head, rest)) = id.split_once('.') else {
        return table.get(id);
    };
    at(table.get(head)?.as_table()?, rest)
}

/// Write `value` at a dotted key in `table`, making the tables on the way and leaving every other
/// key in them, and every comment around them, as it was. Refused when something on the way is not
/// a table, rather than replacing whatever a person put there.
fn put(table: &mut dyn toml_edit::TableLike, id: &str, value: toml_edit::Value) -> Result<(), String> {
    let Some((head, rest)) = id.split_once('.') else {
        let mut new = value;
        // Replace the value where it stands: inserting would rebuild the key and drop the comment
        // above it, and the new value keeps the old one's trailing comment.
        match table.get_mut(id) {
            Some(item) => {
                if let Some(old) = item.as_value() {
                    *new.decor_mut() = old.decor().clone();
                }
                *item = toml_edit::Item::Value(new);
            }
            None => {
                table.insert(id, toml_edit::Item::Value(new));
            }
        }
        return Ok(());
    };
    if table.get(head).is_none() {
        table.insert(head, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let Some(inner) = table.get_mut(head).and_then(|i| i.as_table_like_mut()) else {
        return Err(format!(
            "{head} in this wheel.toml is not a table, so {id} cannot be written under it"
        ));
    };
    put(inner, rest, value)
}

impl AppState {
    /// The machine-wide file: beside the token and the saved layout.
    fn machine_file(&self) -> PathBuf {
        self.instance_dir.join(FILE)
    }

    /// The id of the space `space` names, else the one being shown; an error when there is none.
    pub(crate) fn settings_space_id(&self, space: Option<&str>) -> Result<String, String> {
        self.settings_space(space)?
            .map(|(id, _)| id)
            .ok_or_else(|| "no space is open to run in".into())
    }

    /// The `[runner]` entries: this machine's file under the space's, name by name, in name
    /// order. Any key is a name; its value is the shell command. An entry that is not a string is
    /// skipped and logged.
    pub fn runner_ls(&self, space: Option<&str>) -> Result<Vec<wheel_proto::RunnerInfo>, String> {
        let mut found: BTreeMap<String, wheel_proto::RunnerInfo> = BTreeMap::new();
        let mut read = |path: PathBuf, origin: SettingOrigin| {
            for (name, command) in runners(&path) {
                found.insert(
                    name.clone(),
                    wheel_proto::RunnerInfo {
                        name,
                        command,
                        origin,
                    },
                );
            }
        };
        read(self.machine_file(), SettingOrigin::Machine);
        if let Some((_, cwd)) = self.settings_space(space)? {
            read(Path::new(&cwd).join(FILE), SettingOrigin::Project);
        }
        Ok(found.into_values().collect())
    }

    /// Which space a settings call resolves its project layer against, and that space's directory:
    /// the one named, else the space being shown, else none at all — a machine-wide view, which is
    /// what a client with no space open has to show. A named space that does not exist is refused
    /// rather than quietly answered with the machine view, because a mistyped id must not look
    /// like a project that sets nothing.
    fn settings_space(&self, space: Option<&str>) -> Result<Option<(String, String)>, String> {
        let spaces = self.spaces.lock().unwrap();
        let id = match space {
            Some(id) => id.to_string(),
            None => match spaces.active.clone() {
                Some(id) => id,
                None => return Ok(None),
            },
        };
        spaces
            .spaces
            .iter()
            .find(|s| s.id == id)
            .map(|s| Some((s.id.clone(), s.cwd.clone())))
            .ok_or(format!("no space {id}"))
    }

    /// Every setting, resolved for one space — its project file over the machine file over the
    /// built-in default — in key order, and **which space that was**: the one named, the space
    /// being shown when none was named, or `None` when there is no space at all. The caller is
    /// told rather than assuming, so a reply and a broadcast cannot label a list with a space it
    /// was not resolved for.
    pub fn settings_ls(
        &self,
        space: Option<&str>,
    ) -> Result<(Option<String>, Vec<SettingInfo>), String> {
        let resolved = self.settings_space(space)?;
        let machine = file(&self.machine_file());
        let project = match &resolved {
            Some((_, cwd)) => file(&Path::new(cwd).join(FILE)),
            None => Layer::default(),
        };
        let settings = KEYS
            .iter()
            .map(|k| {
                let project_sets = (!machine_only(k.id))
                    .then(|| project.sets.get(k.id))
                    .flatten();
                // Roots have no fixed default: it follows the recent spaces.
                let default = match k.id {
                    SPACES_ROOTS => self.default_roots(),
                    _ => k.default.to_string(),
                };
                let (value, origin) = match (project_sets, machine.sets.get(k.id)) {
                    (Some(v), _) => (v.clone(), SettingOrigin::Project),
                    (None, Some(v)) => (v.clone(), SettingOrigin::Machine),
                    (None, None) => (default.clone(), SettingOrigin::Default),
                };
                // The project's problem first: it is the layer that would have won, so it is the
                // one a person is surprised by.
                let problem = project
                    .problems
                    .get(k.id)
                    .filter(|_| !machine_only(k.id))
                    .or_else(|| machine.problems.get(k.id))
                    .cloned();
                SettingInfo {
                    key: k.id.to_string(),
                    value,
                    origin,
                    default,
                    help: k.help.to_string(),
                    machine: machine.sets.get(k.id).cloned(),
                    project: project_sets.cloned(),
                    machine_only: machine_only(k.id),
                    problem,
                    kind: k.accepts.kind(),
                    choices: k.accepts.choices(),
                    min: k.accepts.bounds().map(|b| b.0),
                    max: k.accepts.bounds().map(|b| b.1),
                }
            })
            .collect();
        Ok((resolved.map(|(id, _)| id), settings))
    }

    /// The shell a new terminal in the directory `cwd` starts: that directory's `wheel.toml` over
    /// the machine's, `None` when neither sets one and the login shell is meant. Takes the
    /// directory rather than a space so a restore, which opens terminals before its spaces are
    /// installed, resolves it the same way a new pane does.
    pub(crate) fn shell_in(&self, cwd: &str) -> Option<String> {
        let project = file(&Path::new(cwd).join(FILE));
        let machine = file(&self.machine_file());
        project
            .sets
            .get(SHELL)
            .or_else(|| machine.sets.get(SHELL))
            .cloned()
    }

    /// One setting's value in force for `space`, for the thing that uses it. An error only when
    /// the space does not resolve; a key this build does not have is a programming error here, not
    /// a person's, so it is stated as one.
    pub(crate) fn setting(&self, id: &str, space: Option<&str>) -> Result<String, String> {
        let (_, settings) = self.settings_ls(space)?;
        Ok(settings
            .into_iter()
            .find(|s| s.key == id)
            .unwrap_or_else(|| panic!("{id} is not one of wheel's settings"))
            .value)
    }

    /// Every agent wheel has, in name order, with its command from this machine's settings and
    /// where that program is: the table `status` lists. Read from the settings file each time, so
    /// a command a person changes is in force at the next start. `skills` is empty: scanning the
    /// agents' homes is `agent_status`'s work, so daemon start, a spawn and a setting change do
    /// not read every `SKILL.md`.
    pub fn agent_table(&self) -> Vec<wheel_proto::AgentInfo> {
        self.agents(None)
    }

    /// `agent_table` with each agent's skills scanned from its home and the open spaces' directories, for `status`.
    pub fn agent_status(&self, project_dirs: &[PathBuf]) -> Vec<wheel_proto::AgentInfo> {
        self.agents(Some(project_dirs))
    }

    fn agents(&self, scan_skills_in: Option<&[PathBuf]>) -> Vec<wheel_proto::AgentInfo> {
        let machine = file(&self.machine_file());
        agent_names()
            .into_iter()
            .map(|name| {
                let key = format!("agent.{name}.command");
                let command = machine.sets.get(key.as_str()).cloned().unwrap_or_else(|| {
                    KEYS.iter()
                        .find(|k| k.id == key)
                        .map(|k| k.default.to_string())
                        .unwrap_or_default()
                });
                let found = which(&command, &self.path());
                wheel_proto::AgentInfo {
                    name: name.to_string(),
                    command,
                    home: self.agent_home(name).map(|h| h.display().to_string()),
                    path: found.as_ref().ok().map(|p| p.display().to_string()),
                    found: found.is_ok(),
                    why: found.err(),
                    skills: scan_skills_in
                        .map(|dirs| self.agent_skills(name, dirs))
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The skills in the agent's home: the `agent.<name>.home` setting, else the agent's own
    /// default, plus the open spaces' project skills. None for an agent wheel has no default home for.
    fn agent_skills(&self, name: &str, project_dirs: &[PathBuf]) -> Vec<wheel_proto::SkillInfo> {
        let home = self
            .agent_home(name)
            .map(|h| PathBuf::from(crate::dirs::expand_home(&h.to_string_lossy())))
            .or_else(|| crate::resume::Agent::parse(name)?.default_home());
        // Only Claude Code reads `.claude/skills` of a project.
        let projects = if name == "claude" { project_dirs } else { &[] };
        let mut found = home
            .map(|h| crate::skills::scan(&h, projects))
            .unwrap_or_default();
        if name == "claude" {
            for command in crate::skills::claude_builtins() {
                if !found.iter().any(|s| s.name == command.name) {
                    found.push(command);
                }
            }
            found.sort_by(|a, b| a.name.cmp(&b.name));
        }
        found
    }

    /// The configuration directory `agent.<name>.home` sets, when it is set: what the agent's
    /// panes run with unless a pane saved one of its own. None leaves the agent its own default.
    pub(crate) fn agent_home(&self, name: &str) -> Option<PathBuf> {
        let machine = file(&self.machine_file());
        machine
            .sets
            .get(format!("agent.{name}.home").as_str())
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
    }

    /// Log each agent with where its program resolved, as the daemon starts and after every
    /// change to an agent setting.
    pub fn log_agents(&self) {
        for agent in self.agent_table() {
            tracing::info!(agent = %agent.name, command = %agent.command, home = ?agent.home, path = ?agent.path, found = agent.found, why = ?agent.why, "agent resolved");
        }
    }

    /// The agent a new agent pane runs when nobody named one.
    pub fn agent_default(&self) -> String {
        let machine = file(&self.machine_file());
        machine
            .sets
            .get(AGENT_DEFAULT)
            .cloned()
            .unwrap_or_else(|| "claude".into())
    }

    /// The address the remote listener binds, when `remote.listen` is set. Read from this machine's
    /// file as the daemon starts.
    pub fn remote_listen(&self) -> Option<String> {
        file(&self.machine_file())
            .sets
            .get(REMOTE_LISTEN)
            .filter(|l| !l.is_empty())
            .cloned()
    }

    /// The named agent as `agent_table` lists it, or why it cannot be run: no such agent, or its
    /// program was not found.
    pub(crate) fn agent_ready(&self, name: &str) -> Result<wheel_proto::AgentInfo, String> {
        let table = self.agent_table();
        let info = table.into_iter().find(|a| a.name == name).ok_or_else(|| {
            format!(
                "no agent {name}: wheel's agents are {}",
                agent_names().join(", ")
            )
        })?;
        if let Some(why) = &info.why {
            return Err(format!("agent {name} cannot start: {why}"));
        }
        Ok(info)
    }

    /// Write one setting into the machine-wide file, or into `space`'s own, and give back every
    /// setting as it now resolves for that space.
    ///
    /// The file keeps every other key it held: it is read, the one key replaced, and the whole
    /// document written back through a temporary file and a rename, so a crash mid-write leaves the
    /// file that was there. Comments, blank lines and key order in a hand-written file survive.
    pub fn setting_set(
        &self,
        id: &str,
        value: &str,
        space: Option<&str>,
        machine: bool,
    ) -> Result<(Option<String>, Vec<SettingInfo>), String> {
        let k = key(id)?;
        if machine_only(id) && !machine {
            return Err(format!(
                "{id} belongs to this machine: write it with --machine"
            ));
        }
        let value = k.accepts.take(value)?;
        let (path, inside) = self.settings_file(space, machine)?;
        {
            // One writer at a time, for both files: the write is a read, a change and a rename, and
            // two of them interleaved would drop whichever key the loser read before the winner
            // wrote.
            let _one = self.settings_write.lock().unwrap();
            rewrite(&path, inside.as_deref(), k.id, |table| {
                put(table.as_table_mut(), k.id, k.accepts.toml(&value)).map(|()| true)
            })?;
        }
        tracing::info!(key = %k.id, %value, path = %path.display(), machine, "setting written");
        if k.id.starts_with("agent.") {
            self.log_agents();
        }
        // Resolved for the space in context, machine write included, so the caller and every
        // other client are told what is in force there rather than what was just written — a
        // project value still wins over a machine write, and this is where that is visible.
        self.settings_ls(space)
    }

    /// Remove one setting from the machine-wide file or `space`'s own, so the layer below it is
    /// in force, and give back every setting as it now resolves for that space. A file that does
    /// not set the key, or does not exist, is left as it is; the rest of a file that does is kept,
    /// as a write keeps it, and a table the removal leaves empty goes with it.
    pub fn setting_unset(
        &self,
        id: &str,
        space: Option<&str>,
        machine: bool,
    ) -> Result<(Option<String>, Vec<SettingInfo>), String> {
        let k = key(id)?;
        let (path, inside) = self.settings_file(space, machine)?;
        let removed = {
            let _one = self.settings_write.lock().unwrap();
            path.exists()
                && rewrite(&path, inside.as_deref(), k.id, |table| {
                    Ok(unput(table.as_table_mut(), k.id))
                })?
        };
        tracing::info!(key = %k.id, path = %path.display(), machine, removed, "setting removed");
        if k.id.starts_with("agent.") {
            self.log_agents();
        }
        self.settings_ls(space)
    }

    /// The file a write or a removal changes: this machine's, or the `wheel.toml` of the space
    /// named, else of the space being shown. With no space at all only this machine's can be.
    ///
    /// The space is resolved even for this machine's file: the change is answered with the list
    /// resolved for it, so a space that does not exist has to refuse before the file changes, not
    /// after.
    ///
    /// With the file comes the directory it may be written inside: the space's directory for a
    /// project file, none for this machine's.
    fn settings_file(
        &self,
        space: Option<&str>,
        machine: bool,
    ) -> Result<(PathBuf, Option<PathBuf>), String> {
        let resolved = self.settings_space(space)?;
        if machine {
            return Ok((self.machine_file(), None));
        }
        match resolved {
            Some((_, cwd)) => Ok((Path::new(&cwd).join(FILE), Some(PathBuf::from(cwd)))),
            None => Err(
                "no space to resolve a project setting against; make one with `wheel space new <dir>`, name one with --space, or write this machine's file with --machine"
                    .into(),
            ),
        }
    }
}

/// The absolute path of the program a command line starts: its first word as written when that has
/// a directory in it, else the first directory on `path` that holds an executable file of that
/// name. Why not, when it is nowhere.
fn which(command: &str, path: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let runs = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    let Some(program) = crate::claude::words(command).into_iter().next() else {
        return Err("the command is empty".into());
    };
    if program.contains('/') {
        let path = PathBuf::from(&program);
        return match runs(&path) {
            true => Ok(path),
            false => Err(format!("{program} is not an executable file")),
        };
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(&program))
        .find(|p| runs(p))
        .ok_or_else(|| format!("{program} is not on PATH"))
}

/// Remove a dotted key from `table`, and every table on its way that is left empty. Whether
/// there was a key to remove; a parent that is not a table holds no such key.
fn unput(table: &mut dyn toml_edit::TableLike, id: &str) -> bool {
    let Some((head, rest)) = id.split_once('.') else {
        return table.remove(id).is_some();
    };
    let Some(inner) = table.get_mut(head).and_then(|i| i.as_table_like_mut()) else {
        return false;
    };
    let removed = unput(inner, rest);
    if removed && inner.is_empty() {
        table.remove(head);
    }
    removed
}

/// Change the TOML file at `path` with `change`, which says whether it changed anything; a change
/// that changed nothing writes nothing. Every other key in the file is left as it was. Whether the
/// file was written.
///
/// A file that cannot be parsed refuses the change rather than having its keys replaced.
/// The new text goes to a temporary file in the same directory, is flushed to disk, and is renamed
/// over the target, so a crash leaves either the old file or the new one and never half of each;
/// the temporary name carries the process and the key so two writers never share one.
///
/// A `wheel.toml` that is a symlink is written *through*, so a person who keeps their
/// configuration elsewhere and links to it keeps the link. `inside` bounds that: a project file
/// passes its space's directory and is refused when its real path is outside it, so a cloned
/// project whose `wheel.toml` links elsewhere cannot make Wheel write there (reads still follow
/// the link). Only this machine's file passes `None` and writes through any link.
///
/// The file is edited in place as a document, so a comment, a blank line and the order of keys a
/// person wrote by hand survive the change.
pub(crate) fn rewrite(
    path: &Path,
    inside: Option<&Path>,
    id: &str,
    change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<bool, String>,
) -> Result<bool, String> {
    let real = real_path(path);
    if let Some(root) = inside {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !real.starts_with(&root) {
            return Err(format!(
                "{} links to {}, outside {}; Wheel writes only inside the project",
                path.display(),
                real.display(),
                root.display()
            ));
        }
    }
    let path = &real;
    let mut doc: toml_edit::DocumentMut = match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    if !change(&mut doc)? {
        return Ok(false);
    }
    let text = doc.to_string();
    let tmp = path.with_file_name(format!(
        "{FILE}.{}.{}.tmp",
        std::process::id(),
        id.replace('.', "-")
    ));
    let flush = |tmp: &Path| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = flush(&tmp) {
        // Nothing is left in a person's project directory by a write that failed.
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", tmp.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", path.display()));
    }
    Ok(true)
}

/// Where `path` really is: resolved when it exists, else its parent resolved with the name
/// joined on, so a file not written yet is judged by the directory it would land in.
fn real_path(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (
        path.parent().and_then(|p| p.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runners_reads_any_string_key_and_skips_the_rest() {
        let dir = std::env::temp_dir().join(format!("wheel-runner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wheel.toml");
        std::fs::write(
            &path,
            "[runner]\nship = \"make deploy\"\n\"build:css\" = \"npm run build:css\"\nbad = 3\nblank = \"  \"\n",
        )
        .unwrap();
        assert_eq!(
            super::runners(&path),
            [
                ("build:css".to_string(), "npm run build:css".to_string()),
                ("ship".to_string(), "make deploy".to_string())
            ]
        );
        assert!(super::runners(&dir.join("missing.toml")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use super::*;

    /// A dotted key reaches into nested tables, and writing one leaves everything else in place.
    #[test]
    fn a_dotted_key_is_read_and_written_without_disturbing_the_file() {
        let mut doc: toml_edit::DocumentMut =
            "other = 1\n[browser]\nhomepage = \"https://one.example\"\nzoom = 2\n"
                .parse()
                .unwrap();
        let read =
            |doc: &toml_edit::DocumentMut| -> toml::Table { doc.to_string().parse().unwrap() };
        let table = read(&doc);
        assert_eq!(
            at(&table, "browser.homepage").and_then(|v| v.as_str()),
            Some("https://one.example")
        );
        assert!(at(&table, "browser.nothing").is_none());
        assert!(at(&table, "other.homepage").is_none(), "not a table");

        put(
            doc.as_table_mut(),
            "browser.homepage",
            "https://two.example".into(),
        )
        .unwrap();
        let table = read(&doc);
        assert_eq!(
            at(&table, "browser.homepage").and_then(|v| v.as_str()),
            Some("https://two.example")
        );
        assert_eq!(
            at(&table, "browser.zoom").and_then(|v| v.as_integer()),
            Some(2)
        );
        assert_eq!(at(&table, "other").and_then(|v| v.as_integer()), Some(1));

        // A key whose parent is not a table is refused rather than replacing what is there.
        assert!(put(doc.as_table_mut(), "other.homepage", "x".into()).is_err());
        assert_eq!(
            at(&read(&doc), "other").and_then(|v| v.as_integer()),
            Some(1)
        );

        // A key nothing has written yet makes its tables on the way.
        let mut fresh = toml_edit::DocumentMut::new();
        put(
            fresh.as_table_mut(),
            "browser.homepage",
            "about:blank".into(),
        )
        .unwrap();
        assert_eq!(
            at(&read(&fresh), "browser.homepage").and_then(|v| v.as_str()),
            Some("about:blank")
        );
    }

    /// A write changes the one key and leaves a hand-written file's comments, blank line and key
    /// order alone, through a temporary file that is renamed away.
    #[test]
    fn a_write_keeps_a_hand_written_files_comments_and_order() {
        let dir = std::env::temp_dir().join(format!("wheel-comment-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let before = "# my settings\nzzz = 1\n\n[browser]\n# where new panes go\nhomepage = \"about:blank\" # keep\naaa = 2\n";
        std::fs::write(&path, before).unwrap();

        let wrote = rewrite(&path, None, HOMEPAGE, |doc| {
            put(doc.as_table_mut(), HOMEPAGE, "https://two.example".into()).map(|()| true)
        })
        .unwrap();
        assert!(wrote);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before.replace("\"about:blank\"", "\"https://two.example\"")
        );
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            left,
            [std::ffi::OsString::from(FILE)],
            "a temp file was left behind"
        );

        let removed = rewrite(&path, None, HOMEPAGE, |doc| {
            Ok(unput(doc.as_table_mut(), HOMEPAGE))
        })
        .unwrap();
        assert!(removed);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("# my settings")
                && after.contains("zzz = 1")
                && after.contains("aaa = 2"),
            "{after}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a value has to look like, and what the refusal says.
    #[test]
    fn a_page_setting_takes_a_url_a_file_or_about_blank() {
        for good in [
            "https://example.com",
            "http://127.0.0.1:8080/x",
            "file:///Users/me/notes/index.html",
            "file://localhost/tmp/a%20b.html",
            "about:blank",
            "  https://trimmed.example  ",
        ] {
            assert_eq!(
                Accepts::Page.take(good).unwrap(),
                good.trim(),
                "{good:?} is a page"
            );
        }
        // A scheme is case-insensitive, as schemes are.
        for good in ["HTTPS://Example.com", "HtTp://host/x"] {
            assert_eq!(
                Accepts::Page.take(good).unwrap(),
                good,
                "{good:?} is a page, stored as typed"
            );
        }
        for bad in [
            "example.com",
            // A file on another host, or with no absolute path, is no file on this machine.
            "file://otherhost/etc/passwd",
            "file:relative.html",
            "file:///",
            "data:text/html,<p>x",
            "javascript:alert(1)",
            "",
            "about:config",
            // No host: only a port, only userinfo, or neither.
            "http://:80",
            "https://@/",
            "http://user@",
            "https://",
            // Whitespace means it was never a URL.
            "https://a b/c",
            "https://host/a b",
        ] {
            let e = Accepts::Page.take(bad).unwrap_err();
            assert!(e.contains("http://"), "{bad:?}: {e}");
        }
    }

    /// A key wheel does not have is refused, and the refusal names the ones it does.
    #[test]
    fn an_unknown_key_names_the_keys_there_are() {
        assert!(key("browser.homepage").map(|k| k.id).is_ok());
        let e = key("browser.homepge").map(|k| k.id).unwrap_err();
        assert!(e.contains("browser.homepage"), "{e}");
    }

    /// The agents are the `agent.<name>.command` keys, and the default must be one of them.
    #[test]
    fn the_default_agent_is_one_of_the_agents() {
        assert_eq!(agent_names(), ["claude", "codex", "pi"]);
        assert_eq!(Accepts::Agent.take(" codex ").unwrap(), "codex");
        let e = Accepts::Agent.take("gemini").unwrap_err();
        assert!(e.contains("claude, codex, pi"), "{e}");
        assert!(Accepts::Command.take("  ").is_err());
        assert_eq!(
            Accepts::Command
                .take(" wheel-fake-agent --replay x ")
                .unwrap(),
            "wheel-fake-agent --replay x"
        );
    }
}
