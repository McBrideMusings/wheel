//! What voice mode says for an agent's reply (WheelKit's `SpokenText`): the prose, with the markup a
//! voice would read out loud taken away.
use regex::Regex;
use std::sync::LazyLock;

static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").unwrap());
static LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*(?:[-*+]|\d+[.)])[ \t]+").unwrap());
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*>[ \t]?").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[`*~]+").unwrap());

/// `markdown` as speech: fenced code blocks dropped, inline code and emphasis marks removed, links
/// reduced to their text, headings and list marks removed, lines joined into sentences.
pub fn from(markdown: &str) -> String {
    let mut fenced = false;
    let kept: Vec<&str> = markdown
        .split('\n')
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                return false;
            }
            !fenced
        })
        .collect();
    let text = kept.join("\n");
    let text = IMAGE.replace_all(&text, "$1");
    let text = LINK.replace_all(&text, "$1");
    let text = HEADING.replace_all(&text, "");
    let text = LIST.replace_all(&text, "");
    let text = QUOTE.replace_all(&text, "");
    let text = URL.replace_all(&text, "a link");
    let text = MARKS.replace_all(&text, "");
    let text = without_emphasis_underscores(&text);
    text.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| if line.ends_with(['.', '!', '?', ':', ';']) { line.to_owned() } else { format!("{line}.") })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Underscores that mark emphasis go; a run between two letters or digits (`read_aloud`) stays.
fn without_emphasis_underscores(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '_' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i] == '_' {
            i += 1;
        }
        let inside = start > 0 && chars[start - 1].is_ascii_alphanumeric() && chars.get(i).is_some_and(char::is_ascii_alphanumeric);
        if inside {
            out.extend(&chars[start..i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::from;

    #[test]
    fn code_blocks_are_skipped_and_inline_marks_removed() {
        let reply = "## Done\n\nI changed `main.rs` and **fixed** the [bug](https://x.test).\n\n```rust\nfn main() {}\n```\n\n- first\n- second";
        assert_eq!(from(reply), "Done. I changed main.rs and fixed the bug. first. second.");
    }

    #[test]
    fn names_keep_their_underscores_and_links_are_not_read_out() {
        assert_eq!(from("Call read_aloud on _this_ and see https://x.test/a_b now"), "Call read_aloud on this and see a link now.");
    }

    #[test]
    fn an_empty_reply_says_nothing() {
        assert!(from("```\ncode\n```").is_empty());
    }

    #[test]
    fn quotes_and_numbered_lists_lose_their_marks() {
        assert_eq!(from("> quoted\n1. one\n2) two"), "quoted. one. two.");
    }
}
