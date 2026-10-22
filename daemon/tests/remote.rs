//! The remote listener: TLS with a self-signed certificate a client pins by fingerprint, the same
//! token check as the local listener, off unless `remote.listen` is set.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;

/// A scratch directory removed when the test ends, failed or not.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running wheeld, killed when dropped.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(dir: &Path, local: u16) -> Daemon {
    let log = dir.join("wheeld.log");
    let _ = std::fs::remove_file(&log);
    let child = Command::new(env!("CARGO_BIN_EXE_wheeld"))
        .args(["--listen", &format!("127.0.0.1:{local}")])
        .arg("--support-dir")
        .arg(dir)
        .arg("--log-dir")
        .arg(dir)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon = Daemon(child);
    let started = Instant::now();
    while !std::fs::read_to_string(&log).is_ok_and(|t| t.contains("spaces restored")) {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "wheeld never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon
}

/// Accepts the one certificate whose fingerprint it was given, and nothing else.
#[derive(Debug)]
struct Pinned(String);

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match wheeld::remote::fingerprint(end_entity) == self.0 {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::General("fingerprint does not match".into())),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

type Remote = tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

/// Connect over TLS pinning `fingerprint` and upgrade to a WebSocket at `/ws?<query>`.
async fn remote_ws(port: u16, fingerprint: &str, query: &str) -> anyhow::Result<Remote> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(Pinned(fingerprint.to_string())))
    .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost")?, tcp)
        .await?;
    let url = format!("wss://localhost:{port}/ws?{query}");
    let (ws, _) = tokio_tungstenite::client_async(url, tls).await?;
    Ok(ws)
}

/// The next frame of `ev` kind.
async fn next_event(ws: &mut Remote, ev: &str) -> anyhow::Result<serde_json::Value> {
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg? {
            let value: serde_json::Value = serde_json::from_str(&text)?;
            if value["ev"] == ev {
                return Ok(value);
            }
        }
    }
    anyhow::bail!("the connection closed before a {ev} frame")
}

/// Connect with `token` and return the daemon's reply to `status`.
async fn remote_status(port: u16, fingerprint: &str, token: &str) -> anyhow::Result<String> {
    let mut ws = remote_ws(port, fingerprint, &format!("token={token}")).await?;
    ws.send(Message::text(r#"{"op":"status","req":1}"#)).await?;
    Ok(next_event(&mut ws, "status").await?.to_string())
}

/// A frame sent to the daemon on the local listener, and the reply of kind `ev`.
async fn local_call(local: u16, token: &str, frame: &str, ev: &str) -> serde_json::Value {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{local}/ws?token={token}"))
            .await
            .unwrap();
    ws.send(Message::text(frame)).await.unwrap();
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg.unwrap() {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value["ev"] == ev {
                return value;
            }
        }
    }
    panic!("no {ev} reply");
}

/// `remote_info`'s reply from the local listener.
async fn remote_info(local: u16, token: &str) -> serde_json::Value {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{local}/ws?token={token}"))
            .await
            .unwrap();
    ws.send(Message::text(r#"{"op":"remote_info","req":1}"#))
        .await
        .unwrap();
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg.unwrap() {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value["ev"] == "remote" {
                return value;
            }
        }
    }
    panic!("no remote reply");
}

#[tokio::test]
async fn remote_listener_pins_the_certificate() {
    let dir = Scratch(std::env::temp_dir().join(format!("wheeld-remote-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{remote}\"\n"),
    )
    .unwrap();

    let daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let token = token.trim();
    let info = remote_info(local, token).await;
    assert_eq!(info["listen"], format!("127.0.0.1:{remote}"));
    assert_eq!(info["port"], remote);
    let fingerprint = info["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fingerprint.len(), 64);
    assert!(info["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "127.0.0.1"));

    let issued = local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await;
    let pairing = issued["token"].as_str().unwrap().to_string();
    let mut phone = remote_ws(
        remote,
        &fingerprint,
        &format!("pair={pairing}&name=Test%20phone"),
    )
    .await
    .unwrap();
    let paired = next_event(&mut phone, "paired").await.unwrap();
    let credential = paired["credential"].as_str().unwrap().to_string();
    drop(phone);

    let status = remote_status(remote, &fingerprint, &credential)
        .await
        .unwrap();
    assert!(status.contains(r#""ev":"status""#), "{status}");

    let wrong_token = remote_status(remote, &fingerprint, "not-the-token").await;
    assert!(wrong_token.is_err(), "a wrong token was let in");

    let file_token = remote_status(remote, &fingerprint, token).await;
    assert!(
        file_token.is_err(),
        "the remote listener took the token file's token"
    );

    let wrong_pin = remote_status(remote, &"0".repeat(64), &credential).await;
    assert!(wrong_pin.is_err(), "a different fingerprint was accepted");

    drop(daemon);
    let daemon = start(&dir.0, local);
    let again = remote_info(local, token).await;
    assert_eq!(
        again["fingerprint"], fingerprint,
        "restart changed the certificate"
    );
    remote_status(remote, &fingerprint, &credential)
        .await
        .expect("a paired device reconnects after a restart without a new code");
    drop(daemon);
}

#[tokio::test]
async fn a_pairing_token_works_once_and_a_revoke_ends_only_that_device() {
    let dir = Scratch(std::env::temp_dir().join(format!("wheeld-pairing-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{remote}\"\n"),
    )
    .unwrap();
    let daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let token = token.trim();
    let info = remote_info(local, token).await;
    let fingerprint = info["fingerprint"].as_str().unwrap().to_string();

    let issue = || async {
        local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await["token"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (first, second) = (issue().await, issue().await);
    let mut one = remote_ws(remote, &fingerprint, &format!("pair={first}&name=One"))
        .await
        .unwrap();
    let one_credential = next_event(&mut one, "paired").await.unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        remote_ws(remote, &fingerprint, &format!("pair={first}&name=Copy"))
            .await
            .is_err(),
        "a second phone presenting the same token was let in"
    );
    let mut two = remote_ws(remote, &fingerprint, &format!("pair={second}&name=Two"))
        .await
        .unwrap();
    let two_credential = next_event(&mut two, "paired").await.unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        remote_ws(remote, &fingerprint, "pair=not-issued&name=x")
            .await
            .is_err(),
        "a token nobody issued was let in"
    );

    let devices = local_call(local, token, r#"{"op":"device_list","req":1}"#, "devices").await;
    let list = devices["devices"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{devices}");
    assert_eq!(list[0]["name"], "One");
    let one_id = list[0]["id"].as_str().unwrap().to_string();

    local_call(
        local,
        token,
        &format!(r#"{{"op":"device_revoke","req":1,"id":"{one_id}"}}"#),
        "ok",
    )
    .await;
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = one.next().await {
            if msg.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the revoked device's connection stayed open"
    );
    assert!(remote_status(remote, &fingerprint, &one_credential)
        .await
        .is_err());
    remote_status(remote, &fingerprint, &two_credential)
        .await
        .expect("the other device stays paired");
    // A paired phone does not own the Mac's windows or its pairing: those ops are the machine's.
    two.send(Message::text(r#"{"op":"window_offer"}"#))
        .await
        .unwrap();
    two.send(Message::text(r#"{"op":"window","req":9,"command":"ls"}"#))
        .await
        .unwrap();
    let refusal = loop {
        let frame = next_event(&mut two, "error").await.unwrap();
        if frame["req"] == 9 {
            break frame;
        }
    };
    assert!(
        refusal["message"]
            .as_str()
            .unwrap()
            .contains("managed from the machine"),
        "{refusal}"
    );
    let _ = two.close(None).await;
    drop(daemon);
}

#[tokio::test]
async fn a_cancelled_pairing_token_is_refused() {
    let dir = Scratch(std::env::temp_dir().join(format!("wheeld-cancel-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{remote}\"\n"),
    )
    .unwrap();
    let daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let token = token.trim();
    let fingerprint = remote_info(local, token).await["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    let issue = || async {
        local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await["token"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (first, second) = (issue().await, issue().await);
    let frame = format!(r#"{{"op":"pairing_cancel","req":2,"token":"{first}"}}"#);
    local_call(local, token, &frame, "ok").await;
    let refused = remote_ws(remote, &fingerprint, &format!("pair={first}&name=Late")).await;
    assert!(
        refused.is_err() || next_event(&mut refused.unwrap(), "paired").await.is_err(),
        "a cancelled token paired a device"
    );
    // Only the named token is gone.
    let mut phone = remote_ws(remote, &fingerprint, &format!("pair={second}&name=Other"))
        .await
        .unwrap();
    next_event(&mut phone, "paired")
        .await
        .expect("a token that was not cancelled pairs");
    // A paired phone cannot cancel tokens: that is the machine's.
    let mut cancel = phone;
    let frame = format!(
        r#"{{"op":"pairing_cancel","req":7,"token":"{fresh}"}}"#,
        fresh = issue().await
    );
    cancel.send(Message::text(frame)).await.unwrap();
    let refusal = loop {
        let frame = next_event(&mut cancel, "error").await.unwrap();
        if frame["req"] == 7 {
            break frame;
        }
    };
    assert!(refusal["message"]
        .as_str()
        .unwrap()
        .contains("managed from the machine"));
    drop(daemon);
}

#[tokio::test]
async fn nothing_listens_remotely_when_unset() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("wheeld-remote-off-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let (local, other) = (free_port(), free_port());
    let daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let info = remote_info(local, token.trim()).await;
    assert!(info["listen"].is_null(), "{info}");
    assert_eq!(info["fingerprint"], "");
    assert!(!dir.0.join("remote-cert.pem").exists());
    assert!(TcpStream::connect(("127.0.0.1", other)).await.is_err());
    drop(daemon);
}

#[tokio::test]
async fn a_listener_that_cannot_bind_is_reported_not_called_off() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("wheeld-remote-taken-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let local = free_port();
    let daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let info = remote_info(local, token.trim()).await;
    assert_eq!(info["listen"], format!("127.0.0.1:{port}"));
    assert!(
        info["problem"].as_str().unwrap().contains("binding"),
        "{info}"
    );
    drop(daemon);
}
