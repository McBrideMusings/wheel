//! The Swarm pane's run for `wheelhouse backend`: the `swarm` feed's value for one project. The
//! backend sends data only; the pane (Luau) decides what to draw.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::core::agents::{in_project, Agent};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// The local UTC offset now, in seconds east, from `date +%z` (`-0500`); 0 when it cannot be read.
fn offset() -> i64 {
    let z = crate::core::run::text("/bin/date", "/", &["+%z"]).unwrap_or_default();
    let z = z.trim();
    let (sign, digits) = match z.split_at_checked(1) {
        Some(("-", d)) => (-1, d),
        Some(("+", d)) => (1, d),
        _ => return 0,
    };
    let (h, m) = (digits.get(0..2).and_then(|x| x.parse::<i64>().ok()), digits.get(2..4).and_then(|x| x.parse::<i64>().ok()));
    sign * (h.unwrap_or(0) * 3600 + m.unwrap_or(0) * 60)
}

/// The run log of the project in `dir`, for the watcher; none when it cannot be named.
pub fn log_path(dir: &str) -> Option<PathBuf> {
    super::run_log(&home(), dir).ok()
}

/// The `swarm` feed's value for the project in `dir`, given every agent pane `all` that
/// `wheel agent watch` listed, whether it has delivered a list yet and why it could not run, if it could not.
pub fn value(dir: &str, all: &[Agent], loaded: bool, error: Option<String>) -> Value {
    let home = home();
    let now = crate::core::now();
    let (log, run, err) = match super::run_log(&home, dir) {
        Ok(log) => {
            let run = super::read(&home, &log, &in_project(all, dir), now, offset());
            (Some(log), run, error)
        }
        Err(e) => (None, super::value_of(&[], &[], now, 0, |_, _| None, |_: &Path, _: &str| vec![]), Some(e)),
    };
    let mut v = json!({
        "now": now,
        "name": crate::core::project::name_of(dir),
        "loaded": loaded,
        "error": err,
        "log": log.map(|p| p.to_string_lossy().into_owned()),
        "stuck_after": super::STUCK_AFTER,
    });
    if let (Some(o), Value::Object(r)) = (v.as_object_mut(), run) {
        o.extend(r);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_in_no_repository_says_so_and_has_no_run() {
        let v = value("/", &[], true, None);
        assert_eq!((v["state"].as_str(), v["log"].clone()), (Some("no-run"), Value::Null));
        assert!(v["error"].as_str().unwrap().contains("no git repository"));
        assert_eq!(v["workers"], json!([]));
    }

    #[test]
    fn the_offset_is_whole_minutes() {
        assert_eq!(offset() % 60, 0);
    }
}
