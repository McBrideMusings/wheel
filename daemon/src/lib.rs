//! `wheeld`: holds every terminal and agent session, and serves clients over a WebSocket at `/ws`
//! and web panes' folders at `/artifacts/<pane>/`.

mod agents;
mod artifacts;
mod brief;
pub mod pty;
mod refresh;
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
