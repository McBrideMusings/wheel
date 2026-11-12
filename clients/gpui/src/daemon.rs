//! The connection to wheeld: one thread owns the WebSocket, reads frames into a channel and
//! writes whatever the client queued. The same socket the `wheel` CLI and the Swift clients use,
//! and the same wire types: every frame is a `ServerMsg` or a `ClientMsg` of the proto crate.
use anyhow::{Context as _, Result};
use futures::channel::mpsc as fmpsc;
use serde::Deserialize;
use std::{net::TcpStream, path::PathBuf, sync::mpsc, time::Duration};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};
use wheel_proto::{ClientMsg, ServerMsg};

/// Where wheeld listens and the token it wants.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// `ws://host:port/ws?token=…`
    pub ws: String,
    pub token: String,
    url: String,
    token_file: PathBuf,
    /// The bundled wheeld this endpoint names, when it came from the bundle and not from a URL:
    /// the window's server control is its launch agent (`crate::server::Control::for_service`).
    pub service: Option<crate::server::Service>,
}
impl Endpoint {
    /// `url` is the daemon's WebSocket URL; the token comes from `token_file`.
    pub fn new(url: &str, token_file: &PathBuf) -> Result<Self> {
        let token = std::fs::read_to_string(token_file).with_context(|| format!("reading {}", token_file.display()))?.trim().to_owned();
        Ok(Self { ws: format!("{url}?token={token}"), token, url: url.to_owned(), token_file: token_file.clone(), service: None })
    }
    /// The bundled service's endpoint; `token_file` overrides the service's own (`WHEEL_TOKEN_FILE`).
    pub fn for_service(service: crate::server::Service, token_file: Option<PathBuf>) -> Result<Self> {
        let mut endpoint = Self::new(&service.url(), &token_file.unwrap_or_else(|| service.token_file()))?;
        endpoint.service = Some(service);
        Ok(endpoint)
    }
    pub fn token_file(&self) -> &std::path::Path {
        &self.token_file
    }
    /// The same endpoint with the token file read again: wheeld writes a new token when it
    /// starts without one, so a reconnect must not reuse the old text.
    pub fn reread(&self) -> Result<Self> {
        let mut endpoint = Self::new(&self.url, &self.token_file)?;
        endpoint.service = self.service.clone();
        Ok(endpoint)
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

/// A `ClientMsg` as one text frame.
pub fn frame(message: &ClientMsg) -> String {
    serde_json::to_string(message).expect("a ClientMsg serializes")
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
    /// A frame from wheeld, boxed: a `status` reply is large and the other events are not.
    Message(Box<ServerMsg>),
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

/// The two fields of a `status` reply read before the rest: an older server's reply may lack
/// fields this build's `ServerMsg` requires, and its protocol number still has to be read to
/// say so.
#[derive(Deserialize)]
struct StatusHead {
    ev: String,
    #[serde(default = "first_protocol")]
    protocol: u32,
}
fn first_protocol() -> u32 {
    1
}

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
    if ws.send(Message::text(frame(&ClientMsg::Status { req: Some(1) }))).is_err() {
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
                if !up {
                    let Ok(head) = serde_json::from_str::<StatusHead>(&t) else { continue };
                    if head.ev != "status" {
                        continue;
                    }
                    if head.protocol < PROTOCOL {
                        return Ended::Lost { was_up: false, reason: Event::Incompatible { server: head.protocol, client: PROTOCOL } };
                    }
                    up = true;
                    if events.unbounded_send(Event::Up(endpoint.clone())).is_err() {
                        return Ended::ClientGone;
                    }
                }
                // A frame this build's proto crate cannot read (an event a newer server added) is
                // dropped: nothing in the client reads it.
                let Ok(message) = serde_json::from_str::<ServerMsg>(&t) else { continue };
                if events.unbounded_send(Event::Message(Box::new(message))).is_err() {
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
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::{net::TcpListener, sync::{Arc, Mutex}};

    /// A `status` reply at `protocol` with one space, as wheeld writes it.
    pub(crate) fn status_frame(protocol: u32) -> String {
        json!({"ev":"status","req":1,"version":"test","protocol":protocol,"uptime_secs":0,"log_path":"/dev/null","terms":[],"perms":[],
            "spaces":[{"id":"s1","name":"wheel","cwd":"/w","root":{"node":"pane","id":"p1","kind":"empty"}}],
            "sessions":[{"id":"n1","name":"Main","spaces":["s1"]}]}).to_string()
    }
    /// A `spaces` broadcast whose session is named `marker`, so a test can tell which
    /// connection's frames it reads.
    fn marker_frame(marker: u32) -> String {
        json!({"ev":"spaces","spaces":[],"sessions":[{"id":"n1","name":marker.to_string(),"spaces":[]}]}).to_string()
    }

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
                ws.send(Message::text(status_frame(protocol))).unwrap();
                ws.send(Message::text(marker_frame(protocol))).unwrap();
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
    fn the_first_frame_is_a_status_request() {
        assert_eq!(frame(&ClientMsg::Status { req: Some(1) }), r#"{"op":"status","req":1}"#);
        let back: ClientMsg = serde_json::from_str(&frame(&ClientMsg::SplitRatio { req: None, split: "x1".into(), ratio: 0.3 })).unwrap();
        assert_eq!(back, ClientMsg::SplitRatio { req: None, split: "x1".into(), ratio: 0.3 });
    }

    #[test]
    fn reconnects_after_the_socket_closes_and_rereads_the_token() {
        let (port, tokens) = serve(vec![(PROTOCOL, false), (PROTOCOL, true)]);
        let (endpoint, file) = endpoint(port, "one");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(e) if e.token == "one"));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { protocol, .. } if protocol == PROTOCOL)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(&*m, ServerMsg::Spaces { sessions, .. } if sessions[0].name == PROTOCOL.to_string())));
        assert!(matches!(next(&mut connection), Event::Down(_)));
        std::fs::write(&file, "two").unwrap();
        assert!(matches!(next(&mut connection), Event::Up(e) if e.token == "two"));
        assert_eq!(*tokens.lock().unwrap(), ["token=one", "token=two"]);
    }

    #[test]
    fn a_frame_with_an_event_kind_or_phase_this_build_does_not_know_still_arrives() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.read().unwrap();
            // A status whose agent pane is in a phase a newer wheeld added.
            let status = json!({"ev":"status","req":1,"version":"test","protocol":PROTOCOL,"uptime_secs":0,"log_path":"/dev/null","terms":[],"perms":[],
                "spaces":[{"id":"s1","name":"wheel","cwd":"/w","root":{"node":"pane","id":"p1","kind":"agent","agent":"a1","harness":"claude","state":{"phase":"dreaming"}}}],
                "sessions":[{"id":"n1","name":"Main","spaces":["s1"]}]});
            ws.send(Message::text(status.to_string())).unwrap();
            let attached = json!({"ev":"agent_attached","agent":"a1","now":0,"state":{"phase":"ready"},"events":[
                {"kind":"text","text":"before"},{"kind":"hologram","at":7,"frames":3},{"kind":"text","text":"after"}]});
            ws.send(Message::text(attached.to_string())).unwrap();
            while ws.read().is_ok() {}
        });
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        let Event::Message(status) = next(&mut connection) else { panic!("no status") };
        let ServerMsg::Status { spaces, .. } = *status else { panic!("not a status") };
        let wheel_proto::Node::Pane { kind: wheel_proto::PaneKind::Agent { state: Some(state), .. }, .. } = &spaces[0].root else { panic!("no agent pane: {:?}", spaces[0].root) };
        assert_eq!(state.phase(), "dreaming");
        let Event::Message(attached) = next(&mut connection) else { panic!("no agent_attached") };
        let ServerMsg::AgentAttached { events, .. } = *attached else { panic!("not agent_attached") };
        let kinds: Vec<_> = events.iter().map(|e| (e.at, serde_json::to_value(&e.event).unwrap()["kind"].clone())).collect();
        assert_eq!(kinds, [(None, json!("text")), (Some(7), json!("hologram")), (None, json!("text"))]);
        // Kept whole: written back exactly as it arrived.
        assert_eq!(serde_json::to_value(&events[1]).unwrap(), json!({"kind":"hologram","at":7,"frames":3}));
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
            ws.send(Message::text(status_frame(PROTOCOL))).unwrap();
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
            // An old server's status, with none of the fields later builds added.
            ws.send(Message::text(json!({"ev":"status","protocol":1}).to_string())).unwrap();
        });
        assert!(matches!(next(&mut connection), Event::Incompatible { server: 1, .. }));
    }
}
