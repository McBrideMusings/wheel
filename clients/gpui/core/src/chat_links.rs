//! The links in an agent's transcript, the port of WheelKit's `ChatLinks.swift`: web addresses,
//! file paths, pane ids and the person's own `chat.links.patterns` found in plain text, plus the
//! `wheel://` addresses a reply can write to name a pane, a space, a file or an artifact. A link
//! is carried in a `Mark` as its address; a click reads the address back as a `Target`.
use crate::markdown::{self, Block, Mark, Run, Text};
use regex::Regex;
use std::{ops::Range, sync::OnceLock};

/// How a reply's links behave, from the `chat.links.*` settings.
#[derive(Clone, Debug)]
pub struct Settings {
    /// `chat.links.files = pane`: a path is a link.
    pub files: bool,
    /// `chat.links.file_label = full`: a file link shows its path as written, not its name.
    pub full_label: bool,
    /// `chat.links.panes`: a pane id is a link.
    pub panes: bool,
    /// `chat.links.web = system`: a plain click on a web link opens the system browser.
    pub web_system: bool,
    pub patterns: Vec<Pattern>,
}
impl Default for Settings {
    fn default() -> Self {
        Settings { files: true, full_label: false, panes: true, web_system: false, patterns: vec![] }
    }
}
impl Settings {
    /// Take one resolved setting; a key that is not `chat.links.*` is left alone.
    pub fn apply(&mut self, key: &str, value: &str) {
        match key {
            "chat.links.files" => self.files = value != "off",
            "chat.links.file_label" => self.full_label = value == "full",
            "chat.links.panes" => self.panes = value == "true",
            "chat.links.web" => self.web_system = value == "system",
            "chat.links.patterns" => self.patterns = Pattern::parse(value),
            _ => {}
        }
    }
}

/// What a match of a person's pattern does, with its template still holding `$0`, `$1`….
#[derive(Clone, Debug, PartialEq)]
pub enum PatternAction {
    Open(String),
    Run(String),
    Copy(String),
}

/// One entry of `chat.links.patterns`.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub regex: Regex,
    pub action: PatternAction,
}
impl Pattern {
    /// The entries of the setting's value (a JSON list wheeld has checked); one this build
    /// cannot read or compile is left out.
    pub fn parse(json: &str) -> Vec<Pattern> {
        let Ok(serde_json::Value::Array(list)) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
        list.iter().filter_map(|item| {
            let regex = Regex::new(item["match"].as_str()?).ok()?;
            let action = match item["action"].as_str()? {
                "open" => PatternAction::Open(item["url"].as_str()?.to_owned()),
                "run" => PatternAction::Run(item["command"].as_str()?.to_owned()),
                "copy" => PatternAction::Copy(item["text"].as_str().unwrap_or("$0").to_owned()),
                _ => return None,
            };
            Some(Pattern { regex, action })
        }).collect()
    }
    /// The match's action: `$N` filled with group N, percent-encoded into a URL and quoted for
    /// the shell in a command, so the text an agent wrote cannot add to either.
    fn resolved(&self, caps: &regex::Captures) -> Target {
        let fill = |template: &str, quote: &dyn Fn(&str) -> String| {
            let mut out = String::new();
            let mut chars = template.char_indices().peekable();
            while let Some((_, c)) = chars.next() {
                if c == '$' && chars.peek().is_some_and(|(_, d)| d.is_ascii_digit()) {
                    let mut digits = String::new();
                    while let Some((_, d)) = chars.peek().filter(|(_, d)| d.is_ascii_digit()) {
                        digits.push(*d);
                        chars.next();
                    }
                    if let Some(m) = digits.parse::<usize>().ok().and_then(|n| caps.get(n)) {
                        out.push_str(&quote(m.as_str()));
                    }
                } else {
                    out.push(c);
                }
            }
            out
        };
        match &self.action {
            PatternAction::Open(url) => Target::from_address(&fill(url, &|s| escape(s, URL_SAFE))).unwrap_or(Target::System(url.clone())),
            PatternAction::Run(command) => Target::Run(fill(command, &shell_quoted)),
            PatternAction::Copy(text) => Target::Copy(fill(text, &|s| s.to_owned())),
        }
    }
}

const URL_SAFE: &str = "-._~";
fn escape(s: &str, keep: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || keep.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
pub fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn shell_quoted(s: &str) -> String {
    // A control character typed into a terminal is a keystroke (Return, Ctrl-C), never text.
    let s: String = s.chars().filter(|c| *c >= ' ' && *c != '\x7f').collect();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-._/:@%+=,".contains(c)) {
        return s;
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Known only to this process: an address that runs a command or fills the clipboard carries it,
/// so a link a reply wrote itself can never do either.
fn key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// What a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An `http(s)` page.
    Web(String),
    /// A file as written (absolute, `~/`, or relative to the agent's directory) until a probe
    /// resolves it, then absolute.
    File { path: String, line: Option<u32> },
    Pane(String),
    /// A space by its id or its name.
    Space(String),
    Artifact(String),
    /// A command to run in a terminal pane beside the chat.
    Run(String),
    Copy(String),
    /// An address of any other scheme (`mailto:`), for the system to open.
    System(String),
}
impl Target {
    /// The address a `Mark` carries for this target.
    pub fn address(&self) -> String {
        match self {
            Target::Web(url) | Target::System(url) => url.clone(),
            Target::File { path, line } => {
                let path = if let Some(rest) = path.strip_prefix('/') { rest.to_owned() } else { path.clone() };
                let mut s = format!("wheel://file/{}", escape(&path, "-._~/"));
                if let Some(line) = line {
                    s.push_str(&format!("#L{line}"));
                }
                s
            }
            Target::Pane(id) => format!("wheel://pane/{}", escape(id, URL_SAFE)),
            Target::Space(name) => format!("wheel://space/{}", escape(name, URL_SAFE)),
            Target::Artifact(name) => format!("wheel://artifact/{}", escape(name, URL_SAFE)),
            Target::Run(command) => format!("wheel-link://run?key={}&command={}", key(), escape(command, URL_SAFE)),
            Target::Copy(text) => format!("wheel-link://copy?key={}&text={}", key(), escape(text, URL_SAFE)),
        }
    }

    /// The target a link's address names. `wheel://pane/<id>`, `wheel://space/<id or name>`,
    /// `wheel://artifact/<name>` and `wheel://file/<absolute path>[#L<line>]` (`wheel://file/~/…`
    /// for the home folder) are Wheel's; `http(s)` is a page, `file://` a file, an address with
    /// no scheme a file relative to the agent's directory, and any other scheme the system's.
    pub fn from_address(address: &str) -> Option<Target> {
        let address = address.trim();
        if address.is_empty() {
            return None;
        }
        let lower = address.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Some(Target::Web(address.to_owned()));
        }
        if let Some(rest) = address.strip_prefix("wheel://") {
            let (kind, value) = rest.split_once('/').unwrap_or((rest, ""));
            let value = value.trim_end_matches('/');
            if value.is_empty() {
                return None;
            }
            return match kind {
                "pane" => Some(Target::Pane(unescape(value))),
                "space" => Some(Target::Space(unescape(value))),
                "artifact" => Some(Target::Artifact(unescape(value))),
                "file" => {
                    let (path, line) = split_line(value);
                    let path = unescape(path);
                    let path = if path.starts_with("~/") || path == "~" { path } else { format!("/{}", path.trim_start_matches('/')) };
                    Some(Target::File { path, line })
                }
                _ => None,
            };
        }
        if let Some(rest) = address.strip_prefix("wheel-link://") {
            let (kind, query) = rest.split_once('?')?;
            let value = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('='))).map(unescape);
            if value("key").as_deref() != Some(key()) {
                return None;
            }
            return match kind {
                "run" => value("command").map(Target::Run),
                "copy" => value("text").map(Target::Copy),
                _ => None,
            };
        }
        if let Some(rest) = lower.strip_prefix("file://") {
            let path = &address[address.len() - rest.len()..];
            let (path, line) = split_line(path);
            return Some(Target::File { path: unescape(path), line });
        }
        // A scheme is letters, digits, `+`, `-` or `.` before a colon, starting with a letter.
        if let Some((scheme, _)) = address.split_once(':')
            && scheme.len() > 1
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            return Some(Target::System(address.to_owned()));
        }
        if address.starts_with('#') {
            return None;
        }
        let (path, line) = split_line(address);
        let (path, line) = match line {
            Some(_) => (path, line),
            None => match path.rsplit_once(':') {
                Some((p, n)) if n.parse::<u32>().is_ok() => (p, n.parse().ok()),
                _ => (path, None),
            },
        };
        Some(Target::File { path: unescape(path), line })
    }
}

/// `path#L12` as (`path`, 12).
fn split_line(s: &str) -> (&str, Option<u32>) {
    match s.rsplit_once("#L") {
        Some((path, n)) => (path, n.split('-').next().and_then(|n| n.parse().ok())),
        None => (s.split('#').next().unwrap_or(s), None),
    }
}

/// `path` with a leading `~` made the home folder.
pub fn expand_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if let Some(rest) = path.strip_prefix("~/") { format!("{home}/{rest}") } else if path == "~" { home } else { path.to_owned() }
}

/// Whether the system's default app for `path` runs it rather than showing it: an app bundle,
/// an installer, or a script macOS opens in Terminal.
pub fn runs_when_opened(path: &str) -> bool {
    let ext = path.trim_end_matches('/').rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "app" | "command" | "tool" | "terminal" | "workflow" | "pkg" | "mpkg" | "scpt" | "applescript" | "prefpane" | "webloc" | "inetloc" | "fileloc")
}

/// The browser address of a file (`~/` expanded), ending `#L<line>` when the link names one.
pub fn file_url(path: &str, line: Option<u32>) -> String {
    let full = expand_home(path);
    let mut url = format!("file://{}", escape(&full, "-._~/"));
    if let Some(line) = line {
        url.push_str(&format!("#L{line}"));
    }
    url
}

/// One target found in a text, by byte range.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub range: Range<usize>,
    pub target: Target,
}

fn url_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?:https?|wheel)://[^\s<>()\[\]`"']+"#).unwrap())
}
fn pane_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"p\d+").unwrap())
}
fn file_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?:(?:~|\.{1,2})?/)?(?:[\w@+.-]+/)*[\w@+.-]+(?::(\d+)(?::\d+)?)?").unwrap())
}
fn extension_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[\w@+-]{2,}\.[A-Za-z][A-Za-z0-9]{0,9}$").unwrap())
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}
fn after(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

/// Every target in `text`, sorted, none overlapping another: a web or `wheel://` address wins
/// over a path inside it, then the person's own patterns in their order, a pane id, a file.
pub fn scan(text: &str, settings: &Settings) -> Vec<Found> {
    let mut taken: Vec<Found> = vec![];
    let free = |taken: &[Found], r: &Range<usize>| !taken.iter().any(|f| f.range.start < r.end && r.start < f.range.end);
    for m in url_pattern().find_iter(text) {
        let mut end = m.end();
        while end > m.start() + 1 && text[..end].ends_with(['.', ',', ';', ':', '!', '?']) {
            end -= 1;
        }
        let r = m.start()..end;
        if let Some(target) = Target::from_address(&text[r.clone()])
            && free(&taken, &r)
        {
            taken.push(Found { range: r, target });
        }
    }
    for p in &settings.patterns {
        for caps in p.regex.captures_iter(text) {
            let m = caps.get(0).unwrap();
            let r = m.range();
            if !r.is_empty() && free(&taken, &r) {
                taken.push(Found { range: r, target: p.resolved(&caps) });
            }
        }
    }
    if settings.panes {
        for m in pane_pattern().find_iter(text) {
            let lead = before(text, m.start()).is_some_and(|c| word(c) || "/.-".contains(c));
            let next = after(text, m.end());
            let trail = next.is_some_and(|c| word(c) || "/@-".contains(c))
                || (next == Some('.') && after(text, m.end() + 1).is_some_and(word));
            let r = m.range();
            if !lead && !trail && free(&taken, &r) {
                taken.push(Found { range: r, target: Target::Pane(m.as_str().to_owned()) });
            }
        }
    }
    if settings.files {
        let mut at = 0;
        while at < text.len() {
            let Some(caps) = file_pattern().captures_at(text, at) else { break };
            let m = caps.get(0).unwrap();
            let lead = before(text, m.start()).is_some_and(|c| word(c) || "/.~@+:-".contains(c));
            let trail = after(text, m.end()).is_some_and(|c| word(c) || "@+-".contains(c));
            if lead || trail {
                // Try again one character on, as a look-behind would.
                at = m.start() + after(text, m.start()).map_or(1, char::len_utf8);
                continue;
            }
            at = m.end().max(m.start() + 1);
            let mut line = caps.get(1).and_then(|g| g.as_str().parse::<u32>().ok());
            let all = m.as_str();
            let body = all.split(':').next().unwrap_or(all);
            let trimmed = body.trim_end_matches(['.', '-']);
            if trimmed.is_empty() || !looks_like_file(trimmed) {
                continue;
            }
            let end = if trimmed.len() != body.len() {
                // A sentence's full stop ended the match: no line number can follow it.
                line = None;
                m.start() + trimmed.len()
            } else if line.is_none() {
                m.start() + body.len()
            } else {
                m.end()
            };
            let r = m.start()..end;
            if free(&taken, &r) {
                taken.push(Found { range: r, target: Target::File { path: trimmed.to_owned(), line } });
            }
        }
    }
    taken.sort_by_key(|f| f.range.start);
    taken
}

/// A path has a folder in it, or a name with an extension; a lone word or number is not one.
fn looks_like_file(s: &str) -> bool {
    if s.contains('/') {
        return s.len() > 1 && !s.ends_with('/');
    }
    extension_pattern().is_match(s)
}

/// What the client knows of a file a reply names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Not asked yet, or the answer has not come.
    Unknown,
    Missing,
    /// The absolute path wheeld resolved it to.
    Found(String),
}

/// What a client knows about the things a reply names.
pub struct Knowledge<'a> {
    pub file: &'a dyn Fn(&str) -> Probe,
    /// The chip text for a pane id that exists (`p1958 · Claude`); none for no such pane.
    pub pane: &'a dyn Fn(&str) -> Option<String>,
}

/// The most distinct files one text asks wheeld about.
pub const MAX_FILE_PROBES: usize = 24;

/// `text` with every target that exists made a link: a file shows its name (or its path, with
/// `full_label`) and a pane its chip. A target inside an existing link, or inside inline code that
/// is not wholly the target, is left as it is. Marks over a replaced stretch move with it.
pub fn link_text(text: &Text, settings: &Settings, knows: &Knowledge, files: &mut Vec<String>) -> Text {
    let checked = check_links(text, settings, knows, files);
    let text = &checked;
    let mut edits: Vec<(Range<usize>, String, String)> = vec![];
    for found in scan(&text.text, settings) {
        let r = &found.range;
        let inside = |run: &Run| run.range.start < r.end && r.start < run.range.end;
        if text.runs.iter().any(|run| inside(run) && run.mark.link.is_some()) {
            continue;
        }
        if text.runs.iter().any(|run| inside(run) && run.mark.code && run.range != *r) {
            continue;
        }
        let original = &text.text[r.clone()];
        let linked = match &found.target {
            Target::File { path, line } => {
                if !files.contains(path) {
                    if files.len() >= MAX_FILE_PROBES {
                        continue;
                    }
                    files.push(path.clone());
                }
                let Probe::Found(resolved) = (knows.file)(path) else { continue };
                let label = if settings.full_label {
                    original.to_owned()
                } else {
                    let name = path.rsplit('/').next().unwrap_or(path);
                    line.map_or(name.to_owned(), |l| format!("{name}:{l}"))
                };
                (label, Target::File { path: resolved, line: *line }.address())
            }
            Target::Pane(id) => {
                let Some(chip) = (knows.pane)(id) else { continue };
                (chip, found.target.address())
            }
            other => (original.to_owned(), other.address()),
        };
        edits.push((r.clone(), linked.0, linked.1));
    }
    if edits.is_empty() {
        return text.clone();
    }
    // Map an old byte offset to the new text: before an edit it stays, inside one it goes to the
    // edit's start or end, after it moves by the edits' change in length.
    let map = |at: usize, end_side: bool| -> usize {
        let mut shift: isize = 0;
        for (r, label, _) in &edits {
            if at <= r.start && !(end_side && at == r.end && r.is_empty()) {
                break;
            }
            if at < r.end || (at == r.end && !end_side && at == r.start) {
                let start = (r.start as isize + shift) as usize;
                return if end_side { start + label.len() } else { start };
            }
            shift += label.len() as isize - r.len() as isize;
        }
        (at as isize + shift) as usize
    };
    let mut out = String::with_capacity(text.text.len());
    let mut cursor = 0;
    let mut runs: Vec<Run> = text.runs.iter().map(|run| Run { range: map(run.range.start, false)..map(run.range.end, true), mark: run.mark.clone() }).collect();
    for (r, label, address) in &edits {
        out.push_str(&text.text[cursor..r.start]);
        let start = out.len();
        out.push_str(label);
        runs.push(Run { range: start..out.len(), mark: Mark { link: Some(address.clone()), ..Mark::default() } });
        cursor = r.end;
    }
    out.push_str(&text.text[cursor..]);
    runs.retain(|r| r.range.start < r.range.end);
    Text { text: out, runs: markdown::flatten(&runs) }
}

/// The schemes a link a reply wrote may hand to the system: one whose label can hide what it
/// opens (`x-apple.systempreferences:`, `smb:`, `ssh:`) is drawn as text instead.
const SYSTEM_SCHEMES: [&str; 1] = ["mailto"];

/// `text`'s own links (a Markdown link's address) as the client will open them: a file link only
/// when the file exists, with its address made absolute, and a system address only for a scheme
/// in `SYSTEM_SCHEMES`; any other is drawn as plain text.
fn check_links(text: &Text, settings: &Settings, knows: &Knowledge, files: &mut Vec<String>) -> Text {
    if !text.runs.iter().any(|r| r.mark.link.is_some()) {
        return text.clone();
    }
    let mut out = text.clone();
    for run in &mut out.runs {
        let Some(address) = run.mark.link.clone() else { continue };
        run.mark.link = match Target::from_address(&address) {
            Some(Target::File { path, line }) if settings.files => {
                if !files.contains(&path) && files.len() < MAX_FILE_PROBES {
                    files.push(path.clone());
                }
                match files.contains(&path).then(|| (knows.file)(&path)) {
                    Some(Probe::Found(resolved)) => Some(Target::File { path: resolved, line }.address()),
                    _ => None,
                }
            }
            Some(Target::File { .. }) => None,
            Some(Target::System(url)) => {
                let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                SYSTEM_SCHEMES.contains(&scheme.as_str()).then_some(url)
            }
            Some(_) => Some(address),
            None => None,
        };
    }
    out.runs = markdown::flatten(&out.runs);
    out
}

/// Every text of a reply's blocks linked by `link_text`; code blocks are left alone.
pub fn link_blocks(blocks: &[Block], settings: &Settings, knows: &Knowledge, files: &mut Vec<String>) -> Vec<Block> {
    let mut link = |t: &Text| link_text(t, settings, knows, files);
    fn walk(b: &Block, link: &mut dyn FnMut(&Text) -> Text) -> Block {
        match b {
            Block::Paragraph(t) => Block::Paragraph(link(t)),
            Block::Heading { level, text } => Block::Heading { level: *level, text: link(text) },
            Block::Quote(t) => Block::Quote(link(t)),
            Block::List(items) => Block::List(items.iter().map(|i| markdown::Item {
                depth: i.depth,
                marker: i.marker.clone(),
                text: link(&i.text),
                block: i.block.as_ref().map(|b| Box::new(walk(b, link))),
            }).collect()),
            Block::Table { header, rows } => Block::Table { header: header.iter().map(&mut *link).collect(), rows: rows.iter().map(|r| r.iter().map(&mut *link).collect()).collect() },
            Block::Code { .. } | Block::Rule | Block::Image { .. } => b.clone(),
        }
    }
    blocks.iter().map(|b| walk(b, &mut link)).collect()
}

/// The link runs of a text: (range, address).
pub fn links_of(runs: &[Run]) -> Vec<(Range<usize>, String)> {
    runs.iter().filter_map(|r| r.mark.link.clone().map(|l| (r.range.clone(), l))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<(String, Target)> {
        scan(text, &Settings::default()).into_iter().map(|f| (text[f.range].to_owned(), f.target)).collect()
    }

    #[test]
    fn finds_urls_paths_panes_and_wheel_addresses() {
        let found = targets("See https://example.com/a?b=1. Edit /Users/me/x.rs:12 and src/lib.rs, ask p10041, open wheel://pane/p7.");
        assert_eq!(found, vec![
            ("https://example.com/a?b=1".into(), Target::Web("https://example.com/a?b=1".into())),
            ("/Users/me/x.rs:12".into(), Target::File { path: "/Users/me/x.rs".into(), line: Some(12) }),
            ("src/lib.rs".into(), Target::File { path: "src/lib.rs".into(), line: None }),
            ("p10041".into(), Target::Pane("p10041".into())),
            ("wheel://pane/p7".into(), Target::Pane("p7".into())),
        ]);
    }

    #[test]
    fn leaves_words_numbers_and_mid_word_ids_alone() {
        assert!(targets("a plain sentence with 3 numbers, v1.2 and top10 and app5x.").is_empty());
        assert!(targets("the file/ folder and ap10041").is_empty());
        assert_eq!(targets("~/notes.md."), vec![("~/notes.md".into(), Target::File { path: "~/notes.md".into(), line: None })]);
    }

    #[test]
    fn addresses_round_trip() {
        for t in [
            Target::Pane("p12".into()),
            Target::Space("wheel dev".into()),
            Target::Artifact("agent-mode".into()),
            Target::File { path: "/tmp/a b.rs".into(), line: Some(4) },
            Target::File { path: "~/x.md".into(), line: None },
            Target::Run("echo 'hi'".into()),
            Target::Copy("abc".into()),
            Target::Web("https://x.y/z".into()),
        ] {
            assert_eq!(Target::from_address(&t.address()), Some(t.clone()), "{}", t.address());
        }
        assert_eq!(Target::from_address("wheel-link://run?key=forged&command=rm"), None);
        assert_eq!(Target::from_address("mailto:a@b.c"), Some(Target::System("mailto:a@b.c".into())));
        assert_eq!(Target::from_address("src/a.rs#L9"), Some(Target::File { path: "src/a.rs".into(), line: Some(9) }));
        assert_eq!(Target::from_address("file:///tmp/a.txt"), Some(Target::File { path: "/tmp/a.txt".into(), line: None }));
        assert!(runs_when_opened("/Applications/Foo.app") && runs_when_opened("/tmp/x.COMMAND") && !runs_when_opened("/tmp/x.rs"));
    }

    #[test]
    fn link_text_labels_what_exists_and_moves_marks() {
        let text = Text { text: "Read **src/a.rs:3** then p9 then gone.rs".into(), runs: vec![] };
        let parsed = markdown::parse(&text.text);
        let Block::Paragraph(t) = &parsed[0] else { panic!() };
        let knows = Knowledge {
            file: &|p| if p == "src/a.rs" { Probe::Found("/w/src/a.rs".into()) } else { Probe::Missing },
            pane: &|id| (id == "p9").then(|| "p9 · Claude".into()),
        };
        let mut files = vec![];
        let out = link_text(t, &Settings::default(), &knows, &mut files);
        assert_eq!(out.text, "Read a.rs:3 then p9 · Claude then gone.rs");
        assert_eq!(files, vec!["src/a.rs".to_owned(), "gone.rs".to_owned()]);
        let links = links_of(&out.runs);
        assert_eq!(links.len(), 2);
        assert_eq!(&out.text[links[0].0.clone()], "a.rs:3");
        assert_eq!(Target::from_address(&links[0].1), Some(Target::File { path: "/w/src/a.rs".into(), line: Some(3) }));
        assert!(out.runs.iter().any(|r| r.mark.bold && &out.text[r.range.clone()] == "a.rs:3"), "{:?}", out.runs);
        assert_eq!(Target::from_address(&links[1].1), Some(Target::Pane("p9".into())));
    }

    #[test]
    fn markdown_links_and_partial_code_stay_as_written() {
        let parsed = markdown::parse("[docs](https://a.b) and `run src/a.rs` and `src/b.rs`");
        let Block::Paragraph(t) = &parsed[0] else { panic!() };
        let knows = Knowledge { file: &|p| Probe::Found(format!("/w/{p}")), pane: &|_| None };
        let out = link_text(t, &Settings::default(), &knows, &mut vec![]);
        assert_eq!(out.text, "docs and run src/a.rs and b.rs");
        let links = links_of(&out.runs);
        assert_eq!(links.iter().map(|(r, _)| &out.text[r.clone()]).collect::<Vec<_>>(), vec!["docs", "b.rs"]);
        assert!(out.runs.iter().any(|r| r.mark.code && r.mark.link.is_some()));
    }

    #[test]
    fn written_links_open_only_what_is_there_and_safe() {
        let parsed = markdown::parse("[a](wheel://file/w/real.rs#L2) [b](gone.rs) [c](x-apple.systempreferences:com.apple.x) [d](mailto:a@b.c) [e](wheel://pane/p3)");
        let Block::Paragraph(t) = &parsed[0] else { panic!() };
        let knows = Knowledge { file: &|p| if p == "/w/real.rs" { Probe::Found("/w/real.rs".into()) } else { Probe::Missing }, pane: &|_| None };
        let out = link_text(t, &Settings::default(), &knows, &mut vec![]);
        let links: Vec<(String, String)> = links_of(&out.runs).into_iter().map(|(r, a)| (out.text[r].to_owned(), a)).collect();
        assert_eq!(links, vec![
            ("a".to_owned(), "wheel://file/w/real.rs#L2".to_owned()),
            ("d".to_owned(), "mailto:a@b.c".to_owned()),
            ("e".to_owned(), "wheel://pane/p3".to_owned()),
        ]);
    }

    #[test]
    fn patterns_fill_groups_quoted() {
        let mut s = Settings::default();
        s.apply("chat.links.patterns", r##"[{"match":"#(\\d+)","action":"open","url":"https://x.y/issues/$1"},{"match":"bead-(\\w+)","action":"run","command":"bd show $0"}]"##);
        let found = scan("see #42 and bead-x1", &s);
        assert_eq!(found[0].target, Target::Web("https://x.y/issues/42".into()));
        assert_eq!(found[1].target, Target::Run("bd show bead-x1".into()));
    }
}
