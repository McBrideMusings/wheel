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
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_directory_outside_any_repository_is_its_own_primary() {
        let root = scratch("none");
        assert_eq!(primary(&root), root);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
