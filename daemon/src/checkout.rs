//! Which checkout a directory belongs to. A linked git worktree has a `.git` file naming the
//! primary checkout's `.git/worktrees/<name>`; the person's per-directory setup (the profile a
//! terminal picks up) is made for the primary checkout, so a worktree is asked as that.

use std::path::{Path, PathBuf};

/// The primary checkout of the repository `dir` is inside, when `dir` is in a linked worktree;
/// the root of the checkout `dir` is in otherwise, and `dir` itself outside any repository.
pub fn primary(dir: &Path) -> PathBuf {
    for ancestor in dir.ancestors() {
        let dot_git = ancestor.join(".git");
        if dot_git.is_dir() {
            return ancestor.to_path_buf();
        }
        let Ok(pointer) = std::fs::read_to_string(&dot_git) else {
            continue;
        };
        let Some(git_dir) = pointer.trim().strip_prefix("gitdir:") else {
            continue;
        };
        // A worktree's git directory is <primary>/.git/worktrees/<name>.
        let git_dir = ancestor.join(git_dir.trim());
        let primary = git_dir
            .parent()
            .filter(|dir| dir.file_name().is_some_and(|n| n == "worktrees"))
            .and_then(Path::parent)
            .filter(|dir| dir.file_name().is_some_and(|n| n == ".git"))
            .and_then(Path::parent);
        return primary.unwrap_or(ancestor).to_path_buf();
    }
    dir.to_path_buf()
}

/// Whether `dir` is the root of a linked git worktree: its own `.git` is a file that names a
/// `<primary>/.git/worktrees/<name>` directory, which a primary checkout's `.git` folder never is.
pub fn is_linked_worktree(dir: &Path) -> bool {
    let Ok(pointer) = std::fs::read_to_string(dir.join(".git")) else {
        return false;
    };
    pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(|g| Path::new(g.trim()))
        .and_then(Path::parent)
        .is_some_and(|p| p.file_name().is_some_and(|n| n == "worktrees"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-checkout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_linked_worktree_is_asked_as_its_primary_checkout() {
        let root = scratch("linked");
        let primary_dir = root.join("Work/app");
        std::fs::create_dir_all(primary_dir.join(".git/worktrees/feat")).unwrap();
        let tree = root.join("worktrees/app/feat");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        std::fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", primary_dir.join(".git/worktrees/feat").display()),
        )
        .unwrap();
        assert_eq!(primary(&tree), primary_dir);
        assert_eq!(primary(&tree.join("src")), primary_dir);
        assert_eq!(primary(&primary_dir), primary_dir);
        assert!(is_linked_worktree(&tree));
        assert!(!is_linked_worktree(&tree.join("src")));
        assert!(!is_linked_worktree(&primary_dir));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn a_directory_outside_any_repository_is_its_own_primary() {
        let root = scratch("none");
        assert_eq!(primary(&root), root);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// The branch checked out in `dir` and the lines added and removed there against HEAD, untracked
/// files not counted. None for the branch outside a repository or on a detached head; zeros when
/// git cannot say.
pub async fn changes(dir: &Path) -> (Option<String>, u32, u32) {
    let git = |args: &'static [&'static str]| async move {
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    // Outside a repository `git diff` compares two paths instead, so nothing runs there.
    if git(&["rev-parse", "--is-inside-work-tree"]).await.is_none() {
        return (None, 0, 0);
    }
    let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    let (mut added, mut removed) = (0u32, 0u32);
    for line in git(&["diff", "HEAD", "--numstat"]).await.unwrap_or_default().lines() {
        let mut cols = line.split('\t');
        added += cols.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        removed += cols.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    }
    (branch, added, removed)
}

#[cfg(test)]
mod changes_tests {
    use super::changes;

    #[tokio::test]
    async fn a_repository_reports_its_branch_and_the_lines_changed_against_head() {
        let dir = std::env::temp_dir().join(format!("wheel-footer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C").arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status().unwrap().success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "work"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("a.txt"), "one\nthree\nfour\n").unwrap();
        let found = changes(&dir).await;
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, (Some("work".into()), 2, 1));
    }

    #[tokio::test]
    async fn a_directory_outside_a_repository_has_no_branch() {
        let dir = std::env::temp_dir().join(format!("wheel-footer-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let found = changes(&dir).await;
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, (None, 0, 0));
    }
}
