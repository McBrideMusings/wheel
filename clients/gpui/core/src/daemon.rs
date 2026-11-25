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

/// The socket, connected and then made non-blocking: the connection thread sleeps in `poll` on it
/// and on its wake pipe, and reads or writes only what is ready.
fn socket(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    let (ws, _) = tungstenite::connect(url).with_context(|| format!("connecting to wheeld at {url}"))?;
    match ws.get_ref() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true)?;
            // Keystrokes and replies are small frames: Nagle's coalescing would only hold them back.
            stream.set_nodelay(true)?;
        }
        _ => anyhow::bail!("wheeld at {url} is not a plain socket"),
    }
    Ok(ws)
}

/// Where the client queues frames for wheeld. Each one also wakes the connection thread, which
/// otherwise sleeps until wheeld sends something.
#[derive(Clone)]
pub struct Outbox {
    frames: mpsc::Sender<String>,
    /// None for an outbox no connection reads ([`Outbox::detached`]).
    wake: Option<std::sync::Arc<std::os::unix::net::UnixStream>>,
}

impl Outbox {
    pub fn send(&self, text: String) -> Result<(), mpsc::SendError<String>> {
        self.frames.send(text)?;
        // Non-blocking: a full pipe already holds a wake-up the thread has not read.
        if let Some(wake) = &self.wake {
            let _ = std::io::Write::write(&mut &**wake, &[1]);
        }
        Ok(())
    }

    /// An outbox with no connection behind it, and the frames sent to it: for a client a test
    /// drives without wheeld.
    pub fn detached() -> (Self, mpsc::Receiver<String>) {
        let (frames, sent) = mpsc::channel();
        (Self { frames, wake: None }, sent)
    }
}

/// What the connection tells the client, in order.
#[derive(Debug)]
pub enum Event {
    /// A frame from wheeld, boxed: a `status` reply is large and the other events are not.
    Message(Box<ServerMsg>),
    /// A socket is open and wheeld answered `status` at this client's compatibility level. The
    /// endpoint carries the token read for this attempt.
    Up(Endpoint),
    /// The socket closed or could not open; the connection retries on its own. Sent once until
    /// the cause changes or the link comes up.
    Down(String),
    /// wheeld answered at another compatibility level (`compat`), so its frames are not read.
    Incompatible { server: u32, client: u32 },
    /// A frame was not sent because its op is not one wheeld listed: a feature sent it without
    /// checking `Offer::has` first. The connection answers the request itself with an `error`
    /// carrying `req`, as wheeld would.
    Refused { op: String },
}

/// Pause between attempts: doubles from the first to the last, then stays.
const BACKOFF: [Duration; 4] = [Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1), Duration::from_secs(2)];

pub struct Connection {
    pub send: Outbox,
    pub incoming: fmpsc::UnboundedReceiver<Event>,
}
impl Connection {
    /// Connect and ask for the layout; wheeld then broadcasts every change. When the socket
    /// closes, a daemon restart for instance, the connection reads the token file again and
    /// retries with a growing pause until wheeld answers, then asks for the layout again.
    /// A frame sent while the link is not up is dropped, so the view sends only while it is `Up`.
    pub fn open(endpoint: Endpoint) -> Self {
        let (frames, outgoing) = mpsc::channel::<String>();
        let (wake, woken) = std::os::unix::net::UnixStream::pair().expect("a socket pair for the connection's wake-ups");
        wake.set_nonblocking(true).expect("a non-blocking wake socket");
        woken.set_nonblocking(true).expect("a non-blocking wake socket");
        let (events, incoming) = fmpsc::unbounded();
        std::thread::spawn(move || supervise(endpoint, &outgoing, &woken, &events));
        Self { send: Outbox { frames, wake: Some(std::sync::Arc::new(wake)) }, incoming }
    }
}

fn supervise(first: Endpoint, outgoing: &mpsc::Receiver<String>, woken: &std::os::unix::net::UnixStream, events: &fmpsc::UnboundedSender<Event>) {
    let (mut endpoint, mut failures, mut announced) = (first, 0usize, None::<String>);
    loop {
        match session(&endpoint, outgoing, woken, events) {
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

/// One socket from open to close. Every frame the client queues passes the offer of the wheeld
/// at the other end (`Offer::admit`): one whose op it did not list is answered here with an
/// `error` and never sent.
fn session(endpoint: &Endpoint, outgoing: &mpsc::Receiver<String>, woken: &std::os::unix::net::UnixStream, events: &fmpsc::UnboundedSender<Event>) -> Ended {
    let lost = |was_up, why: String| Ended::Lost { was_up, reason: Event::Down(why) };
    let would_block = |e: &tungstenite::Error| matches!(e, tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock);
    let mut ws = match socket(&endpoint.ws) {
        Ok(ws) => ws,
        Err(error) => return lost(false, format!("{error:#}")),
    };
    while outgoing.try_recv().is_ok() {}
    // A frame the socket could not take at once waits in tungstenite's buffer until it can.
    let mut unsent = false;
    match ws.send(Message::text(frame(&ClientMsg::Status { req: Some(1) }))) {
        Ok(()) => {}
        Err(e) if would_block(&e) => unsent = true,
        Err(_) => return lost(false, "wheeld closed the socket".into()),
    }
    let mut offer: Option<wheel_proto::Offer> = None;
    let socket_fd = match ws.get_ref() {
        MaybeTlsStream::Plain(stream) => std::os::fd::AsRawFd::as_raw_fd(stream),
        _ => return lost(false, "wheeld is not a plain socket".into()),
    };
    loop {
        // Sleep until wheeld sends, the client queues a frame, or a held frame can go out.
        let mut fds = [
            libc::pollfd { fd: socket_fd, events: libc::POLLIN | if unsent { libc::POLLOUT } else { 0 }, revents: 0 },
            libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(woken), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: two pollfds owned by this frame, for descriptors this session keeps open.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return lost(offer.is_some(), format!("waiting on wheeld's socket: {}", std::io::Error::last_os_error()));
        }
        let mut drain = [0u8; 64];
        while matches!(std::io::Read::read(&mut &*woken, &mut drain), Ok(n) if n > 0) {}
        if unsent {
            match ws.flush() {
                Ok(()) => unsent = false,
                Err(e) if would_block(&e) => {}
                Err(_) => return lost(offer.is_some(), "wheeld closed the socket".into()),
            }
        }
        loop {
            match outgoing.try_recv() {
                Ok(text) => {
                    let Some(offer) = &offer else { continue };
                    if let Err(refused) = offer.admit(&text) {
                        if refuse(events, &text, refused).is_err() {
                            return Ended::ClientGone;
                        }
                        continue;
                    }
                    match ws.send(Message::text(text)) {
                        Ok(()) => {}
                        Err(e) if would_block(&e) => unsent = true,
                        Err(_) => return lost(true, "wheeld closed the socket".into()),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ended::ClientGone,
            }
        }
        // Every frame that has arrived, those tungstenite already holds included: poll would not
        // wake for a frame read into its buffer along with the one before it.
        loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    // A frame this build's proto crate cannot read (an event a newer server added) is
                    // dropped: nothing in the client reads it.
                    let message = serde_json::from_str::<ServerMsg>(&t).ok();
                    // Every `status` names what wheeld offers; the first decides whether the link comes
                    // up. A status from another level may not read as this build's `ServerMsg`, so
                    // before the link is up its offer is read on its own.
                    let fresh = match &message {
                        Some(message) => message.offer(),
                        None if offer.is_none() => wheel_proto::Offer::read(&t),
                        None => None,
                    };
                    if let Some(fresh) = fresh {
                        if let Err(mismatch) = fresh.check() {
                            return Ended::Lost { was_up: offer.is_some(), reason: Event::Incompatible { server: mismatch.server, client: mismatch.client } };
                        }
                        let first = offer.is_none();
                        offer = Some(fresh);
                        if first && events.unbounded_send(Event::Up(endpoint.clone())).is_err() {
                            return Ended::ClientGone;
                        }
                    } else if offer.is_none() {
                        continue;
                    }
                    let Some(message) = message else { continue };
                    if events.unbounded_send(Event::Message(Box::new(message))).is_err() {
                        return Ended::ClientGone;
                    }
                }
                Ok(Message::Close(_)) => return lost(offer.is_some(), "wheeld closed the socket".into()),
                Ok(_) => {}
                Err(e) if would_block(&e) => break,
                Err(error) => return lost(offer.is_some(), format!("{error}")),
            }
        }
    }
}

/// Tell the client a frame was refused, and answer its request with the `error` wheeld would send.
fn refuse(events: &fmpsc::UnboundedSender<Event>, text: &str, refused: wheel_proto::Refused) -> Result<(), ()> {
    #[derive(Deserialize)]
    struct Req {
        req: Option<u64>,
    }
    let req = serde_json::from_str::<Req>(text).ok().and_then(|r| r.req);
    let message = refused.to_string();
    events.unbounded_send(Event::Refused { op: refused.op }).map_err(drop)?;
    events.unbounded_send(Event::Message(Box::new(ServerMsg::Error { req, term: None, message }))).map_err(drop)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use wheel_proto::COMPAT;
    use serde_json::json;
    use std::{net::TcpListener, sync::{Arc, Mutex}};

    /// A `status` reply at compatibility level `compat`, listing every op, with one space, as
    /// wheeld writes it.
    pub(crate) fn status_frame(compat: u32) -> String {
        json!({"ev":"status","req":1,"version":"test","protocol":wheel_proto::PROTOCOL_VERSION,"compat":compat,"ops":wheel_proto::ops(),"uptime_secs":0,"log_path":"/dev/null","terms":[],"perms":[],
            "spaces":[{"id":"s1","name":"wheel","cwd":"/w","root":{"node":"pane","id":"p1","kind":"empty"}}],
            "sessions":[{"id":"n1","name":"Main","spaces":["s1"]}]}).to_string()
    }
    /// A `spaces` broadcast whose session is named `marker`, so a test can tell which
    /// connection's frames it reads.
    fn marker_frame(marker: u32) -> String {
        json!({"ev":"spaces","spaces":[],"sessions":[{"id":"n1","name":marker.to_string(),"spaces":[]}]}).to_string()
    }

    /// A stand-in wheeld: for each scripted connection, record the token in the URL, answer the
    /// first frame with a `status` at level `compat`, then either close (`hold` false) or keep reading.
    fn serve(script: Vec<(u32, bool)>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tokens = Arc::new(Mutex::new(vec![]));
        let seen = tokens.clone();
        std::thread::spawn(move || {
            for (compat, hold) in script {
                let (stream, _) = listener.accept().unwrap();
                let seen = seen.clone();
                let mut ws = tungstenite::accept_hdr(stream, move |request: &tungstenite::handshake::server::Request, response| {
                    seen.lock().unwrap().push(request.uri().query().unwrap_or_default().to_owned());
                    Ok(response)
                })
                .unwrap();
                ws.read().unwrap();
                ws.send(Message::text(status_frame(compat))).unwrap();
                ws.send(Message::text(marker_frame(compat))).unwrap();
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
    fn the_socket_sends_small_frames_at_once() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            while ws.read().is_ok() {}
        });
        let ws = socket(&format!("ws://127.0.0.1:{port}/ws")).unwrap();
        let MaybeTlsStream::Plain(stream) = ws.get_ref() else { panic!("not a plain socket") };
        assert!(stream.nodelay().unwrap(), "Nagle is on");
    }

    #[test]
    fn an_idle_link_sends_a_queued_frame_at_once_and_still_hears_a_late_one() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (heard, heard_rx) = mpsc::channel::<(String, std::time::Instant)>();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.read().unwrap();
            ws.send(Message::text(status_frame(COMPAT))).unwrap();
            // Quiet, then a frame wheeld sends unasked.
            std::thread::sleep(Duration::from_millis(400));
            ws.send(Message::text(marker_frame(7))).unwrap();
            while let Ok(Message::Text(t)) = ws.read() {
                let _ = heard.send((t.to_string(), std::time::Instant::now()));
            }
        });
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { .. })));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(&*m, ServerMsg::Spaces { sessions, .. } if sessions[0].name == "7")), "the late frame never arrived");

        // Idle now: a queued frame goes out when it is queued, not at the next timer.
        std::thread::sleep(Duration::from_millis(200));
        let sent = std::time::Instant::now();
        connection.send.send(frame(&ClientMsg::Status { req: Some(9) })).unwrap();
        let (text, at) = heard_rx.recv_timeout(Duration::from_secs(5)).expect("the frame never reached wheeld");
        assert!(text.contains("\"req\":9"), "{text}");
        // Inside the 30 ms read timeout this replaced, with room for a loaded machine.
        assert!(at.duration_since(sent) < Duration::from_millis(25), "the frame waited {:?}", at.duration_since(sent));
    }

    #[test]
    fn reconnects_after_the_socket_closes_and_rereads_the_token() {
        let (port, tokens) = serve(vec![(COMPAT, false), (COMPAT, true)]);
        let (endpoint, file) = endpoint(port, "one");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(e) if e.token == "one"));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { compat, .. } if compat == COMPAT)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(&*m, ServerMsg::Spaces { sessions, .. } if sessions[0].name == COMPAT.to_string())));
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
            let status = json!({"ev":"status","req":1,"version":"test","protocol":wheel_proto::PROTOCOL_VERSION,"compat":COMPAT,"ops":wheel_proto::ops(),"uptime_secs":0,"log_path":"/dev/null","terms":[],"perms":[],
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
    fn an_older_compatibility_level_is_reported_once_and_a_restart_to_the_new_one_connects() {
        let (port, _) = serve(vec![(COMPAT - 1, false), (COMPAT - 1, false), (COMPAT, true)]);
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Incompatible { server, client } if server == COMPAT - 1 && client == COMPAT));
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
            ws.send(Message::text(status_frame(COMPAT))).unwrap();
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
        assert!(matches!(next(&mut connection), Event::Incompatible { server: 0, .. }));
    }

    #[test]
    fn an_older_server_missing_an_op_connects_and_only_that_op_is_held_back() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(vec![]));
        let seen = received.clone();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.read().unwrap();
            let mut status: serde_json::Value = serde_json::from_str(&status_frame(COMPAT)).unwrap();
            status["protocol"] = json!(wheel_proto::PROTOCOL_VERSION - 1);
            status["ops"] = json!(wheel_proto::ops().iter().filter(|op| **op != "pane_reload").collect::<Vec<_>>());
            ws.send(Message::text(status.to_string())).unwrap();
            while let Ok(message) = ws.read() {
                if let Message::Text(text) = message {
                    seen.lock().unwrap().push(wheel_proto::op_of(&text).unwrap_or_default());
                }
            }
        });
        let (endpoint, _) = endpoint(port, "t");
        let mut connection = Connection::open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { .. })));
        let reload = ClientMsg::PaneReload { req: Some(7), pane: "p1".into(), set: Default::default(), unset: vec![] };
        connection.send.send(frame(&reload)).unwrap();
        connection.send.send(frame(&ClientMsg::Status { req: Some(8) })).unwrap();
        assert!(matches!(next(&mut connection), Event::Refused { op } if op == "pane_reload"));
        let Event::Message(error) = next(&mut connection) else { panic!("no error") };
        let ServerMsg::Error { req: Some(7), message, .. } = *error else { panic!("not the refusal: {error:?}") };
        assert!(message.contains("`pane_reload`"), "{message}");
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while received.lock().unwrap().is_empty() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(*received.lock().unwrap(), ["status"], "wheeld got the status and never the refused op");
    }
}
