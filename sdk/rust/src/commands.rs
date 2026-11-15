//! A package's `[commands]` table: feeds and actions that are shell commands, served by Wheel's
//! command backend (`wheel-runner commands`) so a package needs no backend of its own
//! (`docs/plugin-runtime.md`, "Command backend"). `wheeld` reads the table to refuse a bad one and
//! to give the package its backend; the backend reads it to run the commands. Both use this parser.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The program `wheeld` runs, in the package folder, for a package with a `[commands]` table.
pub const BACKEND_COMMAND: &str = "wheel-runner commands";

/// How a feed command's standard output becomes the feed's value.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// JSON when the whole output parses as JSON, else its lines.
    #[default]
    Auto,
    /// JSON; output that does not parse is the feed's error.
    Json,
    /// An array of the output's lines.
    Lines,
    /// The output as one string.
    Text,
}

/// `[commands.feeds.<name>]`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    /// Run with `/bin/sh -c`.
    pub run: String,
    /// Seconds between runs while a pane reads the feed; none runs it only on subscribe, after an
    /// action and on a watched change.
    pub every: Option<f64>,
    /// Paths whose change runs the feed again: absolute, `~/…`, or relative to where it runs.
    #[serde(default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub output: Output,
    /// Seconds a run may take before it is killed; default [`FEED_TIMEOUT`].
    pub timeout: Option<f64>,
}

/// `[commands.actions.<name>]`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// Run with `/bin/sh -c`.
    pub run: String,
    /// Seconds a run may take before it is killed; default [`ACTION_TIMEOUT`].
    pub timeout: Option<f64>,
}

pub const FEED_TIMEOUT: f64 = 10.0;
pub const ACTION_TIMEOUT: f64 = 30.0;

/// The whole `[commands]` table.
#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Commands {
    #[serde(default)]
    pub feeds: BTreeMap<String, Feed>,
    #[serde(default)]
    pub actions: BTreeMap<String, Action>,
}

#[derive(Deserialize)]
struct Wrapper {
    commands: Option<toml::Value>,
}

fn plain(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The longest `every` or `timeout` a table may name: one day.
pub const MAX_SECONDS: f64 = 86_400.0;

fn seconds(what: &str, value: Option<f64>, min: f64) -> Result<(), String> {
    match value {
        Some(v) if !(v.is_finite() && (min..=MAX_SECONDS).contains(&v)) => {
            Err(format!("{what} is {v}; it must be from {min} to {MAX_SECONDS} seconds"))
        }
        _ => Ok(()),
    }
}

/// Check a parsed table: plain names, a command in each entry, sane seconds.
pub fn validate(commands: Commands) -> Result<Commands, String> {
    if commands.feeds.is_empty() && commands.actions.is_empty() {
        return Err("[commands] names no feed and no action".into());
    }
    for (name, feed) in &commands.feeds {
        if !plain(name) {
            return Err(format!("[commands.feeds.{name}]: a feed name is letters, digits, _ and -"));
        }
        if feed.run.trim().is_empty() {
            return Err(format!("[commands.feeds.{name}] run is empty"));
        }
        seconds(&format!("[commands.feeds.{name}] every"), feed.every, 0.1)?;
        seconds(&format!("[commands.feeds.{name}] timeout"), feed.timeout, 0.1)?;
    }
    for (name, action) in &commands.actions {
        if !plain(name) {
            return Err(format!("[commands.actions.{name}]: an action name is letters, digits, _ and -"));
        }
        if action.run.trim().is_empty() {
            return Err(format!("[commands.actions.{name}] run is empty"));
        }
        seconds(&format!("[commands.actions.{name}] timeout"), action.timeout, 0.1)?;
    }
    Ok(commands)
}

/// The `[commands]` table of a `wheel-plugin.toml`'s text: None when it has none, an error naming
/// what is wrong when it has a bad one.
pub fn from_manifest(text: &str) -> Result<Option<Commands>, String> {
    let wrapper: Wrapper = toml::from_str(text).map_err(|e| e.to_string())?;
    let Some(table) = wrapper.commands else { return Ok(None) };
    let commands: Commands = table.try_into().map_err(|e: toml::de::Error| format!("[commands]: {}", e.message()))?;
    validate(commands).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_parses_with_its_defaults() {
        let c = from_manifest(
            "[plugin]\nid = \"x\"\nversion = \"1\"\n[commands.feeds.now]\nrun = \"date +%s\"\nevery = 2\n[commands.actions.add]\nrun = \"echo $ARG_NAME\"\n",
        )
        .unwrap()
        .unwrap();
        let now = &c.feeds["now"];
        assert_eq!((now.run.as_str(), now.every, now.output, now.timeout), ("date +%s", Some(2.0), Output::Auto, None));
        assert_eq!(c.actions["add"].run, "echo $ARG_NAME");
    }

    #[test]
    fn no_table_is_none_and_a_bad_one_says_why() {
        assert_eq!(from_manifest("[plugin]\nid = \"x\"\n").unwrap(), None);
        let e = from_manifest("[commands.feeds.now]\nrun = \"x\"\nevry = 2\n").unwrap_err();
        assert!(e.contains("evry"), "{e}");
        let e = from_manifest("[commands.feeds.now]\nrun = \"x\"\noutput = \"yaml\"\n").unwrap_err();
        assert!(e.contains("yaml"), "{e}");
        let e = from_manifest("[commands.feeds.\"a.b\"]\nrun = \"x\"\n").unwrap_err();
        assert!(e.contains("letters"), "{e}");
        let e = from_manifest("[commands.feeds.now]\nrun = \"x\"\nevery = 0\n").unwrap_err();
        assert!(e.contains("every"), "{e}");
        let e = from_manifest("[commands.feeds.now]\nrun = \"x\"\nevery = 1e20\n").unwrap_err();
        assert!(e.contains("86400"), "{e}");
        let e = from_manifest("[commands]\n").unwrap_err();
        assert!(e.contains("no feed"), "{e}");
        let e = from_manifest("[commands.actions.go]\nrun = \"  \"\n").unwrap_err();
        assert!(e.contains("empty"), "{e}");
    }
}
