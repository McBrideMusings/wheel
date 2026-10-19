//! What an artifact *is*: one `[artifacts.<name>]` table in a `wheel.toml` — the space's for scope
//! `project`, the instance directory's for scope `machine` (ADR-0004). Its `path` names the page —
//! a folder, whose page is `index.html`, or one HTML file — wherever it lives, and the rest of the
//! table is the rest of the definition. Nothing beside the page describes it, so a folder of HTML
//! built for anything becomes an artifact by being named, and nothing about an artifact goes into
//! the daemon's saved state file.
//!
//! Every reader of a definition comes through [`entry`], and every place a scope and a name become
//! a folder comes through [`Entry::locate`]: the CLI, the lead's tools, a client op, layout restore,
//! serving and the refresh runner.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wheel_proto::{ArtifactInfo, ArtifactRef, ArtifactScope, SCOPE_ORDER};

use crate::server::AppState;
use crate::settings::FILE;

/// The table in a `wheel.toml` holding one table per artifact.
const TABLE: &str = "artifacts";

/// The page a pane opens for an entry naming a folder.
const PAGE: &str = "index.html";

/// A directory name an artifact's folder may not hold: it is the URL prefix a pane serves its own
/// data directory under, so a folder of that name could never be reached. Taken from serving's own
/// prefix, so what a pane dispatches elsewhere is what this reports.
const RESERVED: &str = crate::web::DATA;

/// The keys an entry has; anything else is reported as a problem rather than ignored.
const KEYS: [&str; 5] = ["path", "title", "command", "every", "pane"];

/// One artifact's entry, and what is wrong with it. Read afresh by every caller, so a value a
/// person changes by hand is in force at the next read.
#[derive(Debug)]
pub(crate) struct Entry {
    /// The `wheel.toml` holding it.
    pub(crate) file: PathBuf,
    /// Where `path` points, relative to the file's directory and with a leading `~/` taken from
    /// the home directory, links not yet followed. None when the entry gives no usable path.
    path: Option<PathBuf>,
    title: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) every: Option<u64>,
    /// Whether this artifact asks to be offered in a pane picker. Absent is a one-off: still
    /// openable by name, by a person or by the lead, and listed by no picker.
    pane: bool,
    problems: Vec<String>,
}

/// What a caller declares for an entry beside its path: the keys a person writes by hand.
#[derive(Default)]
pub struct Declared {
    pub title: Option<String>,
    pub command: Option<String>,
    pub every: Option<u64>,
    pub pane: bool,
}

impl Declared {
    /// The entry these keys make for `path`, titled after `name` when no title is given, with
    /// `pane` written out either way so the key is visible to whoever opens the file next. The one
    /// place a verb's entry is built: what is written is this table, and what is refused is what
    /// [`Entry::read`] reports about it.
    fn table(&self, path: &str, name: &str) -> toml::Table {
        let mut t = toml::Table::new();
        t.insert("path".into(), path.into());
        t.insert(
            "title".into(),
            self.title.as_deref().unwrap_or(name).trim().into(),
        );
        if let Some(command) = &self.command {
            t.insert("command".into(), command.trim().into());
        }
        if let Some(every) = self.every {
            // Past what a TOML integer holds, it is written as a value the reader refuses rather
            // than wrapped round into one it would take.
            t.insert("every".into(), i64::try_from(every).unwrap_or(-1).into());
        }
        t.insert("pane".into(), self.pane.into());
        t
    }
}

/// Where an entry's page really is.
#[derive(Debug, PartialEq)]
pub(crate) struct Located {
    /// The real folder, links followed, holding the real page: a pane serves nothing outside it.
    pub(crate) root: PathBuf,
    /// The page's file name in `root`.
    pub(crate) page: String,
}

/// The `wheel.toml` a scope's entries are in: the space's, whose directory is `cwd`, or the
/// instance directory's.
pub(crate) fn registry(scope: ArtifactScope, cwd: &str, instance: &Path) -> PathBuf {
    match scope {
        ArtifactScope::Project => Path::new(cwd).join(FILE),
        ArtifactScope::Machine => instance.join(FILE),
    }
}

/// Every entry in `file`, by name. A file that is not there holds none. One that cannot be read, is
/// not TOML, or whose `artifacts` is not a table is an error naming the file, so its artifacts are
/// never silently missing from a listing.
fn entries(file: &Path) -> Result<toml::Table, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(toml::Table::new()),
        Err(e) => return Err(format!("{} cannot be read: {e}", file.display())),
    };
    let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!("{} is not valid TOML: {}", file.display(), first_line(&e))
    })?;
    match doc.remove(TABLE) {
        None => Ok(toml::Table::new()),
        Some(toml::Value::Table(table)) => Ok(table),
        Some(other) => Err(format!(
            "{TABLE} in {} is a {}, not a table",
            file.display(),
            other.type_str()
        )),
    }
}

/// Entry `name` in `file`, or None when the file has no entry of that name.
pub(crate) fn entry(file: &Path, name: &str) -> Result<Option<Entry>, String> {
    Ok(entries(file)?
        .remove(name)
        .map(|value| Entry::read(file, value)))
}

/// A parse error's first line: toml's own message carries the offending line and a caret under it,
/// which belongs in the log rather than on one line of a listing.
fn first_line(e: &toml::de::Error) -> String {
    e.message().lines().next().unwrap_or_default().to_string()
}

impl Entry {
    /// Take what an entry declares, reporting every key that is wrong rather than the first. Never
    /// fails: an entry that is malformed or contradictory comes back with problems, so the artifact
    /// is still listed, still shown, and still served — a bad artifact must not be a blank pane and
    /// a line in the log.
    fn read(file: &Path, value: toml::Value) -> Entry {
        let mut e = Entry {
            file: file.to_path_buf(),
            path: None,
            title: None,
            command: None,
            every: None,
            pane: false,
            problems: Vec::new(),
        };
        let toml::Value::Table(table) = value else {
            e.problems
                .push(format!("the entry is a {}, not a table", value.type_str()));
            return e;
        };
        for (key, value) in &table {
            if !KEYS.contains(&key.as_str()) {
                e.problems
                    .push(format!("unknown key {key}, which nothing reads"));
                continue;
            }
            match (key.as_str(), value) {
                // A key with nothing in it says nothing: a blank title would print as a nameless
                // row, a blank command would satisfy `every` while running nothing, and a blank
                // path would be the directory the file is in.
                (key @ ("path" | "title" | "command"), toml::Value::String(text))
                    if text.trim().is_empty() =>
                {
                    e.problems.push(format!("{key} is empty"))
                }
                ("path", toml::Value::String(text)) => match e.resolve(text.trim()) {
                    Ok(path) => e.path = Some(path),
                    Err(why) => e.problems.push(why),
                },
                ("title", toml::Value::String(title)) => e.title = Some(title.clone()),
                ("command", toml::Value::String(command)) => e.command = Some(command.clone()),
                ("every", toml::Value::Integer(secs)) if *secs > 0 => e.every = Some(*secs as u64),
                ("every", _) => e
                    .problems
                    .push("every is not a positive whole number of seconds".into()),
                ("pane", toml::Value::Boolean(pane)) => e.pane = *pane,
                ("pane", _) => e.problems.push("pane is not true or false".into()),
                (key, _) => e.problems.push(format!("{key} is not a string")),
            }
        }
        if !table.contains_key("path") {
            e.problems
                .push("no path, so there is no page to show".into());
        }
        if e.every.is_some() && e.command.is_none() {
            e.problems.push(
                "every asks for a refresh on a timer, but no command says what to run".into(),
            );
        }
        e
    }

    /// Where `text`, an entry's `path`, points: a leading `~` from the home directory, and a
    /// relative path from the directory holding the entry's file. A `~` with no home directory to
    /// mean is refused rather than read as a folder named `~`.
    fn resolve(&self, text: &str) -> Result<PathBuf, String> {
        let path = match text.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                let home = std::env::var_os("HOME")
                    .ok_or("path starts with ~, but HOME is not set, so it names nothing")?;
                PathBuf::from(home).join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(text),
        };
        Ok(match self.file.parent() {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        })
    }

    /// Where the page really is, following every link: a folder's page is its `index.html`, and a
    /// file is the page, in the folder it really sits in. That folder is the whole of what a pane
    /// serves, so a page reads what is beside and below it and never what a link points out to.
    pub(crate) fn locate(&self) -> Result<Located, String> {
        let path = self
            .path
            .as_ref()
            .ok_or("the entry names no page, so there is nothing to show")?;
        let real = path
            .canonicalize()
            .map_err(|e| format!("nothing at {}: {e}", path.display()))?;
        if real.is_dir() {
            return Ok(Located {
                root: real,
                page: PAGE.into(),
            });
        }
        // A file is a page only when it is HTML: anything else would make the folder it sits in a
        // root for a page that is not there.
        let (Some(root), Some(page), true) = (
            real.parent(),
            real.file_name().and_then(|n| n.to_str()),
            is_html(&real),
        ) else {
            return Err(format!(
                "{} is not an HTML file, so it is not a page a pane shows",
                real.display()
            ));
        };
        Ok(Located {
            root: root.to_path_buf(),
            page: page.to_string(),
        })
    }

    /// The folder a watch of this artifact belongs on: where the page really is, or — when nothing
    /// is there yet — the folder the entry names, or the folder above the file it names, for the
    /// watch to wait on until it appears.
    pub(crate) fn watch_dir(&self) -> Option<PathBuf> {
        if let Ok(located) = self.locate() {
            return Some(located.root);
        }
        let path = self.path.as_ref()?;
        if is_html(path) {
            path.parent().map(Path::to_path_buf)
        } else {
            Some(path.clone())
        }
    }

    /// Every problem with this artifact: the entry's own, and what is wrong where it points.
    fn all_problems(&self, located: &Result<Located, String>) -> Vec<String> {
        let mut problems = self.problems.clone();
        match located {
            // An entry with no usable path already said so.
            Err(_) if self.path.is_none() => {}
            Err(e) => problems.push(e.clone()),
            Ok(Located { root, page }) => {
                if !root.join(page).is_file() {
                    problems.push(format!("no {page} in {}", root.display()));
                }
                if page.starts_with('.') {
                    problems.push(format!("{page} is hidden, so no pane serves it"));
                }
                if root.join(RESERVED).is_dir() {
                    problems.push(format!(
                        "{RESERVED}/ is reserved: a pane serves its own data there, so {} is unreachable",
                        root.join(RESERVED).display()
                    ));
                }
            }
        }
        problems
    }
}

/// Whether `path` names an HTML file, by its extension.
fn is_html(path: &Path) -> bool {
    path.extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("html") || x.eq_ignore_ascii_case("htm"))
}

/// Where a new artifact's folder goes when its maker names none: the directory the daemon owns — a
/// space's, whose is `cwd`, or the instance's — and the folder under it that holds such folders.
/// Both come from one `match`, so a scope cannot gain a default without the directory it has to
/// stay inside.
fn default_root(scope: ArtifactScope, cwd: &str, instance: &Path) -> (PathBuf, PathBuf) {
    let (base, under) = match scope {
        ArtifactScope::Project => (Path::new(cwd), ".wheel/artifacts"),
        ArtifactScope::Machine => (instance, "artifacts"),
    };
    (base.to_path_buf(), base.join(under))
}

impl AppState {
    /// The directory of space `space`, or of the active space when none is named. A `project`
    /// artifact's entry is in a space's `wheel.toml`, so every artifact verb comes through here and
    /// a run with no space is refused rather than resolved against nothing.
    fn artifact_space(&self, space: Option<&str>) -> Result<String, String> {
        let spaces = self.spaces.lock().unwrap();
        let id = match space {
            Some(id) => id.to_string(),
            None => spaces.active.clone().ok_or(
                "no space to resolve an artifact against; make one with `wheel space new <dir>`, or name one with --space",
            )?,
        };
        spaces
            .spaces
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.cwd.clone())
            .ok_or(format!("no space {id}"))
    }

    /// The `wheel.toml` `scope`'s entries are in, for a space whose directory is `cwd`.
    pub(crate) fn artifact_registry(&self, scope: ArtifactScope, cwd: &str) -> PathBuf {
        registry(scope, cwd, &self.instance_dir)
    }

    /// The entry `artifact` names for a space whose directory is `cwd`, or None when its scope's
    /// `wheel.toml` has none of that name.
    pub(crate) fn artifact_entry(
        &self,
        artifact: &ArtifactRef,
        cwd: &str,
    ) -> Result<Option<Entry>, String> {
        entry(
            &self.artifact_registry(artifact.scope(), cwd),
            artifact.name(),
        )
    }

    /// Where `artifact`'s page really is, for a space whose directory is `cwd`, or why there is
    /// nothing to show.
    pub(crate) fn artifact_located(
        &self,
        artifact: &ArtifactRef,
        cwd: &str,
    ) -> Result<Located, String> {
        match self.artifact_entry(artifact, cwd)? {
            Some(entry) => entry.locate(),
            None => Err(format!(
                "no {} artifact {}: {} has no entry for it",
                artifact.scope().name(),
                artifact.name(),
                self.artifact_registry(artifact.scope(), cwd).display()
            )),
        }
    }

    /// The reference `name` means for a space whose directory is `cwd`: in `scope` when a caller
    /// pinned one, whether or not it has an entry yet, else the first scope of [`SCOPE_ORDER`]
    /// whose `wheel.toml` has an entry of that name. A name that is nowhere is refused rather than
    /// guessed at.
    pub(crate) fn artifact_ref(
        &self,
        scope: Option<ArtifactScope>,
        name: &str,
        cwd: &str,
    ) -> Result<ArtifactRef, String> {
        self.artifact_ref_with(scope, name, cwd, "")
    }

    /// [`Self::artifact_ref`] for opening a pane, so a pane never stores a scope the caller did not
    /// mean. A pane may name an artifact whose entry is still to be written, so a refusal of a bare
    /// name says how to name one with `--scope`.
    pub(crate) fn pane_artifact_ref(
        &self,
        scope: Option<ArtifactScope>,
        name: &str,
        cwd: &str,
    ) -> Result<ArtifactRef, String> {
        self.artifact_ref_with(
            scope,
            name,
            cwd,
            ". Give --scope to name one that does not exist yet",
        )
    }

    /// Resolves as [`Self::artifact_ref`] does; a bare name with no entry anywhere is refused with
    /// `hint` appended to the message.
    fn artifact_ref_with(
        &self,
        scope: Option<ArtifactScope>,
        name: &str,
        cwd: &str,
        hint: &str,
    ) -> Result<ArtifactRef, String> {
        if let Some(scope) = scope {
            return ArtifactRef::new(scope, name);
        }
        let mut looked = Vec::new();
        for scope in SCOPE_ORDER {
            let artifact = ArtifactRef::new(scope, name)?;
            if self.artifact_entry(&artifact, cwd)?.is_some() {
                return Ok(artifact);
            }
            looked.push(self.artifact_registry(scope, cwd).display().to_string());
        }
        Err(format!(
            "no artifact {name}: no entry for it in {}{hint}",
            looked.join(" or ")
        ))
    }

    /// One artifact as it is now: its entry, where its page resolved to, and what is wrong.
    fn artifact_info(&self, artifact: &ArtifactRef, entry: Entry) -> ArtifactInfo {
        let located = entry.locate();
        let problems = entry.all_problems(&located);
        let (dir, page) = match &located {
            Ok(Located { root, page }) => (root.display().to_string(), Some(page.clone())),
            Err(_) => (
                entry
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                None,
            ),
        };
        if !problems.is_empty() {
            tracing::info!(scope = artifact.scope().name(), name = artifact.name(), entry = %entry.file.display(), %dir, problems = problems.len(), "artifact has problems");
        }
        ArtifactInfo {
            scope: artifact.scope(),
            name: artifact.name().to_string(),
            entry: entry.file.display().to_string(),
            dir,
            page,
            title: entry.title.unwrap_or_else(|| artifact.name().to_string()),
            command: entry.command,
            every: entry.every,
            pane: entry.pane,
            problems,
        }
    }

    /// The folder a new artifact is made in when its maker names none: `name` under the scope's
    /// default root, made if need be — or a refusal, when that root leads out of the directory it
    /// belongs to, which a cloned repo shipping `.wheel/artifacts` as a symlink does. Checked before
    /// the first directory is made, not after: `.wheel` can itself be a symlink out of the space,
    /// and `create_dir_all` follows it, so a check that came afterwards would leave an empty
    /// `artifacts/` wherever it points.
    fn default_folder(&self, artifact: &ArtifactRef, cwd: &str) -> Result<PathBuf, String> {
        let (base, root) = default_root(artifact.scope(), cwd, &self.instance_dir);
        let out = || {
            Err(format!(
                "{} leads out of {}, so nothing is written there; give the folder with --path",
                root.display(),
                base.display()
            ))
        };
        // What a new folder is made under is the nearest one that already exists, so that is what
        // has to be inside the base.
        let nearest = root.ancestors().find(|a| a.exists()).unwrap_or(&root);
        if !crate::web::inside(nearest, &base) {
            return out();
        }
        std::fs::create_dir_all(&root).map_err(|e| format!("make {}: {e}", root.display()))?;
        // Making it cannot have moved it, but a symlink swapped in between the two lines is held to
        // the same rule rather than trusted for having arrived late.
        if !crate::web::inside(&root, &base) {
            return out();
        }
        Ok(root.join(artifact.name()))
    }

    /// Make artifact `name` in `scope`: a folder at `path` (absolute) or the scope's default, a
    /// starter page in it, and the entry naming it in the scope's `wheel.toml`. Refused when the
    /// scope already has an entry of that name or the folder is already there, so nothing an
    /// artifact holds is ever overwritten.
    pub fn artifact_new(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        pane: bool,
        path: Option<&str>,
    ) -> Result<ArtifactInfo, String> {
        let cwd = self.artifact_space(space)?;
        let scope = scope.unwrap_or(ArtifactScope::Project);
        let artifact = ArtifactRef::new(scope, name)?;
        let file = self.artifact_registry(scope, &cwd);
        if entry(&file, name)?.is_some() {
            return Err(format!(
                "there is already a {} artifact {name} in {}",
                scope.name(),
                file.display()
            ));
        }
        // Every folder this call makes, outermost first, so a failure takes back all of them and
        // leaves no directory that was not there before.
        let mut made_parents: Vec<PathBuf> = Vec::new();
        let dir = match path {
            Some(path) => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(format!(
                        "{} is not an absolute path; give the folder to make as one",
                        path.display()
                    ));
                }
                if let Some(parent) = path.parent() {
                    if let Err(e) = make_parents(parent, &mut made_parents) {
                        remove_made(&made_parents);
                        return Err(format!("make {}: {e}", parent.display()));
                    }
                }
                path
            }
            None => self.default_folder(&artifact, &cwd)?,
        };
        // One atomic claim on the folder, so two creators cannot both think they made it and
        // nothing a folder already holds is overwritten.
        if let Err(e) = std::fs::create_dir(&dir) {
            remove_made(&made_parents);
            return Err(match e.kind() {
                std::io::ErrorKind::AlreadyExists => format!(
                    "{} is already there, so nothing is written into it",
                    dir.display()
                ),
                _ => format!("make {}: {e}", dir.display()),
            });
        }
        // The folder holds exactly this page, so a fresh artifact has no problems and no `data/`
        // for a pane's own data directory to collide with. A half-made one is taken back, so the
        // next try is not refused by the folder this one left.
        let made = std::fs::write(dir.join(PAGE), page_for(name))
            .map_err(|e| format!("write {PAGE}: {e}"))
            .and_then(|()| {
                let declared = Declared {
                    pane,
                    ..Declared::default()
                };
                self.write_entry(
                    &file,
                    name,
                    &declared.table(&written_path(&file, &dir), name),
                )
            });
        if let Err(e) = made {
            let _ = std::fs::remove_dir_all(&dir);
            remove_made(&made_parents);
            return Err(e);
        }
        tracing::info!(scope = scope.name(), %name, dir = %dir.display(), entry = %file.display(), "artifact created");
        let entry = entry(&file, name)?.ok_or("the entry just written is not there")?;
        Ok(self.artifact_info(&artifact, entry))
    }

    /// Add entry `name`, the fields [`Declared::table`] built, to `file`. Refused when the file
    /// already has an entry of that name — checked again under the settings lock, so two writers
    /// cannot both add one. The rest of the file, comments included, is kept.
    fn write_entry(&self, file: &Path, name: &str, fields: &toml::Table) -> Result<(), String> {
        let _one = self.settings_write.lock().unwrap();
        crate::settings::rewrite(file, &format!("{TABLE}-{name}"), |doc| {
            let table = doc
                .entry(TABLE)
                .or_insert(toml_edit::table())
                .as_table_mut()
                .ok_or_else(|| format!("{TABLE} in {} is not a table", file.display()))?;
            // Only the per-artifact tables are written out, never an empty `[artifacts]` above them.
            table.set_implicit(true);
            if table.contains_key(name) {
                return Err(format!(
                    "there is already an artifact {name} in {}",
                    file.display()
                ));
            }
            let mut entry = toml_edit::Table::new();
            for (key, value) in fields {
                let value = match value {
                    toml::Value::String(text) => toml_edit::value(text.as_str()),
                    toml::Value::Integer(n) => toml_edit::value(*n),
                    toml::Value::Boolean(b) => toml_edit::value(*b),
                    other => {
                        return Err(format!(
                            "{key} is a {}, which an entry never holds",
                            other.type_str()
                        ))
                    }
                };
                entry.insert(key, value);
            }
            table.insert(name, toml_edit::Item::Table(entry));
            Ok(true)
        })?;
        Ok(())
    }

    /// Register the page at `path` (absolute: a folder holding `index.html`, or one HTML file) as
    /// artifact `name` in `scope` — the folder's name, or the file's name without its extension,
    /// when none is given. Writes the entry and nothing else: the page's folder is left exactly as
    /// it was. Refused when there is no page there to show, when the scope already has an entry of
    /// that name, or when the declared keys are ones the reader would report as a problem.
    pub fn artifact_add(
        &self,
        path: &str,
        name: Option<&str>,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        declared: Declared,
    ) -> Result<ArtifactInfo, String> {
        let cwd = self.artifact_space(space)?;
        let scope = scope.unwrap_or(ArtifactScope::Project);
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(format!(
                "{} is not an absolute path; give the page's folder or file as one",
                path.display()
            ));
        }
        if path.to_str().is_some_and(|text| text != text.trim()) {
            return Err(format!(
                "{:?} begins or ends with a space, which an entry's path cannot keep",
                path.display()
            ));
        }
        let file = self.artifact_registry(scope, &cwd);
        let name = match name {
            Some(name) => name.to_string(),
            None => default_name(&path)?,
        };
        let artifact = ArtifactRef::new(scope, &name)?;
        let written = written_path(&file, &path);
        let table = declared.table(&written, &name);
        // What would be written is read back first, by the reader every caller uses, so add never
        // writes an entry that lists or shows with a problem.
        let probe = Entry::read(&file, toml::Value::Table(table.clone()));
        if !probe.problems.is_empty() {
            return Err(probe.problems.join("; "));
        }
        // Registering nothing would be a typo that shows only when a pane opens it, so the page
        // has to be there now, found by the rule a pane serves it by.
        let located = probe.locate()?;
        if !located.root.join(&located.page).is_file() {
            return Err(format!(
                "no {} in {}, so there is no page to show",
                located.page,
                located.root.display()
            ));
        }
        if entry(&file, &name)?.is_some() {
            return Err(format!(
                "there is already a {} artifact {name} in {}",
                scope.name(),
                file.display()
            ));
        }
        self.write_entry(&file, &name, &table)?;
        tracing::info!(scope = scope.name(), %name, path = %path.display(), entry = %file.display(), "artifact added");
        let entry = entry(&file, &name)?.ok_or("the entry just written is not there")?;
        Ok(self.artifact_info(&artifact, entry))
    }

    /// Delete artifact `name`'s entry — in `scope` when given, else the one a bare name resolves
    /// to — and give back the entry as it was. Every file it named stays where it is; a pane still
    /// showing it serves nothing until it is registered again.
    pub fn artifact_remove(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<ArtifactInfo, String> {
        let was = self.artifact_show(name, scope, space)?;
        let file = PathBuf::from(&was.entry);
        // What the entry said when it was read, so the removal under the lock deletes that entry
        // and not one another writer put in its place since.
        let read = entries(&file)?.remove(name);
        let removed = {
            let _one = self.settings_write.lock().unwrap();
            crate::settings::rewrite(&file, &format!("{TABLE}-{name}"), |doc| {
                let Some(table) = doc.get_mut(TABLE).and_then(|t| t.as_table_like_mut()) else {
                    return Ok(false);
                };
                // The entry as the reader would see it, whichever way the file writes it — its
                // own `[artifacts.<name>]` table or an inline one.
                let now = table.get(name).and_then(|item| {
                    let mut alone = toml_edit::DocumentMut::new();
                    alone.insert(name, item.clone());
                    alone.to_string().parse::<toml::Table>().ok()?.remove(name)
                });
                let same = now.is_some() && now == read;
                if !same {
                    return Ok(false);
                }
                let removed = table.remove(name).is_some();
                // A table the removal leaves empty goes with it, as a removed setting's does.
                if removed && table.is_empty() {
                    doc.remove(TABLE);
                }
                Ok(removed)
            })?
        };
        if !removed {
            return Err(format!(
                "the entry for {name} in {} changed or went before it could be removed; nothing was removed",
                file.display()
            ));
        }
        tracing::info!(scope = was.scope.name(), %name, entry = %file.display(), dir = %was.dir, "artifact removed");
        Ok(was)
    }

    /// Artifact `name`: in `scope` when a caller pinned one, else the one a bare name resolves to.
    pub fn artifact_show(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<ArtifactInfo, String> {
        let cwd = self.artifact_space(space)?;
        let artifact = self.artifact_ref(scope, name, &cwd)?;
        // A bare name is already refused when it is nowhere; a pinned scope is not, so that a pane
        // can name an artifact whose entry has yet to be written. Showing one is a different thing.
        let entry = self.artifact_entry(&artifact, &cwd)?.ok_or_else(|| {
            format!(
                "no {} artifact {name}: {} has no entry for it",
                artifact.scope().name(),
                self.artifact_registry(artifact.scope(), &cwd).display()
            )
        })?;
        Ok(self.artifact_info(&artifact, entry))
    }

    /// Every artifact of every scope, and the scope a bare name resolves to, per name. Ordered by
    /// [`SCOPE_ORDER`], then by name, so the first record carrying a name is the one that name
    /// means. A `wheel.toml` that cannot be read refuses the whole listing, naming the file, rather
    /// than leaving its artifacts out of it.
    pub fn artifact_ls(
        &self,
        space: Option<&str>,
        pane_only: bool,
    ) -> Result<(Vec<ArtifactInfo>, BTreeMap<String, ArtifactScope>), String> {
        let cwd = self.artifact_space(space)?;
        let mut all = Vec::new();
        let mut resolves = BTreeMap::new();
        for scope in SCOPE_ORDER {
            let file = self.artifact_registry(scope, &cwd);
            let by_name: BTreeMap<String, toml::Value> = entries(&file)?.into_iter().collect();
            for (name, value) in by_name {
                // An entry whose name no reference can hold is not an artifact: no pane could
                // name it.
                let Ok(artifact) = ArtifactRef::new(scope, &name) else {
                    tracing::warn!(entry = %file.display(), %name, "an artifact entry's name is not one plain name; it is not listed");
                    continue;
                };
                let info = self.artifact_info(&artifact, Entry::read(&file, value));
                // `resolves` answers what a bare name means, which is a property of the entries and
                // not of this filter: an unflagged artifact still shadows a flagged one of the same
                // name in a later scope, and a caller asking for a picker's list has to be told
                // that rather than shown a name that would open something else.
                resolves.entry(name).or_insert(scope);
                if pane_only && !info.pane {
                    continue;
                }
                all.push(info);
            }
        }
        Ok((all, resolves))
    }
}

/// The name an artifact registered from `path` takes when it is given none: the folder's name, or
/// the file's name without its extension — the folder's, for an `index.html`, which says nothing.
fn default_name(path: &Path) -> Result<String, String> {
    let named = if !is_html(path) {
        path.file_name()
    } else if path
        .file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("index"))
    {
        path.parent().and_then(Path::file_name)
    } else {
        path.file_stem()
    };
    named
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "{} has no name to give the artifact; give one",
                path.display()
            )
        })
}

/// `dir` as an entry in `file` writes it: relative to the file's directory when it is under it, so
/// a project that commits its `wheel.toml` and the folder together works in every clone, and
/// absolute otherwise — including when the relative form would start with `~`, which the reader
/// takes for the home directory.
fn written_path(file: &Path, dir: &Path) -> String {
    file.parent()
        .and_then(|base| dir.strip_prefix(base).ok())
        .filter(|rel| !rel.as_os_str().is_empty() && !rel.starts_with("~"))
        .unwrap_or(dir)
        .display()
        .to_string()
}

/// The page a new artifact starts with, so a pane showing it has something to draw at once. A name
/// is one path component and nothing more, which still leaves `<` and `&` in it, so it is escaped.
fn page_for(name: &str) -> String {
    let name = name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>{name}</title>\n<h1>{name}</h1>\n<p>This artifact is empty. Write its page in index.html.</p>\n"
    )
}

/// Make `parent` and every folder missing above it, one at a time, pushing each folder this call
/// actually created onto `made` (outermost first). A folder that was already there is never listed,
/// whatever `..` components the path holds.
fn make_parents(parent: &Path, made: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut at = PathBuf::new();
    for part in parent.components() {
        at.push(part);
        match std::fs::create_dir(&at) {
            Ok(()) => made.push(at.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && at.is_dir() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Remove the folders `artifact_new` made above a new artifact, deepest first. A folder that has
/// since gained a file stays.
fn remove_made(parents: &[PathBuf]) {
    for dir in parents.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(case: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wheel-entry-{}-{case}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// The entry `name` in a `wheel.toml` holding `text`, written into `dir`.
    fn read(dir: &Path, text: &str, name: &str) -> Entry {
        let file = dir.join(FILE);
        std::fs::write(&file, text).unwrap();
        entry(&file, name).unwrap().expect("the entry is there")
    }

    /// Every problem the entry reader reports, each from the entry that causes it. A problem list
    /// is what a listing and a `show` print, so this is the text a person reads.
    #[test]
    fn the_reader_reports_what_is_wrong_with_an_entry() {
        let dir = folder("problems");
        std::fs::create_dir(dir.join("site")).unwrap();
        std::fs::write(dir.join("site/index.html"), "<h1>hi</h1>").unwrap();

        let e = read(
            &dir,
            "[artifacts.beads]\npath = \"site\"\ntitle = \"Beads\"\ncommand = \"bd ready\"\nevery = 30\n",
            "beads",
        );
        assert!(e.all_problems(&e.locate()).is_empty(), "{:?}", e.problems);
        assert_eq!(e.title.as_deref(), Some("Beads"));
        assert_eq!(
            (e.command.as_deref(), e.every),
            (Some("bd ready"), Some(30))
        );

        // No path, an interval with nothing to run, and a key nothing reads.
        let e = read(&dir, "[artifacts.x]\nevery = 30\ncolour = \"red\"\n", "x");
        assert_eq!(
            e.all_problems(&e.locate()),
            [
                "unknown key colour, which nothing reads",
                "no path, so there is no page to show",
                "every asks for a refresh on a timer, but no command says what to run"
            ]
        );

        // A key with nothing in it: none of the three values is taken.
        let e = read(
            &dir,
            "[artifacts.x]\npath = \" \"\ntitle = \"  \"\ncommand = \"\"\n",
            "x",
        );
        assert_eq!(
            e.problems,
            ["command is empty", "path is empty", "title is empty"]
        );
        assert_eq!((e.title, e.command, e.path), (None, None, None));

        // An interval that is not a positive whole number of seconds, each way of not being one.
        for bad in ["every = 0", "every = -5", "every = \"soon\"", "every = 1.5"] {
            let e = read(
                &dir,
                &format!("[artifacts.x]\npath = \"site\"\ncommand = \"x\"\n{bad}\n"),
                "x",
            );
            assert_eq!(
                e.problems,
                ["every is not a positive whole number of seconds"],
                "{bad}"
            );
        }

        // An entry that is not a table, a pane that is not a boolean.
        assert_eq!(
            read(&dir, "[artifacts]\nx = \"site\"\n", "x").problems,
            ["the entry is a string, not a table"]
        );
        assert_eq!(
            read(
                &dir,
                "[artifacts.x]\npath = \"site\"\npane = \"yes\"\n",
                "x"
            )
            .problems,
            ["pane is not true or false"]
        );

        // A file that is not HTML is not a page, and its folder is no root.
        std::fs::write(dir.join("notes.txt"), "notes").unwrap();
        let e = read(&dir, "[artifacts.x]\npath = \"notes.txt\"\n", "x");
        assert!(
            e.locate()
                .unwrap_err()
                .ends_with("is not an HTML file, so it is not a page a pane shows"),
            "{:?}",
            e.locate()
        );
        assert_eq!(e.all_problems(&e.locate()).len(), 1);

        // A path with nothing there, and a folder with no page and a reserved `data/`.
        let e = read(&dir, "[artifacts.x]\npath = \"gone\"\n", "x");
        let problems = e.all_problems(&e.locate());
        assert!(problems[0].starts_with("nothing at "), "{problems:?}");
        std::fs::create_dir_all(dir.join("empty/data")).unwrap();
        let e = read(&dir, "[artifacts.x]\npath = \"empty\"\n", "x");
        let problems = e.all_problems(&e.locate());
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("no index.html in "), "{problems:?}");
        assert!(problems[1].starts_with("data/ is reserved"), "{problems:?}");

        // A file that is not TOML, or whose `artifacts` is not a table, refuses rather than reading
        // as holding nothing.
        std::fs::write(dir.join(FILE), "artifacts = \n").unwrap();
        assert!(entry(&dir.join(FILE), "x")
            .unwrap_err()
            .contains("is not valid TOML"));
        std::fs::write(dir.join(FILE), "artifacts = 3\n").unwrap();
        assert!(entry(&dir.join(FILE), "x")
            .unwrap_err()
            .contains("not a table"));
        // A file that is not there holds no entries.
        assert!(entry(&dir.join("none.toml"), "x").unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An entry's page, wherever the path points: a folder serves its `index.html`, a file is the
    /// page in the folder it really sits in, a relative path is taken from the file's directory, and
    /// a link — the path itself, or a folder on the way — is followed to where the page really is.
    #[test]
    fn an_entry_locates_its_page_wherever_it_lives() {
        let dir = folder("locate");
        let course = dir.join("elsewhere/course");
        std::fs::create_dir_all(course.join("lessons")).unwrap();
        std::fs::write(course.join("index.html"), "home").unwrap();
        std::fs::write(course.join("sheet.html"), "sheet").unwrap();
        let reg = dir.join("project");
        std::fs::create_dir(&reg).unwrap();

        let at =
            |path: &str| read(&reg, &format!("[artifacts.c]\npath = {path:?}\n"), "c").locate();
        let folder_page = Located {
            root: course.clone(),
            page: "index.html".into(),
        };
        assert_eq!(
            at(course.to_str().unwrap()).unwrap(),
            folder_page,
            "absolute folder"
        );
        assert_eq!(
            at("../elsewhere/course").unwrap(),
            folder_page,
            "relative to the file"
        );
        assert_eq!(
            at("../elsewhere/course/sheet.html").unwrap(),
            Located {
                root: course.clone(),
                page: "sheet.html".into()
            },
            "a file is its own page"
        );
        // A link to the folder serves the folder it points at, and a link to a page serves from the
        // folder the page really sits in, not the one the link sits in.
        std::os::unix::fs::symlink(&course, reg.join("linked")).unwrap();
        assert_eq!(at("linked").unwrap(), folder_page);
        std::os::unix::fs::symlink(course.join("sheet.html"), reg.join("sheet.html")).unwrap();
        assert_eq!(at("sheet.html").unwrap().root, course);
        // `~/` is the home directory.
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let e = read(&reg, "[artifacts.c]\npath = \"~/x/index.html\"\n", "c");
        assert_eq!(e.path, Some(home.join("x/index.html")));
        // Nothing there is an error naming the path; a watch waits on the folder the entry names.
        assert!(at("../missing").unwrap_err().starts_with("nothing at "));
        let e = read(&reg, "[artifacts.c]\npath = \"../later/page.html\"\n", "c");
        assert_eq!(e.watch_dir(), Some(reg.join("../later")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The path a new entry writes is relative when the folder is under the file's directory, so a
    /// committed `wheel.toml` works in every clone, and absolute otherwise.
    #[test]
    fn a_new_entry_names_its_folder_relative_to_its_file_when_it_can() {
        let file = Path::new("/p/proj/wheel.toml");
        assert_eq!(
            written_path(file, Path::new("/p/proj/.wheel/artifacts/dash")),
            ".wheel/artifacts/dash"
        );
        assert_eq!(
            written_path(file, Path::new("/elsewhere/dash")),
            "/elsewhere/dash"
        );
        assert_eq!(written_path(file, Path::new("/p/proj")), "/p/proj");
        // A folder literally named `~` is written absolutely, since `~/x` reads as the home's `x`.
        assert_eq!(written_path(file, Path::new("/p/proj/~/x")), "/p/proj/~/x");
        assert_eq!(written_path(file, Path::new("/p/proj/~x")), "~x");
    }

    /// A registered page is named after its folder, or its file without the extension, unless that
    /// file is an `index.html`.
    #[test]
    fn a_registered_page_is_named_after_what_it_is() {
        for (path, name) in [
            (
                "/u/learn/rainforest-interview-prep",
                "rainforest-interview-prep",
            ),
            ("/u/learn/course/interview-sheet.html", "interview-sheet"),
            ("/u/learn/course/index.html", "course"),
            ("/u/learn/course/INDEX.HTM", "course"),
        ] {
            assert_eq!(default_name(Path::new(path)).unwrap(), name, "{path}");
        }
    }

    /// A new artifact's default folder refuses a `.wheel` that leads out of the space — a cloned
    /// repo can ship it as a symlink — before making anything, so not even an empty folder appears
    /// where the link points.
    #[test]
    fn a_default_folder_never_leads_out_of_its_space() {
        let dir = folder("default");
        let (space, outside) = (dir.join("space"), dir.join("outside"));
        std::fs::create_dir_all(&space).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let state = AppState::new(
            "/dev/null".into(),
            "t".into(),
            dir.join("inst"),
            "/bin/false".into(),
        );
        let artifact = ArtifactRef::new(ArtifactScope::Project, "dash").unwrap();
        let cwd = space.to_str().unwrap();
        std::os::unix::fs::symlink(&outside, space.join(".wheel")).unwrap();
        let refused = state.default_folder(&artifact, cwd).unwrap_err();
        assert!(refused.contains("leads out of"), "{refused}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        // A `.wheel` that stays inside the space is followed.
        std::fs::remove_file(space.join(".wheel")).unwrap();
        std::fs::create_dir(space.join("real")).unwrap();
        std::os::unix::fs::symlink(space.join("real"), space.join(".wheel")).unwrap();
        assert_eq!(
            state.default_folder(&artifact, cwd).unwrap(),
            space.join(".wheel/artifacts/dash")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
