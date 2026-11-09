//! The `[runner]` tables of the machine's and the project's `wheel.toml` (ADR-0025): the runner
//! ships no commands of its own, it runs what a project wrote out in full.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One `[runner]` entry: a command string, or a table with `command` and an optional `confirm`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Runner {
    /// The identifier: the key for a top-level entry, the dotted path (`prototypes.terrarium.dev`) for a nested one.
    pub name: String,
    /// What the tree shows: the entry's `name` when it has one, else its last key.
    pub label: String,
    /// The groups above the entry, outermost first: the keys of the tables it sits in.
    pub group: Vec<String>,
    /// One sentence on what the command does.
    pub desc: Option<String>,
    pub command: String,
    /// The question to ask before the command runs.
    pub confirm: Option<String>,
    /// `machine` or `project`: the file the entry is from; the project's wins over the machine's.
    pub origin: &'static str,
    /// Guard names that must all pass before the command starts: `git-clean`, `git-pushed`, or a
    /// `[guards.<name>]` table of the machine's or the project's `wheel.toml`.
    pub requires: Vec<String>,
    /// A readout of the thing the command runs or deploys: shown by `wheel-runner check`.
    pub status: Option<StatusCheck>,
    /// The command may run in another git worktree of the project (`--worktree <path>`).
    pub worktree: bool,
}

/// One per-command status check.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StatusCheck {
    /// Up when `url` answers 2xx or 3xx.
    Http { url: String },
    /// Up when `url` answers 2xx or 3xx with JSON; the text is the value at the dotted `pull` path.
    Json { url: String, pull: String },
    /// Up when `run` exits 0; the text is the first line it prints.
    Command { run: String },
}

/// A user-defined guard: `run` is a shell expression, `fail` the message when it exits non-zero.
#[derive(Clone, Debug, PartialEq)]
pub struct Guard {
    pub run: String,
    pub fail: String,
}

/// The machine's `wheel.toml`: `$WHEEL_MACHINE_FILE`, else `~/.wheel/wheel.toml`.
pub fn machine_file() -> PathBuf {
    match std::env::var_os("WHEEL_MACHINE_FILE") {
        Some(p) => PathBuf::from(p),
        None => home().join(".wheel").join("wheel.toml"),
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The entries of one file, in file order. A table with a `command` is an entry; any other table is a
/// group and its tables and strings are entries one level down; a string is an entry whose command it
/// is. A table holding `confirm`, `requires`, `worktree`, `status`, `desc` or `name` but no `command` is skipped. A file that is missing or not TOML names none; a value that is neither is skipped.
pub fn read(path: &Path, origin: &'static str) -> Vec<Runner> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return vec![] };
    let Some(table) = doc.get("runner").and_then(|r| r.as_table_like()) else { return vec![] };
    let mut out = vec![];
    walk(table, &[], origin, &mut out);
    out
}

fn text_of(v: Option<&toml_edit::Item>) -> Option<String> {
    v.and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(String::from)
}

fn walk(table: &dyn toml_edit::TableLike, group: &[String], origin: &'static str, out: &mut Vec<Runner>) {
    for (key, item) in table.iter() {
        let name = group.iter().map(String::as_str).chain([key]).collect::<Vec<_>>().join(".");
        let Some(t) = item.as_table_like() else {
            if let Some(command) = text_of(Some(item)) {
                out.push(Runner {
                    name, label: key.to_string(), group: group.to_vec(), desc: None, command, confirm: None, origin,
                    requires: vec![], status: None, worktree: false,
                });
            }
            continue;
        };
        let Some(command) = text_of(t.get("command")) else {
            // A table holding a command's own fields but no command is a command that lost it, not a group.
            if ["confirm", "requires", "worktree", "status", "desc", "name"].iter().any(|k| t.contains_key(k)) {
                continue;
            }
            let mut inner = group.to_vec();
            inner.push(key.to_string());
            walk(t, &inner, origin, out);
            continue;
        };
        let requires = match t.get("requires") {
            None => vec![],
            Some(r) => match r.as_array().map(|a| a.iter().map(|v| v.as_str().map(String::from)).collect::<Option<Vec<_>>>()) {
                Some(Some(names)) => names,
                _ => vec![crate::guards::INVALID.to_string()],
            },
        };
        out.push(Runner {
            label: text_of(t.get("name")).unwrap_or_else(|| key.to_string()),
            group: group.to_vec(),
            desc: text_of(t.get("desc")),
            name, command, confirm: text_of(t.get("confirm")), origin, requires,
            status: t.get("status").and_then(|s| s.as_table_like()).and_then(|s| status_check(s, &text_of)),
            worktree: t.get("worktree").and_then(|v| v.as_bool()).unwrap_or(false),
        });
    }
}

fn status_check(t: &dyn toml_edit::TableLike, text_of: &dyn Fn(Option<&toml_edit::Item>) -> Option<String>) -> Option<StatusCheck> {
    let get = |k: &str| text_of(t.get(k));
    match get("kind")?.as_str() {
        "http" => Some(StatusCheck::Http { url: get("url")? }),
        "json" => Some(StatusCheck::Json { url: get("url")?, pull: get("pull")? }),
        "command" => Some(StatusCheck::Command { run: get("run")? }),
        _ => None,
    }
}

/// The `[guards.<name>]` tables of the machine's and the project's `wheel.toml`, the project's over
/// the machine's. A table without `run` is skipped.
pub fn guards(machine: &Path, dir: &str) -> std::collections::BTreeMap<String, Guard> {
    let mut found = std::collections::BTreeMap::new();
    for path in [machine.to_path_buf(), Path::new(dir).join("wheel.toml")] {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { continue };
        let Some(table) = doc.get("guards").and_then(|g| g.as_table_like()) else { continue };
        for (name, item) in table.iter() {
            let Some(t) = item.as_table_like() else { continue };
            let Some(run) = t.get("run").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) else { continue };
            let fail = t.get("fail").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| format!("{name} failed"));
            found.insert(name.to_string(), Guard { run: run.into(), fail });
        }
    }
    found
}

/// The machine's entries, then the project's, in file order; a project entry replaces the machine
/// entry of the same name where that one stood.
pub fn list(machine: &Path, dir: &str) -> Vec<Runner> {
    let mut found: Vec<Runner> = vec![];
    for r in read(machine, "machine").into_iter().chain(read(&Path::new(dir).join("wheel.toml"), "project")) {
        match found.iter().position(|f| f.name == r.name) {
            Some(i) => found[i] = r,
            None => found.push(r),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-runner-config-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entries_are_strings_or_tables_and_the_project_wins_in_place() {
        let dir = scratch("flat");
        let machine = dir.join("machine.toml");
        std::fs::write(&machine, "[runner]\nonly-machine = \"m\"\nship = \"machine ship\"\n").unwrap();
        std::fs::write(
            dir.join("wheel.toml"),
            "[runner]\nship = \"make deploy\"\nrestart = { command = \"make restart\", confirm = \"Ends every agent.\" }\nbad = 3\nblank = \"  \"\nnone = { confirm = \"x\" }\n",
        )
        .unwrap();
        let found = list(&machine, dir.to_str().unwrap());
        let summary: Vec<(&str, &str, Option<&str>, &str)> =
            found.iter().map(|r| (r.name.as_str(), r.command.as_str(), r.confirm.as_deref(), r.origin)).collect();
        assert_eq!(
            summary,
            [
                ("only-machine", "m", None, "machine"),
                ("ship", "make deploy", None, "project"),
                ("restart", "make restart", Some("Ends every agent."), "project"),
            ]
        );
        assert!(list(&dir.join("missing.toml"), "/no/such/dir").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_table_without_a_command_is_a_group_and_the_dotted_path_names_the_entry() {
        let dir = scratch("nested");
        std::fs::write(
            dir.join("wheel.toml"),
            "[runner.wheel.dev.deploy]\nname = \"Deploy\"\ncommand = \"make deploy-dev\"\ndesc = \"Builds and installs Wheel Dev.app\"\n\n\
             [runner.prototypes.terrarium]\nbuild = \"bun run build\"\ndev = { command = \"bun run dev\" }\n\n[runner.machine]\nhello = \"echo hi\"\n",
        )
        .unwrap();
        let found = list(&dir.join("none.toml"), dir.to_str().unwrap());
        let summary: Vec<(&str, &str, Vec<&str>, Option<&str>)> =
            found.iter().map(|r| (r.name.as_str(), r.label.as_str(), r.group.iter().map(String::as_str).collect(), r.desc.as_deref())).collect();
        assert_eq!(
            summary,
            [
                ("wheel.dev.deploy", "Deploy", vec!["wheel", "dev"], Some("Builds and installs Wheel Dev.app")),
                ("prototypes.terrarium.build", "build", vec!["prototypes", "terrarium"], None),
                ("prototypes.terrarium.dev", "dev", vec!["prototypes", "terrarium"], None),
                ("machine.hello", "hello", vec!["machine"], None),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
