//! Push notifications to the paired phone (wheel-zpa4.2.8).
//!
//! The phone gives `push_register` its Apple device token; the daemon keeps it with the phone's
//! credential ([`crate::devices`]). When an agent starts waiting on the person (`needs_you`),
//! stops for good (`failed`) or finishes a turn that ran longer than `push.after_seconds`
//! (`turn_done`), and `push.events` lists that event, the daemon sends the notification to Apple's
//! push service itself, over HTTP/2 with a token signed by the push key (`push.apns.*`).
//!
//! A token Apple says is dead (`BadDeviceToken`, `Unregistered`, 410) is forgotten, so the phone's
//! next connection registers the current one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use tokio::sync::broadcast::error::RecvError;
use wheel_proto::{AgentState, PaneKind, PaneStatus, ServerMsg};

use crate::server::AppState;
use crate::settings::{
    PUSH_AFTER_S, PUSH_EVENTS, PUSH_KEY, PUSH_KEY_ID, PUSH_SERVER, PUSH_TEAM_ID, PUSH_TOPIC,
};
use crate::spaces::panes;

/// How long one request to Apple may take, connecting included.
const REQUEST_LIFE: Duration = Duration::from_secs(20);
/// Apple wants a token refreshed no more often than every 20 minutes and no older than an hour.
const JWT_LIFE: Duration = Duration::from_secs(40 * 60);

/// What a notification is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    NeedsYou,
    Failed,
    TurnDone,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::NeedsYou => "needs_you",
            Event::Failed => "failed",
            Event::TurnDone => "turn_done",
        }
    }

    pub fn parse(word: &str) -> Option<Event> {
        [Event::NeedsYou, Event::Failed, Event::TurnDone]
            .into_iter()
            .find(|e| e.name() == word)
    }
}

/// One notification: the pane it opens when tapped and the words it shows.
#[derive(Clone, Debug)]
pub struct Notice {
    pub event: Event,
    pub space: String,
    pub pane: String,
    pub title: String,
    pub body: String,
}

impl Notice {
    /// The request body Apple takes. `pane` and `space` ride beside `aps` so a tap opens the pane.
    pub fn payload(&self) -> String {
        serde_json::json!({
            "aps": {
                "alert": {"title": self.title, "body": self.body},
                "sound": "default",
                "thread-id": self.space,
                "category": self.event.name(),
            },
            "event": self.event.name(),
            "space": self.space,
            "pane": self.pane,
        })
        .to_string()
    }
}

/// The signing key and a token made with it, kept until it ages out.
#[derive(Default)]
pub struct Push {
    jwt: Mutex<Option<(String, Instant, String)>>,
}

/// Where and as whom to send, from the machine's settings.
struct Config {
    key: String,
    key_id: String,
    team_id: String,
    topic: String,
    server: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The PKCS#8 bytes inside an Apple `.p8` file (PEM, one base64 block).
fn pem_der(text: &str) -> Result<Vec<u8>, String> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----") && !l.trim().is_empty())
        .collect();
    STANDARD
        .decode(body.trim())
        .map_err(|_| "the push key file is not a PEM private key".to_string())
}

/// An ES256 JSON web token for Apple: signed with the push key, naming its id and the team.
pub fn make_jwt(pem: &str, key_id: &str, team_id: &str, issued: u64) -> Result<String, String> {
    let der = pem_der(pem)?;
    let rng = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
        .map_err(|_| "the push key file does not hold an ES256 (P-256) private key".to_string())?;
    let header = URL_SAFE_NO_PAD.encode(
        serde_json::json!({"alg": "ES256", "kid": key_id})
            .to_string()
            .as_bytes(),
    );
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::json!({"iss": team_id, "iat": issued})
            .to_string()
            .as_bytes(),
    );
    let signing = format!("{header}.{claims}");
    let signature = pair
        .sign(&rng, signing.as_bytes())
        .map_err(|_| "signing the push token failed".to_string())?;
    Ok(format!(
        "{signing}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

/// Apple's answer to one push.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Sent,
    /// Apple will never deliver to this token.
    DeadToken(String),
    Failed(String),
}

impl AppState {
    fn push_config(&self) -> Result<Config, String> {
        let get = |id: &str| self.setting(id, None);
        let config = Config {
            key: get(PUSH_KEY)?,
            key_id: get(PUSH_KEY_ID)?,
            team_id: get(PUSH_TEAM_ID)?,
            topic: get(PUSH_TOPIC)?,
            server: get(PUSH_SERVER)?,
        };
        for (id, value) in [
            (PUSH_KEY, &config.key),
            (PUSH_KEY_ID, &config.key_id),
            (PUSH_TEAM_ID, &config.team_id),
            (PUSH_TOPIC, &config.topic),
        ] {
            if value.is_empty() {
                return Err(format!("{id} is not set in this machine's wheel.toml"));
            }
        }
        Ok(config)
    }

    fn push_jwt(&self, config: &Config) -> Result<String, String> {
        let mut cached = self.push.jwt.lock().unwrap();
        let identity = format!("{}|{}|{}", config.key, config.key_id, config.team_id);
        if let Some((jwt, made, who)) = cached.as_ref() {
            if made.elapsed() < JWT_LIFE && *who == identity {
                return Ok(jwt.clone());
            }
        }
        let pem = std::fs::read_to_string(&config.key)
            .map_err(|e| format!("cannot read the push key {}: {e}", config.key))?;
        let jwt = make_jwt(&pem, &config.key_id, &config.team_id, now_secs())?;
        *cached = Some((jwt.clone(), Instant::now(), identity));
        Ok(jwt)
    }

    /// Send `notice` to every phone with a push token. How many Apple accepted, or why none did.
    pub(crate) async fn push_send(&self, notice: &Notice) -> Result<usize, String> {
        let config = self.push_config()?;
        let targets = self.devices.push_tokens();
        if targets.is_empty() {
            return Err("no phone has registered a push token; open the phone app, allow notifications and connect it".into());
        }
        let jwt = self.push_jwt(&config)?;
        let payload = notice.payload();
        let collapse = format!("{}-{}", notice.pane, notice.event.name());
        let mut sent = 0;
        let mut errors = Vec::new();
        for (device, token) in targets {
            let outcome = deliver(&config, &jwt, &token, &payload, &collapse).await;
            match outcome {
                Outcome::Sent => {
                    sent += 1;
                    tracing::info!(%device, event = notice.event.name(), pane = %notice.pane, "push sent");
                }
                Outcome::DeadToken(reason) => {
                    tracing::warn!(%device, %reason, "push token refused; forgotten");
                    self.devices.set_push_token(&device, None);
                    errors.push(format!("{device}: {reason}"));
                }
                Outcome::Failed(why) => {
                    tracing::warn!(%device, %why, "push failed");
                    errors.push(format!("{device}: {why}"));
                }
            }
        }
        match sent {
            0 => Err(format!("no notification was accepted: {}", errors.join("; "))),
            n => Ok(n),
        }
    }

    /// The notification a test of `event` sends, naming a real pane when the machine has one.
    pub(crate) fn push_test_notice(&self, event: Event) -> Notice {
        let spaces = self.spaces();
        let (space, pane) = spaces
            .spaces
            .first()
            .map(|s| {
                (
                    s.name.clone(),
                    crate::spaces::first_pane(&s.root).to_string(),
                )
            })
            .unwrap_or_default();
        let (title, body) = match event {
            Event::NeedsYou => (format!("{space} needs you"), "Test: an agent is waiting"),
            Event::Failed => (format!("{space} stopped"), "Test: an agent stopped"),
            Event::TurnDone => (format!("{space} finished"), "Test: a long turn ended"),
        };
        Notice {
            event,
            space,
            pane,
            title,
            body: body.to_string(),
        }
    }

    fn push_wanted(&self, event: Event) -> bool {
        self.setting(PUSH_EVENTS, None)
            .is_ok_and(|list| list.split(',').any(|e| e == event.name()))
    }

    /// Send `notice` in the background when `push.events` lists its event and a phone is waiting.
    fn push_notify(self: &Arc<Self>, notice: Notice) {
        if !self.push_wanted(notice.event) || self.devices.push_tokens().is_empty() {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            if let Err(why) = state.push_send(&notice).await {
                tracing::warn!(event = notice.event.name(), pane = %notice.pane, %why, "push not sent");
            }
        });
    }

    /// The space and pane of an agent pane, with the space's name.
    fn agent_pane(&self, agent: &str) -> Option<(String, String, String)> {
        let spaces = self.spaces();
        spaces.spaces.iter().find_map(|space| {
            panes(&space.root).into_iter().find_map(|(pane, kind)| match kind {
                PaneKind::Agent { agent: a, .. } if a == agent => {
                    Some((space.id.clone(), space.name.clone(), pane.to_string()))
                }
                _ => None,
            })
        })
    }

    fn pane_space(&self, pane: &str) -> Option<(String, String)> {
        let spaces = self.spaces();
        spaces
            .spaces
            .iter()
            .find(|s| panes(&s.root).into_iter().any(|(p, _)| p == pane))
            .map(|s| (s.id.clone(), s.name.clone()))
    }

    /// What the agent in `pane` is waiting on, from the permission request in front of it.
    fn waiting_on(&self, pane: &str) -> String {
        let agent = self.spaces().spaces.iter().find_map(|s| {
            panes(&s.root).into_iter().find_map(|(p, kind)| match kind {
                PaneKind::Agent { agent, .. } if p == pane => Some(agent.clone()),
                _ => None,
            })
        });
        let perms = self.perms.list();
        perms
            .iter()
            .find(|p| agent.as_deref() == Some(p.agent.as_str()) || p.agent == pane)
            .map_or_else(|| "Waiting for you".to_string(), |p| p.title.clone())
    }
}

/// POST one notification to Apple's push service over HTTP/2.
async fn deliver(config: &Config, jwt: &str, token: &str, payload: &str, collapse: &str) -> Outcome {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{}/3/device/{token}", config.server.trim_end_matches('/')))
        .header("authorization", format!("bearer {jwt}"))
        .header("apns-topic", &config.topic)
        .header("apns-push-type", "alert")
        .header("apns-priority", "10")
        .header("apns-collapse-id", collapse)
        .header("content-type", "application/json")
        .body(());
    let request = match request {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(format!("bad request: {e}")),
    };
    let sent = tokio::time::timeout(
        REQUEST_LIFE,
        post(&config.server, request, payload.as_bytes().to_vec()),
    )
    .await;
    let (status, body) = match sent {
        Ok(Ok(answer)) => answer,
        Ok(Err(e)) => return Outcome::Failed(e),
        Err(_) => return Outcome::Failed(format!("no answer from {} in {}s", config.server, REQUEST_LIFE.as_secs())),
    };
    if status == 200 {
        return Outcome::Sent;
    }
    let reason = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["reason"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("HTTP {status}"));
    match (status, reason.as_str()) {
        (410, _) | (_, "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic") => {
            Outcome::DeadToken(reason)
        }
        _ => Outcome::Failed(format!("{reason} (HTTP {status})")),
    }
}

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

/// One HTTP/2 exchange: connect to `server` (`https://host[:port]` over TLS, or `http://` in
/// cleartext for a test endpoint), send `request` with `body`, and return the status and body.
async fn post(
    server: &str,
    request: http::Request<()>,
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>), String> {
    let (tls, rest) = match server.split_once("://") {
        Some(("https", rest)) => (true, rest),
        Some(("http", rest)) => (false, rest),
        _ => return Err(format!("{server} is not an http(s) URL")),
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port in {server}"))?),
        None => (authority, if tls { 443 } else { 80 }),
    };
    let tcp = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|e| format!("cannot reach {authority}: {e}"))?;
    let io: Box<dyn Io> = if tls {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let name = rustls_pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| format!("{host} is not a host name"))?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .map_err(|e| format!("TLS to {authority} failed: {e}"))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let (mut client, connection) = h2::client::handshake(io)
        .await
        .map_err(|e| format!("HTTP/2 to {authority} failed: {e}"))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let result = async {
        client = client.ready().await.map_err(|e| e.to_string())?;
        let (response, mut stream) = client
            .send_request(request, false)
            .map_err(|e| e.to_string())?;
        stream
            .send_data(body.into(), true)
            .map_err(|e| e.to_string())?;
        let response = response.await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let mut incoming = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = incoming.data().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            let _ = incoming.flow_control().release_capacity(chunk.len());
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
    .await;
    driver.abort();
    result
}

/// Follow the events that notify and send a notification for each the settings list.
pub(crate) fn follow(state: Arc<AppState>) {
    let mut events = state.global.subscribe();
    tokio::spawn(async move {
        let mut statuses: HashMap<String, PaneStatus> = HashMap::new();
        let mut busy: HashMap<String, Instant> = HashMap::new();
        loop {
            match events.recv().await {
                Ok(ServerMsg::StatusChanged {
                    space,
                    pane: Some(pane),
                    status,
                    ..
                }) => {
                    let old = match status {
                        Some(new) => statuses.insert(pane.clone(), new),
                        None => statuses.remove(&pane),
                    };
                    let Some(new) = status else { continue };
                    let name = state.pane_space(&pane).map_or(space.clone(), |(_, n)| n);
                    match new {
                        PaneStatus::NeedsYou { .. } if !matches!(old, Some(PaneStatus::NeedsYou { .. })) => {
                            let body = state.waiting_on(&pane);
                            state.push_notify(Notice {
                                event: Event::NeedsYou,
                                space,
                                pane,
                                title: format!("{name} needs you"),
                                body,
                            });
                        }
                        PaneStatus::Stopped if old.is_some_and(|o| o != PaneStatus::Stopped) => {
                            state.push_notify(Notice {
                                event: Event::Failed,
                                space,
                                pane,
                                title: format!("{name} stopped"),
                                body: "The agent stopped and will not run again".into(),
                            });
                        }
                        _ => {}
                    }
                }
                Ok(ServerMsg::AgentStateChanged { agent, state: now }) => match now {
                    AgentState::Busy { .. } => {
                        busy.entry(agent).or_insert_with(Instant::now);
                    }
                    AgentState::Ready => {
                        let Some(since) = busy.remove(&agent) else { continue };
                        let after = state
                            .setting(PUSH_AFTER_S, None)
                            .ok()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(120);
                        if since.elapsed() < Duration::from_secs(after) {
                            continue;
                        }
                        if let Some((space, name, pane)) = state.agent_pane(&agent) {
                            state.push_notify(Notice {
                                event: Event::TurnDone,
                                space,
                                pane,
                                title: format!("{name} finished"),
                                body: format!("A turn of {} min ended", since.elapsed().as_secs() / 60),
                            });
                        }
                    }
                    _ => {
                        busy.remove(&agent);
                    }
                },
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};

    fn key_pem() -> (String, Vec<u8>) {
        let rng = SystemRandom::new();
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let public = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng)
            .unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            STANDARD.encode(der.as_ref())
        );
        (pem, ring::signature::KeyPair::public_key(&public).as_ref().to_vec())
    }

    #[test]
    fn the_token_names_the_key_and_team_and_verifies_against_the_public_key() {
        let (pem, public) = key_pem();
        let jwt = make_jwt(&pem, "ABC123DEFG", "TEAM123456", 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header, serde_json::json!({"alg":"ES256","kid":"ABC123DEFG"}));
        assert_eq!(claims, serde_json::json!({"iss":"TEAM123456","iat":1_700_000_000u64}));
        let signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("the signature verifies");
    }

    #[test]
    fn a_file_that_is_not_a_key_is_refused_in_words() {
        assert!(make_jwt("not a key", "K", "T", 0).is_err());
        let err = make_jwt("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----", "K", "T", 0)
            .unwrap_err();
        assert!(err.contains("ES256"), "{err}");
    }

    #[test]
    fn the_payload_carries_the_pane_a_tap_opens() {
        let notice = Notice {
            event: Event::NeedsYou,
            space: "s1".into(),
            pane: "p7".into(),
            title: "wheel needs you".into(),
            body: "Allow Bash: ls".into(),
        };
        let v: serde_json::Value = serde_json::from_str(&notice.payload()).unwrap();
        assert_eq!(v["pane"], "p7");
        assert_eq!(v["space"], "s1");
        assert_eq!(v["aps"]["alert"]["title"], "wheel needs you");
        assert_eq!(v["aps"]["alert"]["body"], "Allow Bash: ls");
    }
}
