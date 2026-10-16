//! `wheeld`: holds every terminal and session, and serves clients over a WebSocket at `/ws`
//! the lead's tools over MCP at `/mcp`, and web panes' folders at `/artifacts/<pane>/`.

mod agents;
mod artifacts;
mod beads;
mod inbox;
pub mod lead;
pub mod pty;
mod refresh;
mod server;
mod settings;

pub use refresh::{warden, WARDEN_ARG};
pub use server::{serve, AgentTiming, AppState};
pub mod mcp;
pub mod perms;
pub mod session;
pub mod spaces;
mod web;
