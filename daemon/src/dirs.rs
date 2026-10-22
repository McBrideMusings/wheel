//! The folders a person can make a space for: browsing the daemon's disk, and the roots browsing
//! starts from. Every client and the CLI read the same answer here.

use std::path::Path;

use wheel_proto::DirEntry;

use crate::server::AppState;

/// `~` and `~/…` start at the daemon user's home; any other path is returned as it came.
pub(crate) fn expand_home(path: &str) -> String {
    let home = || std::env::var("HOME").unwrap_or_else(|_| "/".into());
    match path {
        "~" => home(),
        _ => match path.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", home().trim_end_matches('/')),
            None => path.to_string(),
        },
    }
}

/// The subfolders of `dir` sorted by name, hidden ones only when `dir` itself is hidden, or why
/// the folder cannot be listed. `open` maps an absolute path to the id of the space open there.
fn list(
    path: &str,
    dir: &Path,
    open: impl Fn(&str) -> Option<String>,
) -> Result<Vec<DirEntry>, String> {
    let show_hidden = dir
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'));
    let read = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => format!("no folder at {path}"),
    })?;
    let mut dirs: Vec<DirEntry> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            // `metadata` follows a symlink, so a link to a folder lists as the folder.
            std::fs::metadata(entry.path())
                .ok()?
                .is_dir()
                .then_some(())?;
            let path = entry.path().to_string_lossy().into_owned();
            Some(DirEntry {
                space: open(&path),
                name,
                path,
            })
        })
        .collect();
    dirs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(dirs)
}

impl AppState {
    /// Answer `dir_list`: the expanded path, its subfolders, and the reason when it has none to
    /// give. Logs the path and how many it returned.
    pub(crate) fn dir_list(&self, path: &str) -> (String, Vec<DirEntry>, Option<String>) {
        let expanded = expand_home(path.trim());
        let result = if Path::new(&expanded).is_absolute() {
            let open: Vec<(String, String)> = {
                let spaces = self.spaces.lock().unwrap();
                spaces
                    .spaces
                    .iter()
                    .map(|s| (s.cwd.clone(), s.id.clone()))
                    .collect()
            };
            list(&expanded, Path::new(&expanded), |p| {
                open.iter()
                    .find(|(cwd, _)| cwd == p)
                    .map(|(_, id)| id.clone())
            })
        } else {
            Err(format!("no folder at {expanded}"))
        };
        match result {
            Ok(dirs) => {
                tracing::info!(path = %expanded, count = dirs.len(), "dir_list");
                (expanded, dirs, None)
            }
            Err(error) => {
                tracing::info!(path = %expanded, count = 0, %error, "dir_list refused");
                (expanded, Vec::new(), Some(error))
            }
        }
    }

    /// Answer `dir_roots`: `spaces.roots` with `~` resolved, each root once.
    pub(crate) fn dir_roots(&self) -> Vec<String> {
        let value = self
            .setting(crate::settings::SPACES_ROOTS, None)
            .unwrap_or_default();
        let mut roots: Vec<String> = Vec::new();
        for root in value.split(',').map(str::trim).filter(|r| !r.is_empty()) {
            let root = expand_home(root);
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        tracing::info!(count = roots.len(), ?roots, "dir_roots");
        roots
    }

    /// The `spaces.roots` default: `~` and the parent folder of every recent space, each once.
    pub(crate) fn default_roots(&self) -> String {
        let home = expand_home("~");
        let mut roots = vec!["~".to_string()];
        for recent in self.recent.lock().unwrap().iter() {
            let Some(parent) = Path::new(&recent.dir).parent() else {
                continue;
            };
            let parent = parent.to_string_lossy().into_owned();
            let root = if parent == home {
                "~".to_string()
            } else {
                parent
            };
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots.join(",")
    }
}
