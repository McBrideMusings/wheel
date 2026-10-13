//! `wheeld`: holds every terminal and session, and serves clients over a WebSocket at `/ws`
//! the lead's tools over MCP at `/mcp`, and web panes' folders at `/artifacts/<pane>/`.

mod artifacts;
pub mod pty;
mod refresh;
mod server;
mod settings;

pub use server::{serve, AppState};
pub mod mcp;
pub mod perms;
pub mod session;
pub mod spaces;
mod web;
