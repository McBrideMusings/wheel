//! Named leases a connection holds until it releases them or closes: one holder per key, and a claim from
//! another connection takes the key over. Voice mode holds `voice`.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Mutex;

/// Each change is announced while the table is locked, so announcements arrive in the order the changes
/// happened and the last one every connection hears names the real holder.
#[derive(Default)]
pub(crate) struct Leases {
    held: Mutex<HashMap<String, SocketAddr>>,
}

/// A key is 1–64 characters of `a-z`, `0-9`, `.`, `_` and `-`: a name, never data.
fn check_key(key: &str) -> Result<(), String> {
    let ok = (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "lease key {key:?} is not 1-64 characters of a-z, 0-9, '.', '_' or '-'"
        ))
    }
}

impl Leases {
    /// Give `key` to `peer` and announce it; returns the holder's id as `lease` names it.
    pub(crate) fn claim(
        &self,
        key: &str,
        peer: SocketAddr,
        announce: impl Fn(&str, Option<String>),
    ) -> Result<String, String> {
        check_key(key)?;
        let mut held = self.held.lock().unwrap();
        held.insert(key.to_owned(), peer);
        let owner = peer.to_string();
        announce(key, Some(owner.clone()));
        Ok(owner)
    }

    /// `peer` gives `key` up, announced; refused when it does not hold it.
    pub(crate) fn release(
        &self,
        key: &str,
        peer: SocketAddr,
        announce: impl Fn(&str, Option<String>),
    ) -> Result<(), String> {
        check_key(key)?;
        let mut held = self.held.lock().unwrap();
        if held.get(key) != Some(&peer) {
            return Err(format!("this connection does not hold {key}"));
        }
        held.remove(key);
        announce(key, None);
        Ok(())
    }

    /// `peer` disconnected: drop and announce every key it held.
    pub(crate) fn gone(&self, peer: SocketAddr, announce: impl Fn(&str, Option<String>)) {
        let mut held = self.held.lock().unwrap();
        held.retain(|key, p| {
            let keep = *p != peer;
            if !keep {
                announce(key, None);
            }
            keep
        });
    }

    /// Run `f` while `peer` holds `key`, with the table locked so the holder cannot change meanwhile;
    /// refused when it does not hold it.
    pub(crate) fn while_held(&self, key: &str, peer: SocketAddr, f: impl FnOnce()) -> Result<(), String> {
        let held = self.held.lock().unwrap();
        if held.get(key) != Some(&peer) {
            return Err(format!("this connection does not hold the {key} lease"));
        }
        f();
        Ok(())
    }

    /// Every held key and its holder's id, for `status`.
    pub(crate) fn holders(&self) -> BTreeMap<String, String> {
        let held = self.held.lock().unwrap();
        held.iter()
            .map(|(k, p)| (k.clone(), p.to_string()))
            .collect()
    }
}

impl crate::server::AppState {
    /// Give `key` to `peer` and tell every connection.
    pub(crate) fn lease_claim(&self, key: &str, peer: SocketAddr) -> Result<String, String> {
        self.leases
            .claim(key, peer, |k, o| self.lease_changed(k, o))
    }

    /// `peer` gives `key` up, and every connection is told.
    pub(crate) fn lease_release(&self, key: &str, peer: SocketAddr) -> Result<(), String> {
        self.leases
            .release(key, peer, |k, o| self.lease_changed(k, o))
    }

    /// `peer` disconnected: every key it held is released and announced.
    pub(crate) fn leases_gone(&self, peer: SocketAddr) {
        self.leases.gone(peer, |k, o| self.lease_changed(k, o));
    }

    /// Name the agent voice mode talks to, or clear it; only the `voice` holder may (ADR-0055).
    pub(crate) fn agent_voice(&self, agent: Option<&str>, peer: SocketAddr) -> Result<(), String> {
        let agent = agent.map(|a| self.resolve_agent(a).map(|r| r.agent)).transpose()?;
        if let Some(agent) = &agent {
            if !matches!(agent.harness(), crate::resume::Harness::Claude | crate::resume::Harness::Pi) {
                return Err(format!("{} has no way to take the voice line", agent.harness().title()));
            }
        }
        // Checked under the table's lock, so a takeover cannot land between the check and the mark.
        self.leases.while_held("voice", peer, || {
            self.set_voiced(agent.map(|a| (peer, a)));
        })
    }

    /// Move the voice line to `next`, turning it off on the agent that had it.
    fn set_voiced(&self, next: Option<(SocketAddr, std::sync::Arc<crate::agent::Agent>)>) {
        let mut voiced = self.voiced.lock().unwrap();
        voice_off(voiced.take());
        if let Some((peer, agent)) = &next {
            agent.set_voice(true);
            tracing::info!(agent = %agent.name, %peer, "voice line on");
        }
        *voiced = next;
    }

    /// Turn the voice line off when the voiced agent and its connection match `pred`.
    fn clear_voiced_if(&self, pred: impl FnOnce(&SocketAddr, &std::sync::Arc<crate::agent::Agent>) -> bool) {
        let mut voiced = self.voiced.lock().unwrap();
        if voiced.as_ref().is_some_and(|(p, a)| pred(p, a)) {
            voice_off(voiced.take());
        }
    }

    /// `agent` ended: voice mode talks to nobody when it was the one.
    pub(crate) fn voice_ended(&self, agent: &std::sync::Arc<crate::agent::Agent>) {
        self.clear_voiced_if(|_, a| std::sync::Arc::ptr_eq(a, agent));
    }

    fn lease_changed(&self, key: &str, owner: Option<String>) {
        if key == "voice" {
            self.clear_voiced_if(|peer, _| owner.as_deref() != Some(peer.to_string().as_str()));
        }
        let _ = self.global.send(wheel_proto::ServerMsg::Lease {
            key: key.to_owned(),
            owner,
        });
    }
}

fn voice_off(was: Option<(SocketAddr, std::sync::Arc<crate::agent::Agent>)>) {
    if let Some((peer, agent)) = was {
        agent.set_voice(false);
        tracing::info!(agent = %agent.name, %peer, "voice line off");
    }
}

#[cfg(test)]
mod tests {
    use super::Leases;
    use std::sync::Mutex;

    #[test]
    fn a_claim_takes_over_and_a_close_drops_only_its_own_keys() {
        let (a, b) = (
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
        );
        let heard = Mutex::new(Vec::new());
        let say = |k: &str, o: Option<String>| heard.lock().unwrap().push((k.to_owned(), o));
        let l = Leases::default();
        assert_eq!(l.claim("voice", a, say).unwrap(), "127.0.0.1:1");
        l.claim("voice", b, say).unwrap();
        assert!(l.release("voice", a, say).is_err());
        l.claim("orchestrator", a, say).unwrap();
        assert_eq!(
            l.holders().get("orchestrator").map(String::as_str),
            Some("127.0.0.1:1")
        );
        l.gone(a, say);
        assert!(l.release("voice", b, say).is_ok());
        l.gone(b, say);
        assert!(l.holders().is_empty());
        assert!(l.claim("Voice!", a, say).is_err() && l.claim("", a, say).is_err());
        let heard = heard.into_inner().unwrap();
        let owners: Vec<_> = heard
            .iter()
            .map(|(k, o)| format!("{k}={}", o.as_deref().unwrap_or("-")))
            .collect();
        assert_eq!(
            owners,
            [
                "voice=127.0.0.1:1",
                "voice=127.0.0.1:2",
                "orchestrator=127.0.0.1:1",
                "orchestrator=-",
                "voice=-"
            ]
        );
    }
}
