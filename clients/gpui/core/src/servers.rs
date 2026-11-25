//! The remote wheelds this app is paired with (ADR-0057): the list in the app's preferences,
//! each credential in the Keychain under this app's own service, so no other program's item is
//! read and no cross-binary Keychain prompt appears. The local wheeld is built in and never listed.
use crate::daemon::{Endpoint, socket};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tungstenite::Message;
use wheel_proto::{ClientMsg, ServerMsg};

/// The Keychain service each credential is kept under, the account being the server's id.
pub const KEYCHAIN_SERVICE: &str = "Wheel App Server";

/// One paired remote wheeld.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Server {
    /// `status.host.id`: what windows, the list and the Keychain key it by.
    pub id: String,
    /// `status.host.name` when it paired.
    pub name: String,
    /// `wss://host:port/ws`
    pub url: String,
    /// SHA-256 of the certificate pinned when it paired, lowercase hex.
    pub fingerprint: String,
    /// The device id the server lists this app as, for `wheel remote revoke` there.
    pub device: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}
fn yes() -> bool {
    true
}

/// The code `wheel remote pair` prints.
#[derive(Deserialize)]
struct PairingCode {
    url: String,
    token: String,
    cert_sha256: String,
}

/// Where the list is kept: `WHEEL_GPUI_SERVERS_FILE` for a check, else the app's support folder.
pub fn default_file() -> PathBuf {
    std::env::var_os("WHEEL_GPUI_SERVERS_FILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/wheel-gpui/servers.json"))
}

pub fn load(file: &std::path::Path) -> Vec<Server> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(file: &std::path::Path, list: &[Server]) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(&tmp, file).with_context(|| format!("writing {}", file.display()))
}

/// The endpoint for `server`, with its credential read from the Keychain.
pub fn endpoint(server: &Server) -> Result<Endpoint> {
    let credential = security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, &server.id).with_context(|| format!("reading {}'s credential from the Keychain", server.id))?;
    let credential = String::from_utf8(credential).context("the credential is not text")?;
    Ok(Endpoint::remote(&server.url, &server.id, &credential, &server.fingerprint))
}

/// How long pairing waits for each answer.
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Pair with the wheeld whose pairing code is `code` (the JSON `wheel remote pair` prints), as
/// `device_name`; keep its credential and list it in `file`, replacing an entry with its id.
/// Blocks on the network: run it off the main thread.
pub fn pair(file: &std::path::Path, code: &str, device_name: &str) -> Result<Server> {
    let code: PairingCode = serde_json::from_str(code.trim()).context("that is not a pairing code")?;
    if !code.url.starts_with("wss://") {
        bail!("the pairing code's address must start with wss://");
    }
    if code.cert_sha256.len() != 64 || !code.cert_sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("the pairing code's certificate fingerprint is not 64 hex digits");
    }
    let fingerprint = code.cert_sha256.to_ascii_lowercase();
    let escape = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let mut ws = socket(&format!("{}?pair={}&name={}", code.url, escape(&code.token), escape(device_name)), Some(&fingerprint))?;
    if let Some(stream) = match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Rustls(s) => Some(&s.sock),
        tungstenite::stream::MaybeTlsStream::Plain(s) => Some(s),
        _ => None,
    } {
        stream.set_read_timeout(Some(WAIT))?;
    }
    fn next(ws: &mut crate::daemon::Socket) -> Result<ServerMsg> {
        loop {
            match ws.read().context("waiting for wheeld")? {
                Message::Text(t) => {
                    if let Ok(m) = serde_json::from_str::<ServerMsg>(&t) {
                        return Ok(m);
                    }
                }
                Message::Close(_) => bail!("wheeld closed the connection"),
                _ => {}
            }
        }
    }
    // The server spends the token and sends the credential as the connection's first frame.
    let (device, credential, id) = loop {
        match next(&mut ws)? {
            ServerMsg::Paired { device, credential, server } => break (device, credential, server),
            ServerMsg::Error { message, .. } => bail!("wheeld refused the code: {message}"),
            _ => {}
        }
    };
    // From here the code is spent and the server lists this app: every failure says which device.
    let orphan = |why: String| anyhow::anyhow!("{why}; the server lists this app as {device}: run `wheel remote revoke {device}` there");
    if id.is_empty() {
        return Err(orphan(format!("the wheeld at {} reports no id; restart it to update, then pair again", code.url)));
    }
    ws.send(Message::text(crate::daemon::frame(&ClientMsg::Status { req: Some(1) }))).map_err(|e| orphan(e.to_string()))?;
    let host = loop {
        match next(&mut ws).map_err(|e| orphan(format!("{e:#}")))? {
            ServerMsg::Status { host, .. } => break host,
            _ => continue,
        }
    };
    if host.id != id {
        return Err(orphan(format!("the wheeld paired as {id} reports {} in its status", host.id)));
    }
    let _ = ws.close(None);
    security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, &id, credential.as_bytes()).map_err(|e| orphan(format!("keeping the credential in the Keychain: {e}")))?;
    let server = Server { id: id.clone(), name: host.name.clone(), url: code.url, fingerprint, device: device.clone(), enabled: true };
    let mut list = load(file);
    let before = list.clone();
    match list.iter_mut().find(|s| s.id == id) {
        Some(held) => *held = server.clone(),
        None => list.push(server.clone()),
    }
    if let Err(why) = save(file, &list) {
        if !before.iter().any(|s| s.id == id) {
            let _ = security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, &id);
        }
        return Err(orphan(format!("{why:#}")));
    }
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_that_is_not_wss_or_has_a_bad_fingerprint_is_refused_before_it_is_spent() {
        let file = std::env::temp_dir().join("wheel-gpui-servers-test.json");
        let bad = pair(&file, r#"{"url":"ws://127.0.0.1:1/ws","token":"t","cert_sha256":"00"}"#, "t").unwrap_err();
        assert!(format!("{bad:#}").contains("wss://"), "{bad:#}");
        let bad = pair(&file, r#"{"url":"wss://127.0.0.1:1/ws","token":"t","cert_sha256":"zz"}"#, "t").unwrap_err();
        assert!(format!("{bad:#}").contains("64 hex"), "{bad:#}");
    }

    #[test]
    fn the_list_round_trips() {
        let file = std::env::temp_dir().join(format!("wheel-gpui-servers-{}.json", std::process::id()));
        let list = vec![Server { id: "a-1".into(), name: "dev".into(), url: "wss://h:1/ws".into(), fingerprint: "ab".into(), device: "d1".into(), enabled: true }];
        save(&file, &list).unwrap();
        assert_eq!(load(&file), list);
        let _ = std::fs::remove_file(file);
    }
}
