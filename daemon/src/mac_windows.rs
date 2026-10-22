//! `window`: commands for the Mac app's windows, which only it knows (ADR-0014).
//!
//! wheeld stores no windows. A Mac app that owns them sends `window_offer` after connecting; a
//! `window` command is forwarded to the newest such connection as `window_request`, and its
//! `window_answer` is the reply. With no Mac app attached the command fails and says so.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use wheel_proto::{ServerMsg, WindowCommand};

use crate::server::AppState;

/// How long the Mac app has to answer.
const WAIT: Duration = Duration::from_secs(10);

type Answer = Result<serde_json::Value, String>;

/// The connections that own windows, and the commands waiting on one.
#[derive(Default)]
pub(crate) struct MacApps {
    next: AtomicU64,
    /// In connection order; the last is the newest.
    offered: Mutex<Vec<(SocketAddr, mpsc::Sender<ServerMsg>)>>,
    pending: Mutex<HashMap<u64, (SocketAddr, oneshot::Sender<Answer>)>>,
}

impl MacApps {
    pub(crate) fn offer(&self, peer: SocketAddr, out: mpsc::Sender<ServerMsg>) {
        let mut offered = self.offered.lock().unwrap();
        offered.retain(|(p, _)| *p != peer);
        offered.push((peer, out));
        tracing::info!(%peer, offered = offered.len(), "Mac app attached; it owns the windows");
    }

    /// Whether the connection from `peer` is a Mac app.
    pub(crate) fn is_app(&self, peer: SocketAddr) -> bool {
        self.offered.lock().unwrap().iter().any(|(p, _)| *p == peer)
    }

    /// A connection closed: it owns no windows, and what waited on it fails.
    pub(crate) fn gone(&self, peer: SocketAddr) {
        let had = {
            let mut offered = self.offered.lock().unwrap();
            let before = offered.len();
            offered.retain(|(p, _)| *p != peer);
            before != offered.len()
        };
        if had {
            tracing::info!(%peer, "Mac app detached");
        }
        let mut pending = self.pending.lock().unwrap();
        let ids: Vec<u64> = pending
            .iter()
            .filter(|(_, (p, _))| *p == peer)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((_, tx)) = pending.remove(&id) {
                let _ = tx.send(Err("the Mac app disconnected before it answered".into()));
            }
        }
    }

    pub(crate) fn answer(&self, peer: SocketAddr, id: u64, answer: Answer) {
        let waiting = {
            let mut pending = self.pending.lock().unwrap();
            match pending.get(&id) {
                Some((p, _)) if *p == peer => pending.remove(&id),
                _ => None,
            }
        };
        match waiting {
            Some((_, tx)) => {
                let _ = tx.send(answer);
            }
            None => tracing::warn!(%peer, id, "window answer for a command nobody waits on"),
        }
    }
}

impl AppState {
    /// Run `command` on the newest attached Mac app and return its answer.
    pub async fn window_command(
        &self,
        command: WindowCommand,
    ) -> Result<serde_json::Value, String> {
        let target = self.mac_apps.offered.lock().unwrap().last().cloned();
        let Some((peer, out)) = target else {
            return Err("no Mac app attached".into());
        };
        let id = self.mac_apps.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.mac_apps.pending.lock().unwrap().insert(id, (peer, tx));
        tracing::info!(%peer, id, ?command, "window command forwarded");
        let sent = out
            .send(ServerMsg::WindowRequest { id, command })
            .await
            .is_ok();
        let answer = if !sent {
            Err("no Mac app attached".to_string())
        } else {
            match tokio::time::timeout(WAIT, rx).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(_)) => Err("the Mac app dropped the command".to_string()),
                Err(_) => Err(format!(
                    "the Mac app did not answer within {}s",
                    WAIT.as_secs()
                )),
            }
        };
        self.mac_apps.pending.lock().unwrap().remove(&id);
        match &answer {
            Ok(_) => tracing::info!(%peer, id, "window command answered"),
            Err(why) => tracing::warn!(%peer, id, %why, "window command failed"),
        }
        answer
    }
}
