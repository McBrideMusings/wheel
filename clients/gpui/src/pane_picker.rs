//! The pane picker an empty pane shows: the quick buttons, the search field and one list of every
//! plugin pane and artifact of the space. Pure state and the ranker, so the view draws it and a
//! check reads it back. The ranker's cases live in `clients/shared/pane-picker-ranker.json`, the
//! fixture the Swift ranker reads too.
use serde::Deserialize;

/// What a picked item turns the pane into. The order is the ranker's tie-break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Terminal,
    Browser,
    Agent,
    Plugin,
    Artifact,
}
impl Kind {
    /// The word a list row shows at its right.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Terminal => "terminal",
            Kind::Browser => "browser",
            Kind::Agent => "agent",
            Kind::Plugin => "plugin",
            Kind::Artifact => "artifact",
        }
    }
    /// The SF Symbol drawn beside the item.
    pub fn symbol(self) -> &'static str {
        match self {
            Kind::Terminal => "terminal",
            Kind::Browser => "globe",
            Kind::Agent => "sparkle",
            Kind::Plugin => "puzzlepiece.extension",
            Kind::Artifact => "doc",
        }
    }
}

/// One thing the picker can open: `terminal`, `browser`, `agent:<name>`, `plugin:<plugin>/<pane>`
/// or `artifact:<scope>/<name>`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    /// An artifact's title; the name for everything else.
    pub title: String,
}
impl Item {
    pub fn new(id: impl Into<String>, kind: Kind, name: impl Into<String>, title: impl Into<String>) -> Self {
        Self { id: id.into(), kind, name: name.into(), title: title.into() }
    }
    /// The title is a line of its own only when it says something the name does not.
    pub fn has_title(&self) -> bool {
        self.title != self.name
    }
}

/// A ranked item: its score (lower first) and the character offsets of its name and title the
/// query matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub item: Item,
    pub score: f32,
    pub name_match: Vec<usize>,
    pub title_match: Vec<usize>,
}

/// Every item the query keeps, best first: a name prefix (0), contiguous in the name (1),
/// contiguous in the title (2), scattered in the name (3 + span/100), scattered in the title
/// (4 + span/100); ties by kind, then name. Case-insensitive; an empty query keeps nothing.
pub fn rank(items: &[Item], query: &str) -> Vec<Hit> {
    let want: Vec<char> = query.to_lowercase().chars().collect();
    if want.is_empty() {
        return vec![];
    }
    let mut hits: Vec<Hit> = items.iter().filter_map(|item| score(item, &want)).collect();
    hits.sort_by(|a, b| a.score.total_cmp(&b.score).then(a.item.kind.cmp(&b.item.kind)).then_with(|| a.item.name.to_lowercase().cmp(&b.item.name.to_lowercase())).then_with(|| a.item.name.cmp(&b.item.name)));
    hits
}

fn score(item: &Item, want: &[char]) -> Option<Hit> {
    let name = lowered(&item.name)?;
    let title = if item.has_title() { lowered(&item.title) } else { None };
    let hit = |score: f32, name_match: Vec<usize>, title_match: Vec<usize>| Some(Hit { item: item.clone(), score, name_match, title_match });
    if name.starts_with(want) {
        return hit(0., (0..want.len()).collect(), vec![]);
    }
    if let Some(at) = find(&name, want) {
        return hit(1., (at..at + want.len()).collect(), vec![]);
    }
    if let Some(at) = title.as_deref().and_then(|t| find(t, want)) {
        return hit(2., vec![], (at..at + want.len()).collect());
    }
    if let Some(offsets) = scattered(&name, want) {
        return hit(3. + span(&offsets), offsets, vec![]);
    }
    if let Some(offsets) = title.as_deref().and_then(|t| scattered(t, want)) {
        return hit(4. + span(&offsets), vec![], offsets);
    }
    None
}

/// The text's characters lowercased, or none when lowercasing changes its length (`İ`): offsets
/// into it would then point at the wrong characters.
fn lowered(text: &str) -> Option<Vec<char>> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    (chars.len() == text.chars().count()).then_some(chars)
}

fn find(text: &[char], want: &[char]) -> Option<usize> {
    (want.len() <= text.len()).then(|| (0..=text.len() - want.len()).find(|&s| text[s..s + want.len()] == *want)).flatten()
}

fn span(offsets: &[usize]) -> f32 {
    (offsets[offsets.len() - 1] - offsets[0]) as f32 / 100.
}

/// The query's letters in order, over the tightest stretch of the text that holds them.
fn scattered(text: &[char], want: &[char]) -> Option<Vec<usize>> {
    let mut best: Option<Vec<usize>> = None;
    for start in (0..text.len()).filter(|&i| text[i] == want[0]) {
        let mut out = vec![start];
        let mut from = start + 1;
        for &c in &want[1..] {
            let Some(i) = text[from..].iter().position(|&x| x == c) else { break };
            out.push(from + i);
            from += i + 1;
        }
        if out.len() == want.len() && best.as_ref().is_none_or(|b| span(&out) < span(b)) {
            best = Some(out);
        }
    }
    best
}

/// The search state of one empty pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub query: String,
    /// Index into the ranked rows; 0 when there are none.
    pub selected: usize,
    /// Whether typing goes into the field. An empty pane starts with it; Esc on an empty query
    /// gives it back to the pane.
    pub field_focused: bool,
}
/// A pane that just became empty: no query, the field holding the keys.
impl Default for Picker {
    fn default() -> Self {
        Self { query: String::new(), selected: 0, field_focused: true }
    }
}
impl Picker {
    pub fn type_text(&mut self, text: &str) {
        if text != self.query {
            self.query = text.to_owned();
            self.selected = 0;
        }
    }
    /// ↑ and ↓ stop at the first and last of `count` rows.
    pub fn move_by(&mut self, by: isize, count: usize) {
        if count > 0 {
            self.selected = (self.selected as isize + by).clamp(0, count as isize - 1) as usize;
        }
    }
    /// Esc: the query first, then the field's focus.
    pub fn escape(&mut self) {
        if self.query.is_empty() {
            self.field_focused = false;
        } else {
            self.type_text("");
        }
    }
}

pub fn scope_name(scope: wheel_proto::ArtifactScope) -> &'static str {
    match scope {
        wheel_proto::ArtifactScope::Project => "project",
        wheel_proto::ArtifactScope::Machine => "machine",
    }
}

pub const COUNT_AT_REST: &str = "↩ Terminal";
pub const HINTS: [(&str, &str); 3] = [("↑↓", "move"), ("↩", "open"), ("esc", "leave empty")];

pub fn placeholder(total: usize) -> String {
    format!("Search {total} — agents, plugins, artifacts")
}
/// The text at the field's right: what Return opens at rest, the count while typing.
pub fn count(query: &str, shown: usize, total: usize) -> String {
    if query.is_empty() { COUNT_AT_REST.to_owned() } else { format!("{shown} of {total}") }
}
pub fn no_match(query: &str) -> String {
    format!("No match for “{query}”")
}

/// Byte ranges of `text` covering the characters at `offsets`, merged where they touch.
pub fn byte_ranges(text: &str, offsets: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = vec![];
    for (i, (at, ch)) in text.char_indices().enumerate() {
        if offsets.contains(&i) {
            match out.last_mut() {
                Some(last) if last.end == at => last.end = at + ch.len_utf8(),
                _ => out.push(at..at + ch.len_utf8()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        items: Vec<Item>,
        cases: Vec<Case>,
    }
    #[derive(Deserialize)]
    struct Case {
        query: String,
        expect: Vec<String>,
    }

    #[test]
    fn the_shared_fixture_ranks_as_written() {
        let fixture: Fixture = serde_json::from_str(include_str!("../../shared/pane-picker-ranker.json")).unwrap();
        for case in fixture.cases {
            let got: Vec<String> = rank(&fixture.items, &case.query).into_iter().map(|h| h.item.id).collect();
            assert_eq!(got, case.expect, "query {:?}", case.query);
        }
    }

    #[test]
    fn hits_carry_the_offsets_they_matched() {
        let items = [Item::new("artifact:project/dash", Kind::Artifact, "dash", "Dashboard"), Item::new("plugin:wheelhouse/beads", Kind::Plugin, "Beads", "Beads")];
        let hits = rank(&items, "DA");
        assert_eq!((hits[0].name_match.clone(), hits[0].score), (vec![0, 1], 0.));
        let hits = rank(&items, "bd");
        assert_eq!(hits.iter().map(|h| (h.item.kind, h.name_match.clone(), h.title_match.clone())).collect::<Vec<_>>(), [(Kind::Plugin, vec![0, 3], vec![]), (Kind::Artifact, vec![], vec![4, 8])]);
        assert!(rank(&items, "").is_empty());
    }

    #[test]
    fn scattered_takes_the_tightest_stretch() {
        let text: Vec<char> = "a-b-xab".chars().collect();
        assert_eq!(scattered(&text, &['a', 'b']), Some(vec![5, 6]));
    }

    #[test]
    fn escape_clears_then_leaves_the_field() {
        let mut p = Picker::default();
        p.type_text("x");
        p.escape();
        assert_eq!((p.query.as_str(), p.field_focused), ("", true));
        p.escape();
        assert!(!p.field_focused);
    }

    #[test]
    fn counts_and_ranges() {
        assert_eq!(count("", 0, 161), "↩ Terminal");
        assert_eq!(count("bead", 10, 161), "10 of 161");
        assert_eq!(byte_ranges("é-beads", &[2, 3, 4, 5]), [3..7]);
        assert_eq!(byte_ranges("beads", &[0, 3]), [0..1, 3..4]);
    }
}
