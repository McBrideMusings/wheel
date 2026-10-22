//! `wheel pane statusline`: the status-line command Wheel's `claude` wrapper hands Claude Code for
//! one launch. It tells the daemon the context window Claude Code reports, then prints what the
//! person's own status-line command prints for the same input, so their status line reads the
//! same in and out of Wheel.

use std::path::{Path, PathBuf};

use wheel_proto::ContextUse;

/// The context use in a status-line input: `context_window.context_window_size` and the sum of
/// `context_window.current_usage`'s token counts; 0 while `current_usage` is null (before the first
/// request, and after `/clear` or `/compact`). None when the input names no window.
pub fn context_of(input: &serde_json::Value) -> Option<ContextUse> {
    let window = &input["context_window"];
    let usage = &window["current_usage"];
    let part = |key: &str| usage[key].as_u64().unwrap_or(0);
    Some(ContextUse {
        used: part("input_tokens")
            + part("cache_creation_input_tokens")
            + part("cache_read_input_tokens")
            + part("output_tokens"),
        size: window["context_window_size"].as_u64()?,
    })
}

/// The directory a status-line input says Claude Code runs in.
pub fn cwd_of(input: &serde_json::Value) -> Option<PathBuf> {
    input["workspace"]["current_dir"]
        .as_str()
        .or_else(|| input["cwd"].as_str())
        .map(PathBuf::from)
}

/// The status-line command the person configured, as Claude Code resolves it: the project's
/// `.claude/settings.local.json`, then `.claude/settings.json`, then the user's `settings.json`
/// under `config_dir`. None when none of them sets one.
pub fn person_command(cwd: Option<&Path>, config_dir: &Path) -> Option<String> {
    let mut files = Vec::new();
    if let Some(cwd) = cwd {
        files.push(cwd.join(".claude/settings.local.json"));
        files.push(cwd.join(".claude/settings.json"));
    }
    files.push(config_dir.join("settings.json"));
    files.iter().find_map(|file| {
        let text = std::fs::read_to_string(file).ok()?;
        let settings: serde_json::Value = serde_json::from_str(&text).ok()?;
        let line = &settings["statusLine"];
        (line["type"] == "command")
            .then(|| line["command"].as_str().map(str::to_owned))
            .flatten()
    })
}

/// Claude Code's configuration directory.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_names_the_window_and_the_use() {
        let input = serde_json::json!({"context_window": {
            "context_window_size": 1_000_000,
            "current_usage": {"input_tokens": 2, "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 261_000, "output_tokens": 255}}});
        assert_eq!(
            context_of(&input),
            Some(ContextUse {
                used: 261_267,
                size: 1_000_000
            })
        );
        let before_any_request = serde_json::json!({"context_window":
            {"context_window_size": 200_000, "current_usage": null}});
        assert_eq!(
            context_of(&before_any_request),
            Some(ContextUse {
                used: 0,
                size: 200_000
            })
        );
        assert_eq!(context_of(&serde_json::json!({})), None);
    }

    #[test]
    fn the_projects_status_line_wins_over_the_users() {
        let base = std::env::temp_dir().join(format!("wheel-statusline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        let config = base.join("config");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(person_command(Some(&project), &config), None);
        std::fs::write(
            config.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"user-line"}}"#,
        )
        .unwrap();
        assert_eq!(
            person_command(Some(&project), &config).as_deref(),
            Some("user-line")
        );
        std::fs::write(
            project.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"command","command":"project-line"}}"#,
        )
        .unwrap();
        assert_eq!(
            person_command(Some(&project), &config).as_deref(),
            Some("project-line")
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
