//! Carrying a conversation's file from one agent home to another, for a reload that changes the
//! harness's home variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR`): the agent
//! resumes the conversation from the new home, so the file must be there, at the same place.

use std::path::{Path, PathBuf};

use crate::resume::Harness;

/// What a carry found in the new home, and so did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Carried {
    /// The new home's file is the old one's (a link, or a session directory both homes share).
    Same,
    /// The new home had none; the file was copied in.
    Copied(PathBuf),
    /// The new home held an earlier copy of the file; it was replaced.
    Replaced(PathBuf),
    /// The conversation has no file in the old home yet: nothing to carry.
    Nothing,
}

/// The file `harness` keeps `conversation`, started in `cwd`, in under `home`.
pub(crate) fn conversation_file(harness: Harness, home: &Path, cwd: &str, conversation: &str) -> Option<PathBuf> {
    match harness {
        Harness::Claude => Some(crate::resume::claude_project_dir(home, cwd).join(format!("{conversation}.jsonl")))
            .filter(|f| f.is_file()),
        Harness::Codex => crate::resume::codex_rollout(&home.join("sessions"), conversation),
        Harness::Pi => crate::resume::pi_session_file(home, cwd, conversation),
    }
}

/// Make `conversation`'s file in `new_home` the one in `old_home`: resume a file that is already
/// the same, copy one that is missing, replace an earlier copy. A file there with content the
/// old one lacks is never overwritten: the carry fails naming both files.
pub(crate) fn carry(
    harness: Harness,
    homes: (&Path, &Path),
    cwd: &str,
    conversation: &str,
) -> Result<Carried, String> {
    let (carried, write) = plan(harness, homes, cwd, conversation)?;
    if let Some((source, target)) = write {
        let have = std::fs::read(&source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
        write_whole(&target, &have)?;
    }
    Ok(carried)
}

/// What [`carry`] would do, writing nothing: the outcome, and the file to copy where, if any.
/// Fails where `carry` would, naming both files.
pub(crate) fn plan(
    harness: Harness,
    (old_home, new_home): (&Path, &Path),
    cwd: &str,
    conversation: &str,
) -> Result<(Carried, Option<(PathBuf, PathBuf)>), String> {
    let Some(source) = conversation_file(harness, old_home, cwd, conversation) else {
        return Ok((Carried::Nothing, None));
    };
    let target = match conversation_file(harness, new_home, cwd, conversation) {
        Some(found) => found,
        // A file outside the old home (pi's own session directory) is found from any home.
        None => match source.strip_prefix(old_home) {
            Ok(rel) => new_home.join(rel),
            Err(_) => return Ok((Carried::Same, None)),
        },
    };
    if same_file(&source, &target) {
        return Ok((Carried::Same, None));
    }
    let have = std::fs::read(&source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let carried = match std::fs::read(&target) {
        Ok(there) if have.starts_with(&there) => Carried::Replaced(target.clone()),
        Ok(_) => {
            return Err(format!(
                "{} has turns {} lacks, so the conversation was not carried to the new home; move one of them aside and reload again",
                target.display(),
                source.display()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Carried::Copied(target.clone()),
        Err(e) => return Err(format!("cannot read {}: {e}", target.display())),
    };
    Ok((carried, Some((source, target))))
}

/// Whether two paths name one file: one links to the other, or both are one path.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Write `bytes` to `path` through a file beside it and a rename, so the agent never reads half.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("conversation");
    let staged = dir.join(format!(".{name}.wheel-carry"));
    std::fs::write(&staged, bytes).map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    std::fs::rename(&staged, path).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot move {} to {}: {e}", staged.display(), path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/work/x";
    const ID: &str = "0b5c6f1e-1111-2222-3333-444455556666";

    fn file(harness: Harness, home: &Path) -> PathBuf {
        match harness {
            Harness::Claude => crate::resume::claude_project_dir(home, CWD).join(format!("{ID}.jsonl")),
            Harness::Codex => home.join(format!("sessions/2026/11/17/rollout-2026-11-17T10-00-00-{ID}.jsonl")),
            Harness::Pi => unreachable!(),
        }
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A throwaway HOME holding two homes, removed when dropped.
    fn homes() -> (crate::test_support::Scratch, PathBuf, PathBuf) {
        let dir = crate::test_support::scratch();
        let (old, new) = (dir.join(".claude"), dir.join(".claude-work"));
        (dir, old, new)
    }

    const TURNS: &str = "{\"turn\":1}\n{\"turn\":2}\n";

    #[test]
    fn a_missing_file_is_copied_in() {
        for harness in [Harness::Claude, Harness::Codex] {
            let (_dir, old, new) = homes();
            put(&file(harness, &old), TURNS);
            let carried = carry(harness, (&old, &new), CWD, ID).unwrap();
            assert_eq!(carried, Carried::Copied(file(harness, &new)), "{harness:?}");
            assert_eq!(std::fs::read_to_string(file(harness, &new)).unwrap(), TURNS);
        }
    }

    #[test]
    fn a_linked_file_is_the_same_file() {
        for harness in [Harness::Claude, Harness::Codex] {
            let (_dir, old, new) = homes();
            put(&file(harness, &old), TURNS);
            let linked = file(harness, &new);
            std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(file(harness, &old), &linked).unwrap();
            assert_eq!(carry(harness, (&old, &new), CWD, ID).unwrap(), Carried::Same, "{harness:?}");
            assert!(std::fs::symlink_metadata(&linked).unwrap().file_type().is_symlink());
        }
    }

    #[test]
    fn an_earlier_copy_is_replaced() {
        for harness in [Harness::Claude, Harness::Codex] {
            let (_dir, old, new) = homes();
            put(&file(harness, &old), TURNS);
            put(&file(harness, &new), "{\"turn\":1}\n");
            let carried = carry(harness, (&old, &new), CWD, ID).unwrap();
            assert_eq!(carried, Carried::Replaced(file(harness, &new)), "{harness:?}");
            assert_eq!(std::fs::read_to_string(file(harness, &new)).unwrap(), TURNS);
        }
    }

    #[test]
    fn a_diverged_file_is_refused_naming_both() {
        for harness in [Harness::Claude, Harness::Codex] {
            let (_dir, old, new) = homes();
            put(&file(harness, &old), TURNS);
            put(&file(harness, &new), "{\"turn\":1}\n{\"turn\":\"elsewhere\"}\n");
            let e = carry(harness, (&old, &new), CWD, ID).unwrap_err();
            assert!(e.contains(&file(harness, &old).display().to_string()), "{e}");
            assert!(e.contains(&file(harness, &new).display().to_string()), "{e}");
            assert_eq!(
                std::fs::read_to_string(file(harness, &new)).unwrap(),
                "{\"turn\":1}\n{\"turn\":\"elsewhere\"}\n"
            );
        }
    }

    #[test]
    fn a_codex_copy_on_another_day_is_found() {
        let (_dir, old, new) = homes();
        put(&file(Harness::Codex, &old), TURNS);
        let elsewhere = new.join(format!("sessions/2026/11/18/rollout-2026-11-18T09-00-00-{ID}.jsonl"));
        put(&elsewhere, "{\"turn\":1}\n");
        assert_eq!(carry(Harness::Codex, (&old, &new), CWD, ID).unwrap(), Carried::Replaced(elsewhere));
    }

    #[test]
    fn no_file_carries_nothing() {
        let (_dir, old, new) = homes();
        assert_eq!(carry(Harness::Claude, (&old, &new), CWD, ID).unwrap(), Carried::Nothing);
    }
}
