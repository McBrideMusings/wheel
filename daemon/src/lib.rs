//! `wheeld`: holds every terminal and agent session, and serves clients over a WebSocket at `/ws`
//! and web panes' folders at `/artifacts/<pane>/`.

mod agent;
mod agents;
mod artifacts;
mod brief;
mod builtins;
mod claude;
mod codex;
mod devices;
mod dirs;
mod homes;
pub mod host;
mod keybind;
pub mod login_path;
pub mod logship;
mod mac_windows;
mod page;
mod page_run;
mod pi;
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
mod web;
