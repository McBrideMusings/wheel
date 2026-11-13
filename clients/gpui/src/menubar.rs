//! What the menu bar item shows (the Swift client's `MenuBarView.swift` and `PopoverPlaces.swift`):
//! the count of spaces no window shows on the item, the approvals waiting where no window looks
//! on the Dock tile, and the popover's card per session with a row per space. Every number here
//! comes from wheeld's layout and the app's list of windows; `crate::menubar_popover` draws it.
use crate::layout::{Session, Space};
use crate::windows::Registry;
use std::collections::HashMap;
use wheel_proto::PermInfo;

/// Approval requests waiting in each space, by space id (AgentPanes.waiting): a request counts
/// in the space holding the agent pane that raised it, or the web pane whose page did.
pub fn waiting(perms: &[PermInfo], spaces: &[Space]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for space in spaces {
        let askers: Vec<&str> = space.all().flat_map(|l| [l.agent.as_deref(), (l.kind == "web").then_some(l.id.as_str())]).flatten().collect();
        let count = perms.iter().filter(|p| askers.contains(&p.agent.as_str())).count();
        if count > 0 {
            out.insert(space.id.clone(), count);
        }
    }
    out
}

/// The spaces a window shows now: each main window's active space and each little window's.
fn shown(registry: &Registry) -> Vec<&str> {
    registry.windows.iter().filter_map(|w| w.active.as_deref()).chain(registry.littles.iter().map(|l| l.space.as_str())).collect()
}

/// The spaces in sessions no main window shows and not in a little window, then those wheeld
/// lists in a little window that belongs to no main window (AppModel.windowlessSpaces): what the
/// item's badge counts.
pub fn windowless(registry: &Registry, sessions: &[Session], spaces: &[Space]) -> Vec<String> {
    let background = sessions.iter().filter(|s| !registry.shows_session(&s.id)).flat_map(|s| s.spaces.iter()).filter(|id| registry.little_showing(id).is_none());
    let ownerless = registry.littles.iter().filter(|l| l.owner.is_none() && spaces.iter().any(|s| s.id == l.space)).map(|l| &l.space);
    background.chain(ownerless).cloned().collect()
}

/// Approvals waiting in spaces no window shows (AppModel.unseenApprovals): the Dock tile's badge.
pub fn unseen(registry: &Registry, waiting: &HashMap<String, usize>) -> usize {
    let shown = shown(registry);
    waiting.iter().filter(|(space, _)| !shown.contains(&space.as_str())).map(|(_, n)| n).sum()
}

/// What VoiceOver reads for the item (MenuBarLabel.accessibility).
pub fn accessibility(background: usize, waiting: usize, update: bool) -> String {
    let mut parts = vec!["Wheel".to_owned()];
    if update {
        parts.push("a new server waits for a restart".into());
    }
    if background > 0 {
        parts.push(format!("{background} {} in sessions with no window", if background == 1 { "space" } else { "spaces" }));
    }
    if waiting > 0 {
        parts.push(format!("{waiting} waiting for approval"));
    }
    parts.join(", ")
}

/// One space in a session's card (SpaceRow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub space: String,
    pub name: String,
    /// Approvals waiting in it; the badge replaces the shown dot.
    pub waiting: usize,
    /// A window of the session shows it.
    pub shown: bool,
    /// It is in a little window that belongs to no main window.
    pub ownerless_little: bool,
    /// A little window shows it: the row's Little Window button is lit.
    pub in_little: bool,
}

impl Row {
    /// The row draws dimmed while its space is in a little window a main window owns
    /// (SpaceRow: `isDimmed && !ownerlessLittle`).
    pub fn dimmed(&self) -> bool {
        self.in_little && !self.ownerless_little
    }
}

/// One session's card (SessionCard): its name, the number of each window showing it, its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub session: String,
    pub name: String,
    /// Each window showing it, by its place among the open windows from 1 (AppModel.windowNumber).
    pub windows: Vec<usize>,
    pub rows: Vec<Row>,
}

/// The popover's cards, in wheeld's order of sessions and spaces.
pub fn cards(registry: &Registry, sessions: &[Session], spaces: &[Space], waiting: &HashMap<String, usize>) -> Vec<Card> {
    sessions.iter().map(|session| {
        let holders: Vec<(usize, &crate::windows::Record)> = registry.windows.iter().enumerate().filter(|(_, w)| w.session.as_deref() == Some(session.id.as_str())).collect();
        let windows = holders.iter().map(|(i, _)| i + 1).collect();
        let rows = session.spaces.iter().filter_map(|id| spaces.iter().find(|s| &s.id == id)).map(|space| Row {
            space: space.id.clone(),
            name: space.name.clone(),
            waiting: waiting.get(&space.id).copied().unwrap_or(0),
            shown: holders.iter().any(|(_, w)| w.active.as_deref() == Some(space.id.as_str())),
            ownerless_little: registry.little_showing(&space.id).is_some_and(|l| l.owner.is_none()),
            in_little: registry.little_showing(&space.id).is_some(),
        }).collect();
        Card { session: session.id.clone(), name: session.name.clone(), windows, rows }
    }).collect()
}

/// What closing `session` ends, as a sentence (AppModel.closeWarning): the window's question
/// and the popover's alert both say it.
pub fn close_warning(sessions: &[Session], spaces: &[Space], session: &str) -> String {
    let held: Vec<&Space> = sessions.iter().find(|s| s.id == session).map(|s| s.spaces.iter().filter_map(|id| spaces.iter().find(|sp| &sp.id == id)).collect()).unwrap_or_default();
    if held.is_empty() {
        return "It holds no spaces.".into();
    }
    let panes: Vec<&crate::layout::Leaf> = held.iter().flat_map(|s| crate::layout::leaves_of(&s.root)).collect();
    let count = |n: usize, noun: &str| format!("{n} {noun}{}", if n == 1 { "" } else { "s" });
    let shells = panes.iter().filter(|p| p.kind == "terminal").count();
    let agents = panes.iter().filter(|p| p.kind == "agent").count();
    let mut text = format!("It closes {} and {}", count(held.len(), "space"), count(panes.len(), "pane"));
    let mut running = vec![];
    if shells > 0 {
        running.push(count(shells, "terminal"));
    }
    if agents > 0 {
        running.push(count(agents, "agent"));
    }
    if !running.is_empty() {
        text += &format!(", and ends {}", running.join(" and "));
    }
    text + ". Every window showing it closes."
}

/// The popover's width (MenuBarPopover's `.frame(width: 320)`).
pub const WIDTH: f32 = 320.;
/// Its padding, the gap between its sections, and the gap between the rows of a list.
pub const PAD: f32 = 12.;
pub const GAP: f32 = 10.;
pub const ROW_GAP: f32 = 2.;
/// A row of the popover (PopoverRow: 3pt above and below a line of text).
pub const ROW: f32 = 24.;
/// A card's padding and its header's height with the 4pt under it.
pub const CARD_PAD: f32 = 10.;
pub const CARD_HEAD: f32 = 24.;

/// How tall a card is: its border and padding, its header, then a row per space or the one line
/// saying there are none.
pub fn card_height(card: &Card) -> f32 {
    2. + 2. * CARD_PAD + CARD_HEAD + ROW * card.rows.len().max(1) as f32
}

/// The server line's height while its details are closed: the app icon's (ServerHeader).
pub const SERVER_LINE: f32 = 36.;
/// The pairing row: a title over the address line, 3pt above and below (PairingRow).
pub const PAIRING_ROW: f32 = 36.;

/// The popover's height with the server line closed: the server line, a divider, the pairing row
/// over Settings…, the cards, a divider and New Window and Quit, `GAP` apart. The popover opens at this height and then fits
/// the height its content measures.
pub fn height(cards: &[Card]) -> f32 {
    let sections = cards.len() + 5;
    let cards: f32 = cards.iter().map(card_height).sum();
    2. * PAD + SERVER_LINE + 1. + PAIRING_ROW + ROW_GAP + ROW + cards + 1. + 2. * ROW + ROW_GAP + GAP * (sections - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Leaf, Tree};
    use crate::windows::{Little, Record};

    fn leaf(id: &str, kind: &str, agent: Option<&str>) -> Leaf {
        Leaf { id: id.into(), kind: kind.into(), label: String::new(), term: None, url: None, scope: None, agent: agent.map(Into::into), harness: None, home: None, status: None }
    }
    fn space(id: &str, panes: Vec<Leaf>) -> Space {
        let mut leaves = panes.into_iter().map(Tree::Pane);
        let first = leaves.next().unwrap_or_else(|| Tree::Pane(leaf(&format!("{id}-p"), "terminal", None)));
        let root = leaves.fold(first, |a, b| Tree::Split { id: format!("{id}-split"), horizontal: true, ratio: 0.5, a: Box::new(a), b: Box::new(b) });
        Space { id: id.into(), name: id.to_uppercase(), cwd: "/tmp".into(), root, status: None, popped: vec![], backgrounded: vec![] }
    }
    fn agent(pane: &str, agent: &str) -> Leaf {
        leaf(pane, "agent", Some(agent))
    }
    fn perm(agent: &str) -> PermInfo {
        serde_json::from_value(serde_json::json!({"id": format!("perm-{agent}"), "agent": agent, "title": "Run", "options": []})).unwrap()
    }
    fn session(id: &str, spaces: &[&str]) -> Session {
        Session { id: id.into(), name: id.to_uppercase(), spaces: spaces.iter().map(|s| s.to_string()).collect() }
    }
    fn window(id: &str, session: &str, active: &str) -> Record {
        Record { id: id.into(), name: None, session: Some(session.into()), active: Some(active.into()), sidebar: true, pinned: false }
    }

    #[test]
    fn waiting_counts_each_request_in_the_space_holding_its_agent_or_page() {
        let spaces = [space("s1", vec![agent("p1", "a1"), leaf("p2", "web", None)]), space("s2", vec![agent("p3", "a3")])];
        let perms = [perm("a1"), perm("p2"), perm("a3"), perm("gone")];
        let waiting = waiting(&perms, &spaces);
        assert_eq!(waiting.get("s1"), Some(&2));
        assert_eq!(waiting.get("s2"), Some(&1));
        assert_eq!(waiting.len(), 2);
    }

    #[test]
    fn the_badge_counts_background_sessions_and_ownerless_little_windows() {
        let sessions = [session("n1", &["s1", "s2"]), session("n2", &["s3", "s4"]), session("n3", &["s5"])];
        let spaces: Vec<Space> = ["s1", "s2", "s3", "s4", "s5"].into_iter().map(|id| space(id, vec![])).collect();
        let mut registry = Registry::default();
        registry.put(window("w1", "n1", "s1"));
        // s3 is in a little window owned by w1: not in the background; s5's is ownerless: counted once.
        registry.put_little(Little { id: "l1".into(), space: "s3".into(), owner: Some("w1".into()), pinned: false });
        registry.put_little(Little { id: "l2".into(), space: "s5".into(), owner: None, pinned: false });
        assert_eq!(windowless(&registry, &sessions, &spaces), ["s4", "s5"]);
        // A little window on a space wheeld no longer lists counts for nothing.
        registry.put_little(Little { id: "l3".into(), space: "gone".into(), owner: None, pinned: false });
        assert_eq!(windowless(&registry, &sessions, &spaces), ["s4", "s5"]);
        assert_eq!(accessibility(2, 0, false), "Wheel, 2 spaces in sessions with no window");
        assert_eq!(accessibility(1, 3, false), "Wheel, 1 space in sessions with no window, 3 waiting for approval");
        assert_eq!(accessibility(0, 0, false), "Wheel");
        assert_eq!(accessibility(1, 0, true), "Wheel, a new server waits for a restart, 1 space in sessions with no window");
    }

    #[test]
    fn unseen_approvals_skip_the_spaces_a_window_shows() {
        let mut registry = Registry::default();
        registry.put(window("w1", "n1", "s1"));
        registry.put_little(Little { id: "l1".into(), space: "s2".into(), owner: None, pinned: false });
        let waiting = HashMap::from([("s1".to_owned(), 2), ("s2".to_owned(), 1), ("s3".to_owned(), 4)]);
        assert_eq!(unseen(&registry, &waiting), 4);
    }

    #[test]
    fn closing_a_session_names_its_spaces_panes_terminals_and_agents() {
        let sessions = [session("n1", &["s1", "s2"]), session("n2", &[])];
        let spaces = [space("s1", vec![agent("p1", "a1"), leaf("p2", "web", None)]), space("s2", vec![leaf("p3", "terminal", None)])];
        assert_eq!(close_warning(&sessions, &spaces, "n1"), "It closes 2 spaces and 3 panes, and ends 1 terminal and 1 agent. Every window showing it closes.");
        assert_eq!(close_warning(&sessions, &spaces, "n2"), "It holds no spaces.");
    }

    #[test]
    fn cards_list_every_session_with_its_windows_and_rows() {
        let sessions = [session("n1", &["s1", "s2"]), session("n2", &["s3"]), session("n3", &[])];
        let spaces = [space("s1", vec![]), space("s2", vec![]), space("s3", vec![])];
        let mut registry = Registry::default();
        registry.put(window("w1", "n2", "s3"));
        registry.put(window("w2", "n1", "s2"));
        registry.put(window("w3", "n2", "s3"));
        registry.put_little(Little { id: "l1".into(), space: "s1".into(), owner: None, pinned: false });
        let waiting = HashMap::from([("s2".to_owned(), 1)]);
        let cards = cards(&registry, &sessions, &spaces, &waiting);
        assert_eq!(cards.iter().map(|c| (c.name.as_str(), c.windows.clone())).collect::<Vec<_>>(), [("N1", vec![2]), ("N2", vec![1, 3]), ("N3", vec![])]);
        assert_eq!(cards[0].rows, [
            Row { space: "s1".into(), name: "S1".into(), waiting: 0, shown: false, ownerless_little: true, in_little: true },
            Row { space: "s2".into(), name: "S2".into(), waiting: 1, shown: true, ownerless_little: false, in_little: false },
        ]);
        // An ownerless little window marks its row; one a window owns dims it.
        assert!(!cards[0].rows[0].dimmed());
        registry.put_little(Little { id: "l2".into(), space: "s3".into(), owner: Some("w1".into()), pinned: false });
        let owned = super::cards(&registry, &sessions, &spaces, &waiting);
        assert!(owned[1].rows[0].in_little && owned[1].rows[0].dimmed());
        assert!(cards[1].rows[0].shown);
        assert!(cards[2].rows.is_empty());
        // The pairing row and Settings…, three cards (2 rows, 1 row, and the empty one's line), then
        // the divider and two rows.
        assert_eq!(height(&cards), 2. * PAD + SERVER_LINE + 1. + (PAIRING_ROW + ROW_GAP + ROW) + (3. * (2. + 2. * CARD_PAD + CARD_HEAD) + 4. * ROW) + 1. + 2. * ROW + ROW_GAP + 7. * GAP);
    }
}
