//! The debug server (DebugServer.swift): a loopback HTTP endpoint a check reads the client's
//! state from and drives the client's own code paths through, with no visible window. Guarded
//! like wheeld's socket: the daemon's token as a bearer header, no browser `Origin`, a loopback
//! `Host` only. `WHEEL_DEBUG_PORT` names the port; without it no server runs.
//!
//! This module is the transport: a thread accepts connections, reads one request each, hands it
//! to the GPUI thread over a channel and writes the answer back. The routes are
//! `view::debug::handle`.
use anyhow::{Context as _, Result};
use futures::channel::mpsc as fmpsc;
use std::{collections::HashMap, io::{Read, Write}, net::{TcpListener, TcpStream}, path::Path, sync::mpsc, time::{Duration, Instant}};

/// Most a request may be, head and body: the bodies are chat messages.
pub const MAX_REQUEST: usize = 1 << 20;

/// One request, parsed; `reply` takes the status line's text (`200 OK`) and a JSON body.
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub body: String,
    /// When the connection stops waiting: a request answered later than this is not run.
    pub deadline: Instant,
    pub reply: mpsc::Sender<(String, Vec<u8>)>,
}
impl Request {
    /// Answer with a status line and a JSON body.
    pub fn respond(&self, (status, body): (String, serde_json::Value)) {
        let _ = self.reply.send((status, body.to_string().into_bytes()));
    }
    pub fn expired(&self) -> bool {
        Instant::now() > self.deadline
    }
}
/// The error body every refusal and failure uses.
pub fn error(why: &str) -> serde_json::Value {
    serde_json::json!({"error": why})
}
fn failure(status: &str, why: &str) -> (String, Vec<u8>) {
    (status.to_owned(), error(why).to_string().into_bytes())
}

pub struct Server {
    /// The port bound: `WHEEL_DEBUG_PORT`, or the one the system gave for 0.
    pub port: u16,
    pub requests: fmpsc::UnboundedReceiver<Request>,
}

/// Listen on 127.0.0.1:`port` (0 for any free port) and read the bearer token from `token_file`
/// on every request, as the Swift app does, so a token rotated under a running client still works.
pub fn start(port: u16, token_file: &Path) -> Result<Server> {
    let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("debug server on port {port}"))?;
    let port = listener.local_addr()?.port();
    let (tx, requests) = fmpsc::unbounded();
    let token_file = token_file.to_path_buf();
    std::thread::Builder::new().name("wheel-gpui-debug".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (tx, token_file) = (tx.clone(), token_file.clone());
            std::thread::spawn(move || serve(stream, port, &token_file, &tx));
        }
    })?;
    Ok(Server { port, requests })
}

fn serve(mut stream: TcpStream, port: u16, token_file: &Path, tx: &fmpsc::UnboundedSender<Request>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let (status, body) = loop {
        match parse(&buffer) {
            Parse::TooLarge => break failure("413 Payload Too Large", "request too large"),
            Parse::Complete(parsed) => break answer(parsed, port, token_file, tx),
            Parse::Incomplete => {}
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break failure("400 Bad Request", "incomplete request"),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
    let _ = stream.flush();
}

/// Check the guards, hand the request to the GPUI thread and wait for its answer.
fn answer(parsed: Parsed, port: u16, token_file: &Path, tx: &fmpsc::UnboundedSender<Request>) -> (String, Vec<u8>) {
    let token = std::fs::read_to_string(token_file).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    if let Err((status, why)) = authorize(&parsed.headers, port, token.as_deref()) {
        return failure(status, why);
    }
    let (reply, answered) = mpsc::channel();
    const WAIT: Duration = Duration::from_secs(30);
    let request = Request { method: parsed.method, path: parsed.path, query: parsed.query, body: parsed.body, deadline: Instant::now() + WAIT, reply };
    if tx.unbounded_send(request).is_err() {
        return failure("503 Service Unavailable", "the window is gone");
    }
    match answered.recv_timeout(WAIT) {
        Ok(reply) => reply,
        Err(_) => failure("504 Gateway Timeout", "the window did not answer"),
    }
}

/// The guards every request passes: no browser `Origin`, a loopback `Host` naming this port, and
/// wheeld's token as a bearer. Errors are (status, why).
pub fn authorize(headers: &HashMap<String, String>, port: u16, token: Option<&str>) -> Result<(), (&'static str, &'static str)> {
    if headers.contains_key("origin") {
        return Err(("403 Forbidden", "browser origin"));
    }
    let host = headers.get("host").map(String::as_str).unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err(("403 Forbidden", "host must be loopback"));
    }
    let authorized = token.is_some_and(|t| headers.get("authorization").is_some_and(|h| *h == format!("Bearer {t}")));
    if !authorized {
        return Err(("401 Unauthorized", "missing or wrong token"));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: String,
}
#[derive(Debug, PartialEq)]
pub enum Parse {
    Incomplete,
    TooLarge,
    Complete(Parsed),
}

/// Read a request out of what has arrived so far: the head once its blank line is in, the body
/// once `Content-Length` bytes follow it.
pub fn parse(buffer: &[u8]) -> Parse {
    if buffer.len() > MAX_REQUEST {
        return Parse::TooLarge;
    }
    let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else { return Parse::Incomplete };
    let head = String::from_utf8_lossy(&buffer[..end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("GET").to_owned();
    let target = parts.next().unwrap_or("/");
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())))
        .collect();
    let length = headers.get("content-length").and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
    if length > MAX_REQUEST {
        return Parse::TooLarge;
    }
    let body_bytes = &buffer[end + 4..];
    if body_bytes.len() < length {
        return Parse::Incomplete;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), query_pairs(q)),
        None => (target.to_owned(), HashMap::new()),
    };
    Parse::Complete(Parsed { method, path, query, headers, body: String::from_utf8_lossy(&body_bytes[..length]).into_owned() })
}

/// `a=1&b=two%20words` as a map, percent-decoded, `+` a space; a repeated name keeps the last.
pub fn query_pairs(query: &str) -> HashMap<String, String> {
    query.split('&').filter(|p| !p.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(k), percent_decode(v))
    }).collect()
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => match (bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)), bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16))) {
                (Some(hi), Some(lo)) => {
                    out.push((hi * 16 + lo) as u8);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn a_request_is_complete_once_its_body_arrived() {
        let text = b"POST /agent?agent=w1&text=a%20b+c HTTP/1.1\r\nHost: 127.0.0.1:7421\r\nContent-Length: 5\r\n\r\nhel";
        assert_eq!(parse(text), Parse::Incomplete);
        let mut whole = text.to_vec();
        whole.extend_from_slice(b"lo");
        let Parse::Complete(p) = parse(&whole) else { panic!() };
        assert_eq!((p.method.as_str(), p.path.as_str(), p.body.as_str()), ("POST", "/agent", "hello"));
        assert_eq!(p.query["agent"], "w1");
        assert_eq!(p.query["text"], "a b c");
        assert_eq!(p.headers["host"], "127.0.0.1:7421");
    }

    #[test]
    fn a_head_with_no_body_is_complete_at_its_blank_line() {
        let Parse::Complete(p) = parse(b"GET /state HTTP/1.1\r\nHost: localhost:1\r\n\r\n") else { panic!() };
        assert_eq!(p.path, "/state");
        assert!(p.query.is_empty() && p.body.is_empty());
    }

    #[test]
    fn an_oversized_request_is_refused() {
        assert_eq!(parse(&vec![b'a'; MAX_REQUEST + 1]), Parse::TooLarge);
        assert_eq!(parse(b"POST /x HTTP/1.1\r\nContent-Length: 9999999\r\n\r\n"), Parse::TooLarge);
    }

    #[test]
    fn the_guards_match_the_swift_servers() {
        let ok = headers(&[("host", "127.0.0.1:7431"), ("authorization", "Bearer t0k")]);
        assert_eq!(authorize(&ok, 7431, Some("t0k")), Ok(()));
        let mut origin = ok.clone();
        origin.insert("origin".into(), "http://x".into());
        assert_eq!(authorize(&origin, 7431, Some("t0k")), Err(("403 Forbidden", "browser origin")));
        assert_eq!(authorize(&ok, 7432, Some("t0k")), Err(("403 Forbidden", "host must be loopback")));
        assert_eq!(authorize(&ok, 7431, Some("other")), Err(("401 Unauthorized", "missing or wrong token")));
        assert_eq!(authorize(&ok, 7431, None), Err(("401 Unauthorized", "missing or wrong token")));
        let local = headers(&[("host", "localhost:7431"), ("authorization", "Bearer t0k")]);
        assert_eq!(authorize(&local, 7431, Some("t0k")), Ok(()));
    }

    #[test]
    fn percent_decoding_handles_a_trailing_percent() {
        assert_eq!(percent_decode("a%2Fb%"), "a/b%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%aé"), "%aé");
        assert_eq!(percent_decode("%+f"), "% f");
    }
}
