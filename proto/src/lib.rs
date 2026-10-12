//! The wire protocol between `wheeld` and its clients: one JSON object per WebSocket text frame.
//!
//! A client connects to `ws://<host>/ws?token=<token>`; the token is the contents of the daemon's
//! token file, readable only by the user. Requests from a browser (anything carrying an `Origin`
//! header) are refused.
//!
//! A client sends [`ClientMsg`]; the daemon answers with [`ServerMsg`]. A request may carry a
//! `req` id, which the daemon copies onto the reply so a client can match them.
//! Terminal bytes travel base64-encoded.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

pub const DEFAULT_LISTEN: &str = "127.0.0.1:7420";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Ask for the daemon's state.
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
    },
    /// Start a shell. The daemon replies with `term_opened`; it does not attach the caller.
    TermOpen {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Stream a terminal's output to this connection. Attaching again replaces the earlier stream.
    TermAttach {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
    },
    /// Stop streaming a terminal to this connection. The terminal keeps running.
    TermDetach {
        id: String,
    },
    TermInput {
        id: String,
        data: String,
    },
    TermResize {
        id: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ServerMsg {
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        version: String,
        uptime_secs: u64,
        log_path: String,
        terms: Vec<TermInfo>,
    },
    TermOpened {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        pid: Option<u32>,
    },
    /// Reset the terminal view, then apply the `replay_bytes` of output that follow as
    /// `term_output`; live output comes after them. The daemon sends this on every attach and
    /// again whenever this connection fell behind and output was dropped. The replay is the
    /// terminal's most recent raw output, up to 1 MiB, not a rendered screen.
    TermAttached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        id: String,
        replay_bytes: usize,
    },
    TermOutput {
        id: String,
        data: String,
    },
    /// The shell exited. Also sent right after the replay when attaching to a terminal that had
    /// already exited.
    TermExited {
        id: String,
        code: Option<u32>,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TermInfo {
    pub id: String,
    pub pid: Option<u32>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
    pub buffered_bytes: usize,
}

pub fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_bytes(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_messages_use_snake_case_op_tags() {
        let msg = ClientMsg::TermOpen {
            req: Some(3),
            cwd: None,
            cols: 80,
            rows: 24,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"op":"term_open","req":3,"cols":80,"rows":24}"#);
        assert_eq!(serde_json::from_str::<ClientMsg>(&json).unwrap(), msg);
    }
}
