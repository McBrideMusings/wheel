//! What every view shares and none owns: which project and pane a request is for, running the
//! project's tools (`bd`, `gh`, `git`), what the running agents are doing, noticing that a
//! project's files changed, and the Table component. Nothing here knows what a bead or a diff is.

// Helpers the views to come call (agent spawning) have no caller until those views land.
#![allow(dead_code)]

pub mod agents;
pub mod git;
pub mod page;
pub mod project;
pub mod run;
pub mod table;
pub mod watch;
