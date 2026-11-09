//! Title-row buttons (ADR-0036): `[[title.button]]` entries in this machine's `wheel.toml` and the
//! space's, merged by `id`, with the state each entry's `state` command last printed.
//!
//! wheeld parses the entries, runs `state` on its `every` timer, runs `command` on a press, and
//! tells every client when a space's list changes. The client only draws it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;
use wheel_proto::{ServerMsg, SettingOrigin, TitleButtonInfo, TitleSide};

use crate::server::AppState;
use crate::settings::FILE;

/// How often wheeld looks at the files and which `state` commands are due.
const TICK: Duration = Duration::from_secs(1);
/// A `state` command that runs longer than this is killed and counts as printing nothing.
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
/// A `command` run by a press is killed after this long.
const PRESS_TIMEOUT: Duration = Duration::from_secs(60);
/// The most stdout of a `state` or `command` wheeld keeps.
const OUTPUT_CAP: u64 = 64 * 1024;
/// `every` when an entry names a `state` command and no interval.
const DEFAULT_EVERY: u64 = 5;

/// One `[[title.button]]` entry as a file sets it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub id: String,
    pub side: TitleSide,
    pub hidden: bool,
    pub icon: String,
    pub tip: String,
    pub command: Option<String>,
    pub state: Option<String>,
    pub every: u64,
    pub origin: SettingOrigin,
}

#[derive(Deserialize)]
struct Raw {
    id: Option<String>,
    side: Option<String>,
    #[serde(default)]
    hidden: bool,
    icon: Option<String>,
    tip: Option<String>,
    command: Option<String>,
    state: Option<String>,
    every: Option<i64>,
}

/// What a `state` command prints: one JSON object, every field optional.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct Printed {
    pub icon: Option<String>,
    pub tip: Option<String>,
    pub tint: Option<String>,
    pub badge: Option<String>,
    pub enabled: Option<bool>,
}

fn text(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// The `[[title.button]]` entries of one `wheel.toml`, in file order. A file that is missing sets
/// none; one that is not TOML sets none and is logged; an entry with no `id`, a duplicate `id` in
/// the same file, or a `side` that is not `left` or `right` is skipped and logged.
pub(crate) fn read(path: &Path, origin: SettingOrigin) -> Vec<Entry> {
    let Ok(file) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let table: toml::Table = match toml::from_str(&file) {
        Ok(table) => table,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a wheel.toml is not valid TOML; it names no title buttons");
            return Vec::new();
        }
    };
    let Some(list) = table
        .get("title")
        .and_then(|t| t.get("button"))
        .and_then(|b| b.as_array())
    else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = Vec::new();
    for value in list {
        let raw: Raw = match value.clone().try_into() {
            Ok(raw) => raw,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "a [[title.button]] entry has a field of the wrong type; it is skipped");
                continue;
            }
        };
        let Some(id) = text(raw.id) else {
            tracing::warn!(path = %path.display(), "a [[title.button]] entry has no id; it is skipped");
            continue;
        };
        if out.iter().any(|e| e.id == id) {
            tracing::warn!(path = %path.display(), %id, "two [[title.button]] entries in one file share an id; the second is skipped");
            continue;
        }
        let side = match raw.side.as_deref() {
            None | Some("left") => TitleSide::Left,
            Some("right") => TitleSide::Right,
            Some(other) => {
                tracing::warn!(path = %path.display(), %id, side = other, "a [[title.button]] side is left or right; the entry is skipped");
                continue;
            }
        };
        let command = text(raw.command);
        // Only this machine's file runs a command on a timer; a project's `state` never runs, so a
        // repository cannot run anything until a button of it is pressed.
        let state = text(raw.state).filter(|_| origin == SettingOrigin::Machine);
        let every = match raw.every {
            Some(n) if n >= 1 => n as u64,
            Some(n) => {
                tracing::warn!(path = %path.display(), %id, every = n, "a [[title.button]] every is at least 1 second; the default is used");
                DEFAULT_EVERY
            }
            None => DEFAULT_EVERY,
        };
        let icon = raw.icon.unwrap_or_default();
        if !raw.hidden && icon.is_empty() {
            tracing::warn!(path = %path.display(), %id, "a [[title.button]] entry has no icon; it is skipped");
            continue;
        }
        out.push(Entry {
            id,
            side,
            hidden: raw.hidden,
            icon,
            tip: raw.tip.unwrap_or_default(),
            command,
            state,
            every,
            origin,
        });
    }
    out
}

/// The machine's entries, then the project's: an entry whose `id` the machine already has replaces
/// it where it stood, any other is added at the end.
pub(crate) fn merge(machine: Vec<Entry>, project: Vec<Entry>) -> Vec<Entry> {
    let mut out = machine;
    for entry in project {
        match out.iter().position(|e| e.id == entry.id) {
            Some(at) => out[at] = entry,
            None => out.push(entry),
        }
    }
    out
}

/// The merged entries for the space whose directory is `cwd`.
pub(crate) fn resolve(machine_file: &Path, cwd: &str) -> Vec<Entry> {
    merge(
        read(machine_file, SettingOrigin::Machine),
        read(&Path::new(cwd).join(FILE), SettingOrigin::Project),
    )
}

/// An entry as a client draws it, with the state last printed laid over it.
pub(crate) fn info(entry: &Entry, printed: Option<&Printed>) -> TitleButtonInfo {
    let printed = printed.cloned().unwrap_or_default();
    TitleButtonInfo {
        id: entry.id.clone(),
        side: entry.side,
        hidden: entry.hidden,
        icon: text(printed.icon).unwrap_or_else(|| entry.icon.clone()),
        tip: text(printed.tip).unwrap_or_else(|| entry.tip.clone()),
        tint: printed.tint,
        badge: printed.badge.filter(|b| !b.is_empty()),
        enabled: printed.enabled.unwrap_or(true),
        has_command: entry.command.is_some(),
        origin: entry.origin,
    }
}

/// What wheeld remembers about the buttons: each `state` command's last output and when it ran, and
/// the list each space's clients were last told.
#[derive(Default)]
pub(crate) struct Buttons {
    seen: HashMap<(String, String), Seen>,
    told: HashMap<String, Vec<TitleButtonInfo>>,
    /// Buttons whose `command` is running, so a second press waits for it.
    pressing: HashSet<(String, String)>,
    /// The id the next `state` run gets.
    next_run: u64,
}

#[derive(Default)]
struct Seen {
    command: String,
    printed: Option<Printed>,
    ran: Option<Instant>,
    /// The run in flight, if any. A run that finishes after the entry was removed, re-added or
    /// given another `state` finds a different id here and writes nothing.
    running: Option<u64>,
}

/// What `seen` holds for `entry` while it still comes from the entry's current `state` command.
fn printed_for<'a>(
    seen: &'a HashMap<(String, String), Seen>,
    space: &str,
    entry: &Entry,
) -> Option<&'a Printed> {
    seen.get(&(space.to_string(), entry.id.clone()))
        .filter(|s| entry.state.as_deref() == Some(s.command.as_str()))
        .and_then(|s| s.printed.as_ref())
}

/// Every space's merged entries, read off the runtime's threads: this machine's file once, then each
/// space's own.
fn resolve_all(
    machine_file: &Path,
    spaces: &[(String, String)],
) -> Vec<(String, String, Vec<Entry>)> {
    let machine = read(machine_file, SettingOrigin::Machine);
    spaces
        .iter()
        .map(|(id, cwd)| {
            let project = read(&Path::new(cwd).join(FILE), SettingOrigin::Project);
            (id.clone(), cwd.clone(), merge(machine.clone(), project))
        })
        .collect()
}

impl AppState {
    /// A space's buttons now: the merged entries with the state each last printed.
    pub(crate) fn title_buttons(&self, space: &str) -> Result<Vec<TitleButtonInfo>, String> {
        let Some((id, cwd)) = self.settings_space(Some(space))? else {
            return Err(format!("no space {space}"));
        };
        let entries = resolve(&self.machine_file(), &cwd);
        let buttons = self.title_buttons.lock().unwrap();
        Ok(entries
            .iter()
            .map(|e| info(e, printed_for(&buttons.seen, &id, e)))
            .collect())
    }

    /// Run button `id`'s `command` in the space's directory, detached; its exit is logged. Refused
    /// for a button with no command, one its `state` printed `enabled: false` for, and one whose
    /// command is still running.
    pub(crate) fn title_button_press(&self, space: &str, id: &str) -> Result<(), String> {
        let Some((space_id, cwd)) = self.settings_space(Some(space))? else {
            return Err(format!("no space {space}"));
        };
        let entry = resolve(&self.machine_file(), &cwd)
            .into_iter()
            .find(|e| e.id == id && !e.hidden)
            .ok_or(format!("no title button {id}"))?;
        let command = entry
            .command
            .clone()
            .ok_or(format!("title button {id} has no command"))?;
        let key = (space_id.clone(), id.to_string());
        {
            let mut buttons = self.title_buttons.lock().unwrap();
            if printed_for(&buttons.seen, &space_id, &entry).and_then(|p| p.enabled) == Some(false) {
                return Err(format!("title button {id} is disabled"));
            }
            if !buttons.pressing.insert(key.clone()) {
                return Err(format!("title button {id} is still running"));
            }
        }
        tracing::info!(space = %space_id, button = %id, %command, "title button pressed");
        let Some(state) = self.me.upgrade() else {
            return Ok(());
        };
        tokio::spawn(async move {
            match run(&command, &cwd, &key.0, PRESS_TIMEOUT).await {
                Ok((status, _)) => {
                    tracing::info!(space = %key.0, button = %key.1, %status, "title button command finished")
                }
                Err(e) => {
                    tracing::warn!(space = %key.0, button = %key.1, error = %e, "title button command failed")
                }
            }
            state.title_buttons.lock().unwrap().pressing.remove(&key);
        });
        Ok(())
    }

    /// Start the loop that keeps every space's buttons current: each second it reads both files
    /// and starts the `state` commands that are due, and sends `title_buttons_changed` for a space
    /// whose list is not the one its clients were last told. Call once, inside the tokio runtime.
    pub fn start_title_buttons(&self) {
        let Some(state) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            loop {
                tick.tick().await;
                state.title_buttons_tick().await;
            }
        });
    }

    async fn title_buttons_tick(&self) {
        let spaces: Vec<(String, String)> = self
            .spaces
            .lock()
            .unwrap()
            .spaces
            .iter()
            .map(|s| (s.id.clone(), s.cwd.clone()))
            .collect();
        let machine = self.machine_file();
        let read_spaces = spaces.clone();
        let Ok(resolved) =
            tokio::task::spawn_blocking(move || resolve_all(&machine, &read_spaces)).await
        else {
            return;
        };
        let mut due: Vec<(String, String, String, String, u64)> = Vec::new();
        let mut changed: Vec<(String, Vec<TitleButtonInfo>)> = Vec::new();
        {
            let mut buttons = self.title_buttons.lock().unwrap();
            let live: Vec<&String> = spaces.iter().map(|(id, _)| id).collect();
            buttons.told.retain(|space, _| live.contains(&space));
            buttons.seen.retain(|(space, _), _| live.contains(&space));
            for (space, cwd, entries) in &resolved {
                let wanted: Vec<&str> = entries
                    .iter()
                    .filter(|e| e.state.is_some() && !e.hidden)
                    .map(|e| e.id.as_str())
                    .collect();
                buttons
                    .seen
                    .retain(|(s, id), _| s != space || wanted.contains(&id.as_str()));
                for e in entries {
                    let (Some(command), false) = (&e.state, e.hidden) else {
                        continue;
                    };
                    let run_id = buttons.next_run;
                    let seen = buttons.seen.entry((space.clone(), e.id.clone())).or_default();
                    let stale = seen.command != *command
                        || seen
                            .ran
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(e.every));
                    if stale && seen.running.is_none() {
                        if seen.command != *command {
                            seen.printed = None;
                        }
                        seen.command = command.clone();
                        seen.running = Some(run_id);
                        buttons.next_run += 1;
                        due.push((space.clone(), e.id.clone(), command.clone(), cwd.clone(), run_id));
                    }
                }
                let list: Vec<TitleButtonInfo> = entries
                    .iter()
                    .map(|e| info(e, printed_for(&buttons.seen, space, e)))
                    .collect();
                if buttons.told.get(space) != Some(&list) {
                    buttons.told.insert(space.clone(), list.clone());
                    changed.push((space.clone(), list));
                }
            }
        }
        for (space, buttons) in changed {
            tracing::info!(%space, count = buttons.len(), "title buttons changed");
            let _ = self
                .global
                .send(ServerMsg::TitleButtonsChanged { space, buttons });
        }
        for (space, id, command, cwd, run_id) in due {
            let Some(state) = self.me.upgrade() else {
                return;
            };
            tokio::spawn(async move {
                let printed = match run(&command, &cwd, &space, STATE_TIMEOUT).await {
                    Ok((0, out)) => {
                        let line = out.lines().rev().find(|l| !l.trim().is_empty());
                        match line.map(serde_json::from_str::<Printed>) {
                            Some(Ok(p)) => Some(p),
                            Some(Err(e)) => {
                                tracing::warn!(%space, button = %id, error = %e, "a title button's state is not one JSON object; the button shows its entry");
                                None
                            }
                            None => None,
                        }
                    }
                    Ok((status, _)) => {
                        tracing::warn!(%space, button = %id, status, "a title button's state command failed; the button shows its entry");
                        None
                    }
                    Err(e) => {
                        tracing::warn!(%space, button = %id, error = %e, "a title button's state command could not run; the button shows its entry");
                        None
                    }
                };
                let mut buttons = state.title_buttons.lock().unwrap();
                if let Some(seen) = buttons.seen.get_mut(&(space, id)) {
                    if seen.running == Some(run_id) {
                        seen.printed = printed;
                        seen.ran = Some(Instant::now());
                        seen.running = None;
                    }
                }
            });
        }
    }
}

/// Run `command` through `/bin/sh -c` in `cwd`, as the leader of its own process group; its exit
/// code and the first [`OUTPUT_CAP`] bytes of stdout. After `limit` the whole group is killed, so a
/// `state` that backgrounds or pipes into something leaves nothing running.
async fn run(
    command: &str,
    cwd: &str,
    space: &str,
    limit: Duration,
) -> Result<(i32, String), String> {
    let mut child = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("WHEEL_SPACE", space)
        .env("WHEEL_SPACE_DIR", cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id().ok_or("the command ended before it started")?;
    let out = child.stdout.take().ok_or("no stdout")?;
    let read = async {
        let mut buf = Vec::new();
        let mut capped = tokio::io::AsyncReadExt::take(out, OUTPUT_CAP);
        tokio::io::AsyncReadExt::read_to_end(&mut capped, &mut buf)
            .await
            .map_err(|e| e.to_string())?;
        drop(capped);
        let status = child.wait().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((status.code().unwrap_or(-1), String::from_utf8_lossy(&buf).into()))
    };
    let result = tokio::time::timeout(limit, read).await;
    // Whatever is left of the group, a finished command's background children included.
    // SAFETY: killpg takes a plain process group id and touches no memory of ours.
    unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    result.map_err(|_| format!("ran longer than {} s", limit.as_secs()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("wheel.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_project_entry_replaces_the_machines_by_id_and_new_ones_follow() {
        let dir = std::env::temp_dir().join(format!("tb-merge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let machine = file(
            &dir,
            "[[title.button]]\nid='vpn'\nicon='lock'\n[[title.button]]\nid='pin'\nhidden=true\n",
        );
        let project_dir = dir.join("p");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project = file(
            &project_dir,
            "[[title.button]]\nid='vpn'\nicon='lock.open'\nside='right'\ncommand='true'\n[[title.button]]\nid='deploy'\nicon='shippingbox'\nstate='echo {}'\nevery=2\n",
        );
        let merged = merge(
            read(&machine, SettingOrigin::Machine),
            read(&project, SettingOrigin::Project),
        );
        let ids: Vec<_> = merged.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["vpn", "pin", "deploy"]);
        assert_eq!(merged[0].icon, "lock.open");
        assert_eq!(merged[0].side, TitleSide::Right);
        assert_eq!(merged[0].origin, SettingOrigin::Project);
        assert!(merged[1].hidden);
        assert_eq!(merged[2].every, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_machines_file_keeps_a_state_command() {
        let dir = std::env::temp_dir().join(format!("tb-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, "[[title.button]]\nid='a'\nicon='x'\ncommand='true'\nstate='echo {}'\n");
        assert!(read(&path, SettingOrigin::Machine)[0].state.is_some());
        let project = read(&path, SettingOrigin::Project);
        assert!(project[0].state.is_none());
        assert_eq!(project[0].command.as_deref(), Some("true"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_entry_is_skipped_and_the_rest_stay() {
        let dir = std::env::temp_dir().join(format!("tb-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(
            &dir,
            "[[title.button]]\nicon='x'\n[[title.button]]\nid='a'\nicon='x'\nside='middle'\n[[title.button]]\nid='b'\n[[title.button]]\nid='ok'\nicon='star'\n[[title.button]]\nid='ok'\nicon='dup'\n",
        );
        let got = read(&path, SettingOrigin::Machine);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "ok");
        assert_eq!(got[0].icon, "star");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn printed_state_lays_over_the_entry() {
        let entry = Entry {
            id: "vpn".into(),
            side: TitleSide::Left,
            hidden: false,
            icon: "lock".into(),
            tip: "VPN".into(),
            command: Some("true".into()),
            state: None,
            every: 5,
            origin: SettingOrigin::Machine,
        };
        let printed: Printed =
            serde_json::from_str(r#"{"icon":"lock.fill","tint":"green","badge":"2","enabled":false}"#)
                .unwrap();
        let shown = info(&entry, Some(&printed));
        assert_eq!(
            (shown.icon.as_str(), shown.tip.as_str(), shown.tint.as_deref(), shown.badge.as_deref(), shown.enabled),
            ("lock.fill", "VPN", Some("green"), Some("2"), false)
        );
        assert!(shown.has_command);
    }
}
