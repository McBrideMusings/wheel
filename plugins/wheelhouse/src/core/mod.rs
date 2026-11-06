//! What every view shares and none owns: which project and pane a request is for, running the
//! project's tools (`bd`, `gh`, `git`), what the running agents are doing, noticing that a
//! project's files changed, and the Table component. Nothing here knows what a bead or a diff is.

pub mod agents;
pub mod git;
pub mod layout;
pub mod page;
pub mod project;
pub mod run;
pub mod table;
pub mod tracker;
pub mod watch;
