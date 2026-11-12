//! What the composer adds to typing: the agent's command list opened by its prefix
//! (`Composer.swift` with WheelKit's `SkillSearch`), which message Esc or Up takes back
//! (`AgentChat.takeBack`), and the files it holds for the next message (`ComposerAttachments`).
use std::path::{Path, PathBuf};
use wheel_proto::SkillInfo;

/// One command the list offers: its name without the prefix and what it does, when known.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

/// The character that starts a command in a harness's composer: `/` for Claude, `$` for Codex.
/// None for a harness with no command syntax Wheel knows.
pub fn prefix(harness: &str) -> Option<char> {
    match harness {
        "claude" => Some('/'),
        "codex" => Some('$'),
        _ => None,
    }
}

/// The query when `draft` is a command being typed: it starts with `prefix` and the rest holds no
/// whitespace yet. None once the name is finished or the draft is something else.
pub fn query(draft: &str, prefix: char) -> Option<&str> {
    let rest = draft.strip_prefix(prefix)?;
    (!rest.chars().any(char::is_whitespace)).then_some(rest)
}

/// The commands the list offers for `harness`. Claude's are the ones its session says it accepts
/// (`agent_commands`), described from the skills in its home; none until the session has said.
/// Every other harness offers the skills in its home (`AgentChat.accepted`).
pub fn offered(harness: &str, skills: &[SkillInfo], commands: Option<&[String]>) -> Vec<Command> {
    if harness != "claude" {
        return skills.iter().map(Command::from).collect();
    }
    let mut names: Vec<&String> = commands.unwrap_or_default().iter().collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|n| skills.iter().find(|s| &s.name == n).map(Command::from).unwrap_or_else(|| Command { name: n.clone(), description: String::new() })).collect()
}

impl From<&SkillInfo> for Command {
    fn from(s: &SkillInfo) -> Self {
        Command { name: s.name.clone(), description: s.description.clone() }
    }
}

/// The commands `query` matches, best first. Every query letter must appear in a name in order:
/// a name that starts with the query, then one that contains it, then a looser in-order match
/// with fewer letters between; a description containing the query matches last. An empty query
/// lists every command by name.
pub fn rank(query: &str, commands: &[Command]) -> Vec<Command> {
    let q = query.to_lowercase();
    let mut scored: Vec<(usize, &Command)> = commands.iter().filter_map(|c| {
        let name = c.name.to_lowercase();
        let score = if q.is_empty() || name.starts_with(&q) {
            0
        } else if name.contains(&q) {
            1
        } else if let Some(gaps) = gaps(&q, &name) {
            2 + gaps
        } else if c.description.to_lowercase().contains(&q) {
            1000
        } else {
            return None;
        };
        Some((score, c))
    }).collect();
    scored.sort_by(|a, b| (a.0, &a.1.name).cmp(&(b.0, &b.1.name)));
    scored.into_iter().map(|(_, c)| c.clone()).collect()
}

/// How many letters of `name` sit between the first and last matched letter beyond the query's
/// own, when every letter of `q` appears in order; none when one does not.
fn gaps(q: &str, name: &str) -> Option<usize> {
    let chars: Vec<char> = name.chars().collect();
    let (mut at, mut first, mut last) = (0, None, 0);
    for ch in q.chars() {
        let hit = at + chars[at..].iter().position(|c| *c == ch)?;
        first.get_or_insert(hit);
        last = hit;
        at = hit + 1;
    }
    Some(first.map_or(0, |f| last + 1 - f - q.chars().count()))
}

/// The list's state for one composer. Any change to the draft starts the highlight and the
/// arrows' choice over (`edited`), so Return sends a built-in such as `/clear` as typed until the
/// arrows have chosen; Tab always accepts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picker {
    highlighted: usize,
    chosen: bool,
    /// A draft the person shut the list on with Esc; the list stays shut until it changes.
    dismissed: Option<String>,
}

impl Picker {
    /// The rows the list shows for `draft`: empty when it is shut or nothing matches.
    pub fn matches(&self, draft: &str, prefix: Option<char>, commands: &[Command]) -> Vec<Command> {
        match prefix.and_then(|p| query(draft, p)) {
            Some(q) if self.dismissed.as_deref() != Some(draft) => rank(q, commands),
            _ => vec![],
        }
    }

    /// The highlighted row among `count`.
    pub fn highlighted(&self, count: usize) -> usize {
        self.highlighted.min(count.saturating_sub(1))
    }

    /// Whether the arrows chose the highlighted row since the draft last changed.
    pub fn chosen(&self) -> bool {
        self.chosen
    }

    /// The draft changed: the first row is highlighted and nothing is chosen.
    pub fn edited(&mut self) {
        self.highlighted = 0;
        self.chosen = false;
    }

    /// Down or Up over `count` rows, wrapping at the ends.
    pub fn step(&mut self, count: usize, down: bool) {
        if count == 0 {
            return;
        }
        let at = self.highlighted(count);
        self.highlighted = if down { (at + 1) % count } else { (at + count - 1) % count };
        self.chosen = true;
    }

    /// Esc on an open list: shut it for this draft.
    pub fn dismiss(&mut self, draft: &str) {
        self.dismissed = Some(draft.to_owned());
    }
}

/// The draft a command picked from the list leaves: the prefix, the name and a space.
pub fn accepted(prefix: char, command: &Command) -> String {
    format!("{prefix}{} ", command.name)
}

/// The message Esc or Up in an empty composer takes back: the running turn's while it has said
/// nothing, else, while no turn runs, the last queued one (its `turn_start` is on its way). None
/// while a turn runs and has answered.
pub fn take_back_target(unanswered: Option<u64>, busy: bool, queued: &[u64]) -> Option<u64> {
    unanswered.or_else(|| if busy { None } else { queued.last().copied() })
}

/// The draft after a taken-back message's text comes back: alone in an empty draft, else after
/// what is there on a line of its own.
pub fn restored(draft: &str, text: &str) -> String {
    if draft.is_empty() { text.to_owned() } else { format!("{draft}\n{text}") }
}

/// The message a draft and its attachments' paths make (`composerMessage`): the text, then each
/// path in backticks on its own line, since a path may hold a space and the backticks keep it one
/// path. Empty when both are.
pub fn message(typed: &str, paths: &[String]) -> String {
    let listed = paths.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join("\n");
    [typed.trim(), listed.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// The draft with a message quoted at its top (`attachAsContext`): each line of the Markdown after
/// `> ` (a blank one as `>`), a blank line, then the draft as it was.
pub fn attach_as_context(draft: &str, markdown: &str) -> String {
    let quoted = markdown.split('\n').map(|l| if l.is_empty() { ">".to_owned() } else { format!("> {l}") }).collect::<Vec<_>>().join("\n");
    format!("{quoted}\n\n{draft}")
}

/// One file a composer holds for its next message: what its chip shows and the path that goes
/// out at send.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub id: u64,
    pub name: String,
    /// The size, or "Folder".
    pub detail: String,
    pub path: PathBuf,
    /// A PNG of the image at most 96 px on its longer side, made once when attached; None draws
    /// the document symbol (not an image, or one that does not decode).
    pub thumbnail: Option<Vec<u8>>,
    /// Why it could not be attached; a chip with one holds the message back until it is removed.
    pub failed: Option<String>,
    /// A copy the tray wrote (a pasted image), deleted with its folder when the chip goes.
    owned: bool,
}

/// The files one agent's composer holds (`ComposerAttachments`). The agent runs on this Mac, so
/// a file is attached where it is and nothing is copied; an image with no file (a screenshot on
/// the clipboard) is written once as PNG to a folder of its own so it has a path to send.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tray {
    items: Vec<Attachment>,
    next: u64,
}

/// Where a pasted image is written: one folder per image under the temp dir's `wheel-attachments`.
pub fn attachments_folder() -> PathBuf {
    std::env::temp_dir().join("wheel-attachments")
}

/// The extensions a chip draws as a thumbnail.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "heic", "tif", "tiff", "bmp"];

impl Tray {
    pub fn items(&self) -> &[Attachment] {
        &self.items
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn paths(&self) -> Vec<String> {
        self.items.iter().map(|i| i.path.to_string_lossy().into_owned()).collect()
    }
    /// Whether every chip can go out: none failed.
    pub fn ready(&self) -> bool {
        self.items.iter().all(|i| i.failed.is_none())
    }

    fn push(&mut self, name: String, detail: String, path: PathBuf, thumbnail: Option<Vec<u8>>, failed: Option<String>, owned: bool) {
        self.next += 1;
        self.items.push(Attachment { id: self.next, name, detail, path, thumbnail, failed, owned });
    }

    /// Attach the file or folder at `path` where it is: its name, and its size or "Folder".
    pub fn add_file(&mut self, path: &Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
        let meta = std::fs::metadata(path).ok();
        let detail = match &meta {
            Some(m) if m.is_dir() => "Folder".to_owned(),
            Some(m) => byte_count(m.len()),
            None => String::new(),
        };
        let image = meta.as_ref().is_some_and(|m| m.is_file())
            && path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()));
        let thumbnail = image.then(|| image::open(path).ok().and_then(|i| thumbnail(&i))).flatten();
        self.push(name, detail, path.to_owned(), thumbnail, None, false);
    }

    /// Attach image bytes that have no file: decoded, written as `Image-<stamp>.png` in a new
    /// folder under `folder`, and owned, so removing the chip deletes the folder. Bytes that do
    /// not decode or cannot be written leave a failed chip saying why.
    pub fn add_image(&mut self, bytes: &[u8], folder: &Path, stamp: &str) {
        let name = format!("Image-{stamp}.png");
        let dir = folder.join(uuid::Uuid::new_v4().to_string().to_uppercase());
        let file = dir.join(&name);
        let written = (|| -> Result<(u64, Option<Vec<u8>>), String> {
            let decoded = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            decoded.save_with_format(&file, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            Ok((std::fs::metadata(&file).map(|m| m.len()).map_err(|e| e.to_string())?, thumbnail(&decoded)))
        })();
        match written {
            Ok((len, thumb)) => self.push(name, byte_count(len), file, thumb, None, true),
            Err(why) => {
                let _ = std::fs::remove_dir_all(&dir);
                self.push(name, String::new(), file, None, Some(format!("cannot save it: {why}")), false);
            }
        }
    }

    /// Take chip `id` out; a copy the tray wrote goes with its folder.
    pub fn remove(&mut self, id: u64) {
        let Some(at) = self.items.iter().position(|i| i.id == id) else { return };
        let item = self.items.remove(at);
        if item.owned && let Some(dir) = item.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Empty the tray once its paths went out; the files stay where they are for the agent.
    pub fn sent(&mut self) {
        self.items.clear();
    }
}

/// A chip's thumbnail: the image scaled to at most 96 px on its longer side, as PNG.
fn thumbnail(image: &image::DynamicImage) -> Option<Vec<u8>> {
    let mut png = vec![];
    image.thumbnail(96, 96).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

/// A size as Finder writes it (`ByteCountFormatter`'s file style, powers of 1000): bytes, whole
/// KB, then MB with one decimal and GB with two.
/// A size that rounds up to the next unit is written in that unit ("1.0 MB", never "1000 KB").
pub fn byte_count(n: u64) -> String {
    let (kb, mb) = ((n as f64 / 1e3).round(), (n as f64 / 1e6 * 10.).round() / 10.);
    match n {
        0 => "Zero KB".into(),
        1 => "1 byte".into(),
        n if n < 1000 => format!("{n} bytes"),
        _ if kb < 1000. => format!("{kb} KB"),
        _ if mb < 1000. => format!("{mb:.1} MB"),
        n => format!("{:.2} GB", n as f64 / 1e9),
    }
}

/// The local time as a pasted image's name carries it: `yyyyMMdd-HHmmss`.
pub fn stamp_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaching_a_message_quotes_it_above_the_draft() {
        assert_eq!(attach_as_context("", "one\n\ntwo"), "> one\n>\n> two\n\n");
        assert_eq!(attach_as_context("my words", "**yes**"), "> **yes**\n\nmy words");
    }

    fn commands(names: &[&str]) -> Vec<Command> {
        names.iter().map(|n| Command { name: (*n).into(), description: String::new() }).collect()
    }

    fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_prefix_opens_the_list_until_a_space_ends_the_name() {
        assert_eq!(query("/", '/'), Some(""));
        assert_eq!(query("/co", '/'), Some("co"));
        assert_eq!(query("/compact now", '/'), None);
        assert_eq!(query("hello", '/'), None);
        assert_eq!(prefix("claude"), Some('/'));
        assert_eq!(prefix("codex"), Some('$'));
        assert_eq!(prefix("pi"), None);
    }

    #[test]
    fn ranking_puts_a_prefix_first_then_containment_then_in_order_letters_then_descriptions() {
        let mut list = commands(&["review", "compact", "clear", "recompile", "cxt"]);
        list.push(Command { name: "zz".into(), description: "compact the context".into() });
        assert_eq!(names(&rank("", &list)), ["clear", "compact", "cxt", "recompile", "review", "zz"]);
        assert_eq!(names(&rank("comp", &list)), ["compact", "recompile", "zz"]);
        assert_eq!(names(&rank("ct", &list)), ["compact", "cxt", "zz"]);
        assert_eq!(names(&rank("CL", &list)), ["clear", "recompile"], "case is ignored, and c…l in order matches");
        assert!(rank("q", &list).is_empty());
    }

    #[test]
    fn claude_offers_the_commands_its_session_accepts_described_from_its_skills() {
        let skills = vec![SkillInfo { name: "review".into(), description: "Review the diff".into() }, SkillInfo { name: "deploy".into(), description: "ship".into() }];
        let said = vec!["review".to_owned(), "clear".to_owned(), "compact".to_owned()];
        let offered_claude = offered("claude", &skills, Some(&said));
        assert_eq!(names(&offered_claude), ["clear", "compact", "review"]);
        assert_eq!(offered_claude[2].description, "Review the diff");
        assert!(offered("claude", &skills, None).is_empty(), "nothing until the session says what it accepts");
        assert_eq!(names(&offered("codex", &skills, None)), ["review", "deploy"]);
    }

    #[test]
    fn arrows_choose_and_any_edit_starts_over() {
        let list = commands(&["clear", "compact", "review"]);
        let mut p = Picker::default();
        assert_eq!(p.matches("/", Some('/'), &list).len(), 3);
        assert!(!p.chosen());
        p.step(3, true);
        assert_eq!((p.highlighted(3), p.chosen()), (1, true));
        p.step(3, false);
        p.step(3, false);
        assert_eq!(p.highlighted(3), 2, "Up wraps to the last row");
        assert_eq!(p.highlighted(1), 0, "a shorter list keeps the highlight on a row");
        p.edited();
        assert_eq!((p.highlighted(3), p.chosen()), (0, false), "a typed letter, even one erased again, starts the highlight over");
        p.dismiss("/c");
        assert!(p.matches("/c", Some('/'), &list).is_empty());
        assert_eq!(p.matches("/co", Some('/'), &list).len(), 1, "a changed draft opens the list again");
        assert!(p.matches("/", None, &list).is_empty(), "a harness with no prefix opens no list");
        assert_eq!(accepted('/', &list[1]), "/compact ");
    }

    #[test]
    fn take_back_picks_the_unanswered_turn_or_the_last_queued_one_while_idle() {
        assert_eq!(take_back_target(Some(4), true, &[5, 6]), Some(4));
        assert_eq!(take_back_target(None, true, &[5, 6]), None, "a running turn that answered keeps its queue");
        assert_eq!(take_back_target(None, false, &[5, 6]), Some(6));
        assert_eq!(take_back_target(None, false, &[]), None);
        assert_eq!(restored("", "fix it"), "fix it");
        assert_eq!(restored("also", "fix it"), "also\nfix it");
    }

    #[test]
    fn a_message_puts_each_path_in_backticks_on_its_own_line_after_the_text() {
        let paths = vec!["/tmp/a b.txt".to_owned(), "/tmp/dir".to_owned()];
        assert_eq!(message("  look at these \n", &paths), "look at these\n\n`/tmp/a b.txt`\n`/tmp/dir`");
        assert_eq!(message("", &paths), "`/tmp/a b.txt`\n`/tmp/dir`", "attachments alone make a message");
        assert_eq!(message(" hi ", &[]), "hi");
        assert_eq!(message("  ", &[]), "");
    }

    #[test]
    fn the_tray_attaches_files_where_they_are_and_owns_only_pasted_images() {
        let root = std::env::temp_dir().join(format!("wheel-tray-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("dir")).unwrap();
        std::fs::write(root.join("notes.txt"), vec![b'x'; 1500]).unwrap();
        let mut tray = Tray::default();
        tray.add_file(&root.join("notes.txt"));
        tray.add_file(&root.join("dir"));
        assert_eq!(tray.items().iter().map(|i| (i.name.as_str(), i.detail.as_str(), i.thumbnail.is_some())).collect::<Vec<_>>(), [("notes.txt", "2 KB", false), ("dir", "Folder", false)]);

        let mut png = vec![];
        image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 30, 30, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let pasted = root.join("pasted");
        tray.add_image(&png, &pasted, "20261112-073000");
        let shot = tray.items()[2].clone();
        assert_eq!((shot.name.as_str(), shot.thumbnail.is_some(), shot.failed.is_none()), ("Image-20261112-073000.png", true, true));
        std::fs::write(root.join("broken.png"), b"not a png").unwrap();
        tray.add_file(&root.join("broken.png"));
        assert!(tray.items()[3].thumbnail.is_none(), "an image file that does not decode draws the document symbol");
        tray.remove(tray.items()[3].id);
        assert!(shot.path.starts_with(&pasted) && shot.path.exists());
        assert_eq!(image::open(&shot.path).unwrap().width(), 4);
        assert!(tray.ready());

        tray.add_image(b"not an image", &pasted, "20261112-073001");
        assert!(tray.items()[3].failed.as_deref().is_some_and(|w| w.starts_with("cannot save it")));
        assert!(!tray.ready(), "a failed chip holds the message back");
        tray.remove(tray.items()[3].id);
        assert!(tray.ready());

        tray.remove(shot.id);
        assert!(!shot.path.parent().unwrap().exists(), "removing a pasted image deletes its folder");
        tray.remove(tray.items()[0].id);
        assert!(root.join("notes.txt").exists(), "removing a file's chip leaves the file");
        assert_eq!(tray.paths(), [root.join("dir").to_string_lossy().into_owned()]);
        tray.sent();
        assert!(tray.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sizes_read_as_finder_writes_them() {
        assert_eq!(byte_count(0), "Zero KB");
        assert_eq!(byte_count(1), "1 byte");
        assert_eq!(byte_count(512), "512 bytes");
        assert_eq!(byte_count(12_345), "12 KB");
        assert_eq!(byte_count(1_234_567), "1.2 MB");
        assert_eq!(byte_count(999_500), "1.0 MB", "a size that rounds to 1000 KB is written in MB");
        assert_eq!(byte_count(999_400), "999 KB");
        assert_eq!(byte_count(2_500_000_000), "2.50 GB");
        assert_eq!(stamp_now().len(), 15);
    }
}
