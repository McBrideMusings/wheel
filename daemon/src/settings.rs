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
const FILE: &str = "wheel.toml";

/// What a key's value has to look like. A key wheel cannot check is not a key: every value a
/// person can write is refused here or accepted knowingly, so no setting reaches the thing that
/// uses it as something that thing has to re-check.
#[derive(Clone, Copy)]
enum Accepts {
    /// A page a browser pane can open: `http://…`, `https://…`, `file:///…` or `about:blank`.
    Page,
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

/// One setting wheel has.
struct Key {
    id: &'static str,
    accepts: Accepts,
    default: &'static str,
    help: &'static str,
}

/// The key a browser pane opens when nobody gave it a URL.
pub(crate) const HOMEPAGE: &str = "browser.homepage";

/// Every setting wheel has. A key absent here does not exist: a `get` or a `set` naming one is
/// refused, naming the keys that do exist. A key in a `wheel.toml` that is not one of these is a
/// key wheel never reads, which `wheel config ls` cannot report because it reports settings, not
/// file contents — so a typo in a hand-written file is silent, and finding it is reading the file.
const KEYS: [Key; 1] = [Key {
    id: HOMEPAGE,
    accepts: Accepts::Page,
    default: "about:blank",
    help: "The page a browser pane opens on when nobody gave it a URL.",
}];

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
/// key in them as it was. Refused when something on the way is not a table, rather than replacing
/// whatever a person put there.
fn put(table: &mut toml::Table, id: &str, value: String) -> Result<(), String> {
    let Some((head, rest)) = id.split_once('.') else {
        table.insert(id.to_string(), toml::Value::String(value));
        return Ok(());
    };
    let entry = table
        .entry(head.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(inner) = entry.as_table_mut() else {
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
    /// table written back through a temporary file and a rename, so a crash mid-write leaves the
    /// file that was there. Comments and key order in a hand-written file do not survive that
    /// round trip; the values do.
    pub fn setting_set(
        &self,
        id: &str,
        value: &str,
        space: Option<&str>,
        machine: bool,
    ) -> Result<(Option<String>, Vec<SettingInfo>), String> {
        let k = key(id)?;
        let value = k.accepts.take(value)?;
        let path = self.settings_file(space, machine)?;
        {
            // One writer at a time, for both files: the write is a read, a change and a rename, and
            // two of them interleaved would drop whichever key the loser read before the winner
            // wrote.
            let _one = self.settings_write.lock().unwrap();
            rewrite(&path, k.id, |table| {
                put(table, k.id, value.clone()).map(|()| true)
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
            path.exists() && rewrite(&path, k.id, |table| Ok(unput(table, k.id)))?
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
fn unput(table: &mut toml::Table, id: &str) -> bool {
    let Some((head, rest)) = id.split_once('.') else {
        return table.remove(id).is_some();
    };
    let Some(inner) = table.get_mut(head).and_then(|v| v.as_table_mut()) else {
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
/// Comments and key order in a hand-written file do not survive the round trip; the values do.
fn rewrite(
    path: &Path,
    id: &str,
    change: impl FnOnce(&mut toml::Table) -> Result<bool, String>,
) -> Result<bool, String> {
    let path = &path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    if !change(&mut table)? {
        return Ok(false);
    }
    let text = toml::to_string_pretty(&table).map_err(|e| format!("write {id}: {e}"))?;
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

    /// A dotted key reaches into nested tables, and writing one leaves everything else in place.
    #[test]
    fn a_dotted_key_is_read_and_written_without_disturbing_the_file() {
        let mut table: toml::Table =
            toml::from_str("other = 1\n[browser]\nhomepage = \"https://one.example\"\nzoom = 2\n")
                .unwrap();
        assert_eq!(
            at(&table, "browser.homepage").and_then(|v| v.as_str()),
            Some("https://one.example")
        );
        assert!(at(&table, "browser.nothing").is_none());
        assert!(at(&table, "other.homepage").is_none(), "not a table");

        put(&mut table, "browser.homepage", "https://two.example".into()).unwrap();
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
        assert!(put(&mut table, "other.homepage", "x".into()).is_err());
        assert_eq!(at(&table, "other").and_then(|v| v.as_integer()), Some(1));

        // A key nothing has written yet makes its tables on the way.
        let mut fresh = toml::Table::new();
        put(&mut fresh, "browser.homepage", "about:blank".into()).unwrap();
        assert_eq!(
            at(&fresh, "browser.homepage").and_then(|v| v.as_str()),
            Some("about:blank")
        );
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
