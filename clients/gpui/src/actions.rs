//! Every built-in action by id: its title and its built-in key, in wheeld's spelling. A port of
//! the registry in apple/Mac/Sources/CommandActions.swift: the palette, the keymap and the
//! shortcut sheet all name an action through its entry here, and `view.rs` says which can run
//! now and runs it. The keys follow Ghostty's: a Ghostty tab is a space, a split a pane.
use crate::keymap::Keymap;

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub title: String,
    /// The built-in key, in the spelling wheeld stores a `keybind` trigger in.
    pub keys: Option<String>,
}
fn action(id: &str, title: &str, keys: Option<&str>) -> Action {
    Action { id: id.to_owned(), title: title.to_owned(), keys: keys.map(str::to_owned) }
}

pub const DIRS: [(&str, &str); 4] = [("left", "arrow_left"), ("right", "arrow_right"), ("up", "arrow_up"), ("down", "arrow_down")];

/// The fixed actions, in the registry's order.
pub fn fixed() -> Vec<Action> {
    let mut list = vec![
        action("open-folder", "Open Folder…", Some("cmd+o")),
        action("new-window", "New Window", Some("cmd+n")),
        action("new-space", "New Space…", Some("cmd+t")),
        action("rename-window", "Rename Window…", None),
        action("close-pane", "Close Pane", Some("cmd+w")),
        action("close-space", "Close Space", Some("opt+cmd+w")),
        action("close-window", "Close Window", Some("shift+cmd+w")),
        action("settings", "Settings", Some("cmd+comma")),
        action("usage", "Usage", None),
        action("command-palette", "Command Palette…", Some("cmd+k")),
        action("shortcuts", "Keyboard Shortcuts", Some("cmd+slash")),
        action("toggle-sidebar", "Toggle Sidebar", Some("ctrl+cmd+s")),
        action("mode-spaces", "Show Spaces", Some("opt+cmd+1")),
        action("mode-desk", "Show Desk", Some("opt+cmd+2")),
        action("split-right", "Split Right", Some("cmd+d")),
        action("split-down", "Split Down", Some("shift+cmd+d")),
        action("focus-previous", "Previous Pane", Some("cmd+bracket_left")),
        action("focus-next", "Next Pane", Some("cmd+bracket_right")),
    ];
    for (dir, arrow) in DIRS {
        list.push(Action { id: format!("focus-{dir}"), title: format!("Pane {}", capitalized(dir)), keys: Some(format!("opt+cmd+{arrow}")) });
    }
    for (dir, arrow) in DIRS {
        list.push(Action { id: format!("resize-{dir}"), title: format!("Move Divider {}", capitalized(dir)), keys: Some(format!("ctrl+cmd+{arrow}")) });
    }
    list.extend([
        action("equalize-splits", "Equalize Panes", Some("ctrl+cmd+equal")),
        action("zoom-pane", "Zoom Pane", Some("shift+cmd+enter")),
        action("switch-view", "Switch Agent Pane Between Chat and Terminal", Some("shift+cmd+t")),
        action("pane-terminal", "Show a Terminal in This Pane", None),
        action("pane-browser", "Show a Browser in This Pane", None),
        action("pane-artifact", "Show an Artifact in This Pane…", None),
        action("previous-space", "Previous Space", Some("shift+cmd+bracket_left")),
        action("next-space", "Next Space", Some("shift+cmd+bracket_right")),
    ]);
    for n in 1..=8 {
        list.push(Action { id: format!("space-{n}"), title: format!("Space {n}"), keys: Some(format!("cmd+{n}")) });
    }
    list.push(action("last-space", "Last Space", Some("cmd+9")));
    list
}

/// One "show this agent" action per agent whose program was found, in the picker's order.
pub fn agent_actions(agents: &[(String, String)]) -> Vec<Action> {
    agents.iter().map(|(name, title)| Action { id: format!("pane-agent-{name}"), title: format!("Start {title} in This Pane"), keys: None }).collect()
}

/// One "Run" row per `[runner]` command of the active space. Only the palette lists these, so
/// no key is bound to one: which runners there are changes with the space.
pub fn runner_actions(runners: &[String]) -> Vec<Action> {
    runners.iter().map(|name| Action { id: format!("run-{name}"), title: format!("Run {name}"), keys: None }).collect()
}

/// Each action with its built-in key: what the `keybind` setting applies over.
pub fn defaults(all: &[Action]) -> Vec<(&str, Option<&str>)> {
    all.iter().map(|a| (a.id.as_str(), a.keys.as_deref())).collect()
}

/// The shortcut sheet's groups, in the order it shows them, each with the actions it lists.
pub fn shortcut_groups() -> Vec<(&'static str, Vec<String>)> {
    let mut spaces: Vec<String> = ["new-space", "close-space", "previous-space", "next-space"].map(String::from).to_vec();
    spaces.extend((1..=8).map(|n| format!("space-{n}")));
    spaces.push("last-space".into());
    vec![
        ("Panes", ["split-right", "split-down", "close-pane", "focus-previous", "focus-next", "focus-left", "focus-right", "focus-up", "focus-down",
            "resize-left", "resize-right", "resize-up", "resize-down", "equalize-splits", "zoom-pane"].map(String::from).to_vec()),
        ("Spaces", spaces),
        ("Windows", ["new-window", "close-window", "rename-window", "open-folder", "toggle-sidebar", "mode-spaces", "mode-desk", "settings", "command-palette", "shortcuts"].map(String::from).to_vec()),
        ("Agent", vec!["switch-view".into()]),
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShortcutRow {
    pub id: String,
    pub title: String,
    /// The keys the keymap gives the action now, as a menu prints them; empty when unbound.
    pub keys: String,
}

/// What the shortcut sheet lists: every action in the groups with the key the keymap gives it,
/// so a `keybind` setting shows there as it does in the menus.
pub fn shortcut_sheet(all: &[Action], keymap: &Keymap) -> Vec<(&'static str, Vec<ShortcutRow>)> {
    shortcut_groups().into_iter().map(|(name, ids)| {
        (name, ids.iter().filter_map(|id| all.iter().find(|a| &a.id == id).map(|a| ShortcutRow { id: id.clone(), title: a.title.clone(), keys: keymap.symbol(id) })).collect())
    }).collect()
}

fn capitalized(word: &str) -> String {
    let mut c = word.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_matches_the_swift_one() {
        let all = fixed();
        assert_eq!(all.len(), 43);
        assert!(all.iter().all(|a| a.keys.as_deref().is_none_or(|k| crate::keymap::KeyChord::parse(k).is_some())), "every built-in key is in wheeld's spelling");
        let ids: Vec<&str> = all.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids[..3], ["open-folder", "new-window", "new-space"]);
        assert_eq!(all.iter().find(|a| a.id == "focus-left").unwrap().keys.as_deref(), Some("opt+cmd+arrow_left"));
        assert_eq!(all.iter().find(|a| a.id == "resize-up").unwrap().title, "Move Divider Up");
        assert_eq!(all.iter().find(|a| a.id == "space-9"), None);
        assert_eq!(all.iter().find(|a| a.id == "last-space").unwrap().keys.as_deref(), Some("cmd+9"));
        let keymap = Keymap::new(&defaults(&all), "cmd+e=split-right");
        let sheet = shortcut_sheet(&all, &keymap);
        assert_eq!(sheet[0].0, "Panes");
        assert_eq!(sheet[0].1[0], ShortcutRow { id: "split-right".into(), title: "Split Right".into(), keys: "⌘E".into() });
        assert_eq!(sheet[1].1.len(), 13);
        assert_eq!(runner_actions(&["build".into()])[0].id, "run-build");
        assert_eq!(agent_actions(&[("claude".into(), "Claude".into())])[0].title, "Start Claude in This Pane");
    }
}
