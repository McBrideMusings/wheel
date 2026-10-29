//! What each browser pane's page has written to its console since its last page load.

use std::collections::{HashMap, VecDeque};

use wheel_proto::ConsoleEntry;

/// How many messages a pane keeps; the oldest go first.
pub const CAP: usize = 1000;

/// The most characters of one message kept; the rest is cut.
pub const MAX_TEXT: usize = 4096;

#[derive(Debug, Default)]
pub struct ConsoleBuffers {
    panes: HashMap<String, VecDeque<ConsoleEntry>>,
}

impl ConsoleBuffers {
    pub fn append(&mut self, pane: &str, entries: Vec<ConsoleEntry>) {
        let buffer = self.panes.entry(pane.to_string()).or_default();
        buffer.extend(entries);
        while buffer.len() > CAP {
            buffer.pop_front();
        }
    }

    /// Forget a pane's messages: its page loaded again, or the pane went.
    pub fn clear(&mut self, pane: &str) {
        self.panes.remove(pane);
    }

    /// The pane's messages, oldest first; with `since`, only those written after that time.
    pub fn read(&self, pane: &str, since: Option<u64>) -> Vec<ConsoleEntry> {
        self.panes
            .get(pane)
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
        buffers.append("p1", vec![entry(1, "a"), entry(2, "b")]);
        buffers.append("p2", vec![entry(1, "other pane")]);
        assert_eq!(buffers.read("p1", None).len(), 2);
        assert_eq!(buffers.read("p1", Some(1)), vec![entry(2, "b")]);

        let many = (0..CAP as u64 + 5).map(|i| entry(10 + i, "x")).collect();
        buffers.append("p1", many);
        let kept = buffers.read("p1", None);
        assert_eq!(kept.len(), CAP);
        assert_eq!(kept.last().unwrap().time, 10 + CAP as u64 + 4);
        assert_eq!(kept.first().unwrap().time, 10 + 5);

        buffers.clear("p1");
        assert!(buffers.read("p1", None).is_empty());
        assert_eq!(buffers.read("p2", None).len(), 1);
    }
}
