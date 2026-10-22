//! The remote listener: the same routes as the local one, over TLS with a self-signed certificate
//! that a client pins by fingerprint. Off unless the `remote.listen` setting names an address.
//!
//! The certificate and its key live in the instance directory and are made once, so the
//! fingerprint a client pinned stays valid across restarts. Its subject names are the machine's
//! interface addresses and hostname as they were when it was made; a client pins the fingerprint
//! and checks no name.
//!
//! The certificate follows Apple's rules for TLS servers, which an iPhone enforces: a server
//! authentication usage, and a validity of at most 398 days. It is made again once it is a year
//! old, and a phone pairs again with the new fingerprint.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

use crate::AppState;

const CERT_FILE: &str = "remote-cert.pem";
const KEY_FILE: &str = "remote-key.pem";

/// A certificate is valid from yesterday (clock skew) to this many days from today; Apple accepts
/// at most 398 days.
const VALID_DAYS: i64 = 396;
/// A certificate older than this is replaced, well inside its validity.
const RENEW_AFTER: std::time::Duration = std::time::Duration::from_secs(365 * 86_400);

/// A bound remote listener, as `remote_info` reports it.
pub(crate) enum Remote {
    /// Listening.
    Up {
        listen: String,
        port: u16,
        fingerprint: String,
    },
    /// `remote.listen` is set and the listener did not start; the reason.
    Failed { listen: String, why: String },
}

/// The certificate and key the listener presents.
struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

/// SHA-256 of a DER certificate, lowercase hex: what a client pins.
pub fn fingerprint(cert: &[u8]) -> String {
    Sha256::digest(cert)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The identity saved in `dir`, made first if either file is missing.
fn identity(dir: &Path) -> anyhow::Result<Identity> {
    let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
    let fresh = cert_path
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|made| made.elapsed().ok())
        .is_some_and(|age| age < RENEW_AFTER);
    if !fresh || !key_path.exists() {
        make(&cert_path, &key_path)?;
    }
    let cert = CertificateDer::from_pem_file(&cert_path)
        .with_context(|| format!("reading {}", cert_path.display()))?;
    let key = PrivateKeyDer::from_pem_file(&key_path)
        .with_context(|| format!("reading {}", key_path.display()))?;
    Ok(Identity { cert, key })
}

fn make(cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into()];
    names.extend(local_addresses());
    // A name rcgen refuses (a non-ASCII hostname) is left out: a client pins the fingerprint and
    // checks no name.
    names.extend(hostname().filter(|h| rcgen::CertificateParams::new(vec![h.clone()]).is_ok()));
    names.sort();
    names.dedup();
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(names.clone())?;
    let (from, to) = (civil_date(-1), civil_date(VALID_DAYS));
    params.not_before = rcgen::date_time_ymd(from.0, from.1, from.2);
    params.not_after = rcgen::date_time_ymd(to.0, to.1, to.2);
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params.self_signed(&key)?;
    // Each through a rename, so a crash leaves a whole file or none, never a half-written pair
    // `identity` would then trust.
    write_private(key_path, key.serialize_pem().as_bytes())?;
    let part = cert_path.with_extension("pem.part");
    std::fs::write(&part, cert.pem())?;
    std::fs::rename(&part, cert_path)?;
    tracing::info!(cert = %cert_path.display(), names = ?names, fingerprint = %fingerprint(cert.der()), "remote certificate made");
    Ok(())
}

/// The UTC calendar date `days` from today as (year, month, day).
fn civil_date(days: i64) -> (i32, u8, u8) {
    let today = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64 / 86_400);
    // Howard Hinnant's civil-from-days.
    let z = today + days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        (yoe + era * 400 + i64::from(month <= 2)) as i32,
        month as u8,
        day as u8,
    )
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let part = path.with_extension("pem.part");
    let _ = std::fs::remove_file(&part);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&part)?;
    std::io::Write::write_all(&mut file, bytes)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is 256 writable bytes and the length passed is 256.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).into_owned();
    (!name.is_empty()).then_some(name)
}

/// The address of every network interface that is up, other than IPv6 link-local ones (which need
/// a scope id to dial). Loopback is included.
fn local_addresses() -> Vec<String> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-pointer; the list it receives is freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return found;
    }
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is a node of the list getifaddrs returned, not yet freed.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_flags & libc::IFF_UP as u32 == 0 || entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_addr` is non-null and points at a sockaddr of the family it declares.
        let ip = unsafe {
            match i32::from((*entry.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let sin = &*(entry.ifa_addr as *const libc::sockaddr_in);
                    Some(IpAddr::from(sin.sin_addr.s_addr.to_ne_bytes()))
                }
                libc::AF_INET6 => {
                    let sin6 = &*(entry.ifa_addr as *const libc::sockaddr_in6);
                    Some(IpAddr::from(sin6.sin6_addr.s6_addr))
                }
                _ => None,
            }
        };
        if let Some(ip) = ip {
            let link_local = matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80);
            if !link_local {
                found.push(ip.to_string());
            }
        }
    }
    // SAFETY: `list` came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// Connections the listener holds at once, handshaking or served. A connection past this is closed
/// before its handshake starts.
const MAX_CONNECTIONS: usize = 64;

/// Wrong tokens one peer address may present within [`FAILURE_WINDOW`] before it is refused.
const MAX_FAILURES: usize = 5;

/// How far back a peer's wrong tokens count, and how long it is refused once it reaches
/// [`MAX_FAILURES`].
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// A TLS connection that holds one of the listener's [`MAX_CONNECTIONS`] slots until it closes.
struct Held {
    io: TlsStream<TcpStream>,
    _slot: OwnedSemaphorePermit,
}

impl AsyncRead for Held {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Held {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

/// A listener that completes each connection's TLS handshake on a task of its own, so one client
/// that stalls its handshake holds up no other, and holds at most [`MAX_CONNECTIONS`] open.
struct TlsListener {
    local: SocketAddr,
    ready: mpsc::Receiver<(Held, SocketAddr)>,
}

impl TlsListener {
    fn new(listener: TcpListener, acceptor: TlsAcceptor) -> std::io::Result<Self> {
        let local = listener.local_addr()?;
        let (tx, ready) = mpsc::channel(16);
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        tokio::spawn(async move {
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        tracing::warn!(error = %e, "remote accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    tracing::warn!(%peer, max = MAX_CONNECTIONS, "remote connection refused: at the connection limit");
                    continue;
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let handshake =
                        tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await;
                    match handshake {
                        Ok(Ok(io)) => {
                            let _ = tx.send((Held { io, _slot: slot }, peer)).await;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(%peer, error = %e, "remote tls handshake failed")
                        }
                        Err(_) => tracing::warn!(%peer, "remote tls handshake timed out"),
                    }
                });
            }
        });
        Ok(Self { local, ready })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = Held;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // The accepting task owns the sender for the life of the process.
        self.ready
            .recv()
            .await
            .expect("the remote accept task ended")
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// The peer of a TLS connection. axum can hand handlers a `ConnectInfo<SocketAddr>` only for a
/// plain TCP listener, so the remote routes read this and [`as_socket_addr`] republishes it as the
/// `ConnectInfo<SocketAddr>` the handlers ask for.
#[derive(Clone, Copy)]
struct Peer(SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        Peer(*stream.remote_addr())
    }
}

async fn as_socket_addr(
    axum::extract::ConnectInfo(Peer(peer)): axum::extract::ConnectInfo<Peer>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    next.run(request).await
}

/// The wrong tokens each peer address has presented lately. A peer that reaches [`MAX_FAILURES`]
/// inside [`FAILURE_WINDOW`] is refused outright, a correct token included, until the oldest of
/// them ages out.
#[derive(Default)]
struct Throttle {
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl Throttle {
    /// Whether `peer` has used up its wrong tokens.
    fn blocked(&self, peer: IpAddr, now: Instant) -> bool {
        let mut failures = self.failures.lock().unwrap();
        let Some(recent) = failures.get_mut(&peer) else {
            return false;
        };
        recent.retain(|at| now.duration_since(*at) < FAILURE_WINDOW);
        if recent.is_empty() {
            failures.remove(&peer);
            return false;
        }
        recent.len() >= MAX_FAILURES
    }

    /// Count one wrong token from `peer`, forgetting every peer whose failures have all aged out.
    fn failed(&self, peer: IpAddr, now: Instant) {
        let mut failures = self.failures.lock().unwrap();
        failures.retain(|_, recent| {
            recent.retain(|at| now.duration_since(*at) < FAILURE_WINDOW);
            !recent.is_empty()
        });
        failures.entry(peer).or_default().push(now);
    }
}

/// Refuse a blocked peer with 429, and count each 401 an unblocked peer's request earns. Covers
/// every route on the remote listener: the socket, pairing and the pane pages.
async fn throttle(
    axum::extract::State(throttle): axum::extract::State<Arc<Throttle>>,
    axum::extract::ConnectInfo(Peer(peer)): axum::extract::ConnectInfo<Peer>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let ip = peer.ip();
    if throttle.blocked(ip, Instant::now()) {
        tracing::warn!(%peer, "remote request refused: too many wrong tokens");
        return axum::response::IntoResponse::into_response(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
        );
    }
    let response = next.run(request).await;
    if response.status() == axum::http::StatusCode::UNAUTHORIZED {
        throttle.failed(ip, Instant::now());
    }
    response
}

/// Bind `listen`, present the identity kept in `dir`, and serve the daemon's routes on it from a
/// task of its own. Records the listener on `state` for `remote_info`. Returns the bound address.
pub async fn start(state: &Arc<AppState>, listen: &str, dir: &Path) -> anyhow::Result<SocketAddr> {
    let identity = identity(dir)?;
    let fingerprint = fingerprint(&identity.cert);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert], identity.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let bound = listener.local_addr()?;
    *state.remote.lock().unwrap() = Some(Remote::Up {
        listen: listen.to_string(),
        port: bound.port(),
        fingerprint: fingerprint.clone(),
    });
    tracing::info!(%listen, %bound, %fingerprint, "wheeld remote listening");
    let app = crate::server::app(state.clone(), crate::server::Listener::Remote)
        .layer(axum::middleware::from_fn(as_socket_addr))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(Throttle::default()),
            throttle,
        ));
    let tls = TlsListener::new(listener, acceptor)?;
    tokio::spawn(async move {
        let served = axum::serve(tls, app.into_make_service_with_connect_info::<Peer>()).await;
        if let Err(e) = served {
            tracing::error!(error = %e, "the remote listener stopped");
        }
    });
    Ok(bound)
}

/// Record that `remote.listen` is `listen` and the listener did not start, so `remote_info` says
/// why rather than reporting remote access as off.
pub fn failed(state: &AppState, listen: &str, why: String) {
    *state.remote.lock().unwrap() = Some(Remote::Failed {
        listen: listen.to_string(),
        why,
    });
}

/// The answer to `remote_info`.
pub(crate) fn info(state: &AppState, req: u64) -> wheel_proto::ServerMsg {
    let remote = state.remote.lock().unwrap();
    let (listen, addresses, port, fingerprint, problem) = match remote.as_ref() {
        None => (None, Vec::new(), 0, String::new(), None),
        Some(Remote::Failed { listen, why }) => (
            Some(listen.clone()),
            Vec::new(),
            0,
            String::new(),
            Some(why.clone()),
        ),
        Some(Remote::Up {
            listen,
            port,
            fingerprint,
        }) => {
            let mut addresses = local_addresses();
            addresses.extend(hostname());
            (
                Some(listen.clone()),
                addresses,
                *port,
                fingerprint.clone(),
                None,
            )
        }
    };
    wheel_proto::ServerMsg::Remote {
        req,
        listen,
        addresses,
        port,
        fingerprint,
        problem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_blocked_after_five_wrong_tokens_until_the_oldest_ages_out() {
        let throttle = Throttle::default();
        let (peer, other): (IpAddr, IpAddr) =
            ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        let start = Instant::now();
        for i in 0..MAX_FAILURES {
            assert!(!throttle.blocked(peer, start), "blocked after {i} failures");
            throttle.failed(peer, start);
        }
        assert!(throttle.blocked(peer, start));
        assert!(!throttle.blocked(other, start));
        assert!(throttle.blocked(peer, start + FAILURE_WINDOW - Duration::from_secs(1)));
        assert!(!throttle.blocked(peer, start + FAILURE_WINDOW));
    }
}
