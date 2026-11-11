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
                let Some(pane) = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| l.agent.as_deref() == Some(agent.as_str())).map(|l| l.id.clone()) else {
                    return refuse(MISSING, "no agent pane shows that agent");
                };
                match path {
                    "/agent" => {
                        self.drafts.insert(agent.clone(), String::new());
                        self.send(&ClientMsg::AgentSend { req: None, agent: agent.clone(), text: request.body.clone() });
                    }
                    "/agent/draft" => {
                        self.drafts.insert(agent.clone(), request.body.clone());
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
            _ => return refuse(MISSING, "no such route"),
        };
        cx.notify();
        ok(answer.unwrap_or_else(|| self.state_json()))
    }

    /// Where the header's controls are, for a check that presses them with real events.
    fn geometry_json(&self) -> Value {
        let chips: Vec<Value> = self.header_chips().into_iter().map(|c| {
            let hit = match &c.hit { Hit::Session(id) => format!("session:{id}"), Hit::Space(id) => format!("space:{id}"), Hit::Processes => "processes".into(), Hit::NewWindow => "new-window".into() };
            json!({"hit": hit, "label": c.label, "active": c.active, "frame": [c.rect.x, c.rect.y, c.rect.w, c.rect.h]})
        }).collect();
        json!({"viewport": [self.viewport.0, self.viewport.1], "header": chips})
    }

    /// The client as JSON: what the Swift app's `/state` carries where the GPUI client has it.
    pub fn state_json(&self) -> Value {
        let native = self.native.status();
        let (leaves, _) = self.scene();
        let mut panes = serde_json::Map::new();
        for space in &self.spaces {
            for leaf in leaves_of(&space.root) {
                let placed = leaves.iter().find(|p| p.leaf.id == leaf.id);
                let mut entry = json!({
                    "space": space.id, "kind": leaf.kind, "label": leaf.label, "term": leaf.term, "url": leaf.url, "agent": leaf.agent,
                    "harness": leaf.harness, "status": leaf.status, "visible": placed.is_some(),
                    "frame": placed.map(|p| json!([p.rect.x, p.rect.y, p.rect.w, p.rect.h])),
                    "native": self.native_panes.contains(&leaf.id), "focused": self.focus.current.as_deref() == Some(leaf.id.as_str()),
                    "palette": self.palettes.get(&leaf.id).map(|p| json!({"query": p.query, "rows": p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), "selected": p.selected})),
                    "browser": self.browsers.get(&leaf.id).map(|b| json!({"address": b.address, "failure": b.failure})),
                    "bridge": native["panes"][leaf.id.as_str()],
                });
                if let Some(agent) = &leaf.agent {
                    entry["draft"] = json!(self.drafts.get(agent).cloned().unwrap_or_default());
                    entry["rows"] = json!(self.transcripts.get(agent).map(|t| t.rows.len()).unwrap_or(0));
                }
                panes.insert(leaf.id.clone(), entry);
            }
        }
        let procs: serde_json::Map<String, Value> = self.procs.by_space.iter().map(|(space, list)| (space.clone(), json!(list))).collect();
        json!({
            "daemon_url": self.endpoint.ws.split('?').next().unwrap_or_default(),
            "connection": match &self.link { Link::Connecting => "connecting".to_owned(), Link::Up => "connected".to_owned(), Link::Down(why) => format!("unreachable: {why}"), Link::Incompatible { .. } => "incompatible".to_owned() },
            "banner": self.link.banner(),
            "server_protocol": json!({"version": self.server_protocol.unwrap_or(crate::daemon::PROTOCOL), "client": crate::daemon::PROTOCOL}),
            "server": json!({"restart_available": self.server_control.available(), "restarting": self.server_restarting, "problem": self.server_problem}),
            "hidden": self.hidden,
            "window": self.window_id,
            "sessions": self.sessions.iter().map(|s| json!({"id": s.id, "name": s.name, "spaces": s.spaces})).collect::<Vec<_>>(),
            "session": self.session_id,
            "active": self.space_id,
            "spaces": self.spaces.iter().map(|s| json!({"id": s.id, "name": s.name, "cwd": s.cwd, "status": s.status, "panes": leaves_of(&s.root).iter().map(|l| l.id.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>(),
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
            "pairing": {"open": self.pairing.open},
            "keymap": self.all_actions().iter().map(|a| (a.id.clone(), json!(self.keymap.symbol(&a.id)))).collect::<serde_json::Map<_, _>>(),
            "actions_log": self.actions_log,
            "presses": self.presses,
            "log": self.toast_log,
            "quarantine": {"spaces": self.quarantine.spaces, "panes": self.quarantine.panes},
            "bridge": {"visible": native["visible"], "key_window": native["key_window"], "app_active": native["app_active"], "root_focused": native["root_focused"], "errors": native["errors"]},
        })
    }
}
