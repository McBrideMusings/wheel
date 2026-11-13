//! The Settings window (apple/Mac/Sources/SettingsWindow.swift and the tab files beside it), drawn
//! as a window-wide overlay: a tab bar across the top and the chosen tab's page under it. Every
//! page reads and writes the daemon's settings (`settings_ls`, `setting_set`, `setting_unset`), so
//! a change here shows in the Swift app and the other way round through the `settings` broadcast.
use super::*;
use crate::settings::{self as model, BuiltinAction, Field, Layer, Model, Tab, Topic};
use wheel_proto::{OnEdited, SettingKind};

/// The requests the window has out, so their answers are told apart.
#[derive(Debug, Default)]
pub struct SettingsReqs {
    pub list: Option<u64>,
    pub changes: HashMap<u64, Field>,
    pub homes: HashMap<u64, (Field, String)>,
    pub status: Option<u64>,
    pub files: HashMap<u64, Layer>,
    pub guidance: Option<u64>,
    /// `guidance_path` requests, each with whether the file is launched when the path arrives.
    pub guidance_open: HashMap<u64, bool>,
    /// Built-in requests, each with the built-in it is about ("" for a listing).
    pub builtins: HashMap<u64, String>,
    pub pairing: Option<u64>,
    /// The `remote_info` request whose answer runs the menu bar's pairing ask.
    pub pairing_remote: Option<u64>,
}

/// What the keyboard types into while the window is open.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEdit {
    Field(Field),
    Search(Topic),
}

pub(super) const PANEL: u32 = 0x17201c;
pub(super) const BOX: u32 = 0x1d2823;
pub(super) const LINE: u32 = 0x2c3d34;
pub(super) const TEXT: u32 = 0xd9e8e1;
pub(super) const DIM: u32 = 0x8fa89b;
pub(super) const ACCENT: u32 = 0x31483e;
const ORANGE: u32 = 0xe0b070;
const RED: u32 = 0xe07070;
const GREEN: u32 = 0x6fcf97;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Client {
    // Opening and closing

    pub fn toggle_settings(&mut self) {
        if self.settings_open { self.close_settings() } else { self.open_settings() }
    }

    /// The window comes up: the Project tab starts on the space being shown, and the daemon is
    /// asked for the settings, its status (log shipping, folder access, the PATH), how devices
    /// reach it and the `wheel` link is checked.
    pub fn open_settings(&mut self) {
        self.settings_open = true;
        self.settings.project = self.space_id.clone().or_else(|| self.spaces.first().map(|s| s.id.clone()));
        self.ask_settings();
        self.ask_settings_status();
        self.ask_remote();
        self.check_command_line();
        self.sync_overlay();
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
        self.settings_edit = None;
        self.sync_overlay();
    }

    pub fn settings_tab(&mut self, tab: Tab) {
        self.settings.tab = tab;
        self.settings_edit = None;
        match tab {
            Tab::Artifacts => self.ask_builtins(),
            Tab::Guidance => self.ask_guidance(),
            Tab::Devices => {
                self.ask_remote();
            }
            Tab::General => self.ask_settings_status(),
            _ => {}
        }
    }

    /// The Project tab's list: edit this space's `wheel.toml`.
    pub fn select_settings_project(&mut self, id: &str) {
        if self.settings.project.as_deref() == Some(id) {
            return;
        }
        self.settings.project = Some(id.to_owned());
        self.settings_edit = None;
        self.ask_settings();
    }

    // Requests

    pub(super) fn settings_req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    /// Ask for every setting as it resolves for the chosen project; with no project it is the
    /// machine-wide view.
    pub fn ask_settings(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.list = Some(req);
        let project = self.settings.project.clone();
        self.settings.asking(project.as_deref());
        self.send(&ClientMsg::SettingsLs { req: Some(req), space: project });
    }

    /// Ask wheeld for its status, for the log shipping, folder access and PATH the General tab shows.
    fn ask_settings_status(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.status = Some(req);
        self.send(&ClientMsg::Status { req: Some(req) });
    }

    /// Ask for the remote listener and the paired devices; the listener request's id, None while the link is down.
    pub(super) fn ask_remote(&mut self) -> Option<u64> {
        if self.link != Link::Up {
            return None;
        }
        let a = self.settings_req();
        let b = self.settings_req();
        self.send(&ClientMsg::RemoteInfo { req: Some(a) });
        self.send(&ClientMsg::DeviceList { req: Some(b) });
        Some(a)
    }

    pub fn request_pairing_code(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.pairing = Some(req);
        self.send(&ClientMsg::PairingIssue { req: Some(req) });
    }

    pub fn revoke_device(&mut self, id: &str) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.pairing = Some(req);
        self.send(&ClientMsg::DeviceRevoke { req: Some(req), id: id.to_owned() });
    }

    fn ask_builtins(&mut self) {
        self.send_builtin("", |req| ClientMsg::BuiltinLs { req: Some(req) });
    }

    pub fn install_builtin(&mut self, name: &str) {
        let n = name.to_owned();
        self.send_builtin(name, move |req| ClientMsg::BuiltinInstall { req: Some(req), name: n });
    }

    pub fn update_builtin(&mut self, name: &str, on_edited: OnEdited) {
        self.settings.builtins.edited = None;
        let n = name.to_owned();
        self.send_builtin(name, move |req| ClientMsg::BuiltinUpdate { req: Some(req), name: n, on_edited });
    }

    pub fn remove_builtin(&mut self, name: &str) {
        let n = name.to_owned();
        self.send_builtin(name, move |req| ClientMsg::BuiltinRemove { req: Some(req), name: n });
    }

    fn send_builtin(&mut self, name: &str, make: impl FnOnce(u64) -> ClientMsg) {
        if self.link != Link::Up {
            self.settings.builtins.problem = Some("Not connected to wheeld.".into());
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.builtins.insert(req, name.to_owned());
        self.settings.builtins.problem = None;
        self.send(&make(req));
    }

    fn ask_guidance(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.guidance = Some(req);
        self.send(&ClientMsg::GuidanceLs { req: Some(req), agent: Some(self.settings.guidance.preview_agent.clone()) });
    }

    pub fn preview_guidance(&mut self, agent: &str) {
        if self.settings.guidance.preview_agent != agent {
            self.settings.guidance.preview_agent = agent.to_owned();
            self.ask_guidance();
        }
    }

    /// Open in Editor: the server makes the file when it is missing and says where it is.
    pub fn open_guidance_file(&mut self, agent: Option<&str>) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.guidance_open.insert(req, !self.hidden);
        self.send(&ClientMsg::GuidancePath { req: Some(req), agent: agent.map(str::to_owned) });
    }

    /// The Open button: the server makes the layer's `wheel.toml` when it is absent, then the
    /// editor opens it (ADR-0020).
    pub fn open_settings_file(&mut self, layer: Layer) {
        if self.link != Link::Up {
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.files.insert(req, layer);
        self.send(&ClientMsg::SettingsFileEnsure { req: Some(req), space: (layer == Layer::Project).then(|| self.settings.project.clone()).flatten(), machine: layer == Layer::Global });
    }

    /// Opens a file for editing in the Mac's default app, and records how; a hidden window
    /// (the check) records the press and starts no editor.
    fn open_file(&mut self, path: &str) {
        if self.hidden {
            self.settings.opened_file = Some((path.to_owned(), "not launched".into()));
            return;
        }
        self.pending_open = Some(path.to_owned());
        self.settings.opened_file = Some((path.to_owned(), "default app".into()));
    }

    /// Ask wheeld for the folders a home field's agent could use and what the field's folder holds.
    fn ask_homes(&mut self, field: &Field) {
        let Some(agent) = model::home_agent(&field.key).map(str::to_owned) else { return };
        if self.link != Link::Up {
            return;
        }
        let text = self.settings.text(field);
        let req = self.settings_req();
        self.settings_reqs.homes.insert(req, (field.clone(), text.clone()));
        self.send(&ClientMsg::AgentHomes { req: Some(req), agent, path: Some(text) });
    }

    /// Return in a field: write what was typed into that layer's file, or remove the key from it
    /// when the field was emptied. A field holding what its file sets sends nothing.
    pub fn save_setting(&mut self, field: &Field) {
        if !self.settings.dirty(field) {
            return;
        }
        let Some(text) = self.settings.drafts.get(field).cloned() else { return };
        if text.trim().is_empty() {
            self.clear_setting(field);
        } else {
            self.write_setting(field, &text);
        }
    }

    /// Write `text` into the field's file: a control that already holds its value.
    pub fn write_setting(&mut self, field: &Field, text: &str) {
        let (key, value, space, machine) = (field.key.clone(), text.to_owned(), self.settings.project.clone(), field.layer == Layer::Global);
        self.change_setting(field, |req| ClientMsg::SettingSet { req: Some(req), key, value, space, machine });
    }

    /// Remove the key from that layer's file, so the layer below is in force.
    pub fn clear_setting(&mut self, field: &Field) {
        let (key, space, machine) = (field.key.clone(), self.settings.project.clone(), field.layer == Layer::Global);
        self.change_setting(field, |req| ClientMsg::SettingUnset { req: Some(req), key, space, machine });
    }

    fn change_setting(&mut self, field: &Field, make: impl FnOnce(u64) -> ClientMsg) {
        if self.link != Link::Up {
            self.settings.refused(field, "Not connected to wheeld; your change is still in the field.");
            return;
        }
        let req = self.settings_req();
        self.settings_reqs.changes.insert(req, field.clone());
        self.settings.saving.insert(field.clone());
        self.send(&make(req));
    }

    /// A machine-wide list setting toggled by a checkbox: `status.dots`, `usage.agents`, `usage.labels`.
    pub(super) fn write_machine(&mut self, key: &str, value: String) {
        if self.link != Link::Up {
            return;
        }
        self.send(&ClientMsg::SettingSet { req: None, key: key.to_owned(), value, space: None, machine: true });
    }

    /// Where the `wheel` command stands: the bundled one's `install --status`, run off the UI
    /// thread against the PATH wheeld reported; `install` links it.
    fn run_command_line(&mut self, install: bool) {
        let Some(program) = bundled_wheel() else {
            self.settings.command_line.state = "unavailable".into();
            self.settings.command_line.detail = Some("This build of the app carries no wheel command.".into());
            return;
        };
        if self.settings.command_line.running {
            return;
        }
        self.settings.command_line.running = true;
        let login = self.settings.login_path.as_ref().map(|l| l.path.clone());
        let slot = self.command_line_result.clone();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(program);
            command.arg("install");
            if !install {
                command.arg("--status");
            }
            if let Some(path) = login {
                command.env("PATH", path);
            }
            let result = match command.output() {
                Ok(out) => {
                    let text = if out.status.success() { out.stdout } else { out.stderr };
                    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&text).trim().to_owned())
                }
                Err(e) => (-1, e.to_string()),
            };
            *slot.lock().unwrap() = Some(result);
        });
    }

    pub fn check_command_line(&mut self) {
        self.run_command_line(false);
    }

    pub fn install_command_line(&mut self) {
        self.run_command_line(true);
    }

    /// Each render: a finished `wheel install` run is read into the model, and a running one
    /// keeps the window redrawing until it ends.
    pub fn poll_settings(&mut self, cx: &mut Context<Self>) {
        let done = self.command_line_result.lock().unwrap().take();
        if let Some((code, text)) = done {
            let login = self.settings.login_path.clone();
            self.settings.command_line.read(code, &text, login.as_ref());
        }
        if self.settings.command_line.running {
            self.redraw_in(200, cx);
        }
    }

    /// Redraw the window after `ms`, once: the countdown and the running command have no event
    /// of their own. A timer already armed is left alone.
    pub(super) fn redraw_in(&mut self, ms: u64, cx: &mut Context<Self>) {
        // A timer already due sooner covers this one; a sooner ask arms another timer beside it.
        let due = now_ms() + ms;
        if self.redraw_due.is_some_and(|armed| armed <= due) {
            return;
        }
        self.redraw_due = Some(due);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
            let _ = this.update(cx, |client, cx| {
                if client.redraw_due == Some(due) {
                    client.redraw_due = None;
                }
                // The server's window and countdown run on this timer, not only on a draw: an
                // occluded or hidden window still rolls back on time.
                client.tick_server(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // Replies

    /// A reply the window asked for; false when it answers none of its requests.
    pub fn on_settings_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            // As AppModel.receivedSettings: our listing replaces the rows; the answer to our
            // change clears its field and, when resolved for our project, replaces the rows; a
            // broadcast (another client's write) replaces them when resolved for the space shown,
            // else the open window asks again for its own.
            ServerMsg::Settings { req, space, settings } => {
                if req.is_some() && *req == self.settings_reqs.list {
                    self.settings_reqs.list = None;
                    self.settings.show(settings.clone(), space.clone());
                    return true;
                }
                if let Some(field) = req.and_then(|r| self.settings_reqs.changes.remove(&r)) {
                    self.settings.changed(&field);
                    if *space == self.settings.project {
                        self.settings.show(settings.clone(), space.clone());
                    }
                    return true;
                }
                if req.is_none() {
                    if *space == self.settings.space && self.settings.loaded {
                        self.settings.show(settings.clone(), space.clone());
                    } else if self.settings_open && self.settings_reqs.list.is_none() {
                        self.ask_settings();
                    }
                }
                false
            }
            ServerMsg::Error { req: Some(req), message, .. } => {
                if Some(*req) == self.settings_reqs.list {
                    self.settings_reqs.list = None;
                    self.settings.list_failed(message);
                } else if let Some(field) = self.settings_reqs.changes.remove(req) {
                    self.settings.refused(&field, message);
                } else if self.settings_reqs.builtins.remove(req).is_some() {
                    self.settings.builtins.problem = Some(message.clone());
                    self.settings.builtins.loaded = true;
                } else if Some(*req) == self.settings_reqs.guidance {
                    self.settings_reqs.guidance = None;
                    self.settings.guidance.error = Some(message.clone());
                } else if self.settings_reqs.guidance_open.remove(req).is_some() {
                    self.settings.guidance.error = Some(message.clone());
                } else if Some(*req) == self.settings_reqs.pairing {
                    self.settings_reqs.pairing = None;
                    self.settings.devices.code_problem = Some(message.clone());
                    // A copy that asked for this code gets none: the next code is not its.
                    self.pairing.copy_pending = false;
                } else if Some(*req) == self.settings_reqs.status {
                    self.settings_reqs.status = None;
                } else if !(self.settings_reqs.homes.remove(req).is_some() || self.settings_reqs.files.remove(req).is_some()) {
                    return false;
                }
                true
            }
            ServerMsg::Ok { req } => {
                if let Some(layer) = self.settings_reqs.files.remove(req) {
                    if let Some(path) = self.settings.file(layer) {
                        self.open_file(&path);
                    }
                    return true;
                }
                false
            }
            ServerMsg::AgentHomes { req, homes, checked, .. } => {
                let Some((field, text)) = self.settings_reqs.homes.remove(req) else { return false };
                if let Some(agent) = model::home_agent(&field.key) {
                    self.settings.home_lists.insert(agent.to_owned(), homes.clone());
                }
                if self.settings.text(&field) == text {
                    match checked {
                        Some(home) => self.settings.home_checks.insert(field, home.clone()),
                        None => self.settings.home_checks.remove(&field),
                    };
                }
                true
            }
            ServerMsg::Builtins { req, builtins } => {
                if self.settings_reqs.builtins.remove(req).is_none() {
                    return false;
                }
                self.settings.builtins.builtins = builtins.clone();
                self.settings.builtins.loaded = true;
                true
            }
            ServerMsg::BuiltinEdited { req, name, edited } => {
                if self.settings_reqs.builtins.remove(req).is_none() {
                    return false;
                }
                self.settings.builtins.edited = Some((name.clone(), edited.clone()));
                true
            }
            ServerMsg::Guidance { req, brief, given, files } => {
                if Some(*req) != self.settings_reqs.guidance {
                    return false;
                }
                self.settings_reqs.guidance = None;
                self.settings.guidance.received(brief.clone(), given.clone(), files.clone());
                true
            }
            ServerMsg::GuidancePath { req, path } => {
                let Some(launch) = self.settings_reqs.guidance_open.remove(req) else { return false };
                if launch {
                    self.pending_open = Some(path.clone());
                    self.settings.opened_file = Some((path.clone(), "default app".into()));
                } else {
                    self.settings.opened_file = Some((path.clone(), "not launched".into()));
                }
                true
            }
            ServerMsg::Remote { req, listen, addresses, port, fingerprint, problem } => {
                self.settings.devices.info = Some(model::Remote { listen: listen.clone(), addresses: addresses.clone(), port: *port, fingerprint: fingerprint.clone(), problem: problem.clone() });
                if self.settings_reqs.pairing_remote == Some(*req) {
                    self.settings_reqs.pairing_remote = None;
                    self.pairing_asked();
                }
                true
            }
            ServerMsg::Devices { devices, .. } | ServerMsg::DevicesChanged { devices } => {
                self.settings.devices.paired = devices.clone();
                true
            }
            ServerMsg::Pairing { req, token, expires_ms } => {
                if Some(*req) != self.settings_reqs.pairing {
                    return false;
                }
                self.settings_reqs.pairing = None;
                self.settings.devices.code = Some((token.clone(), *expires_ms));
                self.settings.devices.code_problem = None;
                self.pairing_code_arrived(token);
                true
            }
            ServerMsg::UsageChanged { board } => {
                self.take_usage_board(board);
                false
            }
            _ => false,
        }
    }

    /// The parts of a `status` reply the window shows.
    pub fn on_settings_status(&mut self, message: &ServerMsg) {
        if let ServerMsg::Status { req, logs, host, shell_integration, login_path, usage_board, .. } = message {
            if req.is_some() && *req == self.settings_reqs.status {
                self.settings_reqs.status = None;
            }
            self.settings.log_shipping = Some((**logs).clone());
            self.settings.host = Some((**host).clone());
            self.settings.shell_problem = shell_integration.clone();
            self.settings.usage = usage_board.clone();
            if self.settings.login_path.as_ref() != Some(login_path) {
                self.settings.login_path = Some(login_path.clone());
                if self.settings_open && self.settings.command_line.detail.is_some() {
                    self.check_command_line();
                }
            }
        }
    }

    /// A new connection: answers to the old one's requests never come, and the lists are asked again.
    pub fn settings_reconnected(&mut self) {
        // The menu bar's pairing ask made before the link first came up goes out now; one whose
        // request the old connection carried is dropped with it.
        let unsent = self.pairing.show_asked && self.settings_reqs.pairing_remote.is_none();
        if !unsent {
            self.pairing.show_asked = false;
        }
        self.settings_reqs = SettingsReqs::default();
        self.settings.forget_requests();
        if self.settings_open {
            self.ask_settings();
            self.ask_settings_status();
            self.ask_remote();
        }
        if self.pairing.show_asked {
            self.settings_reqs.pairing_remote = self.ask_remote();
        }
    }

    // Keys and the pointer

    /// Start editing a field: the keyboard types into it, and wheeld is asked about its folder
    /// when it is an agent's home.
    pub fn edit_setting(&mut self, field: Field) {
        if self.settings_edit != Some(SettingsEdit::Field(field.clone())) {
            self.settings_edit = Some(SettingsEdit::Field(field.clone()));
            self.ask_homes(&field);
        }
    }

    /// The window's keys: Esc puts a field back then closes the window, Return saves the field,
    /// and anything else types into the field or the page's search.
    pub fn settings_key(&mut self, key: &str, text: Option<&str>, chord: bool, cx: &mut App) -> bool {
        if !self.settings_open {
            return false;
        }
        // ⌘V pastes into the field or the search, as a TextField takes it.
        let pasted = (chord && key == "v").then(|| cx.read_from_clipboard().and_then(|item| item.text())).flatten();
        match self.settings_edit.clone() {
            Some(SettingsEdit::Field(field)) => match key {
                "v" if pasted.is_some() => {
                    let typed = format!("{}{}", self.settings.text(&field), pasted.unwrap_or_default());
                    self.settings.type_text(&field, &typed);
                    self.ask_homes(&field);
                }
                "escape" => {
                    self.settings.revert(&field);
                    self.settings_edit = None;
                }
                "enter" => {
                    self.save_setting(&field);
                    self.settings_edit = None;
                }
                "backspace" => {
                    let mut t = self.settings.text(&field);
                    t.pop();
                    self.settings.type_text(&field, &t);
                    self.ask_homes(&field);
                }
                _ => {
                    if !chord && let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                        let typed = format!("{}{t}", self.settings.text(&field));
                        self.settings.type_text(&field, &typed);
                        self.ask_homes(&field);
                    }
                }
            },
            Some(SettingsEdit::Search(topic)) => match key {
                "v" if pasted.is_some() => self.settings.page_mut(topic).query.push_str(&pasted.unwrap_or_default()),
                "escape" | "enter" => self.settings_edit = None,
                "backspace" => {
                    self.settings.page_mut(topic).query.pop();
                }
                _ => {
                    if !chord && let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                        self.settings.page_mut(topic).query.push_str(t);
                    }
                }
            },
            None => {
                if key == "escape" {
                    self.close_settings();
                }
            }
        }
        true
    }

    /// A topic control's write: a switch, a choice, a number, an ordered list.
    pub fn write_control(&mut self, field: &Field, value: String) {
        let shown = self.settings.stored(field).unwrap_or_else(|| self.settings.inherited(field));
        if value != shown || self.settings.stored(field).is_none() {
            self.write_setting(field, &value);
        }
    }

    /// The QR modules for `text`, drawn by the bridge's CoreImage and kept while the text holds.
    pub fn qr_rows(&mut self, text: &str) -> Vec<String> {
        if let Some((t, rows)) = &self.qr_cache
            && t == text
        {
            return rows.clone();
        }
        let rows: Vec<String> = self.native.call(serde_json::json!({"op": "qr", "text": text}))["rows"].as_array().map(|r| r.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        if !rows.is_empty() {
            self.qr_cache = Some((text.to_owned(), rows.clone()));
        }
        rows
    }

    // Drawing

    /// The window over the client: a card with the tab bar and the chosen page.
    pub fn settings_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let card_w = 760f32.min(width - 32.);
        let card_h = 600f32.min(height - 32.);
        let mut tabs = div().flex().gap_1().px_3().py_2().border_b_1().border_color(rgb(LINE)).flex_wrap();
        for tab in Tab::ALL {
            let active = self.settings.tab == tab;
            tabs = tabs.child(div().px_2().py(px(3.)).rounded_sm().cursor_pointer().bg(rgb(if active { ACCENT } else { BOX })).text_xs()
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    this.settings_tab(tab);
                }))
                .child(tab.title()));
        }
        let page = match self.settings.tab {
            Tab::General | Tab::Agents | Tab::Usage | Tab::Keys => self.topic_page(self.settings.tab.topic().unwrap(), cx),
            Tab::Chat | Tab::Terminal => self.chat_like_page(self.settings.tab.topic().unwrap(), cx),
            Tab::Artifacts => self.builtins_page(cx),
            Tab::Guidance => self.guidance_page(cx),
            Tab::Project => self.project_page(cx),
            Tab::Devices => self.devices_page(cx),
        };
        let mut card = div().w(px(card_w)).h(px(card_h)).flex().flex_col().rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(0x3d5247)).text_color(rgb(TEXT)).text_sm().overflow_hidden()
            .on_mouse_down(MouseButton::Left, click(cx, |this| {
                // A press on the card outside a field ends the field's edit.
                this.settings_edit = None;
            }))
            .child(div().flex().justify_between().items_center().px_3().pt_2().child(div().font_weight(FontWeight::SEMIBOLD).child("Settings")).child(div().text_xs().text_color(rgb(DIM)).child("Esc closes")))
            .child(tabs)
            .child(div().id("settings-page").overflow_y_scroll().flex_1().min_h_0().p_4().child(page));
        if let Some((name, files)) = self.settings.builtins.edited.clone() {
            card = card.child(self.edited_sheet(&name, &files, cx));
        }
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_center()
            .on_mouse_down(MouseButton::Left, click(cx, |this| {
                this.close_settings();
            }))
            .child(card)
    }

    /// The rows, or why there are none: the daemon is out of reach, refused the list, or has not
    /// answered yet; above them where the layer's file is and a button that opens it.
    fn content(&mut self, layer: Layer, form: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        match &self.link {
            Link::Up => {}
            Link::Down(reason) => return notice(&format!("wheeld unreachable: {reason}. Retrying every 2 seconds."), Some("Settings live in the daemon; they show here once it answers.")),
            Link::Incompatible { .. } => return self.server_box(cx),
            Link::Connecting => return notice("Connecting to wheeld…", Some("Settings live in the daemon; they show here once it answers.")),
        }
        if let Some(error) = &self.settings.list_error {
            return notice(error, None);
        }
        if !self.settings.loaded {
            return notice("Asking wheeld for its settings…", None);
        }
        let mut out = div().flex().flex_col().gap_3();
        if let Some(file) = self.settings.file(layer) {
            out = out.child(div().flex().items_center().gap_2().text_xs().text_color(rgb(DIM))
                .child(div().overflow_hidden().whitespace_nowrap().child(format!("Saved in {}", model::tilde(&file))))
                .child(button("Open", click(cx, move |this| {
                    this.open_settings_file(layer);
                }))));
        }
        out.child(form)
    }

    /// One topic of this machine's settings as a page of its own.
    fn topic_page(&mut self, topic: Topic, cx: &mut Context<Self>) -> gpui::Div {
        let form = self.settings_form(Layer::Global, topic, cx);
        let content = self.content(Layer::Global, form, cx);
        // The pane header is this client's own drawing, so its box shows whether or not wheeld answers.
        if topic == Topic::General { div().flex().flex_col().gap_4().child(self.windows_box(cx)).child(self.pane_header_box(cx)).child(content) } else { content }
    }

    /// Settings › General › Pane header (PaneHeaderSettings): a kind picker, Reset, and for each
    /// control that kind has, where it goes: the top bar, the ⋯ menu, or nowhere.
    fn pane_header_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::pane_header::{Control, KINDS, Placement};
        let kind = self.header_settings_kind;
        let store = self.header_layout.borrow();
        let kinds: Vec<String> = KINDS.iter().map(|k| super::commands::capitalized(k)).collect();
        let picker = segmented(&kinds.iter().zip(KINDS).map(|(t, k)| (t.as_str(), k == kind)).collect::<Vec<_>>(), false, cx, |this, index| {
            this.header_settings_kind = KINDS[index];
        });
        let changed = store.layout.choices.contains_key(kind);
        let mut rows = div().flex().flex_col()
            .child(div().flex().items_center().justify_between().gap_2().px_3().py_2().child(picker)
                .child(button_state("Reset", !changed, click(cx, move |this| {
                    if changed {
                        this.reset_header_layout(kind);
                    }
                }))));
        for control in Control::ALL.into_iter().filter(|c| c.applies(kind)) {
            let now = store.layout.placement(control, kind);
            let icon = self.symbol(control.symbol(), 12., TEXT).unwrap_or_else(|| div().into_any_element());
            let places = segmented(&Placement::ALL.iter().map(|p| (p.title(), *p == now)).collect::<Vec<_>>(), false, cx, move |this, index| {
                this.set_header_placement(control, kind, Placement::ALL[index]);
            });
            rows = rows.child(divider()).child(div().flex().items_center().gap_2().px_3().py_2()
                .child(div().w(px(20.)).flex().justify_center().child(icon))
                .child(div().flex_1().child(control.title()))
                .child(places));
        }
        rows = rows.child(divider()).child(div().px_3().py_2().text_xs().text_color(rgb(DIM))
            .child(format!("Where each icon button of a {kind} pane’s header goes. A narrow header moves what does not fit into the ⋯ menu.")));
        titled("Pane header", boxed(rows))
    }

    /// The keys the daemon reports for one layer that belong to one topic, grouped into a box per
    /// key prefix, in the daemon's order. This machine's page also carries the topic's rows that
    /// are not settings: the command line, folder access, the status dots, the agents found, the
    /// usage agents and labels, and the keymap table.
    fn settings_form(&mut self, layer: Layer, topic: Topic, cx: &mut Context<Self>) -> gpui::Div {
        let mut form = div().flex().flex_col().gap_4();
        if layer == Layer::Global {
            match topic {
                Topic::General => {
                    if self.restart_waiting() {
                        form = form.child(titled("Server", self.server_box(cx)));
                    }
                    form = form.child(self.command_line_box(cx)).child(self.folder_access_box()).child(self.status_dots_box(cx));
                }
                Topic::Agents => form = form.child(self.agents_box()),
                Topic::Usage => form = form.child(self.usage_agents_box(cx)).child(self.usage_labels_box(cx)),
                _ => {}
            }
        }
        let settings: Vec<wheel_proto::SettingInfo> = self.settings.settings_in(layer, topic).into_iter().filter(|s| s.key != "status.dots").cloned().collect();
        for section in model::grouping(&settings) {
            let mut rows = div().flex().flex_col();
            for (index, setting) in section.settings.iter().enumerate() {
                if index > 0 {
                    rows = rows.child(divider());
                }
                let field = self.settings.field(layer, &setting.key);
                rows = rows.child(self.setting_row(setting, field, cx));
                if layer == Layer::Global && setting.key == "logs.push_url" {
                    rows = rows.child(divider()).child(self.log_shipping_row());
                }
            }
            form = form.child(titled(&section.name, boxed(rows)));
        }
        if layer == Layer::Global && topic == Topic::Keys {
            form = form.child(self.keymap_table());
        }
        form
    }

    /// One key in one layer: its label and field, a clear button when the layer's file sets it,
    /// and under a divider what saving will do or why it was refused, the key's help, any problem
    /// with the file, and the key itself.
    fn setting_row(&mut self, setting: &wheel_proto::SettingInfo, field: Field, cx: &mut Context<Self>) -> gpui::Div {
        let refusal = self.settings.refusals.get(&field).cloned();
        let stored = self.settings.stored(&field);
        let text = self.settings.text(&field);
        let editing = self.settings_edit == Some(SettingsEdit::Field(field.clone()));
        let saving = self.settings.saving.contains(&field);
        let inherited = self.settings.inherited(&field);
        let dirty = self.settings.dirty(&field);
        let help = self.settings.row_help(&field);
        let problem = self.settings.row_problem(&field);
        let scope = match field.layer {
            Layer::Global => "every project".to_owned(),
            Layer::Project => self.spaces.iter().find(|s| Some(&s.id) == self.settings.project.as_ref()).map(|s| s.name.clone()).unwrap_or_else(|| "this project".into()),
        };
        let mut line = div().flex().items_center().gap_3().px_3().py_2()
            .child(div().w(px(96.)).flex_none().overflow_hidden().whitespace_nowrap().child(model::row_label(&setting.key)))
            .child(text_field(&text, &inherited, editing, refusal.is_some(), saving, click(cx, {
                let field = field.clone();
                move |this| this.edit_setting(field.clone())
            })));
        if let Some(agent) = model::home_agent(&setting.key).map(str::to_owned) {
            if let Some(home) = self.settings.home_checks.get(&field) {
                let (glyph, colour, meaning) = model::home_look(home.state);
                line = line.child(div().flex().items_center().gap_1().text_xs().child(div().text_sm().text_color(rgb(colour)).child(glyph)).child(div().text_color(rgb(DIM)).child(meaning)));
            }
            let homes = self.settings.home_lists.get(&agent).cloned().unwrap_or_default();
            if !homes.is_empty() {
                let mut menu = div().flex().flex_col().gap_1().text_xs();
                for home in homes {
                    let (glyph, colour, _) = model::home_look(home.state);
                    let path = home.path.clone();
                    let field = field.clone();
                    menu = menu.child(div().flex().gap_1().cursor_pointer().child(div().text_color(rgb(colour)).child(glyph)).child(model::tilde(&home.path))
                        .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                            this.settings.type_text(&field, &path);
                            this.save_setting(&field);
                        })));
                }
                line = line.child(div().flex_none().child(div().text_xs().text_color(rgb(DIM)).child("found:")).child(menu));
            }
        }
        if stored.is_some() {
            let field_c = field.clone();
            line = line.child(div().px_1().cursor_pointer().text_color(rgb(DIM)).child("⊗")
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    this.clear_setting(&field_c);
                })));
        }
        let mut under = div().flex().flex_col().gap_1().px_3().pt_2().pb_2().text_xs();
        if let Some(refusal) = refusal {
            under = under.child(div().text_color(rgb(RED)).child(format!("✗ {refusal}")));
        } else if dirty {
            under = under.child(div().text_color(rgb(DIM)).child(format!("{} for {scope} · Esc puts it back", if text.trim().is_empty() { "Return clears it" } else { "Return saves" })));
        }
        if let Some(problem) = problem {
            under = under.child(div().text_color(rgb(ORANGE)).child(format!("⚠ {problem}")));
        }
        under = under.child(div().text_color(rgb(DIM)).child(help)).child(div().font_family("Menlo").text_color(rgb(DIM)).child(setting.key.clone()));
        div().flex().flex_col().child(line).child(div().mx_3().h(px(1.)).bg(rgb(LINE))).child(under)
    }

    fn log_shipping_row(&self) -> gpui::Div {
        let logs = self.settings.log_shipping.as_ref();
        let mut col = div().flex().flex_col().gap(px(2.)).child(model::log_shipping_title(logs));
        if let Some(detail) = model::log_shipping_detail(logs) {
            col = col.child(div().text_xs().text_color(rgb(DIM)).child(detail));
        }
        div().flex().items_start().gap_2().px_3().py_2().child(div().text_color(rgb(model::log_shipping_colour(logs))).child("●")).child(col)
    }

    fn command_line_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = &self.settings.command_line;
        let installed = c.state == "installed";
        let mut under = div().flex().flex_col().gap_1().px_3().pt_2().pb_2().text_xs();
        if let Some(problem) = &self.settings.shell_problem {
            under = under.child(div().text_color(rgb(ORANGE)).child(format!("⚠ wheeld: {problem}")));
        }
        if let Some(detail) = &c.detail {
            under = under.child(div().font_family("Menlo").text_color(rgb(DIM)).child(detail.clone()));
        }
        under = under.child(div().text_color(rgb(DIM)).child(c.help()));
        let disabled = c.running || c.state == "unavailable";
        let rows = div().flex().flex_col()
            .child(div().flex().items_center().gap_3().px_3().py_2()
                .child(div().w(px(96.)).child("wheel"))
                .child(div().text_color(rgb(if installed { GREEN } else { ORANGE })).child(c.state.clone()))
                .child(div().flex_1())
                .child(button_state(if installed { "Reinstall" } else { "Install" }, disabled, click(cx, move |this| {
                    if !disabled {
                        this.install_command_line();
                    }
                }))))
            .child(div().mx_3().h(px(1.)).bg(rgb(LINE)))
            .child(under);
        titled("Command line", boxed(rows))
    }

    fn folder_access_box(&self) -> gpui::Div {
        let mut col = div().flex().flex_col().gap_1().px_3().py_2().text_xs();
        match &self.settings.host {
            Some(host) => {
                for folder in &host.folders {
                    let name = folder.path.rsplit('/').next().unwrap_or(&folder.path).to_owned();
                    let (word, colour) = if folder.allowed { ("allowed", GREEN) } else if folder.waiting { ("waiting for permission", ORANGE) } else { ("denied", ORANGE) };
                    col = col.child(div().flex().gap_3().text_sm().child(div().w(px(96.)).child(name)).child(div().text_color(rgb(colour)).child(word)));
                }
                if host.folders.iter().any(|f| !f.allowed && !f.waiting) {
                    col = col.child(div().text_color(rgb(DIM)).child("Terminals and agents in Wheel get “Operation not permitted” there, because macOS charges their access to the server. Switch Wheel Server on under Privacy & Security › Files & Folders, then restart the server so it picks the grant up:"))
                        .child(div().font_family("Menlo").child(host.exe.clone()));
                }
            }
            None => col = col.child(div().text_color(rgb(DIM)).child("Wheel Server has not said yet.")),
        }
        titled("Folder access", boxed(col))
    }

    /// One checkbox per rank, writing the machine-wide `status.dots`.
    fn status_dots_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut rows = div().flex().flex_col();
        for (index, rank) in crate::status::RANKS.iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let on = self.status_dots.contains(rank);
            let title = match *rank {
                "needs_you" => "Needs you",
                "stopped" => "Stopped",
                "working" => "Working",
                "done" => "Done",
                _ => "Idle",
            };
            rows = rows.child(div().flex().items_center().gap_2().px_3().py_2().cursor_pointer().child(checkbox(on)).child(title)
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    let value = model::dots_setting(&this.status_dots, rank);
                    this.write_machine("status.dots", value);
                })));
        }
        rows = rows.child(divider()).child(div().flex().flex_col().gap_1().px_3().py_2().text_xs().text_color(rgb(DIM))
            .child("The statuses that show a dot beside a pane, a space’s sidebar row and its tab.").child(div().font_family("Menlo").child("status.dots")));
        titled("Status dots", boxed(rows))
    }

    /// Each agent pane program, found with its path or missing with why and how to install it.
    fn agents_box(&self) -> gpui::Div {
        let mut rows = div().flex().flex_col();
        let agents = Model::agent_rows(&self.agents);
        if agents.is_empty() {
            rows = rows.child(div().p_3().text_xs().text_color(rgb(DIM)).child("The daemon lists no agents."));
        }
        for (index, (title, command, home, found, right)) in agents.into_iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let mut right_col = div().flex().flex_col().items_end().text_xs().text_color(rgb(DIM)).child(div().text_sm().text_color(rgb(if found { GREEN } else { ORANGE })).child(if found { "found" } else { "missing" }));
            for line in right {
                right_col = right_col.child(line);
            }
            rows = rows.child(div().flex().items_start().gap_3().px_3().py_2()
                .child(div().flex().flex_col().text_xs().text_color(rgb(DIM)).child(div().text_sm().text_color(rgb(TEXT)).child(title)).child(command).child(home))
                .child(div().flex_1())
                .child(right_col));
        }
        titled("Agents", boxed(rows))
    }

    /// Which coding agents show usage, and why an agent cannot (`usage.agents`).
    fn usage_agents_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut rows = div().flex().flex_col();
        for (index, agent) in self.settings.usage.agents.iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let id = agent.id.clone();
            let disabled = !agent.readable && !agent.shown;
            rows = rows.child(div().flex().items_center().gap_2().px_3().py_2().cursor_pointer().child(checkbox(agent.shown)).child(agent.name.clone()).child(div().flex_1())
                .child(div().text_xs().text_color(rgb(DIM)).child(if agent.readable { "readable".to_owned() } else { agent.why.clone().unwrap_or_default() }))
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    if !disabled {
                        let value = model::agents_setting(&this.settings.usage, &id);
                        this.write_machine("usage.agents", value);
                    }
                })));
        }
        if self.settings.usage.agents.is_empty() {
            rows = rows.child(div().p_3().text_xs().text_color(rgb(DIM)).child("wheeld has not listed its usage agents yet."));
        }
        titled("Usage agents", boxed(rows))
    }

    /// Settings › General: Restore windows at launch (WindowsSettings), this Mac's own preference,
    /// kept with the saved windows (saved.rs).
    fn windows_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let on = crate::saved::restores();
        div().flex().flex_col().gap_1()
            .child(div().id("restore-windows").flex().items_center().gap_2().cursor_pointer().child(checkbox(on)).child("Restore windows at launch")
                .on_mouse_down(MouseButton::Left, click(cx, move |_| crate::saved::set_restores(!on))))
            .child(div().text_xs().text_color(rgb(DIM)).child("On, Wheel reopens the windows as you left them. Off, it opens one window holding every space."))
    }

    /// What the strip prints beside each login's bars (`usage.labels`).
    fn usage_labels_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut rows = div().flex().flex_col();
        for (index, (id, title)) in model::USAGE_LABELS.iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let on = self.settings.usage.labels.iter().any(|l| l == id);
            rows = rows.child(div().flex().items_center().gap_2().px_3().py_2().cursor_pointer().child(checkbox(on)).child(*title).child(div().flex_1())
                .child(div().text_xs().text_color(rgb(DIM)).child(if *id == "folder" { "only when an agent has two or more logins" } else { "" }))
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    let value = model::labels_setting(&this.settings.usage, id);
                    this.write_machine("usage.labels", value);
                })));
        }
        titled("Usage labels", boxed(rows))
    }

    /// Under the `keybind` field: what the setting did not apply, then every action with the key
    /// that runs it now, marked where the setting changed it from the built-in one.
    fn keymap_table(&self) -> gpui::Div {
        let mut col = div().flex().flex_col().gap_1();
        for problem in &self.keymap.problems {
            col = col.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("⚠ {problem}")));
        }
        let mut rows = div().flex().flex_col();
        let actions = self.all_actions();
        let defaults = Keymap::new(&crate::actions::defaults(&actions), "");
        for (index, action) in actions.iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let chord = self.keymap.chords.get(&action.id);
            let changed = chord != defaults.chords.get(&action.id);
            rows = rows.child(div().flex().items_center().gap_3().px_3().py_1()
                .child(div().child(action.title.clone()))
                .child(div().flex_1())
                .child(div().text_xs().text_color(rgb(DIM)).child(if changed { "set" } else { "" }))
                .child(div().font_family("Menlo").min_w(px(64.)).flex().justify_end().text_color(rgb(if chord.is_some() { TEXT } else { DIM })).child(chord.map(|c| c.symbol()).unwrap_or_else(|| "—".into())))
                .child(div().w(px(150.)).font_family("Menlo").text_xs().text_color(rgb(DIM)).child(action.id.clone())));
        }
        col.child(titled("Actions", boxed(rows)))
    }

    /// The Chat and Terminal tabs: every key of the topic, drawn from its schema with a control
    /// by kind, over a Machine or a Project file, with a search, a count of what changed, Reset
    /// all and Show wheel.toml (TopicSettings.swift).
    fn chat_like_page(&mut self, topic: Topic, cx: &mut Context<Self>) -> gpui::Div {
        let page = self.settings.page(topic);
        let (layer, show_toml, query) = (page.layer, page.show_toml, page.query.clone());
        let project_name = self.spaces.iter().find(|s| Some(&s.id) == self.settings.project.as_ref()).map(|s| s.name.clone()).unwrap_or_else(|| "none".into());
        let searching = self.settings_edit == Some(SettingsEdit::Search(topic));
        let head = div().flex().items_center().gap_2()
            .child(div().flex_1().child(text_field(&query, "Search settings", searching, false, false, click(cx, move |this| {
                this.settings_edit = Some(SettingsEdit::Search(topic));
            }))))
            .child(segmented(&[("Machine", layer == Layer::Global), (&format!("Project · {project_name}"), layer == Layer::Project)], self.settings.project.is_none(), cx, move |this, index| {
                this.settings.page_mut(topic).layer = if index == 0 { Layer::Global } else { Layer::Project };
            }));
        let matches: Vec<wheel_proto::SettingInfo> = self.settings.topic_rows(topic).into_iter().cloned().collect();
        let body = if matches.is_empty() {
            div().py_6().flex().justify_center().text_color(rgb(DIM)).child(format!("No setting matches “{query}”."))
        } else {
            let mut rows = div().flex().flex_col();
            for (index, setting) in matches.iter().enumerate() {
                if index > 0 {
                    rows = rows.child(divider());
                }
                let field = self.settings.field(layer, &setting.key);
                rows = rows.child(self.topic_row(setting, field, cx));
            }
            boxed(rows)
        };
        let changed: Vec<String> = self.settings.topic_changed(topic).iter().map(|s| s.key.clone()).collect();
        let footer = div().flex().items_center().gap_3().text_xs()
            .child(div().text_color(rgb(DIM)).child(format!("{} changed from the defaults", changed.len())))
            .child(div().flex_1())
            .child(div().flex().items_center().gap_1().cursor_pointer().child(checkbox(show_toml)).child("Show wheel.toml")
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    let p = this.settings.page_mut(topic);
                    p.show_toml = !p.show_toml;
                })))
            .child(button_state("Reset all", changed.is_empty(), click(cx, move |this| {
                for key in &changed {
                    let field = this.settings.field(layer, key);
                    this.clear_setting(&field);
                }
            })));
        let mut form = div().flex().flex_col().gap_3().child(head).child(body).child(footer);
        if show_toml {
            form = form.child(div().p_2().rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(LINE)).font_family("Menlo").text_xs().child(self.settings.topic_toml(topic)));
        }
        self.content(layer, form, cx)
    }

    /// One key: its title and description, the control for its kind, and where the shown file
    /// sets it, with a reset that removes it from that file.
    fn topic_row(&mut self, setting: &wheel_proto::SettingInfo, field: Field, cx: &mut Context<Self>) -> gpui::Div {
        let stored = self.settings.stored(&field);
        let value = stored.clone().unwrap_or_else(|| self.settings.inherited(&field));
        let layer_name = if field.layer == Layer::Global { "this machine" } else { "this project" };
        let control: gpui::Div = match setting.kind {
            SettingKind::Bool => {
                let on = value == "true";
                let f = field.clone();
                div().cursor_pointer().child(switch(on)).on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    this.write_control(&f, if on { "false".into() } else { "true".into() });
                }))
            }
            SettingKind::Choice => {
                let mut row = div().flex().gap_1().flex_wrap().justify_end();
                for choice in &setting.choices {
                    let (c, f) = (choice.clone(), field.clone());
                    let active = *choice == value;
                    row = row.child(div().px_2().py(px(2.)).rounded_sm().cursor_pointer().text_xs().bg(rgb(if active { ACCENT } else { BOX })).child(choice.clone())
                        .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                            this.write_control(&f, c.clone());
                        })));
                }
                row
            }
            SettingKind::Int | SettingKind::Float => {
                let whole = setting.kind == SettingKind::Int;
                let (min, max) = (setting.min, setting.max);
                let editing = self.settings_edit == Some(SettingsEdit::Field(field.clone()));
                let text = self.settings.text(&field);
                let shown = if text.is_empty() && !editing { value.clone() } else { text };
                let step = move |this: &mut Client, f: &Field, delta: f64| {
                    let current = this.settings.stored(f).unwrap_or_else(|| this.settings.inherited(f)).parse::<f64>().unwrap_or(min.unwrap_or(0.));
                    let mut n = current + delta;
                    if let Some(min) = min { n = n.max(min) }
                    if let Some(max) = max { n = n.min(max) }
                    let text = if whole { format!("{}", n.round() as i64) } else { format!("{}", (n * 100.).round() / 100.) };
                    this.write_control(f, text);
                };
                let d = if whole { 1. } else { 0.05 };
                let mut stepper = |label: &str, delta: f64| {
                    let field = field.clone();
                    button(label, click(cx, move |this| step(this, &field, delta)))
                };
                let (minus, plus) = (stepper("−", -d), stepper("+", d));
                let edit = click(cx, {
                    let field = field.clone();
                    move |this| this.edit_setting(field.clone())
                });
                div().flex().items_center().gap_1().child(div().w(px(64.)).child(text_field(&shown, "", editing, false, false, edit))).child(minus).child(plus)
            }
            SettingKind::Ordered => {
                let words = setting.choices.clone();
                let on: Vec<String> = value.split(',').map(str::to_owned).filter(|w| words.contains(w)).collect();
                let mut rows: Vec<String> = on.clone();
                rows.extend(words.iter().filter(|w| !on.contains(w)).cloned());
                let mut list = div().flex().flex_col().gap(px(2.)).text_xs();
                for word in rows {
                    let is_on = on.contains(&word);
                    let at = on.iter().position(|w| *w == word);
                    let up = at.filter(|i| *i > 0);
                    let down = at.filter(|i| i + 1 < on.len());
                    let toggled = word.clone();
                    let toggle = write_order(cx, &field, &on, move |l| if is_on { l.retain(|w| *w != toggled) } else { l.push(toggled.clone()) });
                    let move_up = write_order(cx, &field, &on, move |l| if let Some(i) = up { l.swap(i, i - 1) });
                    let move_down = write_order(cx, &field, &on, move |l| if let Some(i) = down { l.swap(i, i + 1) });
                    list = list.child(div().flex().items_center().gap_2()
                        .child(div().cursor_pointer().child(checkbox(is_on)).on_mouse_down(MouseButton::Left, toggle))
                        .child(div().w(px(90.)).child(word))
                        .child(button_state("↑", up.is_none(), move_up))
                        .child(button_state("↓", down.is_none(), move_down)));
                }
                list
            }
            SettingKind::Text => {
                let editing = self.settings_edit == Some(SettingsEdit::Field(field.clone()));
                let text = self.settings.text(&field);
                let shown = if text.is_empty() && !editing { value.clone() } else { text };
                let f = field.clone();
                div().w(px(180.)).child(text_field(&shown, "", editing, self.settings.refusals.contains_key(&field), false, click(cx, move |this| {
                    this.edit_setting(f.clone());
                })))
            }
        };
        let f = field.clone();
        let reset = div().px_1().cursor_pointer().text_color(rgb(if stored.is_some() { TEXT } else { 0x00000000 })).child("↶")
            .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                if this.settings.stored(&f).is_some() {
                    this.clear_setting(&f);
                }
            }));
        let mut col = div().flex().flex_col().gap_1().px_3().py_2()
            .child(div().flex().items_start().gap_3()
                .child(div().flex_1().flex().flex_col().child(div().font_weight(FontWeight::MEDIUM).child(model::topic_title(&setting.key))).child(div().text_xs().text_color(rgb(DIM)).child(setting.help.clone())))
                .child(control)
                .child(reset));
        if stored.is_some() {
            col = col.child(div().text_xs().text_color(rgb(DIM)).child(format!("Set in {layer_name}")));
        }
        if field.layer == Layer::Global && setting.project.is_some() {
            col = col.child(div().text_xs().text_color(rgb(ORANGE)).child("The project sets this too, and its value wins."));
        }
        if let Some(problem) = &setting.problem {
            col = col.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("⚠ {problem}")));
        }
        if let Some(refusal) = self.settings.refusals.get(&field) {
            col = col.child(div().text_xs().text_color(rgb(RED)).child(format!("✗ {refusal}")));
        }
        col
    }

    /// Settings › Artifacts: one row per built-in with its versions and its one action, and
    /// Remove when it is installed.
    fn builtins_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let b = &self.settings.builtins;
        let mut col = div().flex().flex_col().gap_2()
            .child(div().text_xs().text_color(rgb(DIM)).child("Pages Wheel ships, installed for every project on this Mac into its artifacts folder. Installed pages are plain files you can edit; an update asks before it changes an edited one."));
        if let Some(problem) = &b.problem {
            col = col.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("⚠ {problem}")));
        }
        if !b.loaded {
            col = col.child(div().text_xs().text_color(rgb(DIM)).child("Asking wheeld for its built-ins…"));
        }
        let mut rows = div().flex().flex_col();
        for (index, builtin) in b.builtins.iter().enumerate() {
            if index > 0 {
                rows = rows.child(divider());
            }
            let name = builtin.name.clone();
            let mut left = div().flex().flex_col().child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone())).child(div().text_xs().text_color(rgb(DIM)).child(model::builtin_versions(builtin)));
            if !builtin.edited_files.is_empty() {
                left = left.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("Edited: {}", builtin.edited_files.join(", "))));
            }
            let mut row = div().flex().items_center().gap_2().px_3().py_2().child(left).child(div().flex_1());
            let action = model::builtin_action(builtin);
            let n = name.clone();
            match action {
                BuiltinAction::Install => row = row.child(button("Install", click(cx, move |this| { this.install_builtin(&n); }))),
                BuiltinAction::Update => row = row.child(button("Update", click(cx, move |this| { this.update_builtin(&n, OnEdited::Ask); }))),
                BuiltinAction::Restore => row = row.child(button("Restore…", click(cx, move |this| { this.update_builtin(&n, OnEdited::Ask); }))),
                BuiltinAction::UpToDate => {}
            }
            if builtin.installed_version.is_some() {
                let (n, edited) = (name.clone(), builtin.edited_files.clone());
                row = row.child(button("Remove", click(cx, move |this| {
                    this.ask_remove_builtin(&n, &edited);
                })));
            }
            rows = rows.child(row);
        }
        col.child(boxed(rows))
    }

    /// The files an update would change that a person edited, each with its diff, and the choice.
    fn edited_sheet(&self, name: &str, files: &[wheel_proto::EditedFile], cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_2();
        for file in files {
            list = list.child(div().font_weight(FontWeight::SEMIBOLD).child(file.path.clone()))
                .child(div().p_2().rounded_sm().bg(rgb(0x0e1311)).font_family("Menlo").text_xs().child(file.diff.clone()));
        }
        let (n1, n2) = (name.to_owned(), name.to_owned());
        div().flex().flex_col().gap_2().p_3().border_t_1().border_color(rgb(LINE)).max_h(px(300.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("You edited {} in “{name}”", if files.len() == 1 { "a file".to_owned() } else { format!("{} files", files.len()) })))
            .child(div().text_xs().text_color(rgb(DIM)).child(format!("The update changes {} as shown. Keep your edits, or replace them with the new version.", if files.len() == 1 { "it" } else { "them" })))
            .child(div().id("edited-files").overflow_y_scroll().flex_1().min_h_0().child(list))
            .child(div().flex().justify_end().gap_2()
                .child(button("Cancel", click(cx, |this| { this.settings.builtins.edited = None; })))
                .child(button("Keep My Edits", click(cx, move |this| { this.update_builtin(&n1, OnEdited::Keep); })))
                .child(button("Replace", click(cx, move |this| { this.update_builtin(&n2, OnEdited::Replace); }))))
    }

    /// Settings › Guidance in two columns: the documents on the left, what one agent is given on
    /// the right.
    fn guidance_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.link != Link::Up {
            return notice("Not connected to wheeld.", Some("Guidance lives in files the server reads; it shows here once the server answers."));
        }
        let g = &self.settings.guidance;
        let mut col = div().flex().flex_col().gap_3()
            .child(div().text_xs().text_color(rgb(DIM)).child("Guidance is text Wheel gives every agent when its pane starts. Wheel carries it as you wrote it and enforces nothing. An agent keeps the guidance it started with; a pane started or restarted after an edit reads the new text."));
        if let Some(error) = &g.error {
            col = col.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("⚠ {error}")));
        }
        if !g.loaded() {
            if g.error.is_none() {
                col = col.child(div().text_color(rgb(DIM)).child("Asking wheeld for its guidance…"));
            }
            return col;
        }
        let tint = |id: &str| match id {
            "wheel" => 0x8fa89b,
            "all" => 0x6fa8dc,
            _ => GREEN,
        };
        let summary = |file: &wheel_proto::GuidanceFile| {
            let lines = file.text.split('\n').count();
            if file.text.trim().is_empty() { "Empty. Adds nothing.".to_owned() } else { format!("{lines} {}", if lines == 1 { "line" } else { "lines" }) }
        };
        let mut docs = div().flex().flex_col().gap_1().w(px(250.)).flex_none().child(div().font_weight(FontWeight::SEMIBOLD).child("Documents"));
        let row = |id: String, title: String, meta: String, path: Option<String>, agent: Option<String>, editable: bool, cx: &mut Context<Self>| {
            let selected = g.focus.as_deref() == Some(id.as_str()) || (agent.is_some() && agent.as_deref() == Some(g.preview_agent.as_str()) && g.focus.is_none());
            let mut r = div().flex().flex_col().gap(px(2.)).p_2().rounded_sm().cursor_pointer().border_1().border_color(rgb(if selected { tint(&id) } else { LINE }))
                .child(div().flex().items_center().gap_1().child(div().text_color(rgb(tint(&id))).child("●")).child(div().flex_1().font_weight(FontWeight::MEDIUM).child(title)))
                .child(div().text_xs().text_color(rgb(DIM)).child(meta));
            if let Some(path) = &path {
                r = r.child(div().text_xs().text_color(rgb(DIM)).overflow_hidden().whitespace_nowrap().child(model::tilde(path)));
            }
            if editable {
                let a = agent.clone();
                r = r.child(div().flex().justify_end().child(button("Open in Editor", click(cx, move |this| {
                    this.open_guidance_file(a.as_deref());
                }))));
            }
            let (id_c, agent_c) = (id.clone(), agent.clone());
            r.on_mouse_down(MouseButton::Left, click(cx, move |this| {
                if let Some(agent) = &agent_c {
                    this.preview_guidance(agent);
                }
                this.settings.guidance.focus = Some(id_c.clone());
            }))
        };
        docs = docs.child(row("wheel".into(), "Wheel’s guidance".into(), "Ships with Wheel. Read-only.".into(), None, None, false, cx));
        if let Some(all) = g.files.iter().find(|f| f.agent.is_none()) {
            docs = docs.child(row("all".into(), "Your guidance for all agents".into(), summary(all), Some(all.path.clone()), None, true, cx));
        }
        docs = docs.child(div().text_xs().text_color(rgb(DIM)).pt_1().child("Per agent"));
        for file in g.files.iter().filter(|f| f.agent.is_some()) {
            let name = file.agent.clone().unwrap_or_default();
            docs = docs.child(row(name.clone(), name.clone(), summary(file), Some(file.path.clone()), Some(name.clone()), true, cx));
        }
        let mut preview = div().flex_1().flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_1().font_weight(FontWeight::SEMIBOLD).child("What").child(div().px_2().rounded_md().bg(rgba(0x6fcf972e)).child(g.preview_agent.clone())).child("is given"))
            .child(div().text_xs().text_color(rgb(DIM)).child("Read-only: Wheel’s guidance, then yours for every agent, then yours for this agent. Press an agent on the left to see its text. The pane’s artifacts and seat line are added when the pane starts."));
        for segment in g.segments() {
            let empty = segment.text.is_empty();
            preview = preview.child(div().flex().flex_col().gap_1().p_2().rounded_sm().border_1().border_color(rgb(if g.focus.as_deref() == Some(segment.id.as_str()) { tint(&segment.id) } else { LINE }))
                .child(div().flex().items_center().gap_1().text_xs().text_color(rgb(DIM)).child(div().text_color(rgb(tint(&segment.id))).child("●")).child(segment.title.clone()))
                .child(if empty { div().text_xs().text_color(rgb(DIM)).child("Empty. This file adds nothing.") } else { div().font_family("Menlo").text_xs().child(segment.text.clone()) }));
        }
        col.child(div().flex().items_start().gap_4().child(docs).child(preview))
    }

    /// The Project tab: every project down the side, a topic picker, and the project's rows.
    fn project_page(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        if self.link == Link::Up && self.spaces.is_empty() {
            return notice("No projects yet.", Some("A project’s settings live in the wheel.toml at its folder’s root. Open a folder as a space to set them here."));
        }
        let mut list = div().flex().flex_col().gap_1().w(px(190.)).flex_none().child(div().text_xs().text_color(rgb(DIM)).child("Projects"));
        for space in &self.spaces {
            let selected = self.settings.project.as_ref() == Some(&space.id);
            let id = space.id.clone();
            list = list.child(div().flex().flex_col().p_2().rounded_sm().cursor_pointer().bg(rgb(if selected { ACCENT } else { BOX }))
                .child(space.name.clone()).child(div().text_xs().text_color(rgb(DIM)).overflow_hidden().whitespace_nowrap().child(model::tilde(&space.cwd)))
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    this.select_settings_project(&id);
                })));
        }
        let topics: Vec<Topic> = Topic::ALL.into_iter().filter(|t| *t != Topic::Chat && *t != Topic::Terminal && !self.settings.settings_in(Layer::Project, *t).is_empty()).collect();
        let picked = self.settings.project_topic;
        let labels: Vec<(String, bool)> = topics.iter().map(|t| (t.title().to_owned(), *t == picked)).collect();
        let picker = segmented(&labels.iter().map(|(l, a)| (l.as_str(), *a)).collect::<Vec<_>>(), false, cx, move |this, index| {
            if let Some(t) = topics.get(index) {
                this.settings.project_topic = *t;
            }
        });
        let form = self.settings_form(Layer::Project, picked, cx);
        let content = self.content(Layer::Project, form, cx);
        div().flex().items_start().gap_4().child(list).child(div().flex_1().flex().flex_col().gap_3().child(picker).child(content))
    }

    /// Settings › Devices: the addresses another device can dial, the port, and a QR code holding
    /// the URL, token and certificate fingerprint a phone needs to pair; then the paired devices.
    fn devices_page(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        if self.link != Link::Up {
            return notice("wheeld unreachable", Some("Devices come from the daemon; they show here once it answers."));
        }
        let now = now_ms();
        let qr_rows = self.settings.devices.pairing_json(now).map(|payload| self.qr_rows(&payload));
        let d = &self.settings.devices;
        let Some(info) = &d.info else { return notice("Asking wheeld how devices reach it…", None) };
        if let Some(problem) = &info.problem {
            return notice("Remote access did not start", Some(problem));
        }
        if info.listen.is_none() {
            return div().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Remote access is off"))
                .child(div().text_xs().text_color(rgb(DIM)).child("Add this to this machine’s wheel.toml, or set remote.listen on the General tab, then restart wheeld:"))
                .child(div().p_2().rounded_sm().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).font_family("Menlo").child("[remote]\nlisten = \"0.0.0.0:7430\""));
        }
        let mut addresses = div().flex().gap_1().flex_wrap();
        for address in &info.addresses {
            let active = d.address() == Some(address.as_str());
            let a = address.clone();
            addresses = addresses.child(div().px_2().py(px(2.)).rounded_sm().cursor_pointer().text_xs().bg(rgb(if active { ACCENT } else { BOX })).child(address.clone())
                .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                    this.settings.devices.picked = Some(a.clone());
                })));
        }
        let row = |label: &str, value: String| div().flex().gap_2().text_xs().child(div().w(px(84.)).text_color(rgb(DIM)).child(label.to_owned())).child(div().font_family("Menlo").child(value));
        let mut left = div().flex_1().flex().flex_col().gap_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Pair a device"))
            .child(div().text_xs().text_color(rgb(DIM)).child("Address in the code"))
            .child(addresses)
            .child(row("Port", info.port.to_string()))
            .child(row("Listening on", info.listen.clone().unwrap_or_default()))
            .child(row("Fingerprint", info.fingerprint.clone()));
        let mut make = div().flex().items_center().gap_2().child(button(if d.live_token(now).is_none() { "Make a pairing code" } else { "Make another code" }, click(cx, |this| {
            this.request_pairing_code();
        })));
        let live_code_expires = d.code.as_ref().map(|(_, expires)| *expires).filter(|expires| *expires > now);
        if let Some(expires) = live_code_expires {
            let secs = (expires - now) / 1000;
            make = make.child(div().text_xs().text_color(rgb(DIM)).child(format!("Works once, for {}:{:02} more.", secs / 60, secs % 60)));
        }
        make = make.child(button("Pair a device…", click(cx, |this| this.show_pairing_window())));
        left = left.child(make);
        let qr: gpui::Div = match qr_rows {
            Some(rows) if rows.is_empty() => div().w(px(220.)).text_xs().text_color(rgb(ORANGE)).child("The bridge could not draw the code."),
            Some(rows) => {
                let n = rows.len().max(1);
                let cell = (220. / n as f32).floor().max(1.);
                let mut grid = div().flex().flex_col().p_2().bg(rgb(0xffffff)).rounded_sm();
                for r in rows {
                    let mut line = div().flex();
                    for ch in r.chars() {
                        line = line.child(div().w(px(cell)).h(px(cell)).bg(rgb(if ch == '1' { 0x000000 } else { 0xffffff })));
                    }
                    grid = grid.child(line);
                }
                grid
            }
            None => div().w(px(220.)).text_xs().text_color(rgb(DIM)).child(format!("No pairing code: {}", d.unavailable())),
        };
        let mut paired = div().flex().flex_col().gap_1().child(div().font_weight(FontWeight::SEMIBOLD).child("Paired devices"));
        if d.paired.is_empty() {
            paired = paired.child(div().text_xs().text_color(rgb(DIM)).child("None yet. Scan the code from the iPhone app."));
        }
        for device in &d.paired {
            let id = device.id.clone();
            let seen = if device.connected > 0 { "Connected".to_owned() } else { format!("Last seen {}", ago(now.saturating_sub(device.last_seen_ms))) };
            paired = paired.child(div().flex().items_center().gap_2().py_1().border_b_1().border_color(rgb(LINE))
                .child(div().flex().flex_col().child(device.name.clone()).child(div().text_xs().text_color(rgb(DIM)).child(seen)))
                .child(div().flex_1())
                .child(button("Revoke", click(cx, move |this| {
                    this.revoke_device(&id);
                }))));
        }
        if live_code_expires.is_some() {
            self.redraw_in(1000, cx);
        }
        div().flex().flex_col().gap_4().child(div().flex().items_start().gap_4().child(left).child(qr)).child(paired)
    }
}

fn ago(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86_400)
    }
}

/// The `wheel` beside this program (`Contents/Helpers/wheel` in the app, or a sibling), when the
/// build carries one.
fn bundled_wheel() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("../Helpers/wheel"), dir.join("wheel")].into_iter().find(|p| p.is_file())
}

fn notice(title: &str, detail: Option<&str>) -> gpui::Div {
    let mut col = div().flex().flex_col().items_center().gap_1().py_12().child(title.to_owned());
    if let Some(detail) = detail {
        col = col.child(div().text_xs().text_color(rgb(DIM)).child(detail.to_owned()));
    }
    col
}

fn titled(title: &str, body: gpui::Div) -> gpui::Div {
    div().flex().flex_col().gap_1().child(div().font_weight(FontWeight::SEMIBOLD).child(title.to_owned())).child(body)
}

fn boxed(rows: gpui::Div) -> gpui::Div {
    div().rounded_md().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).overflow_hidden().child(rows)
}

fn divider() -> gpui::Div {
    div().h(px(1.)).bg(rgb(LINE))
}

pub(super) fn checkbox(on: bool) -> gpui::Div {
    div().w(px(14.)).h(px(14.)).rounded_sm().border_1().border_color(rgb(0x5a7266)).bg(rgb(if on { 0x7fb89a } else { PANEL })).flex().items_center().justify_center().text_xs().text_color(rgb(0x0e1311)).child(if on { "✓" } else { "" })
}

fn switch(on: bool) -> gpui::Div {
    div().w(px(30.)).h(px(16.)).rounded_full().bg(rgb(if on { 0x7fb89a } else { 0x3d5247 })).flex().items_center().child(div().w(px(12.)).h(px(12.)).rounded_full().bg(rgb(0xf0f0f0)).ml(px(if on { 16. } else { 2. })))
}

/// A press handler that runs `act`, stops the press reaching what lies beneath and redraws.
pub(super) fn click(cx: &mut Context<Client>, act: impl Fn(&mut Client) + 'static) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _, _, cx| {
        act(this);
        cx.stop_propagation();
        cx.notify();
    })
}

/// A press handler that rewrites an ordered setting's list of chosen words with `edit`.
fn write_order(cx: &mut Context<Client>, field: &Field, on: &[String], edit: impl Fn(&mut Vec<String>) + 'static) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let (field, on) = (field.clone(), on.to_vec());
    click(cx, move |this| {
        let mut words = on.clone();
        edit(&mut words);
        this.write_control(&field, words.join(","));
    })
}

pub(super) fn button(label: &str, on: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> gpui::Div {
    button_state(label, false, on)
}

fn button_state(label: &str, disabled: bool, on: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> gpui::Div {
    div().px_2().py(px(2.)).rounded_sm().text_xs().bg(rgb(if disabled { BOX } else { ACCENT })).text_color(rgb(if disabled { DIM } else { TEXT })).cursor_pointer().child(label.to_owned()).on_mouse_down(MouseButton::Left, on)
}

/// A one-line field: its text, or its placeholder greyed and italic when it is empty; a caret while
/// it is edited, a red border when refused.
fn text_field(text: &str, placeholder: &str, editing: bool, refused: bool, saving: bool, on: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> gpui::Div {
    let empty = text.is_empty();
    let mut f = div().flex_1().min_w(px(0.)).px_2().py(px(3.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(if refused { RED } else if editing { 0x7fb89a } else { LINE }))
        .overflow_hidden().whitespace_nowrap().flex().items_center().cursor_text().on_mouse_down(MouseButton::Left, on);
    if saving {
        f = f.text_color(rgb(DIM));
    }
    if empty && !editing {
        f = f.child(div().text_color(rgb(0x5a7266)).italic().child(placeholder.to_owned()));
    } else {
        f = f.child(text.to_owned());
    }
    if editing {
        f = f.child(div().w(px(1.)).h(px(14.)).bg(rgb(TEXT)));
    }
    f
}

/// A row of chips, one lit; `pick` gets the index pressed.
fn segmented(items: &[(&str, bool)], disabled: bool, cx: &mut Context<Client>, pick: impl Fn(&mut Client, usize) + 'static + Clone) -> gpui::Div {
    let mut row = div().flex().gap_1().flex_none();
    for (index, (label, active)) in items.iter().enumerate() {
        let pick = pick.clone();
        row = row.child(div().px_2().py(px(2.)).rounded_sm().text_xs().cursor_pointer().bg(rgb(if *active { ACCENT } else { BOX })).text_color(rgb(if disabled { DIM } else { TEXT })).child(label.to_string())
            .on_mouse_down(MouseButton::Left, click(cx, move |this| {
                if !disabled {
                    pick(this, index);
                }
            })));
    }
    row
}
