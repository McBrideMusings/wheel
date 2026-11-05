//! The project's tracker (beads, through `bd`) as the Issues and Project views read and write it,
//! and the project's vision, the first paragraph of `docs/CONTEXT.md`.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use super::run;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub status: String,
    pub kind: String,
    pub priority: Option<i64>,
    pub parent: Option<String>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub description: String,
    pub updated: String,
    /// `bd ready` lists it.
    pub ready: bool,
    /// Another item that is not closed blocks it.
    pub blocked: bool,
}

pub const STATUSES: [&str; 4] = ["open", "in_progress", "blocked", "deferred"];
pub const KINDS: [&str; 5] = ["task", "bug", "feature", "chore", "epic"];

/// Whether `dir` keeps a tracker.
pub fn has(dir: &str) -> bool {
    PathBuf::from(dir).join(".beads").is_dir()
}

/// Every item of the tracker in `dir`, closed ones included.
pub fn list(dir: &str) -> Result<Vec<Item>, String> {
    let all = run::bd(dir, &["list", "--all"])?;
    let ready: HashSet<String> = run::bd(dir, &["ready"])?.iter().filter_map(|b| b["id"].as_str().map(String::from)).collect();
    let closed: HashSet<&str> = all.iter().filter(|b| b["status"] == "closed").filter_map(|b| b["id"].as_str()).collect();
    Ok(all.iter().map(|b| item(b, &ready, &closed)).collect())
}

fn item(b: &Value, ready: &HashSet<String>, closed: &HashSet<&str>) -> Item {
    let s = |k: &str| b[k].as_str().unwrap_or("").to_string();
    let id = s("id");
    let blocked = b["dependencies"].as_array().map_or(false, |d| {
        d.iter().filter(|d| d["type"] == "blocks").filter_map(|d| d["depends_on_id"].as_str()).any(|d| !closed.contains(d))
    });
    Item {
        ready: ready.contains(&id),
        blocked,
        title: s("title"),
        status: if b["status"].is_null() { "open".into() } else { s("status") },
        kind: if b["issue_type"].is_null() { "task".into() } else { s("issue_type") },
        priority: b["priority"].as_i64(),
        parent: b["parent"].as_str().map(String::from),
        labels: b["labels"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default(),
        assignee: b["assignee"].as_str().filter(|a| !a.is_empty()).map(String::from),
        description: s("description"),
        updated: s("updated_at"),
        id,
    }
}

fn id_ok(id: &str) -> Result<(), String> {
    let ok = !id.is_empty() && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok { Ok(()) } else { Err(format!("not an item id: {id:?}")) }
}

/// Create an item and return its id.
pub fn create(dir: &str, title: &str, kind: &str, priority: i64, description: &str) -> Result<String, String> {
    if title.trim().is_empty() {
        return Err("a title is required".into());
    }
    if !KINDS.contains(&kind) {
        return Err(format!("type must be one of {}", KINDS.join(", ")));
    }
    let p = priority.clamp(0, 4).to_string();
    let mut args = vec!["create", title.trim(), "-t", kind, "-p", &p, "--json"];
    if !description.trim().is_empty() {
        args.extend(["-d", description.trim()]);
    }
    let made = run::json("bd", dir, &args)?;
    // bd answers one issue, or a one-element list.
    let made = if made.is_array() { made[0].clone() } else { made };
    made["id"].as_str().map(String::from).ok_or_else(|| "bd create returned no id".to_string())
}

/// Set an item's status (open, in_progress, blocked, deferred), priority or title.
pub fn update(dir: &str, id: &str, status: Option<&str>, priority: Option<i64>, title: Option<&str>) -> Result<(), String> {
    id_ok(id)?;
    let mut args: Vec<String> = vec!["update".into(), id.into()];
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(format!("status must be one of {}", STATUSES.join(", ")));
        }
        args.extend(["-s".into(), s.into()]);
    }
    if let Some(p) = priority {
        args.extend(["-p".into(), p.clamp(0, 4).to_string()]);
    }
    if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
        args.extend(["--title".into(), t.trim().into()]);
    }
    if args.len() == 2 {
        return Err("nothing to update".into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run::text("bd", dir, &refs).map(|_| ())
}

/// Close an item.
pub fn close(dir: &str, id: &str, reason: &str) -> Result<(), String> {
    id_ok(id)?;
    let mut args = vec!["close", id];
    if !reason.trim().is_empty() {
        args.extend(["-r", reason.trim()]);
    }
    run::text("bd", dir, &args).map(|_| ())
}

/// The vision: the first paragraph of `docs/CONTEXT.md` after its title.
pub fn vision(dir: &str) -> Option<String> {
    let text = std::fs::read_to_string(PathBuf::from(dir).join("docs/CONTEXT.md")).ok()?;
    let (_, para, _) = split_vision(&text)?;
    Some(para)
}

/// Replace the vision paragraph in `docs/CONTEXT.md`, or add one under a new title.
pub fn set_vision(dir: &str, text: &str) -> Result<(), String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("the vision cannot be empty".into());
    }
    let path = PathBuf::from(dir).join("docs/CONTEXT.md");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let new = match split_vision(&old) {
        Some((head, _, tail)) => format!("{head}{text}\n{tail}"),
        None if old.trim().is_empty() => format!("# {}\n\n{text}\n", super::project::name_of(dir)),
        None => format!("{}\n\n{text}\n", old.trim_end()),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))
}

/// `(text before the paragraph, the paragraph on one line, text after it)`.
fn split_vision(text: &str) -> Option<(String, String, String)> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let heading = |l: &str| l.trim().starts_with('#') || l.trim().starts_with("---");
    let start = lines.iter().position(|l| !l.trim().is_empty() && !heading(l))?;
    let end = lines[start..].iter().position(|l| l.trim().is_empty() || heading(l)).map_or(lines.len(), |n| start + n);
    let para = lines[start..end].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    Some((lines[..start].concat(), para, lines[end..].concat()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vision_is_the_first_paragraph_after_the_title() {
        let (head, para, tail) = split_vision("# T\n\nOne\ntwo.\n\n## Next\nx\n").unwrap();
        assert_eq!((head.as_str(), para.as_str(), tail.as_str()), ("# T\n\n", "One two.", "\n## Next\nx\n"));
        assert!(split_vision("# T\n\n").is_none());
    }

    #[test]
    fn setting_the_vision_keeps_the_rest_of_the_file() {
        let d = std::env::temp_dir().join(format!("wh-vision-{}", std::process::id()));
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::write(d.join("docs/CONTEXT.md"), "# T\n\nOld words\nhere.\n\n## Terms\nx\n").unwrap();
        let dir = d.to_str().unwrap();
        set_vision(dir, "New   vision").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("docs/CONTEXT.md")).unwrap(), "# T\n\nNew vision\n\n## Terms\nx\n");
        assert_eq!(vision(dir).as_deref(), Some("New vision"));
        assert!(set_vision(dir, "  ").is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_item_id_is_plain() {
        assert!(id_ok("wheel-cmbu.4").is_ok());
        assert!(id_ok("-rf").is_err());
        assert!(id_ok("a b").is_err());
    }
}
