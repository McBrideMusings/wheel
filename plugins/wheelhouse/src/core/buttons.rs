//! The buttons wheelhouse's views draw, read from two `wheel.toml` files (ADR-0044): this machine's
//! (`$WHEEL_MACHINE_FILE`, which wheeld sets to its own, else `~/.wheel/wheel.toml`) and then the
//! project's. A project table with the same name replaces the machine's, and one holding
//! `enabled = false` drops it. Every `run` runs in the project's directory, wherever it was written.
//!
//! This module also reads the list buttons of a view: `[<view>.list.<name>]` with a `label` and the
//! shell it runs over the view's whole narrowed list, or the rows the person selected.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

/// One `wheel.toml` that was read.
pub struct File {
    pub path: PathBuf,
    pub doc: DocumentMut,
}

/// The machine's `wheel.toml`: `$WHEEL_MACHINE_FILE`, else `~/.wheel/wheel.toml`.
pub fn machine_file() -> PathBuf {
    match std::env::var_os("WHEEL_MACHINE_FILE") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".wheel").join("wheel.toml"),
    }
}

/// The machine's `wheel.toml` and then `<dir>/wheel.toml`, each that exists and parses, and a line
/// for each that exists and cannot be read or does not parse.
pub fn files(dir: &str) -> (Vec<File>, Vec<String>) {
    files_from(&machine_file(), dir)
}

/// [`files`] with the machine's file at `machine`.
pub fn files_from(machine: &Path, dir: &str) -> (Vec<File>, Vec<String>) {
    let (mut out, mut problems) = (vec![], vec![]);
    for path in [machine.to_path_buf(), PathBuf::from(dir).join("wheel.toml")] {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                problems.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        match text.parse::<DocumentMut>() {
            Ok(doc) => out.push(File { path, doc }),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    (out, problems)
}

fn at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let (first, rest) = path.split_first()?;
    rest.iter().try_fold(doc.get(first)?, |item, k| item.get(k))
}

fn enabled(item: &Item) -> bool {
    item.get("enabled").and_then(|v| v.as_bool()) != Some(false)
}

/// The entries of the table at `path` across `files`, machine first: a later file's entry replaces
/// an earlier one of the same name in its place, and one holding `enabled = false` drops it.
pub fn named<'a>(files: &'a [File], path: &[&str]) -> Vec<(String, &'a Item)> {
    let mut out: Vec<(String, &Item)> = vec![];
    for f in files {
        let Some(t) = at(&f.doc, path).and_then(|t| t.as_table_like()) else { continue };
        for (name, item) in t.iter() {
            let i = out.iter().position(|(n, _)| n == name);
            match (i, enabled(item)) {
                (Some(i), true) => out[i].1 = item,
                (Some(i), false) => {
                    out.remove(i);
                }
                (None, true) => out.push((name.to_string(), item)),
                (None, false) => {}
            }
        }
    }
    out
}

/// The table at `path` from the last of `files` that writes one; none when that one holds
/// `enabled = false`.
pub fn table<'a>(files: &'a [File], path: &[&str]) -> Option<&'a Item> {
    files.iter().filter_map(|f| at(&f.doc, path)).last().filter(|t| enabled(t))
}

/// An absent declaration preserves the legacy command under Default. An empty list
/// explicitly marks a provider-independent action such as opening a space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Providers(Option<Vec<String>>);

impl Providers {
    pub fn read(item: &Item) -> Self {
        Self(item.get("providers").and_then(Item::as_array).map(|values| {
            values.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect()
        }))
    }

    pub fn selected<'a>(&self, label: &str, provider: Option<&'a str>) -> Result<Option<&'a str>, String> {
        let Some(p) = provider else { return Ok(None) };
        match &self.0 {
            Some(names) if names.is_empty() => Ok(None),
            Some(names) if names.iter().any(|name| name == p) => Ok(Some(p)),
            _ => Err(format!("{label} does not declare support for {p} in wheel.toml")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub name: String,
    pub label: Option<String>,
    pub run: String,
    pub providers: Providers,
}

impl Script {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// The list buttons under `[<table>.list]` for the project in `path`, and any problem reading them.
pub fn scripts(path: &str, table: &str) -> (Vec<Script>, Vec<String>) {
    let (files, problems) = files(path);
    (scripts_of(&files, table), problems)
}

/// The list buttons under `[<table>.list]` in `files`.
pub fn scripts_of(files: &[File], table: &str) -> Vec<Script> {
    named(files, &[table, "list"])
        .into_iter()
        .map(|(name, s)| Script {
            name,
            providers: Providers::read(s),
            label: s.get("label").and_then(|v| v.as_str()).map(String::from),
            run: s.get("run").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })
        .collect()
}

/// Run the list button `name` of `scripts` over `ids`: the shell runs in `dir` with `var` (BEAD_IDS,
/// ISSUE_NUMBERS) holding the ids space separated, plus `env`, for the pane `pane`. The words for
/// the page on success; an error for a button neither file wrote, an empty list or a failed run.
pub fn run_list(scripts: &[Script], name: &str, ids: &[String], var: &str, env: &[(&str, &str)], dir: &str, pane: &str, what: &str, provider: Option<&str>) -> Result<String, String> {
    let Some(sc) = scripts.iter().find(|x| x.name == name) else {
        return Err(format!("no list button {name}"));
    };
    if ids.is_empty() {
        return Err("the list is empty".into());
    }
    let joined = ids.join(" ");
    let mut all = vec![(var, joined.as_str())];
    all.extend_from_slice(env);
    if let Some(p) = sc.providers.selected(sc.label(), provider)? { all.push(("WHEEL_ACTION_PROVIDER", p)); }
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let log = std::env::temp_dir().join(format!("wheel-{what}-list-{safe}.log"));
    crate::core::run::button(sc.label(), &sc.run, dir, pane, &all, &log)?;
    Ok(format!("{} started over {} {what}. See the new pane.", sc.label(), ids.len()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn explicit_choices_require_support_and_neutral_commands_keep_their_environment() {
        let doc: DocumentMut = "[review]\nproviders=['claude','codex']\n[fixed]\nproviders=['claude']\n[open]\nproviders=[]\n[legacy]\nrun='true'".parse().unwrap();
        for name in ["review", "fixed", "open", "legacy"] {
            let supported = Providers::read(&doc[name]);
            assert_eq!(supported.selected(name, None), Ok(None));
            match name {
                "review" => assert_eq!(supported.selected(name, Some("codex")), Ok(Some("codex"))),
                "open" => assert_eq!(supported.selected(name, Some("codex")), Ok(None)),
                _ => assert!(supported.selected(name, Some("codex")).is_err()),
            }
        }
    }

    #[test]
    fn a_list_button_runs_with_the_ids_and_refuses_an_unknown_one_or_an_empty_list() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-run-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("env.txt");
        let scripts = vec![
            Script { name: "go".into(), providers: Providers::default(), label: Some("Go".into()), run: format!("printf '%s|%s|%s' \"$BEAD_IDS\" \"$BEAD_REPO\" \"$WHEEL_PANE\" > {}", out.display()) },
        ];
        let ids = vec!["t-2".to_string(), "t-1".to_string()];
        let d = dir.to_str().unwrap();
        assert_eq!(run_list(&scripts, "go", &ids, "BEAD_IDS", &[("BEAD_REPO", "t")], d, "p7", "beads", None).unwrap(), "Go started over 2 beads. See the new pane.");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "t-2 t-1|t|p7", "the ids reach the shell in the order given");
        assert_eq!(run_list(&scripts, "nope", &ids, "BEAD_IDS", &[], d, "p7", "beads", None), Err("no list button nope".into()));
        assert_eq!(run_list(&scripts, "go", &[], "BEAD_IDS", &[], d, "p7", "beads", None), Err("the list is empty".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A machine file and a project directory under a fresh temporary root.
    pub(crate) fn fixture(tag: &str, machine: &str, project: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("wheelhouse-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let machine_file = root.join("machine.toml");
        if !machine.is_empty() {
            std::fs::write(&machine_file, machine).unwrap();
        }
        if !project.is_empty() {
            std::fs::write(dir.join("wheel.toml"), project).unwrap();
        }
        (root, machine_file, dir)
    }

    #[test]
    fn each_view_reads_its_own_list_table_and_nothing_under_buttons() {
        let (root, machine, dir) = fixture("buttons", "", "[beads.list.go]\nlabel = \"Go\"\nrun = \"true\"\n[beads.buttons.old]\nrun = \"true\"\n[issues.list.all]\nrun = \"true\"\n");
        let (files, _) = files_from(&machine, dir.to_str().unwrap());
        let (b, i) = (scripts_of(&files, "beads"), scripts_of(&files, "issues"));
        assert_eq!(b.iter().map(|s| s.label()).collect::<Vec<_>>(), ["Go"], "only [beads.list.*] is a list button; [beads.buttons.*] is the per-bead reader's to flag");
        assert_eq!(i.iter().map(|s| s.label()).collect::<Vec<_>>(), ["all"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_machine_button_applies_until_the_project_replaces_or_disables_it() {
        let machine = "[repo.list.review]\nlabel = \"Review\"\nrun = \"machine-review\"\n[repo.list.tidy]\nrun = \"machine-tidy\"\n[repo.list.gone]\nrun = \"machine-gone\"\n";
        let project = "[repo.list.tidy]\nlabel = \"Tidy here\"\nrun = \"project-tidy\"\n[repo.list.gone]\nenabled = false\n[repo.list.mine]\nrun = \"project-mine\"\n";
        let (root, machine, dir) = fixture("layers", machine, project);
        let (files, problems) = files_from(&machine, dir.to_str().unwrap());
        assert!(problems.is_empty());
        let got: Vec<(String, String)> = scripts_of(&files, "repo").iter().map(|s| (s.label().to_string(), s.run.clone())).collect();
        assert_eq!(got, [("Review".into(), "machine-review".into()), ("Tidy here".into(), "project-tidy".into()), ("mine".into(), "project-mine".into())],
            "the machine's review stays, the project's tidy replaces the machine's in its place, gone is dropped, mine is added");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_machine_file_that_does_not_parse_is_a_problem_and_the_project_still_reads() {
        let (root, machine, dir) = fixture("badmachine", "[repo.list.x\n", "[repo.list.mine]\nrun = \"true\"\n");
        let (files, problems) = files_from(&machine, dir.to_str().unwrap());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with(&format!("{}: ", machine.display())), "{}", problems[0]);
        assert_eq!(scripts_of(&files, "repo").iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["mine"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_table_is_the_last_files_and_enabled_false_takes_it_away() {
        let (root, machine, dir) = fixture("table", "[beads.task]\nstart.run = \"m\"\n[beads.epic]\nstart.run = \"m\"\n", "[beads.epic]\nenabled = false\n");
        let (files, _) = files_from(&machine, dir.to_str().unwrap());
        assert_eq!(table(&files, &["beads", "task"]).and_then(|t| t.get("start")).and_then(|s| s.get("run")).and_then(|v| v.as_str()), Some("m"));
        assert!(table(&files, &["beads", "epic"]).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
