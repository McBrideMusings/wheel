//! `wheeld`: holds every terminal and session, and serves clients over a WebSocket at `/ws`
//! the agents' tools over MCP at `/mcp`, and web panes' folders at `/artifacts/<pane>/`.

mod agents;
mod artifacts;
mod beads;
mod brief;
mod escalations;
mod inbox;
pub mod lead;
mod login;
pub mod loops;
pub mod pty;
mod refresh;
mod render;
mod resume;
mod server;
mod settings;
pub mod supervisor_mcp;
pub mod supervisors;
pub mod usage;
mod views;

pub use refresh::{warden, WARDEN_ARG};
pub use server::{serve, AgentTiming, AppState};
pub mod mcp;
pub mod perms;
mod policy;
pub mod session;
pub mod spaces;
mod web;
