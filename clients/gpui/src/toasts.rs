//! The notices over the window (Toasts.swift): one shape for every notice, a stack of at most
//! three at the bottom right, and the order they come in. The model is pure; `view/toasts.rs`
//! draws it.
use wheel_proto::NoticeLevel;

/// How a notice reads: green for news, orange for a warning, red for an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Notice,
    Warning,
    Error,
}

impl Level {
    pub fn of(level: &NoticeLevel) -> Self {
        match level {
            NoticeLevel::Notice => Level::Notice,
            NoticeLevel::Warning => Level::Warning,
            NoticeLevel::Error => Level::Error,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Level::Notice => "notice",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }
    /// The badge's colour (SwiftUI's green, orange and red).
    pub fn colour(self) -> u32 {
        match self {
            Level::Notice => 0x30d158,
            Level::Warning => 0xff9f0a,
            Level::Error => 0xff453a,
        }
    }
    /// The badge's glyph, standing in for the SF Symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            Level::Notice => "✓",
            Level::Warning => "!",
            Level::Error => "×",
        }
    }
}

/// What a button on a notice does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Start the installed app and quit this one.
    Relaunch,
    /// Stop wheeld and start it again from what is installed (`crate::server::Control`).
    RestartServer,
    /// Keep the earlier server version from running after a wheeld that never answered again.
    CancelRollback,
    FocusPane(String),
    ClosePane(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: &'static str,
    pub title: &'static str,
    pub act: Act,
}

/// What closing a notice does besides hiding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dismiss {
    /// Drop it from the posted list.
    Posted,
    /// Remember the installed stamp, so the same install is not offered again.
    ClientUpdate(u64),
    /// Remember what the notice was about (`crate::server::banner_key`), so the same server is not offered again.
    ServerUpdate(String),
    /// Forget which version the server was rolled back to.
    RolledBack,
    /// Take the agent's notice down on every client (`agent_notice_dismiss`), then drop it.
    AgentNotice(String),
    /// Forget the last layout error.
    LayoutError,
}

/// The one shape of every notice: a message, buttons for what can be done about it, a close
/// button unless it clears itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: String,
    pub level: Level,
    pub text: String,
    pub actions: Vec<Action>,
    /// The pane the notice is about; it gets Focus pane and Close pane buttons while it exists.
    pub pane: Option<String>,
    pub dismiss: Dismiss,
    pub closable: bool,
    /// A bar that empties from `from` to `deadline` (unix ms): the rollback's countdown.
    pub countdown: Option<(u64, u64)>,
}

impl Toast {
    pub fn posted(id: String, level: Level, text: String, pane: Option<String>) -> Self {
        Toast { pane, ..Toast::notice(&id, level, text, Dismiss::Posted, true) }
    }
    /// A notice about nothing in particular: no pane, no buttons, no countdown.
    fn notice(id: &str, level: Level, text: impl Into<String>, dismiss: Dismiss, closable: bool) -> Self {
        Toast { id: id.into(), level, text: text.into(), actions: vec![], pane: None, dismiss, closable, countdown: None }
    }
    /// A short message with at most one button sits on one line, as a pill.
    pub fn compact(&self) -> bool {
        self.text.chars().count() <= 60 && self.actions.len() <= 1
    }
}

/// The most notices on screen at once; the newest wait behind them.
pub const LIMIT: usize = 3;

/// What the stack is built from, beyond what was posted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// The executable on disk is newer than the one running: its stamp (unix seconds).
    pub client_installed: Option<u64>,
    /// The stamp whose notice was closed.
    pub client_update_dismissed: Option<u64>,
    /// A restart waits (an older protocol, or a running source that is not the bundled one):
    /// what it is about (`crate::server::banner_key`).
    pub server_updated: Option<String>,
    /// The key whose notice was closed.
    pub server_update_dismissed: Option<String>,
    /// The client has commands to restart wheeld with: the notice gets Restart Server.
    pub server_restart_available: bool,
    /// The earlier version that runs when the countdown ends, unless cancelled.
    pub rollback_offer: Option<crate::server::Offer>,
    /// The version wheeld was rolled back to, while connected, until closed.
    pub rolled_back_to: Option<String>,
    /// wheeld is unreachable for this reason and no restart is expected.
    pub unreachable: Option<String>,
    /// wheeld's last refusal of a layout op this window sent.
    pub layout_error: Option<String>,
}

/// Every notice now, in the order they stack from the top: the client update, the server update,
/// a rollback counting down or a lost connection, a rollback done, the daemon's last refusal,
/// then what was posted (`notify` toasts and agents' notices). A notice about a pane that exists
/// gets Focus pane and Close pane after its own buttons.
pub fn stack(sources: &Sources, posted: &[Toast], pane_exists: impl Fn(&str) -> bool) -> Vec<Toast> {
    let mut list = vec![];
    if let Some(installed) = sources.client_installed
        && Some(installed) != sources.client_update_dismissed
    {
        list.push(Toast {
            actions: vec![Action { id: "relaunch", title: "Relaunch Wheel", act: Act::Relaunch }],
            ..Toast::notice("client-update", Level::Notice, "A newer Wheel is installed", Dismiss::ClientUpdate(installed), true)
        });
    }
    if let Some(key) = &sources.server_updated
        && Some(key) != sources.server_update_dismissed.as_ref()
    {
        list.push(Toast {
            actions: if sources.server_restart_available { vec![Action { id: "restart", title: "Restart Server", act: Act::RestartServer }] } else { vec![] },
            ..Toast::notice("server-update", Level::Notice, crate::server::UPDATED, Dismiss::ServerUpdate(key.clone()), true)
        });
    }
    if let Some(offer) = &sources.rollback_offer {
        list.push(Toast {
            actions: vec![Action { id: "cancel", title: "Cancel", act: Act::CancelRollback }],
            countdown: Some((offer.from, offer.deadline)),
            ..Toast::notice("rollback", Level::Warning, "Rolling back", Dismiss::Posted, false)
        });
    } else if let Some(reason) = &sources.unreachable {
        list.push(Toast::notice("unreachable", Level::Warning, format!("wheeld unreachable: {reason}. Retrying every 2 seconds."), Dismiss::Posted, false));
    }
    if let Some(target) = &sources.rolled_back_to {
        list.push(Toast::notice("rolled-back", Level::Notice, format!("Rolled back to {target}"), Dismiss::RolledBack, true));
    }
    if let Some(error) = &sources.layout_error {
        list.push(Toast { pane: pane_named(error), ..Toast::notice("layout-error", Level::Error, error.clone(), Dismiss::LayoutError, true) });
    }
    list.extend(posted.iter().cloned());
    for toast in &mut list {
        if let Some(pane) = toast.pane.clone().filter(|p| pane_exists(p)) {
            toast.actions.push(Action { id: "focus", title: "Focus pane", act: Act::FocusPane(pane.clone()) });
            toast.actions.push(Action { id: "close", title: "Close pane", act: Act::ClosePane(pane) });
        }
    }
    list
}

/// The pane id a daemon message names, as in "agent w4653 in pane p4652 could not start".
pub fn pane_named(message: &str) -> Option<String> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut rest = message;
    while let Some(at) = rest.find("pane p") {
        let before_ok = !rest[..at].chars().next_back().is_some_and(is_word);
        let digits: String = rest[at + "pane p".len()..].chars().take_while(char::is_ascii_digit).collect();
        let after = rest[at + "pane p".len() + digits.len()..].chars().next();
        if before_ok && !digits.is_empty() && !after.is_some_and(is_word) {
            return Some(format!("p{digits}"));
        }
        rest = &rest[at + 1..];
    }
    None
}

/// Post `toast`, replacing one with the same id.
pub fn post(posted: &mut Vec<Toast>, toast: Toast) {
    posted.retain(|t| t.id != toast.id);
    posted.push(toast);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_pane_a_message_is_about() {
        assert_eq!(pane_named("agent w4653 in pane p4652 could not start").as_deref(), Some("p4652"));
        assert_eq!(pane_named("no such pane p9"), Some("p9".into()));
        assert_eq!(pane_named("the pane pX is odd"), None);
        assert_eq!(pane_named("airplane p12 lands"), None);
        assert_eq!(pane_named("pane p12b"), None);
    }

    #[test]
    fn stacks_in_order_and_adds_pane_buttons() {
        let posted = vec![Toast::posted("notify:a".into(), Level::Notice, "hi".into(), Some("p1".into()))];
        let sources = Sources { client_installed: Some(5), server_updated: Some("protocol 32".into()), server_restart_available: true, layout_error: Some("no such pane p7".into()), ..Sources::default() };
        let list = stack(&sources, &posted, |p| p == "p1");
        assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["client-update", "server-update", "layout-error", "notify:a"]);
        assert_eq!((list[1].text.as_str(), list[1].actions[0].title), ("The server was updated.", "Restart Server"));
        assert_eq!(list[2].pane.as_deref(), Some("p7"));
        assert!(list[2].actions.is_empty(), "a pane that is gone gets no buttons");
        assert_eq!(list[3].actions.iter().map(|a| a.title).collect::<Vec<_>>(), ["Focus pane", "Close pane"]);
        assert!(!list[3].compact(), "two buttons make a card");
        let dismissed = Sources { client_update_dismissed: Some(5), server_update_dismissed: Some("protocol 32".into()), ..sources };
        assert_eq!(stack(&dismissed, &[], |_| false).len(), 1);
    }

    #[test]
    fn a_server_update_without_commands_has_no_button() {
        let sources = Sources { server_updated: Some("abc".into()), ..Sources::default() };
        let list = stack(&sources, &[], |_| false);
        assert_eq!(list.len(), 1);
        assert!(list[0].actions.is_empty());
        assert!(list[0].compact(), "the bare notice is a pill");
    }

    #[test]
    fn a_rollback_counts_down_in_place_of_the_lost_connection() {
        let offer = crate::server::Offer { target: "v1".into(), from: 10, deadline: 20 };
        let sources = Sources { rollback_offer: Some(offer), unreachable: Some("refused".into()), rolled_back_to: Some("v0".into()), ..Sources::default() };
        let list = stack(&sources, &[], |_| false);
        assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["rollback", "rolled-back"]);
        assert_eq!((list[0].text.as_str(), list[0].actions[0].title, list[0].closable, list[0].countdown), ("Rolling back", "Cancel", false, Some((10, 20))));
        assert_eq!((list[1].text.as_str(), list[1].closable), ("Rolled back to v0", true));
        let down = Sources { unreachable: Some("refused".into()), ..Sources::default() };
        let list = stack(&down, &[], |_| false);
        assert_eq!((list[0].id.as_str(), list[0].text.as_str(), list[0].closable), ("unreachable", "wheeld unreachable: refused. Retrying every 2 seconds.", false));
    }

    #[test]
    fn posting_replaces_the_same_id() {
        let mut posted = vec![];
        post(&mut posted, Toast::posted("x".into(), Level::Notice, "one".into(), None));
        post(&mut posted, Toast::posted("y".into(), Level::Notice, "two".into(), None));
        post(&mut posted, Toast::posted("x".into(), Level::Error, "three".into(), None));
        assert_eq!(posted.iter().map(|t| (t.id.as_str(), t.text.as_str())).collect::<Vec<_>>(), [("y", "two"), ("x", "three")]);
    }
}
