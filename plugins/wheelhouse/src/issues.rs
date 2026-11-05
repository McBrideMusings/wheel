//! The Issues view's data: the project's tracker, with create, update and close writing through
//! `bd` (core::tracker).

use crate::core::tracker::{self, Item};

pub mod cli;
pub mod serve;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub has_tracker: bool,
    pub items: Vec<Item>,
    pub error: Option<String>,
}

pub fn collect(dir: &str) -> Snapshot {
    let mut s = Snapshot { name: crate::core::project::name_of(dir), has_tracker: tracker::has(dir), ..Default::default() };
    if s.has_tracker {
        match tracker::list(dir) {
            Ok(i) => s.items = i,
            Err(e) => s.error = Some(e),
        }
    }
    s
}
