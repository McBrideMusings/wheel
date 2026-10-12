//! `wheeld`: holds every terminal and session, and serves clients over a WebSocket at `/ws`
//! and the lead's tools over MCP at `/mcp`.

pub mod pty;
mod server;

pub use server::{serve, AppState};
pub mod lead;
pub mod mcp;
pub mod perms;
pub mod spaces;
