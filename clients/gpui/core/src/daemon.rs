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
    /// `ws://host:port/ws?token=…`, or `wss://…` for a paired remote wheeld.
    pub ws: String,
    pub token: String,
    url: String,
    token_file: PathBuf,
    /// The bundled wheeld this endpoint names, when it came from the bundle and not from a URL:
    /// the window's server control is its launch agent (`crate::server::Control::for_service`).
    pub service: Option<crate::server::Service>,
    /// A paired remote wheeld: its id, as `status.host.id` reports it, and the SHA-256 of the
    /// certificate the app pinned when it paired. None for the local wheeld.
    pub remote: Option<Remote>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub id: String,
    pub fingerprint: String,
}
/// What the app calls the local wheeld until its `status` names it.
pub const LOCAL: &str = "local";
impl Endpoint {
    /// `url` is the daemon's WebSocket URL; the token comes from `token_file`.
    pub fn new(url: &str, token_file: &PathBuf) -> Result<Self> {
        let token = std::fs::read_to_string(token_file).with_context(|| format!("reading {}", token_file.display()))?.trim().to_owned();
        Ok(Self { ws: format!("{url}?token={token}"), token, url: url.to_owned(), token_file: token_file.clone(), service: None, remote: None })
    }
    /// The bundled service's endpoint; `token_file` overrides the service's own (`WHEEL_TOKEN_FILE`).
    pub fn for_service(service: crate::server::Service, token_file: Option<PathBuf>) -> Result<Self> {
        let mut endpoint = Self::new(&service.url(), &token_file.unwrap_or_else(|| service.token_file()))?;
        endpoint.service = Some(service);
        Ok(endpoint)
    }
    /// A paired remote wheeld at `url` (`wss://…/ws`), reached with `credential` over TLS that
    /// trusts only the certificate whose SHA-256 is `fingerprint`.
    pub fn remote(url: &str, id: &str, credential: &str, fingerprint: &str) -> Self {
        Self {
            ws: format!("{url}?token={credential}"),
            token: credential.to_owned(),
            url: url.to_owned(),
            token_file: PathBuf::new(),
            service: None,
            remote: Some(Remote { id: id.to_owned(), fingerprint: fingerprint.to_ascii_lowercase() }),
        }
    }
    /// The key the app holds this wheeld's connection under: the remote's id, else [`LOCAL`].
    pub fn server(&self) -> &str {
        self.remote.as_ref().map_or(LOCAL, |r| r.id.as_str())
    }
    /// The address without the token, for a log line.
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn token_file(&self) -> &std::path::Path {
        &self.token_file
    }
    /// The same endpoint with its credential read again, so a reconnect never reuses old text:
    /// wheeld writes a new token when it starts without one, and a remote paired again (after
    /// `wheel remote revoke`, or with a new certificate) has a new credential and fingerprint in
    /// the server list and the Keychain.
    pub fn reread(&self) -> Result<Self> {
        if let Some(remote) = &self.remote {
            let list = crate::servers::load(&crate::servers::default_file());
            let server = list.iter().find(|s| s.id == remote.id).ok_or_else(|| anyhow::anyhow!("server {} is no longer paired", remote.id))?;
            anyhow::ensure!(server.enabled, "server {} is turned off", remote.id);
            return crate::servers::endpoint(server);
        }
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

pub type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// How long reaching a remote wheeld may take, the TCP connect and the TLS and WebSocket
/// handshakes each.
const REMOTE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The socket, connected and blocking (the connection thread makes it non-blocking and sleeps in
/// `poll` on it and its wake pipe). `pin` reaches a remote wheeld over
/// TLS that trusts only that certificate.
pub fn socket(url: &str, pin: Option<&str>) -> Result<Socket> {
    // An error names the address, never the token in its query.
    let shown = url.split('?').next().unwrap_or(url);
    let ws = match pin {
        None => tungstenite::connect(url).with_context(|| format!("connecting to wheeld at {shown}"))?.0,
        Some(pin) => {
            use tungstenite::client::IntoClientRequest as _;
            let request = url.into_client_request().with_context(|| format!("{shown} is not a WebSocket URL"))?;
            let host = request.uri().host().unwrap_or_default().trim_matches(['[', ']']).to_owned();
            let port = request.uri().port_u16().unwrap_or(443);
            // A blackholed address or a stalled handshake must not hold this server's connection
            // thread (every window on it waits on that thread) for the OS's minute-long defaults.
            let address = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .with_context(|| format!("resolving {shown}"))?
                .next()
                .ok_or_else(|| anyhow::anyhow!("{shown} resolves to no address"))?;
            let stream = TcpStream::connect_timeout(&address, REMOTE_CONNECT_TIMEOUT).with_context(|| format!("connecting to wheeld at {shown}"))?;
            stream.set_read_timeout(Some(REMOTE_CONNECT_TIMEOUT))?;
            stream.set_write_timeout(Some(REMOTE_CONNECT_TIMEOUT))?;
            // A remote wheeld that vanishes without closing (sleep, a network switch) is found by
            // the kernel's keepalive, which errors the socket and wakes the poll below: the
            // window then reads Down.
            wheel_proto::net::keep_alive(&stream).with_context(|| format!("keepalive on {shown}"))?;
            let config = wheel_proto::tls::pinned(pin).context("the pinned TLS configuration")?;
            let connector = tungstenite::Connector::Rustls(std::sync::Arc::new(config));
            tungstenite::client_tls_with_config(request, stream, None, Some(connector)).map_err(|e| anyhow::anyhow!("connecting to wheeld at {shown}: {e}"))?.0
        }
    };
    let stream = tcp(&ws).ok_or_else(|| anyhow::anyhow!("wheeld at {shown} is neither a plain nor a TLS socket"))?;
    // The handshake's timeouts end with it; the connection thread polls a non-blocking socket.
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    // Keystrokes and replies are small frames: Nagle's coalescing would only hold them back.
    stream.set_nodelay(true)?;
    Ok(ws)
}

/// The TCP socket under a plain or TLS WebSocket.
fn tcp(ws: &Socket) -> Option<&TcpStream> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(stream) => Some(stream),
        MaybeTlsStream::Rustls(stream) => Some(&stream.sock),
        _ => None,
    }
}

/// What a subscriber hands the connection thread.
enum Out {
    Frame(u32, String),
    Join(u32, fmpsc::UnboundedSender<Event>),
    Leave(u32),
}

/// Where one subscriber (a window, the menu bar) queues frames for wheeld. Each one also wakes the
/// connection thread, which otherwise sleeps until wheeld sends something.
#[derive(Clone)]
pub struct Outbox {
    frames: mpsc::Sender<Out>,
    /// None for an outbox no connection reads ([`Outbox::detached`]).
    wake: Option<std::sync::Arc<std::os::unix::net::UnixStream>>,
    /// The subscriber's index on its connection (`crate::route`).
    n: u32,
}

impl Outbox {
    pub fn send(&self, text: String) -> Result<(), mpsc::SendError<String>> {
        self.put(Out::Frame(self.n, text)).map_err(|e| match e.0 {
            Out::Frame(_, text) => mpsc::SendError(text),
            _ => mpsc::SendError(String::new()),
        })
    }

    /// This subscriber is done: what only it held on wheeld (an agent stream, a feed, a process
    /// watch) is let go, and nothing more reaches it.
    pub fn leave(&self) {
        let _ = self.put(Out::Leave(self.n));
    }

    fn put(&self, out: Out) -> Result<(), mpsc::SendError<Out>> {
        self.frames.send(out)?;
        // Non-blocking: a full pipe already holds a wake-up the thread has not read.
        if let Some(wake) = &self.wake {
            let _ = std::io::Write::write(&mut &**wake, &[1]);
        }
        Ok(())
    }

    /// An outbox with no connection behind it, and the frames sent to it: for a client a test
    /// drives without wheeld.
    pub fn detached() -> (Self, mpsc::Receiver<String>) {
        let (frames, sent) = mpsc::channel::<Out>();
        let (texts, read) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(out) = sent.recv() {
                if let Out::Frame(_, text) = out
                    && texts.send(text).is_err()
                {
                    return;
                }
            }
        });
        (Self { frames, wake: None, n: 1 }, read)
    }
}

/// A subscriber's side of the connection: what it hears, in order.
pub type Events = fmpsc::UnboundedReceiver<Event>;

/// What the connection tells a subscriber, in order.
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

/// One wheeld's connection, which every window showing that wheeld's sessions subscribes to.
#[derive(Clone)]
pub struct Connection {
    hub: Outbox,
    next: std::sync::Arc<std::sync::atomic::AtomicU32>,
}
impl Connection {
    /// Connect and ask for the layout; wheeld then broadcasts every change. When the socket
    /// closes, a daemon restart for instance, the connection reads the token file again and
    /// retries with a growing pause until wheeld answers, then asks for the layout again.
    /// A frame sent while the link is not up is dropped, so a view sends only while it is `Up`.
    pub fn open(endpoint: Endpoint) -> Self {
        let (frames, outgoing) = mpsc::channel::<Out>();
        let (wake, woken) = std::os::unix::net::UnixStream::pair().expect("a socket pair for the connection's wake-ups");
        wake.set_nonblocking(true).expect("a non-blocking wake socket");
        woken.set_nonblocking(true).expect("a non-blocking wake socket");
        std::thread::spawn(move || supervise(endpoint, &outgoing, &woken));
        Self { hub: Outbox { frames, wake: Some(std::sync::Arc::new(wake)), n: 0 }, next: std::sync::Arc::new(1.into()) }
    }

    /// A new subscriber: its outbox and what the connection tells it. One that joins a link
    /// already up hears `Up` and then a `status` of its own.
    pub fn subscribe(&self) -> (Outbox, Events) {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (events, incoming) = fmpsc::unbounded();
        let _ = self.hub.put(Out::Join(n, events));
        (Outbox { n, ..self.hub.clone() }, incoming)
    }
}

/// What the connection thread keeps across sockets: who listens, and the outage it last told them.
struct Hub {
    router: crate::route::Router,
    /// The endpoint of the link that is up now.
    up: Option<Endpoint>,
    announced: Option<String>,
    /// The last outage, told again to a subscriber that joins during it.
    outage: Option<Event>,
}
impl Hub {
    fn tell(&mut self, n: u32, event: Event) {
        if let Some(events) = self.router.subscribers.get(&n)
            && events.unbounded_send(event).is_err()
        {
            self.router.subscribers.remove(&n);
        }
    }
    fn broadcast(&mut self, event: impl Fn() -> Event) {
        self.router.subscribers.retain(|_, events| events.unbounded_send(event()).is_ok());
    }
    fn outage_again(&self) -> Option<Event> {
        match &self.outage {
            Some(Event::Down(why)) => Some(Event::Down(why.clone())),
            Some(Event::Incompatible { server, client }) => Some(Event::Incompatible { server: *server, client: *client }),
            _ => None,
        }
    }
}

/// What a socket's thread does with one thing a subscriber queued. `Err` when every outbox is gone.
fn take(hub: &mut Hub, out: Out, live: Option<(&mut Socket, &wheel_proto::Offer, &mut bool)>) -> Result<(), Ended> {
    match out {
        Out::Join(n, events) => {
            hub.router.join(n, events);
            match (&hub.up, live) {
                (Some(endpoint), Some((ws, _, unsent))) => {
                    let endpoint = endpoint.clone();
                    hub.tell(n, Event::Up(endpoint));
                    write(ws, unsent, json_status(crate::route::join(n, 1)))?;
                }
                _ => {
                    if let Some(event) = hub.outage_again() {
                        hub.tell(n, event);
                    }
                }
            }
        }
        Out::Leave(n) => {
            let undo = hub.router.leave(n);
            if let Some((ws, _, unsent)) = live {
                for text in undo {
                    write(ws, unsent, text)?;
                }
            }
        }
        Out::Frame(n, text) => {
            let Some((ws, offer, unsent)) = live else { return Ok(()) };
            if let Err(refused) = offer.admit(&text) {
                refuse(hub, n, &text, refused);
                return Ok(());
            }
            match hub.router.outgoing(n, &text) {
                crate::route::Outgoing::Send(text) => write(ws, unsent, text)?,
                crate::route::Outgoing::Hold => {}
                crate::route::Outgoing::Answer(message) => hub.tell(n, Event::Message(Box::new(message))),
            }
        }
    }
    Ok(())
}

fn json_status(req: u64) -> String {
    frame(&ClientMsg::Status { req: Some(req) })
}

fn write(ws: &mut Socket, unsent: &mut bool, text: String) -> Result<(), Ended> {
    match ws.send(Message::text(text)) {
        Ok(()) => Ok(()),
        Err(e) if would_block(&e) => {
            *unsent = true;
            Ok(())
        }
        Err(_) => Err(Ended::Lost { was_up: true, reason: Event::Down("wheeld closed the socket".into()) }),
    }
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock)
}

fn supervise(first: Endpoint, outgoing: &mpsc::Receiver<Out>, woken: &std::os::unix::net::UnixStream) {
    let mut hub = Hub { router: crate::route::Router::default(), up: None, announced: None, outage: None };
    let (mut endpoint, mut failures) = (first, 0usize);
    // A remote wheeld that is unpaired or turned off is not dialled again with its old credential.
    let mut paused = false;
    loop {
        match if paused { None } else { Some(session(&endpoint, outgoing, woken, &mut hub)) } {
            None => {}
            Some(Ended::ClientGone) => return,
            Some(Ended::Lost { was_up, reason }) => {
                hub.up = None;
                if was_up {
                    failures = 0;
                    hub.announced = None;
                }
                // One event per kind of outage: a changed cause (not reachable, then too old, then
                // not reachable again) is announced, a repeat of it is not.
                let key = match &reason {
                    Event::Incompatible { server, .. } => format!("old {server}"),
                    _ => "down".to_owned(),
                };
                hub.outage = Some(reason);
                if hub.announced.as_deref() != Some(key.as_str()) {
                    hub.announced = Some(key);
                    if let Some(outage) = hub.outage_again() {
                        hub.broadcast(|| clone_outage(&outage));
                    }
                }
            }
        }
        // Joins and leaves that came while the socket was down still count.
        std::thread::sleep(BACKOFF[failures.min(BACKOFF.len() - 1)]);
        failures += 1;
        loop {
            match outgoing.try_recv() {
                Ok(out) => {
                    let _ = take(&mut hub, out, None);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        paused = false;
        match endpoint.reread() {
            Ok(fresh) => endpoint = fresh,
            Err(error) => {
                paused = endpoint.remote.is_some();
                if hub.announced.as_deref() != Some("down") {
                    hub.announced = Some("down".to_owned());
                    let why = format!("{error:#}");
                    hub.outage = Some(Event::Down(why.clone()));
                    hub.broadcast(|| Event::Down(why.clone()));
                }
            }
        }
    }
}

fn clone_outage(event: &Event) -> Event {
    match event {
        Event::Down(why) => Event::Down(why.clone()),
        Event::Incompatible { server, client } => Event::Incompatible { server: *server, client: *client },
        _ => Event::Down(String::new()),
    }
}

enum Ended {
    ClientGone,
    Lost { was_up: bool, reason: Event },
}

/// One socket from open to close. Every frame a subscriber queues passes the offer of the wheeld
/// at the other end (`Offer::admit`): one whose op it did not list is answered here with an
/// `error` and never sent.
fn session(endpoint: &Endpoint, outgoing: &mpsc::Receiver<Out>, woken: &std::os::unix::net::UnixStream, hub: &mut Hub) -> Ended {
    let lost = |was_up, why: String| Ended::Lost { was_up, reason: Event::Down(why) };
    let mut ws = match socket(&endpoint.ws, endpoint.remote.as_ref().map(|r| r.fingerprint.as_str())) {
        Ok(ws) => ws,
        Err(error) => return lost(false, format!("{error:#}")),
    };
    if let Some(Err(error)) = tcp(&ws).map(|s| s.set_nonblocking(true)) {
        return lost(false, format!("{error}"));
    }
    hub.router.reset();
    // A frame the socket could not take at once waits in tungstenite's buffer until it can.
    let mut unsent = false;
    if let Err(ended) = write(&mut ws, &mut unsent, json_status(1)) {
        return ended;
    }
    let mut offer: Option<wheel_proto::Offer> = None;
    let Some(socket_fd) = tcp(&ws).map(std::os::fd::AsRawFd::as_raw_fd) else { return lost(false, "wheeld is not a socket".into()) };
    loop {
        // Sleep until wheeld sends, a subscriber queues a frame, or a held frame can go out.
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
                Ok(out) => {
                    // Before the link is up a frame is dropped; a join or leave still counts.
                    let live = offer.as_ref().map(|offer| (&mut ws, offer, &mut unsent));
                    if let Err(ended) = take(hub, out, live) {
                        return ended;
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
                        if first {
                            hub.up = Some(endpoint.clone());
                            hub.outage = None;
                            hub.announced = None;
                            hub.broadcast(|| Event::Up(endpoint.clone()));
                        }
                    } else if offer.is_none() {
                        continue;
                    }
                    let Some(message) = message else { continue };
                    for (n, message) in hub.router.incoming(message, &t) {
                        hub.tell(n, Event::Message(Box::new(message)));
                    }
                    for text in hub.router.take_back() {
                        if let Err(ended) = write(&mut ws, &mut unsent, text) {
                            return ended;
                        }
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

/// Tell subscriber `n` its frame was refused, and answer its request with the `error` wheeld would send.
fn refuse(hub: &mut Hub, n: u32, text: &str, refused: wheel_proto::Refused) {
    #[derive(Deserialize)]
    struct Req {
        req: Option<u64>,
    }
    let req = serde_json::from_str::<Req>(text).ok().and_then(|r| r.req);
    let message = refused.to_string();
    hub.tell(n, Event::Refused { op: refused.op });
    hub.tell(n, Event::Message(Box::new(ServerMsg::Error { req, term: None, message })));
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
    fn open(endpoint: Endpoint) -> (Connection, Outbox, fmpsc::UnboundedReceiver<Event>) {
        let connection = Connection::open(endpoint);
        let (out, events) = connection.subscribe();
        (connection, out, events)
    }
    fn next(events: &mut fmpsc::UnboundedReceiver<Event>) -> Event {
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < end {
            if let Ok(event) = events.try_recv() {
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
        let ws = socket(&format!("ws://127.0.0.1:{port}/ws"), None).unwrap();
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
        let (_held, out, mut connection) = open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { .. })));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(&*m, ServerMsg::Spaces { sessions, .. } if sessions[0].name == "7")), "the late frame never arrived");

        // Idle now: a queued frame goes out when it is queued, not at the next timer.
        std::thread::sleep(Duration::from_millis(200));
        let sent = std::time::Instant::now();
        out.send(frame(&ClientMsg::Status { req: Some(9) })).unwrap();
        let (text, at) = heard_rx.recv_timeout(Duration::from_secs(5)).expect("the frame never reached wheeld");
        assert!(text.contains(&format!("\"req\":{}", crate::route::join(1, 9))), "the first subscriber's req goes out rewritten: {text}");
        // Inside the 30 ms read timeout this replaced, with room for a loaded machine.
        assert!(at.duration_since(sent) < Duration::from_millis(25), "the frame waited {:?}", at.duration_since(sent));
    }

    #[test]
    fn reconnects_after_the_socket_closes_and_rereads_the_token() {
        let (port, tokens) = serve(vec![(COMPAT, false), (COMPAT, true)]);
        let (endpoint, file) = endpoint(port, "one");
        let (_held, _out, mut connection) = open(endpoint);
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
            // wheeld streams an agent to a connection that attached it.
            while !matches!(ws.read(), Ok(Message::Text(t)) if t.contains("agent_attach")) {}
            let attached = json!({"ev":"agent_attached","agent":"a1","now":0,"state":{"phase":"ready"},"events":[
                {"kind":"text","text":"before"},{"kind":"hologram","at":7,"frames":3},{"kind":"text","text":"after"}]});
            ws.send(Message::text(attached.to_string())).unwrap();
            while ws.read().is_ok() {}
        });
        let (endpoint, _) = endpoint(port, "t");
        let (_held, out, mut connection) = open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        let Event::Message(status) = next(&mut connection) else { panic!("no status") };
        let ServerMsg::Status { spaces, .. } = *status else { panic!("not a status") };
        out.send(frame(&ClientMsg::AgentAttach { req: None, agent: "a1".into() })).unwrap();
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
        let (_held, _out, mut connection) = open(endpoint);
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
        let (_held, _out, mut connection) = open(endpoint);
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
        let (_held, _out, mut connection) = open(endpoint);
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
        let (_held, out, mut connection) = open(endpoint);
        assert!(matches!(next(&mut connection), Event::Up(_)));
        assert!(matches!(next(&mut connection), Event::Message(m) if matches!(*m, ServerMsg::Status { .. })));
        let reload = ClientMsg::PaneReload { req: Some(7), pane: "p1".into(), set: Default::default(), unset: vec![] };
        out.send(frame(&reload)).unwrap();
        out.send(frame(&ClientMsg::Status { req: Some(8) })).unwrap();
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
