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

use crate::app::App;

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
/// answer. `prefix` is the pane's address, `/artifacts/<pane>`, which the program never sees.
pub(crate) async fn forward(
    app: &Arc<App>,
    pane: &str,
    daemon_cookie: &str,
    mut req: Request,
) -> Response {
    let port = match app.upstream() {
        Ok(port) => port,
        Err(why) => {
            tracing::info!(%pane, %why, "app proxy: nothing to forward to");
            return not_running(pane, "Nothing is serving this pane's page right now.", &why);
        }
    };
    let stream = match TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::info!(%pane, port, error = %e, "app proxy: the announced port refused");
            return not_running(
                pane,
                "Nothing is serving this pane's page right now.",
                &format!("Port {port} did not accept a connection: {e}."),
            );
        }
    };
    let (mut sender, connection) =
        match hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(%pane, port, error = %e, "app proxy: handshake failed");
                return (StatusCode::BAD_GATEWAY, "the app did not speak HTTP").into_response();
            }
        };
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });

    let prefix = format!("/artifacts/{pane}");
    let path = req
        .uri()
        .path()
        .strip_prefix(&prefix)
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
        HeaderValue::from_str(&format!("127.0.0.1:{port}")).expect("a host is a header value"),
    );
    if parts.headers.contains_key(header::ORIGIN) {
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
            tracing::warn!(%pane, error = %e, "app proxy: could not build the request");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let mut resp = match sender.send_request(out).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(%pane, port, error = %e, "app proxy: the program did not answer");
            return not_running(
                pane,
                "The app is running but did not answer.",
                &format!("Port {port}: {e}."),
            );
        }
    };

    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_upgrade) = client_upgrade {
            let program_upgrade = hyper::upgrade::on(&mut resp);
            let pane = pane.to_string();
            tokio::spawn(async move {
                let (Ok(client), Ok(program)) = tokio::join!(client_upgrade, program_upgrade)
                else {
                    return;
                };
                let (mut client, mut program) = (TokioIo::new(client), TokioIo::new(program));
                match tokio::io::copy_bidirectional(&mut client, &mut program).await {
                    Ok((up, down)) => {
                        tracing::debug!(%pane, up, down, "app proxy: upgraded connection closed")
                    }
                    Err(e) => {
                        tracing::debug!(%pane, error = %e, "app proxy: upgraded connection ended")
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
    // A redirect to a path of the program's own stays inside the pane's address.
    if let Some(location) = parts
        .headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
    {
        if location.starts_with('/') && !location.starts_with("//") {
            if let Ok(value) = HeaderValue::from_str(&format!("{prefix}{location}")) {
                parts.headers.insert(header::LOCATION, value);
            }
        }
    }
    Response::from_parts(parts, Body::new(body))
}
