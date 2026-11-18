//! Markdown as blocks a page draws with its own components (ADR-0049).
//!
//! `blocks` parses a Markdown document with pulldown-cmark and returns one JSON value per block,
//! so a pane script decides every component, width and font itself:
//!
//! - `{kind: "heading", level, text}` and `{kind: "paragraph", text}`: `text` is the inline
//!   Markdown source (bold, italic, code, links), lines joined by spaces, for `Text{markdown = true}`.
//! - `{kind: "image", url, alt, width?, height?}`: a relative path is made a `file://` URL in
//!   `folder`; `width` and `height` are the pixel size of a PNG that could be read.
//! - `{kind: "code", lang, text}`, `{kind: "rule"}`, `{kind: "html", text}`.
//! - `{kind: "list", ordered, start, items: [[block]]}` and `{kind: "quote", blocks: [block]}`.
//! - `{kind: "table", align: ["left"|"center"|"right"|"none"], header: [text], rows: [[text]]}`.
//!
//! An image inside a paragraph splits it: the text before, the image, the text after.

use std::ops::Range;
use std::path::Path;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

type Ev<'a> = (Event<'a>, Range<usize>);

/// The blocks of `markdown`, with relative image paths resolved in `folder`.
pub fn blocks(markdown: &str, folder: Option<&Path>) -> Vec<Value> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events: Vec<Ev> = Parser::new_ext(markdown, options).into_offset_iter().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < events.len() {
        out.extend(block_run(markdown, &events, &mut i, folder));
        // A stray end at the top level: step over it.
        if i < events.len() && matches!(events[i].0, Event::End(_)) {
            i += 1;
        }
    }
    out
}

/// Blocks from `events[*i]` up to the end event that closes the caller's container (not consumed).
fn block_run(md: &str, ev: &[Ev], i: &mut usize, folder: Option<&Path>) -> Vec<Value> {
    let mut out = Vec::new();
    while *i < ev.len() {
        match &ev[*i].0 {
            Event::End(_) => return out,
            Event::Rule => {
                out.push(json!({ "kind": "rule" }));
                *i += 1;
            }
            Event::Html(_) => {
                let mut text = String::new();
                while let Some((Event::Html(h), _)) = ev.get(*i) {
                    text.push_str(h);
                    *i += 1;
                }
                out.push(json!({ "kind": "html", "text": text.trim_end() }));
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    *i += 1;
                    out.extend(inline_run(md, ev, i, folder));
                    *i += 1; // End(Paragraph)
                }
                Tag::Heading { level, .. } => {
                    let level = heading_level(*level);
                    *i += 1;
                    let text = inline_text(md, ev, i);
                    *i += 1; // End(Heading)
                    out.push(json!({ "kind": "heading", "level": level, "text": text }));
                }
                Tag::BlockQuote(_) => {
                    *i += 1;
                    let inner = block_run(md, ev, i, folder);
                    *i += 1;
                    out.push(json!({ "kind": "quote", "blocks": inner }));
                }
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    *i += 1;
                    let mut text = String::new();
                    while let Some((e, _)) = ev.get(*i) {
                        match e {
                            Event::End(TagEnd::CodeBlock) => break,
                            Event::Text(t) => text.push_str(t),
                            _ => {}
                        }
                        *i += 1;
                    }
                    *i += 1;
                    out.push(json!({ "kind": "code", "lang": lang, "text": text.trim_end_matches('\n') }));
                }
                Tag::List(start) => {
                    let ordered = start.is_some();
                    let first = start.unwrap_or(1);
                    *i += 1;
                    let mut items = Vec::new();
                    while let Some((Event::Start(Tag::Item), _)) = ev.get(*i) {
                        *i += 1;
                        items.push(Value::Array(block_run(md, ev, i, folder)));
                        *i += 1; // End(Item)
                    }
                    *i += 1; // End(List)
                    out.push(json!({ "kind": "list", "ordered": ordered, "start": first, "items": items }));
                }
                Tag::Table(aligns) => {
                    let align: Vec<&str> = aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Left => "left",
                            Alignment::Center => "center",
                            Alignment::Right => "right",
                            Alignment::None => "none",
                        })
                        .collect();
                    *i += 1;
                    let mut header = Vec::new();
                    let mut rows: Vec<Vec<String>> = Vec::new();
                    while let Some((e, _)) = ev.get(*i) {
                        match e {
                            Event::End(TagEnd::Table) => break,
                            Event::Start(Tag::TableRow) => rows.push(Vec::new()),
                            Event::Start(Tag::TableCell) => {
                                let in_head = rows.is_empty();
                                *i += 1;
                                let cell = inline_text(md, ev, i);
                                if in_head {
                                    header.push(cell);
                                } else if let Some(r) = rows.last_mut() {
                                    r.push(cell);
                                }
                            }
                            _ => {}
                        }
                        *i += 1;
                    }
                    *i += 1;
                    out.push(json!({ "kind": "table", "align": align, "header": header, "rows": rows }));
                }
                Tag::HtmlBlock => {
                    *i += 1;
                    let mut text = String::new();
                    while let Some((e, _)) = ev.get(*i) {
                        match e {
                            Event::End(TagEnd::HtmlBlock) => break,
                            Event::Html(h) | Event::Text(h) => text.push_str(h),
                            _ => {}
                        }
                        *i += 1;
                    }
                    *i += 1;
                    out.push(json!({ "kind": "html", "text": text.trim_end() }));
                }
                t if is_inline(t) => out.extend(inline_run(md, ev, i, folder)),
                _ => skip(ev, i),
            },
            // Text directly in a tight list item: an inline run with no paragraph around it.
            _ => out.extend(inline_run(md, ev, i, folder)),
        }
    }
    out
}

/// Inline events from `events[*i]` to the first block start or block end (not consumed), as
/// paragraphs split around each image.
fn inline_run(md: &str, ev: &[Ev], i: &mut usize, folder: Option<&Path>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut seg: Option<Range<usize>> = None;
    let flush = |out: &mut Vec<Value>, seg: &mut Option<Range<usize>>| {
        if let Some(r) = seg.take() {
            let text = join_lines(&md[r]);
            if !text.is_empty() {
                out.push(json!({ "kind": "paragraph", "text": text }));
            }
        }
    };
    let mut depth = 0usize;
    while let Some((e, r)) = ev.get(*i) {
        match e {
            Event::Start(Tag::Image { dest_url, .. }) => {
                flush(&mut out, &mut seg);
                let url = dest_url.to_string();
                let start = *i;
                *i += 1;
                while !matches!(ev.get(*i), Some((Event::End(TagEnd::Image), _)) | None) {
                    *i += 1;
                }
                let alt = alt_text(&ev[start + 1..*i]);
                out.push(image(&url, &alt, folder));
                *i += 1;
                continue;
            }
            Event::Start(t) if is_inline(t) => depth += 1,
            Event::End(t) if is_inline_end(t) && depth > 0 => depth -= 1,
            Event::Start(_) | Event::End(_) | Event::Rule => break,
            _ => {}
        }
        seg = Some(match seg {
            Some(s) => s.start..r.end.max(s.end),
            None => r.clone(),
        });
        *i += 1;
    }
    flush(&mut out, &mut seg);
    out
}

/// The inline source from `events[*i]` to the closing end (not consumed), lines joined.
fn inline_text(md: &str, ev: &[Ev], i: &mut usize) -> String {
    let mut span: Option<Range<usize>> = None;
    let mut depth = 0usize;
    while let Some((e, r)) = ev.get(*i) {
        match e {
            Event::Start(_) => depth += 1,
            Event::End(_) if depth == 0 => break,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        span = Some(match span {
            Some(s) => s.start..r.end.max(s.end),
            None => r.clone(),
        });
        *i += 1;
    }
    span.map(|r| join_lines(&md[r])).unwrap_or_default()
}

fn skip(ev: &[Ev], i: &mut usize) {
    let mut depth = 0usize;
    while let Some((e, _)) = ev.get(*i) {
        match e {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    *i += 1;
                    return;
                }
            }
            _ => {}
        }
        *i += 1;
    }
}

fn is_inline(t: &Tag) -> bool {
    matches!(t, Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. } | Tag::Superscript | Tag::Subscript)
}

fn is_inline_end(t: &TagEnd) -> bool {
    matches!(t, TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image | TagEnd::Superscript | TagEnd::Subscript)
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Source lines joined by one space each, a hard break (two trailing spaces or `\`) kept as a newline.
fn join_lines(s: &str) -> String {
    let mut out = String::new();
    for line in s.trim().lines() {
        let hard = line.ends_with("  ") || line.ends_with('\\');
        let t = line.trim();
        let t = if hard && t.ends_with('\\') { &t[..t.len() - 1] } else { t };
        if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(t);
        if hard {
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn alt_text(ev: &[Ev]) -> String {
    ev.iter()
        .filter_map(|(e, _)| match e {
            Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
            _ => None,
        })
        .collect()
}

fn image(target: &str, alt: &str, folder: Option<&Path>) -> Value {
    let local = if target.contains("://") || target.is_empty() || target.starts_with('#') {
        None
    } else if target.starts_with('/') {
        Some(Path::new(target).to_path_buf())
    } else {
        folder.map(|f| f.join(target))
    };
    let url = match &local {
        Some(p) => format!("file://{}", p.to_string_lossy().replace('%', "%25").replace(' ', "%20")),
        None => target.to_string(),
    };
    let mut v = json!({ "kind": "image", "url": url, "alt": alt });
    if let Some((w, h)) = local.as_deref().and_then(png_size) {
        v["width"] = json!(w);
        v["height"] = json!(h);
    }
    v
}

/// The pixel size in a PNG's IHDR chunk.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(head[16..20].try_into().ok()?), u32::from_be_bytes(head[20..24].try_into().ok()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_kind() {
        let md = "# Title\n\nA **bold**\nword and `code`.\n\n- one\n- two *x*\n\n3. c\n4. d\n\n> quoted\n\n```rust\nlet x = 1;\n```\n\n---\n\n| A | B |\n|:--|--:|\n| 1 | **2** |\n";
        let b = blocks(md, None);
        let kinds: Vec<&str> = b.iter().map(|v| v["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["heading", "paragraph", "list", "list", "quote", "code", "rule", "table"]);
        assert_eq!(b[0], json!({ "kind": "heading", "level": 1, "text": "Title" }));
        assert_eq!(b[1]["text"], "A **bold** word and `code`.");
        assert_eq!(b[2]["items"], json!([[{ "kind": "paragraph", "text": "one" }], [{ "kind": "paragraph", "text": "two *x*" }]]));
        assert_eq!(b[3]["ordered"], true);
        assert_eq!(b[3]["start"], 3);
        assert_eq!(b[4]["blocks"], json!([{ "kind": "paragraph", "text": "quoted" }]));
        assert_eq!(b[5], json!({ "kind": "code", "lang": "rust", "text": "let x = 1;" }));
        assert_eq!(b[7], json!({ "kind": "table", "align": ["left", "right"], "header": ["A", "B"], "rows": [["1", "**2**"]] }));
    }

    #[test]
    fn an_image_splits_its_paragraph_and_resolves_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("sdk-md-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1594u32.to_be_bytes());
        png.extend(800u32.to_be_bytes());
        std::fs::write(dir.join("a b.png"), &png).unwrap();
        let b = blocks("Before ![The A](<a b.png>) after.\n\n![w](https://x.org/w.png)\n", Some(&dir));
        let url = format!("file://{}/a%20b.png", dir.display());
        assert_eq!(
            b,
            vec![
                json!({ "kind": "paragraph", "text": "Before" }),
                json!({ "kind": "image", "url": url, "alt": "The A", "width": 1594, "height": 800 }),
                json!({ "kind": "paragraph", "text": "after." }),
                json!({ "kind": "image", "url": "https://x.org/w.png", "alt": "w" }),
            ]
        );
    }

    #[test]
    fn nested_lists_and_loose_items_keep_their_blocks() {
        let b = blocks("- a\n  - b\n\n- c\n\n  more\n", None);
        assert_eq!(b.len(), 1);
        let items = b[0]["items"].as_array().unwrap();
        assert_eq!(items[0][0], json!({ "kind": "paragraph", "text": "a" }));
        assert_eq!(items[0][1]["kind"], "list");
        assert_eq!(items[1], json!([{ "kind": "paragraph", "text": "c" }, { "kind": "paragraph", "text": "more" }]));
    }
}
