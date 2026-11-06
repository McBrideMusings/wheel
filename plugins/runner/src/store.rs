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
    /// When `pid` started, in seconds since the Unix epoch: a later process given the same pid
    /// starts at another time. 0 when it could not be read.
    #[serde(default)]
    pub pid_started: u64,
    /// The process that owns the run: the runner, or the command line that started it.
    pub owner_pid: u32,
    pub stop_requested: bool,
    pub log: String,
}

impl Record {
    /// The command's process, which leads the run's process group.
    pub fn leader(&self) -> crate::sweep::Leader {
        crate::sweep::Leader { pid: self.pid, started: self.pid_started }
    }
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

/// How many runs a project keeps: starting one removes the record and log of every older run that
/// is not running, and every read stops at this many.
pub const KEEP: usize = 100;

/// The ids of the folder's runs, newest first. An id starts with the millisecond its run began, so
/// the ids order the runs, and no record is opened to tell.
fn ids(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return vec![] };
    let mut ids: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".json").filter(|id| !id.starts_with('.')).map(String::from))
        .collect();
    ids.sort_by(|a, b| b.cmp(a));
    ids
}

/// The newest `n` runs of the folder, newest first. Only those records are read.
pub fn recent(folder: &Path, n: usize) -> Vec<Record> {
    ids(folder).iter().filter_map(|id| read(folder, id).ok()).take(n).collect()
}

/// Remove the record and log of every run past the newest `KEEP` that is not running.
pub fn prune(folder: &Path) {
    for id in ids(folder).into_iter().skip(KEEP) {
        if read(folder, &id).is_ok_and(|r| r.state == State::Running) {
            continue;
        }
        let _ = std::fs::remove_file(record_path(folder, &id));
        let _ = std::fs::remove_file(log_path(folder, &id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn record(id: &str, started: i64) -> Record {
        Record {
            id: id.into(), name: "n".into(), command: "true".into(), dir: "/p".into(), state: State::Exited,
            started, ended: Some(started + 1), exit_code: Some(0), pid: 2, pid_started: 0, owner_pid: 1, stop_requested: false, log: String::new(),
        }
    }

    #[test]
    fn records_round_trip_and_list_newest_first() {
        let dir = std::env::temp_dir().join(format!("wheel-runner-store-{}", std::process::id()));
        write(&dir, &record("a", 10)).unwrap();
        write(&dir, &record("b", 20)).unwrap();
        assert_eq!(read(&dir, "a").unwrap(), record("a", 10));
        let ids: Vec<String> = recent(&dir, KEEP).into_iter().map(|r| r.id).collect();
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

    #[test]
    fn a_read_of_the_latest_run_stays_fast_among_thousands_and_old_runs_are_pruned() {
        let dir = std::env::temp_dir().join(format!("wheel-runner-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for n in 0..2000 {
            let mut r = record(&format!("{n:013}-x"), n);
            if n == 5 {
                r.state = State::Running;
            }
            write(&dir, &r).unwrap();
            std::fs::write(log_path(&dir, &r.id), "log\n").unwrap();
        }
        let began = std::time::Instant::now();
        let latest = recent(&dir, 1);
        assert_eq!(latest[0].id, format!("{:013}-x", 1999));
        assert!(began.elapsed() < Duration::from_millis(100), "one record took {:?} among 2000", began.elapsed());
        prune(&dir);
        let left = ids(&dir);
        assert_eq!(left.len(), KEEP + 1, "the newest {KEEP} and the one still running stay");
        assert!(left.contains(&format!("{:013}-x", 5)));
        assert!(!log_path(&dir, &format!("{:013}-x", 6)).exists(), "a pruned run's log goes with its record");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
