//! The transcript's links (`crate::chat_links`): which settings a chat reads, the files wheeld
//! is asked about, and what a click does. A plain click opens a link in Wheel (a page or a file in
//! a browser pane beside the chat, a pane focused, an artifact beside the chat); Command-click
//! opens a page or a file with the system's default app instead.
use super::*;
use crate::chat_links::{self, Knowledge, Probe, Settings, Target};
use serde_json::{Value, json};
use std::cell::Cell;

/// What the client knows and is waiting for about the links of every transcript.
#[derive(Default)]
pub struct LinkState {
    /// `chat.links.*` as resolved for each space; `None` is the machine's.
    pub settings: HashMap<Option<String>, Settings>,
    /// Each (directory, path) asked about, and the answer.
    probes: RefCell<HashMap<(String, String), Probe>>,
    /// Files a frame found that wheeld has not been asked about yet.
    asks: RefCell<Vec<(String, String)>>,
    reqs: HashMap<u64, (String, String)>,
    /// Each agent's row count when its directory's answers were last taken.
    rows_seen: HashMap<String, usize>,
    /// Moves when what a link shows changed (a probe's answer, a pane's chip, an agent's space
    /// or directory, a setting), so a reply's cached links are made again.
    generation: Cell<u64>,
    /// What the links last read from the layout: every pane's chip, every agent's space and
    /// directory. The generation moves only when it differs.
    read_from_layout: Vec<String>,
    /// Replies whose links were made again, and columns measured again from their first row
    /// because the generation moved: counts for `/state` `links`.
    pub(super) relinked: Cell<u64>,
    pub(super) remeasured: Cell<u64>,
    /// A click the last frame resolved to a link: (agent, address, with Command).
    pub(super) pending: Option<(String, String, bool)>,
    /// Command was down at the last release.
    pub(super) up_command: bool,
    /// The pointer is over a link.
    pub(super) hover: bool,
    /// What the last click on a link did, for `/state`.
    pub last: Option<Value>,
}
impl LinkState {
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }
    pub fn moved(&self) {
        self.generation.set(self.generation.get() + 1);
    }
    /// Files asked about whose answers have not come.
    pub fn waiting(&self) -> usize {
        self.reqs.len()
    }
}

impl Client {
    /// Where agent `agent`'s pane is: its space, its pane id, and the directory its relative
    /// paths start from (the agent's own, else the space's).
    pub(super) fn agent_place(&self, agent: &str) -> Option<(String, String, String)> {
        let (space, leaf) = self.spaces.iter().find_map(|s| s.all().find(|l| l.agent.as_deref() == Some(agent)).map(|l| (s, l)))?;
        let dir = self.transcripts.get(agent).and_then(|t| t.footer.as_ref()).map(|f| f.directory.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| space.cwd.clone());
        Some((space.id.clone(), leaf.id.clone(), dir))
    }
    fn link_settings(&self, space: Option<&str>) -> Settings {
        self.links.settings.get(&space.map(str::to_owned)).or_else(|| self.links.settings.get(&None)).cloned().unwrap_or_default()
    }
    /// Take one resolved setting for `space`.
    pub(super) fn link_setting(&mut self, space: Option<&str>, key: &str, value: &str) {
        if key.starts_with("chat.links.") {
            let base = self.link_settings(space);
            self.links.settings.entry(space.map(str::to_owned)).or_insert(base).apply(key, value);
            self.links.moved();
        }
    }
    /// The chip for pane `id`: `p1958 · Claude`; none when no space holds it.
    fn pane_chip(&self, id: &str) -> Option<String> {
        self.spaces.iter().find_map(|s| s.all().find(|l| l.id == id)).map(|l| format!("{id} · {}", self.pane_label(l)))
    }
    /// Run `make` with what the client knows about `agent`'s chat: its settings and the files and
    /// panes a text names. A file not asked about yet is queued for wheeld.
    fn with_knowledge<R>(&self, agent: &str, make: impl FnOnce(&Settings, &Knowledge) -> R) -> R {
        let place = self.agent_place(agent);
        let settings = self.link_settings(place.as_ref().map(|p| p.0.as_str()));
        let dir = place.map(|p| p.2).unwrap_or_default();
        let file = |path: &str| {
            let key = (dir.clone(), path.to_owned());
            if let Some(p) = self.links.probes.borrow().get(&key) {
                return p.clone();
            }
            self.links.probes.borrow_mut().insert(key.clone(), Probe::Unknown);
            self.links.asks.borrow_mut().push(key);
            Probe::Unknown
        };
        let pane = |id: &str| self.pane_chip(id);
        make(&settings, &Knowledge { file: &file, pane: &pane })
    }
    /// A plain text of `agent`'s transcript with its links.
    pub(super) fn linked_text(&self, agent: &str, text: &str) -> Text {
        let plain = Text { text: text.to_owned(), runs: vec![] };
        self.with_knowledge(agent, |s, k| chat_links::link_text(&plain, s, k, &mut vec![]))
    }
    /// A reply's blocks with their links.
    pub(super) fn linked_blocks(&self, agent: &str, blocks: &[Block]) -> Vec<Block> {
        self.links.relinked.set(self.links.relinked.get() + 1);
        self.with_knowledge(agent, |s, k| chat_links::link_blocks(blocks, s, k, &mut vec![]))
    }
    /// Move the generation when what links read from the layout changed: a pane's chip, or an
    /// agent's space or directory.
    fn read_layout(&mut self) {
        let mut read: Vec<String> = self.spaces.iter().flat_map(|s| s.all().map(|l| format!("{} · {}", l.id, self.pane_label(l)))).collect();
        read.extend(self.transcripts.keys().filter_map(|agent| self.agent_place(agent).map(|(space, _, dir)| format!("{agent} {space} {dir}"))));
        read.sort();
        if read != self.links.read_from_layout {
            self.links.read_from_layout = read;
            self.links.moved();
        }
    }
    /// Ask wheeld about the files the last frame found. A transcript that gained a row (a tool
    /// may have made or removed a file) asks again about its directory's files, keeping each
    /// answer until the new one comes.
    pub(super) fn ask_probes(&mut self) {
        self.read_layout();
        let grown: Vec<(String, usize)> = self.transcripts.iter()
            .filter(|(agent, t)| self.links.rows_seen.get(*agent) != Some(&t.rows.len()))
            .map(|(agent, t)| (agent.clone(), t.rows.len())).collect();
        for (agent, rows) in grown {
            self.links.rows_seen.insert(agent.clone(), rows);
            if let Some((_, _, dir)) = self.agent_place(&agent) {
                let known: Vec<(String, String)> = self.links.probes.borrow().iter().filter(|((d, _), p)| *d == dir && **p != Probe::Unknown).map(|(k, _)| k.clone()).collect();
                let waiting: HashSet<(String, String)> = self.links.reqs.values().cloned().collect();
                self.links.asks.borrow_mut().extend(known.into_iter().filter(|k| !waiting.contains(k)));
            }
        }
        let asks: Vec<(String, String)> = self.links.asks.borrow_mut().drain(..).collect();
        if asks.is_empty() || self.link != Link::Up {
            // Unasked, they are asked again once the link is up; a file answered before keeps its answer.
            self.links.probes.borrow_mut().retain(|key, p| *p != Probe::Unknown || !asks.contains(key));
            return;
        }
        for (dir, path) in asks {
            let req = self.req();
            self.links.reqs.insert(req, (dir.clone(), path.clone()));
            self.send(&ClientMsg::FilePeek { req: Some(req), path, cwd: (!dir.is_empty()).then_some(dir), line: None, context: Some(0) });
        }
    }
    /// wheeld's answer about a file. Returns whether the answer was one of these.
    pub(super) fn on_file_probe(&mut self, req: u64, resolved: &str, found: bool) -> bool {
        let Some(key) = self.links.reqs.remove(&req) else { return false };
        self.log(format!("link file {} in {}: {}", key.1, key.0, if found { resolved } else { "missing" }));
        let answer = if found { Probe::Found(resolved.to_owned()) } else { Probe::Missing };
        // The same answer again (a re-ask after a row) changes nothing a link shows.
        if self.links.probes.borrow_mut().insert(key, answer.clone()).as_ref() != Some(&answer) {
            self.links.moved();
        }
        true
    }
    /// The link address under `point` in `agent`'s transcript, from the last frame's texts.
    pub(super) fn link_at(&self, agent: &str, point: Point<Pixels>) -> Option<String> {
        let drawn = self.drawn.borrow();
        let d = drawn.iter().find(|d| d.agent == agent && !d.links.is_empty() && d.layout.bounds().contains(&point))?;
        let at = d.layout.index_for_position(point).ok()?;
        d.links.iter().find(|(r, _)| r.contains(&at)).map(|(_, a)| a.clone())
    }
    /// Whether the pointer at `point` is over any transcript link.
    pub(super) fn over_link(&self, point: Point<Pixels>) -> bool {
        let drawn = self.drawn.borrow();
        drawn.iter().filter(|d| !d.links.is_empty() && d.layout.bounds().contains(&point)).any(|d| {
            d.layout.index_for_position(point).ok().is_some_and(|at| d.links.iter().any(|(r, _)| r.contains(&at)))
        })
    }
    /// Do what a click on `address` in `agent`'s chat does; `system` is Command held.
    pub(super) fn open_link(&mut self, agent: &str, address: &str, system: bool, cx: &mut Context<Self>) {
        let Some(target) = Target::from_address(address) else {
            self.log(format!("link from {agent}: {address} is no link"));
            return;
        };
        let place = self.agent_place(agent);
        let settings = self.link_settings(place.as_ref().map(|p| p.0.as_str()));
        // A run or copy address carries the process's key, which never goes to the log.
        let shown = match &target { Target::Run(_) => "run".to_owned(), Target::Copy(_) => "copy".to_owned(), _ => address.to_owned() };
        let mut record = json!({"agent": agent, "address": shown, "command": system});
        // A page or a file the system opens; a hidden check app records it and opens nothing.
        let system_open = |this: &mut Self, url: String, record: &mut Value, cx: &mut Context<Self>| {
            record["opened"] = json!("system");
            record["url"] = json!(url);
            if this.hidden {
                record["skipped"] = json!(true);
            } else {
                cx.open_url(&url);
            }
        };
        let beside = |this: &mut Self, url: Option<String>, artifact: Option<String>, record: &mut Value| {
            let Some((_, pane, _)) = this.agent_place(agent) else { return };
            record["opened"] = json!("pane");
            record["beside"] = json!(pane);
            record["url"] = json!(url);
            record["artifact"] = json!(artifact);
            this.send_layout(|req| ClientMsg::PaneOpen { req: Some(req), pane, dir: Some(wheel_proto::Dir::Right), url, home: None, artifact, scope: None, focus: false, site: None });
        };
        match target {
            Target::Web(url) => {
                if system || settings.web_system {
                    system_open(self, url, &mut record, cx);
                } else {
                    beside(self, Some(url), None, &mut record);
                }
            }
            Target::File { path, line } => {
                let path = match (&place, path.starts_with('/') || path.starts_with('~')) {
                    (Some((_, _, dir)), false) => format!("{}/{path}", dir.trim_end_matches('/')),
                    _ => path,
                };
                if system && chat_links::runs_when_opened(&path) {
                    // Opening an app or a script runs it: Finder shows it instead.
                    record["opened"] = json!("reveal");
                    record["url"] = json!(path);
                    if !self.hidden {
                        cx.reveal_path(std::path::Path::new(&chat_links::expand_home(&path)));
                    }
                } else if system {
                    system_open(self, chat_links::file_url(&path, None), &mut record, cx);
                } else {
                    beside(self, Some(chat_links::file_url(&path, line)), None, &mut record);
                }
            }
            Target::Pane(id) => {
                record["opened"] = json!("focus");
                record["pane"] = json!(id);
                if self.spaces.iter().any(|s| s.all().any(|l| l.id == id)) {
                    self.show_pane(&id);
                } else {
                    record["missing"] = json!(true);
                }
            }
            Target::Space(name) => {
                record["opened"] = json!("space");
                match self.spaces.iter().find(|s| s.id == name || s.name == name).map(|s| s.id.clone()) {
                    Some(id) => {
                        record["space"] = json!(id);
                        self.show_space_anywhere(&id);
                    }
                    None => record["missing"] = json!(true),
                }
            }
            Target::Artifact(name) => beside(self, None, Some(name), &mut record),
            Target::Run(command) => {
                if let Some((_, pane, _)) = place {
                    record["opened"] = json!("run");
                    self.send_layout(|req| ClientMsg::PaneRun { req: Some(req), pane, command, focus: false, site: None });
                }
            }
            Target::Copy(text) => {
                record["opened"] = json!("copy");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Target::System(url) => system_open(self, url, &mut record, cx),
        }
        self.log(format!("link from {agent}: {record}"));
        self.links.last = Some(record);
    }
}
