//! The command palette over the window: one search field over the verbs, the open spaces and
//! panes, the artifacts, the recent directories and the actions. A port of
//! apple/WheelKit/Sources/WheelKit/CommandPalette.swift's verb list (the desktop's) and New Space
//! mode: pure state, so the view draws it and a check reads it back as one.
use std::collections::HashMap;
use wheel_proto::{ArtifactInfo, ArtifactScope, DirEntry, RecentDir};

/// One action the palette offers: the registry's id and title, with the key the keymap gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteAction {
    pub id: String,
    pub title: String,
    pub shortcut: String,
}
pub const OPEN_FOLDER_ID: &str = "open-folder";

/// A thing to do, named before what it is done to. The root list is these; choosing one lists
/// what it can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    SwitchSpace,
    CloseSpace,
    GoToPane,
    ClosePane,
    NewPane,
    ChangePaneType,
    ShowArtifact,
    Run,
}
impl Verb {
    pub const ALL: [Verb; 8] = [Verb::SwitchSpace, Verb::CloseSpace, Verb::GoToPane, Verb::ClosePane, Verb::NewPane, Verb::ChangePaneType, Verb::ShowArtifact, Verb::Run];
    pub fn name(self) -> &'static str {
        match self {
            Verb::SwitchSpace => "switchSpace",
            Verb::CloseSpace => "closeSpace",
            Verb::GoToPane => "goToPane",
            Verb::ClosePane => "closePane",
            Verb::NewPane => "newPane",
            Verb::ChangePaneType => "changePaneType",
            Verb::ShowArtifact => "showArtifact",
            Verb::Run => "run",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Verb::SwitchSpace => "Switch to Space…",
            Verb::CloseSpace => "Close Space…",
            Verb::GoToPane => "Go to Pane…",
            Verb::ClosePane => "Close Pane…",
            Verb::NewPane => "New Pane…",
            Verb::ChangePaneType => "Change Pane Type…",
            Verb::ShowArtifact => "Show Artifact…",
            Verb::Run => "Run…",
        }
    }
    /// The search field's hint once the verb is chosen.
    pub fn prompt(self) -> String {
        format!("{} — pick one", self.title().trim_end_matches('…'))
    }
    /// Whether an action belongs to this verb's list rather than the root's own.
    fn claims(self, id: &str) -> bool {
        match self {
            Verb::NewPane => id == "split-right" || id == "split-down",
            Verb::ChangePaneType => id.starts_with("pane-") || id == "switch-view",
            Verb::Run => id.starts_with("run-"),
            _ => false,
        }
    }
}

/// What a palette row runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Action { id: String, title: String, shortcut: String },
    /// A space that is open now.
    Space { id: String, name: String, dir: String },
    /// A verb row: choosing it lists what it can act on.
    Verb(Verb),
    /// A close verb's target: `id` is the space or pane it closes.
    Target { verb: Verb, id: String, title: String, detail: String },
    /// Make another space in the folder of one that is open now; `name` is that space's.
    NewSpaceHere { dir: String, name: String },
    /// A pane of an open space; `space` is the id of the space holding it.
    Pane { id: String, space: String, name: String, space_name: String },
    /// An artifact a web pane can show, opened in a new pane split right of the focused one.
    Artifact { scope: ArtifactScope, name: String, title: String },
    /// A directory a space was made for and is not open now.
    Recent { dir: String },
    /// Switch the palette to New Space mode.
    NewSpaceMode,
    /// A folder New Space mode lists; `space` is the id of the space already open there.
    Folder { path: String, space: Option<String> },
    /// New Space mode's note that the typed folder does not exist.
    Missing { path: String },
    /// Ask for a folder with the system picker; New Space mode calls it "Choose in Finder…".
    OpenFolder { choose_in_finder: bool },
}
impl Item {
    pub fn id(&self) -> String {
        match self {
            Item::Action { id, .. } => format!("action:{id}"),
            Item::Space { id, .. } => format!("space:{id}"),
            Item::NewSpaceHere { dir, .. } => format!("new-space-here:{dir}"),
            Item::Verb(verb) => format!("verb:{}", verb.name()),
            Item::Target { verb, id, .. } => format!("target:{}/{id}", verb.name()),
            Item::Pane { id, .. } => format!("pane:{id}"),
            Item::Artifact { scope, name, .. } => format!("artifact:{}/{name}", crate::palette::scope_name(*scope)),
            Item::Recent { dir } => format!("recent:{dir}"),
            Item::NewSpaceMode => "action:new-space".into(),
            Item::Folder { path, .. } => format!("folder:{path}"),
            Item::Missing { path } => format!("missing:{path}"),
            Item::OpenFolder { .. } => format!("action:{OPEN_FOLDER_ID}"),
        }
    }
    /// What the log and a check call the row's kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Item::Action { .. } => "action",
            Item::Space { .. } => "space",
            Item::NewSpaceHere { .. } => "new_space_here",
            Item::Verb(_) => "verb",
            Item::Target { .. } => "target",
            Item::Pane { .. } => "pane",
            Item::Artifact { .. } => "artifact",
            Item::Recent { .. } => "recent",
            Item::NewSpaceMode => "new_space_mode",
            Item::Folder { .. } => "folder",
            Item::Missing { .. } => "missing",
            Item::OpenFolder { .. } => "open_folder",
        }
    }
    pub fn title(&self) -> String {
        match self {
            Item::Action { title, .. } => title.clone(),
            Item::Space { name, .. } => format!("Switch to {name}"),
            Item::NewSpaceHere { name, .. } => format!("New Space in {name}"),
            Item::Verb(verb) => verb.title().to_owned(),
            Item::Target { title, .. } => title.clone(),
            Item::Pane { name, .. } => name.clone(),
            Item::Artifact { name, title, .. } => if title.is_empty() { name.clone() } else { title.clone() },
            Item::Recent { dir } => format!("New Space in {}", last_component(dir)),
            Item::NewSpaceMode => "New Space…".into(),
            Item::Folder { path, .. } => last_component(path).to_owned(),
            Item::Missing { path } => format!("No folder at {path}"),
            Item::OpenFolder { choose_in_finder } => if *choose_in_finder { "Choose in Finder…".into() } else { "Open Folder…".into() },
        }
    }
    /// The second line of the row: the shortcut or the path.
    pub fn detail(&self) -> String {
        match self {
            Item::Action { shortcut, .. } => shortcut.clone(),
            Item::Space { dir, .. } => dir.clone(),
            Item::NewSpaceHere { dir, .. } => dir.clone(),
            Item::Target { detail, .. } => detail.clone(),
            Item::Pane { id, space_name, .. } => format!("{space_name} · {id}"),
            Item::Artifact { scope, name, .. } => format!("{} · {name}", crate::palette::scope_name(*scope)),
            Item::Recent { dir } => dir.clone(),
            Item::Folder { path, space } => if space.is_none() { path.clone() } else { format!("open · {path}") },
            Item::Verb(_) | Item::NewSpaceMode | Item::Missing { .. } | Item::OpenFolder { .. } => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Item,
    pub score: i32,
}
impl Row {
    pub fn id(&self) -> String {
        self.item.id()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenSpace {
    pub id: String,
    pub name: String,
    pub dir: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OpenPane {
    pub id: String,
    pub space: String,
    pub space_name: String,
    pub name: String,
}

/// What the palette searches: everything (`root`) or folders to make a space for (`new_space`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Root,
    NewSpace,
}
impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Root => "root",
            Mode::NewSpace => "new_space",
        }
    }
}

/// What one `dir_list` answered for a folder: its subfolders, or why it has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub dirs: Vec<DirEntry>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandPalette {
    pub mode: Mode,
    /// The verb whose targets the list shows, or None for the root list.
    pub verb: Option<Verb>,
    pub query: String,
    /// Index into `rows`; 0 when there are none.
    pub selected: usize,
    pub rows: Vec<Row>,
    spaces: Vec<OpenSpace>,
    panes: Vec<OpenPane>,
    artifacts: Vec<ArtifactInfo>,
    recents: Vec<RecentDir>,
    actions: Vec<PaletteAction>,
    roots: Vec<String>,
    listings: HashMap<String, Listing>,
}

impl CommandPalette {
    pub fn new(mode: Mode) -> Self {
        Self { mode, verb: None, query: String::new(), selected: 0, rows: vec![], spaces: vec![], panes: vec![], artifacts: vec![], recents: vec![], actions: vec![], roots: vec![], listings: HashMap::new() }
    }
    /// Whether Esc or Backspace on an empty field steps back instead of closing.
    pub fn is_staged(&self) -> bool {
        self.mode == Mode::NewSpace || self.verb.is_some()
    }
    /// The search field's hint.
    pub fn placeholder(&self) -> String {
        match (self.mode, self.verb) {
            (Mode::NewSpace, _) => "Folder name or path".into(),
            (_, Some(verb)) => verb.prompt(),
            _ => "Type an action, a space or a folder".into(),
        }
    }
    /// Choose a verb: the list becomes its targets, with an empty field.
    pub fn enter(&mut self, verb: Verb) {
        self.verb = Some(verb);
        self.query.clear();
        self.refilter();
    }
    /// Step back to the root list.
    pub fn leave_stage(&mut self) {
        self.verb = None;
        self.mode = Mode::Root;
        self.query.clear();
        self.refilter();
    }
    pub fn selected_item(&self) -> Option<&Item> {
        self.rows.get(self.selected).map(|r| &r.item)
    }
    /// Replace what the palette lists from; the query stays and the selection returns to the top.
    pub fn list(&mut self, spaces: Vec<OpenSpace>, panes: Vec<OpenPane>, artifacts: Vec<ArtifactInfo>, recents: Vec<RecentDir>, actions: Vec<PaletteAction>) {
        self.spaces = spaces;
        self.panes = panes;
        self.artifacts = artifacts;
        self.recents = recents;
        self.actions = actions;
        self.refilter();
    }
    /// Switch to New Space mode with an empty field.
    pub fn enter_new_space(&mut self) {
        self.verb = None;
        self.mode = Mode::NewSpace;
        self.query.clear();
        self.refilter();
    }
    /// The browsing roots, as absolute paths.
    pub fn set_roots(&mut self, roots: Vec<String>) {
        self.roots = roots;
        self.refilter();
    }
    /// Keep the answer for `parent`, replacing the last one.
    pub fn listed(&mut self, parent: &str, listing: Listing) {
        self.listings.insert(listing_key(parent), listing);
        self.refilter();
    }
    /// The folder the query names the inside of, as it must be asked of the daemon, or None when
    /// the query is not a path or the palette is not in New Space mode.
    pub fn browsed_parent(&self) -> Option<String> {
        if self.mode != Mode::NewSpace {
            return None;
        }
        split_path(&self.query).map(|(parent, _)| listing_key(&parent))
    }
    /// Tab: the selected folder's path plus `/`, so the next row list is its subfolders. A path
    /// query keeps the way it was typed (`~/Projects/wh` becomes `~/Projects/wheel/`).
    pub fn tab(&mut self) {
        if self.mode != Mode::NewSpace {
            return;
        }
        let Some(Item::Folder { path, .. }) = self.selected_item().cloned() else { return };
        self.query = match split_path(&self.query) {
            Some((parent, _)) => format!("{parent}{}/", last_component(&path)),
            None => if path.ends_with('/') { path } else { format!("{path}/") },
        };
        self.refilter();
    }
    pub fn type_text(&mut self, text: &str) {
        if text == self.query {
            return;
        }
        self.query = text.to_owned();
        self.refilter();
    }
    /// ↑ and ↓ stop at the first and last row.
    pub fn move_by(&mut self, by: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + by).clamp(0, self.rows.len() as isize - 1) as usize;
    }
    pub fn select(&mut self, index: usize) {
        if index < self.rows.len() {
            self.selected = index;
        }
    }
    fn refilter(&mut self) {
        self.rows = match self.mode {
            Mode::Root => verb_rows(self.verb, &self.spaces, &self.panes, &self.artifacts, &self.recents, &self.actions, &self.query),
            Mode::NewSpace => new_space_rows(&self.spaces, &self.recents, &self.roots, &self.listings, &self.query),
        };
        self.selected = 0;
    }
}

/// A path query split at its last `/`: the folder to list (as typed, with its slash) and the
/// rest, which picks among that folder's subfolders. None when the query starts with neither
/// `~` nor `/`.
pub fn split_path(query: &str) -> Option<(String, String)> {
    let q = query.trim_start_matches(' ');
    if !(q.starts_with('~') || q.starts_with('/')) {
        return None;
    }
    match q.rfind('/') {
        Some(slash) => Some((q[..=slash].to_owned(), q[slash + 1..].to_owned())),
        None => Some(("~/".into(), q[1..].to_owned())),
    }
}
/// The form of a folder the palette stores an answer under: no trailing slash, `/` for the root.
pub fn listing_key(parent: &str) -> String {
    let mut key = parent.to_owned();
    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }
    key
}
pub fn last_component(path: &str) -> &str {
    let trimmed = if path.ends_with('/') && path.len() > 1 { &path[..path.len() - 1] } else { path };
    trimmed.rsplit('/').find(|p| !p.is_empty()).unwrap_or(path)
}

type Entry = (Item, Vec<String>);

/// New Space mode. A path query lists the subfolders of its folder, scored by the rest; any
/// other query scores the recent directories and every root's subfolders. A folder the daemon
/// could not list gives a "No folder at" row. "Choose in Finder…" is always last.
fn new_space_rows(spaces: &[OpenSpace], recents: &[RecentDir], roots: &[String], listings: &HashMap<String, Listing>, query: &str) -> Vec<Row> {
    let open_id = |dir: &str| spaces.iter().find(|s| s.dir == dir).map(|s| s.id.clone());
    let listed_folder = |d: &DirEntry| (Item::Folder { path: d.path.clone(), space: d.space.clone().or_else(|| open_id(&d.path)) }, vec![d.name.clone()]);
    let mut items: Vec<Entry> = vec![];
    let mut needle = query.to_owned();
    if let Some((parent, rest)) = split_path(query) {
        needle = rest;
        if let Some(listing) = listings.get(&listing_key(&parent)) {
            if listing.error.is_some() {
                items.push((Item::Missing { path: parent }, vec![]));
            } else {
                items.extend(listing.dirs.iter().map(listed_folder));
            }
        }
    } else {
        let mut seen = std::collections::HashSet::new();
        let mut recent: Vec<&RecentDir> = recents.iter().collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.opened_ms));
        for r in recent {
            if seen.insert(r.dir.clone()) {
                items.push((Item::Folder { path: r.dir.clone(), space: open_id(&r.dir) }, vec![r.dir.clone()]));
            }
        }
        for root in roots {
            for d in listings.get(&listing_key(root)).map(|l| l.dirs.as_slice()).unwrap_or(&[]) {
                if seen.insert(d.path.clone()) {
                    items.push(listed_folder(d));
                }
            }
        }
    }
    let mut rows = match items.first() {
        Some((item @ Item::Missing { .. }, _)) => vec![Row { item: item.clone(), score: 0 }],
        _ => rank(items, &needle),
    };
    rows.push(Row { item: Item::OpenFolder { choose_in_finder: true }, score: 0 });
    rows
}

/// The desktop's root list. A verb chosen: its targets, searched by the query. No verb and no
/// query: the verbs that have something to act on, then the actions no verb claims. A query:
/// the verbs and every verb-and-target row together, so typing a folder finds what can be done
/// to it and typing a verb finds what it can be done to.
fn verb_rows(verb: Option<Verb>, spaces: &[OpenSpace], panes: &[OpenPane], artifacts: &[ArtifactInfo], recents: &[RecentDir], actions: &[PaletteAction], query: &str) -> Vec<Row> {
    let action_item = |a: &PaletteAction| if a.id == OPEN_FOLDER_ID { Item::OpenFolder { choose_in_finder: false } } else { Item::Action { id: a.id.clone(), title: a.title.clone(), shortcut: a.shortcut.clone() } };
    let unclaimed = |a: &&PaletteAction| a.id != "new-space" && !Verb::ALL.iter().any(|v| v.claims(&a.id));
    let targets = |verb: Verb| -> Vec<Entry> {
        match verb {
            Verb::SwitchSpace => spaces.iter().map(|s| (Item::Space { id: s.id.clone(), name: s.name.clone(), dir: s.dir.clone() }, vec![format!("Switch to {}", s.name), s.dir.clone()])).collect(),
            Verb::CloseSpace => spaces.iter().map(|s| (Item::Target { verb, id: s.id.clone(), title: format!("Close Space {}", s.name), detail: s.dir.clone() }, vec![format!("Close Space {}", s.name), s.dir.clone()])).collect(),
            Verb::GoToPane => panes.iter().map(|p| (Item::Pane { id: p.id.clone(), space: p.space.clone(), name: p.name.clone(), space_name: p.space_name.clone() }, vec![format!("Go to {}", p.name), format!("{} {}", p.space_name, p.name)])).collect(),
            Verb::ClosePane => panes.iter().map(|p| (Item::Target { verb, id: p.id.clone(), title: format!("Close Pane {}", p.name), detail: format!("{} · {}", p.space_name, p.id) }, vec![format!("Close Pane {}", p.name), format!("{} {}", p.space_name, p.name)])).collect(),
            Verb::ShowArtifact => artifacts.iter().map(|a| (Item::Artifact { scope: a.scope, name: a.name.clone(), title: a.title.clone() }, vec![format!("Show {}", a.title), a.name.clone()])).collect(),
            Verb::NewPane | Verb::ChangePaneType | Verb::Run => actions.iter().filter(|a| verb.claims(&a.id)).map(|a| (action_item(a), vec![a.title.clone()])).collect(),
        }
    };
    let populated: Vec<Verb> = Verb::ALL.into_iter().filter(|v| !targets(*v).is_empty()).collect();
    if let Some(verb) = verb {
        return rank(targets(verb), query);
    }
    let needle = query.trim();
    if needle.is_empty() {
        let mut rows: Vec<Row> = populated.iter().map(|v| Row { item: Item::Verb(*v), score: 0 }).collect();
        rows.push(Row { item: Item::NewSpaceMode, score: 0 });
        rows.extend(actions.iter().filter(unclaimed).map(|a| Row { item: action_item(a), score: 0 }));
        return rows;
    }
    let mut items: Vec<Entry> = populated.iter().map(|v| (Item::Verb(*v), vec![v.title().to_owned()])).collect();
    items.push((Item::NewSpaceMode, vec!["New Space…".into()]));
    for v in &populated {
        items.extend(targets(*v));
    }
    for s in spaces {
        items.push((Item::NewSpaceHere { dir: s.dir.clone(), name: s.name.clone() }, vec![format!("New Space in {}", s.name), s.dir.clone()]));
    }
    let mut recent: Vec<&RecentDir> = recents.iter().filter(|r| !spaces.iter().any(|s| s.dir == r.dir)).collect();
    recent.sort_by_key(|r| std::cmp::Reverse(r.opened_ms));
    for r in recent {
        items.push((Item::Recent { dir: r.dir.clone() }, vec![format!("New Space in {}", last_component(&r.dir)), r.dir.clone()]));
    }
    items.extend(actions.iter().filter(unclaimed).map(|a| (action_item(a), vec![a.title.clone()])));
    rank(items, needle)
}

/// Every entry the query matches, best score first, ties in the entries' order; all of them in
/// order when the query is empty.
fn rank(items: Vec<Entry>, query: &str) -> Vec<Row> {
    let query = query.trim();
    if query.is_empty() {
        return items.into_iter().map(|(item, _)| Row { item, score: 0 }).collect();
    }
    let mut scored: Vec<(usize, Row)> = items.into_iter().enumerate().filter_map(|(i, (item, texts))| {
        texts.iter().filter_map(|t| score(query, t)).max().map(|s| (i, Row { item, score: s }))
    }).collect();
    scored.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(_, r)| r).collect()
}

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | '.' | ' ')
}

/// The best score of the query's letters appearing in order in `text`, ignoring case, or None
/// when they do not. Each matched letter earns 1; 8 more at a word start (the text's start,
/// after `/ - _ .` or a space, or a capital after a lowercase letter); 5 more when it follows
/// the previous matched letter directly; 3 more inside the path's last component. A gap
/// between two matched letters costs 1 per skipped letter, capped at 3.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let want: Vec<char> = query.to_lowercase().chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    // Lowercasing can change the length (`İ`); positions would then point at other letters.
    if lower.len() != chars.len() || want.is_empty() || want.len() > chars.len() {
        return None;
    }
    let last_start = chars.iter().rposition(|&c| c == '/').map(|i| i + 1).unwrap_or(0);
    let base = |j: usize| -> i32 {
        let mut s = 1;
        if j == 0 || is_separator(chars[j - 1]) || (chars[j].is_uppercase() && chars[j - 1].is_lowercase()) {
            s += 8;
        }
        if j >= last_start && last_start > 0 {
            s += 3;
        }
        s
    };
    // best[j]: the best score with the query's letters so far matched, the latest at text position j.
    let mut best: Vec<Option<i32>> = (0..chars.len()).map(|j| (lower[j] == want[0]).then(|| base(j))).collect();
    for &letter in &want[1..] {
        let mut next: Vec<Option<i32>> = vec![None; chars.len()];
        for j in 0..chars.len() {
            if lower[j] != letter {
                continue;
            }
            let top = (0..j).filter_map(|i| best[i].map(|b| b + if i == j - 1 { 5 } else { -((j - i - 1).min(3) as i32) })).max();
            next[j] = top.map(|t| t + base(j));
        }
        best = next;
    }
    best.into_iter().flatten().max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> CommandPalette {
        let mut p = CommandPalette::new(Mode::Root);
        let action = |id: &str, title: &str, shortcut: &str| PaletteAction { id: id.into(), title: title.into(), shortcut: shortcut.into() };
        p.list(
            vec![OpenSpace { id: "s1".into(), name: "wheel".into(), dir: "/w/wheel".into() }],
            vec![OpenPane { id: "p1".into(), space: "s1".into(), space_name: "wheel".into(), name: "Terminal".into() }],
            vec![],
            vec![RecentDir { dir: "/w/other".into(), opened_ms: 5 }, RecentDir { dir: "/w/wheel".into(), opened_ms: 9 }],
            vec![action("split-right", "Split Right", "⌘D"), action("close-pane", "Close Pane", "⌘W"), action("run-build", "Run build", ""), action("new-space", "New Space…", "⌘T"), action(OPEN_FOLDER_ID, "Open Folder…", "⌘O")],
        );
        p
    }

    #[test]
    fn the_root_list_is_verbs_then_unclaimed_actions() {
        let p = palette();
        let ids: Vec<String> = p.rows.iter().map(Row::id).collect();
        assert_eq!(ids, ["verb:switchSpace", "verb:closeSpace", "verb:goToPane", "verb:closePane", "verb:newPane", "verb:run", "action:new-space", "action:close-pane", "action:open-folder"]);
        assert_eq!(p.rows[8].item.kind(), "open_folder");
    }

    #[test]
    fn a_verb_lists_its_targets_and_steps_back() {
        let mut p = palette();
        p.enter(Verb::Run);
        assert!(p.is_staged());
        assert_eq!(p.placeholder(), "Run — pick one");
        assert_eq!(p.rows.iter().map(Row::id).collect::<Vec<_>>(), ["action:run-build"]);
        p.leave_stage();
        assert!(!p.is_staged());
        assert_eq!(p.rows.len(), 9);
    }

    #[test]
    fn a_query_searches_verbs_targets_and_folders_together() {
        let mut p = palette();
        p.type_text("split");
        assert_eq!(p.rows[0].id(), "action:split-right");
        p.type_text("other");
        assert_eq!(p.rows[0].id(), "recent:/w/other");
        assert_eq!(p.rows[0].item.title(), "New Space in other");
        p.type_text("close pane");
        assert_eq!(p.rows[0].item.kind(), "verb");
        assert!(p.rows.iter().any(|r| r.id() == "target:closePane/p1"));
        p.type_text("zzzz");
        assert!(p.rows.is_empty());
    }

    #[test]
    fn new_space_mode_lists_recents_then_roots_and_tab_completes() {
        let mut p = palette();
        p.enter_new_space();
        assert_eq!(p.mode, Mode::NewSpace);
        p.set_roots(vec!["~/Projects".into()]);
        let dir = |name: &str, path: &str| DirEntry { name: name.into(), path: path.into(), space: None };
        p.listed("~/Projects", Listing { dirs: vec![dir("alpha", "/h/Projects/alpha"), dir("wheel", "/w/wheel")], error: None });
        let ids: Vec<String> = p.rows.iter().map(Row::id).collect();
        assert_eq!(ids, ["folder:/w/wheel", "folder:/w/other", "folder:/h/Projects/alpha", "action:open-folder"]);
        assert_eq!(p.rows[0].item.detail(), "open · /w/wheel");
        p.type_text("~/Pro");
        assert_eq!(p.browsed_parent().as_deref(), Some("~"));
        p.listed("~/", Listing { dirs: vec![dir("Projects", "/h/Projects")], error: None });
        assert_eq!(p.rows[0].id(), "folder:/h/Projects");
        p.tab();
        assert_eq!(p.query, "~/Projects/");
        assert_eq!(p.rows.iter().map(Row::id).collect::<Vec<_>>(), ["folder:/h/Projects/alpha", "folder:/w/wheel", "action:open-folder"]);
        p.type_text("/nowhere/");
        p.listed("/nowhere", Listing { dirs: vec![], error: Some("no such folder".into()) });
        assert_eq!(p.rows[0].item.title(), "No folder at /nowhere/");
    }

    #[test]
    fn scoring_prefers_word_starts_and_runs() {
        assert!(score("wh", "/w/wheel") > score("wh", "/w/awhile"));
        assert!(score("sr", "Split Right").unwrap() > score("sr", "sort").unwrap());
        assert_eq!(score("x", "abc"), None);
        assert_eq!(split_path("~proj"), Some(("~/".into(), "proj".into())));
        assert_eq!(listing_key("/a/b//"), "/a/b");
        assert_eq!(last_component("/a/b/"), "b");
    }
}
