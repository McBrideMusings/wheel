//! Embeds the built-in artifacts: every folder under the repo's `artifacts/` holding a
//! `builtin.toml` (`version = N`) becomes one entry of `OUT_DIR/builtins.rs`, its files carried in
//! the binary with `include_bytes!`. `builtin.toml` itself is the description, not a file of the
//! artifact, so it is not among them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../artifacts");
    println!("cargo:rerun-if-changed={}", root.display());
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
            panic!(
                "{} is a link; a built-in carries only files",
                path.display()
            );
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
