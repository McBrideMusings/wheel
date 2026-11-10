//! The connection to wheeld: one thread owns the WebSocket, reads frames into a channel and
//! writes whatever the client queued. The same socket the `wheel` CLI and the Swift clients use.
use anyhow::{Context as _, Result};
use futures::channel::mpsc as fmpsc;
use serde_json::{Value, json};
use std::{net::TcpStream, path::PathBuf, sync::mpsc, time::Duration};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

/// Where wheeld listens and the token it wants.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// `ws://host:port/ws?token=…`
    pub ws: String,
    /// `http://host:port`, for the artifact routes.
    pub http: String,
    pub token: String,
}
impl Endpoint {
    /// `url` is the daemon's WebSocket URL; the token comes from `token_file`.
    pub fn new(url: &str, token_file: &PathBuf) -> Result<Self> {
        let token = std::fs::read_to_string(token_file).with_context(|| format!("reading {}", token_file.display()))?.trim().to_owned();
        let http = url.replacen("ws://", "http://", 1).replacen("wss://", "https://", 1);
        let http = http.trim_end_matches("/ws").to_owned();
        Ok(Self { ws: format!("{url}?token={token}"), http, token })
    }
    /// The page a web pane shows.
    pub fn artifact(&self, pane: &str) -> String {
        format!("{}/artifacts/{pane}/?token={}", self.http, self.token)
    }
}

/// What the CLI defaults to when no URL or token file is given.
pub fn default_url() -> String {
    std::env::var("WHEEL_URL").unwrap_or_else(|_| "ws://127.0.0.1:7420/ws".into())
}
pub fn default_token_file() -> PathBuf {
    std::env::var_os("WHEEL_TOKEN_FILE").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/wheel/token")
    })
}

fn socket(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    let (ws, _) = tungstenite::connect(url).with_context(|| format!("connecting to wheeld at {url}"))?;
    if let MaybeTlsStream::Plain(stream) = ws.get_ref() {
        stream.set_read_timeout(Some(Duration::from_millis(30)))?;
    }
    Ok(ws)
}

pub struct Connection {
    pub send: mpsc::Sender<String>,
    pub incoming: fmpsc::UnboundedReceiver<Value>,
}
impl Connection {
    /// Connect and ask for the layout; wheeld then broadcasts every change.
    pub fn open(endpoint: &Endpoint) -> Result<Self> {
        let mut ws = socket(&endpoint.ws)?;
        let (send, outgoing) = mpsc::channel::<String>();
        let (in_tx, incoming) = fmpsc::unbounded();
        send.send(json!({"op":"status","req":1}).to_string())?;
        std::thread::spawn(move || loop {
            while let Ok(text) = outgoing.try_recv() {
                if ws.send(Message::text(text)).is_err() {
                    return;
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        if in_tx.unbounded_send(v).is_err() {
                            return;
                        }
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return,
            }
        });
        Ok(Self { send, incoming })
    }
}
