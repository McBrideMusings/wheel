//! The `[runner]` tables of the machine's and the project's `wheel.toml` (ADR-0025): the runner
//! ships no commands of its own, it runs what a project wrote out in full.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One `[runner]` entry: a command string, or a table with `command` and an optional `confirm`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Runner {
    pub name: String,
    pub command: String,
    /// The question to ask before the command runs.
    pub confirm: Option<String>,
    /// `machine` or `project`: the file the entry is from; the project's wins over the machine's.
    pub origin: &'static str,
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

/// The entries of one file. A file that is missing or not TOML names none; an entry that is
/// neither a non-empty command nor a table holding one is skipped.
pub fn read(path: &Path, origin: &'static str) -> Vec<Runner> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return vec![] };
    let Some(table) = doc.get("runner").and_then(|r| r.as_table_like()) else { return vec![] };
    let text_of = |v: Option<&toml_edit::Item>| {
        v.and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(String::from)
    };
    table
        .iter()
        .filter_map(|(name, item)| {
            let (command, confirm) = match item.as_table_like() {
                Some(t) => (text_of(t.get("command")), text_of(t.get("confirm"))),
                None => (text_of(Some(item)), None),
            };
            command.map(|command| Runner { name: name.to_string(), command, confirm, origin })
        })
        .collect()
}

/// The machine's entries, then the project's over them, sorted by name.
pub fn list(machine: &Path, dir: &str) -> Vec<Runner> {
    let mut found = std::collections::BTreeMap::new();
    for r in read(machine, "machine").into_iter().chain(read(&Path::new(dir).join("wheel.toml"), "project")) {
        found.insert(r.name.clone(), r);
    }
    found.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_strings_or_tables_and_the_project_wins() {
        let dir = std::env::temp_dir().join(format!("wheel-runner-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let machine = dir.join("machine.toml");
        std::fs::write(&machine, "[runner]\nship = \"machine ship\"\nonly-machine = \"m\"\n").unwrap();
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
                ("restart", "make restart", Some("Ends every agent."), "project"),
                ("ship", "make deploy", None, "project"),
            ]
        );
        assert!(list(&dir.join("missing.toml"), "/no/such/dir").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
