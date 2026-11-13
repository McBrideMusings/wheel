//! The pairing window's own state (PairingWindow.swift's PairingState): the code itself is the
//! Devices model's. Pure; `view/toasts.rs` draws the window.
use crate::settings::Devices;

/// How long a copied pairing code stays on the clipboard if nothing else is copied.
pub const CLIPBOARD_LIFE_MS: u64 = 60_000;
/// A code stops working after five minutes; the open window asks for a new one every four.
pub const REFRESH_MS: u64 = 240_000;
/// How long the window stays after a device connects, long enough to read who it was.
pub const LINGER_MS: u64 = 1_500;

/// What the menu bar's pairing row asks a window to do once wheeld says how phones reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Copy,
    Show,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub open: bool,
    /// "Close when a device connects": on by default.
    pub close_on_connect: bool,
    pub details_open: bool,
    /// The device that connected while the window was open, once one has.
    pub connected: Option<String>,
    /// The latest events, newest last, at most five.
    pub events: Vec<String>,
    /// The codes the open window has shown; they stop working when it closes.
    pub window_tokens: Vec<String>,
    /// The code on the clipboard, until its minute runs out or another copy replaces it.
    pub copied: Option<(String, u64)>,
    /// A copy asked for a code that has not arrived yet.
    pub copy_pending: bool,
    /// The menu bar's row asked for a copy or the window; it runs when wheeld answers how phones reach it.
    pub asked: Option<Ask>,
    /// When the open window next asks for a code (unix ms).
    pub refresh_at: Option<u64>,
    /// When the window closes itself after a device connected (unix ms).
    pub close_at: Option<u64>,
}

impl Default for Pairing {
    fn default() -> Self {
        Pairing { open: false, close_on_connect: true, details_open: false, connected: None, events: vec![], window_tokens: vec![], copied: None, copy_pending: false, asked: None, refresh_at: None, close_at: None }
    }
}

impl Pairing {
    /// Why the pairing code cannot be copied or shown now; None when it can.
    pub fn disabled_reason(link_up: bool, devices: &Devices) -> Option<&'static str> {
        if !link_up {
            return Some("The server is stopped.");
        }
        let Some(info) = &devices.info else { return Some("Asking wheeld how phones reach it…") };
        if info.problem.is_some() {
            return Some("The remote listener failed. Open Wheel Server for the reason.");
        }
        if info.listen.is_none() {
            return Some("Remote access is off. Turn it on in Settings, Devices.");
        }
        None
    }

    /// The address a phone dials, or why there is none (MenuBarView.swift pairingAddressText).
    pub fn address_text(devices: &Devices) -> String {
        let Some(info) = &devices.info else { return "Asking wheeld…".into() };
        if info.listen.is_none() {
            return "Remote access is off".into();
        }
        if let Some(problem) = &info.problem {
            return format!("Listener failed: {problem}");
        }
        match devices.address() {
            Some(address) => format!("wss://{address}:{}", info.port),
            None => "No address".into(),
        }
    }

    /// The pairing row's second line: why pairing is off, else the address (pairingRowDetail).
    pub fn row_detail(link_up: bool, devices: &Devices) -> String {
        Self::disabled_reason(link_up, devices).map(str::to_owned).unwrap_or_else(|| Self::address_text(devices))
    }

    /// Open the window, keeping the switch and the details as they were. A window already open
    /// starts over: the codes it showed are returned to be cancelled.
    pub fn open(&mut self, now_ms: u64) -> Vec<String> {
        let old = std::mem::take(&mut self.window_tokens);
        *self = Pairing { open: true, close_on_connect: self.close_on_connect, details_open: self.details_open, copied: self.copied.take(), ..Pairing::default() };
        self.refresh_at = Some(now_ms + REFRESH_MS);
        self.event(now_ms, "opened the pairing window");
        old
    }

    /// Close the window: the codes it showed are to be cancelled, returned here.
    pub fn close(&mut self) -> Vec<String> {
        self.open = false;
        self.copy_pending = false;
        self.refresh_at = None;
        self.close_at = None;
        std::mem::take(&mut self.window_tokens)
    }

    pub fn event(&mut self, now_ms: u64, text: &str) {
        let secs = (now_ms / 1000) % 86_400;
        self.events.push(format!("{:02}:{:02}:{:02} {text}", secs / 3600, (secs / 60) % 60, secs % 60));
        if self.events.len() > 5 {
            self.events.drain(..self.events.len() - 5);
        }
    }

    /// A code reached the app: the open window keeps it. Returns whether a copy was waiting for
    /// it, so the caller writes it to the clipboard.
    pub fn code_arrived(&mut self, token: &str) -> bool {
        if self.open && !self.window_tokens.iter().any(|t| t == token) {
            self.window_tokens.push(token.to_owned());
        }
        std::mem::take(&mut self.copy_pending)
    }

    /// wheeld says a remote client connected. A phone that was paired already and reconnected
    /// is not the one the window waits for.
    pub fn device_connected(&mut self, now_ms: u64, name: &str, address: &str, paired: bool) {
        self.event(now_ms, &format!("client “{name}” connected from {address}{}", if paired { " and paired" } else { "" }));
        if !(self.open && paired) {
            return;
        }
        self.connected = Some(name.to_owned());
        if self.close_on_connect {
            self.close_at = Some(now_ms + LINGER_MS);
        }
    }

    /// The copied code's minute ran out: the token to cancel, unless the open window still shows it.
    pub fn copy_expired(&mut self, now_ms: u64) -> Option<String> {
        let (token, until) = self.copied.clone()?;
        if now_ms < until {
            return None;
        }
        self.copied = None;
        (!(self.open && self.window_tokens.contains(&token))).then_some(token)
    }

    /// The soonest moment something timed happens, for a redraw timer.
    pub fn next_tick(&self) -> Option<u64> {
        [self.refresh_at, self.close_at, self.copied.as_ref().map(|(_, until)| *until)].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Remote;

    fn devices(listen: Option<&str>, problem: Option<&str>) -> Devices {
        Devices { info: Some(Remote { listen: listen.map(str::to_owned), addresses: vec!["10.0.0.2".into()], port: 7430, fingerprint: "ab".repeat(32), problem: problem.map(str::to_owned) }), ..Devices::default() }
    }

    #[test]
    fn says_why_pairing_is_off() {
        assert_eq!(Pairing::disabled_reason(false, &devices(Some("0.0.0.0:7430"), None)), Some("The server is stopped."));
        assert_eq!(Pairing::disabled_reason(true, &Devices::default()), Some("Asking wheeld how phones reach it…"));
        assert_eq!(Pairing::disabled_reason(true, &devices(Some("x"), Some("bind failed"))), Some("The remote listener failed. Open Wheel Server for the reason."));
        assert_eq!(Pairing::disabled_reason(true, &devices(None, None)), Some("Remote access is off. Turn it on in Settings, Devices."));
        assert_eq!(Pairing::disabled_reason(true, &devices(Some("0.0.0.0:7430"), None)), None);
    }

    #[test]
    fn the_row_names_the_address_or_why_not() {
        assert_eq!(Pairing::row_detail(true, &devices(Some("0.0.0.0:7430"), None)), "wss://10.0.0.2:7430");
        assert_eq!(Pairing::row_detail(false, &devices(Some("0.0.0.0:7430"), None)), "The server is stopped.");
        assert_eq!(Pairing::row_detail(true, &devices(None, None)), "Remote access is off. Turn it on in Settings, Devices.");
        assert_eq!(Pairing::address_text(&devices(None, None)), "Remote access is off");
        assert_eq!(Pairing::address_text(&devices(Some("x"), Some("bind failed"))), "Listener failed: bind failed");
        assert_eq!(Pairing::address_text(&Devices::default()), "Asking wheeld…");
    }

    #[test]
    fn window_keeps_its_codes_and_closes_on_a_paired_connection() {
        let mut p = Pairing::default();
        assert!(p.open(1_000).is_empty());
        assert_eq!(p.refresh_at, Some(1_000 + REFRESH_MS));
        assert_eq!(p.events.len(), 1);
        assert!(!p.code_arrived("t1"));
        p.copy_pending = true;
        assert!(p.code_arrived("t2"));
        assert_eq!(p.window_tokens, ["t1", "t2"]);
        p.device_connected(2_000, "Old phone", "10.0.0.9", false);
        assert_eq!(p.connected, None, "a reconnecting phone is not the one");
        p.device_connected(3_000, "Pierce’s iPhone", "10.0.0.9", true);
        assert_eq!(p.connected.as_deref(), Some("Pierce’s iPhone"));
        assert_eq!(p.close_at, Some(3_000 + LINGER_MS));
        assert_eq!(p.open(3_100), ["t1", "t2"], "a window opened again lets its earlier codes go");
        assert!(p.close().is_empty());
        assert!(!p.open && p.refresh_at.is_none());
    }

    #[test]
    fn keeps_five_events_and_expires_a_copy() {
        let mut p = Pairing::default();
        for i in 0..7 {
            p.event(i * 1000, &format!("e{i}"));
        }
        assert_eq!(p.events.len(), 5);
        assert!(p.events[0].ends_with("e2"));
        assert!(p.events[0].starts_with("00:00:02 "));
        p.copied = Some(("t".into(), 10_000));
        assert_eq!(p.copy_expired(9_999), None);
        assert_eq!(p.copy_expired(10_000), Some("t".into()));
        p.open(0);
        p.window_tokens.push("u".into());
        p.copied = Some(("u".into(), 10));
        assert_eq!(p.copy_expired(20), None, "the open window keeps its code");
        assert_eq!(p.next_tick(), Some(REFRESH_MS));
    }
}
