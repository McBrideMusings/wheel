//! The process card (ProcessesView.swift): every process a space's panes started, as wheeld last
//! listed them, a filter over name, command and pane, and the chosen row whose details show.
//! wheeld lists them when the card opens and every 2 s while it stays open.
use std::collections::HashMap;
use wheel_proto::ProcInfo;

/// How often an open card asks wheeld again.
pub const REFRESH_MS: u64 = 2000;

#[derive(Debug, Default)]
pub struct Procs {
    /// The processes each space's panes started, by space id.
    pub by_space: HashMap<String, Vec<ProcInfo>>,
    /// The space whose card is open, if any.
    pub open: Option<String>,
    pub filter: String,
    /// The row whose command line, directory and Stop button show.
    pub chosen: Option<u32>,
    /// Unix ms the open card last asked.
    pub asked_at: u64,
}

impl Procs {
    /// The rows the filter keeps for `space`, in wheeld's order (pane order, pid order).
    pub fn rows(&self, space: &str) -> Vec<&ProcInfo> {
        let needle = self.filter.to_lowercase();
        self.by_space.get(space).map(|list| list.iter().filter(|p| keeps(p, &needle)).collect()).unwrap_or_default()
    }
    pub fn open_for(&mut self, space: &str) {
        self.open = Some(space.to_owned());
        self.filter.clear();
        self.chosen = None;
    }
    pub fn close(&mut self) {
        self.open = None;
        self.chosen = None;
    }
    pub fn toggle_chosen(&mut self, pid: u32) {
        self.chosen = if self.chosen == Some(pid) { None } else { Some(pid) };
    }
    /// wheeld's list arrived: a chosen row that left the list is forgotten.
    pub fn listed(&mut self, space: &str, procs: Vec<ProcInfo>) {
        if let Some(pid) = self.chosen
            && !procs.iter().any(|p| p.pid == pid)
        {
            self.chosen = None;
        }
        self.by_space.insert(space.to_owned(), procs);
    }
    /// Whether an open card is due to ask again at `now` (unix ms).
    pub fn due(&self, now: u64) -> bool {
        self.open.is_some() && now >= self.asked_at + REFRESH_MS
    }
}

fn keeps(p: &ProcInfo, needle: &str) -> bool {
    needle.is_empty()
        || p.command.to_lowercase().contains(needle)
        || p.name.to_lowercase().contains(needle)
        || p.pane.as_deref().unwrap_or("").to_lowercase().contains(needle)
}

/// `pid 123 · 4 MB · up 7s`, the row's detail line.
pub fn detail(p: &ProcInfo) -> String {
    format!("pid {} · {} MB · up {}s", p.pid, p.mem_kb / 1024, p.age_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, command: &str, pane: Option<&str>) -> ProcInfo {
        serde_json::from_value(serde_json::json!({
            "pid": pid, "ppid": 1, "pane": pane, "name": name, "command": command, "cpu": 1.5, "mem_kb": 2048, "age_secs": 7
        })).unwrap()
    }

    #[test]
    fn the_filter_matches_name_command_and_pane_whatever_the_case() {
        let mut procs = Procs::default();
        procs.listed("s1", vec![proc(1, "sleep", "sleep 4243", Some("p2")), proc(2, "node", "node server.js", None)]);
        procs.filter = "SLEEP".into();
        assert_eq!(procs.rows("s1").iter().map(|p| p.pid).collect::<Vec<_>>(), [1]);
        procs.filter = "server".into();
        assert_eq!(procs.rows("s1").iter().map(|p| p.pid).collect::<Vec<_>>(), [2]);
        procs.filter = "p2".into();
        assert_eq!(procs.rows("s1").iter().map(|p| p.pid).collect::<Vec<_>>(), [1]);
        procs.filter.clear();
        assert_eq!(procs.rows("s1").len(), 2);
        assert!(procs.rows("other").is_empty());
    }

    #[test]
    fn a_chosen_row_that_left_the_list_is_forgotten_and_the_card_asks_every_two_seconds() {
        let mut procs = Procs::default();
        procs.open_for("s1");
        procs.toggle_chosen(5);
        assert_eq!(procs.chosen, Some(5));
        procs.toggle_chosen(5);
        assert_eq!(procs.chosen, None);
        procs.toggle_chosen(5);
        procs.listed("s1", vec![proc(6, "a", "a", None)]);
        assert_eq!(procs.chosen, None);
        assert!(procs.due(REFRESH_MS));
        procs.asked_at = 1000;
        assert!(!procs.due(2999));
        assert!(procs.due(3000));
        procs.close();
        assert!(!procs.due(10_000));
        assert_eq!(detail(&proc(9, "x", "x", None)), "pid 9 · 2 MB · up 7s");
    }
}
