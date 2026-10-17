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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wheel_proto::{SettingInfo, SettingOrigin};

use crate::server::AppState;

/// The file holding settings, in a project's directory and in the instance directory alike.
const FILE: &str = "wheel.toml";

/// What a key's value has to look like. A key wheel cannot check is not a key: every value a
/// person can write is refused here or accepted knowingly, so no setting reaches the thing that
/// uses it as something that thing has to re-check.
#[derive(Clone, Copy)]
enum Accepts {
    /// A page a browser pane can open: `http://…`, `https://…`, `file:///…` or `about:blank`.
    Page,
    /// A command that speaks ACP: it answers `initialize` on its stdout within [`PROBE_WAIT`].
    Agent,
    /// An absolute path to an executable file.
    Executable,
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
            Accepts::Agent => agent_read(value),
            Accepts::Executable => executable(value),
        }
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

/// How long a command gets to answer `initialize` before it is taken not to speak ACP.
const PROBE_WAIT: Duration = Duration::from_secs(5);

/// What a command's last probe found, by command text, so a `wheel.toml` read on every
/// `settings ls` can tell a probed command from one it has not run. Only a `set` probes and records.
fn probed() -> &'static Mutex<HashMap<String, Result<(), String>>> {
    static PROBED: OnceLock<Mutex<HashMap<String, Result<(), String>>>> = OnceLock::new();
    PROBED.get_or_init(Mutex::default)
}

fn agent_nonempty(value: &str) -> Result<&str, String> {
    match value.trim() {
        "" => Err("an agent command cannot be empty".into()),
        v => Ok(v),
    }
}

/// What the `lead.agent` setting may hold when it is *written*: a command that answers an ACP
/// `initialize` request. A free string could name something that does not speak ACP, which would
/// only show as a lead that never starts, so the command is run and asked, and the answer is
/// remembered. This is the only place a setting's value is executed: it runs because a person asked
/// for exactly this value to be set.
fn agent_probed(value: &str) -> Result<String, String> {
    let value = agent_nonempty(value)?;
    let found = probe(value);
    probed()
        .lock()
        .unwrap()
        .insert(value.to_string(), found.clone());
    found.map(|()| value.to_string())
}

/// What the `lead.agent` setting may hold when it is *read* out of a file: never run. A
/// `wheel.toml` can arrive with a cloned repository, so a command it names must not start because
/// wheeld looked at it. A command a `set` already probed keeps that answer; any other is accepted
/// unprobed, and [`unprobed`] tells the reader so.
fn agent_read(value: &str) -> Result<String, String> {
    let value = agent_nonempty(value)?;
    match probed().lock().unwrap().get(value) {
        Some(known) => known.clone().map(|()| value.to_string()),
        None => Ok(value.to_string()),
    }
}

/// Whether `value` is an agent command no `set` has probed, so what the daemon reports for it is
/// unchecked.
fn unprobed(value: &str) -> bool {
    !probed().lock().unwrap().contains_key(value)
}

/// Run `command`, send it ACP `initialize`, and wait for its answer. The process is killed
/// either way.
fn probe(command: &str) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::RecvTimeoutError;

    let refused = |why: String| format!("{command:?} does not speak ACP: {why}");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("exec {command}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| refused(format!("it cannot be run: {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            // An agent may print a banner first; only a JSON-RPC answer to request 1 counts.
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if msg.get("id") == Some(&serde_json::json!(1)) {
                let _ = tx.send(msg);
                break;
            }
        }
    });
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1}
    });
    let outcome = match writeln!(stdin, "{request}").and_then(|()| stdin.flush()) {
        // A command that is gone before the request is written has exited without answering.
        Err(_) => Err(refused(
            "it exited, or closed its output, without answering initialize".into(),
        )),
        Ok(()) => match rx.recv_timeout(PROBE_WAIT) {
            Err(RecvTimeoutError::Timeout) => Err(refused(format!(
                "no answer to initialize within {}s",
                PROBE_WAIT.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(refused(
                "it exited, or closed its output, without answering initialize".into(),
            )),
            Ok(msg) => match msg.get("result") {
                Some(result) if result.get("protocolVersion").is_some() => Ok(()),
                _ => Err(refused(format!("its answer to initialize was {msg}"))),
            },
        },
    };
    let _ = child.kill();
    let _ = child.wait();
    outcome
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

/// The key naming the command that runs the lead.
pub(crate) const LEAD_AGENT: &str = "lead.agent";
/// The key naming the shell a new terminal pane starts.
pub(crate) const SHELL: &str = "terminal.shell";

/// The key naming the `bd` the lead's bead tools run.
pub(crate) const BD: &str = "beads.bd";

/// The key naming the `claude` whose `auth status` is the lead's login preflight.
pub(crate) const CLAUDE: &str = "claude.path";

/// Every setting wheel has. A key absent here does not exist: a `get` or a `set` naming one is
/// refused, naming the keys that do exist. A key in a `wheel.toml` that is not one of these is a
/// key wheel never reads, which `wheel config ls` cannot report because it reports settings, not
/// file contents — so a typo in a hand-written file is silent, and finding it is reading the file.
const KEYS: [Key; 5] = [
    Key {
        id: HOMEPAGE,
        accepts: Accepts::Page,
        default: "about:blank",
        help: "The page a browser pane opens on when nobody gave it a URL.",
    },
    Key {
        id: LEAD_AGENT,
        accepts: Accepts::Agent,
        default: "claude-agent-acp",
        help: "The command wheeld runs as the lead; it must speak ACP.",
    },
    Key {
        id: SHELL,
        accepts: Accepts::Executable,
        default: "",
        help: "Absolute path of the shell a new terminal pane starts; empty means the login shell. A shell set here starts as a plain shell, not a login shell.",
    },
    Key {
        id: BD,
        accepts: Accepts::Executable,
        default: "",
        help: "Absolute path of the bd the lead's bead tools run; empty means the first bd on wheeld's PATH.",
    },
    Key {
        id: CLAUDE,
        accepts: Accepts::Executable,
        default: "",
        help: "Absolute path of the claude whose `auth status` checks the lead's login; empty means the first claude on wheeld's PATH.",
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
        let Some(text) = value.as_str() else {
            tracing::warn!(path = %path.display(), key = k.id, kind = value.type_str(), "a wheel.toml sets a key to something that is not a string; it is ignored");
            layer.problems.insert(
                k.id,
                format!(
                    "{} sets {} to a {}, not a string; it is ignored",
                    path.display(),
                    k.id,
                    value.type_str()
                ),
            );
            continue;
        };
        match k.accepts.take(text) {
            Ok(value) => {
                if matches!(k.accepts, Accepts::Agent) && unprobed(&value) {
                    tracing::debug!(path = %path.display(), key = k.id, %value, "a wheel.toml names an agent command; not run, so unchecked");
                    layer.problems.insert(
                        k.id,
                        format!(
                            "{}: {value:?} is unchecked: wheeld does not run a command it only read from a file. `wheel config set {} <command>` probes it",
                            path.display(),
                            k.id
                        ),
                    );
                }
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
fn put(table: &mut dyn toml_edit::TableLike, id: &str, value: String) -> Result<(), String> {
    let Some((head, rest)) = id.split_once('.') else {
        let mut new = toml_edit::Value::from(value);
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
                let (value, origin) = match (project.sets.get(k.id), machine.sets.get(k.id)) {
                    (Some(v), _) => (v.clone(), SettingOrigin::Project),
                    (None, Some(v)) => (v.clone(), SettingOrigin::Machine),
                    (None, None) => (k.default.to_string(), SettingOrigin::Default),
                };
                // The project's problem first: it is the layer that would have won, so it is the
                // one a person is surprised by.
                let problem = project
                    .problems
                    .get(k.id)
                    .or_else(|| machine.problems.get(k.id))
                    .cloned();
                SettingInfo {
                    key: k.id.to_string(),
                    value,
                    origin,
                    default: k.default.to_string(),
                    help: k.help.to_string(),
                    machine: machine.sets.get(k.id).cloned(),
                    project: project.sets.get(k.id).cloned(),
                    problem,
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
        let value = match k.accepts {
            Accepts::Agent => agent_probed(value)?,
            other => other.take(value)?,
        };
        let path = self.settings_file(space, machine)?;
        {
            // One writer at a time, for both files: the write is a read, a change and a rename, and
            // two of them interleaved would drop whichever key the loser read before the winner
            // wrote.
            let _one = self.settings_write.lock().unwrap();
            rewrite(&path, k.id, |table| {
                put(table.as_table_mut(), k.id, value.clone()).map(|()| true)
            })?;
        }
        tracing::info!(key = %k.id, %value, path = %path.display(), machine, "setting written");
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
        let path = self.settings_file(space, machine)?;
        let removed = {
            let _one = self.settings_write.lock().unwrap();
            path.exists() && rewrite(&path, k.id, |table| Ok(unput(table.as_table_mut(), k.id)))?
        };
        tracing::info!(key = %k.id, path = %path.display(), machine, removed, "setting removed");
        self.settings_ls(space)
    }

    /// The file a write or a removal changes: this machine's, or the `wheel.toml` of the space
    /// named, else of the space being shown. With no space at all only this machine's can be.
    ///
    /// The space is resolved even for this machine's file: the change is answered with the list
    /// resolved for it, so a space that does not exist has to refuse before the file changes, not
    /// after.
    fn settings_file(&self, space: Option<&str>, machine: bool) -> Result<PathBuf, String> {
        let resolved = self.settings_space(space)?;
        if machine {
            return Ok(self.machine_file());
        }
        match resolved {
            Some((_, cwd)) => Ok(Path::new(&cwd).join(FILE)),
            None => Err(
                "no space to resolve a project setting against; make one with `wheel space new <dir>`, name one with --space, or write this machine's file with --machine"
                    .into(),
            ),
        }
    }
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
/// the temporary name carries the process and the key so two writers never share one. A `wheel.toml`
/// that is a symlink is written *through*: a person who keeps their configuration elsewhere and
/// links to it keeps the link.
///
/// The file is edited in place as a document, so a comment, a blank line and the order of keys a
/// person wrote by hand survive the change.
fn rewrite(
    path: &Path,
    id: &str,
    change: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<bool, String>,
) -> Result<bool, String> {
    let path = &path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that answers `initialize` is accepted; one that is silent, exits, answers with
    /// something else, or does not exist is refused with the reason.
    #[test]
    fn an_agent_setting_takes_only_a_command_that_speaks_acp() {
        let good = r#"sh -c 'read l; echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}"; sleep 30'"#;
        assert_eq!(agent_probed(good).unwrap(), good);
        for (bad, says) in [
            ("sleep 30", "no answer"),
            ("true", "exited"),
            ("wheel-no-such-command-xyz", "exited"),
            (
                r#"sh -c 'read l; echo "{\"id\":1,\"error\":{}}"; sleep 30'"#,
                "its answer",
            ),
            ("", "empty"),
        ] {
            let e = agent_probed(bad).unwrap_err();
            assert!(e.contains(says), "{bad:?}: {e}");
        }
    }

    /// A `wheel.toml` that names an agent command is read without running it, and the value is
    /// reported as unchecked; a `set` of the same command probes it.
    #[test]
    fn reading_a_file_never_runs_its_agent_command() {
        let dir = std::env::temp_dir().join(format!("wheel-no-run-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("pwned");
        let path = dir.join(FILE);
        std::fs::write(
            &path,
            format!("[lead]\nagent = \"touch {}\"\n", marker.display()),
        )
        .unwrap();
        let layer = file(&path);
        assert_eq!(
            layer.sets.get(LEAD_AGENT).unwrap(),
            &format!("touch {}", marker.display())
        );
        assert!(layer
            .problems
            .get(LEAD_AGENT)
            .unwrap()
            .contains("unchecked"));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "reading the file ran its command");
        let _ = std::fs::remove_dir_all(&dir);
    }

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

        let wrote = rewrite(&path, HOMEPAGE, |doc| {
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

        let removed = rewrite(&path, HOMEPAGE, |doc| {
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
}
