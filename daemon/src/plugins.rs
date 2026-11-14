//! Plugin packages (ADR-0041): a folder holding `wheel-plugin.toml` and the Luau scripts of its
//! panes. wheeld finds the packages a space can use, sends a plugin pane's script source to
//! clients (`plugin_source`) with every other `.luau` file of its package for `require`, and
//! watches the package folders so a save reaches every open pane within a moment. It never runs a script: each client does.
//!
//! A package comes from the space's project (`<project>/.wheel/plugins/<id>/`), this machine
//! (`plugins/<id>/` beside the machine `wheel.toml`) or is compiled into wheeld. An id resolves to
//! the first package carrying it, in that order.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wheel_proto::{
    BackendInfo, BackendState, BackendTransport, PluginInfo, PluginPaneInfo, PluginScope, ServerMsg,
};

use crate::server::AppState;
use crate::spaces::{space_find, space_panes, Spaces};
use wheel_proto::PaneKind;

/// The manifest's file name.
pub(crate) const MANIFEST: &str = "wheel-plugin.toml";

/// A built-in package: its id and its files, as (path in the package, text).
type Builtin = (&'static str, &'static [(&'static str, &'static str)]);

/// The packages compiled into wheeld. Adding one is one line here.
const BUILTINS: &[Builtin] = &[
    (
        "sample",
        &[
            (
                MANIFEST,
                include_str!("../plugins/sample/wheel-plugin.toml"),
            ),
            ("main.luau", include_str!("../plugins/sample/main.luau")),
        ],
    ),
    (
        AGENT_VIEW_PLUGIN,
        &[
            (
                MANIFEST,
                include_str!("../plugins/agent/wheel-plugin.toml"),
            ),
            ("main.luau", include_str!("../plugins/agent/main.luau")),
            ("lib/transcript.luau", include_str!("../plugins/agent/lib/transcript.luau")),
            ("lib/tool_card.luau", include_str!("../plugins/agent/lib/tool_card.luau")),
            ("lib/diff.luau", include_str!("../plugins/agent/lib/diff.luau")),
            ("lib/status_header.luau", include_str!("../plugins/agent/lib/status_header.luau")),
            ("lib/composer.luau", include_str!("../plugins/agent/lib/composer.luau")),
            ("lib/style.luau", include_str!("../plugins/agent/lib/style.luau")),
        ],
    ),
];

/// The plugin an agent pane shows when its person asks for the Luau view, and that plugin's pane.
pub(crate) const AGENT_VIEW_PLUGIN: &str = "agent";
pub(crate) const AGENT_VIEW_PANE: &str = "view";

/// The plugin and plugin pane a pane of `kind` shows: a plugin pane's own, the built-in agent view
/// for an agent pane, none for any other kind. Every place that reads a pane's script, feeds or
/// actions asks this.
pub(crate) fn shown_plugin(kind: &PaneKind) -> Option<(String, String)> {
    match kind {
        PaneKind::Plugin { plugin, pane, .. } => Some((plugin.clone(), pane.clone())),
        PaneKind::Agent { .. } => Some((AGENT_VIEW_PLUGIN.to_string(), AGENT_VIEW_PANE.to_string())),
        _ => None,
    }
}

/// Where a package's files are.
#[derive(Debug, Clone)]
enum Files {
    Dir(PathBuf),
    Builtin(&'static [(&'static str, &'static str)]),
}

/// A package found on disk (or compiled in), its manifest read.
#[derive(Debug, Clone)]
pub(crate) struct Package {
    pub info: PluginInfo,
    files: Files,
}

impl Package {
    /// What one backend instance is keyed by: the package folder, or `builtin:<id>` for a package
    /// compiled in.
    pub(crate) fn key(&self) -> String {
        match &self.files {
            Files::Dir(dir) => dir.display().to_string(),
            Files::Builtin(_) => format!("builtin:{}", self.info.id),
        }
    }

    /// The folder a backend runs in, None for a package compiled in.
    pub(crate) fn dir(&self) -> Option<&Path> {
        match &self.files {
            Files::Dir(dir) => Some(dir),
            Files::Builtin(_) => None,
        }
    }
}

#[derive(serde::Deserialize)]
struct RawManifest {
    plugin: RawPlugin,
    #[serde(default)]
    pane: Vec<RawPane>,
    backend: Option<RawBackend>,
}

#[derive(serde::Deserialize)]
struct RawBackend {
    command: String,
    transport: Option<String>,
    restart: Option<String>,
    #[serde(default)]
    watch: Vec<String>,
    #[serde(default)]
    needs: Vec<String>,
}

/// What a manifest says: the package's id, version, panes and `[backend]`.
#[derive(Debug)]
pub(crate) struct Manifest {
    pub id: String,
    pub version: String,
    pub panes: Vec<PluginPaneInfo>,
    pub backend: Option<BackendInfo>,
}

#[derive(serde::Deserialize)]
struct RawPlugin {
    id: String,
    version: String,
}

#[derive(serde::Deserialize)]
struct RawPane {
    id: String,
    title: String,
    script: String,
    #[serde(default)]
    reads: Vec<String>,
    #[serde(default)]
    actions: Vec<String>,
}

/// Whether `name` is one plain name: no separator, no dot-dot, not empty.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

/// Whether `script` stays inside the package folder: relative, with no `..`.
fn inside_package(script: &str) -> bool {
    let path = Path::new(script);
    !script.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// A manifest's `[backend]`, its process not started.
fn parse_backend(raw: RawBackend) -> Result<BackendInfo, String> {
    if raw.command.trim().is_empty() {
        return Err(format!("{MANIFEST}: [backend] command is empty"));
    }
    let transport = match raw.transport.as_deref() {
        None | Some("stdio") => BackendTransport::Stdio,
        Some("unix") => BackendTransport::Unix,
        Some(other) => {
            return Err(format!(
                "{MANIFEST}: [backend] transport {other:?} is not \"stdio\" or \"unix\""
            ))
        }
    };
    let restart = match raw.restart.as_deref() {
        None => "on-failure".to_string(),
        Some(r @ ("always" | "on-failure" | "never")) => r.to_string(),
        Some(other) => {
            return Err(format!(
                "{MANIFEST}: [backend] restart {other:?} is not \"always\", \"on-failure\" or \"never\""
            ))
        }
    };
    Ok(BackendInfo {
        command: raw.command,
        transport,
        restart,
        watch: raw.watch,
        needs: raw.needs,
        state: BackendState::Stopped,
        ..BackendInfo::default()
    })
}

/// Read a manifest's text: its id, version, panes and backend. Unknown tables are tolerated and
/// ignored.
pub(crate) fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let raw: RawManifest = toml::from_str(text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if !plain_name(&raw.plugin.id) {
        return Err(format!(
            "{MANIFEST}: plugin id {:?} is not one plain name",
            raw.plugin.id
        ));
    }
    let mut seen = BTreeSet::new();
    let mut panes = Vec::new();
    for pane in raw.pane {
        if !plain_name(&pane.id) {
            return Err(format!(
                "{MANIFEST}: pane id {:?} is not one plain name",
                pane.id
            ));
        }
        if !seen.insert(pane.id.clone()) {
            return Err(format!("{MANIFEST}: pane id {:?} appears twice", pane.id));
        }
        if !inside_package(&pane.script) {
            return Err(format!(
                "{MANIFEST}: the script of pane {:?}, {:?}, must be a path inside the package folder",
                pane.id, pane.script
            ));
        }
        panes.push(PluginPaneInfo {
            id: pane.id,
            title: pane.title,
            script: pane.script,
            reads: pane.reads,
            actions: pane.actions,
        });
    }
    let backend = raw.backend.map(parse_backend).transpose()?;
    Ok(Manifest {
        id: raw.plugin.id,
        version: raw.plugin.version,
        panes,
        backend,
    })
}

/// A package from its manifest text; a manifest that does not parse lists the package under
/// `fallback_id` with the reason and no panes.
fn package(text: Result<String, String>, fallback_id: &str, scope: PluginScope, files: Files) -> Package {
    let dir = match &files {
        Files::Dir(dir) => Some(dir.display().to_string()),
        Files::Builtin(_) => None,
    };
    let info = match text.and_then(|t| parse_manifest(&t)) {
        Ok(manifest) => PluginInfo {
            id: manifest.id,
            version: manifest.version,
            scope,
            dir,
            panes: manifest.panes,
            backend: manifest.backend,
            error: None,
        },
        Err(error) => PluginInfo {
            id: fallback_id.to_string(),
            version: String::new(),
            scope,
            dir,
            panes: Vec::new(),
            backend: None,
            error: Some(error),
        },
    };
    Package { info, files }
}

/// Every package folder under `root`, by folder name. A folder with no manifest is not a package.
fn scan(root: &Path, scope: PluginScope) -> Vec<Package> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|dir| {
            let name = dir.file_name()?.to_str()?.to_string();
            let manifest = dir.join(MANIFEST);
            if !manifest.is_file() {
                return None;
            }
            let text = std::fs::read_to_string(&manifest)
                .map_err(|e| format!("{} cannot be read: {e}", manifest.display()));
            Some(package(text, &name, scope, Files::Dir(dir)))
        })
        .collect()
}

fn builtin_packages() -> Vec<Package> {
    BUILTINS
        .iter()
        .map(|(id, files)| {
            let text = files
                .iter()
                .find(|(name, _)| *name == MANIFEST)
                .map(|(_, text)| text.to_string())
                .ok_or_else(|| format!("built-in {id} has no {MANIFEST}"));
            package(text, id, PluginScope::Builtin, Files::Builtin(files))
        })
        .collect()
}

/// The folder of a project's plugin packages.
pub(crate) fn project_plugins(space_dir: &str) -> PathBuf {
    Path::new(space_dir).join(".wheel").join("plugins")
}

/// Every package for a space whose directory is `space_dir` (None: no project), project then
/// machine then built-in.
pub(crate) fn discover(space_dir: Option<&str>, machine_dir: &Path) -> Vec<Package> {
    let mut all = Vec::new();
    if let Some(dir) = space_dir {
        all.extend(scan(&project_plugins(dir), PluginScope::Project));
    }
    all.extend(scan(machine_dir, PluginScope::Machine));
    all.extend(builtin_packages());
    all
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters (none, or dots
/// included) and nothing else is special: how a pane's `reads` and `actions` name feeds and actions.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The package an id resolves to: the first carrying it.
pub(crate) fn resolve<'a>(packages: &'a [Package], id: &str) -> Result<&'a Package, String> {
    let found = packages
        .iter()
        .find(|p| p.info.id == id)
        .ok_or_else(|| format!("no plugin {id} for this space"))?;
    match &found.info.error {
        Some(error) => Err(format!("plugin {id} cannot be used: {error}")),
        None => Ok(found),
    }
}

/// The pane `pane_id` of `package`.
fn find_pane<'a>(package: &'a Package, pane_id: &str) -> Result<&'a PluginPaneInfo, String> {
    package
        .info
        .panes
        .iter()
        .find(|p| p.id == pane_id)
        .ok_or_else(|| format!("plugin {} has no pane {pane_id}", package.info.id))
}

/// What a plugin pane runs: its script, and every other `.luau` file of its package keyed by its
/// path in the package, for `require`.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// `<plugin>/<script>`, naming the script in error messages.
    pub chunk: String,
    pub source: String,
    pub modules: BTreeMap<String, String>,
}

/// The pane's script and the package's other `.luau` files, together at most [`MAX_SCRIPT`]
/// bytes. A file a symlink carries out of the package is never read: the script is refused, a
/// module left out.
fn read_script(package: &Package, pane: &PluginPaneInfo) -> Result<Script, String> {
    let chunk = format!("{}/{}", package.info.id, pane.script);
    let too_large = || format!("cannot read {chunk}: the script and the package's Luau files are larger than 1 MB");
    let want = Path::new(&pane.script);
    let (source, modules) = match &package.files {
        Files::Builtin(files) => {
            let source = files
                .iter()
                .find(|(name, _)| Path::new(name) == want)
                .map(|(_, text)| text.to_string())
                .ok_or_else(|| format!("cannot read {chunk}: no such file"))?;
            let modules = files
                .iter()
                .filter(|(name, _)| name.ends_with(".luau") && Path::new(name) != want)
                .map(|(name, text)| (name.to_string(), text.to_string()))
                .collect();
            (source, modules)
        }
        Files::Dir(dir) => {
            let root = dir
                .canonicalize()
                .map_err(|e| format!("cannot read {chunk}: {e}"))?;
            // A symlink must not carry the script out of the package folder.
            let real = dir
                .join(&pane.script)
                .canonicalize()
                .map_err(|e| format!("cannot read {chunk}: {e}"))?;
            if !real.starts_with(&root) {
                return Err(format!("cannot read {chunk}: it is outside the package folder"));
            }
            let source = read_capped(&real, MAX_SCRIPT).map_err(|e| format!("cannot read {chunk}: {e}"))?;
            if source.len() as u64 > MAX_SCRIPT {
                return Err(too_large());
            }
            let mut modules = BTreeMap::new();
            let mut walk = Walk {
                root: &root,
                script: &real,
                left: MAX_SCRIPT - source.len() as u64,
                entries: 0,
            };
            walk.folder(&root, 0, &mut modules).map_err(|TooLarge| too_large())?;
            if walk.entries > MODULE_ENTRIES {
                tracing::warn!(%chunk, "the package has more than {MODULE_ENTRIES} entries; modules past them are left out");
            }
            (source, modules)
        }
    };
    if (source.len() + modules.values().map(String::len).sum::<usize>()) as u64 > MAX_SCRIPT {
        return Err(too_large());
    }
    Ok(Script {
        chunk,
        source,
        modules,
    })
}

/// Reads at most `cap + 1` bytes of `path`, so an oversized file is noticed without reading it all.
fn read_capped(path: &Path, cap: u64) -> std::io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(cap + 1).read_to_string(&mut text))?;
    Ok(text)
}

/// The package's Luau files come to more than [`MAX_SCRIPT`] with the script.
struct TooLarge;

/// Folders a package's modules are never looked for in: hidden ones, and the ones a backend
/// installs packages or writes its build into.
fn skipped_folder(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "build" | "dist" | "vendor")
}

/// How deep below the package folder modules are looked for.
const MODULE_DEPTH: usize = 8;

/// The most folder entries one look for modules visits; past it the rest are left out.
const MODULE_ENTRIES: usize = 10_000;

/// One look for a package's modules.
struct Walk<'a> {
    root: &'a Path,
    /// The pane's own script, which is not a module.
    script: &'a Path,
    /// Bytes still allowed.
    left: u64,
    /// Folder entries visited.
    entries: usize,
}

impl Walk<'_> {
    /// Adds every `.luau` file under `dir` but the script to `modules`, keyed by its path below
    /// the root with `/` separators. Symlinked folders are not entered; a symlinked file that
    /// resolves outside the root, and a folder or file that cannot be read, are left out.
    fn folder(&mut self, dir: &Path, depth: usize, modules: &mut BTreeMap<String, String>) -> Result<(), TooLarge> {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(read) => read.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "a plugin package folder cannot be read; its modules are left out");
                return Ok(());
            }
        };
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            self.entries += 1;
            if self.entries > MODULE_ENTRIES {
                return Ok(());
            }
            let Ok(kind) = entry.file_type() else { continue };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MODULE_DEPTH && !skipped_folder(name) {
                    self.folder(&path, depth + 1, modules)?;
                }
                continue;
            }
            if !name.ends_with(".luau") {
                continue;
            }
            let Ok(real) = path.canonicalize() else { continue };
            if real == self.script || !real.starts_with(self.root) || !real.is_file() {
                continue;
            }
            let Ok(key) = path.strip_prefix(self.root) else { continue };
            let key: Vec<&str> = key.components().filter_map(|c| c.as_os_str().to_str()).collect();
            let key = key.join("/");
            let text = match read_capped(&real, self.left) {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(module = %key, error = %e, "a plugin module cannot be read; it is left out");
                    continue;
                }
            };
            if text.len() as u64 > self.left {
                return Err(TooLarge);
            }
            self.left -= text.len() as u64;
            modules.insert(key, text);
        }
        Ok(())
    }
}

/// One plugin pane of the layout: where it is and what it shows.
struct PluginPane {
    pane: String,
    cwd: String,
    plugin: String,
    plugin_pane: String,
}

fn plugin_panes_of(spaces: &Spaces) -> Vec<PluginPane> {
    spaces
        .spaces
        .iter()
        .flat_map(|space| {
            space_panes(space).into_iter().filter_map(|(pane, kind)| {
                let (plugin, plugin_pane) = shown_plugin(kind)?;
                Some(PluginPane {
                    pane: pane.to_string(),
                    cwd: space.cwd.clone(),
                    plugin,
                    plugin_pane,
                })
            })
        })
        .collect()
}

/// One folder the watch covers: the folder, whether the watch is recursive, and the folder's inode
/// when the watch started, so a folder deleted and made again (a watch on the old one is dead) is a
/// different target.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    path: PathBuf,
    recursive: bool,
    inode: u64,
}

/// The running watch: the folders it covers, and the task that owns its watchers.
pub(crate) struct Watch {
    targets: Vec<Target>,
    task: JoinHandle<()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How long a burst of file events is gathered before the scripts are read.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);

/// The largest script wheeld sends: 1 MB.
const MAX_SCRIPT: u64 = 1024 * 1024;

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or(0, |m| m.ino())
}

/// `path` with symlinks resolved as far as it exists; the part that does not exist stays as it is.
fn real_path(path: &Path) -> PathBuf {
    let Some(ancestor) = path.ancestors().find(|a| a.exists()) else {
        return path.to_path_buf();
    };
    let rest = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
    ancestor.canonicalize().unwrap_or_else(|_| ancestor.to_path_buf()).join(rest)
}

/// What to watch for the wanted folders: each one that exists, recursively; one that does not,
/// through its nearest existing ancestor, non-recursively, which sees the next folder on the way
/// appear. The watched folder's parent is watched too, non-recursively: a watch on a folder
/// that is deleted reports nothing, but its parent reports the deletion.
fn watch_targets(wanted: &[PathBuf]) -> Vec<Target> {
    let mut targets = Vec::new();
    for dir in wanted {
        let Some(base) = dir.ancestors().find(|a| a.is_dir()) else {
            continue;
        };
        let mut push = |path: &Path, recursive: bool| {
            targets.push(Target { path: path.to_path_buf(), recursive, inode: inode(path) });
        };
        push(base, base == dir.as_path());
        if let Some(parent) = base.parent().filter(|p| p.is_dir()) {
            push(parent, false);
        }
    }
    targets.sort_by(|a, b| a.path.cmp(&b.path).then(b.recursive.cmp(&a.recursive)));
    targets.dedup_by(|b, a| a.path == b.path);
    targets
}

impl AppState {
    /// The folder of this machine's plugin packages, beside the machine `wheel.toml`.
    pub(crate) fn machine_plugins(&self) -> PathBuf {
        self.machine_file
            .parent()
            .map_or_else(|| PathBuf::from("plugins"), |p| p.join("plugins"))
    }

    /// Every package a space can use.
    pub fn plugin_ls(&self, space: Option<&str>) -> Result<Vec<PluginInfo>, String> {
        let cwd = self.settings_space(space)?.map(|(_, cwd)| cwd);
        Ok(discover(cwd.as_deref(), &self.machine_plugins())
            .into_iter()
            .map(|p| {
                let mut info = p.info.clone();
                if let Some(backend) = &mut info.backend {
                    self.backends.overlay(&p.key(), backend);
                }
                info
            })
            .collect())
    }

    /// The package and pane that `plugin`/`plugin_pane` mean in a space whose directory is `cwd`.
    pub(crate) fn plugin_check(&self, cwd: &str, plugin: &str, plugin_pane: &str) -> Result<(), String> {
        let packages = discover(Some(cwd), &self.machine_plugins());
        let package = resolve(&packages, plugin)?;
        find_pane(package, plugin_pane).map(|_| ())
    }

    /// The script a plugin pane shows, read now: `(plugin, plugin_pane, script)`. It records
    /// nothing: only the watch decides what clients last had.
    pub fn plugin_source(&self, pane: &str) -> Result<(String, String, Script), String> {
        let (cwd, plugin, plugin_pane) = {
            let spaces = self.spaces.lock().unwrap();
            let (space, kind) = spaces
                .spaces
                .iter()
                .find_map(|s| space_find(s, pane).map(|k| (s, k)))
                .ok_or_else(|| format!("no pane {pane}"))?;
            let (plugin, plugin_pane) = shown_plugin(kind)
                .ok_or_else(|| format!("{pane} is not a plugin or agent pane"))?;
            (space.cwd.clone(), plugin, plugin_pane)
        };
        let script = self.plugin_script(&cwd, &plugin, &plugin_pane)?;
        Ok((plugin, plugin_pane, script))
    }

    fn plugin_script(&self, cwd: &str, plugin: &str, plugin_pane: &str) -> Result<Script, String> {
        let packages = discover(Some(cwd), &self.machine_plugins());
        let package = resolve(&packages, plugin)?;
        read_script(package, find_pane(package, plugin_pane)?)
    }

    /// Bring the plugin watch up to date with the panes that exist now, on a blocking thread
    /// (it reads folders and scripts), so the caller may hold the spaces lock. Does nothing
    /// outside the tokio runtime.
    pub fn sync_plugin_watch(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if let Some(state) = self.me.upgrade() {
            drop(runtime.spawn_blocking(move || state.plugin_sync()));
        }
    }

    /// [`AppState::sync_plugin_watch`], returning when the watch is up to date.
    pub async fn sync_plugin_watch_now(&self) {
        if let Some(state) = self.me.upgrade() {
            let _ = tokio::task::spawn_blocking(move || state.plugin_sync()).await;
        }
    }

    /// Forget closed panes, remember the script each new plugin pane shows, and restart the watch
    /// when the folders to watch are not the ones being watched. Blocking.
    fn plugin_sync(&self) {
        // One sync at a time; the guard also serializes the swap of the watch.
        let mut watch = self.plugin_watch.lock().unwrap();
        // The layout or a plugin folder changed: a pane may be gone, of another kind, or no
        // longer allowed a feed.
        let _ = self.grants_changed.send(());
        let panes = plugin_panes_of(&self.spaces.lock().unwrap());
        {
            let mut seen = self.plugin_seen.lock().unwrap();
            seen.retain(|pane, _| panes.iter().any(|p| &p.pane == pane));
        }
        for p in &panes {
            let known = self.plugin_seen.lock().unwrap().contains_key(&p.pane);
            if !known {
                if let Ok(script) = self.plugin_script(&p.cwd, &p.plugin, &p.plugin_pane) {
                    self.plugin_seen
                        .lock()
                        .unwrap()
                        .entry(p.pane.clone())
                        .or_insert(script);
                }
            }
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut wanted: Vec<PathBuf> = Vec::new();
        if !panes.is_empty() {
            wanted.extend(panes.iter().map(|p| project_plugins(&p.cwd)));
            wanted.push(self.machine_plugins());
        }
        wanted.sort();
        wanted.dedup();
        let targets = watch_targets(&wanted);
        if watch.as_ref().map_or(targets.is_empty(), |w| w.targets == targets) {
            return;
        }
        if targets.is_empty() {
            *watch = None;
            return;
        }
        let (mut watchers, mut rx) = match watch_targets_start(&targets, wanted) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "could not watch the plugin folders; a saved script shows only after the pane reopens");
                return;
            }
        };
        tracing::info!(folders = targets.len(), "plugin watch started");
        let me = self.me.clone();
        let task = runtime.spawn(async move {
            let _watchers = &mut watchers;
            while rx.recv().await.is_some() {
                // A save is often several events; one read covers them.
                tokio::time::sleep(DEBOUNCE).await;
                while rx.try_recv().is_ok() {}
                let Some(state) = me.upgrade() else { return };
                let _ = tokio::task::spawn_blocking(move || state.plugin_files_changed()).await;
            }
        });
        *watch = Some(Watch { targets, task });
        // A save between the last read and the new watch raised no event: read once more.
        self.plugin_broadcast_changes();
    }

    /// Something under a watched folder changed: re-point the watch (a folder may have appeared,
    /// or been removed and made again), then send what changed.
    fn plugin_files_changed(&self) {
        self.plugin_sync();
        self.plugin_broadcast_changes();
    }

    /// Read each plugin pane's script and modules and send every client the ones that now differ
    /// from what the last broadcast carried.
    fn plugin_broadcast_changes(&self) {
        let panes = plugin_panes_of(&self.spaces.lock().unwrap());
        for p in panes {
            let script = match self.plugin_script(&p.cwd, &p.plugin, &p.plugin_pane) {
                Ok(read) => read,
                Err(e) => {
                    tracing::warn!(pane = %p.pane, error = %e, "a plugin script cannot be read; clients keep the last source");
                    continue;
                }
            };
            {
                let mut seen = self.plugin_seen.lock().unwrap();
                if seen.get(&p.pane) == Some(&script) {
                    continue;
                }
                seen.insert(p.pane.clone(), script.clone());
            }
            tracing::info!(pane = %p.pane, chunk = %script.chunk, bytes = script.source.len(), modules = script.modules.len(), "plugin script changed; sent to clients");
            let _ = self.global.send(ServerMsg::PluginSource {
                req: None,
                pane: p.pane,
                plugin: p.plugin,
                plugin_pane: p.plugin_pane,
                chunk: script.chunk,
                source: script.source,
                modules: script.modules,
            });
        }
    }
}

/// Start a watcher per target, sending `()` for each event that touches one of the `wanted`
/// folders or a folder on the way to one.
fn watch_targets_start(
    targets: &[Target],
    wanted: Vec<PathBuf>,
) -> Result<(Vec<notify::RecommendedWatcher>, mpsc::UnboundedReceiver<()>), String> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut watchers = Vec::new();
    // Events name real paths (/private/var, not /var), so compare against those.
    let wanted: Vec<PathBuf> = wanted.iter().map(|w| real_path(w)).collect();
    for target in targets {
        let tx = tx.clone();
        let wanted = wanted.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let relevant = event
                .paths
                .iter()
                .any(|p| wanted.iter().any(|w| p.starts_with(w) || w.starts_with(p)));
            if relevant {
                let _ = tx.send(());
            }
        })
        .map_err(|e| e.to_string())?;
        let mode = if target.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(&target.path, mode)
            .map_err(|e| format!("{}: {e}", target.path.display()))?;
        watchers.push(watcher);
    }
    Ok((watchers, rx))
}

/// The map type `AppState` keeps: pane id to the script clients last had.
pub(crate) type Seen = HashMap<String, Script>;

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[plugin]
id = "demo"
version = "1.2.3"

[backend]
command = "demo-server"

[[pane]]
id = "main"
title = "Demo"
script = "main.luau"
reads = ["a.b"]
actions = ["go"]
"#;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_plugin(root: &Path, folder: &str, manifest: &str) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest).unwrap();
        std::fs::write(dir.join("main.luau"), "-- script").unwrap();
    }

    #[test]
    fn a_good_manifest_parses_and_a_backend_is_tolerated() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!((m.id.as_str(), m.version.as_str()), ("demo", "1.2.3"));
        assert_eq!(m.panes.len(), 1);
        assert_eq!(m.panes[0].reads, ["a.b"]);
        assert_eq!(m.panes[0].actions, ["go"]);
        let m = parse_manifest(
            "[plugin]\nid=\"x\"\nversion=\"1\"\n[[pane]]\nid=\"p\"\ntitle=\"P\"\nscript=\"s.luau\"\n",
        )
        .unwrap();
        assert!(m.panes[0].reads.is_empty() && m.panes[0].actions.is_empty());
        assert!(m.backend.is_none());
    }

    #[test]
    fn a_backend_table_parses_with_its_defaults() {
        let backend = parse_manifest(GOOD).unwrap().backend.unwrap();
        assert_eq!(backend.command, "demo-server");
        assert_eq!(backend.transport, BackendTransport::Stdio);
        assert_eq!(backend.restart, "on-failure");
        assert_eq!(backend.state, BackendState::Stopped);
        assert!(backend.watch.is_empty() && backend.needs.is_empty() && backend.pid.is_none());
        let full = "[plugin]\nid=\"x\"\nversion=\"1\"\n[backend]\ncommand=\"run it\"\ntransport=\"unix\"\nrestart=\"never\"\nwatch=[\"src\"]\nneeds=[\"net\"]\n";
        let backend = parse_manifest(full).unwrap().backend.unwrap();
        assert_eq!(backend.transport, BackendTransport::Unix);
        assert_eq!(backend.restart, "never");
        assert_eq!(backend.watch, ["src"]);
        assert_eq!(backend.needs, ["net"]);
    }

    #[test]
    fn a_bad_backend_table_is_refused() {
        let head = "[plugin]\nid=\"x\"\nversion=\"1\"\n[backend]\n";
        for bad in [
            "command=\"\"\n",
            "transport=\"stdio\"\n",
            "command=\"a\"\ntransport=\"tcp\"\n",
            "command=\"a\"\nrestart=\"sometimes\"\n",
        ] {
            assert!(parse_manifest(&format!("{head}{bad}")).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_bad_manifest_is_refused() {
        assert!(parse_manifest("not toml [").is_err());
        assert!(parse_manifest("[plugin]\nid=\"x\"\n").is_err(), "no version");
        assert!(parse_manifest("[plugin]\nid=\"a/b\"\nversion=\"1\"\n").is_err());
    }

    #[test]
    fn a_script_path_that_escapes_the_package_is_refused() {
        for script in ["/etc/passwd", "../x.luau", "a/../../x.luau", ""] {
            let text = format!(
                "[plugin]\nid=\"x\"\nversion=\"1\"\n[[pane]]\nid=\"p\"\ntitle=\"P\"\nscript=\"{script}\"\n"
            );
            assert!(parse_manifest(&text).is_err(), "{script:?}");
        }
        let ok = "[plugin]\nid=\"x\"\nversion=\"1\"\n[[pane]]\nid=\"p\"\ntitle=\"P\"\nscript=\"ui/main.luau\"\n";
        assert!(parse_manifest(ok).is_ok());
    }

    #[test]
    fn a_bad_package_lists_with_an_error_and_no_panes() {
        let machine = tmp("bad");
        write_plugin(&machine, "broken", "[plugin\n");
        let all = discover(None, &machine);
        let broken = all.iter().find(|p| p.info.id == "broken").unwrap();
        assert!(broken.info.error.is_some() && broken.info.panes.is_empty());
        assert!(resolve(&all, "broken").is_err());
        std::fs::remove_dir_all(&machine).unwrap();
    }

    #[test]
    fn an_id_resolves_project_then_machine_then_builtin() {
        let root = tmp("order");
        let (space, machine) = (root.join("space"), root.join("machine"));
        write_plugin(&project_plugins(space.to_str().unwrap()), "sample", &GOOD.replace("demo", "sample"));
        write_plugin(&machine, "sample", &GOOD.replace("demo", "sample").replace("1.2.3", "9.9.9"));
        write_plugin(&machine, "demo", GOOD);
        let all = discover(space.to_str(), &machine);
        let scopes: Vec<_> = all.iter().map(|p| (p.info.id.as_str(), p.info.scope)).collect();
        assert_eq!(
            scopes,
            [
                ("sample", PluginScope::Project),
                ("demo", PluginScope::Machine),
                ("sample", PluginScope::Machine),
                ("sample", PluginScope::Builtin),
                ("agent", PluginScope::Builtin),
            ]
        );
        assert_eq!(resolve(&all, "sample").unwrap().info.scope, PluginScope::Project);
        let pane = find_pane(resolve(&all, "demo").unwrap(), "main").unwrap();
        assert_eq!(read_script(resolve(&all, "demo").unwrap(), pane).unwrap().source, "-- script");
        assert!(find_pane(resolve(&all, "demo").unwrap(), "nope").is_err());
        assert!(resolve(&all, "ghost").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_project_plugin_lists_only_for_its_own_space() {
        let root = tmp("scope");
        let (a, b, machine) = (root.join("a"), root.join("b"), root.join("machine"));
        write_plugin(&project_plugins(a.to_str().unwrap()), "demo", GOOD);
        std::fs::create_dir_all(&b).unwrap();
        let in_a = discover(a.to_str(), &machine);
        let in_b = discover(b.to_str(), &machine);
        assert!(in_a.iter().any(|p| p.info.id == "demo"));
        assert!(!in_b.iter().any(|p| p.info.id == "demo"));
        assert!(in_b.iter().any(|p| p.info.id == "sample"), "built-ins are everywhere");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_sample_builtin_reads_its_embedded_script() {
        let all = discover(None, Path::new("/nonexistent"));
        let sample = resolve(&all, "sample").unwrap();
        let pane = find_pane(sample, "counter").unwrap();
        let script = read_script(sample, pane).unwrap();
        assert_eq!(script.chunk, "sample/main.luau");
        assert!(!script.source.is_empty());
    }

    #[test]
    fn the_agent_builtin_reads_its_embedded_script_and_modules() {
        let all = discover(None, Path::new("/nonexistent"));
        let agent = resolve(&all, AGENT_VIEW_PLUGIN).unwrap();
        let pane = find_pane(agent, AGENT_VIEW_PANE).unwrap();
        assert_eq!(pane.reads, ["wheel.agent.*"]);
        assert_eq!(pane.actions, ["wheel.agent.*"]);
        let script = read_script(agent, pane).unwrap();
        assert_eq!(script.chunk, "agent/main.luau");
        assert!(script.source.contains("wheel.agent.transcript"));
        let names: Vec<&str> = script.modules.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["lib/composer.luau", "lib/diff.luau", "lib/status_header.luau", "lib/style.luau", "lib/tool_card.luau", "lib/transcript.luau"]
        );
    }

    #[test]
    fn an_agent_pane_shows_the_agent_view_and_a_terminal_shows_nothing() {
        let kind = |json: &str| serde_json::from_str::<PaneKind>(json).unwrap();
        let agent = kind(r#"{"kind":"agent","agent":"claude-1","harness":"claude"}"#);
        assert_eq!(shown_plugin(&agent), Some(("agent".to_string(), "view".to_string())));
        let plugin = kind(r#"{"kind":"plugin","plugin":"demo","pane":"main"}"#);
        assert_eq!(shown_plugin(&plugin), Some(("demo".to_string(), "main".to_string())));
        assert_eq!(shown_plugin(&kind(r#"{"kind":"terminal","term":"t1"}"#)), None);
    }

    #[test]
    fn a_project_package_with_id_agent_wins_over_the_builtin() {
        let root = tmp("agent-override");
        let (space, machine) = (root.join("space"), root.join("machine"));
        let own = GOOD.replace("demo", "agent").replace("\"main\"", "\"view\"");
        write_plugin(&project_plugins(space.to_str().unwrap()), "agent", &own);
        let all = discover(space.to_str(), &machine);
        let agent = resolve(&all, AGENT_VIEW_PLUGIN).unwrap();
        assert_eq!(agent.info.scope, PluginScope::Project);
        let pane = find_pane(agent, AGENT_VIEW_PANE).unwrap();
        assert_eq!(read_script(agent, pane).unwrap().source, "-- script");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_symlinked_script_out_of_the_package_is_not_read() {
        let root = tmp("link");
        let machine = root.join("machine");
        write_plugin(&machine, "demo", GOOD);
        std::fs::write(root.join("secret"), "secret").unwrap();
        let script = machine.join("demo/main.luau");
        std::fs::remove_file(&script).unwrap();
        std::os::unix::fs::symlink(root.join("secret"), &script).unwrap();
        let all = discover(None, &machine);
        let demo = resolve(&all, "demo").unwrap();
        assert!(read_script(demo, find_pane(demo, "main").unwrap()).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_script_over_one_megabyte_is_refused() {
        let root = tmp("big");
        let machine = root.join("machine");
        write_plugin(&machine, "demo", GOOD);
        let script = machine.join("demo/main.luau");
        std::fs::write(&script, vec![b'-'; MAX_SCRIPT as usize]).unwrap();
        let all = discover(None, &machine);
        let demo = resolve(&all, "demo").unwrap();
        let pane = find_pane(demo, "main").unwrap();
        assert_eq!(read_script(demo, pane).unwrap().source.len() as u64, MAX_SCRIPT);
        std::fs::write(&script, vec![b'-'; MAX_SCRIPT as usize + 1]).unwrap();
        let err = read_script(demo, pane).unwrap_err();
        assert!(err.contains("larger than 1 MB"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_packages_other_luau_files_come_with_the_script() {
        let root = tmp("modules");
        let machine = root.join("machine");
        write_plugin(&machine, "demo", GOOD);
        let dir = machine.join("demo");
        std::fs::create_dir_all(dir.join("lib/deep")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("lib/ui.luau"), "return 1").unwrap();
        std::fs::write(dir.join("lib/deep/x.luau"), "return 2").unwrap();
        std::fs::write(dir.join("lib/notes.md"), "no").unwrap();
        std::fs::write(dir.join(".git/hidden.luau"), "no").unwrap();
        std::fs::write(dir.join("node_modules/dep.luau"), "no").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/built.luau"), "no").unwrap();
        // Not UTF-8: left out, never a refusal of the pane.
        std::fs::write(dir.join("lib/latin1.luau"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(root.join("outside.luau"), "secret").unwrap();
        std::os::unix::fs::symlink(root.join("outside.luau"), dir.join("lib/leak.luau")).unwrap();
        let all = discover(None, &machine);
        let demo = resolve(&all, "demo").unwrap();
        let script = read_script(demo, find_pane(demo, "main").unwrap()).unwrap();
        assert_eq!(script.source, "-- script");
        let keys: Vec<&str> = script.modules.keys().map(String::as_str).collect();
        assert_eq!(keys, ["lib/deep/x.luau", "lib/ui.luau"]);
        assert_eq!(script.modules["lib/ui.luau"], "return 1");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_package_folder_that_is_a_symlink_is_found_and_its_modules_come_with_it() {
        let root = tmp("symlinked-root");
        let machine = root.join("machine");
        write_plugin(&root, "repo-package", GOOD);
        std::fs::create_dir_all(root.join("repo-package/lib")).unwrap();
        std::fs::write(root.join("repo-package/lib/ui.luau"), "return 1").unwrap();
        std::fs::create_dir_all(&machine).unwrap();
        std::os::unix::fs::symlink(root.join("repo-package"), machine.join("demo")).unwrap();
        let all = discover(None, &machine);
        let demo = resolve(&all, "demo").unwrap();
        let script = read_script(demo, find_pane(demo, "main").unwrap()).unwrap();
        assert_eq!(script.source, "-- script");
        let keys: Vec<&str> = script.modules.keys().map(String::as_str).collect();
        assert_eq!(keys, ["lib/ui.luau"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn modules_count_against_the_one_megabyte_budget() {
        let root = tmp("bigmodules");
        let machine = root.join("machine");
        write_plugin(&machine, "demo", GOOD);
        let half = MAX_SCRIPT as usize / 2;
        std::fs::write(machine.join("demo/a.luau"), vec![b'-'; half]).unwrap();
        std::fs::write(machine.join("demo/b.luau"), vec![b'-'; half]).unwrap();
        let all = discover(None, &machine);
        let demo = resolve(&all, "demo").unwrap();
        let err = read_script(demo, find_pane(demo, "main").unwrap()).unwrap_err();
        assert!(err.contains("larger than 1 MB"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_star_matches_any_run_of_characters_and_nothing_else_is_special() {
        assert!(glob_match("clock.now", "clock.now"));
        assert!(!glob_match("clock.now", "clock.nowx"));
        assert!(!glob_match("clock.now", "clock.no"));
        assert!(glob_match("clock.*", "clock.now"));
        assert!(glob_match("clock.*", "clock."));
        assert!(glob_match("*", "a.b.c"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*c", "abbbc"));
        assert!(glob_match("a*c", "ac"));
        assert!(!glob_match("a*c", "ab"));
        assert!(!glob_match("a*c", "cab"));
        assert!(glob_match("*.now", "clock.now"));
        assert!(!glob_match("*.now", "clock.nowhere"));
        assert!(glob_match("a*b*c", "a1b2c"));
        assert!(!glob_match("a*b*c", "a1c2b"));
        assert!(glob_match("a**c", "ac"));
        assert!(!glob_match("clock.*", "wheel.spaces"));
        assert!(glob_match("a?.[x]", "a?.[x]") && !glob_match("a?.[x]", "ab.x"));
        assert!(!glob_match("ab*ba", "aba"), "prefix and suffix may not overlap");
    }
}
