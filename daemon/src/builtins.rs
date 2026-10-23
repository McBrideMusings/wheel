//! Built-in artifacts: pages wheeld carries in its binary (`daemon/build.rs` embeds every folder of
//! the repo's `artifacts/`) and installs at machine scope, into the instance directory's
//! `artifacts/<name>/`, with an ordinary machine entry naming that folder. Once installed they are
//! plain files a person may edit; `installed.json` in the instance directory records the version
//! and the sha256 of every file Wheel wrote, which is how an update tells an edited file from one
//! it may overwrite.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wheel_proto::{ArtifactRef, ArtifactScope, BuiltinInfo, EditedFile, OnEdited};

use crate::server::AppState;

/// One built-in as the binary carries it.
pub(crate) struct Builtin {
    pub(crate) name: &'static str,
    pub(crate) version: u32,
    /// Path inside the artifact's folder, with `/` between parts, and the file's bytes.
    pub(crate) files: &'static [(&'static str, &'static [u8])],
}

pub(crate) static BUILTINS: &[Builtin] = include!(concat!(env!("OUT_DIR"), "/builtins.rs"));

/// The record of what Wheel installed, in the instance directory.
const INSTALLED: &str = "installed.json";

/// One installed built-in: its version, and the sha256 of each file as Wheel last wrote it (or as a
/// person kept it through an update).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Record {
    version: u32,
    files: BTreeMap<String, String>,
}

type Installed = BTreeMap<String, Record>;

/// What an update did: the built-ins as they now are, or the edited files it left alone because it
/// was asked to report them.
pub enum Updated {
    Done(Vec<BuiltinInfo>),
    Edited(Vec<EditedFile>),
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn shipped(name: &str) -> Result<&'static Builtin, String> {
    BUILTINS.iter().find(|b| b.name == name).ok_or_else(|| {
        format!(
            "no built-in artifact {name}: wheeld carries {}",
            BUILTINS
                .iter()
                .map(|b| b.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Where `rel` (a path the binary carries, `/` between parts) lands under `dir`.
fn under(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(dir.to_path_buf(), |p, part| p.join(part))
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The files in `dir` an update must not overwrite unasked: a recorded file whose bytes are no
/// longer what was recorded (a missing one included), and a file a person put where `shipped`
/// carries one, which Wheel never wrote. A link counts as edited either way, so nothing is ever
/// written through one.
fn edited(dir: &Path, record: &Record, shipped: &[(&str, &[u8])]) -> Vec<String> {
    let differs = |rel: &str, want: &str| {
        let path = under(dir, rel);
        is_link(&path) || std::fs::read(&path).map_or(true, |bytes| sha256(&bytes) != want)
    };
    let mut out: Vec<String> = record
        .files
        .iter()
        .filter(|(rel, hash)| differs(rel, hash))
        .map(|(rel, _)| rel.clone())
        .collect();
    for (rel, bytes) in shipped {
        if !record.files.contains_key(*rel)
            && std::fs::symlink_metadata(under(dir, rel)).is_ok()
            && differs(rel, &sha256(bytes))
        {
            out.push(rel.to_string());
        }
    }
    out.sort();
    out
}

/// What an update would make of `rel`: a unified diff from the file as it is now to `new`.
fn diff(dir: &Path, rel: &str, new: Option<&[u8]>) -> String {
    let now = std::fs::read(under(dir, rel)).unwrap_or_default();
    let now = String::from_utf8_lossy(&now);
    let new = String::from_utf8_lossy(new.unwrap_or_default());
    similar::TextDiff::from_lines(now.as_ref(), new.as_ref())
        .unified_diff()
        .context_radius(3)
        .header(&format!("{rel} (yours)"), &format!("{rel} (new)"))
        .to_string()
}

/// Write `bytes` at `path`, making its parent folders. A link there is replaced, never written
/// through.
fn put(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("make {}: {e}", parent.display()))?;
    }
    if is_link(path) {
        std::fs::remove_file(path).map_err(|e| format!("delete {}: {e}", path.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Delete `rel` under `dir`, and every folder above it inside `dir` that it leaves empty.
fn drop_file(dir: &Path, rel: &str) -> Result<(), String> {
    let path = under(dir, rel);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("delete {}: {e}", path.display()));
        }
    }
    let mut parent = path.parent();
    while let Some(p) = parent.filter(|p| *p != dir) {
        if std::fs::remove_dir(p).is_err() {
            break;
        }
        parent = p.parent();
    }
    Ok(())
}

impl AppState {
    fn installed_file(&self) -> PathBuf {
        self.instance_dir.join(INSTALLED)
    }

    /// The record, or an empty one when there is no file yet. A file that is there and cannot be
    /// read is an error, never an empty record: an install over it would forget what it held.
    fn read_installed(&self) -> Result<Installed, String> {
        let path = self.installed_file();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("{} is not a record Wheel can read: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Installed::new()),
            Err(e) => Err(format!("read {}: {e}", path.display())),
        }
    }

    /// Write the record through a temporary file and a rename, so a crash leaves the old one.
    fn write_installed(&self, installed: &Installed) -> Result<(), String> {
        let path = self.installed_file();
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let text = serde_json::to_vec_pretty(installed).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Where built-in `name` is installed: the machine scope's default folder for that name.
    fn builtin_dir(&self, name: &str) -> PathBuf {
        crate::artifacts::default_root(ArtifactScope::Machine, "", &self.instance_dir)
            .1
            .join(name)
    }

    /// Every built-in this wheeld carries, as it stands on this machine.
    pub fn builtin_ls(&self) -> Result<Vec<BuiltinInfo>, String> {
        let installed = self.read_installed()?;
        Ok(BUILTINS
            .iter()
            .map(|b| {
                let record = installed.get(b.name);
                BuiltinInfo {
                    name: b.name.to_string(),
                    shipped_version: b.version,
                    installed_version: record.map(|r| r.version),
                    edited_files: record
                        .map(|r| edited(&self.builtin_dir(b.name), r, b.files))
                        .unwrap_or_default(),
                }
            })
            .collect())
    }

    /// Install built-in `name`: its files, its machine entry, its record. Refused when the folder or
    /// a machine entry of that name is already there — an artifact Wheel did not install, or one it
    /// did, which `update` is for — so nothing a person made is overwritten.
    pub fn builtin_install(&self, name: &str) -> Result<Vec<BuiltinInfo>, String> {
        let builtin = shipped(name)?;
        let _one = self.builtins_write.lock().unwrap();
        let mut installed = self.read_installed()?;
        if installed.contains_key(name) {
            return Err(format!(
                "built-in {name} is already installed; update or remove it"
            ));
        }
        let file = self.artifact_registry(ArtifactScope::Machine, "");
        if crate::artifacts::entry(&file, name)?.is_some() {
            return Err(format!(
                "{} already has a machine artifact {name} that Wheel did not install, so the built-in is not installed over it",
                file.display()
            ));
        }
        let dir = self.default_folder(&ArtifactRef::new(ArtifactScope::Machine, name)?, "")?;
        // One atomic claim on the folder, as `artifact new` makes one.
        if let Err(e) = std::fs::create_dir(&dir) {
            return Err(match e.kind() {
                std::io::ErrorKind::AlreadyExists => format!(
                    "{} is already there and Wheel did not install it, so the built-in is not installed over it",
                    dir.display()
                ),
                _ => format!("make {}: {e}", dir.display()),
            });
        }
        let mut record = Record {
            version: builtin.version,
            files: BTreeMap::new(),
        };
        let written = builtin
            .files
            .iter()
            .try_for_each(|(rel, bytes)| {
                put(&under(&dir, rel), bytes)?;
                record.files.insert(rel.to_string(), sha256(bytes));
                Ok::<(), String>(())
            })
            .and_then(|()| {
                let fields = crate::artifacts::Declared::default()
                    .table(&crate::artifacts::written_path(&file, &dir), name);
                self.write_entry(&file, None, name, &fields)
            });
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        installed.insert(name.to_string(), record);
        if let Err(e) = self.write_installed(&installed) {
            let _ = self.delete_entry(&file, None, name);
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        tracing::info!(%name, version = builtin.version, dir = %dir.display(), files = builtin.files.len(), "built-in installed");
        self.builtin_ls()
    }

    /// Write the version of `name` this wheeld carries over the installed one — the same version
    /// again puts back what a person changed, a newer installed one is refused. A file a person
    /// edited is kept, replaced, or — with `ask` — reported with nothing changed; a file the new
    /// version drops is deleted unless it was edited and kept. The record follows every file as it
    /// is written, so an update that fails partway leaves no file it did write looking edited.
    pub fn builtin_update(&self, name: &str, on_edited: OnEdited) -> Result<Updated, String> {
        let builtin = shipped(name)?;
        let _one = self.builtins_write.lock().unwrap();
        let mut installed = self.read_installed()?;
        let Some(old) = installed.get(name).cloned() else {
            return Err(format!(
                "built-in {name} is not installed; install it first"
            ));
        };
        if old.version > builtin.version {
            return Err(format!(
                "built-in {name} {} is installed, newer than the {} this wheeld carries; nothing changed",
                old.version, builtin.version
            ));
        }
        let dir = self.builtin_dir(name);
        if is_link(&dir) {
            return Err(format!(
                "{} is a link; Wheel writes only into the folder it installed",
                dir.display()
            ));
        }
        let changed = edited(&dir, &old, builtin.files);
        let new: BTreeMap<&str, &[u8]> = builtin.files.iter().copied().collect();
        if !changed.is_empty() && on_edited == OnEdited::Ask {
            tracing::info!(%name, edited = ?changed, "built-in update asks about edited files; nothing changed");
            return Ok(Updated::Edited(
                changed
                    .iter()
                    .map(|rel| EditedFile {
                        path: rel.clone(),
                        diff: diff(&dir, rel, new.get(rel.as_str()).copied()),
                    })
                    .collect(),
            ));
        }
        let keep = |rel: &str| on_edited == OnEdited::Keep && changed.iter().any(|c| c == rel);
        // Starts as the old record and changes file by file; a kept file keeps its old hash, so it
        // still reads as edited.
        let mut record = old.clone();
        let mut written = || {
            for (rel, bytes) in &new {
                if !keep(rel) {
                    put(&under(&dir, rel), bytes)?;
                    record.files.insert(rel.to_string(), sha256(bytes));
                }
            }
            for rel in old.files.keys() {
                if !new.contains_key(rel.as_str()) && !keep(rel) {
                    drop_file(&dir, rel)?;
                    record.files.remove(rel);
                }
            }
            Ok::<(), String>(())
        };
        if let Err(e) = written() {
            installed.insert(name.to_string(), record);
            let _ = self.write_installed(&installed);
            tracing::warn!(%name, error = %e, "built-in update failed partway; the record holds what was written");
            return Err(e);
        }
        record.version = builtin.version;
        let file = self.artifact_registry(ArtifactScope::Machine, "");
        if crate::artifacts::entry(&file, name)?.is_none() {
            let fields = crate::artifacts::Declared::default()
                .table(&crate::artifacts::written_path(&file, &dir), name);
            self.write_entry(&file, None, name, &fields)?;
        }
        installed.insert(name.to_string(), record);
        self.write_installed(&installed)?;
        tracing::info!(%name, from = old.version, to = builtin.version, edited = ?changed, ?on_edited, "built-in updated");
        Ok(Updated::Done(self.builtin_ls()?))
    }

    /// Remove installed built-in `name`: its machine entry, its folder and its record.
    pub fn builtin_remove(&self, name: &str) -> Result<Vec<BuiltinInfo>, String> {
        shipped(name)?;
        let _one = self.builtins_write.lock().unwrap();
        let mut installed = self.read_installed()?;
        if installed.remove(name).is_none() {
            return Err(format!("built-in {name} is not installed"));
        }
        let file = self.artifact_registry(ArtifactScope::Machine, "");
        if crate::artifacts::entry(&file, name)?.is_some() {
            self.delete_entry(&file, None, name)?;
        }
        let dir = self.builtin_dir(name);
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("delete {}: {e}", dir.display()));
            }
        }
        self.write_installed(&installed)?;
        self.announce_gone(ArtifactScope::Machine, "", name);
        tracing::info!(%name, dir = %dir.display(), "built-in removed");
        self.builtin_ls()
    }
}
