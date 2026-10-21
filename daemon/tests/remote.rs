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

/// Connect over TLS pinning `fingerprint`, upgrade to a WebSocket with `token`, and return the
/// daemon's reply to `status`.
async fn remote_status(port: u16, fingerprint: &str, token: &str) -> anyhow::Result<String> {
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
    let url = format!("wss://localhost:{port}/ws?token={token}");
    let (mut ws, _) = tokio_tungstenite::client_async(url, tls).await?;
    ws.send(Message::text(r#"{"op":"status","req":1}"#)).await?;
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg? {
            if text.contains(r#""ev":"status""#) {
                return Ok(text.to_string());
            }
        }
    }
    anyhow::bail!("the connection closed before a status reply")
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

    let status = remote_status(remote, &fingerprint, token).await.unwrap();
    assert!(status.contains(r#""ev":"status""#), "{status}");

    let wrong_token = remote_status(remote, &fingerprint, "not-the-token").await;
    assert!(wrong_token.is_err(), "a wrong token was let in");

    let wrong_pin = remote_status(remote, &"0".repeat(64), token).await;
    assert!(wrong_pin.is_err(), "a different fingerprint was accepted");

    drop(daemon);
    let daemon = start(&dir.0, local);
    let again = remote_info(local, token).await;
    assert_eq!(
        again["fingerprint"], fingerprint,
        "restart changed the certificate"
    );
    remote_status(remote, &fingerprint, token).await.unwrap();
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
