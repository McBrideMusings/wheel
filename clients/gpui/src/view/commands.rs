//! The window's commands: which action each key runs (apple/Mac/Sources/KeyBindings.swift and
//! KeyActions.swift), what each action does here, the command palette's operations
//! (CommandActions.swift's AppModel extension), the shortcut sheet and the confirmations a
//! close asks. The palette, a key and a check all run an action through `run_action`, so an
//! action has one procedure.
use super::*;
use crate::{actions::{self, Action}, command_palette::{CommandPalette, Item, Mode, OpenPane, OpenSpace, PaletteAction, Verb}, keymap::{KeyChord, Keymap}};
use gpui::Keystroke;
use wheel_proto::{Dir, StartKind};

/// A destructive step waiting on a yes: closing a pane or a space that runs something, or
/// replacing what a pane runs. The window shows it as a sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirmation {
    pub title: String,
    pub message: String,
    pub button: String,
    pub run: Pending,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    ClosePane(String),
    CloseSpace(String),
    /// Close Other Spaces in Window: every space listed goes.
    CloseSpaces(Vec<String>),
    SetKind(String, NewPaneKind),
    StartAgent(String, String),
    /// Remove an installed built-in artifact: its folder and its entry go.
    RemoveBuiltin(String),
    /// Close a session: every space in it and every pane in those.
    CloseSession(String),
}

/// The requests the open palette has out, so their answers are told apart.
#[derive(Debug, Default)]
pub struct PaletteReqs {
    pub recent: Option<u64>,
    pub runners: Option<u64>,
    pub artifacts: Option<u64>,
    pub roots: Option<u64>,
    pub dirs: HashMap<u64, String>,
    /// A `pane_split` made for "Show Artifact": the new pane shows this artifact.
    pub artifact_after_split: HashMap<u64, ArtifactInfo>,
}

/// What a pane is called in the palette.
pub fn palette_name(leaf: &Leaf) -> String {
    match leaf.kind.as_str() {
        "empty" => "Empty pane".into(),
        "terminal" => "Terminal".into(),
        "agent" => capitalized(leaf.agent.as_deref().unwrap_or("agent")),
        _ => leaf.label.clone(),
    }
}
pub(super) fn capitalized(word: &str) -> String {
    let mut c = word.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Client {
    /// Every action there is now: the fixed ones and one per agent whose program was found.
    pub fn all_actions(&self) -> Vec<Action> {
        let mut all = actions::fixed();
        let agents: Vec<(String, String)> = self.agents.iter().filter(|a| a.found).map(|a| (a.name.clone(), capitalized(&a.name))).collect();
        all.extend(actions::agent_actions(&agents));
        all
    }
    /// Build the keymap again from the actions there are and the `keybind` setting.
    pub fn rebuild_keymap(&mut self) {
        let all = self.all_actions();
        let map = Keymap::new(&actions::defaults(&all), &self.keybind);
        if map != self.keymap {
            self.keymap = map;
            self.refresh_command_palette();
        }
    }
    /// The `keybind` setting arrived with a settings list.
    pub fn set_keybind(&mut self, value: &str) {
        if value != self.keybind {
            self.keybind = value.to_owned();
            self.rebuild_keymap();
        }
    }
    fn focused_leaf(&self) -> Option<Leaf> {
        self.scene().0.into_iter().find(|p| Some(&p.leaf.id) == self.focus.current.as_ref()).map(|p| p.leaf)
    }
    fn many_panes(&self) -> bool {
        self.space().is_some_and(|s| leaves_of(&s.root).len() > 1)
    }
    /// Whether the action means anything now. The surfaces other parity beads own (settings,
    /// usage, the sidebar, the window modes, the agent pane's two views, renaming) are
    /// listed and unbound until those beads land.
    pub fn action_enabled(&self, id: &str) -> bool {
        self.enabled(id, false)
    }
    /// `routing` answers for the bridge's chord set: the keyboard may sit in a native pane by a
    /// click GPUI never saw, so a pane action counts as possible while the space has a pane; the
    /// pane itself is resolved when the key arrives.
    fn enabled(&self, id: &str, routing: bool) -> bool {
        let up = self.link == Link::Up;
        let drawn = |kind: &str| matches!(kind, "empty" | "terminal" | "web" | "browser" | "agent" | "plugin");
        let has_pane = || if routing { self.space().is_some_and(|s| leaves_of(&s.root).iter().any(|l| drawn(&l.kind))) } else { self.focused_leaf().is_some_and(|l| drawn(&l.kind)) };
        let spaces = || self.session_spaces().len();
        match id {
            "open-folder" | "new-window" | "command-palette" | "shortcuts" | "close-window" | "rename-window" => true,
            "new-space" => up,
            "close-pane" => if routing { self.space().is_some() } else { self.focused_leaf().is_some() },
            "close-space" => self.space().is_some(),
            "split-right" | "split-down" | "pane-terminal" | "pane-browser" | "pane-artifact" => has_pane(),
            "focus-previous" | "focus-next" | "focus-left" | "focus-right" | "focus-up" | "focus-down"
            | "resize-left" | "resize-right" | "resize-up" | "resize-down" | "equalize-splits" => self.many_panes(),
            "previous-space" | "next-space" => spaces() > 1,
            "last-space" => spaces() > 0,
            "settings" => true,
            "toggle-sidebar" | "usage" | "mode-spaces" | "mode-desk" => true,
            "zoom-pane" => self.zoom_enabled(),
            "switch-view" => self.focused_leaf().is_some_and(|l| self.other_view(&l).is_some()),
            other => {
                if let Some(n) = other.strip_prefix("space-").and_then(|n| n.parse::<usize>().ok()) {
                    return spaces() >= n;
                }
                if other.starts_with("pane-agent-") {
                    return has_pane();
                }
                if let Some(name) = other.strip_prefix("run-") {
                    return up && self.runners_space == self.space_id && self.runners.iter().any(|r| r == name);
                }
                false
            }
        }
    }
    /// The chords that run an action now: what the bridge hands the GPUI view the keyboard for.
    pub fn sync_routed(&mut self) {
        let mut chords: Vec<String> = self.keymap.chords.iter().filter(|(id, _)| self.enabled(id, true)).map(|(_, c)| c.text()).collect();
        chords.sort();
        if chords != self.routed {
            self.native.call(serde_json::json!({"op": "keymap", "chords": chords}));
            self.routed = chords;
        }
    }
    /// Run the action `k` is bound to; false when the keymap leaves the key to the rest of the view.
    pub fn run_key(&mut self, k: &Keystroke, cx: &mut App) -> bool {
        let Some(chord) = KeyChord::from_keystroke(k) else { return false };
        let Some(id) = self.keymap.action(&chord).map(str::to_owned) else { return false };
        if !self.action_enabled(&id) {
            return false;
        }
        // A key that reached the GPUI view from a native pane (the bridge routed it) runs its
        // action and, unless the action moved the keyboard, gives the pane its keyboard back.
        let held = self.native.status()["held_before_key"].as_str().filter(|h| !h.is_empty()).map(str::to_owned);
        let before = (self.focus.current.clone(), self.space_id.clone());
        self.run_action(&id, cx);
        if let Some(held) = held
            && (self.focus.current.clone(), self.space_id.clone()) == before
            && !matches!(id.as_str(), "close-pane" | "close-space")
            && self.command_palette.is_none()
            && !self.shortcuts_open
            && !self.settings_open
            && self.confirmation.is_none()
        {
            self.native.focus(&held);
        }
        true
    }
    /// Run action `id` as the palette, a key or a check asks, and log it.
    pub fn run_action(&mut self, id: &str, cx: &mut App) {
        if !self.action_enabled(id) {
            self.note_action(format!("{id}: not run (unknown or not available now)"));
            return;
        }
        self.note_action(format!("{id}: run"));
        let focused = self.focus.current.clone();
        match id {
            "open-folder" => {
                self.close_command_palette();
                self.native.call(serde_json::json!({"op": "choose_folder"}));
            }
            "new-window" => self.new_window(cx),
            "new-space" => self.open_new_space_palette(),
            "close-pane" => {
                if let Some(pane) = focused {
                    self.ask_close_pane(&pane);
                }
            }
            "close-space" => {
                if let Some(space) = self.space_id.clone() {
                    self.ask_close_space(&space);
                }
            }
            "close-window" => self.request_close_window(),
            "rename-window" => self.begin_window_rename(),
            "toggle-sidebar" => self.toggle_sidebar(),
            "usage" => self.toggle_usage(),
            "mode-spaces" => self.set_mode("spaces"),
            "mode-desk" => self.set_mode("desk"),
            "command-palette" => self.open_command_palette(Mode::Root),
            "shortcuts" => self.toggle_shortcuts(),
            "settings" => self.toggle_settings(),
            "split-right" | "split-down" => {
                if let Some(pane) = focused {
                    let dir = if id == "split-right" { Dir::Right } else { Dir::Down };
                    self.send_layout(|req| ClientMsg::PaneSplit { req: Some(req), pane, dir: Some(dir), focus: false, site: None });
                }
            }
            "focus-previous" | "focus-next" => self.cycle_focus(id == "focus-next"),
            "equalize-splits" => self.equalize_splits(),
            "zoom-pane" => {
                if let Some(pane) = focused {
                    self.toggle_zoom(&pane);
                }
            }
            "switch-view" => {
                if let Some(pane) = focused {
                    self.switch_view(&pane);
                }
            }
            "pane-terminal" | "pane-browser" => {
                if let Some(pane) = focused {
                    self.ask_set_kind(&pane, if id == "pane-terminal" { NewPaneKind::Terminal } else { NewPaneKind::Browser });
                }
            }
            "pane-artifact" => {
                if let Some(pane) = focused {
                    self.open_palette(&pane);
                }
            }
            "previous-space" | "next-space" => self.cycle_space(if id == "next-space" { 1 } else { -1 }),
            "last-space" => self.show_space_at(self.session_spaces().len().saturating_sub(1)),
            other => {
                if let Some(dir) = other.strip_prefix("focus-").and_then(dir_named) {
                    self.move_focus(dir);
                } else if let Some(dir) = other.strip_prefix("resize-").and_then(dir_named) {
                    self.resize_focused(dir);
                } else if let Some(n) = other.strip_prefix("space-").and_then(|n| n.parse::<usize>().ok()) {
                    self.show_space_at(n - 1);
                } else if let Some(agent) = other.strip_prefix("pane-agent-") {
                    if let Some(pane) = focused {
                        self.ask_start_agent(&pane, agent);
                    }
                } else if let Some(name) = other.strip_prefix("run-") {
                    self.start_runner(name);
                }
            }
        }
    }
    /// Record an action, keeping the last 32, for a failing check to read.
    fn note_action(&mut self, what: String) {
        if self.actions_log.len() >= 32 {
            self.actions_log.remove(0);
        }
        self.actions_log.push(what);
    }

    // Panes

    /// ⌘[ and ⌘]: the pane before or after the focused one, wrapping.
    pub fn cycle_focus(&mut self, forward: bool) {
        let order: Vec<String> = self.scene().0.into_iter().map(|p| p.leaf.id).collect();
        if let Some(next) = self.focus.step(&order, forward) {
            self.focus_pane(&next);
        }
    }
    /// ⌥⌘ and an arrow: the pane beside the focused one that way, as it is drawn: the nearest
    /// pane across the gap whose span overlaps the focused one's on the other axis.
    pub fn move_focus(&mut self, dir: Dir) {
        let leaves = self.scene().0;
        let Some(from) = leaves.iter().find(|p| Some(&p.leaf.id) == self.focus.current.as_ref()).map(|p| p.rect) else { return };
        let overlaps = |r: &Rect| match dir {
            Dir::Left | Dir::Right => r.y < from.y + from.h && from.y < r.y + r.h,
            Dir::Up | Dir::Down => r.x < from.x + from.w && from.x < r.x + r.w,
        };
        let distance = |r: &Rect| -> Option<f32> {
            let d = match dir {
                Dir::Left => from.x - (r.x + r.w),
                Dir::Right => r.x - (from.x + from.w),
                Dir::Up => from.y - (r.y + r.h),
                Dir::Down => r.y - (from.y + from.h),
            };
            (d >= 0.).then_some(d)
        };
        let next = leaves.iter().filter(|p| Some(&p.leaf.id) != self.focus.current.as_ref() && overlaps(&p.rect))
            .filter_map(|p| distance(&p.rect).map(|d| (d, p.leaf.id.clone())))
            .min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, id)| id);
        if let Some(next) = next {
            self.focus_pane(&next);
        }
    }
    /// ⌃⌘ and an arrow: move the divider of the nearest split along that axis around the
    /// focused pane across that way by 5%.
    pub fn resize_focused(&mut self, dir: Dir) {
        let Some(pane) = self.focus.current.clone() else { return };
        let horizontal = matches!(dir, Dir::Left | Dir::Right);
        fn find(t: &Tree, pane: &str, horizontal: bool) -> Option<(String, f64)> {
            match t {
                Tree::Pane(_) => None,
                Tree::Split { id, horizontal: h, ratio, a, b } => {
                    let holds = |t: &Tree| leaves_of(t).iter().any(|l| l.id == pane);
                    let inner = if holds(a) { find(a, pane, horizontal) } else if holds(b) { find(b, pane, horizontal) } else { return None };
                    inner.or_else(|| (*h == horizontal).then(|| (id.clone(), *ratio)))
                }
            }
        }
        let Some((split, ratio)) = self.space().and_then(|s| find(&s.root, &pane, horizontal)) else { return };
        let step = if matches!(dir, Dir::Left | Dir::Up) { -0.05 } else { 0.05 };
        let ratio = (ratio + step).clamp(0.1, 0.9);
        self.send(&ClientMsg::SplitRatio { req: None, split, ratio });
    }
    /// ⌃⌘=: give every pane along each split the same share.
    pub fn equalize_splits(&mut self) {
        fn along(t: &Tree, horizontal: bool) -> usize {
            match t {
                Tree::Pane(_) => 1,
                Tree::Split { horizontal: h, a, b, .. } if *h == horizontal => along(a, horizontal) + along(b, horizontal),
                Tree::Split { .. } => 1,
            }
        }
        fn walk(t: &Tree, out: &mut Vec<(String, f64)>) {
            if let Tree::Split { id, horizontal, a, b, .. } = t {
                let (na, nb) = (along(a, *horizontal), along(b, *horizontal));
                out.push((id.clone(), na as f64 / (na + nb) as f64));
                walk(a, out);
                walk(b, out);
            }
        }
        let mut ratios = vec![];
        if let Some(space) = self.space() {
            walk(&space.root, &mut ratios);
        }
        for (split, ratio) in ratios {
            self.send(&ClientMsg::SplitRatio { req: None, split, ratio });
        }
    }

    // Spaces

    /// ⇧⌘[ and ⇧⌘]: the space before or after the shown one in this session, wrapping.
    pub fn cycle_space(&mut self, step: isize) {
        let held: Vec<String> = self.session_spaces().iter().map(|s| s.id.clone()).collect();
        let Some(i) = held.iter().position(|s| Some(s) == self.space_id.as_ref()) else { return };
        if held.len() > 1 {
            let n = held.len() as isize;
            self.show_space_at((((i as isize + step) % n + n) % n) as usize);
        }
    }
    /// ⌘1 to ⌘9: the session's space at `index`.
    pub fn show_space_at(&mut self, index: usize) {
        if let Some(space) = self.session_spaces().get(index).map(|s| s.id.clone()) {
            self.select_space(&space);
        }
    }
    /// Show `space`, switching to the session holding it when another does.
    pub(super) fn show_space_anywhere(&mut self, space: &str) {
        if !self.session_spaces().iter().any(|s| s.id == space)
            && let Some(session) = crate::windows::session_holding(&self.sessions, space).map(|s| s.id.clone())
        {
            self.select_session(&session);
        }
        self.select_space(space);
    }

    // Shortcut sheet

    /// ⌘/: show the shortcut sheet over the window, or hide it.
    pub fn toggle_shortcuts(&mut self) {
        self.shortcuts_open = !self.shortcuts_open;
        self.sync_overlay();
    }
    pub fn close_shortcuts(&mut self) {
        if self.shortcuts_open {
            self.shortcuts_open = false;
            self.sync_overlay();
        }
    }
    /// The shortcut sheet's groups, with the key the keymap gives each action now.
    pub fn shortcut_sheet(&self) -> Vec<(&'static str, Vec<actions::ShortcutRow>)> {
        actions::shortcut_sheet(&self.all_actions(), &self.keymap)
    }

    // Asking first

    pub(super) fn confirm(&mut self, title: &str, message: String, button: String, run: Pending) {
        self.confirmation = Some(Confirmation { title: title.to_owned(), message, button, run });
        self.sync_overlay();
    }
    /// The sheet's answer: run the step on a yes, drop it either way.
    pub fn answer_confirmation(&mut self, yes: bool) {
        let Some(pending) = self.confirmation.take() else { return };
        self.sync_overlay();
        if yes {
            self.run_pending(pending.run);
        }
    }
    fn leaf(&self, pane: &str) -> Option<Leaf> {
        self.spaces.iter().flat_map(|s| s.shown()).find(|l| l.id == pane).cloned()
    }
    /// The question a pane that runs a shell or an agent asks before it stops running it.
    fn ending_question(kind: &str) -> Option<(&'static str, &'static str)> {
        match kind {
            "terminal" => Some(("Kill this pane's shell?", "Whatever is running in it stops.")),
            "agent" => Some(("End this pane's agent?", "Its conversation and anything it is doing stop.")),
            _ => None,
        }
    }
    /// ⌘W: close `pane`, asking first when it runs a shell or an agent.
    pub fn ask_close_pane(&mut self, pane: &str) {
        let Some(leaf) = self.leaf(pane) else { return };
        match Self::ending_question(&leaf.kind) {
            Some((title, message)) => self.confirm(title, message.into(), "Close Pane".into(), Pending::ClosePane(pane.to_owned())),
            None => {
                let pane = pane.to_owned();
                self.send_layout(|req| ClientMsg::PaneClose { req: Some(req), pane });
            }
        }
    }
    /// ⌥⌘W: close `space`, asking first when it runs a shell or an agent.
    pub fn ask_close_space(&mut self, id: &str) {
        let Some(space) = self.spaces.iter().find(|s| s.id == id) else { return };
        let leaves: Vec<&Leaf> = space.all().collect();
        let shells = leaves.iter().filter(|l| l.kind == "terminal").count();
        let agents = leaves.iter().filter(|l| l.kind == "agent").count();
        if shells + agents == 0 {
            let id = id.to_owned();
            return self.send_layout(|req| ClientMsg::SpaceClose { req: Some(req), id, if_empty: false });
        }
        let mut parts = vec![];
        if shells > 0 {
            parts.push(if shells == 1 { "its terminal's shell is killed".to_owned() } else { format!("its {shells} terminals' shells are killed") });
        }
        if agents > 0 {
            parts.push(if agents == 1 { "its agent ends".to_owned() } else { format!("its {agents} agents end") });
        }
        let text = capitalized(&parts.join(" and ")) + ".";
        let title = format!("Close “{}”?", space.name);
        self.confirm(&title, text, "Close Space".into(), Pending::CloseSpace(id.to_owned()));
    }
    /// Show a terminal or a browser in `pane`, asking first when that ends what it runs.
    pub fn ask_set_kind(&mut self, pane: &str, new: NewPaneKind) {
        let Some(leaf) = self.leaf(pane) else { return };
        let Some((title, message)) = Self::ending_question(&leaf.kind) else { return self.set_kind(pane, new) };
        let (shows, button) = match new {
            NewPaneKind::Terminal => ("shows a terminal", "Show Terminal"),
            NewPaneKind::Browser => ("shows a browser", "Show Browser"),
            NewPaneKind::Empty => ("is empty", "Empty Pane"),
            NewPaneKind::Web => ("shows an artifact", "Show Artifact"),
            NewPaneKind::Plugin => ("shows a plugin pane", "Show Plugin Pane"),
        };
        self.confirm(title, format!("{message} The pane then {shows}."), button.into(), Pending::SetKind(pane.to_owned(), new));
    }
    /// Start agent `agent` in `pane`, asking first when that ends what it runs.
    pub fn ask_start_agent(&mut self, pane: &str, agent: &str) {
        let Some(leaf) = self.leaf(pane) else { return };
        let run = Pending::StartAgent(pane.to_owned(), agent.to_owned());
        let Some((title, message)) = Self::ending_question(&leaf.kind) else {
            return self.run_pending(run);
        };
        let name = capitalized(agent);
        self.confirm(title, format!("{message} The pane then runs {name}."), format!("Start {name}"), run);
    }
    /// Settings › Artifacts › Remove asks first: the folder goes, and with it any edit made there.
    pub fn ask_remove_builtin(&mut self, name: &str, edited: &[String]) {
        let message = if edited.is_empty() {
            "Its folder and its entry go. Panes showing it show nothing until it is installed again.".to_owned()
        } else {
            format!("Its folder goes, with your edits to {}.", edited.join(", "))
        };
        self.confirm(&format!("Remove “{name}”?"), message, "Remove".into(), Pending::RemoveBuiltin(name.to_owned()));
    }
    fn run_pending(&mut self, run: Pending) {
        match run {
            Pending::ClosePane(pane) => self.send_layout(|req| ClientMsg::PaneClose { req: Some(req), pane }),
            Pending::CloseSpace(id) => self.send_layout(|req| ClientMsg::SpaceClose { req: Some(req), id, if_empty: false }),
            Pending::CloseSpaces(ids) => {
                for id in ids {
                    self.send_layout(|req| ClientMsg::SpaceClose { req: Some(req), id, if_empty: false });
                }
            }
            Pending::SetKind(pane, kind) => self.set_kind(&pane, kind),
            Pending::StartAgent(pane, agent) => self.send(&ClientMsg::PaneStart { req: None, pane, kind: StartKind::Agent, message: None, agent: Some(agent), model: None, permission_mode: None, sandbox: None }),
            Pending::RemoveBuiltin(name) => self.remove_builtin(&name),
            Pending::CloseSession(session) => self.send_layout(|req| ClientMsg::SessionClose { req: Some(req), session }),
        }
    }

    // The command palette

    pub fn open_command_palette(&mut self, mode: Mode) {
        if self.command_palette.is_some() {
            return;
        }
        self.command_palette = Some(CommandPalette::new(mode));
        self.menu = None;
        self.refresh_command_palette();
        self.ask_recent_dirs();
        self.ask_runners();
        self.ask_palette_artifacts();
        if mode == Mode::NewSpace {
            self.ask_roots();
        }
        self.sync_overlay();
    }
    pub fn close_command_palette(&mut self) {
        if self.command_palette.take().is_none() {
            return;
        }
        // The listings' late answers still land, so the next opening starts from them.
        self.palette_reqs.roots = None;
        self.palette_reqs.dirs.clear();
        self.sync_overlay();
    }
    /// The `+` and ⌘T: the palette in New Space mode, switching one already open into it.
    pub(super) fn open_new_space_palette(&mut self) {
        match &self.command_palette {
            None => self.open_command_palette(Mode::NewSpace),
            Some(p) if p.mode != Mode::NewSpace => self.enter_new_space_mode(),
            _ => {}
        }
    }
    fn enter_new_space_mode(&mut self) {
        if let Some(p) = &mut self.command_palette {
            p.enter_new_space();
        }
        self.ask_roots();
    }
    pub(super) fn req(&mut self) -> u64 {
        let req = self.next_req;
        self.next_req += 1;
        req
    }
    fn ask_recent_dirs(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.req();
        self.palette_reqs.recent = Some(req);
        self.send(&ClientMsg::SpaceRecent { req: Some(req) });
    }
    /// Ask for the shown space's `[runner]` commands; another space's never stand in while it answers.
    pub fn ask_runners(&mut self) {
        let (true, Some(space)) = (self.link == Link::Up, self.space_id.clone()) else { return };
        let req = self.req();
        self.palette_reqs.runners = Some(req);
        self.send(&ClientMsg::RunnerLs { req: Some(req), space: Some(space) });
    }
    fn ask_palette_artifacts(&mut self) {
        self.palette_artifacts.clear();
        let (true, Some(space)) = (self.link == Link::Up, self.space_id.clone()) else { return };
        let req = self.req();
        self.palette_reqs.artifacts = Some(req);
        self.send(&ClientMsg::ArtifactLs { req: Some(req), space: Some(space), pane: true });
    }
    fn ask_roots(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.req();
        self.palette_reqs.roots = Some(req);
        self.send(&ClientMsg::DirRoots { req: Some(req) });
    }
    fn ask_dir_list(&mut self, path: Option<String>) {
        let (Some(path), true) = (path, self.link == Link::Up) else { return };
        let req = self.req();
        self.palette_reqs.dirs.insert(req, path.clone());
        self.send(&ClientMsg::DirList { req: Some(req), path });
    }
    /// A reply to one of the palette's requests. Returns false when `req` is none of its.
    pub fn on_palette_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::SpaceRecent { req, dirs } if self.palette_reqs.recent == Some(*req) => {
                self.recent_dirs = dirs.clone();
                self.refresh_command_palette();
            }
            ServerMsg::Runners { req, runners } if self.palette_reqs.runners == Some(*req) => {
                self.runners = runners.iter().map(|r| r.name.clone()).collect();
                self.runners_space = self.space_id.clone();
                self.refresh_command_palette();
            }
            ServerMsg::Artifacts { req, artifacts, .. } if self.palette_reqs.artifacts == Some(*req) => {
                self.palette_artifacts = artifacts.clone();
                self.refresh_command_palette();
            }
            ServerMsg::DirRoots { req, roots } if self.palette_reqs.roots == Some(*req) => {
                let roots = roots.clone();
                if let Some(p) = &mut self.command_palette {
                    p.set_roots(roots.clone());
                }
                for root in roots {
                    self.ask_dir_list(Some(root));
                }
            }
            ServerMsg::DirList { req, dirs, error, .. } if self.palette_reqs.dirs.contains_key(req) => {
                let path = self.palette_reqs.dirs.remove(req).unwrap_or_default();
                if let Some(p) = &mut self.command_palette {
                    p.listed(&path, crate::command_palette::Listing { dirs: dirs.clone(), error: error.clone() });
                }
            }
            ServerMsg::Created { req, id, .. } if self.palette_reqs.artifact_after_split.contains_key(req) => {
                let info = self.palette_reqs.artifact_after_split.remove(req).unwrap();
                self.send(&ClientMsg::PaneSetKind { req: None, pane: id.clone(), kind: NewPaneKind::Web, artifact: Some(info.name), scope: Some(info.scope), url: None, plugin: None });
            }
            _ => return false,
        }
        true
    }
    /// List the palette again from the layout, the recent directories and the actions as they are now.
    pub fn refresh_command_palette(&mut self) {
        if self.command_palette.is_none() {
            return;
        }
        let spaces: Vec<OpenSpace> = self.spaces.iter().map(|s| OpenSpace { id: s.id.clone(), name: s.name.clone(), dir: s.cwd.clone() }).collect();
        let panes: Vec<OpenPane> = self.spaces.iter().flat_map(|s| leaves_of(&s.root).into_iter().map(|l| OpenPane { id: l.id.clone(), space: s.id.clone(), space_name: s.name.clone(), name: palette_name(l) })).collect();
        let actions: Vec<PaletteAction> = self.all_actions().into_iter().chain(actions::runner_actions(&self.runners))
            .filter(|a| self.action_enabled(&a.id))
            .map(|a| PaletteAction { shortcut: self.keymap.symbol(&a.id), id: a.id, title: a.title })
            .collect();
        let (artifacts, recents) = (self.palette_artifacts.clone(), self.recent_dirs.clone());
        if let Some(p) = &mut self.command_palette {
            p.list(spaces, panes, artifacts, recents, actions);
        }
    }
    /// A key typed while the palette is open. Returns false when it is closed.
    pub fn command_palette_key(&mut self, key: &str, text: Option<&str>, chord: bool, cx: &mut App) -> bool {
        let Some(palette) = &mut self.command_palette else { return false };
        match key {
            "escape" => {
                if palette.is_staged() {
                    palette.leave_stage();
                } else {
                    self.close_command_palette();
                }
            }
            "up" => palette.move_by(-1),
            "down" => palette.move_by(1),
            "enter" => self.command_palette_run(cx),
            "tab" => {
                let before = palette.browsed_parent();
                palette.tab();
                let after = palette.browsed_parent();
                if after != before {
                    self.ask_dir_list(after);
                }
            }
            "backspace" => {
                if palette.query.is_empty() {
                    if palette.is_staged() {
                        palette.leave_stage();
                    }
                } else {
                    let mut query = palette.query.clone();
                    query.pop();
                    self.command_palette_typed(&query);
                }
            }
            _ => {
                if !chord && let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                    let query = format!("{}{t}", palette.query);
                    self.command_palette_typed(&query);
                }
            }
        }
        true
    }
    pub fn command_palette_typed(&mut self, text: &str) {
        let Some(palette) = &mut self.command_palette else { return };
        let before = palette.browsed_parent();
        palette.type_text(text);
        let after = palette.browsed_parent();
        if after != before {
            self.ask_dir_list(after);
        }
    }
    pub fn command_palette_select(&mut self, index: usize) {
        if let Some(p) = &mut self.command_palette {
            p.select(index);
        }
    }
    /// Return: close the palette and run the selected row.
    pub fn command_palette_run(&mut self, cx: &mut App) {
        let Some(item) = self.command_palette.as_ref().and_then(|p| p.selected_item().cloned()) else { return };
        // These keep the palette open: one changes its mode, one its stage, the other only explains.
        match &item {
            Item::NewSpaceMode => return self.enter_new_space_mode(),
            Item::Verb(verb) => {
                if let Some(p) = &mut self.command_palette {
                    p.enter(*verb);
                }
                return;
            }
            Item::Missing { .. } => return,
            _ => {}
        }
        self.close_command_palette();
        self.note_action(format!("palette: run {} {}", item.kind(), item.id()));
        match item {
            Item::Action { id, .. } => self.run_action(&id, cx),
            Item::OpenFolder { .. } => self.run_action("open-folder", cx),
            Item::Space { id, .. } => self.show_space_anywhere(&id),
            Item::Pane { id, space, .. } => {
                self.show_space_anywhere(&space);
                self.focus_pane(&id);
            }
            Item::Artifact { scope, name, .. } => {
                let Some(info) = self.palette_artifacts.iter().find(|a| a.scope == scope && a.name == name).cloned() else { return };
                let Some(pane) = self.focus.current.clone() else { return };
                let req = self.req();
                self.palette_reqs.artifact_after_split.insert(req, info);
                let split = ClientMsg::PaneSplit { req: Some(req), pane, dir: Some(Dir::Right), focus: false, site: None };
                self.unzoom_for(&split);
                self.send(&split);
            }
            Item::Recent { dir } | Item::NewSpaceHere { dir, .. } | Item::Folder { path: dir, .. } => self.create_space(&dir),
            Item::Target { verb: Verb::CloseSpace, id, .. } => self.ask_close_space(&id),
            Item::Target { verb: Verb::ClosePane, id, .. } => self.ask_close_pane(&id),
            Item::Target { .. } | Item::NewSpaceMode | Item::Verb(_) | Item::Missing { .. } => {}
        }
    }
    /// Make a space for `dir` in the session shown, and show it.
    pub fn create_space(&mut self, dir: &str) {
        self.send(&ClientMsg::SpaceCreate { req: None, cwd: dir.to_owned(), name: None, focus: true, background: false, pane: None, session: self.session_id.clone() });
    }
    /// Run `[runner]` command `name` of the shown space in a new terminal pane, as `wheel run` does.
    fn start_runner(&mut self, name: &str) {
        let Some(space) = self.space_id.clone() else { return };
        self.send(&ClientMsg::Run { req: None, name: name.to_owned(), space: Some(space), focus: false });
    }
    /// Tell the bridge where the window-wide overlay is, so the native views leave it a hole and
    /// the GPUI view takes the keyboard while one is open.
    pub(super) fn sync_overlay(&mut self) {
        let open = self.name_edit.is_some() || self.command_palette.is_some() || self.shortcuts_open || self.settings_open || self.confirmation.is_some() || self.pairing.open || self.procs.open.is_some() || self.conversations.open.is_some() || self.pane_info.is_some() || self.usage_open || self.title_menu.is_some();
        if open {
            let (w, h) = self.viewport;
            self.native.call(serde_json::json!({"op": "overlay", "open": true, "rect": [0., 0., f64::from(w), f64::from(h)]}));
        } else {
            self.native.call(serde_json::json!({"op": "overlay", "open": false}));
            if let Some(pane) = self.focus.current.clone() {
                self.native.focus(if self.native_panes.contains(&pane) { &pane } else { "" });
            }
        }
    }
    /// A key while an overlay other than the palette is open: the sheet takes Esc and its own key,
    /// the confirmation Return and Esc. Returns whether the key was taken.
    pub fn overlay_key(&mut self, key: &str) -> bool {
        if self.confirmation.is_some() {
            match key {
                "enter" => self.answer_confirmation(true),
                "escape" => self.answer_confirmation(false),
                _ => {}
            }
            return true;
        }
        if self.shortcuts_open {
            if key == "escape" {
                self.close_shortcuts();
            }
            return true;
        }
        false
    }
}

pub(super) fn dir_named(name: &str) -> Option<Dir> {
    match name {
        "left" => Some(Dir::Left),
        "right" => Some(Dir::Right),
        "up" => Some(Dir::Up),
        "down" => Some(Dir::Down),
        _ => None,
    }
}

/// The command palette over the window: the window dims, and a search field above the matching
/// rows sits near its top (CommandPaletteView.swift).
pub fn command_palette_overlay(palette: &CommandPalette, width: f32, height: f32, cx: &mut Context<Client>) -> gpui::Div {
    let card_w = 560f32.min(width - 32.);
    let field: gpui::Div = if palette.query.is_empty() {
        div().text_color(rgb(0x6f8a7d)).child(palette.placeholder())
    } else {
        div().text_color(rgb(0xd9e8e1)).child(palette.query.clone())
    };
    let mut card = div().w(px(card_w)).max_h(px((height - 64.).max(100.))).flex().flex_col().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(0x3d5247))
        .text_color(rgb(0xd9e8e1)).overflow_hidden()
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .child(div().h(px(40.)).px_3().flex().items_center().gap_2().border_b_1().border_color(rgb(0x2c3d34))
            .child(div().text_color(rgb(0x6f8a7d)).child("⌕")).child(field.flex_1().overflow_hidden().whitespace_nowrap()));
    if palette.rows.is_empty() {
        card = card.child(div().min_h(px(60.)).flex().items_center().justify_center().text_color(rgb(0x8fa89b)).child(format!("Nothing matches “{}”", palette.query)));
    } else {
        let mut list = div().id("command-palette-rows").overflow_y_scroll().max_h(px(320.)).flex().flex_col().p_1();
        for (index, row) in palette.rows.iter().enumerate() {
            let selected = index == palette.selected;
            let detail = row.item.detail();
            let mut r = div().h(px(28.)).px_2().rounded_md().cursor_pointer().flex().items_center().gap_2()
                .bg(rgb(if selected { 0x31483e } else { 0x17201c }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.command_palette_select(index);
                    this.command_palette_run(cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(div().w(px(18.)).flex_none().text_color(rgb(0x6f8a7d)).child(icon(&row.item)))
                .child(div().overflow_hidden().whitespace_nowrap().child(row.item.title()));
            if !detail.is_empty() {
                r = r.child(div().flex_1()).child(div().text_xs().text_color(rgb(0x8fa89b)).overflow_hidden().whitespace_nowrap().child(detail));
            }
            list = list.child(r);
        }
        card = card.child(list);
    }
    div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_start().pt(px(48.))
        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
            this.close_command_palette();
            cx.stop_propagation();
            cx.notify();
        }))
        .child(card)
}
/// A glyph per row kind, standing in for the Swift app's SF Symbols.
fn icon(item: &Item) -> &'static str {
    match item {
        Item::Action { .. } => "⌘",
        Item::Space { .. } => "▦",
        Item::NewSpaceHere { .. } | Item::NewSpaceMode | Item::Recent { .. } => "+",
        Item::Pane { .. } => "▥",
        Item::Artifact { .. } => "▤",
        Item::OpenFolder { .. } | Item::Folder { .. } => "▭",
        Item::Verb(_) => "›",
        Item::Target { .. } => "✕",
        Item::Missing { .. } => "⚠",
    }
}

/// The shortcut sheet (⌘/) over the window: every action that has keys, by group, each with the
/// key the keymap gives it now, in three columns: Panes, Spaces, then Windows over Agent.
pub fn shortcut_sheet_overlay(groups: &[(&'static str, Vec<actions::ShortcutRow>)], width: f32, height: f32, cx: &mut Context<Client>) -> gpui::Div {
    let group_view = |group: &(&'static str, Vec<actions::ShortcutRow>)| {
        let mut g = div().flex().flex_col().gap(px(2.)).child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0x8fa89b)).child(group.0));
        for row in &group.1 {
            g = g.child(div().flex().justify_between().gap_2().py(px(1.))
                .child(div().overflow_hidden().whitespace_nowrap().child(row.title.clone()))
                .child(div().font_family("Menlo").text_color(rgb(0x8fa89b)).child(row.keys.clone())));
        }
        g
    };
    let mut columns = div().flex().items_start().gap(px(24.)).p_3();
    for column in [vec![0usize], vec![1], vec![2, 3]] {
        let mut col = div().flex_1().flex().flex_col().gap(px(14.));
        for index in column {
            if let Some(group) = groups.get(index) {
                col = col.child(group_view(group));
            }
        }
        columns = columns.child(col);
    }
    let card = div().w(px(820f32.min(width - 32.))).max_h(px(520f32.min(height - 32.))).flex().flex_col().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(0x3d5247))
        .text_color(rgb(0xd9e8e1)).overflow_hidden()
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .child(div().p_3().font_weight(FontWeight::SEMIBOLD).border_b_1().border_color(rgb(0x2c3d34)).child("Keyboard Shortcuts"))
        .child(div().id("shortcut-sheet").overflow_y_scroll().flex_1().min_h_0().child(columns));
    div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_center()
        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
            this.close_shortcuts();
            cx.stop_propagation();
            cx.notify();
        }))
        .child(card)
}

/// A confirmation sheet: the question, what happens, and Cancel beside the step's button.
pub fn confirmation_overlay(c: &Confirmation, width: f32, height: f32, cx: &mut Context<Client>) -> gpui::Div {
    let button = |label: String, yes: bool, cx: &mut Context<Client>| {
        div().px_3().py_1().rounded_sm().cursor_pointer().bg(rgb(if yes { 0x31483e } else { 0x22302a }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.answer_confirmation(yes);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(label)
    };
    let card = div().w(px(380f32.min(width - 32.))).flex().flex_col().gap_2().p_4().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(0x3d5247)).text_color(rgb(0xd9e8e1))
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(c.title.clone()))
        .child(div().text_xs().text_color(rgb(0x8fa89b)).child(c.message.clone()))
        .child(div().flex().justify_end().gap_2().mt_2().child(button("Cancel".into(), false, cx)).child(button(c.button.clone(), true, cx)));
    div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_center()
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .child(card)
}
