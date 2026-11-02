//! What each attached client shows (ADR-0022).
//!
//! wheeld keeps no active space and no focused pane. A client shows what it likes, per window,
//! and reports each change with `view`; the daemon holds the latest report per connection and
//! window for `client_ls`, drops them when the connection closes, and never sends one client's
//! view to another.

use std::net::SocketAddr;
use std::sync::Mutex;

use wheel_proto::{ClientInfo, WindowView};

use crate::server::{AppState, Caller};

/// The views connections reported, in connection order.
#[derive(Default)]
pub(crate) struct Views {
    by_connection: Mutex<Vec<(SocketAddr, String, Vec<WindowView>)>>,
}

impl Views {
    /// Record `view` for its window on the connection from `peer`, named `client`. A view that
    /// shows nothing drops the window.
    pub(crate) fn report(&self, peer: SocketAddr, client: String, view: WindowView) {
        let mut all = self.by_connection.lock().unwrap();
        let at = match all.iter().position(|(p, _, _)| *p == peer) {
            Some(at) => at,
            None => {
                all.push((peer, client.clone(), Vec::new()));
                all.len() - 1
            }
        };
        all[at].1 = client;
        let windows = &mut all[at].2;
        windows.retain(|w| w.window != view.window);
        if view.session.is_some() || view.space.is_some() || view.pane.is_some() {
            windows.push(view);
        }
        if windows.is_empty() {
            all.remove(at);
        }
    }

    /// The connection closed: its windows are gone.
    pub(crate) fn gone(&self, peer: SocketAddr) {
        self.by_connection
            .lock()
            .unwrap()
            .retain(|(p, _, _)| *p != peer);
    }

    pub(crate) fn list(&self) -> Vec<ClientInfo> {
        self.by_connection
            .lock()
            .unwrap()
            .iter()
            .map(|(peer, client, windows)| {
                let mut windows = windows.clone();
                windows.sort_by(|a, b| a.window.cmp(&b.window));
                ClientInfo {
                    client: client.clone(),
                    connection: peer.to_string(),
                    windows,
                }
            })
            .collect()
    }
}

impl AppState {
    /// What a connection is called in `client_ls`: `mac` for the Mac app, the device's name for a
    /// paired device, else `local`.
    pub(crate) fn client_name(&self, peer: SocketAddr, caller: &Caller) -> String {
        match caller {
            Caller::Device(id) => self.devices.name(id).unwrap_or_else(|| id.clone()),
            Caller::Local if self.mac_apps.is_app(peer) => "mac".to_string(),
            Caller::Local => "local".to_string(),
        }
    }

    /// The id of the session that holds space `space`.
    pub(crate) fn session_id_holding(&self, space: &str) -> Option<String> {
        let spaces = self.spaces.lock().unwrap();
        let at = spaces.session_holding(space)?;
        Some(spaces.sessions[at].id.clone())
    }

    /// Check a reported view against the layout: a window cannot show a space or pane that does
    /// not exist, or a pane that is not in the space it reports.
    pub(crate) fn check_view(&self, view: &WindowView) -> Result<(), String> {
        let spaces = self.spaces.lock().unwrap();
        if let Some(session) = &view.session {
            spaces.session_index(session)?;
        }
        let held = match &view.space {
            Some(space) => Some(spaces.index(space)?),
            None => None,
        };
        if let Some(pane) = &view.pane {
            let at = spaces.holding(pane)?;
            if held.is_some_and(|h| h != at) {
                return Err(format!("pane {pane} is not in space {}", spaces.spaces[at].id));
            }
        }
        Ok(())
    }
}
