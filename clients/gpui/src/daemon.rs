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
    url: String,
    token_file: PathBuf,
}
impl Endpoint {
    /// `url` is the daemon's WebSocket URL; the token comes from `token_file`.
    pub fn new(url: &str, token_file: &PathBuf) -> Result<Self> {
        let token = std::fs::read_to_string(token_file).with_context(|| format!("reading {}", token_file.display()))?.trim().to_owned();
        let http = url.replacen("ws://", "http://", 1).replacen("wss://", "https://", 1);
        let http = http.trim_end_matches("/ws").to_owned();
        Ok(Self { ws: format!("{url}?token={token}"), http, token, url: url.to_owned(), token_file: token_file.clone() })
    }
    /// The same endpoint with the token file read again: wheeld writes a new token when it
    /// starts without one, so a reconnect must not reuse the old text.
    pub fn reread(&self) -> Result<Self> {
        Self::new(&self.url, &self.token_file)
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

/// What the connection tells the client, in order.
#[derive(Debug)]
pub enum Event {
    /// A frame from wheeld.
    Message(Value),
    /// A socket is open and wheeld answered `status` with a protocol this client speaks. The
    /// endpoint carries the token read for this attempt.
    Up(Endpoint),
    /// The socket closed or could not open; the connection retries on its own. Sent once until
    /// the cause changes or the link comes up.
    Down(String),
    /// wheeld answered with a protocol older than the one this client needs.
    Incompatible { server: u32, client: u32 },
}

/// The protocol the client needs: ops it sends and fields it reads exist from this version on,
/// the rule the `wheel` command applies (`cli/src/main.rs`).
pub const PROTOCOL: u32 = wheel_proto::PROTOCOL_VERSION;

/// Pause between attempts: doubles from the first to the last, then stays.
const BACKOFF: [Duration; 4] = [Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1), Duration::from_secs(2)];

pub struct Connection {
    pub send: mpsc::Sender<String>,
    pub incoming: fmpsc::UnboundedReceiver<Event>,
}
impl Connection {
    /// Connect and ask for the layout; wheeld then broadcasts every change. When the socket
    /// closes, a daemon restart for instance, the connection reads the token file again and
    /// retries with a growing pause until wheeld answers, then asks for the layout again.
    /// A frame sent while the link is not up is dropped, so the view sends only while it is `Up`.
    pub fn open(endpoint: Endpoint) -> Self {
        let (send, outgoing) = mpsc::channel::<String>();
        let (events, incoming) = fmpsc::unbounded();
        std::thread::spawn(move || supervise(endpoint, &outgoing, &events));
        Self { send, incoming }
    }
}

fn supervise(first: Endpoint, outgoing: &mpsc::Receiver<String>, events: &fmpsc::UnboundedSender<Event>) {
    let (mut endpoint, mut failures, mut announced) = (first, 0usize, None::<String>);
    loop {
        match session(&endpoint, outgoing, events) {
            Ended::ClientGone => return,
            Ended::Lost { was_up, reason } => {
                if was_up {
                    failures = 0;
                    announced = None;
                }
                // One event per kind of outage: a changed cause (not reachable, then too old, then
                // not reachable again) is announced, a repeat of it is not.
                let key = match &reason {
                    Event::Incompatible { server, .. } => format!("old {server}"),
                    _ => "down".to_owned(),
                };
                if announced.as_deref() != Some(key.as_str()) {
                    announced = Some(key);
                    if events.unbounded_send(reason).is_err() {
                        return;
                    }
                }
            }
        }
        std::thread::sleep(BACKOFF[failures.min(BACKOFF.len() - 1)]);
        failures += 1;
        if matches!(outgoing.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
            return;
        }
        match endpoint.reread() {
            Ok(fresh) => endpoint = fresh,
            Err(error) => {
                if announced.as_deref() != Some("down") {
                    announced = Some("down".to_owned());
                    if events.unbounded_send(Event::Down(format!("{error:#}"))).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

enum Ended {
    ClientGone,
    Lost { was_up: bool, reason: Event },
}

/// One socket from open to close.
fn session(endpoint: &Endpoint, outgoing: &mpsc::Receiver<String>, events: &fmpsc::UnboundedSender<Event>) -> Ended {
    let lost = |was_up, why: String| Ended::Lost { was_up, reason: Event::Down(why) };
    let mut ws = match socket(&endpoint.ws) {
        Ok(ws) => ws,
        Err(error) => return lost(false, format!("{error:#}")),
    };
    while outgoing.try_recv().is_ok() {}
    if ws.send(Message::text(json!({"op":"status","req":1}).to_string())).is_err() {
        return lost(false, "wheeld closed the socket".into());
    }
    let mut up = false;
    loop {
        loop {
            match outgoing.try_recv() {
                Ok(text) => {
                    if up && ws.send(Message::text(text)).is_err() {
                        return lost(true, "wheeld closed the socket".into());
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ended::ClientGone,
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                if !up && v["ev"] == "status" {
                    let server = v["protocol"].as_u64().map_or(1, |p| u32::try_from(p).unwrap_or(u32::MAX));
                    if server < PROTOCOL {
                        return Ended::Lost { was_up: false, reason: Event::Incompatible { server, client: PROTOCOL } };
                    }
                    up = true;
                    if events.unbounded_send(Event::Up(endpoint.clone())).is_err() {
                        return Ended::ClientGone;
                    }
                }
                if up && events.unbounded_send(Event::Message(v)).is_err() {
                    return Ended::ClientGone;
                }
            }
            Ok(Message::Close(_)) => return lost(up, "wheeld closed the socket".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(error) => return lost(up, format!("{error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::{Arc, Mutex}};

    /// A stand-in wheeld: for each scripted connection, record the token in the URL, answer the
    /// first frame with a `status` at `protocol`, then either close (`hold` false) or keep reading.
    fn serve(script: Vec<(u32, bool)>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tokens = Arc::new(Mutex::new(vec![]));
        let seen = tokens.clone();
        std::thread::spawn(move || {
            for (protocol, hold) in script {
                let (stream, _) = listener.accept().unwrap();
                let seen = seen.clone();
                let mut ws = tungstenite::accept_hdr(stream, move |request: &tungstenite::handshake::server::Request, response| {
                    seen.lock().unwrap().push(request.uri().query().unwrap_or_default().to_owned());
                    Ok(response)
                })
                .unwrap();
                ws.read().unwrap();
                ws.send(Message::text(json!({"ev":"status","protocol":protocol}).to_string())).unwrap();
                ws.send(Message::text(json!({"ev":"marker","n":protocol}).to_string())).unwrap();
                if hold {
                    while ws.read().is_ok() {}
                }
            }
        });
        (port, tokens)
    }
    fn next(connection: &mut Connection) -> Event {
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < end {
            if let Ok(event) = connection.incoming.try_recv() {
                return event;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no event in 15 s");
    }
    fn endpoint(port: u16, token: &str) -> (Endpoint, PathBuf) {
        let file = std::env::temp_dir().join(format!("wheel-gpui-token-{port}"));
        std::fs::write(&file, token).unwrap();
        (Endpoint::new(&format!("ws://127.0.0.1:{port}/ws"), &file).unwrap(), file)
    }

    #[test]
    fn reconnects_after_the_socket_closes_and_rereads_the_token() {
        let (port, tokens) = serve(vec![(PROTOCOL, false), (PROTOCOL, true)]);
        let (endpoint, file) = endpoint(port, "one");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(e) if e.token == "one"));
        assert!(matches!(next(&mut connection), Event::Message(m) if m["ev"] == "status"));
        assert!(matches!(next(&mut connection), Event::Message(m) if m["ev"] == "marker"));
        assert!(matches!(next(&mut connection), Event::Down(_)));
        std::fs::write(&file, "two").unwrap();
        assert!(matches!(next(&mut connection), Event::Up(e) if e.token == "two"));
        assert_eq!(*tokens.lock().unwrap(), ["token=one", "token=two"]);
    }

    #[test]
    fn an_older_protocol_is_reported_once_and_a_restart_to_the_new_one_connects() {
        let (port, _) = serve(vec![(1, false), (1, false), (PROTOCOL, true)]);
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Incompatible { server: 1, client } if client == PROTOCOL));
        // The second old answer announces nothing; the next event is the new server's Up.
        assert!(matches!(next(&mut connection), Event::Up(_)));
    }

    #[test]
    fn a_daemon_that_is_not_there_is_announced_once_and_connected_when_it_starts() {
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Down(_)));
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.read().unwrap();
            ws.send(Message::text(json!({"ev":"status","protocol":PROTOCOL}).to_string())).unwrap();
            while ws.read().is_ok() {}
        });
        assert!(matches!(next(&mut connection), Event::Up(_)));
    }

    #[test]
    fn an_unreachable_daemon_that_starts_old_is_reported_as_old() {
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Down(_)));
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        std::thread::spawn(move || loop {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.read().unwrap();
            ws.send(Message::text(json!({"ev":"status","protocol":1}).to_string())).unwrap();
        });
        assert!(matches!(next(&mut connection), Event::Incompatible { server: 1, .. }));
    }
}
