//! The buttons a project writes in its `wheel.toml` for a view: `[<view>.buttons.<name>]` with a
//! `label`, the rows it shows `on` (the view decides what each word means; `list` is the view's
//! whole narrowed list) and the shell it runs. Wheelhouse only reads and runs them (ADR-0025).

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub name: String,
    pub label: Option<String>,
    pub on: Vec<String>,
    pub run: String,
}

impl Script {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Whether the button acts on the view's whole narrowed list rather than one row.
    pub fn on_list(&self) -> bool {
        self.on.iter().any(|k| k == "list")
    }
}

/// The buttons under `[<table>.buttons]` in `<path>/wheel.toml`, and any problem reading them.
pub fn scripts(path: &str, table: &str) -> (Vec<Script>, Vec<String>) {
    let file = PathBuf::from(path).join("wheel.toml");
    let Ok(text) = std::fs::read_to_string(&file) else { return (vec![], vec![]) };
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(t) => t,
        Err(e) => return (vec![], vec![format!("{}: {e}", file.display())]),
    };
    let mut out = vec![];
    if let Some(buttons) = doc.get(table).and_then(|b| b.get("buttons")).and_then(|b| b.as_table_like()) {
        for (name, s) in buttons.iter() {
            out.push(Script {
                name: name.to_string(),
                label: s.get("label").and_then(|v| v.as_str()).map(String::from),
                on: s
                    .get("on")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                run: s.get("run").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            });
        }
    }
    (out, vec![])
}

/// Run the list button `name` of `scripts` over `ids`: the shell runs in `dir` with `var` (BEAD_IDS,
/// ISSUE_NUMBERS) holding the ids space separated, plus `env`, for the pane `pane`. The words for
/// the page on success; an error for a button that is not a list button, an empty list or a failed run.
pub fn run_list(scripts: &[Script], name: &str, ids: &[String], var: &str, env: &[(&str, &str)], dir: &str, pane: &str, what: &str) -> Result<String, String> {
    let Some(sc) = scripts.iter().find(|x| x.name == name && x.on_list()) else {
        return Err(format!("no list button {name}"));
    };
    if ids.is_empty() {
        return Err("the list is empty".into());
    }
    let joined = ids.join(" ");
    let mut all = vec![(var, joined.as_str())];
    all.extend_from_slice(env);
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let log = std::env::temp_dir().join(format!("wheel-{what}-list-{safe}.log"));
    crate::core::run::button(sc.label(), &sc.run, dir, pane, &all, &log)?;
    Ok(format!("{} started over {} {what}. See the new pane.", sc.label(), ids.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_button_runs_with_the_ids_and_refuses_a_row_button_an_unknown_one_or_an_empty_list() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-run-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("env.txt");
        let scripts = vec![
            Script { name: "go".into(), label: Some("Go".into()), on: vec!["list".into()], run: format!("printf '%s|%s|%s' \"$BEAD_IDS\" \"$BEAD_REPO\" \"$WHEEL_PANE\" > {}", out.display()) },
            Script { name: "one".into(), label: None, on: vec!["ready".into()], run: "true".into() },
        ];
        let ids = vec!["t-2".to_string(), "t-1".to_string()];
        let d = dir.to_str().unwrap();
        assert_eq!(run_list(&scripts, "go", &ids, "BEAD_IDS", &[("BEAD_REPO", "t")], d, "p7", "beads").unwrap(), "Go started over 2 beads. See the new pane.");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "t-2 t-1|t|p7", "the ids reach the shell in the order given");
        assert_eq!(run_list(&scripts, "one", &ids, "BEAD_IDS", &[], d, "p7", "beads"), Err("no list button one".into()), "a row button is not a list button");
        assert_eq!(run_list(&scripts, "nope", &ids, "BEAD_IDS", &[], d, "p7", "beads"), Err("no list button nope".into()));
        assert_eq!(run_list(&scripts, "go", &[], "BEAD_IDS", &[], d, "p7", "beads"), Err("the list is empty".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_view_reads_its_own_table_and_a_list_button_says_so() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-buttons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wheel.toml"), "[beads.buttons.go]\nlabel = \"Go\"\non = [\"leaf\"]\nrun = \"true\"\n[issues.buttons.all]\non = [\"list\"]\nrun = \"true\"\n").unwrap();
        let (b, _) = scripts(dir.to_str().unwrap(), "beads");
        let (i, _) = scripts(dir.to_str().unwrap(), "issues");
        assert_eq!(b.iter().map(|s| s.label()).collect::<Vec<_>>(), ["Go"]);
        assert!(!b[0].on_list());
        assert_eq!(i.iter().map(|s| s.label()).collect::<Vec<_>>(), ["all"]);
        assert!(i[0].on_list());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
