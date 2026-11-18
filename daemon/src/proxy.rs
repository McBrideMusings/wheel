//! The reverse proxy a web pane showing an `app` entry goes through (ADR-0026): a request to
//! `/artifacts/<pane>/<path>` becomes a request for `/<path>` on the port the program announced,
//! and its response streams back as it arrives, so server-sent events flow and a WebSocket upgrade
//! is carried through as bytes in both directions. The pane is the host: the page is the program's
//! own, and the daemon adds nothing to it.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::app::{App, Unreachable};

/// Headers that describe one connection, not the message, so a proxy never passes them on.
const HOP_BY_HOP: [HeaderName; 7] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
];

/// The page a pane shows while its app cannot answer: what is wrong, and the way to read why.
fn not_running(pane: &str, why: &str, detail: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>App not running</title>\
         <style>body{{font:15px -apple-system,system-ui,sans-serif;margin:3em auto;max-width:34em;padding:0 1em;color:#444}}\
         h1{{font-size:1.2em;color:#222}}code{{background:#8881;padding:.1em .3em;border-radius:3px}}</style>\
         <h1>This pane's app is not running</h1><p>{why}</p><p>{detail}</p>\
         <p>Start it with <code>wheel artifact app start {pane}</code>; its output is in <code>wheel artifact app logs {pane}</code>.</p>",
        why = escape(why),
        detail = escape(detail),
        pane = escape(pane),
    );
    (
        StatusCode::BAD_GATEWAY,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// The page a pane shows while its app is starting: it says so and asks again every second, so
/// the app's own page replaces it as soon as the program has announced its port.
fn starting() -> Response {
    let body = "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <meta http-equiv=refresh content=1>\
         <title>Starting</title>\
         <style>body{font:15px -apple-system,system-ui,sans-serif;margin:3em auto;max-width:34em;padding:0 1em;color:#444}\
         h1{font-size:1.2em;color:#222}</style>\
         <h1>Starting\u{2026}</h1><p>This pane's app is starting. The page appears when it is ready.</p>";
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `uri`'s query without the `token` pair, which is the daemon's and not the program's.
fn program_query(uri: &Uri) -> Option<String> {
    let kept: Vec<&str> = uri
        .query()?
        .split('&')
        .filter(|pair| pair.split('=').next() != Some("token"))
        .collect();
    (!kept.is_empty()).then(|| kept.join("&"))
}

/// The request's cookies minus the daemon's own, as one header value; None when none are left.
fn program_cookies(headers: &HeaderMap, daemon_cookie: &str) -> Option<HeaderValue> {
    let kept: Vec<&str> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.starts_with(&format!("{daemon_cookie}=")))
        .collect();
    (!kept.is_empty())
        .then(|| HeaderValue::from_str(&kept.join("; ")).ok())
        .flatten()
}

fn wants_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        && headers.contains_key(header::UPGRADE)
}

/// Send `req` (already authorized, for pane `pane`) to the program behind `app` and return its
/// answer. The program never sees the pane's address, `/artifacts/<pane>`, in a path.
pub(crate) async fn forward(
    app: &Arc<App>,
    pane: &str,
    daemon_cookie: &str,
    req: Request,
) -> Response {
    let port = match app.upstream() {
        Ok(port) => port,
        Err(Unreachable::Starting) => {
            tracing::info!(%pane, "app proxy: the app is starting");
            return starting();
        }
        Err(Unreachable::Down(why)) => {
            tracing::info!(%pane, %why, "app proxy: nothing to forward to");
            return not_running(pane, "Nothing is serving this pane's page right now.", &why);
        }
    };
    let prefix = format!("/artifacts/{pane}");
    match forward_port(port, &prefix, pane, daemon_cookie, None, None, req).await {
        Ok(resp) => resp,
        Err(Down::Refused(e)) => not_running(
            pane,
            "Nothing is serving this pane's page right now.",
            &format!("Port {port} did not accept a connection: {e}."),
        ),
        Err(Down::NoAnswer(e)) => not_running(
            pane,
            "The app is running but did not answer.",
            &format!("Port {port}: {e}."),
        ),
    }
}

/// Why a port could not answer: nothing accepted the connection, or what did never replied.
pub(crate) enum Down {
    Refused(std::io::Error),
    NoAnswer(hyper::Error),
}

/// Send `req` to `127.0.0.1:<port>` with `prefix` taken off its path, and return the answer with
/// a redirect to the program's own paths put back under `prefix`. `label` names the caller in
/// log lines. The program sees `origin` as the request's `Origin` when given; otherwise a request
/// that carried an `Origin` arrives with the program's own. It sees `host` as the request's `Host`
/// when given, and `127.0.0.1:<port>` otherwise. The port is tried on IPv4 and then IPv6, since a program may listen on only one. A
/// handshake that fails is answered here; a port that is down is the caller's to describe.
pub(crate) async fn forward_port(
    port: u16,
    prefix: &str,
    label: &str,
    daemon_cookie: &str,
    origin: Option<HeaderValue>,
    host: Option<HeaderValue>,
    mut req: Request,
) -> Result<Response, Down> {
    let stream = match TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        Err(v4) => match TcpStream::connect(("::1", port)).await {
            Ok(stream) => stream,
            Err(_) => {
                tracing::info!(%label, port, error = %v4, "proxy: the port refused");
                return Err(Down::Refused(v4));
            }
        },
    };
    let (mut sender, connection) =
        match hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(%label, port, error = %e, "proxy: handshake failed");
                return Ok((StatusCode::BAD_GATEWAY, "the program did not speak HTTP").into_response());
            }
        };
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });

    let path = req
        .uri()
        .path()
        .strip_prefix(prefix)
        .filter(|p| !p.is_empty())
        .unwrap_or("/")
        .to_string();
    let target = match program_query(req.uri()) {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let upgrading = wants_upgrade(req.headers());
    let client_upgrade = upgrading.then(|| hyper::upgrade::on(&mut req));

    let (parts, body) = req.into_parts();
    let mut out = Request::builder()
        .method(parts.method)
        .uri(&target)
        .version(http::Version::HTTP_11);
    let headers = out.headers_mut().expect("a fresh builder has headers");
    for (name, value) in &parts.headers {
        if HOP_BY_HOP.contains(name)
            || name == header::HOST
            || name == header::COOKIE
            || name == header::ORIGIN
            || name == header::AUTHORIZATION
        {
            continue;
        }
        if name == header::UPGRADE && !upgrading {
            continue;
        }
        headers.append(name, value.clone());
    }
    headers.insert(
        header::HOST,
        host.unwrap_or_else(|| {
            HeaderValue::from_str(&format!("127.0.0.1:{port}")).expect("a host is a header value")
        }),
    );
    if let Some(origin) = origin {
        headers.insert(header::ORIGIN, origin);
    } else if parts.headers.contains_key(header::ORIGIN) {
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_str(&format!("http://127.0.0.1:{port}"))
                .expect("an origin is a header value"),
        );
    }
    if let Some(cookies) = program_cookies(&parts.headers, daemon_cookie) {
        headers.insert(header::COOKIE, cookies);
    }
    if upgrading {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
    let out = match out.body(body) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!(%label, error = %e, "proxy: could not build the request");
            return Ok(StatusCode::BAD_REQUEST.into_response());
        }
    };

    let mut resp = match sender.send_request(out).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(%label, port, error = %e, "proxy: the program did not answer");
            return Err(Down::NoAnswer(e));
        }
    };

    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_upgrade) = client_upgrade {
            let program_upgrade = hyper::upgrade::on(&mut resp);
            let label = label.to_string();
            tokio::spawn(async move {
                let (Ok(client), Ok(program)) = tokio::join!(client_upgrade, program_upgrade)
                else {
                    return;
                };
                let (mut client, mut program) = (TokioIo::new(client), TokioIo::new(program));
                match tokio::io::copy_bidirectional(&mut client, &mut program).await {
                    Ok((up, down)) => {
                        tracing::debug!(%label, up, down, "proxy: upgraded connection closed")
                    }
                    Err(e) => {
                        tracing::debug!(%label, error = %e, "proxy: upgraded connection ended")
                    }
                }
            });
        }
    }

    let (mut parts, body) = resp.into_parts();
    if parts.status != StatusCode::SWITCHING_PROTOCOLS {
        for name in &HOP_BY_HOP {
            parts.headers.remove(name);
        }
    }
    // A redirect to a path of the program's own, written as a path or as the program's own
    // loopback address, stays under `prefix`.
    if let Some(location) = parts
        .headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(path) = own_path(location, port) {
            if let Ok(value) = HeaderValue::from_str(&format!("{prefix}{path}")) {
                parts.headers.insert(header::LOCATION, value);
            }
        }
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// The hosts that name this machine's loopback in an address.
const LOOPBACK_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"];

/// An `http` address on this machine's loopback, split into its port (80 when unwritten) and the
/// rest (`/…`, `?…`, `#…` or empty); the host is compared without case.
fn loopback_parts(address: &str) -> Option<(u16, &str)> {
    let scheme = address.get(..7)?;
    if !scheme.eq_ignore_ascii_case("http://") {
        return None;
    }
    let rest = &address[7..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse::<u16>().ok()?),
        _ => (authority, 80),
    };
    LOOPBACK_HOSTS
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
        .then_some((port, tail))
}

/// The port of `origin` when it is an origin on this machine's loopback, such as
/// `http://localhost:5173` (80 when unwritten).
pub(crate) fn loopback_origin_port(origin: &str) -> Option<u16> {
    loopback_parts(origin).and_then(|(port, rest)| rest.is_empty().then_some(port))
}

/// The `Host` a browser sends for `host` (a name of this machine's loopback, such as `localhost`)
/// at `port`: `localhost:5173`, or `localhost` alone at 80. None for any other host.
pub(crate) fn loopback_host(host: &str, port: u16) -> Option<String> {
    let host = LOOPBACK_HOSTS
        .iter()
        .find(|h| h.eq_ignore_ascii_case(host))?;
    Some(if port == 80 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    })
}

/// The port of a browser page on this machine's loopback, such as `http://localhost:5173/`.
pub(crate) fn loopback_port(page: &str) -> Option<u16> {
    loopback_parts(page).map(|(port, _)| port)
}

/// The path a redirect's `location` names on the program at `port` itself: a root-relative path,
/// or an `http` address on this machine's loopback at that port. None for anywhere else, which is
/// left as the program wrote it.
fn own_path(location: &str, port: u16) -> Option<&str> {
    if location.starts_with('/') && !location.starts_with("//") {
        return Some(location);
    }
    match loopback_parts(location)? {
        (at, "") if at == port => Some("/"),
        (at, tail) if at == port && tail.starts_with('/') => Some(tail),
        (at, tail) if at == port && tail.starts_with('?') => Some(tail),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{loopback_host, loopback_origin_port, loopback_port, own_path, starting};
    use axum::http::StatusCode;

    #[test]
    fn only_a_bare_loopback_origin_is_one() {
        assert_eq!(loopback_origin_port("http://localhost:5173"), Some(5173));
        assert_eq!(loopback_origin_port("http://127.0.0.1"), Some(80));
        assert_eq!(loopback_origin_port("http://[::1]:3000"), Some(3000));
        assert_eq!(loopback_origin_port("http://localhost:5173/"), None);
        assert_eq!(loopback_origin_port("https://localhost:5173"), None);
        assert_eq!(loopback_origin_port("http://evil.test"), None);
        assert_eq!(loopback_origin_port("null"), None);
    }

    #[test]
    fn a_loopback_host_is_sent_as_a_browser_writes_it() {
        assert_eq!(loopback_host("localhost", 5173).as_deref(), Some("localhost:5173"));
        assert_eq!(loopback_host("127.0.0.1", 80).as_deref(), Some("127.0.0.1"));
        assert_eq!(loopback_host("[::1]", 3000).as_deref(), Some("[::1]:3000"));
        assert_eq!(loopback_host("evil.test", 3000), None);
        assert_eq!(loopback_host("localhost:5173", 3000), None);
    }

    #[test]
    fn a_redirect_to_the_programs_own_address_stays_under_the_prefix() {
        assert_eq!(own_path("/next", 5173), Some("/next"));
        assert_eq!(own_path("//evil.test/x", 5173), None);
        assert_eq!(own_path("http://localhost:5173/a?b=1", 5173), Some("/a?b=1"));
        assert_eq!(own_path("http://127.0.0.1:5173", 5173), Some("/"));
        assert_eq!(own_path("http://[::1]:5173/x", 5173), Some("/x"));
        assert_eq!(own_path("http://localhost:51730/x", 5173), None);
        assert_eq!(own_path("http://localhost:8080/x", 5173), None);
        assert_eq!(own_path("https://example.com/", 5173), None);
        assert_eq!(own_path("HTTP://LOCALHOST:5173/up", 5173), Some("/up"));
        assert_eq!(own_path("http://0.0.0.0:5173/z", 5173), Some("/z"));
    }

    #[test]
    fn a_loopback_page_names_its_port() {
        assert_eq!(loopback_port("http://localhost:5173/"), Some(5173));
        assert_eq!(loopback_port("http://127.0.0.1:8080"), Some(8080));
        assert_eq!(loopback_port("http://[::1]:3000/a#b"), Some(3000));
        assert_eq!(loopback_port("http://localhost/"), Some(80));
        assert_eq!(loopback_port("http://localhost.example.com:5173/"), None);
        assert_eq!(loopback_port("https://localhost:5173/"), None);
        assert_eq!(loopback_port("https://example.com/"), None);
        assert_eq!(loopback_port("about:blank"), None);
    }

    #[tokio::test]
    async fn starting_page_waits_and_is_not_the_error_page() {
        let resp = starting();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let page = String::from_utf8(body.to_vec()).unwrap();
        assert!(page.contains("http-equiv=refresh"), "{page}");
        assert!(page.contains("Starting"), "{page}");
        assert!(!page.contains("not running"), "{page}");
    }
}
