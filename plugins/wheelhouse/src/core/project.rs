//! The project and pane an action or command is for.

/// Who is asking.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    /// The project's directory.
    pub dir: String,
    /// The pane, or `default` when none asked.
    pub pane: String,
}

/// The project a command run outside a request serves: the space's directory when Wheel runs it,
/// else the working directory.
pub fn here() -> String {
    std::env::var("WHEEL_SPACE_DIR")
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// A project's name: the last component of its directory.
pub fn name_of(dir: &str) -> String {
    dir.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_named_for_its_directory() {
        assert_eq!(name_of("/Users/p/wheel/"), "wheel");
    }
}
