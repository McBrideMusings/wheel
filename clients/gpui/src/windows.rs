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
}

/// The windows in the order they opened, and the one that was key last.
#[derive(Debug, Default)]
pub struct Registry {
    pub windows: Vec<Record>,
    pub focused: Option<String>,
}
impl Registry {
    pub fn put(&mut self, record: Record) {
        match self.windows.iter_mut().find(|w| w.id == record.id) {
            Some(held) => *held = record,
            None => self.windows.push(record),
        }
    }
    pub fn remove(&mut self, id: &str) {
        self.windows.retain(|w| w.id != id);
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
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
    let shown: Vec<&str> = registry.windows.iter().filter_map(|w| w.session.as_deref()).collect();
    let background: Vec<Value> = sessions.iter().filter(|s| !shown.contains(&s.id.as_str()))
        .flat_map(|s| spaces_of(Some(&s.id), sessions, spaces)).map(|s| named(&s)).collect();
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
        "little": [],
    })
}

/// "The “Main” session holds 3 spaces with 6 panes." (AppModel.closeSummary)
pub fn close_summary(window: &Record, sessions: &[Session], spaces: &[Space]) -> String {
    let held = spaces_of(window.session.as_deref(), sessions, spaces);
    let panes: usize = held.iter().map(|s| leaves_of(&s.root).len()).sum();
    let name = sessions.iter().find(|s| Some(&s.id) == window.session.as_ref()).map(|s| s.name.as_str()).unwrap_or("This");
    let noun = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    format!("The “{name}” session holds {} {} with {panes} {}.", held.len(), noun(held.len(), "space", "spaces"), noun(panes, "pane", "panes"))
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
        Space { id: id.into(), name: name.into(), cwd: "/".into(), root, status: None }
    }
    fn session(id: &str, name: &str, spaces: &[&str]) -> Session {
        Session { id: id.into(), name: name.into(), spaces: spaces.iter().map(|s| s.to_string()).collect() }
    }
    fn window(id: &str, session: &str, active: &str) -> Record {
        Record { id: id.into(), name: None, session: Some(session.into()), active: Some(active.into()), sidebar: true }
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
        assert_eq!(close_summary(registry.get("w1").unwrap(), &sessions, &spaces), "The “Main” session holds 2 spaces with 3 panes.");
        // A window on a session with no spaces closes.
        registry.put(window("w3", "n3", ""));
        assert!(!registry.close_needs_answer("w3", &sessions));
        let side = window("w4", "n2", "s3");
        assert_eq!(close_summary(&side, &sessions, &spaces), "The “Side” session holds 1 space with 3 panes.");
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
