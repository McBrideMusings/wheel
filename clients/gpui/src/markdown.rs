//! A reply's Markdown as the rows the transcript draws, the port of WheelUI's `ChatRich.swift`
//! and `CodeBlocks.swift`: headings, emphasis, lists, links, inline code, fenced code blocks with
//! a language and syntax colours, quotes and tables. `pulldown-cmark` parses; `syntect` colours a
//! fenced block whose language it knows. The view maps `Mark`s to GPUI highlight runs.
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::{ops::Range, sync::OnceLock};
use syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings};

/// One styled stretch of a text block.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Mark {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    /// A syntax colour as 0xRRGGBB.
    pub color: Option<u32>,
    /// Inside the pointer selection; the view adds it, never the parser.
    pub selected: bool,
}

/// A run of text with the marks that apply to it, all runs sorted and never overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub range: Range<usize>,
    pub mark: Mark,
}

/// A stretch of inline text with its marks.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Text {
    pub text: String,
    pub runs: Vec<Run>,
}

/// One line of a list: an item's own text under its marker, or a block that sits inside an item
/// (a code block, a quote, a table, a trailing paragraph) with no marker of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub depth: usize,
    /// `•`, or `3.` for a numbered item; empty for a line that continues the item above.
    pub marker: String,
    pub text: Text,
    pub block: Option<Box<Block>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Text),
    Heading { level: u8, text: Text },
    List(Vec<Item>),
    /// A fenced or indented code block, `code` without the fence's final newline.
    Code { language: Option<String>, code: String },
    Quote(Text),
    Table { header: Vec<Text>, rows: Vec<Vec<Text>> },
    Rule,
}

/// A code block's syntax colours: one run per coloured stretch, by byte range into the code.
/// None when `syntect` knows no syntax for the language.
pub fn highlight(language: Option<&str>, code: &str) -> Option<Vec<Run>> {
    static SETS: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    let (syntaxes, themes) = SETS.get_or_init(|| (SyntaxSet::load_defaults_newlines(), ThemeSet::load_defaults()));
    let token = language?.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#').next().filter(|t| !t.is_empty())?;
    let syntax = syntaxes.find_syntax_by_token(token)?;
    let theme = &themes.themes["base16-ocean.dark"];
    let mut lines = HighlightLines::new(syntax, theme);
    let mut runs = vec![];
    let mut at = 0;
    for line in LinesWithEndings::from(code) {
        for (style, piece) in lines.highlight_line(line, syntaxes).ok()? {
            let range = at..at + piece.len();
            at = range.end;
            let c = style.foreground;
            let color = (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b);
            let trimmed = piece.trim_end_matches('\n');
            if trimmed.is_empty() {
                continue;
            }
            runs.push(Run { range: range.start..range.start + trimmed.len(), mark: Mark { color: Some(color), ..Mark::default() } });
        }
    }
    Some(runs)
}

/// A builder for one `Text`: marks open as a stack and close into runs.
#[derive(Default)]
struct Inline {
    text: Text,
    open: Vec<(usize, Mark)>,
}
impl Inline {
    fn push_str(&mut self, s: &str) {
        self.text.text.push_str(s);
    }
    fn begin(&mut self, mark: Mark) {
        self.open.push((self.text.text.len(), mark));
    }
    fn end(&mut self) {
        if let Some((start, mark)) = self.open.pop() {
            let end = self.text.text.len();
            if end > start {
                self.text.runs.push(Run { range: start..end, mark });
            }
        }
    }
    fn code(&mut self, s: &str) {
        let start = self.text.text.len();
        self.text.text.push_str(s);
        self.text.runs.push(Run { range: start..self.text.text.len(), mark: Mark { code: true, ..Mark::default() } });
    }
    fn separate(&mut self) {
        if !self.text.text.is_empty() && !self.text.text.ends_with('\n') {
            self.text.text.push('\n');
        }
    }
    fn finish(mut self) -> Text {
        while !self.open.is_empty() {
            self.end();
        }
        self.text.runs = flatten(&self.text.runs);
        self.text
    }
}

/// Nested runs as sorted, non-overlapping runs: inner marks add to the outer's.
pub fn flatten(runs: &[Run]) -> Vec<Run> {
    let mut edges: Vec<usize> = runs.iter().flat_map(|r| [r.range.start, r.range.end]).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut out: Vec<Run> = vec![];
    for pair in edges.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut mark = Mark::default();
        let mut any = false;
        for r in runs.iter().filter(|r| r.range.start <= a && r.range.end >= b) {
            any = true;
            mark.bold |= r.mark.bold;
            mark.italic |= r.mark.italic;
            mark.code |= r.mark.code;
            mark.strike |= r.mark.strike;
            mark.selected |= r.mark.selected;
            if r.mark.link.is_some() {
                mark.link = r.mark.link.clone();
            }
            if r.mark.color.is_some() {
                mark.color = r.mark.color;
            }
        }
        if !any {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.range.end == a && last.mark == mark => last.range.end = b,
            _ => out.push(Run { range: a..b, mark }),
        }
    }
    out
}

/// The marker for the next line of the open item: the item's own the first time, then empty.
fn take_marker(lists: &mut [(bool, u64)], marked: &mut [bool]) -> String {
    let Some((ordered, number)) = lists.last_mut() else { return String::new() };
    match marked.last_mut() {
        Some(used) if !*used => {
            *used = true;
            let marker = if *ordered { format!("{number}.") } else { "•".to_owned() };
            *number += 1;
            marker
        }
        _ => String::new(),
    }
}

/// Push the open item's text as a line, if it has any or the item has no line yet.
fn flush_item(inline: &mut Option<Inline>, lists: &mut [(bool, u64)], marked: &mut [bool], items: &mut Vec<Item>) {
    let Some(i) = inline.take() else { return };
    let text = i.finish();
    if text.text.is_empty() && marked.last() == Some(&true) {
        return;
    }
    let marker = take_marker(lists, marked);
    items.push(Item { depth: lists.len().saturating_sub(1), marker, text, block: None });
}

/// A finished block goes into the open list item as a line, else onto the page.
fn emit(block: Block, blocks: &mut Vec<Block>, inline: &mut Option<Inline>, lists: &mut [(bool, u64)], marked: &mut [bool], items: &mut Vec<Item>) {
    if lists.is_empty() {
        blocks.push(block);
    } else {
        flush_item(inline, lists, marked, items);
        let marker = take_marker(lists, marked);
        items.push(Item { depth: lists.len() - 1, marker, text: Text::default(), block: Some(Box::new(block)) });
    }
}

/// `markdown` as blocks.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks = vec![];
    // The container being filled: a list item, a table cell, a quote or a heading takes the inline
    // text that would otherwise make paragraphs.
    let mut inline: Option<Inline> = None;
    let mut heading: Option<u8> = None;
    let mut quote = 0usize;
    let mut quote_text: Option<Inline> = None;
    let mut lists: Vec<(bool, u64)> = vec![];
    let mut items: Vec<Item> = vec![];
    // One flag per open item: whether its marker has been used by a line already.
    let mut marked: Vec<bool> = vec![];
    let mut code: Option<(Option<String>, String)> = None;
    let mut table: Option<TableBuild> = None;
    let parser = Parser::new_ext(markdown, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH);
    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if quote > 0 && lists.is_empty() && table.is_none() {
                        let q = quote_text.get_or_insert_with(Inline::default);
                        q.separate();
                        inline = None;
                    } else if let Some(i) = &mut inline {
                        i.separate();
                    } else {
                        inline = Some(Inline::default());
                    }
                }
                Tag::Heading { level, .. } => {
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    heading = Some(match level { HeadingLevel::H1 => 1, HeadingLevel::H2 => 2, HeadingLevel::H3 => 3, HeadingLevel::H4 => 4, HeadingLevel::H5 => 5, HeadingLevel::H6 => 6 });
                    inline = Some(Inline::default());
                }
                Tag::BlockQuote(_) => {
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    quote += 1;
                    if quote == 1 {
                        quote_text = Some(Inline::default());
                    }
                }
                Tag::CodeBlock(kind) => {
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    let language = match kind {
                        CodeBlockKind::Fenced(lang) => Some(lang.trim().to_owned()).filter(|l| !l.is_empty()),
                        CodeBlockKind::Indented => None,
                    };
                    code = Some((language, String::new()));
                }
                Tag::List(first) => {
                    // A list opening inside an item ends that item's own text: its nested items follow it.
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    lists.push((first.is_some(), first.unwrap_or(1)));
                }
                Tag::Item => {
                    inline = Some(Inline::default());
                    marked.push(false);
                }
                Tag::Table(_) => {
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    table = Some(TableBuild::default());
                }
                Tag::TableRow => {
                    if let Some(t) = &mut table {
                        t.cells.clear();
                    }
                }
                Tag::TableCell => inline = Some(Inline::default()),
                Tag::Emphasis => target(&mut inline, &mut quote_text).begin(Mark { italic: true, ..Mark::default() }),
                Tag::Strong => target(&mut inline, &mut quote_text).begin(Mark { bold: true, ..Mark::default() }),
                Tag::Strikethrough => target(&mut inline, &mut quote_text).begin(Mark { strike: true, ..Mark::default() }),
                Tag::Link { dest_url, .. } => target(&mut inline, &mut quote_text).begin(Mark { link: Some(dest_url.to_string()), ..Mark::default() }),
                Tag::Image { dest_url, .. } => target(&mut inline, &mut quote_text).begin(Mark { link: Some(dest_url.to_string()), ..Mark::default() }),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if lists.is_empty() && table.is_none() && quote == 0 && heading.is_none()
                        && let Some(i) = inline.take()
                    {
                        blocks.push(Block::Paragraph(i.finish()));
                    }
                }
                TagEnd::Heading(_) => {
                    if let (Some(level), Some(i)) = (heading.take(), inline.take()) {
                        emit(Block::Heading { level, text: i.finish() }, &mut blocks, &mut inline, &mut lists, &mut marked, &mut items);
                    }
                }
                TagEnd::BlockQuote(_) => {
                    quote = quote.saturating_sub(1);
                    if quote == 0 && let Some(q) = quote_text.take() {
                        emit(Block::Quote(q.finish()), &mut blocks, &mut inline, &mut lists, &mut marked, &mut items);
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some((language, mut text)) = code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        emit(Block::Code { language, code: text }, &mut blocks, &mut inline, &mut lists, &mut marked, &mut items);
                    }
                }
                TagEnd::Item => {
                    flush_item(&mut inline, &mut lists, &mut marked, &mut items);
                    marked.pop();
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() && !items.is_empty() {
                        blocks.push(Block::List(std::mem::take(&mut items)));
                    }
                }
                TagEnd::TableCell => {
                    if let (Some(i), Some(t)) = (inline.take(), &mut table) {
                        t.cells.push(i.finish());
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut table {
                        t.header = std::mem::take(&mut t.cells);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut table {
                        let row = std::mem::take(&mut t.cells);
                        t.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(t) = table.take() {
                        emit(Block::Table { header: t.header, rows: t.rows }, &mut blocks, &mut inline, &mut lists, &mut marked, &mut items);
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => target(&mut inline, &mut quote_text).end(),
                _ => {}
            },
            Event::Text(s) => {
                if let Some((_, c)) = &mut code {
                    c.push_str(&s);
                } else {
                    target(&mut inline, &mut quote_text).push_str(&s);
                }
            }
            Event::Code(s) => target(&mut inline, &mut quote_text).code(&s),
            Event::Html(s) | Event::InlineHtml(s) => target(&mut inline, &mut quote_text).push_str(&s),
            Event::SoftBreak => target(&mut inline, &mut quote_text).push_str(" "),
            Event::HardBreak => target(&mut inline, &mut quote_text).push_str("\n"),
            Event::Rule => emit(Block::Rule, &mut blocks, &mut inline, &mut lists, &mut marked, &mut items),
            Event::TaskListMarker(done) => target(&mut inline, &mut quote_text).push_str(if done { "☑ " } else { "☐ " }),
            _ => {}
        }
    }
    if let Some(i) = inline.take() {
        let t = i.finish();
        if !t.text.is_empty() {
            blocks.push(Block::Paragraph(t));
        }
    }
    blocks
}

/// A table while its cells arrive: `cells` is the row being filled.
#[derive(Default)]
struct TableBuild {
    header: Vec<Text>,
    rows: Vec<Vec<Text>>,
    cells: Vec<Text>,
}

/// The inline text being filled: the open item, cell or paragraph first, else the open quote,
/// else a paragraph started for text that arrives outside any.
fn target<'a>(inline: &'a mut Option<Inline>, quote: &'a mut Option<Inline>) -> &'a mut Inline {
    if inline.is_some() {
        return inline.as_mut().unwrap();
    }
    if quote.is_some() {
        return quote.as_mut().unwrap();
    }
    inline.get_or_insert_with(Inline::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(t: &Text) -> &str {
        &t.text
    }

    #[test]
    fn paragraphs_headings_and_inline_marks() {
        let blocks = parse("# Title\n\nSome **bold** and *it* and `code` and [a link](https://x.y).\n");
        assert_eq!(blocks.len(), 2);
        let Block::Heading { level, text } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!((*level, plain(text)), (1, "Title"));
        let Block::Paragraph(p) = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(p.text, "Some bold and it and code and a link.");
        let marks: Vec<(&str, &Mark)> = p.runs.iter().map(|r| (&p.text[r.range.clone()], &r.mark)).collect();
        assert_eq!(marks[0], ("bold", &Mark { bold: true, ..Mark::default() }));
        assert_eq!(marks[1], ("it", &Mark { italic: true, ..Mark::default() }));
        assert_eq!(marks[2], ("code", &Mark { code: true, ..Mark::default() }));
        assert_eq!(marks[3], ("a link", &Mark { link: Some("https://x.y".into()), ..Mark::default() }));
    }

    #[test]
    fn fenced_code_keeps_its_language_and_drops_the_fence_newline() {
        let blocks = parse("Before:\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nAfter.");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1], Block::Code { language: Some("rust".into()), code: "fn main() {\n    println!(\"hi\");\n}".into() });
        assert_eq!(blocks[2], Block::Paragraph(Text { text: "After.".into(), runs: vec![] }));
    }

    #[test]
    fn lists_nest_and_number() {
        let blocks = parse("1. one\n2. two\n   - inner\n");
        let Block::List(items) = &blocks[0] else { panic!("{blocks:?}") };
        let seen: Vec<(usize, &str, &str)> = items.iter().map(|i| (i.depth, i.marker.as_str(), plain(&i.text))).collect();
        assert_eq!(seen, vec![(0, "1.", "one"), (0, "2.", "two"), (1, "•", "inner")]);
    }

    #[test]
    fn a_block_inside_an_item_stays_under_its_item_and_keeps_the_numbering() {
        let blocks = parse("1. Run:\n   ```sh\n   make\n   ```\n2. two\n   - inner\n\n   more text\n3. three\n");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let Block::List(items) = &blocks[0] else { panic!("{blocks:?}") };
        let seen: Vec<(usize, &str, &str, bool)> = items.iter().map(|i| (i.depth, i.marker.as_str(), plain(&i.text), i.block.is_some())).collect();
        assert_eq!(seen, vec![(0, "1.", "Run:", false), (0, "", "", true), (0, "2.", "two", false), (1, "•", "inner", false), (0, "", "more text", false), (0, "3.", "three", false)]);
        assert_eq!(items[1].block.as_deref(), Some(&Block::Code { language: Some("sh".into()), code: "make".into() }));
    }

    #[test]
    fn tables_and_quotes() {
        let blocks = parse("| a | b |\n| - | - |\n| 1 | **2** |\n\n> quoted\n> words\n");
        let Block::Table { header, rows } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(header.iter().map(plain).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(rows[0].iter().map(plain).collect::<Vec<_>>(), vec!["1", "2"]);
        assert!(rows[0][1].runs[0].mark.bold);
        assert_eq!(blocks[1], Block::Quote(Text { text: "quoted words".into(), runs: vec![] }));
    }

    #[test]
    fn flatten_splits_nested_marks_into_disjoint_runs() {
        let runs = vec![
            Run { range: 0..10, mark: Mark { bold: true, ..Mark::default() } },
            Run { range: 3..6, mark: Mark { italic: true, ..Mark::default() } },
        ];
        let flat = flatten(&runs);
        assert_eq!(flat.iter().map(|r| (r.range.clone(), r.mark.bold, r.mark.italic)).collect::<Vec<_>>(), vec![(0..3, true, false), (3..6, true, true), (6..10, true, false)]);
    }

    #[test]
    fn highlight_colours_a_known_language_and_skips_an_unknown_one() {
        let runs = highlight(Some("rust"), "fn main() {}\nlet x = 1;").unwrap();
        assert!(runs.len() > 2, "{runs:?}");
        assert!(runs.iter().all(|r| r.mark.color.is_some()));
        assert!(runs.windows(2).all(|w| w[0].range.end <= w[1].range.start), "overlap: {runs:?}");
        let colours: std::collections::HashSet<u32> = runs.iter().filter_map(|r| r.mark.color).collect();
        assert!(colours.len() > 1, "one colour only: {colours:?}");
        assert!(highlight(Some("no-such-language"), "x").is_none());
        assert!(highlight(None, "x").is_none());
    }
}
