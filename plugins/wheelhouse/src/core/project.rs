//! The project, pane and view a request is for.
//!
//! The host (wheeld) names all three in `_meta` on each call and read: `wheel/dir` (the project),
//! `wheel/pane` (the asking pane) and `wheel/artifact` (the artifact whose pane asks, which is the
//! view's name). A process run with no host has no `_meta`; it serves the project in its own
//! directory.

use rmcp::model::RequestMetaObject;

/// Who is asking.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    /// The project's directory.
    pub dir: String,
    /// The pane, or `default` when the host names none.
    pub pane: String,
    /// The artifact (view) name, when the host names one.
    pub artifact: Option<String>,
}

impl Origin {
    pub fn of(meta: &RequestMetaObject) -> Self {
        Self::from_map(&meta.0 .0)
    }

    pub fn from_map(map: &serde_json::Map<String, serde_json::Value>) -> Self {
        let get = |k: &str| map.get(k).and_then(|v| v.as_str()).map(String::from);
        let dir = get("wheel/dir")
            .or_else(|| std::env::var("WHEEL_SPACE_DIR").ok())
            .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_default();
        Origin { dir, pane: get("wheel/pane").unwrap_or_else(|| "default".into()), artifact: get("wheel/artifact") }
    }
}

/// A project's name: the last component of its directory.
pub fn name_of(dir: &str) -> String {
    dir.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// The view and project a resource URI names: `a2ui://<view>?dir=<project>`.
pub fn parse_uri(uri: &str) -> Option<(String, Option<String>)> {
    let u = url::Url::parse(uri).ok()?;
    let dir = u.query_pairs().find(|(k, _)| k == "dir").map(|(_, v)| v.into_owned());
    Some((u.host_str()?.to_string(), dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn origin_reads_the_hosts_meta() {
        let m = json!({ "wheel/dir": "/p/a", "wheel/pane": "p7", "wheel/artifact": "beads" });
        let o = Origin::from_map(m.as_object().unwrap());
        assert_eq!(o, Origin { dir: "/p/a".into(), pane: "p7".into(), artifact: Some("beads".into()) });
    }

    #[test]
    fn a_uri_names_its_view_and_project() {
        assert_eq!(parse_uri("a2ui://beads?dir=%2Fp%2Fa"), Some(("beads".into(), Some("/p/a".into()))));
        assert_eq!(parse_uri("a2ui://diff"), Some(("diff".into(), None)));
    }

    #[test]
    fn a_project_is_named_for_its_directory() {
        assert_eq!(name_of("/Users/p/wheel/"), "wheel");
    }
}
