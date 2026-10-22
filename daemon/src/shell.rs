//! How the programs wheeld starts learn they are inside Wheel. Every terminal and agent process
//! gets `WHEEL_ENV`, `WHEEL_URL`, `WHEEL_TOKEN_FILE`, `WHEEL_SPACE` and `WHEEL_PANE`, so the `wheel`
//! command reaches this daemon from any of them without a flag.
//!
//! A terminal also gets shell integration: zsh starts from a `ZDOTDIR` and bash from `--rcfile`,
//! each running the person's own startup files first and then putting Wheel's bin directory on
//! `PATH`. That directory holds `claude`, `codex` and `pi` wrappers that add the brief to the real
//! program's instructions, and a `wheel` link to the CLI the app bundle ships. The files live in
//! the instance directory and are written again on every start.

use std::path::{Path, PathBuf};

const ZSHENV: &str = include_str!("shell/zshenv");
const ZPROFILE: &str = include_str!("shell/zprofile");
const ZSHRC: &str = include_str!("shell/zshrc");
const BASHRC: &str = include_str!("shell/bashrc");
const AGENT_WRAPPER: &str = include_str!("shell/agent-wrapper");
const BRIEF: &str = include_str!("brief.md");

/// Where the `wheel` command finds this daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// The daemon's WebSocket URL.
    pub url: String,
    /// The file holding the client token.
    pub token_file: PathBuf,
}

/// How to start one process: the program (the user's login shell when None), its arguments and its
/// environment.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Launch {
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// `claude` takes the brief as a file; subcommands that start no session do not take it.
const CLAUDE_EXEC: &str = r#"exec "$real" --append-system-prompt-file "$WHEEL_BRIEF_FILE" "$@""#;
const CLAUDE_SKIP: &str =
    "mcp|doctor|update|install|auth|plugin|config|setup-token|agents|migrate-installer|--help|-h|--version|-v";
/// `codex` takes instructions as a config value; its `developer_instructions` key adds to the
/// session's instructions where `model_instructions_file` would replace them.
const CODEX_EXEC: &str =
    r#"exec "$real" -c "developer_instructions='''$(cat "$WHEEL_BRIEF_FILE")'''" "$@""#;
const CODEX_SKIP: &str =
    "completion|update|doctor|login|logout|mcp|plugin|help|features|--help|-h|--version|-V";

/// `pi` appends the text of a file it is handed, so the brief goes in as a file.
const PI_EXEC: &str = r#"exec "$real" --append-system-prompt "$WHEEL_BRIEF_FILE" "$@""#;
const PI_SKIP: &str = "install|remove|uninstall|update|list|config|auth|mcp|--help|-h|--version|-v";

fn wrapper(name: &str, skip: &str, exec: &str) -> String {
    AGENT_WRAPPER
        .replace("@NAME@", name)
        .replace("@SKIP@", skip)
        .replace("@EXEC@", exec)
}

/// The directory holding the shell integration inside `instance_dir`.
pub(crate) fn dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join("shell")
}

/// The bin directory a terminal puts on its `PATH`.
pub(crate) fn bin(instance_dir: &Path) -> PathBuf {
    dir(instance_dir).join("bin")
}

fn write(path: &Path, text: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Replaced through a rename, so a shell starting now reads the old file or the new, never half.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)
}

/// Write the shell integration under `instance_dir`. `cli` is the `wheel` program to link into the
/// bin directory when there is one.
pub(crate) fn install(instance_dir: &Path, cli: Option<&Path>) -> std::io::Result<()> {
    let root = dir(instance_dir);
    write(&root.join("zsh/.zshenv"), ZSHENV, 0o644)?;
    write(&root.join("zsh/.zprofile"), ZPROFILE, 0o644)?;
    write(&root.join("zsh/.zshrc"), ZSHRC, 0o644)?;
    write(&root.join("bash/rcfile"), BASHRC, 0o644)?;
    write(&root.join("brief.md"), BRIEF, 0o644)?;
    let bin = bin(instance_dir);
    write(
        &bin.join("claude"),
        &wrapper("claude", CLAUDE_SKIP, CLAUDE_EXEC),
        0o755,
    )?;
    write(
        &bin.join("codex"),
        &wrapper("codex", CODEX_SKIP, CODEX_EXEC),
        0o755,
    )?;
    write(&bin.join("pi"), &wrapper("pi", PI_SKIP, PI_EXEC), 0o755)?;
    let link = bin.join("wheel");
    let _ = std::fs::remove_file(&link);
    if let Some(cli) = cli {
        std::os::unix::fs::symlink(cli, &link)?;
    }
    Ok(())
}

/// Where a terminal or agent sits: its space's id, its pane's id and, for a terminal, the space's
/// directory.
pub(crate) type Seat<'a> = (&'a str, &'a str, Option<&'a str>);

/// The environment `wheel` reads, for the process in `pane` of `space` (a terminal opened by a
/// client has neither).
pub(crate) fn env(
    control: &Control,
    instance_dir: &Path,
    seat: Option<Seat>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("WHEEL_ENV".to_string(), "1".to_string()),
        ("WHEEL_URL".to_string(), control.url.clone()),
        (
            "WHEEL_TOKEN_FILE".to_string(),
            control.token_file.display().to_string(),
        ),
        (
            "WHEEL_BIN".to_string(),
            bin(instance_dir).display().to_string(),
        ),
        (
            "WHEEL_BRIEF_FILE".to_string(),
            dir(instance_dir).join("brief.md").display().to_string(),
        ),
    ];
    if let Some((space, pane, space_dir)) = seat {
        env.push(("WHEEL_SPACE".into(), space.into()));
        env.push(("WHEEL_PANE".into(), pane.into()));
        if let Some(dir) = space_dir {
            env.push(("WHEEL_SPACE_DIR".into(), dir.into()));
        }
    }
    env
}

/// How to start a terminal whose shell is `shell` (the login shell when None): the environment plus
/// the shell's own way to run Wheel's startup files. `path` is the daemon's PATH, which the shell
/// starts with; `login_shell` is `$SHELL`, which names the login shell; `user_zdotdir` is the
/// `ZDOTDIR` the daemon runs with.
pub(crate) fn launch(
    shell: Option<String>,
    path: &str,
    login_shell: Option<&str>,
    user_zdotdir: Option<&str>,
    control: &Control,
    instance_dir: &Path,
    seat: Option<Seat>,
) -> Launch {
    let mut env = env(control, instance_dir, seat);
    let which = shell
        .clone()
        .or(login_shell.map(String::from))
        .unwrap_or_default();
    let name = Path::new(&which)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let mut launch = Launch {
        program: shell,
        ..Default::default()
    };
    // zsh and bash put the bin directory on PATH in Wheel's startup files, after the person's.
    if matches!(name, "zsh" | "bash") {
        env.push(("PATH".into(), path.into()));
    }
    match name {
        "zsh" => {
            if let Some(dir) = user_zdotdir.filter(|d| !d.is_empty()) {
                env.push(("WHEEL_USER_ZDOTDIR".into(), dir.into()));
            }
            env.push((
                "ZDOTDIR".into(),
                dir(instance_dir).join("zsh").display().to_string(),
            ));
        }
        "bash" => {
            launch.program = Some(which);
            launch.args = vec![
                "--rcfile".into(),
                dir(instance_dir).join("bash/rcfile").display().to_string(),
            ];
        }
        // Another shell has no startup hook here; its PATH starts with the bin directory, which
        // its own startup files may reorder.
        _ => {
            env.push((
                "PATH".into(),
                format!("{}:{path}", bin(instance_dir).display()),
            ));
        }
    }
    launch.env = env;
    launch
}

/// The `PATH` an agent process runs with: Wheel's bin directory (so `wheel` resolves), then the
/// daemon's `path`.
pub(crate) fn agent_path(instance_dir: &Path, path: &str) -> String {
    format!("{}:{path}", bin(instance_dir).display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control() -> Control {
        Control {
            url: "ws://127.0.0.1:7420/ws".into(),
            token_file: "/s/token".into(),
        }
    }

    fn get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_pane_terminal_gets_its_seat_in_the_environment() {
        let env = env(
            &control(),
            Path::new("/s"),
            Some(("s1", "p4", Some("/work"))),
        );
        assert_eq!(get(&env, "WHEEL_SPACE_DIR"), Some("/work"));
        assert_eq!(get(&env, "WHEEL_ENV"), Some("1"));
        assert_eq!(get(&env, "WHEEL_URL"), Some("ws://127.0.0.1:7420/ws"));
        assert_eq!(get(&env, "WHEEL_TOKEN_FILE"), Some("/s/token"));
        assert_eq!(get(&env, "WHEEL_SPACE"), Some("s1"));
        assert_eq!(get(&env, "WHEEL_PANE"), Some("p4"));
    }

    #[test]
    fn a_client_terminal_has_no_seat() {
        let env = env(&control(), Path::new("/s"), None);
        assert_eq!(get(&env, "WHEEL_SPACE"), None);
        assert_eq!(get(&env, "WHEEL_PANE"), None);
    }

    #[test]
    fn zsh_starts_from_wheels_zdotdir_and_keeps_the_persons() {
        let l = launch(
            None,
            "/usr/bin:/bin",
            Some("/bin/zsh"),
            Some("/u/z"),
            &control(),
            Path::new("/s"),
            None,
        );
        assert_eq!(l.program, None);
        assert_eq!(get(&l.env, "ZDOTDIR"), Some("/s/shell/zsh"));
        assert_eq!(get(&l.env, "WHEEL_USER_ZDOTDIR"), Some("/u/z"));
    }

    #[test]
    fn bash_starts_from_wheels_rcfile() {
        let l = launch(
            None,
            "/usr/bin:/bin",
            Some("/bin/bash"),
            None,
            &control(),
            Path::new("/s"),
            None,
        );
        assert_eq!(l.program.as_deref(), Some("/bin/bash"));
        assert_eq!(l.args, ["--rcfile", "/s/shell/bash/rcfile"]);
        assert_eq!(get(&l.env, "ZDOTDIR"), None);
    }

    #[test]
    fn a_configured_shell_decides_over_the_login_shell() {
        let l = launch(
            Some("/opt/homebrew/bin/bash".into()),
            "/usr/bin:/bin",
            Some("/bin/zsh"),
            None,
            &control(),
            Path::new("/s"),
            None,
        );
        assert_eq!(l.program.as_deref(), Some("/opt/homebrew/bin/bash"));
        assert_eq!(l.args.len(), 2);
    }

    #[test]
    fn other_shells_get_the_bin_directory_on_path() {
        let l = launch(
            None,
            "/usr/bin:/bin",
            Some("/usr/bin/fish"),
            None,
            &control(),
            Path::new("/s"),
            None,
        );
        assert!(get(&l.env, "PATH").unwrap().starts_with("/s/shell/bin:"));
    }

    #[test]
    fn the_brief_fits_codex_s_literal_string() {
        assert!(!BRIEF.contains("'''"));
    }

    fn sh(script: &str, env: &[(&str, &str)]) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .envs(env.iter().copied())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fake_tool(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\necho \"real $*\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_claude_wrapper_adds_the_brief_file_inside_wheel_and_passes_through_outside() {
        let root = std::env::temp_dir().join(format!("wheel-shell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        install(&root, None).unwrap();
        let real = root.join("real");
        fake_tool(&real, "claude");
        let path = format!("{}:{}:/usr/bin:/bin", bin(&root).display(), real.display());
        let inside = sh(
            "claude -p hi",
            &[
                ("PATH", &path),
                ("WHEEL_ENV", "1"),
                (
                    "WHEEL_BRIEF_FILE",
                    dir(&root).join("brief.md").to_str().unwrap(),
                ),
            ],
        );
        assert!(
            inside.contains("--append-system-prompt-file") && inside.contains("brief.md -p hi"),
            "{inside}"
        );
        let outside = sh("claude -p hi", &[("PATH", &path)]);
        assert!(!outside.contains("append-system-prompt"), "{outside}");
        let sub = sh(
            "claude mcp list",
            &[
                ("PATH", &path),
                ("WHEEL_ENV", "1"),
                (
                    "WHEEL_BRIEF_FILE",
                    dir(&root).join("brief.md").to_str().unwrap(),
                ),
            ],
        );
        assert!(!sub.contains("append-system-prompt"), "{sub}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_codex_wrapper_passes_the_brief_as_developer_instructions() {
        let root = std::env::temp_dir().join(format!("wheel-shell-codex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        install(&root, None).unwrap();
        let real = root.join("real");
        fake_tool(&real, "codex");
        let path = format!("{}:{}:/usr/bin:/bin", bin(&root).display(), real.display());
        let out = sh(
            "codex fix it",
            &[
                ("PATH", &path),
                ("WHEEL_ENV", "1"),
                (
                    "WHEEL_BRIEF_FILE",
                    dir(&root).join("brief.md").to_str().unwrap(),
                ),
            ],
        );
        assert!(
            out.contains("developer_instructions='''You are running inside Wheel"),
            "{out}"
        );
        assert!(out.trim_end().ends_with("fix it"), "{out}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pi_wrapper_appends_the_brief_inside_wheel_and_skips_subcommands() {
        let root = std::env::temp_dir().join(format!("wheel-shell-pi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        install(&root, None).unwrap();
        let real = root.join("real");
        fake_tool(&real, "pi");
        let path = format!("{}:{}:/usr/bin:/bin", bin(&root).display(), real.display());
        let brief = dir(&root).join("brief.md");
        let env = [
            ("PATH", path.as_str()),
            ("WHEEL_ENV", "1"),
            ("WHEEL_BRIEF_FILE", brief.to_str().unwrap()),
        ];
        let inside = sh("pi fix it", &env);
        assert!(
            inside.contains("--append-system-prompt") && inside.contains("brief.md fix it"),
            "{inside}"
        );
        let sub = sh("pi install foo", &env);
        assert!(!sub.contains("append-system-prompt"), "{sub}");
        let outside = sh("pi fix it", &[("PATH", &path)]);
        assert!(!outside.contains("append-system-prompt"), "{outside}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run `shell` interactively through `launch` with `HOME` at a directory whose startup files
    /// leave a mark and put their own directory first on `PATH`; return what it printed.
    fn run_shell(shell: &str, name: &str, extra: &[(&str, &str)]) -> String {
        let root = std::env::temp_dir().join(format!("wheel-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        for file in [".zshenv", ".zshrc", ".bash_profile"] {
            std::fs::write(
                home.join(file),
                format!("MARK=\"${{MARK}}{file} \"; export MARK\nPATH=/user/bin:$PATH\n"),
            )
            .unwrap();
        }
        for (file, text) in extra {
            let path = home.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let instance = root.join("inst");
        install(&instance, None).unwrap();
        let launch = launch(
            Some(shell.into()),
            "/usr/bin:/bin",
            None,
            None,
            &control(),
            &instance,
            Some(("s1", "p2", None)),
        );
        let mut cmd = std::process::Command::new(launch.program.as_deref().unwrap());
        cmd.args(&launch.args)
            .args(["-i", "-c", "echo \"mark=$MARK\"; echo \"path=$PATH\"; echo \"pane=$WHEEL_PANE zdotdir=${ZDOTDIR-unset}\""])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .envs(launch.env.iter().cloned());
        let out = cmd.output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn zsh_runs_the_persons_files_and_then_puts_the_bin_directory_first() {
        let out = run_shell("/bin/zsh", "zsh", &[]);
        assert!(out.contains("mark=.zshenv .zshrc"), "{out}");
        assert!(
            out.contains("path=") && out.contains("shell/bin:/user/bin:"),
            "{out}"
        );
        assert!(out.contains("pane=p2 zdotdir=unset"), "{out}");
    }

    #[test]
    fn bash_runs_the_persons_files_and_then_puts_the_bin_directory_first() {
        let out = run_shell("/bin/bash", "bash", &[]);
        assert!(out.contains("mark=.bash_profile"), "{out}");
        assert!(out.contains("shell/bin:/user/bin:"), "{out}");
        assert!(out.contains("pane=p2"), "{out}");
    }

    #[test]
    fn zsh_follows_a_zshenv_that_moves_zdotdir() {
        let out = run_shell(
            "/bin/zsh",
            "zsh-moved",
            &[
                (".zshenv", "ZDOTDIR=\"$HOME/zd\"\n"),
                ("zd/.zshrc", "MARK=\"${MARK}zd-zshrc \"; export MARK\n"),
            ],
        );
        assert!(out.contains("mark=zd-zshrc"), "{out}");
        assert!(out.contains("shell/bin:"), "{out}");
        assert!(
            out.contains("zdotdir=") && !out.contains("zdotdir=unset"),
            "{out}"
        );
    }
}
