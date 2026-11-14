//! The Project pane's data: the project's status against its vision. Counts of the tracker's
//! items by state, and each epic's progress.

use std::collections::HashMap;

use crate::core::tracker::{self, Item};

pub mod cli;
pub mod feed;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub done: usize,
    pub progress: usize,
    pub ready: usize,
    pub blocked: usize,
    pub open: usize,
    pub deferred: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Epic {
    pub id: String,
    pub title: String,
    pub done: usize,
    pub all: usize,
    pub ready: usize,
    pub closed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub has_tracker: bool,
    pub vision: Option<String>,
    pub counts: Counts,
    pub epics: Vec<Epic>,
    pub error: Option<String>,
}

/// Which count an item belongs to; epics are containers and count in none.
fn state(i: &Item) -> &'static str {
    match i.status.as_str() {
        "closed" => "done",
        "in_progress" => "progress",
        "deferred" => "deferred",
        "blocked" => "blocked",
        _ if i.blocked => "blocked",
        _ if i.ready => "ready",
        _ => "open",
    }
}

pub fn summarize(items: &[Item]) -> (Counts, Vec<Epic>) {
    let mut c = Counts::default();
    let work: Vec<&Item> = items.iter().filter(|i| i.kind != "epic").collect();
    for i in &work {
        match state(i) {
            "done" => c.done += 1,
            "progress" => c.progress += 1,
            "deferred" => c.deferred += 1,
            "blocked" => c.blocked += 1,
            "ready" => c.ready += 1,
            _ => c.open += 1,
        }
    }
    let mut kids: HashMap<&str, Vec<&Item>> = HashMap::new();
    for i in &work {
        if let Some(p) = i.parent.as_deref() {
            kids.entry(p).or_default().push(i);
        }
    }
    let mut epics: Vec<Epic> = items.iter().filter(|i| i.kind == "epic" || kids.contains_key(i.id.as_str())).map(|e| {
        let k = kids.get(e.id.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        Epic { id: e.id.clone(), title: e.title.clone(), done: k.iter().filter(|i| i.status == "closed").count(), all: k.len(),
            ready: k.iter().filter(|i| state(i) == "ready").count(), closed: e.status == "closed" }
    }).collect();
    epics.sort_by(|a, b| a.closed.cmp(&b.closed).then(a.id.cmp(&b.id)));
    (c, epics)
}

pub fn collect(dir: &str) -> Snapshot {
    let mut s = Snapshot { name: crate::core::project::name_of(dir), has_tracker: tracker::has(dir), vision: tracker::vision(dir), ..Default::default() };
    if s.has_tracker {
        match tracker::list(dir) {
            Ok(items) => (s.counts, s.epics) = summarize(&items),
            Err(e) => s.error = Some(e),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: &str, status: &str, parent: Option<&str>, ready: bool) -> Item {
        Item { id: id.into(), title: id.into(), status: status.into(), kind: kind.into(), priority: Some(2), parent: parent.map(String::from),
            labels: vec![], assignee: None, description: String::new(), updated: String::new(), ready, blocked: false }
    }

    #[test]
    fn counts_skip_epics_and_epics_count_their_children() {
        let items = vec![item("e", "epic", "open", None, false), item("a", "task", "closed", Some("e"), false),
            item("b", "task", "open", Some("e"), true), item("c", "task", "in_progress", None, false)];
        let (c, epics) = summarize(&items);
        assert_eq!((c.done, c.ready, c.progress, c.open), (1, 1, 1, 0));
        assert_eq!((epics.len(), epics[0].done, epics[0].all, epics[0].ready), (1, 1, 2, 1));
    }
}
