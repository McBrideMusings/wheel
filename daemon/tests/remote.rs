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
    start_as(dir, local, "wheel")
}

fn start_as(dir: &Path, local: u16, instance: &str) -> Daemon {
    let log = dir.join("wheeld.log");
    let _ = std::fs::remove_file(&log);
    let child = Command::new(env!("CARGO_BIN_EXE_wheeld"))
        .args(["--listen", &format!("127.0.0.1:{local}")])
        .args(["--instance", instance])
        .arg("--support-dir")
        .arg(dir)
        .arg("--machine-file")
        .arg(dir.join("wheel.toml"))
        .arg("--log-dir")
        .arg(dir)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon = Daemon(child);
    let started = Instant::now();
    while !std::fs::read_to_string(&log).is_ok_and(|t| t.contains("spaces restored")) {
        assert!(
            started.elapsed() < Duration::from_secs(300),
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
    let closed = tokio::time::timeout(Duration::from_secs(60), async {
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
async fn another_instance_never_binds_the_default_instances_listener() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("wheeld-remote-dev-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let port = free_port();
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let local = free_port();
    let daemon = start_as(&dir.0, local, "wheel-dev");
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let info = remote_info(local, token.trim()).await;
    assert!(info["listen"].is_null(), "{info}");
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    drop(daemon);
}

#[tokio::test]
async fn another_instance_binds_its_own_listener() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("wheeld-remote-dev2-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let port = free_port();
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote.wheel-dev]\nlisten = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let local = free_port();
    let daemon = start_as(&dir.0, local, "wheel-dev");
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let info = remote_info(local, token.trim()).await;
    assert_eq!(info["listen"], format!("127.0.0.1:{port}"), "{info}");
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

#[tokio::test]
async fn a_peer_presenting_wrong_tokens_is_refused_even_with_a_right_one() {
    let dir = Scratch(
        std::env::temp_dir().join(format!("wheeld-remote-throttle-{}", std::process::id())),
    );
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
    let pairing = local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await
        ["token"]
        .as_str()
        .unwrap()
        .to_string();

    for _ in 0..5 {
        let refused = remote_ws(remote, &fingerprint, "token=wrong")
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("401"), "{refused}");
    }
    let blocked = remote_ws(remote, &fingerprint, &format!("pair={pairing}&name=Late"))
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("429"), "{blocked}");
    drop(daemon);
}

#[tokio::test]
async fn a_peer_past_its_connection_share_is_closed_and_gets_the_slot_back_when_it_drops() {
    use tokio::io::AsyncReadExt;
    let dir =
        Scratch(std::env::temp_dir().join(format!("wheeld-remote-cap-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{remote}\"\n"),
    )
    .unwrap();
    let daemon = start(&dir.0, local);

    // Sockets that never start a handshake each hold a slot until the daemon's 10s timeout.
    let mut held = Vec::new();
    let held_since = tokio::time::Instant::now();
    for _ in 0..16 {
        held.push(TcpStream::connect(("127.0.0.1", remote)).await.unwrap());
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut over = TcpStream::connect(("127.0.0.1", remote)).await.unwrap();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(30), over.read(&mut byte)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "the 17th connection from one peer stayed open: {closed:?}"
    );

    // A slot comes back when its socket closes. Ask until a connection is held open, giving up
    // before the daemon's 10s handshake timeout would free the held slots on its own, so only the
    // drop can let this pass (the last 500ms read still ends before that timeout).
    drop(held);
    let deadline = held_since + Duration::from_secs(9);
    loop {
        let mut again = TcpStream::connect(("127.0.0.1", remote)).await.unwrap();
        let waiting = tokio::time::timeout(Duration::from_millis(500), again.read(&mut byte)).await;
        if waiting.is_err() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a freed slot still refused a connection"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(daemon);
}

type Local = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// Send `frame` on the local listener and return the first reply carrying `"req":<req>`.
async fn local_req(ws: &mut Local, frame: serde_json::Value) -> serde_json::Value {
    let req = frame["req"].clone();
    ws.send(Message::text(frame.to_string())).await.unwrap();
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg.unwrap() {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value["req"] == req {
                return value;
            }
        }
    }
    panic!("no reply to {req}");
}

/// `local_req` for a request a scripted peer answers while it waits.
async fn local_req_after(
    ws: &mut Local,
    frame: serde_json::Value,
    peer: impl std::future::Future<Output = ()>,
) -> serde_json::Value {
    let (reply, ()) = tokio::join!(local_req(ws, frame), peer);
    reply
}

#[tokio::test]
async fn page_commands_reach_the_phone_only_while_it_shows_its_own_page() {
    let dir = Scratch(std::env::temp_dir().join(format!("wheeld-phone-page-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let project = dir.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!("[remote]\nlisten = \"127.0.0.1:{remote}\"\n"),
    )
    .unwrap();
    let _daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let token = token.trim();
    let fingerprint = remote_info(local, token).await["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();

    let (mut cli, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{local}/ws?token={token}"))
            .await
            .unwrap();
    let space = local_req(
        &mut cli,
        serde_json::json!({"op":"space_create","req":1,"cwd":project.to_str().unwrap()}),
    )
    .await;
    assert_eq!(space["ev"], "created", "{space}");
    let status = local_req(&mut cli, serde_json::json!({"op":"status","req":2})).await;
    let status: wheel_proto::ServerMsg = serde_json::from_value(status).unwrap();
    let wheel_proto::ServerMsg::Status { spaces, .. } = status else {
        panic!("expected status");
    };
    let pane = wheeld::spaces::first_pane(&spaces[0].root).to_string();
    let set = local_req(
        &mut cli,
        serde_json::json!({"op":"pane_set_kind","req":3,"pane":pane,"kind":"browser","url":"https://example.com/"}),
    )
    .await;
    assert_eq!(set["ev"], "ok", "{set}");

    let inspect = |req: u64, on: &str| {
        serde_json::json!({"op":"pane_inspect","req":req,"pane":pane,"action":"eval","arg":"1+1","on":on})
    };

    // No phone attached: the command says so, without waiting.
    let refused = local_req(&mut cli, inspect(10, "phone")).await;
    assert_eq!(refused["message"], "the phone is not attached", "{refused}");
    let refused = local_req(&mut cli, inspect(11, "tablet")).await;
    assert!(refused["message"].as_str().unwrap().contains("mac or phone"), "{refused}");

    // Pair a phone, which offers to render and shows the pane from the Mac.
    let issued = local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await;
    let pairing = issued["token"].as_str().unwrap();
    let mut phone = remote_ws(remote, &fingerprint, &format!("pair={pairing}&name=Test%20phone"))
        .await
        .unwrap();
    next_event(&mut phone, "paired").await.unwrap();
    phone.send(Message::text(r#"{"op":"render_offer"}"#)).await.unwrap();
    let mode = |m: &str| {
        Message::text(
            serde_json::json!({"op":"pane_browser_mode","req":5,"pane":pane,"mode":m}).to_string(),
        )
    };
    phone.send(mode("mac")).await.unwrap();
    next_event(&mut phone, "ok").await.unwrap();

    let refused = local_req(&mut cli, inspect(12, "phone")).await;
    assert!(
        refused["message"].as_str().unwrap().contains("from the Mac, not its own page"),
        "{refused}"
    );
    let mac = local_req(&mut cli, inspect(13, "mac")).await;
    assert!(mac["message"].as_str().unwrap().contains("no client is connected"), "{mac}");

    let info = local_req(&mut cli, serde_json::json!({"op":"pane_info","req":14,"pane":pane})).await;
    assert!(info.to_string().contains("Phone Test phone"), "{info}");
    assert!(info.to_string().contains("the Mac's page"), "{info}");

    // The phone turns to its own page: the command reaches it and its answer comes back.
    phone.send(mode("phone")).await.unwrap();
    next_event(&mut phone, "ok").await.unwrap();
    let asked = local_req_after(&mut cli, inspect(15, "phone"), async {
        let req = next_event(&mut phone, "inspect_pane").await.unwrap();
        assert_eq!(req["pane"], pane);
        assert_eq!(req["action"], "eval");
        phone
            .send(Message::text(
                serde_json::json!({"op":"inspected","id":req["id"],"text":"2"}).to_string(),
            ))
            .await
            .unwrap();
    })
    .await;
    assert_eq!(asked["text"], "2", "{asked}");

    let info = local_req(&mut cli, serde_json::json!({"op":"pane_info","req":16,"pane":pane})).await;
    assert!(info.to_string().contains("own page"), "{info}");

    // The app suspends: the connection closes and the command fails at once.
    drop(phone);
    let started = Instant::now();
    let gone = loop {
        let reply = local_req(&mut cli, inspect(17, "phone")).await;
        if reply["message"] == "the phone is not attached" || started.elapsed() > Duration::from_secs(10) {
            break reply;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(gone["message"], "the phone is not attached", "{gone}");
}

/// What the fake Apple push service received: the request path, its headers and its body.
struct Pushed {
    path: String,
    headers: std::collections::HashMap<String, String>,
    body: serde_json::Value,
}

/// An HTTP/2 server in cleartext that records every push and answers 410 `Unregistered` for a
/// token that starts with `dead`, 200 for any other.
async fn fake_apns() -> (u16, tokio::sync::mpsc::UnboundedReceiver<Pushed>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut conn = h2::server::handshake(tcp).await.unwrap();
                while let Some(Ok((request, mut respond))) = conn.accept().await {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let (parts, mut body) = request.into_parts();
                        let mut bytes = Vec::new();
                        while let Some(chunk) = body.data().await {
                            let chunk = chunk.unwrap();
                            let _ = body.flow_control().release_capacity(chunk.len());
                            bytes.extend_from_slice(&chunk);
                        }
                        let path = parts.uri.path().to_string();
                        let dead = path.starts_with("/3/device/dead");
                        let headers = parts
                            .headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
                            .collect();
                        let _ = tx.send(Pushed {
                            path,
                            headers,
                            body: serde_json::from_slice(&bytes).unwrap(),
                        });
                        let status = if dead { 410 } else { 200 };
                        let response = http::Response::builder().status(status).body(()).unwrap();
                        let mut send = respond.send_response(response, false).unwrap();
                        let reply = if dead { br#"{"reason":"Unregistered"}"#.to_vec() } else { Vec::new() };
                        let _ = send.send_data(reply.into(), true);
                    });
                }
            });
        }
    });
    (port, rx)
}

fn push_key_pem() -> String {
    use base64::Engine as _;
    use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    let rng = ring::rand::SystemRandom::new();
    let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        base64::engine::general_purpose::STANDARD.encode(der.as_ref())
    )
}

async fn next_push(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Pushed>) -> Pushed {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("a push within 20s")
        .expect("the fake push service is running")
}

#[tokio::test]
async fn a_phone_with_a_token_is_pushed_when_an_agent_needs_the_person() {
    let dir = Scratch(std::env::temp_dir().join(format!("wheeld-push-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let project = dir.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(dir.0.join("AuthKey_TESTKEY123.p8"), push_key_pem()).unwrap();
    let (apns, mut pushes) = fake_apns().await;
    let (local, remote) = (free_port(), free_port());
    std::fs::write(
        dir.0.join("wheel.toml"),
        format!(
            "[remote]\nlisten = \"127.0.0.1:{remote}\"\n\n[agent.claude]\ncommand = \"{fake}\"\n\n[push]\nafter_seconds = \"1\"\n\n[push.apns]\nkey = \"{key}\"\nkey_id = \"TESTKEY123\"\nteam_id = \"TEAM123456\"\ntopic = \"com.example.phone\"\nserver = \"http://127.0.0.1:{apns}\"\n",
            fake = env!("CARGO_BIN_EXE_wheel-fake-agent"),
            key = dir.0.join("AuthKey_TESTKEY123.p8").display(),
        ),
    )
    .unwrap();
    let _daemon = start(&dir.0, local);
    let token = std::fs::read_to_string(dir.0.join("token")).unwrap();
    let token = token.trim();
    let fingerprint = remote_info(local, token).await["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    let (mut cli, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{local}/ws?token={token}"))
            .await
            .unwrap();

    // A phone that never registered a token cannot be pushed to, and the test says so.
    let refused = local_req(&mut cli, serde_json::json!({"op":"push_test","req":1,"event":"needs_you"})).await;
    assert!(
        refused["message"].as_str().unwrap().contains("no phone has registered a push token"),
        "{refused}"
    );
    let bad = local_req(&mut cli, serde_json::json!({"op":"push_test","req":2,"event":"lunch"})).await;
    assert!(bad["message"].as_str().unwrap().contains("not a push event"), "{bad}");

    // Only a paired phone keeps a token.
    let local_register = local_req(&mut cli, serde_json::json!({"op":"push_register","req":3,"token":"abcd"})).await;
    assert_eq!(local_register["ev"], "error", "{local_register}");
    let issued = local_call(local, token, r#"{"op":"pairing_issue","req":1}"#, "pairing").await;
    let pairing = issued["token"].as_str().unwrap();
    let mut phone = remote_ws(remote, &fingerprint, &format!("pair={pairing}&name=Test%20phone"))
        .await
        .unwrap();
    next_event(&mut phone, "paired").await.unwrap();
    phone
        .send(Message::text(r#"{"op":"push_register","req":7,"token":"not hex!"}"#))
        .await
        .unwrap();
    assert_eq!(next_event(&mut phone, "error").await.unwrap()["req"], 7);
    phone
        .send(Message::text(r#"{"op":"push_register","req":8,"token":"AB12cd34"}"#))
        .await
        .unwrap();
    assert_eq!(next_event(&mut phone, "ok").await.unwrap()["req"], 8);

    // The test notification reaches Apple's service signed, with the pane a tap opens.
    let space = local_req(
        &mut cli,
        serde_json::json!({"op":"space_create","req":4,"cwd":project.to_str().unwrap()}),
    )
    .await;
    assert_eq!(space["ev"], "created", "{space}");
    let sent = local_req(&mut cli, serde_json::json!({"op":"push_test","req":5,"event":"needs_you"})).await;
    assert_eq!(sent["ev"], "ok", "{sent}");
    let push = next_push(&mut pushes).await;
    assert_eq!(push.path, "/3/device/ab12cd34");
    assert!(push.headers["authorization"].starts_with("bearer ey"), "{:?}", push.headers);
    assert_eq!(push.headers["apns-topic"], "com.example.phone");
    assert_eq!(push.headers["apns-push-type"], "alert");
    assert_eq!(push.body["event"], "needs_you");
    assert!(push.body["pane"].as_str().is_some_and(|p| p.starts_with('p')), "{}", push.body);

    // An agent waiting on a permission pushes the request's own words, once.
    let status = local_req(&mut cli, serde_json::json!({"op":"status","req":6})).await;
    let status: wheel_proto::ServerMsg = serde_json::from_value(status).unwrap();
    let wheel_proto::ServerMsg::Status { spaces, .. } = status else { panic!("expected status") };
    let pane = wheeld::spaces::first_pane(&spaces[0].root).to_string();
    let started = local_req(
        &mut cli,
        serde_json::json!({"op":"pane_start","req":9,"pane":pane,"kind":"agent"}),
    )
    .await;
    assert_eq!(started["ev"], "started", "{started}");
    let agent = started["agent"].as_str().unwrap().to_string();
    let queued = local_req(
        &mut cli,
        serde_json::json!({"op":"agent_send","req":10,"agent":agent,"text":"needs permission"}),
    )
    .await;
    assert_eq!(queued["ev"], "agent_queued", "{queued}");
    let waiting = next_push(&mut pushes).await;
    assert_eq!(waiting.body["event"], "needs_you");
    assert_eq!(waiting.body["pane"], pane.as_str());
    assert_eq!(waiting.body["aps"]["alert"]["body"], "rm -rf build");
    assert!(
        waiting.body["aps"]["alert"]["title"].as_str().unwrap().ends_with("needs you"),
        "{}",
        waiting.body
    );

    // A token Apple calls unregistered is forgotten: the next test finds no phone.
    phone
        .send(Message::text(r#"{"op":"push_register","req":11,"token":"dead01"}"#))
        .await
        .unwrap();
    next_event(&mut phone, "ok").await.unwrap();
    let dead = local_req(&mut cli, serde_json::json!({"op":"push_test","req":12,"event":"failed"})).await;
    assert!(dead["message"].as_str().unwrap().contains("Unregistered"), "{dead}");
    let gone = local_req(&mut cli, serde_json::json!({"op":"push_test","req":13,"event":"failed"})).await;
    assert!(gone["message"].as_str().unwrap().contains("no phone has registered"), "{gone}");
}
