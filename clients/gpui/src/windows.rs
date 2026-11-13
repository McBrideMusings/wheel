//! The app's open windows, as `wheel window` sees them (AppModel+Windows.swift): every window
//! writes its record here (its name, the session and space it shows, its sidebar) and the one
//! wheeld forwards a window command to answers from these records and acts on the other windows
//! through their handles. Every window is still its own client of wheeld (ADR-0022); this is
//! the app-side list the Swift client keeps as `WindowLayout.windows`.
use crate::layout::{Session, Space, leaves_of};
use serde_json::{Value, json};
use std::cell::RefCell;

/// One open window as it last described itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub id: String,
    /// The name given with Rename Window or `wheel window rename`; none shows the space's name.
    pub name: Option<String>,
    pub session: Option<String>,
    /// The space shown.
    pub active: Option<String>,
    pub sidebar: bool,
    /// The pin: the window floats over every other.
    pub pinned: bool,
}

/// One little window as it last described itself (WheelKit's LittleWindow): one space's grid
/// under a strip, with no tabs and no sidebar. Its id is its window's id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Little {
    pub id: String,
    pub space: String,
    /// The main window whose tab for `space` it dims; none when it belongs to none.
    pub owner: Option<String>,
    pub pinned: bool,
}

/// The windows in the order they opened, and the one that was key last.
#[derive(Debug, Default)]
pub struct Registry {
    pub windows: Vec<Record>,
    pub focused: Option<String>,
    /// The little windows, in the order they opened.
    pub littles: Vec<Little>,
    /// Where each window's content was last measured on screen, by window id (saved.rs).
    pub frames: std::collections::HashMap<String, crate::saved::Frame>,
    /// Told after every change to the windows: the menu bar item counts what no window shows.
    pub(crate) watchers: Vec<futures::channel::mpsc::UnboundedSender<()>>,
}
impl Registry {
    /// A channel that hears once after each change to the list, until its receiver drops.
    pub fn watch(&mut self) -> futures::channel::mpsc::UnboundedReceiver<()> {
        let (send, receive) = futures::channel::mpsc::unbounded();
        self.watchers.push(send);
        receive
    }
    fn changed(&mut self) {
        self.watchers.retain(|w| w.unbounded_send(()).is_ok());
    }
    pub fn put_little(&mut self, little: Little) {
        if self.littles.iter().any(|l| *l == little) {
            return;
        }
        match self.littles.iter_mut().find(|l| l.id == little.id) {
            Some(held) => *held = little,
            None => self.littles.push(little),
        }
        self.changed();
    }
    /// Whether a main window shows `session`.
    pub fn shows_session(&self, session: &str) -> bool {
        self.windows.iter().any(|w| w.session.as_deref() == Some(session))
    }
    /// The little window showing `space`, if one is open.
    pub fn little_showing(&self, space: &str) -> Option<&Little> {
        self.littles.iter().find(|l| l.space == space)
    }
    /// The main window holding `space`: the focused one when its session holds it, else the
    /// first whose session does (WindowLayout.window(holding:)).
    pub fn window_holding(&self, space: &str, sessions: &[Session]) -> Option<&Record> {
        let home = session_holding(sessions, space).map(|s| s.id.as_str())?;
        let holds = |w: &&Record| w.session.as_deref() == Some(home);
        self.focused().filter(holds).or_else(|| self.windows.iter().find(holds))
    }
    pub fn put(&mut self, record: Record) {
        if self.windows.iter().any(|w| *w == record) {
            return;
        }
        match self.windows.iter_mut().find(|w| w.id == record.id) {
            Some(held) => *held = record,
            None => self.windows.push(record),
        }
        self.changed();
    }
    /// Window `id` moved or resized to `frame`.
    pub fn set_frame(&mut self, id: &str, frame: crate::saved::Frame) {
        if self.frames.get(id) == Some(&frame) {
            return;
        }
        self.frames.insert(id.to_owned(), frame);
        self.changed();
    }
    pub fn remove(&mut self, id: &str) {
        self.windows.retain(|w| w.id != id);
        self.littles.retain(|l| l.id != id);
        self.frames.remove(id);
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
        self.changed();
    }
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.windows.iter().find(|w| w.id == id)
    }
    /// The window that was key last, else the newest.
    pub fn focused(&self) -> Option<&Record> {
        self.focused.as_deref().and_then(|id| self.get(id)).or(self.windows.last())
    }
    /// Closing `id` ends what its session shows: no other window shows the session, and it holds
    /// spaces (AppModel.windowCloseNeedsAnswer).
    pub fn close_needs_answer(&self, id: &str, sessions: &[Session]) -> bool {
        let Some(session) = self.get(id).and_then(|w| w.session.as_deref()) else { return false };
        let shown_elsewhere = self.windows.iter().any(|w| w.id != id && w.session.as_deref() == Some(session));
        !shown_elsewhere && sessions.iter().find(|s| s.id == session).is_some_and(|s| !s.spaces.is_empty())
    }
}

thread_local! {
    /// Windows live on the main thread, and so does this list.
    static REGISTRY: RefCell<Registry> = RefCell::default();
}

/// The session whose spaces hold `space`, if one does.
pub fn session_holding<'a>(sessions: &'a [Session], space: &str) -> Option<&'a Session> {
    sessions.iter().find(|s| s.spaces.iter().any(|id| id == space))
}

/// Run `f` over the app's list of windows.
pub fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    REGISTRY.with(|r| f(&mut r.borrow_mut()))
}

/// A window's title: its name, else the name of the space it shows, else "Wheel" (WindowLayout.title).
pub fn title(window: &Record, spaces: &[Space]) -> String {
    window.name.clone()
        .or_else(|| window.active.as_ref().and_then(|a| spaces.iter().find(|s| &s.id == a)).map(|s| s.name.clone()))
        .unwrap_or_else(|| "Wheel".into())
}

/// The item `text` names: its id, else its one name; else which there are, as the Swift app says.
fn resolve<T>(kind: &str, text: &str, items: &[T], id: impl Fn(&T) -> &str, name: impl Fn(&T) -> String) -> Result<String, String> {
    if items.iter().any(|i| id(i) == text) {
        return Ok(text.to_owned());
    }
    let named: Vec<&T> = items.iter().filter(|i| name(i) == text).collect();
    match named.len() {
        1 => Ok(id(named[0]).to_owned()),
        0 => Err(format!("no {kind} {text}; the {kind}s are {}", items.iter().map(|i| format!("{} ({})", name(i), id(i))).collect::<Vec<_>>().join(", "))),
        n => Err(format!("{n} {kind}s are named {text}; use one of their ids: {}", named.iter().map(|i| id(i)).collect::<Vec<_>>().join(", "))),
    }
}

/// The window a CLI argument names: its id, else its title (AppModel.windowID(named:)).
pub fn window_id(registry: &Registry, text: &str, spaces: &[Space]) -> Result<String, String> {
    resolve("window", text, &registry.windows, |w| &w.id, |w| title(w, spaces))
}

/// The space a CLI argument names: its id, else its name (AppModel.spaceID(named:)).
pub fn space_id(text: &str, spaces: &[Space]) -> Result<String, String> {
    resolve("space", text, spaces, |s| &s.id, |s| s.name.clone())
}

/// What `a space is always in a session` refuses `wheel window space-move <space> background` with.
pub const NO_BACKGROUND_MOVE: &str = "a space is always in a session; `wheel session move` puts it in one, and a session no window shows is the background";

/// The spaces of `session`, in its order.
fn spaces_of<'a>(session: Option<&str>, sessions: &[Session], spaces: &'a [Space]) -> Vec<&'a Space> {
    sessions.iter().find(|s| Some(s.id.as_str()) == session)
        .map(|s| s.spaces.iter().filter_map(|id| spaces.iter().find(|sp| &sp.id == id)).collect())
        .unwrap_or_default()
}

/// `wheel window ls`: every window with its spaces, the background (the spaces of the sessions
/// no window shows) and the little windows, as the Swift client answers.
pub fn ls(registry: &Registry, sessions: &[Session], spaces: &[Space]) -> Value {
    let named = |s: &&Space| json!({"id": s.id, "name": s.name});
    let focused = registry.focused().map(|w| w.id.clone());
    // A space in a little window is not in the background (WindowLayout.backgroundSpaces).
    let background: Vec<Value> = sessions.iter().filter(|s| !registry.shows_session(&s.id))
        .flat_map(|s| spaces_of(Some(&s.id), sessions, spaces)).filter(|s| registry.little_showing(&s.id).is_none()).map(|s| named(&s)).collect();
    let little = |l: &Little| {
        let space = match spaces.iter().find(|s| s.id == l.space) { Some(s) => json!({"id": s.id, "name": s.name}), None => json!({"id": l.space}) };
        json!({"id": l.id, "space": space, "owner": l.owner})
    };
    json!({
        "windows": registry.windows.iter().map(|w| json!({
            "id": w.id,
            "name": w.name,
            "title": title(w, spaces),
            "session": w.session,
            "spaces": spaces_of(w.session.as_deref(), sessions, spaces).iter().map(named).collect::<Vec<_>>(),
            "active": w.active,
            "sidebar": w.sidebar,
            "focused": Some(&w.id) == focused.as_ref(),
        })).collect::<Vec<_>>(),
        "background": background,
        "little": registry.littles.iter().map(little).collect::<Vec<_>>(),
    })
}

/// "The “Main” session holds 3 spaces with 6 panes. 1 of them is in a little window."
/// (AppModel.closeSummary)
pub fn close_summary(registry: &Registry, window: &Record, sessions: &[Session], spaces: &[Space]) -> String {
    let held = spaces_of(window.session.as_deref(), sessions, spaces);
    let panes: usize = held.iter().map(|s| leaves_of(&s.root).len()).sum();
    let name = sessions.iter().find(|s| Some(&s.id) == window.session.as_ref()).map(|s| s.name.as_str()).unwrap_or("This");
    let noun = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    let mut text = format!("The “{name}” session holds {} {} with {panes} {}.", held.len(), noun(held.len(), "space", "spaces"), noun(panes, "pane", "panes"));
    let away = held.iter().filter(|s| registry.little_showing(&s.id).is_some()).count();
    if away > 0 {
        text += &format!(" {away} of them {} in a little window.", noun(away, "is", "are"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Leaf, Tree};

    fn space(id: &str, name: &str, panes: usize) -> Space {
        let leaf = |n: usize| Tree::Pane(Leaf { id: format!("{id}p{n}"), kind: "terminal".into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None });
        let mut root = leaf(0);
        for n in 1..panes {
            root = Tree::Split { id: format!("{id}x{n}"), horizontal: true, ratio: 0.5, a: Box::new(root), b: Box::new(leaf(n)) };
        }
        Space { id: id.into(), name: name.into(), cwd: "/".into(), root, status: None, popped: vec![], backgrounded: vec![] }
    }
    fn session(id: &str, name: &str, spaces: &[&str]) -> Session {
        Session { id: id.into(), name: name.into(), spaces: spaces.iter().map(|s| s.to_string()).collect() }
    }
    fn window(id: &str, session: &str, active: &str) -> Record {
        Record { id: id.into(), name: None, session: Some(session.into()), active: Some(active.into()), sidebar: true, pinned: false }
    }
    fn fixture() -> (Registry, Vec<Session>, Vec<Space>) {
        let spaces = vec![space("s1", "wheel", 2), space("s2", "docs", 1), space("s3", "site", 3)];
        let sessions = vec![session("n1", "Main", &["s1", "s2"]), session("n2", "Side", &["s3"]), session("n3", "Empty", &[])];
        let mut registry = Registry::default();
        registry.put(window("w1", "n1", "s1"));
        registry.put(Record { name: Some("Notes".into()), ..window("w2", "n1", "s2") });
        (registry, sessions, spaces)
    }

    #[test]
    fn ls_lists_windows_with_their_spaces_and_the_background() {
        let (mut registry, sessions, spaces) = fixture();
        registry.focused = Some("w1".into());
        let answer = ls(&registry, &sessions, &spaces);
        assert_eq!(answer["windows"][0]["title"], "wheel");
        assert_eq!(answer["windows"][1]["title"], "Notes");
        assert_eq!(answer["windows"][1]["name"], "Notes");
        assert_eq!(answer["windows"][0]["name"], Value::Null);
        assert_eq!(answer["windows"][0]["spaces"], json!([{"id": "s1", "name": "wheel"}, {"id": "s2", "name": "docs"}]));
        assert_eq!(answer["windows"][0]["focused"], true);
        assert_eq!(answer["windows"][1]["focused"], false);
        assert_eq!(answer["background"], json!([{"id": "s3", "name": "site"}]));
        assert_eq!(answer["little"], json!([]));
    }

    #[test]
    fn a_little_window_is_listed_and_leaves_the_background() {
        let (mut registry, sessions, spaces) = fixture();
        registry.focused = Some("w2".into());
        assert_eq!(registry.window_holding("s1", &sessions).map(|w| w.id.as_str()), Some("w2"));
        assert_eq!(registry.window_holding("s3", &sessions), None);
        registry.put_little(Little { id: "l1".into(), space: "s1".into(), owner: Some("w2".into()), pinned: false });
        registry.put_little(Little { id: "l2".into(), space: "s3".into(), owner: None, pinned: true });
        let answer = ls(&registry, &sessions, &spaces);
        assert_eq!(answer["little"], json!([{"id": "l1", "space": {"id": "s1", "name": "wheel"}, "owner": "w2"}, {"id": "l2", "space": {"id": "s3", "name": "site"}, "owner": null}]));
        assert_eq!(answer["background"], json!([]));
        assert_eq!(registry.little_showing("s3").map(|l| l.id.as_str()), Some("l2"));
        registry.remove("l2");
        assert!(registry.little_showing("s3").is_none());
        assert_eq!(ls(&registry, &sessions, &spaces)["background"], json!([{"id": "s3", "name": "site"}]));
    }

    #[test]
    fn names_resolve_by_id_then_title_and_say_what_exists() {
        let (registry, _, spaces) = fixture();
        assert_eq!(window_id(&registry, "w2", &spaces), Ok("w2".into()));
        assert_eq!(window_id(&registry, "Notes", &spaces), Ok("w2".into()));
        assert_eq!(window_id(&registry, "nope", &spaces), Err("no window nope; the windows are wheel (w1), Notes (w2)".into()));
        assert_eq!(space_id("docs", &spaces), Ok("s2".into()));
        assert_eq!(space_id("s3", &spaces), Ok("s3".into()));
        let twice = vec![space("a", "x", 1), space("b", "x", 1)];
        assert_eq!(space_id("x", &twice), Err("2 spaces are named x; use one of their ids: a, b".into()));
        assert_eq!(space_id("y", &twice), Err("no space y; the spaces are x (a), x (b)".into()));
    }

    #[test]
    fn only_the_last_window_on_a_session_with_spaces_asks_before_closing() {
        let (mut registry, sessions, spaces) = fixture();
        // Two windows show Main: either closes with no question.
        assert!(!registry.close_needs_answer("w1", &sessions));
        registry.remove("w2");
        assert!(registry.close_needs_answer("w1", &sessions));
        assert_eq!(close_summary(&registry, registry.get("w1").unwrap(), &sessions, &spaces), "The “Main” session holds 2 spaces with 3 panes.");
        registry.put_little(Little { id: "l1".into(), space: "s2".into(), owner: Some("w1".into()), pinned: false });
        assert_eq!(close_summary(&registry, registry.get("w1").unwrap(), &sessions, &spaces), "The “Main” session holds 2 spaces with 3 panes. 1 of them is in a little window.");
        registry.remove("l1");
        // A window on a session with no spaces closes.
        registry.put(window("w3", "n3", ""));
        assert!(!registry.close_needs_answer("w3", &sessions));
        let side = window("w4", "n2", "s3");
        assert_eq!(close_summary(&registry, &side, &sessions, &spaces), "The “Side” session holds 1 space with 3 panes.");
    }

    #[test]
    fn the_session_holding_a_space_is_found_by_its_spaces() {
        let (_, sessions, _) = fixture();
        assert_eq!(session_holding(&sessions, "s3").map(|s| s.id.as_str()), Some("n2"));
        assert!(session_holding(&sessions, "nowhere").is_none());
    }

    #[test]
    fn the_focused_window_is_the_last_key_else_the_newest() {
        let (mut registry, _, _) = fixture();
        assert_eq!(registry.focused().unwrap().id, "w2");
        registry.focused = Some("w1".into());
        assert_eq!(registry.focused().unwrap().id, "w1");
        registry.remove("w1");
        assert_eq!(registry.focused().unwrap().id, "w2");
    }
}
