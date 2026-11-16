//! What each browser pane's page has written to its console since its last page load, kept
//! apart for each side that draws its own copy of the page: the Mac's, and the phone's.

use std::collections::{HashMap, VecDeque};

use wheel_proto::ConsoleEntry;

use crate::render::Side;

/// How many messages a pane keeps; the oldest go first.
pub const CAP: usize = 1000;

/// The most characters of one message kept; the rest is cut.
pub const MAX_TEXT: usize = 4096;

#[derive(Debug, Default)]
pub struct ConsoleBuffers {
    panes: HashMap<(String, Side), VecDeque<ConsoleEntry>>,
}

impl ConsoleBuffers {
    pub fn append(&mut self, pane: &str, side: Side, entries: Vec<ConsoleEntry>) {
        let buffer = self.panes.entry((pane.to_string(), side)).or_default();
        buffer.extend(entries);
        while buffer.len() > CAP {
            buffer.pop_front();
        }
    }

    /// Forget what one side's page wrote: that side loaded the page again.
    pub fn clear(&mut self, pane: &str, side: Side) {
        self.panes.remove(&(pane.to_string(), side));
    }

    /// Forget both sides' messages: the pane went.
    pub fn forget(&mut self, pane: &str) {
        self.panes.retain(|(p, _), _| p != pane);
    }

    /// What one side's page wrote, oldest first; with `since`, only those written after that time.
    pub fn read(&self, pane: &str, side: Side, since: Option<u64>) -> Vec<ConsoleEntry> {
        self.panes
            .get(&(pane.to_string(), side))
            .into_iter()
            .flatten()
            .filter(|e| since.is_none_or(|t| e.time > t))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(time: u64, text: &str) -> ConsoleEntry {
        ConsoleEntry {
            time,
            level: "log".into(),
            text: text.into(),
            url: None,
            line: None,
        }
    }

    #[test]
    fn it_stores_caps_reads_since_and_clears_on_load() {
        let mut buffers = ConsoleBuffers::default();
        buffers.append("p1", Side::Mac, vec![entry(1, "a"), entry(2, "b")]);
        buffers.append("p2", Side::Mac, vec![entry(1, "other pane")]);
        assert_eq!(buffers.read("p1", Side::Mac, None).len(), 2);
        assert_eq!(buffers.read("p1", Side::Mac, Some(1)), vec![entry(2, "b")]);

        let many = (0..CAP as u64 + 5).map(|i| entry(10 + i, "x")).collect();
        buffers.append("p1", Side::Mac, many);
        let kept = buffers.read("p1", Side::Mac, None);
        assert_eq!(kept.len(), CAP);
        assert_eq!(kept.last().unwrap().time, 10 + CAP as u64 + 4);
        assert_eq!(kept.first().unwrap().time, 10 + 5);

        buffers.clear("p1", Side::Mac);
        assert!(buffers.read("p1", Side::Mac, None).is_empty());
        assert_eq!(buffers.read("p2", Side::Mac, None).len(), 1);
    }

    #[test]
    fn each_side_keeps_its_own_page_and_a_load_clears_only_its_own() {
        let mut buffers = ConsoleBuffers::default();
        buffers.append("p1", Side::Mac, vec![entry(1, "from mac")]);
        buffers.append("p1", Side::Phone, vec![entry(2, "from phone")]);
        assert_eq!(buffers.read("p1", Side::Mac, None), vec![entry(1, "from mac")]);
        assert_eq!(buffers.read("p1", Side::Phone, None), vec![entry(2, "from phone")]);

        buffers.clear("p1", Side::Phone);
        assert!(buffers.read("p1", Side::Phone, None).is_empty());
        assert_eq!(buffers.read("p1", Side::Mac, None), vec![entry(1, "from mac")]);

        buffers.append("p1", Side::Phone, vec![entry(3, "again")]);
        buffers.forget("p1");
        assert!(buffers.read("p1", Side::Mac, None).is_empty());
        assert!(buffers.read("p1", Side::Phone, None).is_empty());
    }
}
