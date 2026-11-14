//! A PR's description or comment as data for a pane: GitHub-flavoured Markdown (headings, lists,
//! quotes, code, rules, tables, inline emphasis, links) turned into blocks (see [`blocks`]), and its
//! images as image blocks.
//!
//! Images that live in the repo (`github.com/<owner>/<repo>/blob/<ref>/<path>?raw=true` and
//! `raw.githubusercontent.com`) need GitHub's authentication in a private repo, which the client
//! has not got, so [`prefetch`] downloads them with `gh` into a cache folder and the page points
//! the image at the file. Every other image keeps its own URL.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Command;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("wheel-prs-img")
}

/// Where a repo-hosted image's bytes are kept once downloaded, or `None` for a URL GitHub serves openly.
fn local_path(url: &str) -> Option<PathBuf> {
    repo_file(url)?;
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    let ext = url.split(['?', '#']).next().and_then(|u| u.rsplit('.').next()).filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("img");
    Some(cache_dir().join(format!("{:x}.{ext}", h.finish())))
}

/// `(owner/repo, ref, path)` of an image URL that points into a repository.
fn repo_file(url: &str) -> Option<(String, String, String)> {
    let u = url::Url::parse(url).ok()?;
    let seg: Vec<&str> = u.path_segments()?.collect();
    match u.host_str()? {
        "github.com" if seg.len() >= 5 && (seg[2] == "blob" || seg[2] == "raw") => Some((format!("{}/{}", seg[0], seg[1]), seg[3].to_string(), seg[4..].join("/"))),
        "raw.githubusercontent.com" if seg.len() >= 4 => Some((format!("{}/{}", seg[0], seg[1]), seg[2].to_string(), seg[3..].join("/"))),
        _ => None,
    }
}

/// The URL to draw an image from: its file in the cache when it has been downloaded, else its own.
pub fn source(url: &str) -> String {
    match local_path(url) {
        Some(p) if p.is_file() => url::Url::from_file_path(&p).map(|u| u.to_string()).unwrap_or_else(|_| url.to_string()),
        _ => url.to_string(),
    }
}

/// Download the repo-hosted images in `markdown` that are not cached yet. A failure leaves that image to its own URL.
pub fn prefetch(dir: &str, markdown: &str) {
    for url in images_in(markdown) {
        let (Some(path), Some((repo, r#ref, file))) = (local_path(&url), repo_file(&url)) else { continue };
        if path.is_file() {
            continue;
        }
        let _ = std::fs::create_dir_all(cache_dir());
        let out = Command::new("gh")
            .args(["api", &format!("repos/{repo}/contents/{file}?ref={}", r#ref), "-H", "Accept: application/vnd.github.raw"])
            .current_dir(dir)
            .output();
        if let Ok(o) = out {
            if o.status.success() && !o.stdout.is_empty() {
                let _ = std::fs::write(&path, o.stdout);
            }
        }
    }
}

/// The image URLs a Markdown text names, in Markdown and in HTML.
pub fn images_in(markdown: &str) -> Vec<String> {
    super::detail::split_images(markdown).1
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '~' | '<' | '>') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn attr(tag: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let key = format!("{name}={q}");
        if let Some(i) = tag.find(&key) {
            let rest = &tag[i + key.len()..];
            return Some(rest[..rest.find(q)?].to_string());
        }
    }
    None
}

struct Draw {
    out: Vec<Value>,
    inl: String,
    images: Vec<(String, String)>,
    /// The open lists: the next number of an ordered one, `None` for a bulleted one.
    lists: Vec<Option<u64>>,
    bullet: Option<String>,
    quote: u32,
    heading: Option<HeadingLevel>,
    code: Option<String>,
    alt: Option<(String, String)>,
    links: Vec<String>,
    comment: bool,
    table: Option<Vec<Vec<String>>>,
    row: Vec<String>,
}

impl Draw {
    fn indent(&self) -> u32 {
        (self.lists.len() as u32).saturating_sub(1) * 16 + self.quote * 12
    }

    /// The text collected so far, as one block, then the images it named.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.inl);
        let bullet = self.bullet.take();
        let text = text.trim().to_string();
        if !text.is_empty() || bullet.is_some() && !self.images.is_empty() {
            let heading = match self.heading {
                Some(HeadingLevel::H1) => 1,
                Some(HeadingLevel::H2) => 2,
                Some(HeadingLevel::H3) => 3,
                Some(_) => 4,
                None => 0,
            };
            let shown = format!("{}{text}", bullet.unwrap_or_default());
            self.out.push(json!({ "kind": "text", "text": shown, "heading": heading, "quote": self.quote > 0, "indent": self.indent() }));
        }
        for (url, alt) in std::mem::take(&mut self.images) {
            self.out.push(json!({ "kind": "image", "url": source(&url), "alt": alt, "indent": self.indent() }));
        }
    }

    fn text(&mut self, s: &str) {
        if let Some(c) = &mut self.code {
            c.push_str(s);
        } else if let Some((_, alt)) = &mut self.alt {
            alt.push_str(s);
        } else {
            self.inl.push_str(&esc(s));
        }
    }

    /// Raw HTML in the text: comments vanish, `<img>` is an image, `<br>` a line break, every other tag is dropped and
    /// the text between tags stays.
    fn html(&mut self, s: &str, block: bool) {
        let mut rest = s;
        loop {
            if self.comment {
                match rest.find("-->") {
                    Some(i) => {
                        rest = &rest[i + 3..];
                        self.comment = false;
                    }
                    None => return,
                }
                continue;
            }
            let Some(i) = rest.find('<') else {
                self.html_text(rest, block);
                return;
            };
            self.html_text(&rest[..i], block);
            let tail = &rest[i..];
            if let Some(after) = tail.strip_prefix("<!--") {
                self.comment = true;
                rest = after;
                continue;
            }
            let Some(e) = tail.find('>') else {
                self.html_text(tail, block);
                return;
            };
            let tag = &tail[..=e];
            let name: String = tag[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
            match name.as_str() {
                "img" => {
                    if let Some(src) = attr(tag, "src").filter(|u| u.starts_with("http")) {
                        self.images.push((src, attr(tag, "alt").unwrap_or_default()));
                    }
                }
                "br" => self.inl.push('\n'),
                _ => {}
            }
            rest = &tail[e + 1..];
        }
    }

    fn html_text(&mut self, t: &str, block: bool) {
        if !(block && t.trim().is_empty()) && !t.is_empty() {
            let t = if block { t.trim() } else { t };
            self.inl.push_str(&esc(t));
        }
    }
}

/// `md` as blocks, in order: `{kind: "text", text, heading, quote, indent}` (`text` is inline Markdown with its
/// list bullet; `heading` is 0 for none, 1 to 3 for H1 to H3, 4 for deeper), `{kind: "image", url, alt, indent}`,
/// `{kind: "code", text, indent}` and `{kind: "rule"}`. `indent` is in points.
pub fn blocks(md: &str) -> Vec<Value> {
    let mut d = Draw { out: vec![], inl: String::new(), images: vec![], lists: vec![], bullet: None, quote: 0, heading: None, code: None,
        alt: None, links: vec![], comment: false, table: None, row: vec![] };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(md, opts) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                d.flush();
                d.heading = Some(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                d.flush();
                d.heading = None;
            }
            Event::End(TagEnd::Paragraph) => d.flush(),
            Event::Start(Tag::BlockQuote(_)) => {
                d.flush();
                d.quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                d.flush();
                d.quote -= 1;
            }
            Event::Start(Tag::List(start)) => {
                d.flush();
                d.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                d.flush();
                d.lists.pop();
            }
            Event::Start(Tag::Item) => {
                d.flush();
                d.bullet = Some(match d.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    }
                    _ => "• ".to_string(),
                });
            }
            Event::End(TagEnd::Item) => d.flush(),
            Event::TaskListMarker(done) => d.bullet = Some(if done { "☑ " } else { "☐ " }.to_string()),
            Event::Start(Tag::CodeBlock(_)) => {
                d.flush();
                d.code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                let code = d.code.take().unwrap_or_default();
                let indent = d.indent();
                d.out.push(json!({ "kind": "code", "text": code.trim_end(), "indent": indent }));
            }
            Event::Rule => {
                d.flush();
                d.out.push(json!({ "kind": "rule" }));
            }
            Event::Start(Tag::Emphasis) => d.inl.push('*'),
            Event::End(TagEnd::Emphasis) => d.inl.push('*'),
            Event::Start(Tag::Strong) => d.inl.push_str("**"),
            Event::End(TagEnd::Strong) => d.inl.push_str("**"),
            Event::Start(Tag::Strikethrough) => d.inl.push_str("~~"),
            Event::End(TagEnd::Strikethrough) => d.inl.push_str("~~"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                d.links.push(dest_url.replace(')', "%29"));
                d.inl.push('[');
            }
            Event::End(TagEnd::Link) => {
                let dest = d.links.pop().unwrap_or_default();
                d.inl.push_str(&format!("]({dest})"));
            }
            Event::Start(Tag::Image { dest_url, .. }) => d.alt = Some((dest_url.to_string(), String::new())),
            Event::End(TagEnd::Image) => {
                if let Some((url, alt)) = d.alt.take() {
                    if url.starts_with("http") {
                        d.images.push((url, alt));
                    }
                }
            }
            Event::Start(Tag::Table(_)) => {
                d.flush();
                d.table = Some(vec![]);
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => d.row.clear(),
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut d.inl);
                d.row.push(cell);
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut d.row);
                if let Some(t) = &mut d.table {
                    t.push(row);
                }
            }
            Event::End(TagEnd::Table) => {
                for (i, row) in d.table.take().unwrap_or_default().into_iter().enumerate() {
                    let line = row.join("  |  ");
                    d.inl = if i == 0 { format!("**{}**", line.replace("**", "")) } else { line };
                    d.flush();
                }
            }
            Event::Code(s) => d.inl.push_str(&format!("`{}`", s.replace('`', "'"))),
            Event::Text(s) => d.text(&s),
            Event::SoftBreak | Event::HardBreak => {
                if d.code.is_some() {
                    d.text("\n");
                } else {
                    d.inl.push('\n');
                }
            }
            Event::Html(s) => d.html(&s, true),
            Event::InlineHtml(s) => d.html(&s, false),
            _ => {}
        }
    }
    d.flush();
    d.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(out: &[Value]) -> Vec<String> {
        out.iter().filter(|c| c["kind"] == "text" || c["kind"] == "code").map(|c| c["text"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn headings_lists_code_and_comments_become_blocks_and_html_comments_vanish() {
        let out = blocks("## Problem\nA **bold** claim with `code`.\n\n- one\n- two\n\n1. first\n\n```\nlet x = 1;\n```\n\n<!-- hidden\nmore -->\nAfter");
        assert_eq!(texts(&out), vec!["Problem", "A **bold** claim with `code`.", "• one", "• two", "1. first", "let x = 1;", "After"]);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0], json!({ "kind": "text", "text": "Problem", "heading": 2, "quote": false, "indent": 0 }));
        assert_eq!(out[1]["heading"], 0);
        assert_eq!(out[5], json!({ "kind": "code", "text": "let x = 1;", "indent": 0 }));
    }

    #[test]
    fn images_in_markdown_and_html_are_blocks_after_their_text_and_other_tags_drop() {
        let out = blocks("Before:\n![shot](https://x/a.png)\n\n<a href=\"u\"><picture><img src=\"https://x/b.png\" alt=\"after\"></picture></a>\n<sup>Open in <b>tool</b></sup>");
        let imgs: Vec<&str> = out.iter().filter(|c| c["kind"] == "image").map(|c| c["url"].as_str().unwrap()).collect();
        assert_eq!(imgs, vec!["https://x/a.png", "https://x/b.png"]);
        assert_eq!(out.iter().find(|c| c["kind"] == "image").unwrap()["alt"], "shot");
        assert!(texts(&out).iter().any(|t| t == "Open in tool"));
    }

    #[test]
    fn quotes_nested_lists_tasks_rules_tables_and_breaks() {
        let out = blocks("> quoted\n\n- a\n  - b\n\n- [x] done\n- [ ] todo\n\n---\n\n| h1 | h2 |\n|----|----|\n| c1 | c2 |\n\nline<br>two");
        assert_eq!(out[0]["quote"], true);
        assert_eq!(out[0]["indent"], 12);
        let b = out.iter().find(|c| c["text"] == "• b").unwrap();
        assert_eq!(b["indent"], 16);
        let t = texts(&out);
        assert!(t.contains(&"☑ done".to_string()) && t.contains(&"☐ todo".to_string()));
        assert!(out.iter().any(|c| c["kind"] == "rule"));
        assert!(t.contains(&"**h1  |  h2**".to_string()) && t.contains(&"c1  |  c2".to_string()));
        assert!(t.contains(&"line\ntwo".to_string()));
    }

    #[test]
    fn a_repo_image_is_found_by_its_blob_url() {
        let u = "https://github.com/o/r/blob/abc123/.pr-assets/a.png?raw=true";
        assert_eq!(repo_file(u), Some(("o/r".into(), "abc123".into(), ".pr-assets/a.png".into())));
        assert_eq!(repo_file("https://camo.githubusercontent.com/x"), None);
        assert!(local_path(u).unwrap().to_string_lossy().ends_with(".png"));
        assert_eq!(source("https://camo.githubusercontent.com/x"), "https://camo.githubusercontent.com/x");
    }
}
