//! The notices over the window (Toasts.swift), the pairing window (PairingWindow.swift) and the
//! card that stands in for a quarantined pane (QuarantineCard.swift), as the client draws them;
//! the models are `crate::toasts`, `crate::pairing` and `crate::crash_guard`.
use super::*;
use super::settings::{BOX, DIM, LINE, PANEL, TEXT, button, checkbox, click, now_ms};
use crate::{pairing::{self, Pairing}, toasts::{Act, Dismiss, LIMIT, Level, Toast, stack}};

/// What a server command's thread hands back: the command, the version a rollback ran, and the
/// script's last line or why it failed.
pub struct ServerOutcome {
    pub op: crate::server::Op,
    pub target: Option<String>,
    pub result: Result<String, String>,
}

impl Client {
    // MARK: Notices

    /// Every notice now, in the order they stack from the top.
    pub fn toasts(&self) -> Vec<Toast> {
        stack(&self.toast_sources, &self.posted, |pane| self.spaces.iter().any(|s| leaves_of(&s.root).iter().any(|l| l.id == pane)))
    }
    /// Show `toast`, replacing one with the same id.
    pub fn post(&mut self, toast: Toast) {
        self.log(format!("toast {} posted: {}, {} actions", toast.id, toast.level.name(), toast.actions.len()));
        crate::toasts::post(&mut self.posted, toast);
    }
    /// A line for a check to read; the last 200 are kept, as the Mac app's and the bridge's logs are.
    pub fn log(&mut self, line: String) {
        self.toast_log.push(line);
        if self.toast_log.len() > 200 {
            self.toast_log.drain(..self.toast_log.len() - 200);
        }
    }
    pub fn remove_toast(&mut self, id: &str) {
        self.posted.retain(|t| t.id != id);
    }
    /// The close button: whatever the toast does on close, then it is gone.
    pub fn dismiss_toast(&mut self, toast: &Toast) {
        self.log(format!("toast {} dismissed", toast.id));
        match &toast.dismiss {
            Dismiss::Posted => {}
            Dismiss::ClientUpdate(stamp) => self.toast_sources.client_update_dismissed = Some(*stamp),
            Dismiss::ServerUpdate(key) => {
                self.toast_sources.server_update_dismissed = Some(key.clone());
                self.log(format!("server banner dismissed for {key}"));
            }
            Dismiss::RolledBack => self.server_watch.rolled_back_to = None,
            Dismiss::AgentNotice(agent) => self.send(&ClientMsg::AgentNoticeDismiss { req: None, agent: agent.clone() }),
            Dismiss::LayoutError => self.toast_sources.layout_error = None,
        }
        self.remove_toast(&toast.id);
    }
    pub fn run_toast_action(&mut self, act: &Act) {
        match act {
            Act::Relaunch => self.relaunch_client(),
            Act::RestartServer => self.restart_server(),
            Act::CancelRollback => {
                self.log("server: rollback cancelled".into());
                self.server_watch.cancel_rollback();
            }
            Act::FocusPane(pane) => self.show_pane(pane),
            Act::ClosePane(pane) => self.ask_close_pane(pane),
        }
    }
    /// Show the space holding `pane` and give it the keyboard.
    pub fn show_pane(&mut self, pane: &str) {
        if let Some(space) = self.spaces.iter().find(|s| leaves_of(&s.root).iter().any(|l| l.id == pane)).map(|s| s.id.clone()) {
            self.show_space_anywhere(&space);
        }
        self.focus_pane(pane);
    }
    /// A layout op this window sends carries a request id, so wheeld's refusal of it becomes the
    /// layout-error notice; sending one clears the last.
    pub fn send_layout(&mut self, make: impl FnOnce(u64) -> ClientMsg) {
        self.send_layout_req(make);
    }
    /// `send_layout`, returning the request id so its answer can be told apart.
    pub fn send_layout_req(&mut self, make: impl FnOnce(u64) -> ClientMsg) -> u64 {
        self.toast_sources.layout_error = None;
        let req = self.req();
        self.layout_reqs.insert(req);
        self.send(&make(req));
        req
    }
    /// A message the stack is built from. Returns whether it was one.
    pub fn on_toast_message(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Toast { id, level, text, pane } => {
                self.post(Toast::posted(format!("notify:{id}"), Level::of(level), text.clone(), pane.clone()));
            }
            ServerMsg::ToastClear { id } => self.remove_toast(&format!("notify:{id}")),
            ServerMsg::AgentNotice { agent, notice } => match notice {
                Some(notice) => {
                    let pane = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| l.agent.as_deref() == Some(agent)).map(|l| l.id.clone());
                    let mut toast = Toast::posted(format!("chat:{agent}"), Level::Warning, format!("{agent}: {notice}"), pane);
                    toast.dismiss = Dismiss::AgentNotice(agent.clone());
                    self.post(toast);
                }
                None => self.remove_toast(&format!("chat:{agent}")),
            },
            ServerMsg::PermRequested { perm } => {
                if !self.perms.iter().any(|p| p.id == perm.id) {
                    self.perms.push(perm.clone());
                }
            }
            ServerMsg::PermResolved { id, .. } => self.perms.retain(|p| &p.id != id),
            ServerMsg::DeviceConnected { name, address, paired, .. } => {
                self.pairing.device_connected(now_ms(), name, address, *paired);
                self.log(format!("device {name} connected from {address}, paired={paired}"));
            }
            ServerMsg::Error { req: Some(req), message, .. } if self.layout_reqs.remove(req) => {
                self.toast_sources.layout_error = Some(message.clone());
            }
            ServerMsg::Ok { req } if self.layout_reqs.remove(req) => {}
            _ => return false,
        }
        true
    }

    // MARK: The server

    /// The source hash of the wheeld this app carries; None for a bare binary or another daemon.
    pub fn bundled_source(&self) -> Option<&str> {
        self.server_control.service().and_then(|s| s.bundled_source.as_deref())
    }
    /// The running wheeld's source hash, from its `status`; None before it answered one.
    pub fn running_source(&self) -> Option<&str> {
        self.settings.host.as_ref().map(|h| h.source.as_str())
    }
    /// A newer server is installed and waits for a restart (AppModel.serverRestartWaiting). The
    /// app never restarts it itself (ADR-0020); the notice, Settings and Restart Server follow this.
    pub fn restart_waiting(&self) -> bool {
        crate::server::restart_waiting(self.link == Link::Up, self.running_source(), self.bundled_source(), self.server_protocol, crate::daemon::PROTOCOL)
    }
    /// What a dismissed notice is about (AppModel.serverBannerKey).
    fn server_banner_key(&self) -> String {
        crate::server::banner_key(self.bundled_source(), self.server_protocol)
    }
    /// The header's word and dot colour for the server (AppModel.serverStatus).
    pub fn server_status_word(&self) -> crate::server::Status {
        let link = match self.link {
            Link::Connecting => crate::server::LinkKind::Connecting,
            Link::Up | Link::Incompatible { .. } => crate::server::LinkKind::Connected,
            Link::Down(_) => crate::server::LinkKind::Unreachable,
        };
        let remote = self.settings.devices.info.as_ref().map(|i| crate::server::Remote { problem: i.problem.is_some(), listening: i.listen.is_some() });
        crate::server::status(link, self.restart_waiting() || matches!(self.link, Link::Incompatible { .. }), remote)
    }
    /// After each `status`: the notice follows whether a restart waits, and one line per connection
    /// says whether the running server is the bundled one (AppModel.logServerSource).
    pub fn note_server_source(&mut self) {
        let waiting = self.restart_waiting();
        self.toast_sources.server_updated = waiting.then(|| self.server_banner_key());
        if let Some(service) = self.server_control.service() {
            let line = crate::server::source_line(&service.label, self.running_source(), self.bundled_source(), waiting);
            if self.server_source_logged.as_deref() != Some(&line) {
                self.server_source_logged = Some(line.clone());
                self.log(line);
            }
        }
    }
    /// Copy this app's server out of the bundle and load its launch agent unless one runs
    /// (DaemonService.register): once per process, at launch, as the Swift app does. A running
    /// server is never touched (ADR-0020).
    pub fn register_server(&mut self) {
        static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.server_control.service().is_some() && !REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.run_server_op(crate::server::Op::Start, None);
        }
    }
    /// Restart wheeld from what is installed, in one click (ServerControl.swift `restartServer`).
    /// The command runs on a thread; the next tick reads how it ended.
    pub fn restart_server(&mut self) {
        if !self.server_control.available() || self.server_busy.is_some() {
            return;
        }
        self.log(format!("server: restart asked for; {} spaces", self.spaces.len()));
        self.run_server_op(crate::server::Op::Restart, None);
    }
    /// Run `op` on a thread; `target` is the version a rollback runs, for its log line. One
    /// command at a time: every caller checks `server_busy` first.
    fn run_server_op(&mut self, op: crate::server::Op, target: Option<String>) {
        debug_assert!(self.server_busy.is_none(), "a server command is already running");
        self.server_busy = Some(op);
        let (control, slot) = (self.server_control.clone(), self.server_ops.clone());
        std::thread::spawn(move || {
            let result = control.run(op);
            slot.lock().unwrap().push(ServerOutcome { op, target, result });
        });
    }
    /// A finished command, read into the model (the Swift completion handlers of DaemonService
    /// and ServerControl).
    fn finish_server_op(&mut self, done: ServerOutcome, now: u64, connected: bool) {
        use crate::server::Op;
        let ServerOutcome { op, target, result } = done;
        let label = self.server_control.service().map(|s| s.label.clone()).unwrap_or_else(|| "server".into());
        let line = result.as_ref().map(String::as_str).unwrap_or_else(|why| why.as_str()).to_owned();
        match op {
            Op::Start => {
                self.service_status = match &result {
                    Ok(l) if l.is_empty() => "started".into(),
                    Ok(l) => l.clone(),
                    Err(why) => format!("start failed: {why}"),
                };
                self.log(format!("daemon {label}: {line}"));
                self.server_problem = result.err();
            }
            Op::Restart => {
                self.service_status = match &result { Ok(_) => "restarted from the app".into(), Err(f) => format!("restart failed: {f}") };
                match &result {
                    Ok(l) if l.is_empty() => {}
                    Ok(_) => self.log(format!("daemon {label}: {line}")),
                    Err(_) => {}
                }
                self.log(match &result { Ok(_) => "server: restarted from the app".into(), Err(f) => format!("server: restart failed: {f}") });
                self.server_problem = result.err();
            }
            Op::RollbackTarget => {
                self.log(format!("daemon {label}: rollbackable: {}", match &result { Ok(t) => t.clone(), Err(why) => format!("no ({why})") }));
                if let Some(line) = self.server_watch.target_answered(now, result, connected) {
                    self.log(line);
                }
            }
            Op::Rollback => {
                let target = target.unwrap_or_default();
                self.service_status = match &result { Ok(_) => "rolled back from the app".into(), Err(f) => format!("rollback failed: {f}") };
                self.server_problem = result.as_ref().err().cloned();
                let line = self.server_watch.rollback_ended(&target, result);
                self.log(line);
            }
        }
    }
    /// Each render and each timer: finished commands are read into the model, the restart window
    /// and the rollback countdown advance (only between commands, so one script runs at a time),
    /// the notices follow, and a timer is armed for the next deadline or while a command runs.
    pub fn tick_server(&mut self, cx: &mut Context<Self>) {
        use crate::server::{Op, Step};
        let now = now_ms();
        let connected = self.link == Link::Up;
        let done: Vec<ServerOutcome> = self.server_ops.lock().unwrap().drain(..).collect();
        for outcome in done {
            self.server_busy = None;
            self.finish_server_op(outcome, now, connected);
        }
        if self.server_busy.is_none() {
            let (step, log) = self.server_watch.tick(now, connected, self.server_control.available());
            for line in log {
                self.log(line);
            }
            match step {
                Some(Step::AskRollbackTarget) => self.run_server_op(Op::RollbackTarget, None),
                Some(Step::Rollback(target)) => self.run_server_op(Op::Rollback, Some(target)),
                None => {}
            }
        }
        self.toast_sources.rollback_offer = self.server_watch.rollback_offer.clone();
        self.toast_sources.rolled_back_to = if connected { self.server_watch.rolled_back_to.clone() } else { None };
        self.toast_sources.unreachable = match &self.link {
            Link::Down(reason) if !self.server_watch.restart_expected => Some(reason.clone()),
            _ => None,
        };
        if self.server_busy.is_some() || self.server_watch.rollback_offer.is_some() {
            self.redraw_in(200, cx);
        } else if let Some(due) = self.server_watch.next_due() {
            self.redraw_in(due.saturating_sub(now).max(1), cx);
        }
    }
    /// Settings › General's Server box and the window's notice while a restart waits
    /// (ServerSourceSettings): the words, Restart Server when the client has commands for it, and
    /// why the last restart did not work.
    pub(super) fn server_box(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut col = div().flex().flex_col().gap_2().p(px(10.)).rounded_md().bg(rgb(BOX)).border_1().border_color(rgb(LINE))
            .child(div().text_color(rgb(crate::server::ORANGE)).child(crate::server::UPDATED));
        if self.server_control.available() {
            col = col.child(div().flex().child(button(if self.server_busy == Some(crate::server::Op::Restart) { "Restarting…" } else { "Restart Server" }, click(cx, |this| {
                this.restart_server();
            }))));
        }
        if let Some(problem) = &self.server_problem {
            col = col.child(div().text_xs().text_color(rgb(0xff453a)).child(problem.clone()));
        }
        col
    }

    // MARK: The client on disk

    /// The app bundle this executable runs from (`<bundle>.app/Contents/MacOS/<exe>`); a bare
    /// cargo binary has none, and is neither watched for an install nor relaunched.
    fn bundle() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe.ancestors().nth(3)?;
        (bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
    }
    /// When the running executable was last written (unix seconds). An install replaces the
    /// bundle, so this changing means the app on disk is not the one running.
    pub fn executable_stamp() -> Option<u64> {
        Self::bundle()?;
        let exe = std::env::current_exe().ok()?;
        let modified = std::fs::metadata(exe).ok()?.modified().ok()?;
        Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
    }
    /// Compare the executable on disk with the one this process started from, at most every 30 s.
    pub fn check_client_update(&mut self) {
        let now = std::time::Instant::now();
        if self.client_checked_at.is_some_and(|at| now.duration_since(at).as_secs() < 30) {
            return;
        }
        self.client_checked_at = Some(now);
        let (Some(started), Some(on_disk)) = (self.launch_stamp, Self::executable_stamp()) else { return };
        let installed = (on_disk != started).then_some(on_disk);
        if installed != self.toast_sources.client_installed {
            self.toast_sources.client_installed = installed;
            self.log(match installed {
                None => "client: installed app matches the running one".into(),
                Some(stamp) => format!("client: a newer app is installed (started {started}, on disk {stamp}); relaunch offered"),
            });
        }
    }
    /// Start the installed app and quit this one. Terminals and agents live in wheeld, so they
    /// stay up through it (ADR-0020). A hidden window only records the ask.
    pub fn relaunch_client(&mut self) {
        let Some(bundle) = Self::bundle() else { return };
        self.log(format!("client: relaunch asked for {}", bundle.display()));
        if self.hidden {
            return;
        }
        if std::process::Command::new("/usr/bin/open").arg("-n").arg(&bundle).status().is_ok_and(|s| s.success()) {
            std::process::exit(0);
        }
    }

    // MARK: Pairing

    pub fn pairing_disabled_reason(&self) -> Option<&'static str> {
        Pairing::disabled_reason(self.link == Link::Up, &self.settings.devices)
    }
    /// Open the pairing window and ask for a code; it asks again every four minutes while open.
    pub fn show_pairing_window(&mut self) {
        if self.pairing_disabled_reason().is_some() {
            return;
        }
        for token in self.pairing.open(now_ms()) {
            self.send(&ClientMsg::PairingCancel { req: None, token });
        }
        self.request_pairing_code();
        self.sync_overlay();
    }
    pub fn close_pairing_window(&mut self) {
        for token in self.pairing.close() {
            self.log(format!("pairing window closed: cancelled code {}…", &token[..token.len().min(6)]));
            self.send(&ClientMsg::PairingCancel { req: None, token });
        }
        self.sync_overlay();
    }
    /// Copy a fresh pairing code as a concealed clipboard item, cleared after a minute if unchanged.
    pub fn copy_pairing_code(&mut self) {
        if self.pairing_disabled_reason().is_some() {
            return;
        }
        self.pairing.copy_pending = true;
        self.request_pairing_code();
    }
    /// The code reached the app (the Settings reply handler saw `pairing`): whatever asked for it takes it now.
    pub fn pairing_code_arrived(&mut self, token: &str) {
        if self.pairing.code_arrived(token)
            && let Some(json) = self.settings.devices.pairing_json(now_ms())
        {
            // A new copy replaces the old one, whose minute no longer runs.
            if let Some((old, _)) = self.pairing.copied.take()
                && old != token
            {
                self.send(&ClientMsg::PairingCancel { req: None, token: old });
            }
            self.pairing.copied = Some((token.to_owned(), now_ms() + pairing::CLIPBOARD_LIFE_MS));
            let answer = self.native.call(serde_json::json!({"op": "clipboard_concealed", "text": json, "clear_after_ms": pairing::CLIPBOARD_LIFE_MS}));
            self.log(format!("pairing code copied to the clipboard as a concealed item: {answer}"));
        }
    }
    pub fn copy_pairing_image(&mut self) {
        if let Some(json) = self.settings.devices.pairing_json(now_ms()) {
            let answer = self.native.call(serde_json::json!({"op": "qr", "text": json, "copy": true}));
            self.log(format!("pairing code image copied to the clipboard: {}", answer["copied"]));
        }
    }
    /// What the pairing window and the copied code do on a timer: a new code every four minutes,
    /// the window closing after a device connected, the copied code's minute running out.
    pub fn tick_pairing(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        if self.pairing.open && self.pairing.refresh_at.is_some_and(|at| now >= at) {
            self.pairing.refresh_at = Some(now + pairing::REFRESH_MS);
            self.request_pairing_code();
        }
        if self.pairing.open && self.pairing.close_at.is_some_and(|at| now >= at) {
            self.close_pairing_window();
        }
        if let Some(token) = self.pairing.copy_expired(now) {
            self.send(&ClientMsg::PairingCancel { req: None, token });
        }
        if let Some(next) = self.pairing.next_tick() {
            self.redraw_in(next.saturating_sub(now).max(50), cx);
        }
    }

    // MARK: Drawing

    /// The notices, bottom right over the window. They overlay the grid, so one coming or going
    /// moves nothing under it.
    pub(super) fn toast_stack(&self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let mut column = div().absolute().left_0().top_0().w(px(width)).h(px(height)).flex().flex_col().items_end().justify_end().gap_2().p(px(14.));
        for toast in self.toasts().into_iter().take(LIMIT) {
            column = column.child(toast_view(toast, width, cx));
        }
        column
    }

    /// The pairing window: the code, who connected, Copy code and Copy image, and the details.
    pub(super) fn pairing_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let now = now_ms();
        let qr_rows = self.settings.devices.pairing_json(now).map(|payload| self.qr_rows(&payload));
        let p = &self.pairing;
        let code: gpui::Div = match qr_rows {
            Some(rows) if !rows.is_empty() => {
                let cell = (190f32 / rows.len() as f32).floor().max(1.);
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
            _ => div().w(px(206.)).h(px(206.)).flex().items_center().justify_center().text_xs().text_color(rgb(DIM)).child("Asking wheeld for a code…"),
        };
        let status = div().flex().items_center().gap_2()
            .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(if p.connected.is_some() { 0x30d158 } else { 0xff9f0a })))
            .child(p.connected.as_ref().map(|n| format!("{n} connected")).unwrap_or_else(|| "Waiting for a device to scan…".into()));
        let link = |label: &str, cx: &mut Context<Self>, act: fn(&mut Client)| div().text_xs().text_color(rgb(0x7fb89a)).cursor_pointer().child(label.to_owned()).on_mouse_down(MouseButton::Left, click(cx, act));
        let buttons = div().flex().gap_4().child(link("Copy code", cx, |this| this.copy_pairing_code())).child(link("Copy image", cx, |this| this.copy_pairing_image()));
        let details_toggle = link(if p.details_open { "Hide details" } else { "Details" }, cx, |this| this.pairing.details_open = !this.pairing.details_open);
        let mut card = div().w(px(320f32.min(width - 32.))).flex().flex_col().items_center().gap_2().p_4().rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().w_full().flex().justify_between().items_center().child(div().font_weight(FontWeight::SEMIBOLD).child("Pair a device"))
                .child(div().px_2().rounded_sm().cursor_pointer().bg(rgb(BOX)).child("×").on_mouse_down(MouseButton::Left, click(cx, |this| this.close_pairing_window()))))
            .child(code).child(status).child(buttons).child(details_toggle);
        if p.details_open {
            let info = self.settings.devices.info.as_ref();
            let row = |label: &str, value: String| div().flex().gap_2().text_xs().child(div().w(px(88.)).flex_none().text_color(rgb(DIM)).child(label.to_owned())).child(div().flex_1().min_w(px(0.)).child(value));
            let mut details = div().w_full().flex().flex_col().gap_1().p_2().rounded_sm().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).font_family("Menlo")
                .child(row("Listening", info.and_then(|i| i.listen.clone()).unwrap_or_else(|| "off".into())))
                .child(row("Addresses", info.map(|i| i.addresses.join(", ")).unwrap_or_else(|| "unknown".into())))
                .child(row("Certificate", info.map(|i| format!("sha256 {}…{}", &i.fingerprint[..i.fingerprint.len().min(4)], &i.fingerprint[i.fingerprint.len().saturating_sub(4)..])).unwrap_or_else(|| "unknown".into())))
                .child(row("Problem", info.and_then(|i| i.problem.clone()).unwrap_or_else(|| "none".into())))
                .child(div().h(px(1.)).bg(rgb(LINE)))
                .child(div().flex().items_center().gap_2().text_xs().cursor_pointer().child(checkbox(p.close_on_connect)).child("Close when a device connects")
                    .on_mouse_down(MouseButton::Left, click(cx, |this| this.pairing.close_on_connect = !this.pairing.close_on_connect)))
                .child(div().h(px(1.)).bg(rgb(LINE)));
            for line in &p.events {
                details = details.child(div().text_xs().text_color(rgb(DIM)).child(line.clone()));
            }
            card = card.child(details);
        }
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(card)
    }

    /// Stands in for a pane that was being drawn when the client last quit unexpectedly, so the
    /// rest opens. Open Anyway draws it for real: if it crashes again, the next launch holds it out again.
    pub(super) fn quarantine_card(&self, pane: &str, kind: &str, cx: &mut Context<Self>) -> gpui::Div {
        let id = pane.to_owned();
        let mut buttons = div().flex().gap_2();
        if let Some(report) = self.quarantine.report.clone() {
            buttons = buttons.child(button("Show Crash Report", click(cx, move |this| this.pending_open = Some(report.to_string_lossy().into_owned()))));
        }
        buttons = buttons.child(button("Open Anyway", click(cx, move |this| this.quarantine.release_pane(&id))));
        div().size_full().flex().flex_col().items_center().justify_center().gap_2().p_6().text_center()
            .child(div().text_color(rgb(DIM)).child("⚠"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Wheel quit unexpectedly while drawing pane {pane}")))
            .child(div().text_xs().text_color(rgb(DIM)).child(format!("Its other panes and spaces opened as usual. This pane is a {kind} pane.")))
            .child(buttons)
    }
}

/// A tinted rounded button of a notice, before its click handler.
fn pill_base(label: String, tint: u32) -> gpui::Div {
    div().px(px(10.)).py(px(3.)).rounded_full().cursor_pointer().bg(rgba((tint << 8) | 0x2e)).text_xs().font_weight(FontWeight::MEDIUM).text_color(rgb(tint)).child(label)
}

impl Client {
    /// The card for a whole space the last run died drawing, over where its panes would be.
    pub(super) fn quarantine_space_card(&self, space: &str, cx: &mut Context<Self>) -> gpui::Div {
        let id = space.to_owned();
        let mut buttons = div().flex().gap_2();
        if let Some(report) = self.quarantine.report.clone() {
            buttons = buttons.child(button("Show Crash Report", click(cx, move |this| this.pending_open = Some(report.to_string_lossy().into_owned()))));
        }
        buttons = buttons.child(button("Open Anyway", click(cx, move |this| this.quarantine.release_space(&id))));
        div().size_full().flex().flex_col().items_center().justify_center().gap_2().p_6().text_center()
            .child(div().text_color(rgb(DIM)).child("⚠"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Wheel quit unexpectedly while drawing space {space}")))
            .child(div().text_xs().text_color(rgb(DIM)).child("Its other spaces opened as usual."))
            .child(buttons)
    }
}

/// One notice: a short one with at most one button as a pill, else a card with the message, its
/// buttons and Copy, and the close button.
fn toast_view(toast: Toast, width: f32, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
    let tint = toast.level.colour();
    let badge = div().w(px(18.)).h(px(18.)).rounded_full().bg(rgb(tint)).flex_none().flex().items_center().justify_center().text_xs().font_weight(FontWeight::BOLD).text_color(rgb(0x000000)).child(toast.level.symbol());
    let pill = |label: String, cx: &mut Context<Client>, act: Box<dyn Fn(&mut Client)>| {
        pill_base(label, tint).on_mouse_down(MouseButton::Left, click(cx, move |this| act(this)))
    };
    let mut actions = div().flex().gap(px(6.)).items_center();
    for action in &toast.actions {
        let act = action.act.clone();
        actions = actions.child(pill(action.title.to_owned(), cx, Box::new(move |this| this.run_toast_action(&act))));
    }
    let copied = toast.text.clone();
    let copy = pill_base("Copy".into(), tint)
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
            this.log(format!("toast: copied {} characters", copied.chars().count()));
            cx.stop_propagation();
        }));
    let close = {
        let t = toast.clone();
        div().w(px(22.)).h(px(22.)).rounded_full().flex_none().cursor_pointer().bg(rgba(0xffffff1a)).flex().items_center().justify_center().text_xs().child("×")
            .on_mouse_down(MouseButton::Left, click(cx, move |this| this.dismiss_toast(&t)))
    };
    let id = toast.id.clone();
    let base = div().id(gpui::ElementId::Name(format!("toast.{id}").into())).max_w(px(460f32.min(width - 28.))).bg(rgb(0x1d2823)).border_1().border_color(rgb(0x3d5247)).text_color(rgb(0xd9e8e1)).shadow_lg()
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
    if let Some((from, deadline)) = toast.countdown {
        // The bar empties from the left as the deadline nears; the seconds left say the same.
        let now = now_ms().clamp(from, deadline);
        let left = ((deadline - now) as f32 / 1000.).ceil() as u64;
        let fraction = if deadline > from { (deadline - now) as f32 / (deadline - from) as f32 } else { 0. };
        return base.w(px(460f32.min(width - 28.))).rounded_lg().pl(px(12.)).pr(px(8.)).py(px(10.)).flex().items_start().gap(px(10.)).child(badge)
            .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap_2()
                .child(div().flex().justify_between().child(toast.text.clone()).child(div().text_xs().text_color(rgb(0x9fb3aa)).child(format!("{left} s"))))
                .child(div().w_full().h(px(4.)).rounded_full().bg(rgba(0xffffff1a)).child(div().h_full().w(gpui::relative(fraction)).rounded_full().bg(rgb(tint))))
                .child(actions.child(copy)));
    }
    if toast.compact() {
        let mut row = base.rounded_full().pl(px(12.)).pr(px(7.)).py(px(7.)).flex().items_center().gap(px(10.)).child(badge).child(div().whitespace_nowrap().overflow_hidden().child(toast.text.clone())).child(actions).child(copy);
        if toast.closable {
            row = row.child(close);
        }
        row
    } else {
        // A card takes its full width so the message wraps inside it.
        let mut row = base.w(px(460f32.min(width - 28.))).rounded_lg().pl(px(12.)).pr(px(8.)).py(px(10.)).flex().items_start().gap(px(10.)).child(badge)
            .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap_2().child(div().child(toast.text.clone())).child(actions.child(copy)));
        if toast.closable {
            row = row.child(close);
        }
        row
    }
}
