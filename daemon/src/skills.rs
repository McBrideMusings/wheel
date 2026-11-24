//! The skills an agent has: one folder per skill under `<home>/skills`, each holding a `SKILL.md`
//! whose frontmatter names it and says in a line what it does. Clients offer them when a person
//! types the agent's skill prefix in a composer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use notify::{RecursiveMode, Watcher};
use wheel_proto::SkillInfo;

/// Each home's last scan.
static KEPT: OnceLock<Mutex<HashMap<PathBuf, Kept>>> = OnceLock::new();

/// One home's last scan, kept until a file under a folder it read changes.
struct Kept {
    projects: Vec<PathBuf>,
    skills: Vec<SkillInfo>,
    fresh: Arc<AtomicBool>,
    _watch: Option<crate::fswatch::Handle>,
}

/// [`scan`], kept per home: every status build asks for every agent's skills, and the folders
/// rarely change between two of them. A file event under a folder the scan read, or under the
/// nearest folder that exists above one it looked for, drops the kept scan.
pub(crate) fn scan_kept(home: &Path, projects: &[PathBuf]) -> Vec<SkillInfo> {
    let kept = KEPT.get_or_init(Default::default);
    if let Some(k) = kept.lock().unwrap().get(home) {
        if k.projects == projects && k.fresh.load(Ordering::SeqCst) {
            return k.skills.clone();
        }
    }
    // Watched before the scan, so a change made while it reads drops the result.
    let fresh = Arc::new(AtomicBool::new(true));
    let watch = watch_read(home, projects, fresh.clone());
    let skills = scan(home, projects);
    // Kept only while watched: with no watch, nothing would ever drop it.
    if watch.is_some() {
        kept.lock().unwrap().insert(
            home.to_path_buf(),
            Kept {
                projects: projects.to_vec(),
                skills: skills.clone(),
                fresh,
                _watch: watch,
            },
        );
    }
    skills
}

/// A watch on every folder [`scan`] reads for `home` and `projects`, clearing `fresh` on any change.
fn watch_read(home: &Path, projects: &[PathBuf], fresh: Arc<AtomicBool>) -> Option<crate::fswatch::Handle> {
    let mut dirs = vec![home.join("skills"), home.join("plugins")];
    dirs.extend(projects.iter().map(|p| p.join(".claude").join("skills")));
    dirs.extend(plugin_skill_dirs(home, projects).into_iter().map(|(_, d)| d));
    let mut watch = crate::fswatch::watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
            fresh.store(false, Ordering::SeqCst);
        }
    })
    .ok()?;
    for dir in dirs {
        // A folder not made yet is seen being made from the nearest one that exists above it.
        let result = match dir.is_dir() {
            true => watch.watch(&dir, RecursiveMode::Recursive),
            false => match dir.ancestors().skip(1).find(|a| a.is_dir()) {
                Some(above) => watch.watch(above, RecursiveMode::NonRecursive),
                None => Ok(()),
            },
        };
        if let Err(e) = result {
            tracing::warn!(dir = %dir.display(), error = %e, "cannot watch a skills folder; its skills are read again on every status");
            return None;
        }
    }
    Some(watch)
}

/// The skills an agent offers, sorted by name: those under `<home>/skills`, each `.claude/skills`
/// folder of the `projects` directories, and, for the plugins `<home>/plugins/installed_plugins.json`
/// lists, each plugin's `skills` folder, named `<plugin>:<skill>` as Claude Code runs them. A folder
/// without a readable `SKILL.md` is not a skill. Codex keeps its bundled skills under
/// `skills/.system`, so that one hidden folder is searched too; other hidden folders are not. A
/// home skill wins over a project skill of the same name.
pub(crate) fn scan(home: &Path, projects: &[PathBuf]) -> Vec<SkillInfo> {
    let root = home.join("skills");
    let mut found = Vec::new();
    collect(&root, "", &mut found);
    collect(&root.join(".system"), "", &mut found);
    let from_home = found.len();
    for dir in projects {
        collect(&dir.join(".claude").join("skills"), "", &mut found);
    }
    let from_projects = found.len() - from_home;
    let plugins = plugin_skill_dirs(home, projects);
    let plugin_count = plugins.len();
    let before = found.len();
    for (plugin, dir) in plugins {
        collect(&dir, &format!("{plugin}:"), &mut found);
    }
    let from_plugins = found.len() - before;
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    tracing::debug!(
        home = %home.display(),
        home_skills = from_home,
        project_dirs = projects.len(),
        project_skills = from_projects,
        plugins = plugin_count,
        plugin_skills = from_plugins,
        total = found.len(),
        "skills scanned"
    );
    found
}

/// `(plugin name, skills folder)` for each installed plugin that applies: a user-scope install,
/// or a project-scope one whose `projectPath` is one of `projects`.
fn plugin_skill_dirs(home: &Path, projects: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(home.join("plugins").join("installed_plugins.json"))
    else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        tracing::warn!(home = %home.display(), "installed_plugins.json is not JSON; plugin skills not listed");
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    for (key, installs) in json["plugins"].as_object().into_iter().flatten() {
        let plugin = key.split('@').next().unwrap_or(key).to_string();
        for install in installs.as_array().into_iter().flatten() {
            let applies = match install["scope"].as_str() {
                Some("user") => true,
                Some("project") => install["projectPath"]
                    .as_str()
                    .is_some_and(|p| projects.iter().any(|d| d == Path::new(p))),
                _ => false,
            };
            let Some(path) = install["installPath"].as_str().filter(|_| applies) else {
                continue;
            };
            let skills = Path::new(path).join("skills");
            if skills.is_dir() && !dirs.iter().any(|(_, d)| *d == skills) {
                dirs.push((plugin.clone(), skills));
            }
        }
    }
    dirs
}

/// The slash commands Claude Code runs itself in a pane, with what each does. The agent's skill
/// scan cannot find them, and a composer filters this list to the ones an agent lists as accepted.
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

fn collect(dir: &Path, prefix: &str, into: &mut Vec<SkillInfo>) {
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
            name: format!("{prefix}{}", name.unwrap_or(folder)),
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
    fn a_kept_scan_is_reused_until_a_skill_or_a_new_skills_folder_appears() {
        let home = crate::test_support::scratch().canonicalize().unwrap();
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let write = |dir: PathBuf, name: &str| {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\n")).unwrap();
        };
        write(home.join("skills/a"), "a");
        let projects = [project.clone()];
        let names = |s: Vec<SkillInfo>| s.into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(scan_kept(&home, &projects)), ["a"]);
        // Folder events from the setup may still arrive; once quiet, the scan is kept.
        let kept_now = || {
            KEPT.get().and_then(|k| k.lock().unwrap().get(&home).map(|k| k.fresh.load(Ordering::SeqCst)))
        };
        let quiet = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < quiet {
            scan_kept(&home, &projects);
            std::thread::sleep(std::time::Duration::from_millis(300));
            if kept_now() == Some(true) {
                break;
            }
        }
        assert_eq!(kept_now(), Some(true), "the scan was never kept");

        // A new home skill, then a project's first .claude/skills folder, each drop the kept scan.
        for (dir, want) in [
            (home.join("skills/b"), vec!["a", "b"]),
            (project.join(".claude/skills/p"), vec!["a", "b", "p"]),
        ] {
            let name = dir.file_name().unwrap().to_str().unwrap().to_string();
            write(dir, &name);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while kept_now() == Some(true) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(names(scan_kept(&home, &projects)), want);
        }
    }

    #[test]
    fn scan_lists_skill_folders_and_the_system_folder() {
        let home = crate::test_support::scratch();
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
        let names: Vec<_> = scan(&home, &[]).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b", "s"]);
    }

    #[test]
    fn scan_adds_project_skills_and_plugin_skills_named_by_plugin() {
        let root = crate::test_support::scratch();
        let home = root.join("home");
        let project = root.join("proj");
        let plugin = root.join("cache/canvas");
        for (dir, text) in [
            ("home/skills/a", "---\nname: a\ndescription: Ay\n---\n"),
            (
                "proj/.claude/skills/p",
                "---\nname: p\ndescription: Pee\n---\n",
            ),
            ("cache/canvas/skills/post", "---\nname: post\n---\n"),
        ] {
            let d = root.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), text).unwrap();
        }
        std::fs::create_dir_all(home.join("plugins")).unwrap();
        let installed = format!(
            r#"{{"plugins":{{"canvas@m":[{{"scope":"user","installPath":"{}"}}],"other@m":[{{"scope":"project","projectPath":"/elsewhere","installPath":"{}"}}]}}}}"#,
            plugin.display(),
            plugin.display()
        );
        std::fs::write(home.join("plugins/installed_plugins.json"), installed).unwrap();
        let names: Vec<_> = scan(&home, &[project])
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["a", "canvas:post", "p"]);
    }
}
