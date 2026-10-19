//! The permission policy (ADR-0007): a supervisor's workers ask permission like any agent, and the
//! policy answers what is plainly the worker's own work — an edit inside its worktree, one of the
//! project's declared commands, `bd` on its own bead, `git` in its worktree. Anything else stays a
//! pending request for a person and raises an item in the lead's inbox. Only the policy and a
//! person's client answer a worker's request; no agent has a call that does.
//!
//! The project declares its commands in the `wheel.toml` at the root of its space's directory:
//!
//! ```toml
//! [policy]
//! commands = ["cargo test", "cargo clippy"]
//! ```
//!
//! A command is approved when it starts with a declared one. It is read from the space's directory,
//! never from the worker's worktree, so a worker cannot declare its own commands.

use std::path::{Component, Path, PathBuf};
use std::sync::Weak;

use agent_client_protocol::schema::v1::RequestPermissionRequest;
use wheel_proto::{LoopEventKind, PermOption, PolicyInfo};

use crate::server::AppState;

/// `bd` verbs a worker may run on its own bead.
const BD_VERBS: [&str; 3] = ["show", "update", "close"];

/// `bd` flags that point it at another database or directory.
const BD_REDIRECTS: [&str; 4] = ["--db", "-C", "--directory", "--global"];

/// `git` subcommands that work on the worktree and its own branch. Left out: `push`, `pull`,
/// `fetch`, `remote`, `config`, `worktree`, `submodule`, `clone`, and `branch`, `tag` and `stash`,
/// which change what every worktree of the repository shares.
const GIT_VERBS: [&str; 18] = [
    "status",
    "diff",
    "log",
    "show",
    "add",
    "commit",
    "restore",
    "rm",
    "mv",
    "switch",
    "checkout",
    "merge",
    "rebase",
    "reset",
    "cherry-pick",
    "blame",
    "grep",
    "ls-files",
];

/// What a permission request asks to do, as far as the policy reads it.
#[derive(Debug, Default, Clone)]
pub(crate) struct Ask {
    /// The ACP tool kind in its wire spelling: `edit`, `execute`, `read`, ...
    pub kind: Option<String>,
    pub title: String,
    /// The files the tool call names, as the agent wrote them.
    pub paths: Vec<PathBuf>,
    /// The shell command of an `execute` call.
    pub command: Option<String>,
}

impl Ask {
    pub(crate) fn of(req: &RequestPermissionRequest) -> Ask {
        let fields = &req.tool_call.fields;
        let mut paths: Vec<PathBuf> = fields
            .locations
            .iter()
            .flatten()
            .map(|l| l.path.clone())
            .collect();
        let mut command = None;
        if let Some(input) = &fields.raw_input {
            for key in [
                "file_path",
                "path",
                "notebook_path",
                "destination",
                "new_path",
                "target_path",
                "dest",
                "to",
            ] {
                if let Some(p) = input[key].as_str() {
                    paths.push(PathBuf::from(p));
                }
            }
            command = input["command"].as_str().map(String::from);
        }
        // An agent names a file in its locations and in its input alike.
        let mut seen = std::collections::HashSet::new();
        paths.retain(|p| seen.insert(p.clone()));
        Ask {
            kind: fields.kind.as_ref().and_then(|k| {
                serde_json::to_value(k)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
            }),
            title: fields
                .title
                .clone()
                .unwrap_or_else(|| req.tool_call.tool_call_id.to_string()),
            paths,
            command,
        }
    }
}

/// What the policy makes of a request, with the values it went by.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Approve(String),
    Wait(String),
}

/// The policy for one worker: its worktree, its bead, and the repository whose `wheel.toml`
/// declares the project's commands.
#[derive(Clone)]
pub(crate) struct Policy {
    pub repo: PathBuf,
    pub worktree: PathBuf,
    pub bead: String,
}

impl Policy {
    /// The commands the project declares, or why they cannot be read.
    pub(crate) fn declared(repo: &Path) -> Result<Vec<String>, String> {
        let path = repo.join("wheel.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{} cannot be read: {e}", path.display())),
        };
        let table: toml::Table = toml::from_str(&text)
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;
        let Some(commands) = table.get("policy").and_then(|p| p.get("commands")) else {
            return Ok(Vec::new());
        };
        let list = commands.as_array().ok_or_else(|| {
            format!(
                "{}: policy.commands is a {}, not a list of strings",
                path.display(),
                commands.type_str()
            )
        })?;
        list.iter()
            .map(|c| match c.as_str().map(str::trim) {
                Some(c) if words(c).is_some() => Ok(c.to_string()),
                _ => Err(format!(
                    "{}: policy.commands holds {c}, which is not one plain command",
                    path.display()
                )),
            })
            .collect()
    }

    pub(crate) fn decide(&self, ask: &Ask) -> Verdict {
        match ask.kind.as_deref() {
            Some("edit" | "delete" | "move") => self.edit(ask),
            Some("execute") => self.execute(ask),
            other => Verdict::Wait(format!(
                "a {} call is not one the policy covers",
                other.unwrap_or("unclassified")
            )),
        }
    }

    fn edit(&self, ask: &Ask) -> Verdict {
        if ask.paths.is_empty() {
            return Verdict::Wait("the edit names no file".into());
        }
        if ask.kind.as_deref() == Some("move") && ask.paths.len() < 2 {
            return Verdict::Wait("the move names no destination".into());
        }
        for path in &ask.paths {
            if !self.inside(path) {
                return Verdict::Wait(format!(
                    "{} is not inside the worktree {}",
                    path.display(),
                    self.worktree.display()
                ));
            }
        }
        Verdict::Approve(format!(
            "{} {} inside the worktree {}",
            ask.kind.as_deref().unwrap_or("edit"),
            ask.paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            self.worktree.display()
        ))
    }

    fn execute(&self, ask: &Ask) -> Verdict {
        let Some(command) = ask.command.as_deref() else {
            return Verdict::Wait("the call carries no command".into());
        };
        let Some(w) = words(command) else {
            return Verdict::Wait(format!(
                "{command:?} chains, redirects or expands, so the policy cannot tell what it runs"
            ));
        };
        match w[0].as_str() {
            "bd" => {
                if let Some(why) = self.bd(&w) {
                    return Verdict::Wait(why);
                }
                return Verdict::Approve(format!("bd on its own bead {}: {command}", self.bead));
            }
            "git" => {
                if let Some(why) = self.git(&w) {
                    return Verdict::Wait(why);
                }
                return Verdict::Approve(format!(
                    "git in the worktree {}: {command}",
                    self.worktree.display()
                ));
            }
            _ => {}
        }
        match Policy::declared(&self.repo) {
            Ok(declared) => {
                for d in &declared {
                    if words(d).is_some_and(|d| w.starts_with(&d)) {
                        return Verdict::Approve(format!(
                            "declared command {d:?} in {}: {command}",
                            self.repo.join("wheel.toml").display()
                        ));
                    }
                }
                Verdict::Wait(format!(
                    "{command:?} is not one of the {} declared commands",
                    declared.len()
                ))
            }
            Err(e) => Verdict::Wait(format!("the declared commands cannot be read: {e}")),
        }
    }

    /// Why `bd` with these words is not on the worker's own bead; None when it is.
    fn bd(&self, w: &[String]) -> Option<String> {
        if let Some(flag) = w.iter().find(|t| {
            BD_REDIRECTS.contains(&t.as_str())
                || t.starts_with("--db=")
                || t.starts_with("--directory=")
                || t.starts_with("-C")
        }) {
            return Some(format!("bd {flag} points bd somewhere else"));
        }
        let verb = w.get(1).map(String::as_str).unwrap_or_default();
        if !BD_VERBS.contains(&verb) {
            return Some(format!("bd {verb:?} is not one of {}", BD_VERBS.join(", ")));
        }
        if w.get(2) != Some(&self.bead) {
            return Some(format!(
                "bd {verb} names {:?}, not its own bead {}",
                w.get(2).map(String::as_str).unwrap_or("no bead"),
                self.bead
            ));
        }
        // A word after the bead that is neither a flag nor a flag's value is another bead.
        for i in 3..w.len() {
            let value_of_flag = w[i - 1].starts_with('-')
                && !w[i - 1].contains('=')
                && !BD_BOOLEAN.contains(&w[i - 1].as_str());
            if !w[i].starts_with('-') && !value_of_flag {
                return Some(format!(
                    "bd {verb} names {:?} besides its own bead {}",
                    w[i], self.bead
                ));
            }
        }
        None
    }

    /// Whether `path` is a file of the worktree, once `..` and symlinks are resolved. The
    /// worktree's `.git` is not: it points at the repository.
    fn inside(&self, path: &Path) -> bool {
        let Ok(root) = self.worktree.canonicalize() else {
            return false;
        };
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.worktree.join(path)
        };
        let resolved = resolve(&joined);
        match resolved.strip_prefix(&root) {
            Ok(rest) => !rest
                .components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case(".git")),
            Err(_) => false,
        }
    }

    /// The policy as `wheel supervisor policy` prints it.
    pub(crate) fn info(supervisor: &str, repo: &Path) -> PolicyInfo {
        let commands_file = repo.join("wheel.toml");
        let (commands, problem) = match Policy::declared(repo) {
            Ok(commands) => (commands, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let mut rules = vec![
            "An edit, delete or move whose every file is inside the worker's worktree, apart from the worktree's .git.".to_string(),
            format!(
                "A command that starts with one of the project's declared commands, from [policy] commands in {}.",
                commands_file.display()
            ),
            format!(
                "bd {} on the worker's own bead, and no other bead; never with --db, -C, --directory or --global.",
                BD_VERBS.join(", ")
            ),
            format!("git {} in the worker's worktree.", GIT_VERBS.join(", ")),
            "A command that chains, redirects or expands (; & | < > ( ) { } $ ` \\ * ? ~ # !) is never approved.".to_string(),
            "Everything else waits for the person and raises an item in the lead's inbox.".to_string(),
        ];
        if let Some(problem) = problem {
            rules.push(format!("No command is declared in force: {problem}"));
        }
        PolicyInfo {
            supervisor: supervisor.to_string(),
            rules,
            commands_file: commands_file.display().to_string(),
            commands,
        }
    }
}

/// `git` flags that run a program, write a file outside the worktree, or read outside it.
const GIT_FORBIDDEN: [&str; 12] = [
    "--exec",
    "-x",
    "--output",
    "-O",
    "--open-files-in-pager",
    "--no-index",
    "--upload-pack",
    "--receive-pack",
    "--git-dir",
    "--work-tree",
    "--config-env",
    "-c",
];

/// `bd` flags that take no value, so the word after one is a positional.
const BD_BOOLEAN: [&str; 14] = [
    "--json",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--claim",
    "--readonly",
    "--sandbox",
    "--stdin",
    "--persistent",
    "--ephemeral",
    "--history",
    "--no-history",
    "--allow-empty-description",
];

impl Policy {
    /// Why `git` with these words is not plain work in the worktree; None when it is.
    fn git(&self, w: &[String]) -> Option<String> {
        let verb = w.get(1).map(String::as_str).unwrap_or_default();
        if !GIT_VERBS.contains(&verb) {
            return Some(format!(
                "git {verb:?} is not one of {}",
                GIT_VERBS.join(", ")
            ));
        }
        for t in &w[2..] {
            let name = t.split('=').next().unwrap_or_default();
            if GIT_FORBIDDEN
                .iter()
                .any(|f| name == *f || (f.len() == 2 && !f.starts_with("--") && t.starts_with(f)))
            {
                return Some(format!(
                    "git {t} runs a program or reaches outside the worktree"
                ));
            }
            // A path argument is a file of the worktree.
            if !t.starts_with('-')
                && (Path::new(t).is_absolute()
                    || Path::new(t).components().any(|c| c == Component::ParentDir))
                && !self.inside(Path::new(t))
            {
                return Some(format!("git names {t}, which is not inside the worktree"));
            }
        }
        None
    }
}

/// The words of a command line, when it is one plain command: quotes group words, and nothing
/// chains, redirects, expands or globs. None otherwise.
fn words(command: &str) -> Option<Vec<String>> {
    const SHELL: &str = ";&|<>(){}$`\\*?[]~#!\n\r";
    let mut out = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if matches!(c, '$' | '`' | '\\' | '!') => return None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                started = true;
            }
            None if c == ' ' || c == '\t' => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None if SHELL.contains(c) => return None,
            None => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        out.push(word);
    }
    (!out.is_empty()).then_some(out)
}

/// `path` with every symlink in its existing part followed, one component at a time, so a `..`
/// after a link goes where the operating system takes it. The part that does not exist yet is kept
/// as written.
fn resolve(path: &Path) -> PathBuf {
    let mut cur = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                cur.pop();
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                cur.push(name);
                if cur
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    if let Ok(real) = cur.canonicalize() {
                        cur = real;
                    }
                }
            }
            other => cur.push(other),
        }
    }
    cur
}

/// The policy applied to one worker's session: what its permission requests go through before a
/// person sees them.
pub(crate) struct WorkerGate {
    pub me: Weak<AppState>,
    pub policy: Policy,
    pub supervisor: String,
    pub space: String,
    /// The supervisor's pane, where the worker shows.
    pub pane: String,
    pub session: String,
}

impl WorkerGate {
    /// The option to answer `ask` with when the policy approves it, after recording the approval.
    /// When it does not, the reason the request waits for a person, for [`WorkerGate::waiting`].
    pub(crate) fn approve(&self, ask: &Ask, options: &[PermOption]) -> Result<String, String> {
        let Some(state) = self.me.upgrade() else {
            return Err("the daemon is shutting down".into());
        };
        // The person unlocked this worker's pane: its requests are theirs to answer, whatever the
        // policy would say.
        if state.worker_unlocked(&self.session) {
            return Err(format!(
                "{}: the worker's pane is unlocked, so the request waits for the person",
                ask.title
            ));
        }
        match self.policy.decide(ask) {
            Verdict::Approve(reason) => {
                let Some(option) = options.iter().find(|o| o.kind == "allow_once") else {
                    return Err(format!(
                        "{reason}; but the request offers no allow_once option"
                    ));
                };
                state.emit(
                    &self.supervisor,
                    Some(&self.session),
                    Some(&self.policy.bead),
                    LoopEventKind::ApprovedByPolicy,
                    format!("{}: {reason}", ask.title),
                );
                Ok(option.id.clone())
            }
            Verdict::Wait(reason) => Err(format!("{}: {reason}", ask.title)),
        }
    }

    /// Request `perm`, which the policy left pending for `reason`, waits for a person: the loop
    /// records it, and the lead's inbox gets an item that closes when the request is answered or
    /// withdrawn. The request is registered before this runs, so the worker's stall clock, which
    /// pauses while a request waits, never sees the event ahead of the request.
    pub(crate) fn waiting(&self, perm: &str, ask: &Ask, reason: String) {
        let Some(state) = self.me.upgrade() else {
            return;
        };
        state.emit(
            &self.supervisor,
            Some(&self.session),
            Some(&self.policy.bead),
            LoopEventKind::PendingForPerson,
            reason,
        );
        state.inbox_worker_permission(
            &self.space,
            &self.pane,
            &self.session,
            ask.title.clone(),
            perm.to_string(),
            state.worker_unlocked(&self.session),
        );
    }
}

impl AppState {
    /// The permission policy of supervisor `id`.
    pub fn supervisor_policy(&self, id: &str) -> Result<PolicyInfo, String> {
        let info = self.supervisor_show(id)?;
        let repo = self.space_repo(&info.space)?;
        Ok(Policy::info(id, Path::new(&repo)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(dir: &Path) -> Policy {
        Policy {
            repo: dir.join("repo"),
            worktree: dir.join("wt"),
            bead: "t-1".into(),
        }
    }

    fn edit(paths: &[&str]) -> Ask {
        Ask {
            kind: Some("edit".into()),
            title: "edit".into(),
            paths: paths.iter().map(PathBuf::from).collect(),
            command: None,
        }
    }

    fn run(command: &str) -> Ask {
        Ask {
            kind: Some("execute".into()),
            title: command.into(),
            paths: Vec::new(),
            command: Some(command.into()),
        }
    }

    fn approves(v: Verdict) -> bool {
        matches!(v, Verdict::Approve(_))
    }

    #[test]
    fn an_edit_is_approved_only_when_every_file_resolves_inside_the_worktree() {
        let dir = std::env::temp_dir().join(format!("wheel-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("wt/src")).unwrap();
        std::fs::create_dir_all(dir.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("wt/link")).unwrap();
        let p = policy(&dir);
        let wt = dir.join("wt");
        let inside = wt.join("src/new.rs");
        assert!(approves(p.decide(&edit(&[inside.to_str().unwrap()]))));
        assert!(approves(p.decide(&edit(&["src/new.rs"]))));
        for outside in [
            dir.join("elsewhere/x").display().to_string(),
            wt.join("../elsewhere/x").display().to_string(),
            "../elsewhere/x".to_string(),
            wt.join("link/x").display().to_string(),
            wt.join(".git").display().to_string(),
            wt.join("src/../.git/config").display().to_string(),
            wt.join("link/../x").display().to_string(),
            wt.join(".GIT/config").display().to_string(),
        ] {
            assert!(!approves(p.decide(&edit(&[&outside]))), "{outside}");
        }
        // One file outside holds the whole request, and a request naming no file waits.
        assert!(!approves(
            p.decide(&edit(&["src/new.rs", "../elsewhere/x"]))
        ));
        assert!(!approves(p.decide(&edit(&[]))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bd_and_git_are_approved_only_as_plain_work_on_the_workers_own_bead_and_worktree() {
        let p = policy(Path::new("/nowhere"));
        for yes in [
            "bd show t-1",
            "bd update t-1 --status blocked --notes \"why: a question\"",
            "bd close t-1",
            "git status",
            "git commit -m \"feat: add it (t-1)\"",
            "git diff --stat",
        ] {
            assert!(approves(p.decide(&run(yes))), "{yes}");
        }
        for no in [
            "bd show t-2",
            "bd close t-1 t-2",
            "bd ready",
            "bd -C /other show t-1",
            "bd --db /other/x.db show t-1",
            "git push origin main",
            "git rebase --exec 'touch x' main",
            "git log --output=/tmp/x",
            "git diff --no-index /etc/passwd x",
            "git grep -Osh foo",
            "git show /etc/passwd",
            "bd update t-1 --priority 1 t-2",
            "bd show t-1 --directory=/other",
            "git -C /other status",
            "git -c core.pager=sh status",
            "git status; rm -rf /",
            "git status && curl x",
            "git log | sh",
            "git commit -m \"$(whoami)\"",
            "git add *.rs",
            "git commit -m 'unterminated",
            "",
        ] {
            assert!(!approves(p.decide(&run(no))), "{no}");
        }
    }

    #[test]
    fn declared_commands_come_from_the_repos_wheel_toml_and_match_by_prefix() {
        let dir = std::env::temp_dir().join(format!("wheel-policy-decl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = Policy {
            repo: dir.clone(),
            worktree: dir.join("wt"),
            bead: "t-1".into(),
        };
        assert!(!approves(p.decide(&run("cargo test"))));
        std::fs::write(
            dir.join("wheel.toml"),
            "[policy]\ncommands = [\"cargo test\", \"admin vet\"]\n",
        )
        .unwrap();
        assert!(approves(
            p.decide(&run("cargo test -p wheeld --test spaces"))
        ));
        assert!(approves(p.decide(&run("admin vet"))));
        assert!(!approves(p.decide(&run("cargo build"))));
        assert!(!approves(p.decide(&run("cargo testing"))));
        assert!(!approves(p.decide(&run("cargo test; rm x"))));
        std::fs::write(
            dir.join("wheel.toml"),
            "[policy]\ncommands = \"cargo test\"\n",
        )
        .unwrap();
        assert!(Policy::declared(&dir).is_err());
        assert!(!approves(p.decide(&run("cargo test"))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_call_that_is_not_an_edit_or_a_command_waits() {
        let p = policy(Path::new("/nowhere"));
        for kind in ["read", "fetch", "search", "think", "other"] {
            let ask = Ask {
                kind: Some(kind.into()),
                ..edit(&["src/a.rs"])
            };
            assert!(!approves(p.decide(&ask)), "{kind}");
        }
        assert!(!approves(p.decide(&Ask::default())));
    }
}
