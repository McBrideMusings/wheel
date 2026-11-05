//! The Diff view's data, three scopes of the checkout the space sits in: uncommitted work, the
//! branch against the default branch's merge-base, and the branch against the branch it was cut
//! from (up to two candidates, nearest first). Read-only: no git write, no worktree edit.

use std::process::Command;

use serde::Serialize;

use crate::core::git;

pub mod cli;
pub mod serve;

/// A file over this many lines keeps its counts and drops its hunks.
const MAX_LINES_PER_FILE: usize = 1500;
/// Beyond this many lines later files keep counts only.
const MAX_LINES_TOTAL: usize = 12000;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Line {
    /// `+`, `-` or a space.
    pub kind: char,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct File {
    pub path: String,
    pub old: Option<String>,
    /// A, D, M, R, or `?` for untracked.
    pub status: char,
    pub add: usize,
    pub del: usize,
    pub binary: bool,
    pub truncated: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Scope {
    pub key: String,
    pub label: String,
    pub base: String,
    pub fork: Option<String>,
    pub commits: Option<i64>,
    pub files: Vec<File>,
}

impl Scope {
    pub fn add(&self) -> usize {
        self.files.iter().map(|f| f.add).sum()
    }
    pub fn del(&self) -> usize {
        self.files.iter().map(|f| f.del).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub name: String,
    pub top: String,
    pub branch: Option<String>,
    pub head: String,
    pub default: Option<String>,
    pub scopes: Vec<Scope>,
    pub error: Option<String>,
}

fn header_path(line: &str) -> Option<String> {
    // diff --git a/<path> b/<path>: the path after " b/" is the new one.
    line.strip_prefix("diff --git ")?.rsplit_once(" b/").map(|(_, p)| p.to_string())
}

fn hunk_start(line: &str) -> Option<(u32, u32, String)> {
    // @@ -<old>[,n] +<new>[,n] @@ <header>
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, header) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let first = |r: &str| r.split(',').next().and_then(|n| n.parse().ok());
    Some((first(old)?, first(new)?, header.trim_start().to_string()))
}

/// `git diff` output as files with their hunks.
pub fn parse(text: &str) -> Vec<File> {
    let mut files: Vec<File> = vec![];
    let (mut o, mut n) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in text.split('\n') {
        if line.starts_with("diff --git ") {
            files.push(File { path: header_path(line).unwrap_or_default(), old: None, status: 'M', add: 0, del: 0, binary: false, truncated: false, hunks: vec![] });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if !in_hunk {
            if line.starts_with("new file mode") {
                f.status = 'A';
            } else if line.starts_with("deleted file mode") {
                f.status = 'D';
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = 'R';
                f.old = Some(p.into());
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = p.into();
            } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                f.binary = true;
            } else if line.starts_with("+++ ") && line != "+++ /dev/null" {
                f.path = line[4..].strip_prefix("b/").unwrap_or(&line[4..]).into();
            }
        }
        if let Some((a, b, header)) = hunk_start(line) {
            (o, n) = (a, b);
            in_hunk = true;
            f.hunks.push(Hunk { header, lines: vec![] });
            continue;
        }
        if !in_hunk || line.starts_with('\\') {
            continue;
        }
        let h = f.hunks.last_mut().unwrap();
        let text = line.get(1..).unwrap_or("").to_string();
        match line.chars().next() {
            Some('+') => {
                h.lines.push(Line { kind: '+', old: None, new: Some(n), text });
                n += 1;
                f.add += 1;
            }
            Some('-') => {
                h.lines.push(Line { kind: '-', old: Some(o), new: None, text });
                o += 1;
                f.del += 1;
            }
            Some(' ') => {
                h.lines.push(Line { kind: ' ', old: Some(o), new: Some(n), text });
                o += 1;
                n += 1;
            }
            _ => {}
        }
    }
    let mut budget = MAX_LINES_TOTAL;
    for f in &mut files {
        let size: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
        if size > MAX_LINES_PER_FILE || size > budget {
            f.hunks.clear();
            f.truncated = true;
        } else {
            budget -= size;
        }
    }
    files
}

/// The working tree (and untracked files) against `base`.
fn files_against(top: &str, base: &str) -> Vec<File> {
    let mut files = parse(&git::quiet(top, &["diff", "--no-color", "--no-ext-diff", "-M", "--unified=3", base, "--"]));
    for p in git::quiet(top, &["ls-files", "--others", "--exclude-standard"]).lines() {
        let out = Command::new("/usr/bin/git")
            .args(["diff", "--no-color", "--no-index", "--unified=3", "--", "/dev/null", p])
            .current_dir(top)
            .output();
        if let Ok(o) = out {
            for mut f in parse(&String::from_utf8_lossy(&o.stdout)) {
                f.path = p.into();
                f.status = '?';
                files.push(f);
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Branches HEAD could have been cut from, nearest first: fewest commits between the merge-base
/// and HEAD. A ref that already contains HEAD is a descendant, never a parent. At most two, one
/// per fork point.
fn parent_candidates(top: &str, default: &Option<String>) -> Vec<(i64, String, String)> {
    let own = git::current_branch(top);
    let refs = git::quiet(top, &["for-each-ref", "--no-contains=HEAD", "--sort=-committerdate", "--count=25", "--format=%(refname:short)", "refs/heads", "refs/remotes"]);
    let mut out = vec![];
    for r in refs.lines() {
        if r.ends_with("/HEAD") || Some(r) == own.as_deref() || own.as_deref().map_or(false, |o| r.ends_with(&format!("/{o}"))) {
            continue;
        }
        let mb = git::quiet(top, &["merge-base", r, "HEAD"]).trim().to_string();
        if mb.is_empty() {
            continue;
        }
        if let Some(n) = git::count(top, &[&format!("{mb}..HEAD")]) {
            out.push((n, default.as_deref() != Some(r), r.to_string(), mb));
        }
    }
    out.sort();
    let mut best: Vec<(i64, String, String)> = vec![];
    for (n, _, r, mb) in out {
        if best.iter().any(|(_, _, m)| *m == mb) {
            continue;
        }
        best.push((n, r, mb));
        if best.len() == 2 {
            break;
        }
    }
    best
}

fn short(sha: &str) -> String {
    sha.chars().take(9).collect()
}

/// Read project `dir`'s checkout.
pub fn collect(dir: &str) -> Snapshot {
    let top = match git::toplevel(dir) {
        Ok(t) => t,
        Err(_) => return Snapshot { name: crate::core::project::name_of(dir), top: dir.into(), error: Some(format!("{dir} is not a git checkout")), ..Default::default() },
    };
    let default = git::default_ref(&top);
    let branch = git::current_branch(&top);
    let mut scopes = vec![Scope { key: "uncommitted".into(), label: "Uncommitted".into(), base: "HEAD".into(), fork: None, commits: None, files: files_against(&top, "HEAD") }];
    if let Some(d) = &default {
        let mb = git::quiet(&top, &["merge-base", d, "HEAD"]).trim().to_string();
        if !mb.is_empty() {
            scopes.push(Scope { key: "default".into(), label: format!("Branch vs {}", d.rsplit('/').next().unwrap_or(d)), base: d.clone(),
                fork: Some(short(&mb)), commits: git::count(&top, &[&format!("{mb}..HEAD")]), files: files_against(&top, &mb) });
        }
    }
    for (i, (n, r, mb)) in parent_candidates(&top, &default).into_iter().enumerate() {
        scopes.push(Scope { key: format!("base{i}"), label: format!("vs {r}"), base: r, fork: Some(short(&mb)), commits: Some(n), files: files_against(&top, &mb) });
    }
    Snapshot { name: crate::core::project::name_of(&top), head: git::quiet(&top, &["rev-parse", "--short", "HEAD"]).trim().into(), branch, default, scopes, top, error: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@ fn main\n one\n-two\n+TWO\n three\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";

    #[test]
    fn a_diff_parses_into_files_with_numbered_lines() {
        let f = parse(SAMPLE);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].path.as_str(), f[0].add, f[0].del, f[0].status), ("src/a.rs", 1, 1, 'M'));
        assert_eq!(f[0].hunks[0].header, "fn main");
        let l = &f[0].hunks[0].lines;
        assert_eq!((l[1].kind, l[1].old, l[1].new), ('-', Some(2), None));
        assert_eq!((l[2].kind, l[2].old, l[2].new), ('+', None, Some(2)));
        assert_eq!((f[1].path.as_str(), f[1].status), ("new.txt", 'A'));
    }

    #[test]
    fn a_file_over_the_line_limit_keeps_only_its_counts() {
        let mut text = String::from("diff --git a/big b/big\n--- a/big\n+++ b/big\n@@ -0,0 +1,2000 @@\n");
        for i in 0..2000 {
            text.push_str(&format!("+line {i}\n"));
        }
        let f = parse(&text);
        assert!(f[0].truncated && f[0].hunks.is_empty());
        assert_eq!(f[0].add, 2000);
    }

    #[test]
    fn this_checkout_has_an_uncommitted_scope_first() {
        let s = collect(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(s.scopes[0].key, "uncommitted");
        assert!(s.error.is_none());
    }
}
