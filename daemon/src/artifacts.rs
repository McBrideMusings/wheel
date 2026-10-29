//! What an artifact *is*: one `[artifacts.<name>]` table in a `wheel.toml` — the space's for scope
//! `project`, the instance directory's for scope `machine` (ADR-0004). Its `path` names the page —
//! a folder, whose page is `index.html`, or one HTML file — wherever it lives, and the rest of the
//! table is the rest of the definition. Nothing beside the page describes it, so a folder of HTML
//! built for anything becomes an artifact by being named, and nothing about an artifact goes into
//! the daemon's saved state file.
//!
//! Every reader of a definition comes through [`entry`], and every place a scope and a name become
//! a folder comes through [`Entry::locate`]: the CLI, a client op, layout restore, serving and the
//! refresh runner.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wheel_proto::{ArtifactInfo, ArtifactRef, ArtifactScope, PostFile, SCOPE_ORDER};

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
const KEYS: [&str; 12] = [
    "path",
    "url",
    "title",
    "description",
    "command",
    "every",
    "pane",
    "ends_with_pane",
    "expires_at",
    "renderer",
    "reply_to",
    "webhook",
];

/// One artifact's entry, and what is wrong with it. Read afresh by every caller, so a value a
/// person changes by hand is in force at the next read.
#[derive(Debug)]
pub(crate) struct Entry {
    /// The `wheel.toml` holding it.
    pub(crate) file: PathBuf,
    /// Where `path` points, relative to the file's directory and with a leading `~/` taken from
    /// the home directory, links not yet followed. None when the entry gives no usable path.
    path: Option<PathBuf>,
    /// The page a pane opens on, for an entry that names one in place of a folder (ADR-0018).
    pub(crate) url: Option<String>,
    title: Option<String>,
    /// One sentence saying what the page is for, so an agent can match it to a turn.
    description: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) every: Option<u64>,
    /// Whether this artifact asks to be offered in a pane picker. Absent is a one-off: still
    /// openable by name, by a person or an agent, and listed by no picker.
    pane: bool,
    /// The agent pane whose closing deletes this artifact, written by `artifact new`.
    ends_with_pane: Option<String>,
    /// When the daemon deletes this artifact, in seconds since the Unix epoch, written by
    /// `artifact new`.
    expires_at: Option<u64>,
    /// The artifact whose page draws this one: set, this entry is a post, a folder of data and
    /// no page of its own (ADR-0010).
    pub(crate) renderer: Option<String>,
    /// The agent pane a page's Send types its answer into.
    pub(crate) reply_to: Option<String>,
    /// The URL the daemon POSTs a page's Send to.
    pub(crate) webhook: Option<String>,
    problems: Vec<String>,
}

/// What a caller declares for an entry beside its path: the keys a person writes by hand.
#[derive(Default)]
pub struct Declared {
    /// Written in place of the path: the entry names a page to open in a browser pane.
    pub url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub command: Option<String>,
    pub every: Option<u64>,
    pub pane: bool,
    pub ends_with_pane: Option<String>,
    pub expires_at: Option<u64>,
    pub renderer: Option<String>,
    pub reply_to: Option<String>,
    pub webhook: Option<String>,
}

impl Declared {
    /// The entry these keys make for `path`, titled after `name` when no title is given, with
    /// `pane` written out either way so the key is visible to whoever opens the file next. The one
    /// place a verb's entry is built: what is written is this table, and what is refused is what
    /// [`Entry::read`] reports about it.
    pub(crate) fn table(&self, path: &str, name: &str) -> toml::Table {
        let mut t = toml::Table::new();
        match &self.url {
            Some(url) => t.insert("url".into(), url.as_str().into()),
            None => t.insert("path".into(), path.into()),
        };
        t.insert(
            "title".into(),
            self.title.as_deref().unwrap_or(name).trim().into(),
        );
        if let Some(description) = &self.description {
            t.insert("description".into(), description.trim().into());
        }
        if let Some(command) = &self.command {
            t.insert("command".into(), command.trim().into());
        }
        if let Some(every) = self.every {
            // Past what a TOML integer holds, it is written as a value the reader refuses rather
            // than wrapped round into one it would take.
            t.insert("every".into(), i64::try_from(every).unwrap_or(-1).into());
        }
        t.insert("pane".into(), self.pane.into());
        if let Some(pane) = &self.ends_with_pane {
            t.insert("ends_with_pane".into(), pane.as_str().into());
        }
        if let Some(at) = self.expires_at {
            t.insert("expires_at".into(), i64::try_from(at).unwrap_or(-1).into());
        }
        if let Some(renderer) = &self.renderer {
            t.insert("renderer".into(), renderer.as_str().into());
        }
        if let Some(pane) = &self.reply_to {
            t.insert("reply_to".into(), pane.as_str().into());
        }
        if let Some(url) = &self.webhook {
            t.insert("webhook".into(), url.as_str().into());
        }
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
    /// For a post, the post's own folder: the files its renderer's page reads under `data/`.
    pub(crate) data: Option<PathBuf>,
}

/// The `wheel.toml` a scope's entries are in: the space's, whose directory is `cwd`, or this
/// machine's (`machine_file`).
pub(crate) fn registry(scope: ArtifactScope, cwd: &str, machine_file: &Path) -> PathBuf {
    match scope {
        ArtifactScope::Project => Path::new(cwd).join(FILE),
        ArtifactScope::Machine => machine_file.to_path_buf(),
    }
}

/// The directory a scope's `wheel.toml` may be written inside: the space's own for a project
/// file, none for the machine's, which keeps writing through any link.
fn inside_of(scope: ArtifactScope, cwd: &str) -> Option<&Path> {
    match scope {
        ArtifactScope::Project => Some(Path::new(cwd)),
        ArtifactScope::Machine => None,
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
            url: None,
            title: None,
            description: None,
            command: None,
            every: None,
            pane: false,
            ends_with_pane: None,
            expires_at: None,
            renderer: None,
            reply_to: None,
            webhook: None,
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
                (
                    key @ ("path" | "url" | "title" | "description" | "command"
                    | "ends_with_pane" | "renderer" | "reply_to" | "webhook"),
                    toml::Value::String(text),
                ) if text.trim().is_empty() => e.problems.push(format!("{key} is empty")),
                ("path", toml::Value::String(text)) => match e.resolve(text.trim()) {
                    Ok(path) => e.path = Some(path),
                    Err(why) => e.problems.push(why),
                },
                ("url", toml::Value::String(url)) => match crate::settings::page(url) {
                    Ok(url) => e.url = Some(url),
                    Err(why) => e.problems.push(why),
                },
                ("renderer", toml::Value::String(name)) => {
                    match ArtifactRef::new(ArtifactScope::Project, name.trim()) {
                        Ok(_) => e.renderer = Some(name.trim().to_string()),
                        Err(why) => e.problems.push(format!("renderer: {why}")),
                    }
                }
                ("reply_to", toml::Value::String(pane)) => {
                    match wheel_proto::plain_name("reply_to", pane.trim()) {
                        Ok(_) => e.reply_to = Some(pane.trim().to_string()),
                        Err(why) => e.problems.push(why),
                    }
                }
                ("webhook", toml::Value::String(url)) => match webhook_url(url.trim()) {
                    Ok(()) => e.webhook = Some(url.trim().to_string()),
                    Err(why) => e.problems.push(why),
                },
                ("title", toml::Value::String(title)) => e.title = Some(title.clone()),
                ("description", toml::Value::String(text)) if text.trim().contains('\n') => e
                    .problems
                    .push("description is more than one line".into()),
                ("description", toml::Value::String(text)) => {
                    e.description = Some(text.trim().to_string())
                }
                ("command", toml::Value::String(command)) => e.command = Some(command.clone()),
                ("every", toml::Value::Integer(secs)) if *secs > 0 => e.every = Some(*secs as u64),
                ("every", _) => e
                    .problems
                    .push("every is not a positive whole number of seconds".into()),
                ("ends_with_pane", toml::Value::String(pane)) => {
                    e.ends_with_pane = Some(pane.clone())
                }
                ("expires_at", toml::Value::Integer(at)) if *at > 0 => {
                    e.expires_at = Some(*at as u64)
                }
                ("expires_at", _) => e
                    .problems
                    .push("expires_at is not a positive whole number of seconds".into()),
                ("pane", toml::Value::Boolean(pane)) => e.pane = *pane,
                ("pane", _) => e.problems.push("pane is not true or false".into()),
                (key, _) => e.problems.push(format!("{key} is not a string")),
            }
        }
        match (table.contains_key("path"), table.contains_key("url")) {
            (false, false) => e
                .problems
                .push("no path, so there is no page to show".into()),
            (true, true) => e
                .problems
                .push("path and url both name the page; keep one".into()),
            _ => {}
        }
        if table.contains_key("url") {
            for (key, set) in [
                ("command", e.command.is_some()),
                ("every", e.every.is_some()),
                ("renderer", e.renderer.is_some()),
                ("reply_to", e.reply_to.is_some()),
                ("webhook", e.webhook.is_some()),
            ] {
                if set {
                    e.problems.push(format!(
                        "a url entry opens a browser pane, which has no page of ours to carry {key}"
                    ));
                }
            }
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
        if let Some(url) = &self.url {
            return Err(format!(
                "the entry names {url}, a page to open in a browser pane, so it has no folder"
            ));
        }
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
                data: None,
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
            data: None,
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
        if self.url.is_some() {
            // Nothing is read from disk, and the url itself was checked when the entry was read.
            return problems;
        }
        if self.renderer.is_some() {
            // A post is a folder of data; its renderer's page is checked where it is. Only the
            // folder has to be there.
            if self.command.is_some() || self.every.is_some() {
                problems.push("a post carries no refresh command".into());
            }
            if let Err(e) = located {
                if self.path.is_some() {
                    problems.push(e.clone());
                }
            }
            return problems;
        }
        match located {
            // An entry with no usable path already said so.
            Err(_) if self.path.is_none() => {}
            Err(e) => problems.push(e.clone()),
            Ok(Located { root, page, .. }) => {
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
pub(crate) fn default_root(scope: ArtifactScope, cwd: &str, instance: &Path) -> (PathBuf, PathBuf) {
    let (base, under) = match scope {
        ArtifactScope::Project => (Path::new(cwd), ".wheel/artifacts"),
        ArtifactScope::Machine => (instance, "artifacts"),
    };
    (base.to_path_buf(), base.join(under))
}

/// [`AppState::renderer_located`] for a caller that holds only the machine file, such as a
/// watch that re-reads the post's renderer when its `wheel.toml` changes.
pub(crate) fn renderer_located_in(
    machine_file: &Path,
    name: &str,
    cwd: &str,
) -> Result<Located, String> {
    let missing = || format!("renderer {name} is not an artifact");
    let mut found = None;
    for scope in SCOPE_ORDER {
        ArtifactRef::new(scope, name).map_err(|_| missing())?;
        if let Some(e) = entry(&registry(scope, cwd, machine_file), name).map_err(|_| missing())? {
            found = Some(e);
            break;
        }
    }
    let renderer = found.ok_or_else(missing)?;
    if renderer.renderer.is_some() {
        return Err(format!("renderer {name} is a post"));
    }
    renderer.locate()
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
        registry(scope, cwd, &self.machine_file)
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
            Some(entry) if entry.renderer.is_some() => {
                let post = entry.locate()?;
                let name = entry.renderer.as_deref().unwrap_or_default();
                let mut renderer = self.renderer_located(name, cwd)?;
                renderer.data = Some(post.root);
                Ok(renderer)
            }
            Some(entry) => entry.locate(),
            None => Err(format!(
                "no {} artifact {}: {} has no entry for it",
                artifact.scope().name(),
                artifact.name(),
                self.artifact_registry(artifact.scope(), cwd).display()
            )),
        }
    }

    /// Where the page that draws post `entry` is: the renderer named by the entry, looked up the
    /// way a bare name is (the project's entry, then the machine's). Refused when there is no
    /// such artifact, or when it is itself a post.
    pub(crate) fn renderer_located(&self, name: &str, cwd: &str) -> Result<Located, String> {
        renderer_located_in(&self.machine_file, name, cwd)
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
        self.artifact_ref_with(scope, name, cwd, false)
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
        self.artifact_ref_with(scope, name, cwd, true)
    }

    /// Resolves as [`Self::artifact_ref`] does; a bare name with no entry anywhere is refused, and
    /// when `for_pane` the message says how to name one that does not exist yet.
    fn artifact_ref_with(
        &self,
        scope: Option<ArtifactScope>,
        name: &str,
        cwd: &str,
        for_pane: bool,
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
        let hint = if for_pane {
            ". Give --scope to name one that does not exist yet"
        } else {
            ""
        };
        Err(format!(
            "no artifact {name}: no entry for it in {}{hint}",
            looked.join(" or ")
        ))
    }

    /// One artifact as it is now: its entry, where its page resolved to, and what is wrong.
    fn artifact_info(&self, artifact: &ArtifactRef, entry: Entry, cwd: &str) -> ArtifactInfo {
        let located = entry.locate();
        let mut problems = entry.all_problems(&located);
        if let Some(name) = &entry.renderer {
            if let Err(e) = self.renderer_located(name, cwd) {
                problems.push(e);
            }
        }
        let (dir, page) = match &located {
            Ok(Located { root, .. }) if entry.renderer.is_some() => {
                (root.display().to_string(), None)
            }
            Ok(Located { root, page, .. }) => (root.display().to_string(), Some(page.clone())),
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
            url: entry.url,
            title: entry.title.unwrap_or_else(|| artifact.name().to_string()),
            description: entry.description,
            command: entry.command,
            every: entry.every,
            pane: entry.pane,
            ends_with_pane: entry.ends_with_pane,
            expires_at: entry.expires_at,
            renderer: entry.renderer,
            reply_to: entry.reply_to,
            webhook: entry.webhook,
            problems,
        }
    }

    /// The folder a new artifact is made in when its maker names none: `name` under the scope's
    /// default root, made if need be — or a refusal, when that root leads out of the directory it
    /// belongs to, which a cloned repo shipping `.wheel/artifacts` as a symlink does. Checked before
    /// the first directory is made, not after: `.wheel` can itself be a symlink out of the space,
    /// and `create_dir_all` follows it, so a check that came afterwards would leave an empty
    /// `artifacts/` wherever it points.
    pub(crate) fn default_folder(
        &self,
        artifact: &ArtifactRef,
        cwd: &str,
    ) -> Result<PathBuf, String> {
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
    #[allow(clippy::too_many_arguments)]
    pub fn artifact_new(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        pane: bool,
        path: Option<&str>,
        (ends_with_pane, expires_in): (Option<&str>, Option<u64>),
        post: Post<'_>,
    ) -> Result<ArtifactInfo, String> {
        let Post {
            renderer,
            files,
            reply_to,
            webhook,
            description,
            url,
        } = post;
        if let Some(url) = url {
            if path.is_some()
                || ends_with_pane.is_some()
                || expires_in.is_some()
                || renderer.is_some()
                || !files.is_empty()
                || reply_to.is_some()
                || webhook.is_some()
            {
                return Err("a url registers a page that is already served, so it cannot be combined with --path, a lifetime, a post or a Send target".into());
            }
            return self.artifact_new_url(name, scope, space, pane, url);
        }
        if let Some(url) = webhook {
            webhook_url(url)?;
        }
        if let Some(pane) = reply_to {
            wheel_proto::plain_name("--reply-to", pane)?;
        }
        if renderer.is_none() && !files.is_empty() {
            return Err("files are the data of a post, so they need --renderer to say which page draws them".into());
        }
        if renderer.is_some() && files.is_empty() {
            return Err("a post is made of the files it holds, so give at least one".into());
        }
        // Only a folder in the scope's default root is ever deleted, so a lifetime on one made
        // anywhere else would promise a deletion that never happens.
        if path.is_some() && (ends_with_pane.is_some() || expires_in.is_some()) {
            return Err(
                "--ends-with-session and --expires apply only to a folder in the default location, so they cannot be combined with --path"
                    .into(),
            );
        }
        if let Some(pane) = ends_with_pane {
            if !self.is_agent_pane(pane) {
                return Err(format!(
                    "{pane} is not an agent pane, so there is no session for the artifact to end with; run this from an agent pane"
                ));
            }
        }
        let expires_at = match expires_in {
            Some(0) => return Err("--expires must be longer than zero seconds".into()),
            Some(secs) => Some(now_secs().saturating_add(secs)),
            None => None,
        };
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
        if let Some(renderer) = renderer {
            let located = self.renderer_located(renderer, &cwd)?;
            if let Some(need) = renderer_reads(&located) {
                if !files.iter().any(|f| f.name == need) {
                    let given: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
                    return Err(format!(
                        "renderer {renderer} reads {need} from a post, and this one holds {}; name the file {need}",
                        given.join(", ")
                    ));
                }
            }
        }
        let sources = post_sources(files)?;
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
        let made = (if renderer.is_some() {
            copy_post(&dir, &sources)
        } else {
            std::fs::write(dir.join(PAGE), page_for(name)).map_err(|e| format!("write {PAGE}: {e}"))
        })
        .and_then(|()| {
            let declared = Declared {
                pane,
                ends_with_pane: ends_with_pane.map(str::to_string),
                expires_at,
                renderer: renderer.map(str::to_string),
                reply_to: reply_to.map(str::to_string),
                webhook: webhook.map(str::to_string),
                description: description.map(str::to_string),
                ..Declared::default()
            };
            self.write_entry(
                &file,
                inside_of(scope, &cwd),
                name,
                &declared.table(&written_path(&file, &dir), name),
            )
        });
        if let Err(e) = made {
            let _ = std::fs::remove_dir_all(&dir);
            remove_made(&made_parents);
            return Err(e);
        }
        if ends_with_pane.is_some() || expires_at.is_some() {
            if let Err(e) = self.remember_lifetime_dir(&cwd) {
                let _ = self.delete_entry(&file, inside_of(scope, &cwd), name);
                let _ = std::fs::remove_dir_all(&dir);
                remove_made(&made_parents);
                return Err(e);
            }
        }
        tracing::info!(scope = scope.name(), %name, dir = %dir.display(), entry = %file.display(), "artifact created");
        let entry = entry(&file, name)?.ok_or("the entry just written is not there")?;
        Ok(self.artifact_info(&artifact, entry, &cwd))
    }

    /// Register `url` as artifact `name`: an entry with no folder, whose pane is a browser pane on
    /// the page. The server behind it is the caller's to run.
    fn artifact_new_url(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        pane: bool,
        url: &str,
    ) -> Result<ArtifactInfo, String> {
        let cwd = self.artifact_space(space)?;
        let scope = scope.unwrap_or(ArtifactScope::Project);
        let artifact = ArtifactRef::new(scope, name)?;
        let file = self.artifact_registry(scope, &cwd);
        let declared = Declared {
            url: Some(crate::settings::page(url)?),
            pane,
            ..Declared::default()
        };
        if entry(&file, name)?.is_some() {
            return Err(format!(
                "there is already a {} artifact {name} in {}",
                scope.name(),
                file.display()
            ));
        }
        self.write_entry(
            &file,
            inside_of(scope, &cwd),
            name,
            &declared.table("", name),
        )?;
        tracing::info!(scope = scope.name(), %name, %url, entry = %file.display(), "artifact created as a url");
        let entry = entry(&file, name)?.ok_or("the entry just written is not there")?;
        Ok(self.artifact_info(&artifact, entry, &cwd))
    }

    /// Add entry `name`, the fields [`Declared::table`] built, to `file`. Refused when the file
    /// already has an entry of that name — checked again under the settings lock, so two writers
    /// cannot both add one. The rest of the file, comments included, is kept.
    pub(crate) fn write_entry(
        &self,
        file: &Path,
        inside: Option<&Path>,
        name: &str,
        fields: &toml::Table,
    ) -> Result<(), String> {
        let _one = self.settings_write.lock().unwrap();
        crate::settings::rewrite(file, inside, &format!("{TABLE}-{name}"), |doc| {
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
        self.write_entry(&file, inside_of(scope, &cwd), &name, &table)?;
        tracing::info!(scope = scope.name(), %name, path = %path.display(), entry = %file.display(), "artifact added");
        let entry = entry(&file, &name)?.ok_or("the entry just written is not there")?;
        Ok(self.artifact_info(&artifact, entry, &cwd))
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
        let cwd = self.artifact_space(space)?;
        self.delete_entry(&file, inside_of(was.scope, &cwd), name)?;
        tracing::info!(scope = was.scope.name(), %name, entry = %file.display(), dir = %was.dir, "artifact removed");
        self.announce_gone(was.scope, &cwd, name);
        Ok(was)
    }

    /// Tell every connection that artifact `name` has no entry any more, so a client can drop the
    /// browser storage its page kept. Only a `project` entry belongs to a space's directory.
    pub(crate) fn announce_gone(&self, scope: ArtifactScope, cwd: &str, name: &str) {
        let cwd = (scope == ArtifactScope::Project).then(|| cwd.to_string());
        tracing::info!(scope = scope.name(), %name, ?cwd, "artifact gone announced");
        let _ = self.global.send(wheel_proto::ServerMsg::ArtifactGone {
            scope,
            name: name.to_string(),
            cwd,
        });
    }

    /// Whether `pane` is an agent pane in some space right now.
    fn is_agent_pane(&self, pane: &str) -> bool {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .find_map(|s| crate::spaces::find_pane(&s.root, pane))
            .is_some_and(|kind| matches!(kind, wheel_proto::PaneKind::Agent { .. }))
    }

    /// Delete entry `name` from `file`, leaving every file it names where it is. Refused, with
    /// nothing removed, when the entry changed or went between being read and being deleted.
    pub(crate) fn delete_entry(
        &self,
        file: &Path,
        inside: Option<&Path>,
        name: &str,
    ) -> Result<(), String> {
        // What the entry said when it was read, so the removal under the lock deletes that entry
        // and not one another writer put in its place since.
        let read = entries(file)?.remove(name);
        let removed = {
            let _one = self.settings_write.lock().unwrap();
            crate::settings::rewrite(file, inside, &format!("{TABLE}-{name}"), |doc| {
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
        Ok(())
    }

    /// Delete every artifact whose lifetime is over: one that expired, or one whose agent pane is
    /// no longer an agent pane. Only an artifact Wheel made — its folder inside the scope's default
    /// root, never the root itself — loses its folder, unless another entry in the same file still names it; a flagged entry pointing anywhere else
    /// (hand-edited, or a folder registered from elsewhere) is left exactly as it is. Called when a
    /// pane closes, when the daemon starts serving and on a timer, so a daemon that was down when a
    /// pane closed still deletes what that pane's session owned.
    pub fn artifact_sweep(&self) {
        let (cwds, live) = {
            let spaces = self.spaces.lock().unwrap();
            let mut cwds: Vec<String> = Vec::new();
            let mut live = std::collections::HashSet::new();
            for space in &spaces.spaces {
                if !cwds.contains(&space.cwd) {
                    cwds.push(space.cwd.clone());
                }
                for (id, kind) in crate::spaces::panes(&space.root) {
                    if matches!(kind, wheel_proto::PaneKind::Agent { .. }) {
                        live.insert(id.to_string());
                    }
                }
            }
            (cwds, live)
        };
        let open = cwds.clone();
        let cwds = {
            let mut all = self.remembered_lifetime_dirs();
            for cwd in cwds {
                if !all.contains(&cwd) {
                    all.push(cwd);
                }
            }
            all
        };
        let now = now_secs();
        let mut registries = vec![(ArtifactScope::Machine, String::new())];
        registries.extend(cwds.into_iter().map(|c| (ArtifactScope::Project, c)));
        for (scope, cwd) in registries {
            let file = self.artifact_registry(scope, &cwd);
            let Ok(all) = entries(&file) else { continue };
            for (name, value) in all {
                let entry = Entry::read(&file, value);
                let why = match (&entry.ends_with_pane, entry.expires_at) {
                    (Some(pane), _) if !live.contains(pane) => format!("pane {pane} ended"),
                    (_, Some(at)) if at <= now => format!("expired at {at}"),
                    _ => continue,
                };
                self.reap(scope, &cwd, &name, &file, &entry, &why);
            }
        }
        self.forget_lifetime_dirs(&open);
    }

    /// The file listing the directories of spaces that made an artifact with a lifetime, kept in
    /// the instance directory so a space closed before a restart is still swept after it: once the
    /// space is gone, nothing else on disk names its directory.
    fn lifetime_dirs_file(&self) -> PathBuf {
        self.instance_dir.join("lifetime-dirs")
    }

    fn remembered_lifetime_dirs(&self) -> Vec<String> {
        let _one = self.settings_write.lock().unwrap();
        self.read_lifetime_dirs().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "cannot read the directories to sweep");
            Vec::new()
        })
    }

    /// The remembered directories, one JSON string per line so a path holding a newline stays one
    /// path. A file that cannot be read is an error, so a caller never writes back a list that only
    /// looked empty and drops what was remembered.
    fn read_lifetime_dirs(&self) -> Result<Vec<String>, std::io::Error> {
        match std::fs::read_to_string(self.lifetime_dirs_file()) {
            Ok(text) => text
                .lines()
                .map(|l| {
                    serde_json::from_str::<String>(l).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{l:?}: {e}"))
                    })
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_lifetime_dirs(&self, dirs: &[String]) -> Result<(), std::io::Error> {
        let file = self.lifetime_dirs_file();
        let tmp = file.with_extension("tmp");
        let text: String = dirs
            .iter()
            .map(|d| {
                format!(
                    "{}\n",
                    serde_json::to_string(d).expect("a string is always JSON")
                )
            })
            .collect();
        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &file))
    }

    /// Add `cwd` to the directories the sweep visits. Called after the entry is written, so a
    /// concurrent [`Self::forget_lifetime_dirs`] either sees the entry and keeps the directory, or
    /// runs first and this puts it back. A failure is returned, so the caller never leaves an
    /// artifact with a lifetime that no sweep would visit after a restart.
    fn remember_lifetime_dir(&self, cwd: &str) -> Result<(), String> {
        let _one = self.settings_write.lock().unwrap();
        let mut dirs = self
            .read_lifetime_dirs()
            .map_err(|e| format!("cannot read the directories to sweep: {e}"))?;
        if !dirs.iter().any(|d| d == cwd) {
            dirs.push(cwd.to_string());
            self.write_lifetime_dirs(&dirs).map_err(|e| {
                format!(
                    "cannot save the directories to sweep in {}: {e}",
                    self.lifetime_dirs_file().display()
                )
            })?;
        }
        Ok(())
    }

    /// Whether the project registry of `cwd` holds an entry with a lifetime. A registry that
    /// cannot be read counts as holding one: it may read again once fixed.
    fn holds_lifetime(&self, cwd: &str) -> bool {
        let file = registry(ArtifactScope::Project, cwd, &self.machine_file);
        entries(&file).map_or(true, |all| {
            all.into_iter().map(|(_, v)| v).any(|value| {
                let e = Entry::read(&file, value);
                e.ends_with_pane.is_some() || e.expires_at.is_some()
            })
        })
    }

    /// Drop every remembered directory that is not an open space's and no longer holds an entry
    /// with a lifetime. The registries are read without the lock, since a read can be slow; the
    /// few directories that look droppable are read again under the lock a new entry is written
    /// and remembered under, so one made in between is never dropped.
    fn forget_lifetime_dirs(&self, open: &[String]) {
        // The file is replaced by a rename, so a read without the lock still sees a whole file.
        let Ok(dirs) = self.read_lifetime_dirs() else {
            return;
        };
        let candidates: Vec<&String> = dirs
            .iter()
            .filter(|cwd| !open.contains(cwd) && !self.holds_lifetime(cwd))
            .collect();
        if candidates.is_empty() {
            return;
        }
        let _one = self.settings_write.lock().unwrap();
        let Ok(dirs) = self.read_lifetime_dirs() else {
            return;
        };
        let kept: Vec<String> = dirs
            .iter()
            .filter(|cwd| !candidates.contains(cwd) || self.holds_lifetime(cwd))
            .cloned()
            .collect();
        if kept.len() != dirs.len() {
            if let Err(e) = self.write_lifetime_dirs(&kept) {
                tracing::warn!(error = %e, "cannot save the directories to sweep");
            }
        }
    }

    /// Delete one artifact [`Self::artifact_sweep`] found out of time: its entry, then its folder.
    fn reap(
        &self,
        scope: ArtifactScope,
        cwd: &str,
        name: &str,
        file: &Path,
        entry: &Entry,
        why: &str,
    ) {
        // The sweep's view of the panes is older than this moment: a pane made since may have
        // already made an artifact, so the pane is looked up again before anything is deleted.
        if let Some(pane) = &entry.ends_with_pane {
            if self.is_agent_pane(pane) {
                return;
            }
        }
        let Ok(Located { root, page, .. }) = entry.locate() else {
            return;
        };
        let (_, default) = default_root(scope, cwd, &self.instance_dir);
        let in_default = default
            .canonicalize()
            .is_ok_and(|d| root.parent() == Some(d.as_path()));
        if page != PAGE || !in_default {
            tracing::debug!(scope = scope.name(), %name, dir = %root.display(), "artifact is out of time but not in the default folder; left alone");
            return;
        }
        if let Err(e) = self.delete_entry(file, inside_of(scope, cwd), name) {
            tracing::warn!(scope = scope.name(), %name, error = %e, "cannot delete an artifact out of time");
            return;
        }
        self.announce_gone(scope, cwd, name);
        // Two hand-edited entries can name one folder; it goes only with the last of them.
        let shared = entries(file).is_ok_and(|all| {
            all.into_iter().map(|(_, v)| v).any(|value| {
                Entry::read(file, value)
                    .locate()
                    .is_ok_and(|l| l.root == root)
            })
        });
        if shared {
            tracing::info!(scope = scope.name(), %name, dir = %root.display(), "artifact deleted; its folder is another entry's, so it stays");
            return;
        }
        if let Err(e) = std::fs::remove_dir_all(&root) {
            tracing::warn!(scope = scope.name(), %name, dir = %root.display(), error = %e, "deleted an artifact's entry but not its folder");
        }
        tracing::info!(scope = scope.name(), %name, dir = %root.display(), %why, "artifact deleted");
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
        Ok(self.artifact_info(&artifact, entry, &cwd))
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
                let info = self.artifact_info(&artifact, Entry::read(&file, value), &cwd);
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

/// Seconds since the Unix epoch.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
pub(crate) fn written_path(file: &Path, dir: &Path) -> String {
    file.parent()
        .and_then(|base| dir.strip_prefix(base).ok())
        .filter(|rel| !rel.as_os_str().is_empty() && !rel.starts_with("~"))
        .unwrap_or(dir)
        .display()
        .to_string()
}

/// What `artifact new` makes beyond a starter page: a post's renderer and files, and where a page's
/// Send goes.
#[derive(Default)]
pub struct Post<'a> {
    pub renderer: Option<&'a str>,
    pub files: &'a [PostFile],
    pub reply_to: Option<&'a str>,
    pub webhook: Option<&'a str>,
    pub description: Option<&'a str>,
    /// A page to open in a browser pane, registered in place of a folder.
    pub url: Option<&'a str>,
}

/// The file a renderer's page reads from a post's folder, declared by the page as
/// `<meta name="wheel-post-reads" content="index.md">`. None when the page declares none, or
/// cannot be read here.
fn renderer_reads(located: &Located) -> Option<String> {
    let html = std::fs::read_to_string(located.root.join(&located.page)).ok()?;
    let at = html.find("name=\"wheel-post-reads\"")?;
    let tag_start = html[..at].rfind('<')?;
    let tag = &html[tag_start..at + html[at..].find('>')?];
    let rest = &tag[tag.find("content=\"")? + "content=\"".len()..];
    let name = &rest[..rest.find('"')?];
    (!name.is_empty()).then(|| name.to_string())
}

/// A webhook is an `http://` or `https://` URL with a host and no whitespace.
fn webhook_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| format!("webhook {url:?} is not an http:// or https:// URL"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    if host.is_empty() || url.contains(char::is_whitespace) {
        return Err(format!("webhook {url:?} has no host, or holds a space"));
    }
    Ok(())
}

/// One file of a post, checked: the name it takes inside the post's folder and the file it is
/// copied from.
struct Source {
    name: PathBuf,
    from: PathBuf,
}

/// Check every file a post is made of before anything is made: a name that stays inside the
/// folder and is not hidden or repeated, from an absolute path to a regular file.
fn post_sources(files: &[PostFile]) -> Result<Vec<Source>, String> {
    let mut out: Vec<Source> = Vec::new();
    for f in files {
        let name = PathBuf::from(&f.name);
        let plain = !f.name.is_empty()
            && name.components().all(|c| match c {
                std::path::Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
                _ => false,
            });
        if !plain {
            return Err(format!(
                "{:?} is not a file name inside a post: no .., no leading dot, no absolute path",
                f.name
            ));
        }
        if out.iter().any(|s| s.name == name) {
            return Err(format!("{} is in the post twice", f.name));
        }
        let from = PathBuf::from(&f.from);
        if !from.is_absolute() {
            return Err(format!("{} is not an absolute path", from.display()));
        }
        if !from.is_file() {
            return Err(format!("{} is not a file", from.display()));
        }
        out.push(Source { name, from });
    }
    Ok(out)
}

/// Copy a post's files into its new folder, making the folders between.
fn copy_post(dir: &Path, sources: &[Source]) -> Result<(), String> {
    for s in sources {
        let to = dir.join(&s.name);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("make {}: {e}", parent.display()))?;
        }
        std::fs::copy(&s.from, &to)
            .map_err(|e| format!("copy {} to {}: {e}", s.from.display(), to.display()))?;
    }
    Ok(())
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

    /// An entry names a page by `path` or by `url`, never both, and a url entry carries nothing
    /// that needs a folder of ours.
    #[test]
    fn a_url_entry_names_a_page_and_no_folder() {
        let dir = folder("url");
        let e = read(
            &dir,
            "[artifacts.sim]\nurl = \"http://127.0.0.1:3100/\"\n",
            "sim",
        );
        assert_eq!(e.url.as_deref(), Some("http://127.0.0.1:3100/"));
        assert!(e.all_problems(&e.locate()).is_empty(), "{:?}", e.problems);

        let e = read(
            &dir,
            "[artifacts.x]\nurl = \"ftp://host/\"\npath = \"site\"\ncommand = \"x\"\n",
            "x",
        );
        assert_eq!(e.url, None);
        let problems = e.all_problems(&e.locate());
        assert!(
            problems[0].contains("not a page a pane can open"),
            "{problems:?}"
        );
        assert_eq!(problems[1], "path and url both name the page; keep one");
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
            data: None,
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
                page: "sheet.html".into(),
                data: None,
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

    fn state_in(dir: &Path) -> std::sync::Arc<AppState> {
        AppState::new(
            "/dev/null".into(),
            "t".into(),
            dir.join("inst"),
            "/bin/false".into(),
        )
    }

    /// Two entries naming one folder in the default root: reaping the first deletes only its
    /// entry, and the folder goes with the last of them.
    #[test]
    fn a_shared_default_folder_goes_with_the_last_entry_reaped() {
        let dir = folder("shared");
        let state = state_in(&dir);
        let space = dir.join("space");
        let shared = space.join(".wheel/artifacts/dash");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join(PAGE), "hi").unwrap();
        let cwd = space.to_str().unwrap();
        let file = space.join(FILE);
        std::fs::write(
            &file,
            "[artifacts.a]\npath = \".wheel/artifacts/dash\"\nexpires_at = 1\n\n[artifacts.b]\npath = \".wheel/artifacts/dash\"\nexpires_at = 1\n",
        )
        .unwrap();
        let reap = |name: &str| {
            let e = entry(&file, name).unwrap().unwrap();
            state.reap(ArtifactScope::Project, cwd, name, &file, &e, "test");
        };
        reap("a");
        assert!(entry(&file, "a").unwrap().is_none());
        assert!(entry(&file, "b").unwrap().is_some());
        assert!(shared.exists(), "another entry still names the folder");
        reap("b");
        assert!(entry(&file, "b").unwrap().is_none());
        assert!(!shared.exists(), "the last entry takes the folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A remembered directory is dropped once it is no open space's and holds no entry with a
    /// lifetime; one with a lifetime, or an open space's, stays. A path holding a newline is one
    /// path.
    #[test]
    fn forgetting_lifetime_dirs_keeps_only_what_still_needs_a_sweep() {
        let dir = folder("forget");
        let state = state_in(&dir);
        std::fs::create_dir_all(dir.join("inst")).unwrap();
        let live = dir.join("live\nline");
        let plain = dir.join("plain");
        let open = dir.join("open");
        for d in [&live, &plain, &open] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(
            live.join(FILE),
            "[artifacts.a]\npath = \"x\"\nexpires_at = 99999999999\n",
        )
        .unwrap();
        std::fs::write(plain.join(FILE), "[artifacts.a]\npath = \"x\"\n").unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_string();
        for d in [&live, &plain, &open] {
            state.remember_lifetime_dir(&s(d)).unwrap();
        }
        assert_eq!(
            state.remembered_lifetime_dirs(),
            vec![s(&live), s(&plain), s(&open)]
        );
        state.forget_lifetime_dirs(&[s(&open)]);
        assert_eq!(state.remembered_lifetime_dirs(), vec![s(&live), s(&open)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A directory that cannot be saved is an error, so an artifact is never left with a lifetime
    /// no later sweep would visit.
    #[test]
    fn remembering_a_directory_that_cannot_be_saved_is_an_error() {
        let dir = folder("unsaved");
        let state = state_in(&dir);
        std::fs::create_dir_all(dir.join("inst/lifetime-dirs.tmp")).unwrap();
        let err = state.remember_lifetime_dir("/some/space").unwrap_err();
        assert!(err.contains("cannot save"), "{err}");
        std::fs::remove_dir(dir.join("inst/lifetime-dirs.tmp")).unwrap();
        std::fs::write(dir.join("inst/lifetime-dirs"), "/plain/path\n").unwrap();
        let err = state.remember_lifetime_dir("/some/space").unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
