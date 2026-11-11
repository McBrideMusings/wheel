//! The artifact palette inside one pane: what it lists, what is typed into its field and which
//! row is selected. A port of apple/WheelKit/Sources/WheelKit/ArtifactPalette.swift: pure state,
//! so the view draws it and a check reads it back as one.
use wheel_proto::{ArtifactInfo, ArtifactScope};

/// What the palette knows about the listing.
#[derive(Clone, Debug, PartialEq)]
pub enum Listing {
    /// `artifact_ls` is out and unanswered.
    Loading,
    /// The daemon's `pane = true` artifacts for this pane's space.
    Ready(Vec<ArtifactInfo>),
    /// The daemon refused; its message, verbatim.
    Failed(String),
}

/// One listed row, with the character offsets of its title and name the query matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub info: ArtifactInfo,
    pub title_match: Vec<usize>,
    pub name_match: Vec<usize>,
}
impl Row {
    pub fn id(&self) -> String {
        row_id(&self.info)
    }
    /// The title is a line of its own only when it says something the name does not.
    pub fn has_title(&self) -> bool {
        self.info.title != self.info.name
    }
}
pub fn row_id(info: &ArtifactInfo) -> String {
    format!("{}/{}", scope_name(info.scope), info.name)
}
pub fn scope_name(scope: ArtifactScope) -> &'static str {
    match scope {
        ArtifactScope::Project => "project",
        ArtifactScope::Machine => "machine",
    }
}

/// One footer key hint, with whether pressing it does anything now.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub symbol: &'static str,
    pub word: &'static str,
    pub enabled: bool,
}
fn key(symbol: &'static str, word: &'static str, enabled: bool) -> Key {
    Key { symbol, word, enabled }
}

pub const PLACEHOLDER: &str = "Search artifacts by title or name";
pub const NO_ARTIFACTS_FIELD: &str = "No artifacts to search";
pub const UNAVAILABLE_FIELD: &str = "Search unavailable";
pub const EMPTY_TITLE: &str = "No artifacts for this space";
pub const EMPTY_HELP: &str = "An artifact is offered here when its wheel.toml entry says pane = true.";
pub const EMPTY_ASK: &str = "Ask an agent to make one, or run";
pub const EMPTY_COMMAND: &str = "wheel artifact new dash --pane";
pub const FAILED_TITLE: &str = "Couldn’t list artifacts";
pub const FAILED_LEAD: &str = "wheeld refused the request and said:";
pub const RETRY: &str = "Try Again";

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// The kind the pane had when the palette opened (its wire name).
    pub opened_over: String,
    /// The `artifact_ls` request out for this palette, so its answer is told apart.
    pub req: Option<u64>,
    pub listing: Listing,
    pub query: String,
    /// Index into `rows`; 0 when there are none.
    pub selected: usize,
    /// Every row the query keeps: project first, then machine, each by name.
    pub rows: Vec<Row>,
}

impl Palette {
    pub fn new(opened_over: &str) -> Self {
        Self { opened_over: opened_over.to_owned(), req: None, listing: Listing::Loading, query: String::new(), selected: 0, rows: vec![] }
    }
    /// Every artifact the daemon listed, whatever the query.
    pub fn all(&self) -> &[ArtifactInfo] {
        match &self.listing {
            Listing::Ready(list) => list,
            _ => &[],
        }
    }
    pub fn failure(&self) -> Option<&str> {
        match &self.listing {
            Listing::Failed(message) => Some(message),
            _ => None,
        }
    }
    pub fn is_loading(&self) -> bool {
        self.listing == Listing::Loading
    }
    /// The field takes typing only when there is something to search.
    pub fn searchable(&self) -> bool {
        self.is_loading() || !self.all().is_empty()
    }
    pub fn field_placeholder(&self) -> &'static str {
        match &self.listing {
            Listing::Loading => PLACEHOLDER,
            Listing::Failed(_) => UNAVAILABLE_FIELD,
            Listing::Ready(list) => if list.is_empty() { NO_ARTIFACTS_FIELD } else { PLACEHOLDER },
        }
    }
    pub fn footer(&self) -> String {
        match &self.listing {
            Listing::Loading => "Listing artifacts".into(),
            Listing::Failed(_) => "Listing failed".into(),
            Listing::Ready(list) => {
                if self.query.is_empty() {
                    format!("{} {}", list.len(), if list.len() == 1 { "artifact" } else { "artifacts" })
                } else {
                    format!("{} of {}", self.rows.len(), list.len())
                }
            }
        }
    }
    pub fn no_match_detail(&self) -> String {
        format!("Searched the titles and names of {} artifacts.", self.all().len())
    }
    /// The footer's key hints.
    pub fn keys(&self) -> Vec<Key> {
        match &self.listing {
            Listing::Loading => vec![key("esc", "close", true)],
            Listing::Failed(_) => vec![key("↩", "try again", true), key("esc", "close", true)],
            Listing::Ready(list) => {
                // With nothing listed, Return has nothing to open.
                if list.is_empty() {
                    return vec![key("↩", "open", false), key("esc", "close", true)];
                }
                vec![key("↑↓", "move", true), key("↩", "open", !self.rows.is_empty()), key("esc", "close", true)]
            }
        }
    }
    /// The row ↩ opens, if any.
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }
    pub fn listed(&mut self, list: Vec<ArtifactInfo>) {
        self.listing = Listing::Ready(list);
        self.refilter();
    }
    pub fn failed(&mut self, message: &str) {
        self.listing = Listing::Failed(message.to_owned());
        self.rows.clear();
        self.selected = 0;
    }
    pub fn relisting(&mut self) {
        self.listing = Listing::Loading;
        self.rows.clear();
        self.selected = 0;
    }
    /// Typing resets the selection to the first row of the first non-empty group.
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
        let last = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + by).clamp(0, last) as usize;
    }
    pub fn select(&mut self, id: &str) {
        if let Some(i) = self.rows.iter().position(|r| r.id() == id) {
            self.selected = i;
        }
    }
    /// Esc: the query first, then the palette. Returns whether the palette should close.
    pub fn escape(&mut self) -> bool {
        if self.query.is_empty() {
            return true;
        }
        self.type_text("");
        false
    }
    fn refilter(&mut self) {
        self.rows = filter(self.all(), &self.query);
        self.selected = 0;
    }
}

/// Case-insensitive substring over title and name. Scattered-letter matching runs only when no
/// row has a substring hit.
pub fn filter(list: &[ArtifactInfo], query: &str) -> Vec<Row> {
    let mut ordered: Vec<&ArtifactInfo> = list.iter().collect();
    ordered.sort_by(|a, b| (a.scope != ArtifactScope::Project).cmp(&(b.scope != ArtifactScope::Project)).then_with(|| a.name.cmp(&b.name)));
    if query.is_empty() {
        return ordered.into_iter().map(|info| Row { info: info.clone(), title_match: vec![], name_match: vec![] }).collect();
    }
    for scattered in [false, true] {
        let hits: Vec<Row> = ordered.iter().filter_map(|info| {
            let title = if info.title != info.name { matches(query, &info.title, scattered) } else { None };
            let name = matches(query, &info.name, scattered);
            if title.is_none() && name.is_none() {
                return None;
            }
            Some(Row { info: (*info).clone(), title_match: title.unwrap_or_default(), name_match: name.unwrap_or_default() })
        }).collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    vec![]
}

/// The character offsets of `text` the query matched, or none.
fn matches(query: &str, text: &str, scattered: bool) -> Option<Vec<usize>> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let want: Vec<char> = query.to_lowercase().chars().collect();
    // Lowercasing can change the length (`İ`); a match then points at the wrong characters.
    if chars.len() != text.chars().count() || want.is_empty() || want.len() > chars.len() {
        return None;
    }
    if let Some(start) = (0..=chars.len() - want.len()).find(|&s| chars[s..s + want.len()] == want[..]) {
        return Some((start..start + want.len()).collect());
    }
    if !scattered {
        return None;
    }
    let mut out = vec![];
    let mut from = 0;
    for c in want {
        let i = chars[from..].iter().position(|&x| x == c)? + from;
        out.push(i);
        from = i + 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(scope: ArtifactScope, name: &str, title: Option<&str>) -> ArtifactInfo {
        serde_json::from_value(serde_json::json!({"scope": scope_name(scope), "name": name, "entry": "/w/wheel.toml", "dir": format!("/w/{name}"), "title": title.unwrap_or(name), "pane": true})).unwrap()
    }
    fn listed() -> Palette {
        let mut p = Palette::new("empty");
        p.listed(vec![info(ArtifactScope::Machine, "beads", Some("Beads board")), info(ArtifactScope::Project, "site", None), info(ArtifactScope::Project, "dash", Some("Dashboard"))]);
        p
    }

    #[test]
    fn rows_come_project_first_then_by_name() {
        let p = listed();
        assert_eq!(p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), ["project/dash", "project/site", "machine/beads"]);
        assert_eq!(p.footer(), "3 artifacts");
        assert!(p.searchable());
    }

    #[test]
    fn substring_hits_win_over_scattered_ones() {
        let mut p = listed();
        p.type_text("da");
        assert_eq!(p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), ["project/dash"]);
        assert_eq!(p.rows[0].title_match, [0, 1]);
        assert_eq!(p.rows[0].name_match, [0, 1]);
        assert_eq!(p.footer(), "1 of 3");
        // No substring holds "bd": the scattered pass keeps Dashboard (b…d in its title) and beads.
        p.type_text("bd");
        assert_eq!(p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), ["project/dash", "machine/beads"]);
        assert_eq!(p.rows[0].title_match, [4, 8]);
        assert!(p.rows[0].name_match.is_empty());
        assert_eq!(p.rows[1].title_match, [0, 3]);
        p.type_text("zz");
        assert!(p.rows.is_empty());
        assert_eq!(p.no_match_detail(), "Searched the titles and names of 3 artifacts.");
    }

    #[test]
    fn moving_stops_at_the_ends_and_typing_resets() {
        let mut p = listed();
        p.move_by(5);
        assert_eq!(p.selected_row().map(|r| r.id()).as_deref(), Some("machine/beads"));
        p.move_by(-1);
        assert_eq!(p.selected, 1);
        p.type_text("s");
        assert_eq!(p.selected, 0);
        p.move_by(-3);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn escape_clears_the_query_before_it_closes() {
        let mut p = listed();
        p.type_text("x");
        assert!(!p.escape());
        assert_eq!(p.query, "");
        assert!(p.escape());
    }

    #[test]
    fn empty_and_failed_listings_say_so() {
        let mut p = Palette::new("empty");
        assert_eq!(p.keys(), [key("esc", "close", true)]);
        p.listed(vec![]);
        assert!(!p.searchable());
        assert_eq!(p.field_placeholder(), NO_ARTIFACTS_FIELD);
        assert_eq!(p.keys()[0], key("↩", "open", false));
        p.failed("no such space");
        assert_eq!(p.failure(), Some("no such space"));
        assert_eq!(p.footer(), "Listing failed");
        assert_eq!(p.field_placeholder(), UNAVAILABLE_FIELD);
    }
}
