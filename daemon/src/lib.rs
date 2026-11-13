//! `wheeld`: holds every terminal and agent, and serves clients over a WebSocket at `/ws`
//! and web panes' folders at `/artifacts/<pane>/`.

mod driver;
mod agents;
mod app;
mod artifacts;
mod brief;
mod builtins;
mod claude;
mod clients;
mod codex;
mod console;
mod devices;
mod dirs;
mod files;
mod guidance;
mod homes;
pub mod host;
mod keybind;
mod link_patterns;
pub mod checkout;
pub mod login_path;
pub mod logship;
mod mac_windows;
mod mcp;
mod page;
mod page_run;
mod pane_info;
mod pi;
mod procs;
mod proxy;
mod push;
pub mod pty;
mod refresh;
pub mod remote;
mod render;
mod resume;
mod runner_feed;
mod server;
mod settings;
mod settings_watch;
mod task_output;
mod skills;
mod submit;
mod title_buttons;
pub mod usage;
mod views;
mod orchestrator;
mod orchestrator_events;

pub use refresh::{warden, WARDEN_ARG};
pub use server::{serve, AgentTiming, AppState};
pub mod perms;
pub mod agent;
mod shell;
mod sites;
pub mod spaces;
mod status;
mod stream_time;
mod streams;
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
