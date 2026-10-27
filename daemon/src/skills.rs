//! The skills an agent has: one folder per skill under `<home>/skills`, each holding a `SKILL.md`
//! whose frontmatter names it and says in a line what it does. Clients offer them when a person
//! types the agent's skill prefix in a composer.

use std::path::Path;

use wheel_proto::SkillInfo;

/// The skills under `home`, sorted by name. A folder without a readable `SKILL.md` is not a
/// skill. Codex keeps its bundled skills under `skills/.system`, so that one hidden folder is
/// searched too; other hidden folders are not.
pub(crate) fn scan(home: &Path) -> Vec<SkillInfo> {
    let root = home.join("skills");
    let mut found = Vec::new();
    collect(&root, &mut found);
    collect(&root.join(".system"), &mut found);
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    found
}

/// The slash commands Claude Code runs itself in a pane, with what each does. The agent's skill
/// scan cannot find them, and a composer filters this list to the ones a session lists as accepted.
/// A skill of the same name wins.
pub(crate) fn claude_builtins() -> Vec<SkillInfo> {
    const COMMANDS: &[(&str, &str)] = &[
        (
            "autocompact",
            "Show or set the context size at which the conversation is summarized",
        ),
        ("clear", "Start a new conversation and empty this one"),
        ("color", "Set the session's color"),
        ("compact", "Summarize the conversation to free up context"),
        ("config", "Change a Claude Code setting"),
        ("context", "Show how the context window is used"),
        ("effort", "Set how hard the model thinks"),
        ("mcp", "Show the MCP servers and whether they are connected"),
        ("model", "Show or change the model"),
        ("output-style", "Show or change the output style"),
        ("recap", "Summarize what this session has done so far"),
        ("reload-skills", "Reread the skills on disk"),
        ("rename", "Rename the session"),
        ("usage", "Show plan usage and cost"),
    ];
    COMMANDS
        .iter()
        .map(|(name, description)| SkillInfo {
            name: (*name).into(),
            description: (*description).into(),
        })
        .collect()
}

fn collect(dir: &Path, into: &mut Vec<SkillInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let folder = entry.file_name().to_string_lossy().into_owned();
        if folder.starts_with('.') {
            continue;
        }
        // `is_dir` follows symlinks: a linked skill folder is a skill.
        if !entry.path().is_dir() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("SKILL.md")) else {
            continue;
        };
        let (name, description) = frontmatter(&text);
        into.push(SkillInfo {
            name: name.unwrap_or(folder),
            description,
        });
    }
}

/// The `name` and `description` of a `SKILL.md`'s `---` block. A value may be quoted, or a `>` or
/// `|` block whose indented lines follow.
fn frontmatter(text: &str) -> (Option<String>, String) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, String::new());
    }
    let mut name = None;
    let mut description = String::new();
    let mut in_description = false;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if in_description && (line.starts_with(' ') || line.starts_with('\t')) {
            description.push(' ');
            description.push_str(line.trim());
            continue;
        }
        in_description = false;
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(unquote(v)).filter(|n| !n.is_empty());
        } else if let Some(v) = line.strip_prefix("description:") {
            let v = v.trim();
            if matches!(v, ">" | "|" | ">-" | "|-") {
                in_description = true;
            } else {
                description = unquote(v);
            }
        }
    }
    (name, description.trim().to_string())
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner.to_string();
        }
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_reads_plain_quoted_and_block_values() {
        let (n, d) = frontmatter("---\nname: review\ndescription: Review a diff\n---\nbody");
        assert_eq!(
            (n.as_deref(), d.as_str()),
            (Some("review"), "Review a diff")
        );
        let (n, d) = frontmatter("---\nname: \"x\"\ndescription: >\n  one\n  two\nother: y\n---\n");
        assert_eq!((n.as_deref(), d.as_str()), (Some("x"), "one two"));
        assert_eq!(frontmatter("no block"), (None, String::new()));
    }

    #[test]
    fn scan_lists_skill_folders_and_the_system_folder() {
        let home = std::env::temp_dir().join(format!("wheel-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for (dir, text) in [
            ("skills/b", "---\nname: b\ndescription: Bee\n---\n"),
            ("skills/a", "---\ndescription: Ay\n---\n"),
            ("skills/.system/s", "---\nname: s\ndescription: Sys\n---\n"),
            ("skills/.hidden/h", "---\nname: h\n---\n"),
        ] {
            let d = home.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), text).unwrap();
        }
        std::fs::create_dir_all(home.join("skills/empty")).unwrap();
        let names: Vec<_> = scan(&home).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b", "s"]);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
