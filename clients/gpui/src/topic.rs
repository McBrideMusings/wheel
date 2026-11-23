//! The orchestrator's current topic (ADR-0054, design §8.2 M1/M8) as Orchestrator mode's title row
//! shows it: `threads/current` names the topic, `threads/<id>.md` holds its title and notes. The
//! reading mirrors the phone's `OrchestratorTopic` (`apple/iOS/Sources/TopicNotesSheet.swift`). The
//! view is `view/topic.rs`.
use std::collections::HashMap;

/// The topic files, relative to the orchestrator's folder.
pub const CURRENT: &str = "threads/current";
pub fn notes_path(id: &str) -> String {
    format!("threads/{id}.md")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    pub id: String,
    pub title: String,
    /// The notes file's body after its front matter, trimmed; empty when it has none.
    pub notes: String,
}

impl Topic {
    /// The topic `id` from its notes file's lines; a file with no `title:` is titled by its id.
    pub fn of(id: &str, lines: &[String]) -> Topic {
        let mut body: &[String] = lines;
        let mut title = None;
        if body.first().map(String::as_str) == Some("---")
            && let Some(end) = body[1..].iter().position(|l| l == "---").map(|i| i + 1)
        {
            for line in &body[1..end] {
                if let Some((key, value)) = line.split_once(':')
                    && key.trim() == "title"
                {
                    title = Some(value.trim().to_owned());
                }
            }
            body = &body[end + 1..];
        }
        Topic { id: id.to_owned(), title: title.filter(|t| !t.is_empty()).unwrap_or_else(|| id.to_owned()), notes: body.join("\n").trim().to_owned() }
    }
}

/// The id `threads/current` holds, when it is one `tools/switch` would write: letters, digits, `-`,
/// `.` and `_`, and never `.` or `..`.
pub fn current_id(lines: &[String]) -> Option<String> {
    let id = lines.first()?.trim();
    (!id.is_empty() && id != "." && id != ".." && id.chars().all(|c| c.is_alphanumeric() || "-._".contains(c))).then(|| id.to_owned())
}

/// Which file a `file_peek` the client sent for the topic reads.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Current,
    Notes(String),
}

/// The topic line's state: the topic last read, the reads in flight, and whether the notes card is open.
#[derive(Default)]
pub struct Line {
    pub topic: Option<Topic>,
    pub reqs: HashMap<u64, Read>,
    pub open: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn reads_the_title_from_front_matter_and_keeps_the_body_as_notes() {
        let t = Topic::of("plugin-reload", &lines("---\ntitle: wheel · plugin hot reload\nopened: 2026-11-20\n---\n\n## Decisions\n- keep state in the feed\n"));
        assert_eq!(t.title, "wheel · plugin hot reload");
        assert_eq!(t.notes, "## Decisions\n- keep state in the feed");
        let bare = Topic::of("bare", &lines("just notes"));
        assert_eq!((bare.title.as_str(), bare.notes.as_str()), ("bare", "just notes"));
        let untitled = Topic::of("x", &lines("---\ntitle:\n---\n"));
        assert_eq!((untitled.title.as_str(), untitled.notes.as_str()), ("x", ""));
        let unclosed = Topic::of("y", &lines("---\ntitle: never closed"));
        assert_eq!((unclosed.title.as_str(), unclosed.notes.as_str()), ("y", "---\ntitle: never closed"));
    }

    #[test]
    fn current_takes_only_an_id_switch_would_write() {
        assert_eq!(current_id(&lines("  plugin-reload_2.v1  \n")), Some("plugin-reload_2.v1".into()));
        for bad in ["", "..", ".", "../etc/passwd", "a b", "a/b"] {
            assert_eq!(current_id(&lines(bad)), None, "{bad:?}");
        }
        assert_eq!(current_id(&[]), None);
    }
}
