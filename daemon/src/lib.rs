//! `wheeld`: holds every terminal and session, and serves clients over a WebSocket at `/ws`.

pub mod pty;
mod server;

pub use server::{serve, AppState};
