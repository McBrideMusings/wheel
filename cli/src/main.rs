//! `wheel`: drive a running `wheeld` from the command line. Every command speaks the same
//! WebSocket protocol the apps use, so anything a client can do, a script can check.
//!
//! Exit codes: 0 success, 2 bad arguments, 3 timed out before `--until` matched,
//! 4 the terminal exited before `--until` matched, 5 could not connect or authenticate,
//! 6 the daemon refused the request, 1 anything else.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{decode_bytes, encode_bytes, ClientMsg, LeadEvent, ServerMsg};

#[derive(Parser)]
#[command(name = "wheel", about = "Drive a running wheeld")]
struct Cli {
    /// Daemon WebSocket URL.
    #[arg(long, global = true, default_value_t = format!("ws://{}/ws", wheel_proto::DEFAULT_LISTEN))]
    url: String,
    /// File holding the daemon's client token. Defaults to ~/Library/Application Support/wheel/token.
    #[arg(long, global = true)]
    token_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the daemon's state as JSON.
    Status,
    /// Terminals.
    #[command(subcommand)]
    Term(TermCommand),
    /// The lead agent.
    #[command(subcommand)]
    Lead(LeadCommand),
    /// Permission requests waiting on a person.
    #[command(subcommand)]
    Perm(PermCommand),
}

#[derive(Subcommand)]
enum LeadCommand {
    /// Send the lead a message and print its reply as it streams.
    Send {
        text: String,
        /// Seconds to wait for the reply to finish (0 to 3600).
        #[arg(long, default_value_t = 300.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the conversation so far.
    Read,
    /// Stop the lead's current turn.
    Cancel,
}

#[derive(Subcommand)]
enum PermCommand {
    /// Print waiting permission requests as JSON.
    List,
    /// Answer one: `allow`, `deny`, or one of its option ids.
    Answer { id: String, choice: String },
    /// Wait for a permission request and print it as JSON (an already-waiting one counts).
    Wait {
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Start a shell and print its id.
    Open {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 32)]
        rows: u16,
    },
    /// Type text into a terminal.
    Send {
        id: String,
        text: String,
        /// Press Return after the text.
        #[arg(long)]
        enter: bool,
    },
    /// Print a terminal's buffered output, then live output until --until appears or --timeout passes.
    Read {
        id: String,
        /// Stop when this text appears; exit 3 if it never does, 4 if the terminal exits first.
        #[arg(long)]
        until: Option<String>,
        /// Skip the buffered output: only output produced after this command attached counts.
        #[arg(long)]
        new: bool,
        /// Seconds to keep reading live output (0 to 3600).
        #[arg(long, default_value_t = 1.0, value_parser = parse_timeout)]
        timeout: f64,
    },
}

fn parse_timeout(s: &str) -> Result<f64, String> {
    let secs: f64 = s
        .parse()
        .map_err(|_| format!("{s:?} is not a number of seconds"))?;
    if (0.0..=3600.0).contains(&secs) {
        Ok(secs)
    } else {
        Err("timeout must be between 0 and 3600 seconds".into())
    }
}

/// A failure, carrying the exit code a script can branch on.
#[derive(Debug)]
enum Fail {
    Timeout(String),
    Exited(Option<u32>),
    Connect(String),
    Daemon(String),
    Other(String),
}

impl Fail {
    fn code(&self) -> u8 {
        match self {
            Fail::Other(_) => 1,
            Fail::Timeout(_) => 3,
            Fail::Exited(_) => 4,
            Fail::Connect(_) => 5,
            Fail::Daemon(_) => 6,
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Timeout(m) | Fail::Connect(m) | Fail::Other(m) => f.write_str(m),
            Fail::Exited(code) => write!(
                f,
                "terminal exited (code {code:?}) before the text appeared"
            ),
            Fail::Daemon(m) => write!(f, "daemon: {m}"),
        }
    }
}

fn other(e: impl std::fmt::Display) -> Fail {
    Fail::Other(e.to_string())
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Conn {
    ws: Ws,
    next_req: u64,
    /// Pushed events that arrived while waiting for a reply, delivered by the next `recv`.
    events: std::collections::VecDeque<ServerMsg>,
}

impl Conn {
    async fn open(url: &str, token_file: Option<PathBuf>) -> Result<Conn, Fail> {
        let path = match token_file {
            Some(path) => path,
            None => wheel_proto::default_token_path()
                .map_err(|_| Fail::Connect("HOME is not set; pass --token-file".into()))?,
        };
        let token = std::fs::read_to_string(&path)
            .map_err(|e| Fail::Connect(format!("read token from {}: {e}", path.display())))?;
        let (ws, _) = tokio_tungstenite::connect_async(format!("{url}?token={}", token.trim()))
            .await
            .map_err(|e| Fail::Connect(format!("connect to {url}: {e}")))?;
        Ok(Conn {
            ws,
            next_req: 0,
            events: Default::default(),
        })
    }

    fn req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    async fn send(&mut self, msg: &ClientMsg) -> Result<(), Fail> {
        let text = serde_json::to_string(msg).map_err(other)?;
        self.ws
            .send(Message::Text(text.into()))
            .await
            .map_err(other)
    }

    /// The next daemon message, or None if `wait` passes first.
    async fn recv(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        match self.events.pop_front() {
            Some(event) => Ok(Some(event)),
            None => self.recv_frame(wait).await,
        }
    }

    /// Send a request and wait for the reply carrying its `req`.
    async fn call(&mut self, msg: ClientMsg, req: u64) -> Result<ServerMsg, Fail> {
        self.send(&msg).await?;
        let mut held = std::collections::VecDeque::new();
        let result = loop {
            let Some(reply) = self.recv_frame(Duration::from_secs(10)).await? else {
                break Err(Fail::Timeout("no reply from daemon within 10s".into()));
            };
            match &reply {
                ServerMsg::Error {
                    req: Some(r),
                    message,
                } if *r == req => break Err(Fail::Daemon(message.clone())),
                ServerMsg::Ok { req: r }
                | ServerMsg::Status { req: Some(r), .. }
                | ServerMsg::TermOpened { req: Some(r), .. }
                | ServerMsg::TermAttached { req: Some(r), .. }
                | ServerMsg::LeadAttached { req: Some(r), .. }
                | ServerMsg::LeadQueued { req: r, .. }
                    if *r == req =>
                {
                    break Ok(reply)
                }
                _ => held.push_back(reply),
            }
        };
        self.events.extend(held);
        result
    }

    /// The next frame off the socket, skipping the queue `recv` drains first.
    async fn recv_frame(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(Fail::Other("daemon closed the connection".into())),
                Ok(Some(frame)) => {
                    if let Message::Text(text) = frame.map_err(other)? {
                        return serde_json::from_str(&text).map(Some).map_err(other);
                    }
                }
            }
        }
    }
}

async fn status(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let reply = conn.call(ClientMsg::Status { req: Some(req) }, req).await?;
    println!("{}", serde_json::to_string_pretty(&reply).map_err(other)?);
    Ok(())
}

async fn term_open(conn: &mut Conn, cwd: Option<String>, cols: u16, rows: u16) -> Result<(), Fail> {
    let req = conn.req();
    match conn
        .call(
            ClientMsg::TermOpen {
                req: Some(req),
                cwd,
                cols,
                rows,
            },
            req,
        )
        .await?
    {
        ServerMsg::TermOpened { id, .. } => {
            println!("{id}");
            Ok(())
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

async fn term_send(conn: &mut Conn, id: String, text: String, enter: bool) -> Result<(), Fail> {
    let mut bytes = text.into_bytes();
    if enter {
        bytes.push(b'\r');
    }
    let req = conn.req();
    conn.call(
        ClientMsg::TermInput {
            req: Some(req),
            id,
            data: encode_bytes(&bytes),
        },
        req,
    )
    .await?;
    Ok(())
}

async fn term_read(
    conn: &mut Conn,
    id: String,
    until: Option<String>,
    new: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    let first = conn
        .call(ClientMsg::TermAttach { req: Some(req), id }, req)
        .await?;
    // Bytes of replay still to arrive; with --new they are dropped rather than printed.
    let mut replay_left = match first {
        ServerMsg::TermAttached { replay_bytes, .. } if new => replay_bytes,
        _ => 0,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    // Bytes, not text: a character split across two frames still matches.
    let mut seen: Vec<u8> = Vec::new();
    let needle = until.as_ref().map(|n| n.as_bytes());
    let mut out = std::io::stdout();
    loop {
        // Always drain what is already queued, even with --timeout 0.
        let left = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(Duration::from_millis(20));
        match conn.recv(left).await? {
            None => {
                return match &until {
                    Some(n) => Err(Fail::Timeout(format!("timed out before {n:?} appeared"))),
                    None => Ok(()),
                }
            }
            Some(ServerMsg::TermOutput { data, .. }) => {
                let mut bytes = decode_bytes(&data).map_err(other)?;
                let skip = replay_left.min(bytes.len());
                replay_left -= skip;
                bytes.drain(..skip);
                out.write_all(&bytes).map_err(other)?;
                out.flush().map_err(other)?;
                seen.extend_from_slice(&bytes);
                if let Some(n) = needle {
                    if seen.windows(n.len().max(1)).any(|w| w == n) {
                        return Ok(());
                    }
                    // Only the tail can still complete a match.
                    let keep = n.len().saturating_sub(1);
                    if seen.len() > keep {
                        seen.drain(..seen.len() - keep);
                    }
                }
            }
            Some(ServerMsg::TermAttached { replay_bytes, .. }) => {
                // The daemon dropped output to this connection and is replaying; start over.
                eprintln!("\n[output resynced; replaying {replay_bytes} bytes]");
                seen.clear();
                replay_left = if new { replay_bytes } else { 0 };
            }
            Some(ServerMsg::TermExited { code, .. }) => {
                eprintln!("\n[terminal exited: {code:?}]");
                return match until {
                    Some(_) => Err(Fail::Exited(code)),
                    None => Ok(()),
                };
            }
            Some(_) => {}
        }
        if tokio::time::Instant::now() >= deadline && until.is_none() {
            return Ok(());
        }
    }
}

fn print_perm_hint(perm: &wheel_proto::PermInfo) {
    eprintln!(
        "\n[waiting on you] {}: {} wants {:?} -- wheel perm answer {} allow|deny",
        perm.id, perm.session, perm.title, perm.id
    );
}

async fn lead_send(conn: &mut Conn, text: String, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(ClientMsg::LeadAttach { req: Some(req) }, req)
        .await?;
    let req = conn.req();
    let ServerMsg::LeadQueued { turn, .. } = conn
        .call(
            ClientMsg::LeadSend {
                req: Some(req),
                text,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to lead_send".into()));
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    let mut out = std::io::stdout();
    // Our turn may wait behind others; it starts with the `user` event carrying its id.
    let mut started = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = conn.recv(left).await? else {
            return Err(Fail::Timeout(format!(
                "the lead did not finish within {timeout}s"
            )));
        };
        match msg {
            ServerMsg::Lead { event } => match event {
                LeadEvent::User { turn: t, .. } if t == turn => started = true,
                LeadEvent::Text { text } if started => {
                    out.write_all(text.as_bytes()).map_err(other)?;
                    out.flush().map_err(other)?;
                }
                LeadEvent::ToolCall { title, status, .. } if started => {
                    eprintln!("\n[tool] {title} ({status})")
                }
                LeadEvent::TurnEnd {
                    turn: t,
                    stop_reason,
                } if t == turn => {
                    println!();
                    if stop_reason != "end_turn" {
                        eprintln!("[turn ended: {stop_reason}]");
                    }
                    return Ok(());
                }
                LeadEvent::Error {
                    turn: Some(t),
                    message,
                } if t == turn => return Err(Fail::Daemon(message)),
                // The agent process stopped. A turn in flight is lost; a queued one runs after the restart.
                LeadEvent::Error {
                    turn: None,
                    message,
                } if started => return Err(Fail::Daemon(message)),
                _ => {}
            },
            ServerMsg::PermRequested { perm } => print_perm_hint(&perm),
            _ => {}
        }
    }
}

async fn lead_read(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::LeadAttached { state, events, .. } = conn
        .call(ClientMsg::LeadAttach { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to lead_attach".into()));
    };
    for event in events {
        match event {
            LeadEvent::User { text, .. } => println!("\n> {text}"),
            LeadEvent::Text { text } => println!("{text}"),
            LeadEvent::ToolCall { title, status, .. } => println!("[tool] {title} ({status})"),
            LeadEvent::Error { message, .. } => println!("[error] {message}"),
            _ => {}
        }
    }
    eprintln!("[lead: {}]", serde_json::to_string(&state).map_err(other)?);
    Ok(())
}

async fn perm_list(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    println!("{}", serde_json::to_string_pretty(&perms).map_err(other)?);
    Ok(())
}

async fn perm_wait(conn: &mut Conn, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    if let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    {
        if let Some(perm) = perms.into_iter().next() {
            println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
            return Ok(());
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no permission request within {timeout}s"
                )))
            }
            Some(ServerMsg::PermRequested { perm }) => {
                println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

async fn simple(conn: &mut Conn, make: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(make(req), req).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = async {
        let mut conn = Conn::open(&cli.url, cli.token_file).await?;
        match cli.command {
            Command::Status => status(&mut conn).await,
            Command::Term(TermCommand::Open { cwd, cols, rows }) => {
                term_open(&mut conn, cwd, cols, rows).await
            }
            Command::Term(TermCommand::Send { id, text, enter }) => {
                term_send(&mut conn, id, text, enter).await
            }
            Command::Term(TermCommand::Read {
                id,
                until,
                new,
                timeout,
            }) => term_read(&mut conn, id, until, new, timeout).await,
            Command::Lead(LeadCommand::Send { text, timeout }) => {
                lead_send(&mut conn, text, timeout).await
            }
            Command::Lead(LeadCommand::Read) => lead_read(&mut conn).await,
            Command::Lead(LeadCommand::Cancel) => {
                simple(&mut conn, |req| ClientMsg::LeadCancel { req: Some(req) }).await
            }
            Command::Perm(PermCommand::List) => perm_list(&mut conn).await,
            Command::Perm(PermCommand::Answer { id, choice }) => {
                simple(&mut conn, |req| ClientMsg::PermAnswer {
                    req: Some(req),
                    id,
                    choice,
                })
                .await
            }
            Command::Perm(PermCommand::Wait { timeout }) => perm_wait(&mut conn, timeout).await,
        }
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => {
            eprintln!("wheel: {fail}");
            ExitCode::from(fail.code())
        }
    }
}
