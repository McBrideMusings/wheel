//! What this daemon can reach on the machine: its own program path and the folders macOS guards.

use wheel_proto::{FolderAccess, HostAccess};

/// The folders macOS asks the person about before an app may open them. A daemon started by
/// launchd cannot show that question, so it fails until the person grants the app Full Disk
/// Access or the folder.
const GUARDED: [&str; 3] = ["Downloads", "Desktop", "Documents"];

/// The running program and whether each guarded folder lists, read now.
pub fn access() -> HostAccess {
    access_in(std::env::var_os("HOME").map(std::path::PathBuf::from))
}

fn access_in(home: Option<std::path::PathBuf>) -> HostAccess {
    let folders: Vec<FolderAccess> = GUARDED
        .iter()
        .map(|name| {
            let path = home
                .as_ref()
                .map_or_else(|| std::path::PathBuf::from(name), |home| home.join(name));
            // A folder that is not there is not a denial.
            let error = std::fs::read_dir(&path)
                .err()
                .filter(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
                .map(|e| e.to_string());
            FolderAccess {
                path: path.display().to_string(),
                allowed: error.is_none(),
                error,
            }
        })
        .collect();
    for folder in folders.iter().filter(|f| !f.allowed) {
        tracing::debug!(path = %folder.path, error = ?folder.error, "guarded folder cannot be listed");
    }
    HostAccess {
        exe: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        source: env!("WHEELD_SOURCE_HASH").to_string(),
        folders,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Removes the directory on the way out, even when an assertion fails.
    struct Home(std::path::PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_folder_the_daemon_may_not_list_is_denied_and_a_missing_one_is_not() {
        let dir = std::env::temp_dir().join(format!("wheel-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let home = Home(dir);
        std::fs::create_dir(home.0.join("Downloads")).unwrap();
        std::fs::create_dir(home.0.join("Desktop")).unwrap();
        std::fs::set_permissions(
            home.0.join("Downloads"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let got = access_in(Some(home.0.clone()));
        std::fs::set_permissions(
            home.0.join("Downloads"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let allowed: Vec<bool> = got.folders.iter().map(|f| f.allowed).collect();
        assert_eq!(allowed, [false, true, true]);
        assert!(got.folders[0]
            .error
            .as_deref()
            .unwrap()
            .contains("ermission denied"));
        assert!(got.folders[1].error.is_none());
        assert_eq!(got.source, env!("WHEELD_SOURCE_HASH"));
        assert_eq!(got.source.len(), 16);
        assert!(got.exe.ends_with(
            std::env::current_exe()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        ));
    }
}
