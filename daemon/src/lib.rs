//! `wheeld`: holds every terminal and agent session, and serves clients over a WebSocket at `/ws`
//! and web panes' folders at `/artifacts/<pane>/`.

mod agent;
mod agents;
mod artifacts;
mod brief;
mod claude;
mod codex;
mod devices;
pub mod login_path;
mod mac_windows;
mod page;
mod pi;
pub mod pty;
mod refresh;
pub mod remote;
mod render;
mod resume;
mod server;
mod settings;
pub mod usage;
mod views;

pub use refresh::{warden, WARDEN_ARG};
pub use server::{serve, AgentTiming, AppState};
pub mod perms;
pub mod session;
mod shell;
pub mod spaces;
mod web;
