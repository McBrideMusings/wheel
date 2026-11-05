//! What this daemon can reach on the machine: its own program path and the folders macOS guards.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use wheel_proto::{FolderAccess, HostAccess};

/// The folders macOS asks the person about before an app may open them. A daemon started by
/// launchd cannot show that question, so it fails until the person grants the app Full Disk
/// Access or the folder.
const GUARDED: [&str; 3] = ["Downloads", "Desktop", "Documents"];

/// How long a reply waits for a folder that has not answered yet before it says "waiting for
/// permission". A first look at an allowed folder returns in well under a millisecond.
const FIRST_ANSWER_WAIT: Duration = Duration::from_millis(300);

/// How old a folder's answer is before the next reply starts one new look.
const REFRESH: Duration = Duration::from_secs(2);

/// The running program and whether each guarded folder lists. It never waits on a folder for more
/// than `FIRST_ANSWER_WAIT`: see [`Probe`].
pub fn access() -> HostAccess {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            Probe::new(
                GUARDED
                    .iter()
                    .map(|name| match &home {
                        Some(home) => home.join(name),
                        None => PathBuf::from(name),
                    })
                    .collect(),
                Arc::new(list),
                FIRST_ANSWER_WAIT,
                REFRESH,
            )
        })
        .access()
}

/// The reason `path` cannot be listed, when macOS (or the mode bits) denies it. A folder that is
/// not there is not a denial.
fn list(path: &Path) -> Option<String> {
    std::fs::read_dir(path)
        .err()
        .filter(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        .map(|e| e.to_string())
}

/// The last answer for one folder, and whether a look at it is under way.
struct Slot {
    path: PathBuf,
    /// When the last look finished and the error it found; None until the first look ends.
    answer: Option<(Instant, Option<String>)>,
    looking: bool,
}

/// Looks at the guarded folders from threads of its own and keeps the answers.
///
/// `opendir` on a folder macOS guards does not fail while the permission prompt is open: it waits
/// for the person's answer, for minutes if they are away. So a look runs on its own thread, at most
/// one per folder at a time, and a reply uses the last answer. A folder with no answer yet after
/// `wait` is reported as waiting for permission, never as an error.
pub struct Probe {
    slots: Arc<Mutex<Vec<Slot>>>,
    answered: Arc<Condvar>,
    check: Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>,
    wait: Duration,
    refresh: Duration,
}

impl Probe {
    pub fn new(
        paths: Vec<PathBuf>,
        check: Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>,
        wait: Duration,
        refresh: Duration,
    ) -> Self {
        let slots = paths
            .into_iter()
            .map(|path| Slot {
                path,
                answer: None,
                looking: false,
            })
            .collect();
        Self {
            slots: Arc::new(Mutex::new(slots)),
            answered: Arc::new(Condvar::new()),
            check,
            wait,
            refresh,
        }
    }

    pub fn access(&self) -> HostAccess {
        let mut slots = self.slots.lock().unwrap();
        for index in 0..slots.len() {
            let stale = slots[index]
                .answer
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() >= self.refresh);
            if stale && !slots[index].looking {
                slots[index].looking = true;
                let path = slots[index].path.clone();
                let (shared, answered, check) = (
                    self.slots.clone(),
                    self.answered.clone(),
                    self.check.clone(),
                );
                let spawned = std::thread::Builder::new()
                    .name("folder-probe".into())
                    .spawn(move || {
                        let error = check(&path);
                        if let Some(error) = &error {
                            tracing::debug!(path = %path.display(), %error, "guarded folder cannot be listed");
                        }
                        let mut slots = shared.lock().unwrap();
                        if let Some(slot) = slots.iter_mut().find(|s| s.path == path) {
                            slot.answer = Some((Instant::now(), error));
                            slot.looking = false;
                        }
                        answered.notify_all();
                    });
                if spawned.is_err() {
                    slots[index].looking = false;
                }
            }
        }
        // Only a folder that has never answered holds the reply back.
        let deadline = Instant::now() + self.wait;
        while slots.iter().any(|s| s.answer.is_none() && s.looking) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            slots = self.answered.wait_timeout(slots, left).unwrap().0;
        }
        let folders: Vec<FolderAccess> = slots
            .iter()
            .map(|slot| match &slot.answer {
                Some((_, error)) => FolderAccess {
                    path: slot.path.display().to_string(),
                    allowed: error.is_none(),
                    error: error.clone(),
                    waiting: false,
                },
                None => FolderAccess {
                    path: slot.path.display().to_string(),
                    allowed: false,
                    error: None,
                    waiting: true,
                },
            })
            .collect();
        drop(slots);
        HostAccess {
            exe: std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            source: env!("WHEELD_SOURCE_HASH").to_string(),
            folders,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Removes the directory on the way out, even when an assertion fails.
    struct Home(std::path::PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_folder_the_daemon_may_not_list_is_denied_and_a_missing_one_is_not() {
        let dir = std::env::temp_dir().join(format!("wheel-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let home = Home(dir);
        std::fs::create_dir(home.0.join("Downloads")).unwrap();
        std::fs::create_dir(home.0.join("Desktop")).unwrap();
        std::fs::set_permissions(
            home.0.join("Downloads"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let got = Probe::new(
            ["Downloads", "Desktop", "Documents"]
                .iter()
                .map(|name| home.0.join(name))
                .collect(),
            Arc::new(list),
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
        .access();
        std::fs::set_permissions(
            home.0.join("Downloads"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let allowed: Vec<bool> = got.folders.iter().map(|f| f.allowed).collect();
        assert_eq!(allowed, [false, true, true]);
        assert!(got.folders[0]
            .error
            .as_deref()
            .unwrap()
            .contains("ermission denied"));
        assert!(got.folders[1].error.is_none());
        assert_eq!(got.source, env!("WHEELD_SOURCE_HASH"));
        assert_eq!(got.source.len(), 16);
        assert!(got.exe.ends_with(
            std::env::current_exe()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        ));
    }

    /// A look that never returns, as `opendir` does under an open permission prompt: the reply
    /// still comes back within the wait, says waiting, and many replies start one look per folder.
    #[test]
    fn a_folder_waiting_for_the_prompt_is_reported_waiting_and_looked_at_once() {
        let looks = Arc::new(AtomicUsize::new(0));
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = Arc::new(Mutex::new(released));
        let check = {
            let looks = looks.clone();
            move |path: &Path| {
                if path.ends_with("Documents") {
                    looks.fetch_add(1, Ordering::SeqCst);
                    let _ = released.lock().unwrap().recv();
                }
                None
            }
        };
        let probe = Probe::new(
            vec!["/x/Downloads".into(), "/x/Documents".into()],
            Arc::new(check),
            Duration::from_millis(20),
            Duration::from_millis(1),
        );
        let begun = Instant::now();
        let mut last = probe.access();
        for _ in 0..50 {
            last = probe.access();
        }
        assert!(
            begun.elapsed() < Duration::from_secs(10),
            "51 replies took {:?}",
            begun.elapsed()
        );
        assert_eq!(looks.load(Ordering::SeqCst), 1, "one look at the folder that waits");
        assert!(last.folders[0].allowed && !last.folders[0].waiting);
        assert!(!last.folders[1].allowed && last.folders[1].waiting);
        assert!(last.folders[1].error.is_none());
        // The person answers the prompt: the look ends and the next reply says allowed.
        release.send(()).unwrap();
        let mut got = probe.access();
        for _ in 0..100 {
            if !got.folders[1].waiting {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            got = probe.access();
        }
        assert!(got.folders[1].allowed && !got.folders[1].waiting);
    }
}
