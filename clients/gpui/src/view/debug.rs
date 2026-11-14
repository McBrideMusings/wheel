//! The debug server's routes (DebugServer.swift), run on the GPUI thread against the window:
//! `GET /state` reads the client as JSON, `POST /window/snapshot?path=` draws the window to a
//! PNG, and the rest drive the client's own code paths, so a check exercises what the buttons,
//! keys and cards do, never a private shortcut. Each POST answers with the state as it was once
//! the request ran and before wheeld answered, as the Swift server does. The transport is
//! `crate::debug`.
use super::*;
use crate::debug::{Request, error};
use serde_json::{Value, json};

/// What a request came to: an answer, or a snapshot the caller draws once the view is free
/// (drawing re-enters the view, which cannot happen while it is being updated).
pub enum Answer {
    Done(String, Value),
    Snapshot(String),
}

fn ok(body: Value) -> Answer {
    Answer::Done("200 OK".to_owned(), body)
}
fn refuse(status: &str, why: &str) -> Answer {
    Answer::Done(status.to_owned(), error(why))
}
const BAD: &str = "400 Bad Request";
const MISSING: &str = "404 Not Found";

impl Client {
    /// Answer one request. `window` is the window the request reached, for a snapshot.
    pub fn debug_request(&mut self, request: &Request, cx: &mut Context<Self>) -> Answer {
        let q = |name: &str| request.query.get(name).cloned();
        let path = request.path.as_str();
        // Reads are not logged: a check polls `/state` and the log is what it reads.
        if request.method != "GET" {
            let mut query: Vec<String> = request.query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            query.sort();
            let separator = if query.is_empty() { "" } else { "?" };
            self.log(format!("debug: {} {path}{separator}{}", request.method, query.join("&")));
        }
        if self.hidden && matches!(path, "/window/front" | "/window/back" | "/app/activate" | "/input/mouse" | "/input/key") {
            return refuse("403 Forbidden", "this app is hidden for a check and never takes focus or input; use the debug routes (/key, /action, /pane/*, /procs), or run it without WHEEL_HIDDEN to watch");
        }
        if request.method == "GET" {
            return match path {
                "/state" => ok(self.state_json()),
                "/geometry" => ok(self.geometry_json()),
                _ => refuse(MISSING, "no such route"),
            };
        }
        if request.method != "POST" {
            return refuse("405 Method Not Allowed", "GET or POST");
        }
        let answer = match path {
            "/window/snapshot" => {
                let Some(target) = q("path") else { return refuse(BAD, "path is required") };
                return Answer::Snapshot(target);
            }
            "/key" => {
                let Some(keys) = q("keys") else { return refuse(BAD, "keys=<chord> is required") };
                let Some(chord) = crate::keymap::KeyChord::parse(&keys) else { return refuse(BAD, "keys must be a chord such as shift+cmd+d or escape") };
                let taken = self.key(&KeyDownEvent { keystroke: chord.keystroke(), is_held: false, prefer_character_input: false }, cx);
                Some(json!({"taken": taken}))
            }
            "/type" => {
                let Some(text) = q("text") else { return refuse(BAD, "text= is required") };
                for ch in text.chars() {
                    // The event a key typing `ch` delivers: GPUI names the space bar `space`.
                    let key = if ch == ' ' { "space".to_owned() } else { ch.to_lowercase().to_string() };
                    let keystroke = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key, key_char: Some(ch.to_string()) };
                    self.key(&KeyDownEvent { keystroke, is_held: false, prefer_character_input: true }, cx);
                }
                None
            }
            "/action" => {
                let Some(id) = q("id") else { return refuse(BAD, "id= is required") };
                if !self.all_actions().iter().any(|a| a.id == id) {
                    return refuse(MISSING, "no action with that id");
                }
                self.run_action(&id, cx);
                None
            }
            "/confirm" => {
                if self.confirmation.is_none() {
                    return refuse(MISSING, "no confirmation is open");
                }
                match q("answer").as_deref() {
                    Some("yes") => self.answer_confirmation(true),
                    Some("no") => self.answer_confirmation(false),
                    _ => return refuse(BAD, "answer must be yes or no"),
                }
                None
            }
            "/agent" | "/agent/draft" | "/agent/focus" => {
                let Some(agent) = q("agent") else { return refuse(BAD, "agent=<name> is required") };
                let Some(pane) = self.spaces.iter().flat_map(|s| s.shown()).find(|l| l.agent.as_deref() == Some(agent.as_str())).map(|l| l.id.clone()) else {
                    return refuse(MISSING, "no agent pane shows that agent");
                };
                match path {
                    "/agent" => {
                        self.drafts.insert(agent.clone(), String::new());
                        self.send(&ClientMsg::AgentSend { req: None, agent: agent.clone(), text: request.body.clone() });
                    }
                    "/agent/draft" => {
                        self.drafts.insert(agent.clone(), request.body.clone());
                        self.draft_edited(&agent.clone());
                    }
                    _ => self.focus_pane(&pane),
                }
                None
            }
            "/perm" => {
                let (Some(id), Some(choice)) = (q("id"), q("choice")) else { return refuse(BAD, "id= and choice= are required") };
                if !self.perms.iter().any(|p| p.id == id) {
                    return refuse(MISSING, "no waiting permission with that id");
                }
                self.log(format!("debug: answer {id} {choice}"));
                self.send(&ClientMsg::PermAnswer { req: None, id, choice });
                None
            }
            "/procs" => {
                let Some(space) = q("space").or_else(|| self.space_id.clone()) else { return refuse(BAD, "space=<id> is required") };
                let pid = q("pid").and_then(|p| p.parse::<u32>().ok());
                match q("action").as_deref().unwrap_or("list") {
                    "list" => self.request_procs(&space),
                    "open" => self.open_processes(&space),
                    "close" => self.close_processes(),
                    "choose" => {
                        let Some(pid) = pid else { return refuse(BAD, "pid=<n> is required") };
                        self.procs.toggle_chosen(pid);
                    }
                    "filter" => self.procs.filter = q("text").unwrap_or_default(),
                    "stop" => {
                        let Some(pid) = pid else { return refuse(BAD, "pid=<n> is required") };
                        self.stop_proc(&space, pid, q("force").as_deref() == Some("true"));
                    }
                    _ => return refuse(BAD, "action must be list, open, close, choose, filter or stop"),
                }
                None
            }
            "/focus" => {
                if let Some(space) = q("space") {
                    self.show_space_anywhere(&space);
                } else if let Some(pane) = q("pane") {
                    self.show_pane(&pane);
                } else {
                    return refuse(BAD, "space= or pane= is required");
                }
                None
            }
            "/space" => {
                let Some(cwd) = q("cwd") else { return refuse(BAD, "cwd=<dir> is required") };
                self.create_space(&cwd);
                None
            }
            "/space/close" => {
                let Some(id) = q("id") else { return refuse(BAD, "id= is required") };
                self.ask_close_space(&id);
                None
            }
            "/split" => {
                let (Some(pane), Some(dir)) = (q("pane"), q("dir")) else { return refuse(BAD, "pane= and dir=left|right|up|down are required") };
                let dir = match dir.as_str() {
                    "left" => wheel_proto::Dir::Left,
                    "right" => wheel_proto::Dir::Right,
                    "up" => wheel_proto::Dir::Up,
                    "down" => wheel_proto::Dir::Down,
                    _ => return refuse(BAD, "dir must be left, right, up or down"),
                };
                self.send_layout(|req| ClientMsg::PaneSplit { req: Some(req), pane, dir: Some(dir), focus: false, site: None });
                None
            }
            "/kind" => {
                let (Some(pane), Some(kind)) = (q("pane"), q("kind")) else { return refuse(BAD, "pane= and kind=empty|terminal|web|browser are required") };
                let kind = match kind.as_str() {
                    "empty" => NewPaneKind::Empty,
                    "terminal" => NewPaneKind::Terminal,
                    "web" => NewPaneKind::Web,
                    "browser" => NewPaneKind::Browser,
                    _ => return refuse(BAD, "kind must be empty, terminal, web or browser"),
                };
                let artifact = q("artifact");
                let scope = match q("scope").as_deref() {
                    None => None,
                    Some("project") => Some(wheel_proto::ArtifactScope::Project),
                    Some("machine") => Some(wheel_proto::ArtifactScope::Machine),
                    Some(_) => return refuse(BAD, "scope must be project or machine"),
                };
                self.send_layout(|req| ClientMsg::PaneSetKind { req: Some(req), pane, kind, artifact, scope, url: None });
                None
            }
            "/start" => {
                let (Some(pane), Some(kind)) = (q("pane"), q("kind")) else { return refuse(BAD, "pane= and kind=terminal|agent are required") };
                match kind.as_str() {
                    "terminal" => self.send_layout(|req| ClientMsg::PaneSetKind { req: Some(req), pane, kind: NewPaneKind::Terminal, artifact: None, scope: None, url: None }),
                    "agent" => {
                        let Some(agent) = q("agent") else { return refuse(BAD, "agent=<name> is required") };
                        self.send(&ClientMsg::PaneStart { req: None, pane, kind: wheel_proto::StartKind::Agent, message: q("message"), agent: Some(agent), model: None, permission_mode: None, sandbox: None });
                    }
                    _ => return refuse(BAD, "kind must be terminal or agent"),
                }
                None
            }
            "/ratio" => {
                let (Some(split), Some(ratio)) = (q("split"), q("ratio").and_then(|r| r.parse::<f64>().ok())) else { return refuse(BAD, "split= and ratio=<0..1> are required") };
                self.send(&ClientMsg::SplitRatio { req: None, split, ratio });
                None
            }
            "/pane/close" | "/pane/focused" => {
                let Some(pane) = q("pane") else { return refuse(BAD, "pane= is required") };
                if path == "/pane/close" { self.ask_close_pane(&pane) } else { self.focus_pane(&pane) }
                None
            }
            "/settings" => {
                match q("action").as_deref() {
                    Some("open") => self.open_settings(),
                    Some("close") => self.close_settings(),
                    Some("tab") => {
                        let Some(tab) = q("tab").and_then(|t| crate::settings::Tab::named(&t)) else { return refuse(BAD, "tab= must name a Settings tab") };
                        self.settings_tab(tab);
                    }
                    Some("project") => {
                        let Some(id) = q("id") else { return refuse(BAD, "id= is required") };
                        self.select_settings_project(&id);
                    }
                    _ => return refuse(BAD, "action must be open, close, tab or project"),
                }
                None
            }
            "/palette" => {
                if let Some(pane) = q("pane") {
                    match q("action").as_deref() {
                        Some("open") => self.open_palette(&pane),
                        Some("close") => self.close_palette(&pane),
                        _ => return refuse(BAD, "action must be open or close"),
                    }
                } else {
                    match q("action").as_deref() {
                        Some("open") => self.open_command_palette(if q("mode").as_deref() == Some("new_space") { crate::command_palette::Mode::NewSpace } else { crate::command_palette::Mode::Root }),
                        Some("close") => self.close_command_palette(),
                        Some("type") => self.command_palette_typed(&q("text").unwrap_or_default()),
                        Some("select") => self.command_palette_select(q("index").and_then(|i| i.parse().ok()).unwrap_or(0)),
                        Some("run") => self.command_palette_run(cx),
                        _ => return refuse(BAD, "action must be open, close, type, select or run"),
                    }
                }
                None
            }
            "/pairing" => {
                match q("action").as_deref() {
                    Some("open") => self.show_pairing_window(),
                    Some("close") => self.close_pairing_window(),
                    _ => return refuse(BAD, "action must be open or close"),
                }
                None
            }
            "/toast/clear" => {
                self.posted.clear();
                None
            }
            // The sidebar toggle and its rows: `open` is true, false or toggle; `space` clicks that space's row.
            "/sidebar" => {
                let open = match q("open").as_deref() {
                    None => None,
                    Some("true") => Some(true),
                    Some("false") => Some(false),
                    Some("toggle") => Some(!self.sidebar_open),
                    Some(_) => return refuse(BAD, "open is true, false or toggle"),
                };
                if let Some(space) = q("space") && !self.spaces.iter().any(|s| s.id == space) {
                    return refuse(MISSING, "no space with that id");
                }
                if let Some(open) = open {
                    self.set_sidebar(open);
                }
                if let Some(space) = q("space") {
                    self.show_space_anywhere(&space);
                }
                None
            }
            // What a click on a space's row runs: the space shows.
            "/spacetap" => {
                let Some(space) = q("space") else { return refuse(BAD, "space= is required") };
                let Some(r) = self.space_hit_rect(&space) else { return refuse(MISSING, "no such space in this window") };
                self.space_press_down(&space, r.x + r.w / 2., r.y + r.h / 2.);
                self.pointer_up(cx);
                None
            }
            // A space drag without a pointer: `action` is start (`space`), hover (`target`: sidebar:<index>
            // or window for this window, sidebar:<window>:<index>, window:<window>, card:<session>:<index>
            // or desktop), drop or cancel.
            "/spacedrag" => {
                match q("action").as_deref() {
                    Some("start") => {
                        let Some(space) = q("space") else { return refuse(BAD, "space= is required") };
                        if !self.spaces.iter().any(|s| s.id == space) {
                            return refuse(MISSING, "no such space");
                        }
                        let at = self.space_hit_rect(&space).map(|r| (r.x + r.w / 2., r.y + r.h / 2.)).unwrap_or_default();
                        self.begin_space_drag(&space, false, at);
                    }
                    Some("hover") => {
                        if self.space_drag.is_none() {
                            return refuse("409 Conflict", "no space drag in progress");
                        }
                        let Some(target) = q("target").and_then(|t| debug_space_target(&t, &self.window_id)) else {
                            return refuse(BAD, "target is sidebar:<index>, window, sidebar:<window>:<index>, window:<window>, card:<session>:<index> or desktop");
                        };
                        self.hover_space_drag(Some(target));
                    }
                    Some("drop") => self.end_space_drag(cx),
                    Some("cancel") => self.cancel_space_drag(false),
                    _ => return refuse(BAD, "action is start, hover, drop or cancel"),
                }
                None
            }
            // A pane drag without a pointer: `action` is start (`pane`), hover (`target` space:<id>,
            // new-space, or pane:<id>:<zone> with zone center, left, right, up or down), drop or cancel.
            "/panedrag" => {
                match q("action").as_deref() {
                    Some("start") => {
                        let Some(pane) = q("pane") else { return refuse(BAD, "pane= is required") };
                        let Some(leaf) = self.spaces.iter().flat_map(|s| s.shown()).find(|l| l.id == pane).cloned() else { return refuse(MISSING, "no such pane") };
                        let label = self.pane_label(&leaf);
                        self.begin_pane_drag(&pane, &label, (0., 0.), (0., 0.), false);
                    }
                    Some("hover") => {
                        if self.pane_drag.is_none() {
                            return refuse("409 Conflict", "no pane drag in progress");
                        }
                        let target = q("target").unwrap_or_default();
                        if let Some(rest) = target.strip_prefix("pane:") {
                            let Some((pane, zone)) = rest.rsplit_once(':').and_then(|(p, z)| crate::grid_drop::Zone::parse(z).map(|z| (p.to_owned(), z))) else {
                                return refuse(BAD, "target is pane:<id>:<center|left|right|up|down>");
                            };
                            self.hover_pane_drag(None);
                            self.hover_grid_drop(Some(crate::grid_drop::GridDrop { target: pane, zone }));
                        } else {
                            use crate::sidebar::PaneAway;
                            let away = match target.as_str() {
                                "new-space" => PaneAway::NewSpace(self.window_id.clone()),
                                "desktop" => PaneAway::Desktop,
                                t if t.starts_with("new-space:") => PaneAway::NewSpace(t[10..].to_owned()),
                                t if t.starts_with("space:") => PaneAway::End(t[6..].to_owned()),
                                t if t.starts_with("beside:") => {
                                    let Some((pane, dir)) = t[7..].rsplit_once(':').and_then(|(p, d)| match crate::grid_drop::Zone::parse(d) {
                                        Some(crate::grid_drop::Zone::Edge(dir)) => Some((p.to_owned(), dir)),
                                        _ => None,
                                    }) else {
                                        return refuse(BAD, "target is beside:<pane>:<left|right|up|down>");
                                    };
                                    let Some(space) = self.space_holding(&pane).map(|s| s.id.clone()) else { return refuse(MISSING, "no such pane") };
                                    PaneAway::Beside { pane, dir, space }
                                }
                                _ => return refuse(BAD, "target is space:<id>, new-space[:<window>], desktop, beside:<pane>:<dir> or pane:<id>:<zone>"),
                            };
                            self.hover_grid_drop(None);
                            self.name_pane_away(Some(away));
                            self.arm_pane_hold(cx);
                        }
                    }
                    Some("drop") => self.end_pane_drag(cx),
                    Some("cancel") => self.cancel_pane_drag(),
                    _ => return refuse(BAD, "action is start, hover, drop or cancel"),
                }
                None
            }
            // Where a pane kind's header puts a control, as Settings › General › Pane header does:
            // `kind`, `control` (a Swift HeaderControl name) and `placement` bar|menu|off, or `reset`.
            "/paneheader" => {
                let Some(kind) = q("kind") else { return refuse(BAD, "kind= is required") };
                if q("reset").is_some() {
                    self.reset_header_layout(&kind);
                } else {
                    let (Some(control), Some(placement)) = (q("control").as_deref().and_then(crate::pane_header::Control::named), q("placement").as_deref().and_then(crate::pane_header::Placement::named)) else {
                        return refuse(BAD, "control= and placement= (bar, menu or off) are required");
                    };
                    self.set_header_placement(control, &kind, placement);
                }
                None
            }
            // A pane header's menu and buttons: `action` open (`pane`, optional `x`, `y`, and
            // `level=header` for the ⋯ menu), close, or run (`pane`, `item`: a menu item's id or a
            // header button's).
            "/panemenu" => {
                match q("action").as_deref() {
                    Some("open") => {
                        let Some(pane) = q("pane").filter(|p| !self.header_menu(p, 0).is_empty()) else { return refuse(MISSING, "pane= names no pane in the grid") };
                        let num = |k: &str| q(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(40.);
                        // `level=header` opens the ⋯ menu of the header as it is drawn; otherwise the right-click one.
                        let level = if q("level").as_deref() == Some("header") { self.header_level(&pane) } else { 0 };
                        self.open_pane_menu(&pane, num("x"), num("y"), level);
                    }
                    Some("close") => self.close_pane_menu(),
                    Some("run") => {
                        let (Some(pane), Some(item)) = (q("pane"), q("item")) else { return refuse(BAD, "pane= and item= are required") };
                        if !self.choose_header_menu(&item, &pane) {
                            return refuse("409 Conflict", "the pane menu offers no such enabled item");
                        }
                    }
                    _ => return refuse(BAD, "action is open, close or run"),
                }
                None
            }
            // The window's name field (`kind=window`) and the Rename Session card (`kind=session&id=`):
            // begin, commit with `name=`, or cancel, as Return and Esc do.
            "/rename" if matches!(q("kind").as_deref(), Some("window" | "session")) => {
                let target = match q("kind").as_deref() {
                    Some("session") => {
                        let Some(id) = q("id").filter(|id| self.sessions.iter().any(|s| &s.id == id)) else { return refuse(MISSING, "id= names no session") };
                        NameTarget::Session(id)
                    }
                    _ => NameTarget::Window,
                };
                let begin = |this: &mut Self| match &target {
                    NameTarget::Window => this.begin_window_rename(),
                    NameTarget::Session(id) => this.begin_session_rename(id),
                };
                match q("action").as_deref() {
                    Some("begin") => begin(self),
                    Some("commit") => {
                        if self.name_edit.as_ref().map(|e| &e.target) != Some(&target) {
                            begin(self);
                        }
                        if let Some(name) = q("name") && let Some(edit) = self.name_edit.as_mut() {
                            edit.draft = name;
                        }
                        self.commit_name_edit();
                    }
                    Some("cancel") => self.cancel_name_edit(),
                    _ => return refuse(BAD, "action is begin, commit or cancel"),
                }
                None
            }
            // Renaming as the field does it: `action` is begin (the menu item), commit (Return, with `name`) or cancel (Escape).
            "/rename" => {
                let Some(id) = q("id") else { return refuse(BAD, "kind=space|window|session is required, with id= for a space or a session") };
                if q("kind").as_deref() != Some("space") {
                    return refuse(BAD, "kind is space, window or session");
                }
                if !self.spaces.iter().any(|s| s.id == id) {
                    return refuse(MISSING, "no such space");
                }
                match q("action").as_deref() {
                    Some("begin") => self.begin_rename(&id),
                    Some("commit") => {
                        if self.rename.as_ref().map(|r| r.space.as_str()) != Some(id.as_str()) {
                            self.begin_rename(&id);
                        }
                        if let Some(name) = q("name") && let Some(r) = self.rename.as_mut() {
                            r.draft = name;
                        }
                        self.commit_rename();
                    }
                    Some("cancel") => self.cancel_rename(),
                    _ => return refuse(BAD, "action is begin, commit or cancel"),
                }
                None
            }
            // A space's menu: `space` opens it (with `item`, runs that item as the menu would).
            "/spacemenu" => {
                let Some(space) = q("space") else { return refuse(BAD, "space= is required") };
                if !self.spaces.iter().any(|s| s.id == space) {
                    return refuse(MISSING, "no such space");
                }
                match q("item") {
                    Some(item) => {
                        if !self.space_menu_items(&space).iter().any(|i| i.id == item) {
                            return refuse(MISSING, "no such menu item");
                        }
                        self.run_space_menu(&item, &space, cx);
                    }
                    None => {
                        let at = self.space_hit_rect(&space).map(|r| (r.x + 20., r.y + r.h)).unwrap_or_default();
                        self.open_space_menu(&space, at.0, at.1);
                    }
                }
                None
            }
            // A press on a title-row control by its id (`sidebar`, `session`, `mode`, `plugin-<id>`,
            // `processes`, `palette`, `pin`, `new-space`, or `tab:<space>`), through the row's own press.
            "/title" => {
                let Some(id) = q("id") else { return refuse(BAD, "id= is required") };
                let Some(chip) = self.header_chips().into_iter().find(|c| header::frame_id(&c.hit).as_deref() == Some(id.as_str()) || matches!(&c.hit, Hit::Space(s) if format!("tab:{s}") == id)) else {
                    return refuse(MISSING, "no title-row control with that id");
                };
                self.press_title(&chip.hit);
                None
            }
            // The session or mode menu: open (`menu=session|mode`), close, or select a row (`item=`).
            "/titlemenu" => {
                match q("action").as_deref() {
                    Some("open") => {
                        let menu = match q("menu").as_deref() { Some("session") => TitleMenu::Session, Some("mode") => TitleMenu::Mode, _ => return refuse(BAD, "menu is session or mode") };
                        if self.title_menu != Some(menu) {
                            self.toggle_title_menu(menu);
                        }
                    }
                    Some("close") => self.close_title_menu(),
                    Some("select") => {
                        let Some(item) = q("item") else { return refuse(BAD, "item= is required") };
                        if self.title_menu.is_none() {
                            return refuse(BAD, "no title menu is open");
                        }
                        self.run_title_menu(&item);
                    }
                    _ => return refuse(BAD, "action is open, close or select"),
                }
                None
            }
            "/mode" => {
                let Some(mode) = q("mode") else { return refuse(BAD, "mode= is required") };
                if !header::MODES.iter().any(|m| m.0 == mode) {
                    return refuse(BAD, "mode is spaces or desk");
                }
                self.set_mode(&mode);
                None
            }
            "/pin" => {
                match q("on").as_deref() {
                    Some("true") => self.set_pinned(true),
                    Some("false") => self.set_pinned(false),
                    _ => return refuse(BAD, "on is true or false"),
                }
                None
            }
            // Little windows, as the Swift app's `/window?action=little|littleback|littlegoto|
            // littleclosebutton`: on a main window `open`, `goto` and `back` take `space=`; on the
            // little window itself (`window=<id>`) `back` is the strip's back icon and `close` its red light.
            "/little" => {
                let space = q("space").map(|s| crate::windows::space_id(&s, &self.spaces));
                match (q("action").as_deref(), space) {
                    (Some("close"), None) if self.is_little() => self.request_close_window(),
                    (Some("back"), None) if self.is_little() => self.little_back(),
                    (Some(_), Some(Err(e))) => return refuse("404 Not Found", &e),
                    (Some("open"), Some(Ok(space))) => {
                        if self.open_little(&space, cx).is_none() {
                            return refuse("409 Conflict", "the layout places that space nowhere");
                        }
                    }
                    (Some("goto" | "back"), Some(Ok(space))) if self.little_showing(&space).is_none() => return refuse("404 Not Found", "no little window shows that space"),
                    (Some("goto"), Some(Ok(space))) => self.go_to_little(&space, cx),
                    (Some("back"), Some(Ok(space))) => self.bring_back(&space, cx),
                    _ => return refuse(BAD, "action is open, goto or back with space=, or back or close on a little window"),
                }
                None
            }
            // The Usage window and the strip, as the Swift app's `/usage`: open, close, refresh
            // (`due_only=true` for the timer's read), a window's meter level, a label or an agent toggle.
            "/usage" => {
                match q("action").as_deref() {
                    Some("open") => self.open_usage(),
                    Some("close") => self.close_usage(),
                    Some("refresh") => self.refresh_usage(q("due_only").as_deref() == Some("true")),
                    Some("meter") => {
                        let Some(row) = q("row") else { return refuse(BAD, "row=<agent>:<window> is required") };
                        let Some((agent, window)) = row.split_once(':') else { return refuse(BAD, "row=<agent>:<window> is required") };
                        let level = match q("level").as_deref() {
                            Some("off") => None,
                            Some("meter") => Some(wheel_proto::UsageMeterLevel::Meter),
                            Some("full") => Some(wheel_proto::UsageMeterLevel::Full),
                            _ => return refuse(BAD, "level is off, meter or full"),
                        };
                        self.set_meter(agent, window, level);
                    }
                    Some("label") => {
                        let Some(label) = q("label") else { return refuse(BAD, "label= is required") };
                        if !crate::strip::LABELS.contains(&label.as_str()) {
                            return refuse(BAD, "label is billing, folder or email");
                        }
                        let on = self.settings.usage.labels.iter().any(|l| *l == label);
                        if q("on").as_deref().map(|v| v == "true") != Some(on) {
                            self.toggle_usage_label(&label);
                        }
                    }
                    Some("agent") => {
                        let Some(agent) = q("agent") else { return refuse(BAD, "agent= is required") };
                        let Some(info) = self.settings.usage.agents.iter().find(|a| a.id == agent) else { return refuse(MISSING, "no such usage agent") };
                        if q("on").as_deref().map(|v| v == "true") != Some(info.shown) {
                            self.toggle_usage_agent(&agent);
                        }
                    }
                    _ => return refuse(BAD, "action is open, close, refresh, meter, label or agent"),
                }
                None
            }
            // The hover preview as the pointer drives it: `space` holds the pointer over its row, none lets it go.
            "/preview" => {
                match q("space") {
                    Some(space) => {
                        if !self.spaces.iter().any(|s| s.id == space) {
                            return refuse(MISSING, "no such space");
                        }
                        self.row_hover(Some(space));
                    }
                    None => self.row_hover(None),
                }
                self.tick_preview(cx);
                None
            }
            _ => return refuse(MISSING, "no such route"),
        };
        cx.notify();
        ok(answer.unwrap_or_else(|| self.state_json()))
    }

    /// Where the title row's controls are, for a check that presses them with real events:
    /// `title_buttons` by the Swift ids, and `tabs` while the sidebar is hidden.
    fn geometry_json(&self) -> Value {
        let row = self.title_row();
        let tabs: Vec<Value> = row.chips.iter().filter_map(|c| match &c.hit { Hit::Space(id) => Some(json!({"space": id, "label": c.label, "active": c.active, "frame": [c.rect.x, c.rect.y, c.rect.w, c.rect.h]})), _ => None }).collect();
        json!({"viewport": [self.viewport.0, self.viewport.1], "title_buttons": self.title_button_frames(), "tabs": tabs,
            "tab_strip": row.track.map(|t| json!([t.x, t.y, t.w, t.h])), "title": row.chips.iter().find(|c| c.hit == Hit::Title).map(|c| json!({"label": c.label, "frame": [c.rect.x, c.rect.y, c.rect.w, c.rect.h]}))})
    }

    /// The client as JSON: what the Swift app's `/state` carries where the GPUI client has it.
    pub fn state_json(&self) -> Value {
        let native = self.native.status();
        let layers = self.layers();
        let leaves: Vec<&Placed> = layers.iter().flat_map(|l| &l.leaves).collect();
        let mut panes = serde_json::Map::new();
        for space in &self.spaces {
            for leaf in space.shown() {
                let placed = leaves.iter().find(|p| p.leaf.id == leaf.id);
                let level = placed.map_or(0, |p| self.header_level_for(&leaf.kind, p.rect.w));
                let mut entry = json!({
                    "space": space.id, "kind": leaf.kind, "label": leaf.label, "term": leaf.term, "url": leaf.url, "agent": leaf.agent,
                    "harness": leaf.harness, "status": leaf.status, "visible": placed.is_some(), "site": space.site_of(&leaf.id).map(|s| s.name()),
                    "menu": self.header_menu(&leaf.id, 0).iter().map(|i| json!({"id": i.id, "title": i.title, "enabled": i.enabled})).collect::<Vec<_>>(),
                    "header_buttons": self.header_button_frames(&leaf.id),
                    "header_order": self.header_buttons_at(&leaf.id, level).iter().map(|b| b.id).collect::<Vec<_>>(),
                    "header_level": level,
                    "more_menu": self.header_menu(&leaf.id, level).iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                    "frame": placed.map(|p| json!([p.rect.x, p.rect.y, p.rect.w, p.rect.h])),
                    "opacity": self.grid_opacity(&leaf.id),
                    "native": self.native_panes.contains(&leaf.id), "focused": self.focus.current.as_deref() == Some(leaf.id.as_str()),
                    "palette": self.palettes.get(&leaf.id).map(|p| json!({"query": p.query, "rows": p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), "selected": p.selected})),
                    "browser": self.browsers.get(&leaf.id).map(|b| json!({"address": b.address, "failure": b.failure})),
                    "bridge": native["panes"][leaf.id.as_str()],
                });
                if let Some(agent) = &leaf.agent {
                    entry["draft"] = json!(self.drafts.get(agent).cloned().unwrap_or_default());
                    entry["rows"] = json!(self.transcripts.get(agent).map(|t| t.rows.len()).unwrap_or(0));
                    entry["perm_card"] = self.perm_card_json(&leaf.id);
                    entry["footer"] = self.running_json(agent);
                    entry["composer"] = self.composer_json(agent, leaf.harness.as_deref().unwrap_or(""));
                    entry["conversation_error"] = json!(self.conversations.errors.get(agent));
                }
                panes.insert(leaf.id.clone(), entry);
            }
        }
        let procs: serde_json::Map<String, Value> = self.procs.by_space.iter().map(|(space, list)| (space.clone(), json!(list))).collect();
        let mut out = json!({
            "window_info": self.window_json(),
            "daemon_url": self.endpoint.ws.split('?').next().unwrap_or_default(),
            "connection": match &self.link { Link::Connecting => "connecting".to_owned(), Link::Up => "connected".to_owned(), Link::Down(why) => format!("unreachable: {why}"), Link::Incompatible { .. } => "incompatible".to_owned() },
            "banner": self.link.banner(),
            "server_protocol": json!({"version": self.server_protocol.unwrap_or(crate::daemon::PROTOCOL), "client": crate::daemon::PROTOCOL}),
            "daemon_service": super::server::with(|sv| sv.control.service().map(|s| json!({"label": s.label, "instance": s.instance, "listen": s.listen, "status": sv.service_status, "bundled_source": s.bundled_source, "running_source": self.running_source(), "update": self.restart_waiting()}))).flatten(),
            "server": super::server::with(|sv| json!({
                "status": self.server_status_word().word,
                "control": sv.control.unavailable().unwrap_or("available"),
                "control_kind": sv.control.kind(),
                "restart_available": sv.control.available(),
                "restarting": sv.busy == Some(crate::server::Op::Restart),
                "busy": sv.busy.map(|op| format!("{op:?}")),
                "problem": sv.problem,
                "stopped_by_menu": sv.stopped_by_menu,
                "notice": self.server_stopped_notice().then(|| json!({"title": super::title::SERVER_STOPPED_TITLE, "hint": super::title::SERVER_STOPPED_HINT, "grid_drawn": self.grid_drawn.len()})),
                "restart_waiting": self.restart_waiting(),
                "banner": self.toasts().iter().any(|t| t.id == "server-update"),
                "restart_expected": sv.watch.restart_expected,
                "rollback_offer": sv.watch.rollback_offer.as_ref().map(|o| json!({"target": o.target, "seconds_left": (o.deadline.saturating_sub(super::settings::now_ms())) as f64 / 1000.})),
                "rolled_back_to": sv.watch.rolled_back_to,
                "started": self.server_started,
                "log": sv.log,
            })),
            "hidden": self.hidden,
            "window": self.window_id,
            "sessions": self.sessions.iter().map(|s| json!({"id": s.id, "name": s.name, "spaces": s.spaces})).collect::<Vec<_>>(),
            "session": self.session_id,
            "active": self.space_id,
            "spaces": self.spaces.iter().map(|s| json!({"id": s.id, "name": s.name, "cwd": s.cwd, "status": s.status, "panes": leaves_of(&s.root).iter().map(|l| l.id.clone()).collect::<Vec<_>>(),
                "popped": s.popped.iter().flat_map(leaves_of).map(|l| l.id.clone()).collect::<Vec<_>>(), "backgrounded": s.backgrounded.iter().flat_map(leaves_of).map(|l| l.id.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "conversations": self.conversations_json(),
            "pane_menu": self.pane_menu.as_ref().map(|(pane, x, y, level)| json!({"pane": pane, "at": [x, y], "level": level})),
            "grid_drop_highlight": self.grid_drop_highlight().map(|h| json!([h.x, h.y, h.w, h.h])),
            "focus_key": self.focus.current,
            "panes": panes,
            "perms": self.perms,
            "procs": procs,
            "processes_card": self.procs.open.as_ref().map(|space| json!({"space": space, "filter": self.procs.filter, "chosen": self.procs.chosen, "rows": self.procs.rows(space).iter().map(|p| json!({"pid": p.pid, "name": p.name, "pane": p.pane, "cpu": p.cpu})).collect::<Vec<_>>()})),
            "toasts": self.toasts().iter().map(|t| json!({"id": t.id, "level": t.level.name(), "text": t.text, "actions": t.actions.iter().map(|a| a.title).collect::<Vec<_>>(), "pane": t.pane, "closable": t.closable})).collect::<Vec<_>>(),
            "palette": self.command_palette.as_ref().map(|p| json!({"open": true, "mode": match p.mode { crate::command_palette::Mode::Root => "root", crate::command_palette::Mode::NewSpace => "new_space" }, "query": p.query, "rows": p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), "selected": p.selected})).unwrap_or(json!({"open": false})),
            "settings": {"open": self.settings_open, "tab": self.settings.tab.id(), "project": self.settings.project},
            "confirmation": self.confirmation.as_ref().map(|c| json!({"title": c.title, "message": c.message, "button": c.button})),
            "shortcuts_open": self.shortcuts_open,
            "sidebar": self.sidebar_json(),
            "title": self.title_json(),
            "usage": self.usage_json(),
            "pairing": {"open": self.pairing.open},
            "keymap": self.all_actions().iter().map(|a| (a.id.clone(), json!(self.keymap.symbol(&a.id)))).collect::<serde_json::Map<_, _>>(),
            "actions_log": self.actions_log,
            "presses": self.presses,
            "log": self.toast_log,
            "quarantine": {"spaces": self.quarantine.spaces, "panes": self.quarantine.panes},
            "bridge": {"visible": native["visible"], "key_window": native["key_window"], "app_active": native["app_active"], "root_focused": native["root_focused"], "errors": native["errors"]},
        });
        // The popped cards, bottom first, in window coordinates.
        out["cards"] = layers.iter().filter_map(|l| l.card.as_ref()).map(|c| json!({"panes": c.panes, "frame": [c.frame.x, c.frame.y, c.frame.w, c.frame.h], "grip": [c.grip.x, c.grip.y, c.grip.w, c.grip.h]})).collect();
        out["tray"] = self.tray_json();
        out["frames"] = json!(self.frames);
        out["header_layout"] = self.header_layout.borrow().layout.choices_json();
        out["header_settings_kind"] = json!(self.header_settings_kind);
        out["grid_motion"] = self.motion_json();
        out["card_drag"] = json!(self.card_drag.as_ref().map(|d| json!({"pane": d.pane, "resize": d.resize, "delta": [d.delta.0, d.delta.1],
            "edge": self.card_hot_edge().map(|e| crate::grid_drop::Zone::Edge(e).label())})));
        out["space_menu"] = json!(self.space_menu.as_ref().map(|(space, x, y)| json!({"space": space, "at": [x, y]})));
        out["edge_bar"] = json!(self.edge_bar().map(|b| [b.x, b.y, b.w, b.h]));
        out["composer_errors"] = json!(self.composer_errors);
        out["message_menu"] = self.message_menu_json();
        out["reply_ends"] = json!(self.columns.iter().map(|(a, c)| (a.clone(), json!(c.reply_ends))).collect::<serde_json::Map<_, _>>());
        out["read_aloud"] = json!(self.read_aloud);
        out
    }
}

/// The drop target a `/spacedrag` hover names; `me` is the window answering.
fn debug_space_target(text: &str, me: &str) -> Option<crate::sidebar::SpaceDropTarget> {
    use crate::sidebar::SpaceDropTarget as T;
    let parts: Vec<&str> = text.split(':').collect();
    Some(match parts[..] {
        ["window"] => T::Window(me.to_owned()),
        ["desktop"] => T::Desktop,
        ["sidebar", index] => T::Sidebar { window: me.to_owned(), index: index.parse().ok()? },
        ["window", window] => T::Window(window.to_owned()),
        ["sidebar", window, index] => T::Sidebar { window: window.to_owned(), index: index.parse().ok()? },
        ["card", session, index] => T::Card { session: session.to_owned(), index: index.parse().ok()? },
        _ => return None,
    })
}
