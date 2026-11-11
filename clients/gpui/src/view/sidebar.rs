//! The spaces sidebar as drawn and driven (SpacesSidebar.swift, SpaceMenu.swift, SpaceDrag.swift,
//! SpacePreview.swift): a row per space of the session shown with its pane stack, its agents'
//! marks, its name and its waiting count, a close button; a press shows the space and a press
//! that moves drags it; a right press opens its menu; a pane's header dragged onto a row moves
//! the pane there; a pointer held over a row opens a preview of its grid.
use super::*;
use super::settings::now_ms;
use crate::sidebar::{self, MenuItem, PaneAway, PaneDrag, Rename, Row, SpaceDrag, SpaceDropTarget};
use std::path::Path;

const ACCENT: u32 = 0x7fb89a;
const ROW_BG_ACTIVE: u32 = 0x31483e;
const CARD_BG: u32 = 0x17201c;
const CARD_BORDER: u32 = 0x2c3d34;
const DIM: u32 = 0x8fa89b;
const RED: u32 = 0xe07070;

impl Client {
    /// The sidebar's rectangle: under the title row, down the left edge, over the bottom row.
    pub fn sidebar_rect(&self) -> Rect {
        Rect { x: header::MARGIN, y: HEADER, w: sidebar::WIDTH, h: (self.viewport.1 - HEADER - BOTTOM).max(0.) }
    }
    /// The rows of the session shown, as drawn.
    pub fn sidebar_rows(&self) -> Vec<Row> {
        sidebar::rows(&self.session_ids(), self.sidebar_rect())
    }
    /// Where a space's row or chip is, for a check that presses it.
    pub fn space_hit_rect(&self, space: &str) -> Option<Rect> {
        if self.sidebar_open {
            self.sidebar_rows().into_iter().find(|r| r.id == space).map(|r| r.rect)
        } else {
            self.header_chips().into_iter().find(|c| c.hit == Hit::Space(space.to_owned())).map(|c| c.rect)
        }
    }
    /// The area the grid is placed in: right of the sidebar while it is open.
    pub fn grid_area(&self) -> Rect {
        let left = if self.sidebar_open { header::MARGIN + sidebar::WIDTH + sidebar::GAP } else { 0. };
        Rect { x: left, y: HEADER, w: (self.viewport.0 - left).max(0.), h: (self.viewport.1 - HEADER - BOTTOM).max(0.) }
    }
    pub fn set_sidebar(&mut self, open: bool) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            self.log(format!("sidebar: {}", if open { "shown" } else { "hidden" }));
            self.cancel_space_drag(false);
            self.cancel_pane_drag();
            self.close_space_menu();
            self.close_preview();
        }
    }
    pub fn toggle_sidebar(&mut self) {
        self.set_sidebar(!self.sidebar_open);
    }
    pub fn space_name(&self, id: &str) -> Option<String> {
        self.spaces.iter().find(|s| s.id == id).map(|s| s.name.clone())
    }
    fn session_ids(&self) -> Vec<String> {
        self.session_spaces().iter().map(|s| s.id.clone()).collect()
    }

    // MARK: Presses and drags

    /// A press on a space's row: the space shows at once, as a sidebar selects, and a press that
    /// then moves `DRAG_DISTANCE` drags it.
    pub fn space_press_down(&mut self, space: &str, x: f32, y: f32) {
        if self.space_drag.is_some() {
            return;
        }
        self.press(format!("space row {space} at ({x:.0},{y:.0})"));
        self.close_space_menu();
        if self.rename.as_ref().is_some_and(|r| r.space != space) {
            self.cancel_rename();
        }
        self.select_space(space);
        self.space_press = Some((space.to_owned(), x, y));
    }
    /// A press on a pane's header, which may become a pane drag.
    pub fn pane_press_down(&mut self, pane: &str, label: &str, x: f32, y: f32, grab: (f32, f32)) {
        if self.pane_drag.is_none() {
            self.pane_press = Some((pane.to_owned(), label.to_owned(), x, y, grab));
        }
    }
    /// The pointer moved to (`x`, `y`): a held press becomes a drag, a drag finds its target, and
    /// the row under the pointer is the one the preview follows.
    pub fn pointer_moved(&mut self, x: f32, y: f32, pressed: bool) {
        // A release GPUI never saw (a native view took it) leaves no press armed.
        if !pressed {
            self.space_press = None;
            self.pane_press = None;
        }
        if let Some((space, px0, py0)) = self.space_press.clone()
            && self.space_drag.is_none()
            && ((x - px0).powi(2) + (y - py0).powi(2)).sqrt() >= sidebar::DRAG_DISTANCE
        {
            self.begin_space_drag(&space, true, (x, y));
        }
        if let Some((pane, label, px0, py0, grab)) = self.pane_press.clone()
            && self.pane_drag.is_none()
            && ((x - px0).powi(2) + (y - py0).powi(2)).sqrt() >= sidebar::DRAG_DISTANCE
        {
            self.begin_pane_drag(&pane, &label, (x, y), grab);
        }
        if self.space_drag.is_some() {
            self.move_space_drag(x, y);
        }
        if self.pane_drag.is_some() {
            self.move_pane_drag(x, y);
        }
        let over = if self.sidebar_open && self.space_drag.is_none() && self.pane_drag.is_none() && self.space_menu.is_none() && self.rename.is_none() {
            sidebar::row_at(&self.sidebar_rows(), x, y).map(|r| r.id.clone())
        } else {
            None
        };
        self.row_hover(over);
    }
    /// The button came up: a drag drops, a press that never moved is over.
    pub fn pointer_up(&mut self) {
        self.space_press = None;
        self.pane_press = None;
        if self.space_drag.as_ref().is_some_and(|d| d.by_pointer) {
            self.end_space_drag();
        }
        if self.pane_drag.is_some() {
            self.end_pane_drag();
        }
    }
    pub fn begin_space_drag(&mut self, space: &str, by_pointer: bool, at: (f32, f32)) {
        if self.space_drag.is_some() || !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.close_preview();
        self.space_drag = Some(SpaceDrag { space: space.to_owned(), by_pointer, target: None, at });
        self.log(format!("space drag {space}: started"));
    }
    pub fn move_space_drag(&mut self, x: f32, y: f32) {
        let target = if self.sidebar_open { sidebar::space_target(self.sidebar_rect(), &self.sidebar_rows(), x, y) } else { SpaceDropTarget::Window };
        if let Some(drag) = &mut self.space_drag {
            drag.at = (x, y);
        }
        self.hover_space_drag(Some(target));
    }
    /// Name the target directly, as the debug route does without a pointer.
    pub fn hover_space_drag(&mut self, target: Option<SpaceDropTarget>) {
        let Some(drag) = &mut self.space_drag else { return };
        if drag.target != target {
            drag.target = target;
            let (space, words) = (drag.space.clone(), drag.target.as_ref().map(describe_target).unwrap_or_else(|| "nothing".into()));
            self.log(format!("space drag {space}: over {words}"));
        }
    }
    /// Release: put the space where the pointer is. The layout changes once wheeld answers.
    pub fn end_space_drag(&mut self) {
        let Some(drag) = self.space_drag.take() else { return };
        self.space_press = None;
        let Some(target) = drag.target else {
            self.log(format!("space drag {}: released over nothing", drag.space));
            return;
        };
        self.log(format!("space drag {}: dropped on {}", drag.space, describe_target(&target)));
        let Some(session) = self.session_id.clone() else { return };
        let held = self.session_ids();
        let space = drag.space;
        match target {
            SpaceDropTarget::Sidebar(index) => {
                // A drop beside the space's own slot changes nothing.
                if sidebar::bar_index(index, &held, &space).is_none() {
                    return;
                }
                let index = sidebar::move_index(index, &held, &space);
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index: Some(index) });
            }
            SpaceDropTarget::Window => {
                if held.last() == Some(&space) {
                    return;
                }
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index: None });
            }
        }
    }
    pub fn cancel_space_drag(&mut self, escape: bool) {
        self.space_press = None;
        if let Some(drag) = self.space_drag.take() {
            self.log(format!("space drag {}: cancelled{}", drag.space, if escape { " by escape" } else { "" }));
        }
    }
    /// The insertion bar's place, if one is drawn: none beside the dragged row itself.
    pub fn space_drag_insertion(&self) -> Option<usize> {
        let drag = self.space_drag.as_ref()?;
        match drag.target.as_ref()? {
            SpaceDropTarget::Sidebar(index) => sidebar::bar_index(*index, &self.session_ids(), &drag.space),
            SpaceDropTarget::Window => None,
        }
    }
    pub fn space_drag_hint(&self) -> Option<String> {
        let drag = self.space_drag.as_ref()?;
        let name = self.space_name(&drag.space).unwrap_or_else(|| drag.space.clone());
        let window = self.sessions.iter().find(|s| Some(&s.id) == self.session_id.as_ref()).map(|s| s.name.clone()).unwrap_or_default();
        Some(sidebar::space_hint(drag.target.as_ref()?, &name, &window))
    }

    pub fn begin_pane_drag(&mut self, pane: &str, label: &str, at: (f32, f32), grab: (f32, f32)) {
        if self.pane_drag.is_some() {
            return;
        }
        self.close_preview();
        self.pane_drag = Some(PaneDrag { pane: pane.to_owned(), label: label.to_owned(), away: None, at, grab });
        self.log(format!("pane drag {pane}: started"));
    }
    pub fn move_pane_drag(&mut self, x: f32, y: f32) {
        let away = if self.sidebar_open { sidebar::pane_away(self.sidebar_rect(), &self.sidebar_rows(), x, y) } else { None };
        if let Some(drag) = &mut self.pane_drag {
            drag.at = (x, y);
        }
        self.hover_pane_drag(away);
    }
    pub fn hover_pane_drag(&mut self, away: Option<PaneAway>) {
        let Some(drag) = &mut self.pane_drag else { return };
        if drag.away != away {
            drag.away = away;
            let (pane, words) = (drag.pane.clone(), drag.away.as_ref().map(describe_away).unwrap_or_else(|| "nothing".into()));
            self.log(format!("pane drag {pane}: over {words}"));
        }
    }
    pub fn end_pane_drag(&mut self) {
        let Some(drag) = self.pane_drag.take() else { return };
        self.pane_press = None;
        let Some(away) = drag.away else {
            self.log(format!("pane drag {}: released over nothing", drag.pane));
            return;
        };
        self.log(format!("pane drag {}: dropped on {}", drag.pane, describe_away(&away)));
        let pane = drag.pane;
        let to = match away {
            PaneAway::End(space) => wheel_proto::MoveTarget::End { space },
            PaneAway::NewSpace => wheel_proto::MoveTarget::NewSpace { name: None, focus: false },
        };
        self.send_layout(|req| ClientMsg::PaneMove { req: Some(req), pane, to });
    }
    pub fn cancel_pane_drag(&mut self) {
        self.pane_press = None;
        if let Some(drag) = self.pane_drag.take() {
            self.log(format!("pane drag {}: cancelled", drag.pane));
        }
    }
    pub fn pane_drag_hint(&self) -> Option<String> {
        let drag = self.pane_drag.as_ref()?;
        Some(sidebar::pane_hint(drag.away.as_ref()?, &drag.label, |id| self.space_name(id)))
    }
    /// Whether the dragged pane is over `space`'s row, which draws outlined.
    fn pane_is_over(&self, space: &str) -> bool {
        matches!(self.pane_drag.as_ref().and_then(|d| d.away.as_ref()), Some(PaneAway::End(s)) if s == space)
    }
    fn pane_is_over_new_space(&self) -> bool {
        matches!(self.pane_drag.as_ref().and_then(|d| d.away.as_ref()), Some(PaneAway::NewSpace))
    }
    /// A pane's name for the chip that follows the pointer (AppModel.paneLabel).
    pub fn pane_label(&self, leaf: &Leaf) -> String {
        let capitalized = |s: &str| { let mut c = s.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
        match leaf.kind.as_str() {
            "terminal" => "Terminal".into(),
            "agent" => capitalized(leaf.harness.as_deref().unwrap_or("agent")),
            "web" => leaf.label.clone(),
            "browser" => "Browser".into(),
            "empty" => "Empty pane".into(),
            _ => leaf.id.clone(),
        }
    }

    // MARK: The menu

    pub fn space_menu_items(&self, space: &str) -> Vec<MenuItem> {
        let home = self.sessions.iter().find(|s| s.spaces.iter().any(|id| id == space));
        let sessions: Vec<(String, String)> = self.sessions.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        sidebar::menu(space, home.map(|s| s.id.as_str()), &sessions, home.map(|s| s.spaces.as_slice()).unwrap_or(&[]))
    }
    pub fn open_space_menu(&mut self, space: &str, x: f32, y: f32) {
        if !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.close_preview();
        // The menu stays inside the window: it is moved up when it would run off the bottom.
        let h = self.menu_height(space);
        let y = y.min(self.viewport.1 - h - 4.).max(0.);
        self.space_menu = Some((space.to_owned(), x, y));
        self.sync_sidebar_overlay();
    }
    /// The menu's height as drawn: its rows, a line per section break, the padding.
    fn menu_height(&self, space: &str) -> f32 {
        let items = self.space_menu_items(space);
        let dividers = items.windows(2).filter(|w| w[0].section != w[1].section).count() as f32;
        items.len() as f32 * sidebar::MENU_ROW_H + dividers * 7. + 8.
    }
    pub fn close_space_menu(&mut self) {
        if self.space_menu.take().is_some() {
            self.sync_sidebar_overlay();
        }
    }
    /// Run a space menu item, as the menu and the debug route do (AppModel.runSpaceMenu).
    pub fn run_space_menu(&mut self, id: &str, space: &str, cx: &mut App) {
        self.close_space_menu();
        let Some(info) = self.spaces.iter().find(|s| s.id == space).cloned() else { return };
        if !self.space_menu_items(space).iter().any(|i| i.id == id && i.enabled) {
            self.log(format!("space menu {space}: {id} not available"));
            return;
        }
        self.log(format!("space menu {space}: {id}"));
        let home = self.sessions.iter().find(|s| s.spaces.iter().any(|i| i == space)).cloned();
        let space_id = space.to_owned();
        match id {
            "new-session" => {
                let taken: Vec<&str> = self.sessions.iter().map(|s| s.name.as_str()).collect();
                let name = header::session_name(&taken);
                let req = self.req();
                self.layout_reqs.insert(req);
                self.space_for_new_session = Some((req, space_id));
                self.send(&ClientMsg::SessionNew { req: Some(req), name });
            }
            "left" | "right" => {
                let Some(home) = home else { return };
                let Some(i) = home.spaces.iter().position(|s| s == space) else { return };
                let index = if id == "left" { i - 1 } else { i + 1 };
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space: space_id, session: home.id, index: Some(index) });
            }
            "rename" => self.begin_rename(space),
            "copy-path" => cx.write_to_clipboard(ClipboardItem::new_string(info.cwd.clone())),
            "reveal" => cx.reveal_path(Path::new(&info.cwd)),
            "new-pane" => {
                // The space's focused pane while it is shown, else its first.
                let first = leaves_of(&info.root).first().map(|l| l.id.clone());
                let pane = self.focus.current.clone().filter(|p| self.space_id.as_deref() == Some(space) && leaves_of(&info.root).iter().any(|l| &l.id == p)).or(first);
                if let Some(pane) = pane {
                    self.send_layout(|req| ClientMsg::PaneSplit { req: Some(req), pane, dir: None, focus: false, site: None });
                }
            }
            "close-others" => self.ask_close_other_spaces(space),
            "close" => self.ask_close_space(space),
            other => {
                if let Some(session) = other.strip_prefix("session:") {
                    let session = session.to_owned();
                    self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space: space_id, session, index: None });
                }
            }
        }
    }
    /// Close every space the window holds but `space`, after asking (askCloseOtherSpaces).
    fn ask_close_other_spaces(&mut self, space: &str) {
        let others: Vec<String> = self.session_ids().into_iter().filter(|id| id != space).collect();
        if others.is_empty() {
            return;
        }
        let names: Vec<String> = others.iter().filter_map(|id| self.space_name(id)).map(|n| format!("“{n}”")).collect();
        let count = if others.len() == 1 { "1 other space".to_owned() } else { format!("{} other spaces", others.len()) };
        let button = if others.len() == 1 { "Close Other Space" } else { "Close Other Spaces" };
        self.confirm(&format!("Close {count} in this window?"), format!("Every terminal and agent in {} stops.", names.join(", ")), button.into(), Pending::CloseSpaces(others));
    }

    // MARK: Renaming

    pub fn begin_rename(&mut self, space: &str) {
        let Some(name) = self.space_name(space) else { return };
        self.close_space_menu();
        self.rename = Some(Rename { space: space.to_owned(), draft: name, error: None, req: None });
    }
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }
    /// Return commits, Escape cancels, the rest edits the draft; false when no rename is up.
    pub fn rename_key(&mut self, key: &str, text: Option<&str>, command: bool) -> bool {
        if self.rename.is_none() {
            return false;
        }
        match key {
            "escape" => self.cancel_rename(),
            "enter" => self.commit_rename(),
            _ => {
                let draft = &mut self.rename.as_mut().unwrap().draft;
                if let Typed::Nothing = type_into(draft, key, text, command) {
                    return !command;
                }
            }
        }
        true
    }
    /// Send the draft as the space's name, or show why it cannot be sent (AppModel.renameSpace).
    pub fn commit_rename(&mut self) {
        let Some(rename) = self.rename.clone() else { return };
        // One request at a time: a second Return waits for the first answer.
        if rename.req.is_some() {
            return;
        }
        match sidebar::rename_check(&rename.draft, self.link == Link::Up) {
            Err(why) => self.rename.as_mut().unwrap().error = Some(why),
            Ok(name) => {
                let space = rename.space.clone();
                if self.space_name(&space).as_deref() == Some(name.as_str()) {
                    self.rename = None;
                    return;
                }
                let req = self.req();
                let rename = self.rename.as_mut().unwrap();
                rename.error = None;
                rename.req = Some(req);
                self.log(format!("rename space {space} to {name}"));
                self.send(&ClientMsg::SpaceRename { req: Some(req), space, name });
            }
        }
    }
    /// The answers the sidebar waits on: a rename's, and the session made for Move to New Session.
    pub fn on_sidebar_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Ok { req } if self.rename.as_ref().and_then(|r| r.req) == Some(*req) => {
                self.rename = None;
                true
            }
            ServerMsg::Error { req: Some(req), message, .. } if self.rename.as_ref().and_then(|r| r.req) == Some(*req) => {
                let rename = self.rename.as_mut().unwrap();
                rename.req = None;
                rename.error = Some(message.clone());
                true
            }
            ServerMsg::Created { req, id, .. } if self.space_for_new_session.as_ref().map(|(r, _)| r) == Some(req) => {
                self.layout_reqs.remove(req);
                let (_, space) = self.space_for_new_session.take().unwrap();
                let session = id.clone();
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index: None });
                true
            }
            _ => false,
        }
    }

    // MARK: The hover preview

    /// The row under the pointer changed: the preview opens after `PREVIEW_OPEN_MS` over a row
    /// (at once when another row's is open), and closes `PREVIEW_CLOSE_MS` after leaving.
    pub fn row_hover(&mut self, space: Option<String>) {
        let now = now_ms();
        match space {
            Some(space) => {
                if self.hover.as_ref().map(|(s, _)| s) != Some(&space) {
                    self.hover = Some((space, now));
                }
                self.preview_leave = None;
            }
            None => {
                if self.hover.take().is_some() && self.preview.is_some() {
                    self.preview_leave = Some(now);
                }
            }
        }
    }
    /// Advance the preview's timers; called each render and from the redraw timer.
    pub fn tick_preview(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        // A previewed space that closed or left the session takes its card and its hole with it.
        if self.preview.as_ref().is_some_and(|p| !self.session_ids().contains(p)) {
            self.close_preview();
        }
        if let Some((space, since)) = self.hover.clone() {
            let delay = if self.preview.is_some() { 0 } else { sidebar::PREVIEW_OPEN_MS };
            if now >= since + delay {
                if self.preview.as_deref() != Some(space.as_str()) {
                    self.open_preview(&space);
                }
            } else {
                self.redraw_in(since + delay - now, cx);
            }
        }
        if let Some(left) = self.preview_leave {
            if now >= left + sidebar::PREVIEW_CLOSE_MS {
                self.preview_leave = None;
                self.close_preview();
            } else {
                self.redraw_in(left + sidebar::PREVIEW_CLOSE_MS - now, cx);
            }
        }
        if let Some(space) = self.preview.clone()
            && now >= self.preview_asked + 2000
        {
            self.preview_asked = now;
            self.request_procs(&space);
            self.redraw_in(2000, cx);
        }
    }
    pub fn open_preview(&mut self, space: &str) {
        if !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.preview = Some(space.to_owned());
        self.preview_asked = now_ms();
        self.request_procs(space);
        self.log(format!("preview: opened {space}"));
        self.sync_sidebar_overlay();
    }
    pub fn close_preview(&mut self) {
        self.hover = None;
        self.preview_leave = None;
        if let Some(was) = self.preview.take() {
            self.log(format!("preview: closed (was {was})"));
            self.sync_sidebar_overlay();
        }
    }
    /// Where the preview card is drawn: right of the sidebar, level with its row, kept inside the window.
    pub fn preview_rect(&self) -> Option<Rect> {
        let space = self.preview.as_ref()?;
        let row = self.sidebar_rows().into_iter().find(|r| &r.id == space)?;
        let procs = self.procs.by_space.get(space).map(Vec::len).unwrap_or(0);
        let lines = procs.clamp(1, 4) + usize::from(procs > 4);
        let h = 28. + sidebar::PREVIEW_GRID_H + 20. + lines as f32 * 18. + 30.;
        let y = row.rect.y.min(self.viewport.1 - BOTTOM - h).max(HEADER);
        Some(Rect { x: header::MARGIN + sidebar::WIDTH + sidebar::GAP, y, w: sidebar::PREVIEW_W, h })
    }
    /// The last thing a pane shows, as one line (AppModel.lastLine): an agent's last reply, message
    /// or tool call, the last line of a terminal, a browser's address, a web pane's artifact.
    pub fn last_line(&self, leaf: &Leaf) -> Option<String> {
        fn last(text: &str) -> Option<String> {
            text.lines().map(str::trim).filter(|l| !l.is_empty()).next_back().map(str::to_owned)
        }
        match leaf.kind.as_str() {
            "agent" => {
                let t = self.transcripts.get(leaf.agent.as_deref()?)?;
                t.rows.iter().rev().find_map(|row| match &row.kind {
                    Kind::Reply(text) => last(text),
                    Kind::User { text, .. } => last(text).map(|l| format!("> {l}")),
                    Kind::Tool { title, .. } => Some(title.clone()),
                    _ => None,
                })
            }
            "terminal" => self.native.status()["panes"][leaf.id.as_str()]["viewport"].as_str().and_then(last),
            "browser" => leaf.url.clone(),
            "web" => Some(leaf.label.clone()),
            _ => None,
        }
    }
    /// What the preview shows, for the state query: (pane, kind, status rank, last line).
    pub fn preview_panes(&self, space: &str) -> Vec<(String, String, Option<&'static str>, Option<String>)> {
        self.spaces.iter().find(|s| s.id == space).map(|s| leaves_of(&s.root).into_iter()
            .map(|l| (l.id.clone(), l.kind.clone(), l.status.as_ref().map(status::rank), self.last_line(l))).collect()).unwrap_or_default()
    }
    /// The bridge's hole for the menu or the preview, which sit over the grid's native views.
    fn sync_sidebar_overlay(&mut self) {
        if self.command_palette.is_some() || self.shortcuts_open || self.settings_open || self.confirmation.is_some() || self.pairing.open || self.procs.open.is_some() {
            return;
        }
        let rect = if let Some((space, x, y)) = self.space_menu.clone() {
            Some(Rect { x, y, w: sidebar::MENU_W, h: self.menu_height(&space) })
        } else {
            self.preview_rect()
        };
        let op = match rect {
            Some(r) => serde_json::json!({"op": "overlay", "open": true, "rect": [f64::from(r.x), f64::from(r.y), f64::from(r.w), f64::from(r.h)]}),
            None => serde_json::json!({"op": "overlay", "open": false}),
        };
        self.native.call(op);
    }

    /// The sidebar for `/state`.
    pub fn sidebar_json(&self) -> serde_json::Value {
        use serde_json::json;
        let rows: Vec<_> = self.sidebar_rows().into_iter().map(|r| {
            let space = self.spaces.iter().find(|s| s.id == r.id);
            let (marks, more) = space.map(|s| status::agent_statuses(s, &self.status_dots, status::AGENT_DOT_LIMIT)).unwrap_or_default();
            json!({"id": r.id, "name": space.map(|s| s.name.clone()), "active": self.space_id.as_deref() == Some(r.id.as_str()),
                "frame": [r.rect.x, r.rect.y, r.rect.w, r.rect.h], "close": [r.close.x, r.close.y, r.close.w, r.close.h],
                "dots": marks.iter().map(status::rank).collect::<Vec<_>>(), "more": more,
                "caption": space.and_then(|s| s.status.as_ref()).filter(|s| self.status_dots.contains(status::rank(s))).and_then(status::caption),
                "panes": space.map(|s| leaves_of(&s.root).len()).unwrap_or(0), "pane_over": self.pane_is_over(&r.id)})
        }).collect();
        let r = self.sidebar_rect();
        json!({
            "open": self.sidebar_open, "frame": [r.x, r.y, r.w, r.h], "rows": rows,
            "drag": self.space_drag.as_ref().map(|d| json!({"space": d.space, "target": d.target.as_ref().map(describe_target), "insertion": self.space_drag_insertion(), "hint": self.space_drag_hint()})),
            "pane_drag": self.pane_drag.as_ref().map(|d| json!({"pane": d.pane, "label": d.label, "away": d.away.as_ref().map(describe_away), "hint": self.pane_drag_hint(), "new_space": self.pane_is_over_new_space()})),
            "renaming": self.rename.as_ref().map(|r| json!({"space": r.space, "draft": r.draft, "error": r.error})),
            "menu": self.space_menu.as_ref().map(|(space, _, _)| json!({"space": space, "items": self.space_menu_items(space).iter().map(|i| json!({"id": i.id, "title": i.title, "enabled": i.enabled, "section": i.section, "destructive": i.destructive})).collect::<Vec<_>>()})),
            "preview": self.preview.as_ref().map(|space| json!({"space": space, "frame": self.preview_rect().map(|r| [r.x, r.y, r.w, r.h]),
                "panes": self.preview_panes(space).iter().map(|(pane, kind, status, last)| json!({"pane": pane, "kind": kind, "status": status, "last": last})).collect::<Vec<_>>(),
                "procs": self.procs.by_space.get(space).map(|p| p.len()).unwrap_or(0)})),
        })
    }

    // MARK: Drawing

    /// The sidebar: its rows, the insertion bar of a space drag, the outline of a pane drag.
    pub fn sidebar_element(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let r = self.sidebar_rect();
        let rows = self.sidebar_rows();
        let insertion = self.space_drag_insertion();
        let dragged = self.space_drag.as_ref().map(|d| d.space.clone());
        let over_new = self.pane_is_over_new_space();
        let mut bar = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded_lg().bg(rgb(CARD_BG))
            .border_2().border_color(rgb(if over_new { ACCENT } else { CARD_BORDER })).overflow_hidden();
        for row in &rows {
            let Some(space) = self.spaces.iter().find(|s| s.id == row.id) else { continue };
            let active = self.space_id.as_deref() == Some(row.id.as_str());
            let (id, rr, close) = (row.id.clone(), row.rect, row.close);
            let dimmed = dragged.as_deref() == Some(id.as_str());
            let caption = space.status.as_ref().filter(|s| self.status_dots.contains(status::rank(s))).and_then(status::caption);
            let renaming = self.rename.as_ref().filter(|rn| rn.space == id).cloned();
            let (press_id, menu_id, close_id) = (id.clone(), id.clone(), id.clone());
            let mut el = div().absolute().left(px(rr.x - r.x)).top(px(rr.y - r.y)).w(px(rr.w)).h(px(rr.h)).px(px(6.)).rounded_md().flex().items_center().gap(px(4.))
                .bg(rgb(if active { ROW_BG_ACTIVE } else { CARD_BG })).cursor_pointer()
                .when(dimmed, |d| d.opacity(0.4))
                .when(self.pane_is_over(&id), |d| d.border_2().border_color(rgb(ACCENT)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    if this.rename.as_ref().is_some_and(|rn| rn.space == press_id) {
                        cx.stop_propagation();
                        return;
                    }
                    // A control-click asks for the menu, as a right press does.
                    if e.modifiers.control {
                        cx.stop_propagation();
                        this.open_space_menu(&press_id, f32::from(e.position.x), f32::from(e.position.y));
                        cx.notify();
                        return;
                    }
                    this.space_press_down(&press_id, f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_space_menu(&menu_id, f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }))
                .child(pane_stack(leaves_of(&space.root).len(), active))
                .child(div().w(px(12.)).h(px(12.)).flex_none().flex().items_center().children(status::space_dots(space, &self.status_dots)));
            match renaming {
                Some(rn) => {
                    el = el.child(div().flex_1().min_w(px(0.)).flex().flex_col().child(
                        div().h(px(20.)).px(px(4.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().text_xs().flex().items_center()
                            .child(format!("{}▏", rn.draft))));
                }
                None => {
                    el = el.child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_sm()
                        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD)).child(space.name.clone()));
                    if let Some(caption) = caption {
                        el = el.child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().max_w(px(100.)).child(caption));
                    }
                }
            }
            el = el.child(div().w(px(close.w)).h(px(close.h)).flex_none().rounded_sm().flex().items_center().justify_center().text_xs().text_color(rgb(DIM)).cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.press(format!("close space {close_id}"));
                    this.ask_close_space(&close_id);
                    cx.notify();
                }))
                .child("×"));
            bar = bar.child(el);
            if let Some(err) = self.rename.as_ref().filter(|rn| rn.space == id).and_then(|rn| rn.error.clone()) {
                bar = bar.child(div().absolute().left(px(rr.x - r.x + 6.)).top(px(rr.y - r.y + rr.h)).w(px(rr.w - 12.)).text_xs().text_color(rgb(RED)).child(err));
            }
        }
        if let Some(bar_rect) = insertion.and_then(|i| sidebar::bar_rect(&rows, i)) {
            bar = bar.child(div().absolute().left(px(bar_rect.x - r.x)).top(px(bar_rect.y - r.y)).w(px(bar_rect.w)).h(px(bar_rect.h)).rounded_full().bg(rgb(ACCENT)));
        }
        bar
    }
    /// The chip under the pointer during a space or pane drag, naming what is held.
    pub fn drag_chip(&self) -> Option<gpui::Div> {
        let (text, at) = if let Some(d) = &self.space_drag {
            (self.space_name(&d.space).unwrap_or_else(|| d.space.clone()), d.at)
        } else if let Some(d) = &self.pane_drag {
            (if d.away == Some(PaneAway::NewSpace) { format!("{} → new space", d.label) } else { d.label.clone() }, d.at)
        } else {
            return None;
        };
        Some(div().absolute().left(px(at.0 + 12.)).top(px(at.1 + 12.)).px(px(10.)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(CARD_BORDER)).text_sm().whitespace_nowrap().child(text))
    }
    /// The space's context menu, anchored at the press.
    pub fn space_menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (space, x, y) = self.space_menu.clone()?;
        let items = self.space_menu_items(&space);
        let mut list = div().absolute().left(px(x)).top(px(y)).w(px(sidebar::MENU_W)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(CARD_BORDER)).flex().flex_col().text_sm();
        let mut last_section = None;
        for item in items {
            if last_section.is_some_and(|s| s != item.section) {
                list = list.child(div().h(px(1.)).my(px(3.)).bg(rgb(CARD_BORDER)));
            }
            last_section = Some(item.section);
            let (id, sp) = (item.id.clone(), space.clone());
            let colour = if !item.enabled { 0x5a6e64 } else if item.destructive { RED } else { 0xd9e8e1 };
            let mut row = div().h(px(sidebar::MENU_ROW_H)).px(px(10.)).flex().items_center().text_color(rgb(colour)).child(item.title.clone());
            if item.enabled {
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.run_space_menu(&id, &sp, cx);
                    cx.notify();
                }));
            }
            list = list.child(row);
        }
        Some(list)
    }
    /// The hover preview: the space's grid as the panes look, each with its kind, its mark and
    /// its last line, then the processes its panes started (SpacePreviewView).
    pub fn preview_card(&self) -> Option<gpui::Div> {
        let space = self.preview.clone()?;
        let r = self.preview_rect()?;
        let info = self.spaces.iter().find(|s| s.id == space)?;
        let mut grid = div().relative().w(px(r.w - 20.)).h(px(sidebar::PREVIEW_GRID_H));
        let (mut leaves, mut dividers) = (vec![], vec![]);
        place(&info.root, Rect { x: 0., y: 0., w: r.w - 20., h: sidebar::PREVIEW_GRID_H }, &HashMap::new(), &mut leaves, &mut dividers);
        for p in leaves {
            let title = match p.leaf.kind.as_str() {
                "agent" => p.leaf.harness.clone().unwrap_or_else(|| "agent".into()),
                "terminal" => "terminal".into(),
                other => other.to_owned(),
            };
            let focused = self.focus.current.as_deref() == Some(p.leaf.id.as_str());
            let mut pane = div().absolute().left(px(p.rect.x)).top(px(p.rect.y)).w(px(p.rect.w)).h(px(p.rect.h)).p(px(6.)).rounded_sm().bg(rgb(0x22302a)).overflow_hidden()
                .border_1().border_color(rgb(if focused { ACCENT } else { CARD_BORDER })).flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap(px(5.)).text_xs().font_weight(FontWeight::SEMIBOLD).child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(title))
                    .child(div().w(px(12.)).h(px(12.)).flex_none().children(p.leaf.status.as_ref().and_then(|s| status::dot(s, &self.status_dots, gpui::ElementId::Name(format!("preview-dot-{}", p.leaf.id).into()))))));
            if let Some(last) = self.last_line(&p.leaf) {
                pane = pane.child(div().text_xs().text_color(rgb(DIM)).overflow_hidden().child(last));
            }
            grid = grid.child(pane);
        }
        let procs = self.procs.by_space.get(&space).cloned().unwrap_or_default();
        let mut list = div().flex().flex_col().gap(px(2.)).text_xs();
        if procs.is_empty() {
            list = list.child(div().text_color(rgb(DIM)).child("No processes running"));
        } else {
            for p in procs.iter().take(4) {
                list = list.child(div().flex().gap(px(6.)).child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(p.name.clone())).child(div().text_color(rgb(DIM)).child(format!("{:.0}%", p.cpu))));
            }
            if procs.len() > 4 {
                list = list.child(div().text_color(rgb(DIM)).child(format!("+{} more", procs.len() - 4)));
            }
        }
        Some(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).p(px(10.)).rounded_lg().bg(rgb(CARD_BG)).border_1().border_color(rgb(CARD_BORDER)).flex().flex_col().gap(px(8.))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().child(info.name.clone()))
            .child(grid)
            .child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().child(info.cwd.clone()))
            .child(list))
    }
}

fn describe_target(target: &SpaceDropTarget) -> String {
    match target {
        SpaceDropTarget::Sidebar(index) => format!("sidebar at {index}"),
        SpaceDropTarget::Window => "the window".into(),
    }
}
fn describe_away(away: &PaneAway) -> String {
    match away {
        PaneAway::End(space) => format!("the end of space {space}"),
        PaneAway::NewSpace => "a new space".into(),
    }
}

/// A space's panes as a stack of small squares, one per pane up to four, each offset to the
/// right of the one in front (PaneStack.swift).
fn pane_stack(panes: usize, active: bool) -> gpui::Div {
    let layers = sidebar::stack_layers(panes);
    let (size, step) = (12., 3.);
    let mut stack = div().relative().flex_none().w(px(size + (layers - 1) as f32 * step)).h(px(size));
    for i in (0..layers).rev() {
        stack = stack.child(div().absolute().left(px(i as f32 * step)).top(px(0.)).w(px(size)).h(px(size)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(if active { 0xd9e8e1 } else { DIM })));
    }
    stack
}

