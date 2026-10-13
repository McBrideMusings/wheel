//! What an artifact *is*: a folder holding `index.html` and an `artifact.toml` manifest. The
//! manifest is the whole definition and it lives on disk, so a project's artifacts travel with its
//! clone and nothing about them goes into the daemon's saved state file.
//!
//! The folder comes from [`AppState::artifact_dir`] (`web.rs`), the one place a scope and a name
//! become a path. Everything here reads or writes inside that folder and computes no root of its
//! own.

use std::collections::BTreeMap;
use std::path::Path;

use wheel_proto::{ArtifactInfo, ArtifactRef, ArtifactScope, LOCAL_PREFIX, SCOPE_ORDER};

use crate::server::AppState;

/// The manifest file in every artifact's folder.
const MANIFEST: &str = "artifact.toml";

/// The page a pane loads for an artifact.
const PAGE: &str = "index.html";

/// A directory name an artifact's folder may not use: a pane serves its own data directory under
/// `data/`, so a folder of that name could never be reached.
const RESERVED: &str = "data";

/// The keys `artifact.toml` has; anything else is reported as a problem rather than ignored.
const KEYS: [&str; 3] = ["title", "command", "every"];

/// What one artifact's manifest declares, and what is wrong with it.
struct Manifest {
    title: Option<String>,
    command: Option<String>,
    every: Option<u64>,
    problems: Vec<String>,
}

/// Read the manifest in `dir` and check the folder around it. Never fails: a manifest that is
/// missing, unreadable, malformed or contradictory comes back as problems, so the artifact is
/// still listed, still shown, and still served — a bad artifact must not be a blank pane and a
/// line in the log.
fn load(dir: &Path) -> Manifest {
    let mut m = Manifest {
        title: None,
        command: None,
        every: None,
        problems: Vec::new(),
    };
    match std::fs::read_to_string(dir.join(MANIFEST)) {
        Ok(text) => match text.parse::<toml::Table>() {
            Ok(table) => m.read(table),
            Err(e) => m
                .problems
                .push(format!("{MANIFEST} is not valid TOML: {}", first_line(&e))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => m.problems.push("no manifest".into()),
        Err(e) => m.problems.push(format!("{MANIFEST} cannot be read: {e}")),
    }
    if !dir.join(PAGE).is_file() {
        m.problems.push(format!("no {PAGE}"));
    }
    if dir.join(RESERVED).is_dir() {
        m.problems.push(format!(
            "{RESERVED}/ is reserved: a pane serves its own data there, so this folder is unreachable"
        ));
    }
    m
}

/// A parse error's first line: toml's own message carries the offending line and a caret under it,
/// which belongs in the log rather than on one line of a listing.
fn first_line(e: &toml::de::Error) -> String {
    e.message().lines().next().unwrap_or_default().to_string()
}

impl Manifest {
    /// Take what the manifest declares, reporting every key that is wrong rather than the first.
    fn read(&mut self, table: toml::Table) {
        for (key, value) in &table {
            if !KEYS.contains(&key.as_str()) {
                self.problems
                    .push(format!("unknown key {key}, which nothing reads"));
                continue;
            }
            match (key.as_str(), value) {
                // A key with nothing in it says nothing: a blank title would print as a nameless
                // row, and a blank command would satisfy `every` while running nothing.
                (key @ ("title" | "command"), toml::Value::String(text))
                    if text.trim().is_empty() =>
                {
                    self.problems.push(format!("{key} is empty"))
                }
                ("title", toml::Value::String(title)) => self.title = Some(title.clone()),
                ("command", toml::Value::String(command)) => self.command = Some(command.clone()),
                ("every", toml::Value::Integer(secs)) if *secs > 0 => {
                    self.every = Some(*secs as u64)
                }
                ("every", _) => self
                    .problems
                    .push("every is not a positive whole number of seconds".into()),
                (key, _) => self.problems.push(format!("{key} is not a string")),
            }
        }
        if self.every.is_some() && self.command.is_none() {
            self.problems.push(
                "every asks for a refresh on a timer, but no command says what to run".into(),
            );
        }
    }
}

impl AppState {
    /// The directory of space `space`, or of the active space when none is named. An artifact's
    /// `repo` and `local` roots sit under a space's directory, so every artifact verb comes
    /// through here and a run with no space is refused rather than resolved against nothing.
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

    /// One artifact as it is on disk: where it resolved to, what its manifest says, and what is
    /// wrong with it.
    fn artifact_info(&self, artifact: &ArtifactRef, cwd: &str) -> ArtifactInfo {
        let dir = self.artifact_dir(artifact, cwd);
        let Manifest {
            title,
            command,
            every,
            problems,
        } = load(&dir);
        if !problems.is_empty() {
            tracing::info!(scope = artifact.scope().name(), name = artifact.name(), dir = %dir.display(), problems = problems.len(), "artifact has problems");
        }
        ArtifactInfo {
            scope: artifact.scope(),
            name: artifact.name().to_string(),
            dir: dir.display().to_string(),
            title: title.unwrap_or_else(|| artifact.name().to_string()),
            command,
            every,
            problems,
        }
    }

    /// Make artifact `name` in `scope`: its folder, a manifest naming it, and a page. Refused when
    /// the folder is already there, so nothing an artifact holds is ever overwritten.
    pub fn artifact_new(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
    ) -> Result<ArtifactInfo, String> {
        let cwd = self.artifact_space(space)?;
        let scope = scope.unwrap_or(ArtifactScope::Repo);
        let artifact = ArtifactRef::new(scope, name)?;
        let dir = self.artifact_dir_to_make(&artifact, &cwd)?;
        // Before the folder, so a write that fails leaves nothing of this artifact unignored; the
        // line is about the whole `local` root, not about one artifact, so an extra one costs
        // nothing and a missing one would let a scratch dashboard into the project's history.
        if scope == ArtifactScope::Local {
            gitignore_local(Path::new(&cwd))?;
        }
        // One atomic claim on the name, so two creators cannot both think they made it and nothing
        // an artifact already holds is overwritten.
        std::fs::create_dir(&dir).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!(
                "there is already a {} artifact {name} at {}",
                scope.name(),
                dir.display()
            ),
            _ => format!("make {}: {e}", dir.display()),
        })?;
        // The folder holds exactly these two files, so a fresh artifact has no problems and no
        // `data/` for a pane's own data directory to collide with. A half-written one is taken
        // back, so the next try is not refused by the folder this one left.
        let write = |file: &str, text: String| {
            std::fs::write(dir.join(file), text).map_err(|e| format!("write {file}: {e}"))
        };
        if let Err(e) =
            write(MANIFEST, manifest_for(name)).and_then(|()| write(PAGE, page_for(name)))
        {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        tracing::info!(scope = scope.name(), %name, dir = %dir.display(), "artifact created");
        Ok(self.artifact_info(&artifact, &cwd))
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
        let dir = self.artifact_dir(&artifact, &cwd);
        // A bare name is already refused when it is nowhere; a pinned scope is not, so that a pane
        // can name an artifact whose folder has yet to appear. Showing one is a different thing.
        if !dir.is_dir() {
            return Err(format!(
                "no {} artifact {name}: nothing at {}",
                artifact.scope().name(),
                dir.display()
            ));
        }
        Ok(self.artifact_info(&artifact, &cwd))
    }

    /// Every artifact of every scope, and the scope a bare name resolves to, per name. Ordered by
    /// [`SCOPE_ORDER`], so the first record carrying a name is the one that name means.
    pub fn artifact_ls(
        &self,
        space: Option<&str>,
    ) -> Result<(Vec<ArtifactInfo>, BTreeMap<String, ArtifactScope>), String> {
        let cwd = self.artifact_space(space)?;
        let mut all = Vec::new();
        let mut resolves = BTreeMap::new();
        for scope in SCOPE_ORDER {
            for name in self.artifact_names(scope, &cwd) {
                let Ok(artifact) = ArtifactRef::new(scope, &name) else {
                    continue;
                };
                resolves.entry(name).or_insert(scope);
                all.push(self.artifact_info(&artifact, &cwd));
            }
        }
        Ok((all, resolves))
    }
}

/// The manifest a new artifact starts with: a title, and the other two keys as comments, so the
/// file says what it can carry without declaring a command nobody asked for.
fn manifest_for(name: &str) -> String {
    format!(
        "title = {}\n\n# command = \"your-refresh-command\"   # this build reads it and never runs it\n# every = 30                        # seconds between refreshes\n",
        toml_string(name)
    )
}

/// `name` as a TOML basic string.
fn toml_string(name: &str) -> String {
    toml::Value::String(name.to_string()).to_string()
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

/// Whether a `.gitignore` line already keeps the `local` artifact root out of the project: the
/// prefix itself, or the same path written with or without its slashes, or the `.wheel` directory
/// above it. Not a pattern engine — these are the forms a person writes by hand, and a form this
/// does not know costs one redundant line, never a committed artifact.
fn covers_local(line: &str) -> bool {
    let line = line.trim().trim_start_matches('/').trim_end_matches('/');
    line == LOCAL_PREFIX.trim_end_matches('/') || line == ".wheel"
}

/// Leave [`LOCAL_PREFIX`] in `cwd`'s `.gitignore`, once however many times this runs: everything a
/// `local` artifact owns sits under that one prefix, so ignoring it keeps a scratch dashboard out
/// of the project's history.
fn gitignore_local(cwd: &Path) -> Result<(), String> {
    let path = cwd.join(".gitignore");
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    if current.lines().any(covers_local) {
        return Ok(());
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(LOCAL_PREFIX);
    next.push('\n');
    std::fs::write(&path, next).map_err(|e| format!("write {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), prefix = LOCAL_PREFIX, "gitignored the local artifact root");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(case: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wheel-manifest-{}-{case}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every problem the manifest loader reports, each from the folder that causes it. A problem
    /// list is what a listing and a `show` print, so this is the text a person reads.
    #[test]
    fn the_loader_reports_what_is_wrong_with_an_artifact() {
        let dir = folder("problems");
        // Nothing at all: no manifest and no page, and neither hides the other.
        let m = load(&dir);
        assert_eq!(m.problems, ["no manifest", "no index.html"]);
        assert_eq!((m.title, m.command, m.every), (None, None, None));

        std::fs::write(dir.join(PAGE), "<h1>hi</h1>").unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            "title = \"Beads\"\ncommand = \"bd ready\"\nevery = 30\n",
        )
        .unwrap();
        let m = load(&dir);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert_eq!(m.title.as_deref(), Some("Beads"));
        assert_eq!(m.command.as_deref(), Some("bd ready"));
        assert_eq!(m.every, Some(30));

        // An interval with nothing to run, and a key nothing reads.
        std::fs::write(dir.join(MANIFEST), "every = 30\ncolour = \"red\"\n").unwrap();
        let m = load(&dir);
        assert_eq!(
            m.problems,
            [
                "unknown key colour, which nothing reads",
                "every asks for a refresh on a timer, but no command says what to run"
            ]
        );
        assert_eq!(m.every, Some(30));

        // A key with nothing in it: a blank title names nothing, and a blank command satisfies
        // `every` while running nothing, so neither value is taken.
        std::fs::write(
            dir.join(MANIFEST),
            "title = \"  \"\ncommand = \"\"\nevery = 5\n",
        )
        .unwrap();
        let m = load(&dir);
        assert_eq!(
            m.problems,
            [
                "command is empty",
                "title is empty",
                "every asks for a refresh on a timer, but no command says what to run"
            ]
        );
        assert_eq!((m.title, m.command), (None, None));

        // An interval that is not a positive whole number of seconds, each way of not being one.
        for bad in ["every = 0", "every = -5", "every = \"soon\"", "every = 1.5"] {
            let text = format!("command = \"x\"\n{bad}\n");
            std::fs::write(dir.join(MANIFEST), text).unwrap();
            let m = load(&dir);
            assert_eq!(
                m.problems,
                ["every is not a positive whole number of seconds"],
                "{bad}"
            );
            assert_eq!(m.every, None, "{bad}");
        }

        // A title that is not a string, and a manifest that is not TOML at all.
        std::fs::write(dir.join(MANIFEST), "title = 7\n").unwrap();
        assert_eq!(load(&dir).problems, ["title is not a string"]);
        std::fs::write(dir.join(MANIFEST), "title = \n").unwrap();
        let m = load(&dir);
        assert_eq!(m.problems.len(), 1, "{:?}", m.problems);
        assert!(
            m.problems[0].starts_with("artifact.toml is not valid TOML:"),
            "{:?}",
            m.problems
        );

        // `data/` is reserved: a pane serves its own data directory there.
        std::fs::write(dir.join(MANIFEST), "title = \"Beads\"\n").unwrap();
        std::fs::create_dir(dir.join(RESERVED)).unwrap();
        let m = load(&dir);
        assert_eq!(m.problems.len(), 1, "{:?}", m.problems);
        assert!(
            m.problems[0].starts_with("data/ is reserved"),
            "{:?}",
            m.problems
        );
        // A *file* called data is not a directory under the pane's URL, so it is nothing to report.
        std::fs::remove_dir(dir.join(RESERVED)).unwrap();
        std::fs::write(dir.join(RESERVED), "rows").unwrap();
        assert!(load(&dir).problems.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The prefix lands in `.gitignore` exactly once however many times a local artifact is made,
    /// and what was already in the file stays.
    #[test]
    fn the_local_prefix_is_gitignored_exactly_once() {
        let dir = folder("gitignore");
        let path = dir.join(".gitignore");
        gitignore_local(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ".wheel/local/\n");
        gitignore_local(&dir).unwrap();
        gitignore_local(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ".wheel/local/\n");

        // An existing file keeps its lines, and a last line with no newline still gets one.
        std::fs::write(&path, "target\n/dist").unwrap();
        gitignore_local(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "target\n/dist\n.wheel/local/\n"
        );
        gitignore_local(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "target\n/dist\n.wheel/local/\n"
        );

        // A line already covering the root, however it is written, adds nothing.
        for covering in [
            ".wheel/local/",
            "/.wheel/local/",
            ".wheel/local",
            "  .wheel",
            ".wheel/",
        ] {
            std::fs::write(&path, format!("target\n{covering}\n")).unwrap();
            gitignore_local(&dir).unwrap();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                format!("target\n{covering}\n"),
                "{covering} already covers the local root"
            );
        }
        // A line that covers something else does not.
        std::fs::write(&path, ".wheel/localish\n").unwrap();
        gitignore_local(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            ".wheel/localish\n.wheel/local/\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A new artifact's own files load clean: the manifest the daemon writes is the one it reads.
    #[test]
    fn a_new_artifacts_files_have_no_problems() {
        let dir = folder("new");
        std::fs::write(dir.join(MANIFEST), manifest_for("my \"dash\"")).unwrap();
        std::fs::write(dir.join(PAGE), page_for("my dash")).unwrap();
        let m = load(&dir);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert_eq!(m.title.as_deref(), Some("my \"dash\""));
        assert_eq!((m.command, m.every), (None, None));
        // A control character is a legal file name and an illegal bare TOML string character.
        std::fs::write(dir.join(MANIFEST), manifest_for("a\nb\tc\\d")).unwrap();
        let m = load(&dir);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert_eq!(m.title.as_deref(), Some("a\nb\tc\\d"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
