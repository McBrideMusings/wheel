//! Embeds the built-in artifacts: every folder under the repo's `artifacts/` holding a
//! `builtin.toml` (`version = N`) becomes one entry of `OUT_DIR/builtins.rs`, its files carried in
//! the binary with `include_bytes!`. `builtin.toml` itself is the description, not a file of the
//! artifact, so it is not among them.
//!
//! Also names the server's source: `WHEELD_SOURCE_HASH` is a SHA-256 over every file that makes
//! wheeld (its crate, the protocol crate and the embedded built-ins), path and bytes, so two
//! builds of the same source carry the same hash. The app compares the running wheeld's with the
//! bundled one's to say a restart would change the server (ADR-0020).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../artifacts");
    println!("cargo:rerun-if-changed={}", root.display());
    // Every file the hash covers, as (path from the repo root, absolute path).
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    let mut out = String::from("&[\n");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|it| {
            it.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let described = dir.join("builtin.toml");
        // A folder without one is a page a machine entry names, not a built-in.
        let text = match std::fs::read_to_string(&described) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => panic!("{}: {e}", described.display()),
        };
        let version = text
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("version")?
                    .trim()
                    .strip_prefix('=')?
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
            .unwrap_or_else(|| panic!("{} has no `version = N` line", described.display()));
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        files.sort();
        sources.extend(
            files
                .iter()
                .map(|(rel, abs)| (format!("artifacts/{name}/{rel}"), abs.clone())),
        );
        writeln!(
            out,
            "    Builtin {{ name: {name:?}, version: {version}, files: &["
        )
        .unwrap();
        for (rel, abs) in files {
            if rel == "builtin.toml" {
                continue;
            }
            writeln!(
                out,
                "        ({rel:?}, include_bytes!({:?})),",
                abs.display().to_string()
            )
            .unwrap();
        }
        writeln!(out, "    ] }},").unwrap();
    }
    out.push(']');
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("builtins.rs");
    std::fs::write(dest, out).unwrap();

    for (tree, dir) in [("daemon/src", "src"), ("proto/src", "../proto/src")] {
        let dir = manifest.join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        sources.extend(
            files
                .into_iter()
                .map(|(rel, abs)| (format!("{tree}/{rel}"), abs)),
        );
    }
    for (name, file) in [
        ("daemon/Cargo.toml", "Cargo.toml"),
        ("daemon/build.rs", "build.rs"),
        ("proto/Cargo.toml", "../proto/Cargo.toml"),
    ] {
        let file = manifest.join(file);
        println!("cargo:rerun-if-changed={}", file.display());
        sources.push((name.to_string(), file));
    }
    sources.sort();
    let mut hash = Sha256::new();
    for (name, file) in &sources {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=WHEELD_SOURCE_HASH={}", &hex[..16]);
}

/// Every file under `dir`, as its path relative to `base` with `/` between parts, and its absolute path.
fn walk(base: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // Links are not followed: what a built-in carries is what its folder holds.
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            panic!("{} is a link; the build reads only files", path.display());
        }
        if kind.is_dir() {
            walk(base, &path, files);
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((rel, path.canonicalize().unwrap()));
        }
    }
}
