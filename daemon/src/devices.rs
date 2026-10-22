//! Paired devices and the one-time tokens that pair them (ADR-0015).
//!
//! A pairing token works once and for five minutes. A phone presents it on the remote listener and
//! is given a credential of its own; the daemon keeps only the credential's SHA-256, in
//! `devices.json` in the instance directory, so a copy of that file opens nothing. Revoking a
//! device deletes its row, which refuses its credential from then on.

use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wheel_proto::DeviceInfo;

/// How long a pairing token works.
pub const TOKEN_LIFE: Duration = Duration::from_secs(300);
/// Tokens waiting to be used at once; a further request drops the oldest.
const PENDING_LIMIT: usize = 16;
const FILE: &str = "devices.json";

#[derive(Serialize, Deserialize, Clone)]
struct Device {
    id: String,
    name: String,
    /// SHA-256 of the credential, lowercase hex.
    hash: String,
    paired_ms: u64,
    last_seen_ms: u64,
}

struct Pending {
    token: String,
    until: Instant,
}

#[derive(Default)]
struct Inner {
    devices: Vec<Device>,
    pending: Vec<Pending>,
    /// Open connections by device id.
    open: std::collections::HashMap<String, u32>,
    next: u64,
}

/// The paired devices and the pairing tokens not yet used.
pub struct Devices {
    path: PathBuf,
    inner: Mutex<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn random_hex() -> String {
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut raw))
        .expect("the system's random source is readable");
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Equal strings, comparing every byte whatever the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

impl Devices {
    /// The devices saved in `dir`; a file that cannot be read pairs nobody and is logged.
    pub fn load(dir: &std::path::Path) -> Devices {
        let path = dir.join(FILE);
        let devices: Vec<Device> = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "devices file unreadable; no device is paired");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let next = devices
            .iter()
            .filter_map(|d| d.id.strip_prefix('d')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        tracing::info!(count = devices.len(), "paired devices loaded");
        Devices {
            path,
            inner: Mutex::new(Inner {
                devices,
                next,
                ..Default::default()
            }),
        }
    }

    fn save(&self, inner: &Inner) -> bool {
        let part = self.path.with_extension("json.part");
        let text = serde_json::to_string_pretty(&inner.devices).expect("devices serialize");
        let written = (|| {
            use std::os::unix::fs::OpenOptionsExt as _;
            let _ = std::fs::remove_file(&part);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&part)?;
            std::io::Write::write_all(&mut file, text.as_bytes())?;
            std::fs::rename(&part, &self.path)
        })();
        if let Err(e) = &written {
            tracing::error!(path = %self.path.display(), error = %e, "cannot save the paired devices");
        }
        written.is_ok()
    }

    /// A new token and when it stops working (unix ms).
    pub fn issue(&self) -> (String, u64) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        inner.pending.retain(|p| p.until > now);
        if inner.pending.len() >= PENDING_LIMIT {
            inner.pending.remove(0);
        }
        let token = random_hex();
        inner.pending.push(Pending {
            token: token.clone(),
            until: now + TOKEN_LIFE,
        });
        (token, now_ms() + TOKEN_LIFE.as_millis() as u64)
    }

    /// Spend `token`: the new device's id and its credential, or why the token is no good. A token
    /// is gone after one try that names it, right or wrong name aside.
    pub fn redeem(&self, token: &str, name: &str) -> Result<(String, String), &'static str> {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        let found = inner
            .pending
            .iter()
            .position(|p| same(&p.token, token))
            .map(|at| inner.pending.remove(at));
        let Some(found) = found else {
            return Err("that pairing token was not issued, or was already used");
        };
        if found.until <= now {
            return Err("that pairing token has expired");
        }
        let credential = random_hex();
        let id = format!("d{}", inner.next);
        inner.next += 1;
        let name = match name.trim() {
            "" => "Device",
            name => name,
        };
        let at = now_ms();
        inner.devices.push(Device {
            id: id.clone(),
            name: name.chars().take(80).collect(),
            hash: hash(&credential),
            paired_ms: at,
            last_seen_ms: at,
        });
        if !self.save(&inner) {
            // A device the daemon would forget at its next start is not paired.
            inner.devices.retain(|d| d.id != id);
            return Err(
                "the daemon could not save the paired device; check its instance directory",
            );
        }
        Ok((id, credential))
    }

    /// The device `credential` belongs to.
    pub fn authenticate(&self, credential: &str) -> Option<String> {
        let wanted = hash(credential);
        let inner = self.inner.lock().unwrap();
        inner
            .devices
            .iter()
            .find(|d| same(&d.hash, &wanted))
            .map(|d| d.id.clone())
    }

    /// The device's name, if it is still paired.
    pub fn name(&self, id: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner
            .devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
    }

    /// A connection of `id` opened.
    pub fn opened(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        *inner.open.entry(id.to_string()).or_default() += 1;
        self.touch(&mut inner, id);
    }

    /// A connection of `id` closed.
    pub fn closed(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(n) = inner.open.get_mut(id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.open.remove(id);
            }
        }
        self.touch(&mut inner, id);
    }

    fn touch(&self, inner: &mut Inner, id: &str) {
        if let Some(device) = inner.devices.iter_mut().find(|d| d.id == id) {
            device.last_seen_ms = now_ms();
            self.save(inner);
        }
    }

    /// Forget `id`; false when no such device is paired.
    pub fn revoke(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.devices.len();
        inner.devices.retain(|d| d.id != id);
        inner.open.remove(id);
        let removed = inner.devices.len() != before;
        if removed {
            self.save(&inner);
        }
        removed
    }

    /// Every paired device, newest pairing last.
    pub fn list(&self) -> Vec<DeviceInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .devices
            .iter()
            .map(|d| {
                let connected = inner.open.get(&d.id).copied().unwrap_or(0);
                DeviceInfo {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    paired_ms: d.paired_ms,
                    last_seen_ms: if connected > 0 {
                        now_ms()
                    } else {
                        d.last_seen_ms
                    },
                    connected,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheeld-devices-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_token_pairs_one_device_once() {
        let dir = scratch("once");
        let devices = Devices::load(&dir);
        let (token, _) = devices.issue();
        let (id, credential) = devices.redeem(&token, "Ada's iPhone").unwrap();
        assert_eq!(devices.authenticate(&credential), Some(id.clone()));
        assert!(devices.redeem(&token, "Another phone").is_err());
        assert_eq!(devices.list().len(), 1);
        assert_eq!(devices.name(&id).as_deref(), Some("Ada's iPhone"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_expired_token_is_refused_and_spent() {
        let dir = scratch("expired");
        let devices = Devices::load(&dir);
        let (token, _) = devices.issue();
        devices.inner.lock().unwrap().pending[0].until = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            devices.redeem(&token, "late"),
            Err("that pairing token has expired")
        );
        assert!(devices.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_credential_survives_a_restart_and_a_revoke_ends_only_that_device() {
        let dir = scratch("revoke");
        let devices = Devices::load(&dir);
        let (a, b) = (devices.issue().0, devices.issue().0);
        let (first, first_credential) = devices.redeem(&a, "one").unwrap();
        let (_, second_credential) = devices.redeem(&b, "two").unwrap();
        let again = Devices::load(&dir);
        assert_eq!(again.authenticate(&first_credential), Some(first.clone()));
        assert!(again.revoke(&first));
        assert_eq!(again.authenticate(&first_credential), None);
        assert!(again.authenticate(&second_credential).is_some());
        assert!(!again.revoke(&first));
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(
            !text.contains(&second_credential),
            "the file holds the credential itself"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
