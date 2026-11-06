//! The status line: one line for the run a project most recently started, with its state, how
//! long it ran (or has been running) and where its log is.

use crate::store::{Record, State};

pub const NONE: &str = "no runs yet";

/// `● deploy running 0:42 · /path/log`, `✓ deploy exit 0 in 1:12 · …`, `✗ … exit 2 …`,
/// `■ … stopped …`, `? … lost …`.
pub fn line(latest: Option<&Record>, now: i64) -> String {
    let Some(r) = latest else { return NONE.into() };
    let took = crate::duration(r.ended.unwrap_or(now) - r.started);
    let (mark, what) = match (r.state, r.exit_code) {
        (State::Running, _) => ("●", format!("running {took}")),
        (State::Exited, Some(0)) => ("✓", format!("exit 0 in {took}")),
        (State::Exited, code) => ("✗", format!("exit {} in {took}", code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))),
        (State::Stopped, _) => ("■", format!("stopped after {took}")),
        (State::Lost, _) => ("?", format!("lost after {took}: its runner ended")),
    };
    format!("{mark} {} {what} · {}", r.name, r.log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: State, exit_code: Option<i32>, ended: Option<i64>) -> Record {
        Record {
            id: "i".into(), name: "deploy".into(), command: "c".into(), dir: "/p".into(), state, started: 100, ended, exit_code,
            pid: 2, pid_started: 0, owner_pid: 1, stop_requested: false, log: "/l/i.log".into(),
        }
    }

    #[test]
    fn the_line_names_state_duration_and_log() {
        assert_eq!(line(None, 0), NONE);
        assert_eq!(line(Some(&run(State::Running, None, None)), 142), "● deploy running 0:42 · /l/i.log");
        assert_eq!(line(Some(&run(State::Exited, Some(0), Some(172))), 500), "✓ deploy exit 0 in 1:12 · /l/i.log");
        assert_eq!(line(Some(&run(State::Exited, Some(2), Some(105))), 500), "✗ deploy exit 2 in 0:05 · /l/i.log");
        assert_eq!(line(Some(&run(State::Stopped, Some(143), Some(110))), 500), "■ deploy stopped after 0:10 · /l/i.log");
    }
}
