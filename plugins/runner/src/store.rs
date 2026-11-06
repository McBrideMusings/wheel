//! A run on disk: one record (`<id>.json`) and one log (`<id>.log`) per run, in a folder per
//! project, so a run's status, exit code and log outlive the page, the host and the process that
//! ran it, and any process can read them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    /// The command exited by itself; `exit_code` says how.
    Exited,
    /// A stop was asked for and the run ended.
    Stopped,
    /// The process that ran it died first and the watcher ended what it had started.
    Lost,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub command: String,
    pub dir: String,
    pub state: State,
    /// Seconds since the Unix epoch.
    pub started: i64,
    pub ended: Option<i64>,
    pub exit_code: Option<i32>,
    /// The command's process, which leads the run's process group.
    pub pid: u32,
    /// The process that owns the run: the runner, or the command line that started it.
    pub owner_pid: u32,
    pub stop_requested: bool,
    pub log: String,
}

/// Where the runs of project `dir` are kept: `$WHEEL_RUNNER_DIR`, else `~/.wheel/runs`, then one
/// folder named for the project's path.
pub fn folder(dir: &str) -> PathBuf {
    let root = match std::env::var_os("WHEEL_RUNNER_DIR") {
        Some(p) => PathBuf::from(p),
        None => crate::config::home().join(".wheel").join("runs"),
    };
    let slug: String = dir.trim_matches('/').chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    // Two paths that differ only in punctuation share a slug; the hash tells them apart.
    let hash = dir.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    root.join(format!("{}-{:08x}", if slug.is_empty() { "root" } else { &slug }, hash as u32))
}

pub fn record_path(folder: &Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.json"))
}

/// Whether `id` is a run id: letters, digits, `-` and `_`, so it names a file in the folder and
/// nothing outside it.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn log_path(folder: &Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.log"))
}

pub fn write(folder: &Path, r: &Record) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let text = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    // A reader never sees half a record: write beside it, then rename over it.
    let tmp = folder.join(format!(".{}.tmp", r.id));
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, record_path(folder, &r.id)).map_err(|e| e.to_string())
}

pub fn read(folder: &Path, id: &str) -> Result<Record, String> {
    if !valid_id(id) {
        return Err(format!("no run {id}"));
    }
    let path = record_path(folder, id);
    let text = std::fs::read_to_string(&path).map_err(|_| format!("no run {id}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every run of the folder, newest first.
pub fn all(folder: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(folder) else { return vec![] };
    let mut runs: Vec<Record> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".json").filter(|id| !id.starts_with('.')).and_then(|id| read(folder, id).ok())
        })
        .collect();
    // An id starts with the millisecond its run began, so the ids order the runs.
    runs.sort_by(|a, b| b.id.cmp(&a.id));
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, started: i64) -> Record {
        Record {
            id: id.into(), name: "n".into(), command: "true".into(), dir: "/p".into(), state: State::Exited,
            started, ended: Some(started + 1), exit_code: Some(0), pid: 2, owner_pid: 1, stop_requested: false, log: String::new(),
        }
    }

    #[test]
    fn records_round_trip_and_list_newest_first() {
        let dir = std::env::temp_dir().join(format!("wheel-runner-store-{}", std::process::id()));
        write(&dir, &record("a", 10)).unwrap();
        write(&dir, &record("b", 20)).unwrap();
        assert_eq!(read(&dir, "a").unwrap(), record("a", 10));
        let ids: Vec<String> = all(&dir).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(read(&dir, "zzz").unwrap_err(), "no run zzz");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_projects_folder_is_named_for_its_path() {
        let f = folder("/Users/p/my project");
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Users-p-my-project-"), "{name}");
        assert_ne!(folder("/a/b-c"), folder("/a/b/c"));
        assert_eq!(read(&f, "../x").unwrap_err(), "no run ../x");
    }
}
