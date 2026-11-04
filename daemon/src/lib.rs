//! `wheeld`: holds every terminal and agent session, and serves clients over a WebSocket at `/ws`
//! and web panes' folders at `/artifacts/<pane>/`.

mod agent;
mod agents;
mod artifacts;
mod brief;
mod builtins;
mod claude;
mod codex;
mod console;
mod devices;
mod dirs;
mod files;
mod homes;
pub mod host;
mod keybind;
mod link_patterns;
pub mod login_path;
pub mod logship;
mod mac_windows;
mod page;
mod page_run;
mod pane_info;
mod pi;
mod procs;
pub mod pty;
mod refresh;
pub mod remote;
mod render;
mod resume;
mod runner_feed;
mod server;
mod settings;
mod skills;
mod submit;
pub mod usage;
mod views;

pub use refresh::{warden, WARDEN_ARG};
pub use server::{serve, AgentTiming, AppState};
pub mod perms;
pub mod session;
mod shell;
pub mod spaces;
mod status;
mod uploads;
mod web;
// perf 1793567506
// perf 1793567559
// perf 1793568137
// perf 1793570679
// perf 1793570685
// perf 1793570790
// e 1793574330
// e 1793574466
// e 1793574654
// e 1793574698
// e 1793574818
// e 1793574823
