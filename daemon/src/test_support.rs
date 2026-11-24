//! Scratch directories for unit tests.

use std::ops::Deref;
use std::path::Path;

/// A fresh empty directory under the system temp dir, removed when dropped (also on panic).
#[derive(Debug)]
pub struct Scratch(tempfile::TempDir);

impl PartialEq<Scratch> for std::path::PathBuf {
    fn eq(&self, other: &Scratch) -> bool {
        self.as_path() == other.0.path()
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.path().as_os_str()
    }
}

pub fn scratch() -> Scratch {
    Scratch(tempfile::Builder::new().prefix("wheel-test-").tempdir().expect("create scratch dir"))
}

impl Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.0.path()
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_dir_is_empty_unique_and_removed_on_drop() {
        let (a, b) = (scratch(), scratch());
        assert_ne!(*a, *b);
        assert_eq!(std::fs::read_dir(&a).unwrap().count(), 0);
        let path = a.to_path_buf();
        drop(a);
        assert!(!path.exists());
    }
}
