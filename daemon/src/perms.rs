//! Permission requests waiting on a person. Every connected client sees each request, and the
//! first answer resolves it; any later answer is refused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tokio::sync::{broadcast, oneshot};
use wheel_proto::{PermInfo, PermWhy, ServerMsg};

struct Pending {
    info: PermInfo,
    answer: oneshot::Sender<String>,
}

pub struct Perms {
    next_id: AtomicU64,
    pending: Mutex<HashMap<String, Pending>>,
    /// Daemon-wide events every connection forwards.
    events: broadcast::Sender<ServerMsg>,
}

impl Perms {
    pub fn new(events: broadcast::Sender<ServerMsg>) -> Perms {
        Perms {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// Register a request, announce it, and wait for the chosen option id. Dropping the future
    /// first withdraws the request, so a caller that gave up leaves nothing waiting.
    pub async fn ask(
        &self,
        agent: &str,
        title: String,
        options: Vec<wheel_proto::PermOption>,
    ) -> Result<String, String> {
        let (id, answer) = self.open(agent, title, options, None, PermWhy::default());
        let _withdraw = WithdrawOnDrop { perms: self, id };
        answer
            .await
            .map_err(|_| "the request was withdrawn before anyone answered".to_string())
    }

    /// Register a request and announce it. The receiver yields the chosen option id. The request
    /// waits until answered or `drop_agent`; dropping the receiver does not withdraw it.
    /// `tool_call` is the agent's id for the tool call the request gates, when an agent asked.
    pub fn open(
        &self,
        agent: &str,
        title: String,
        options: Vec<wheel_proto::PermOption>,
        tool_call: Option<String>,
        why: PermWhy,
    ) -> (String, oneshot::Receiver<String>) {
        let id = format!("p{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let info = PermInfo {
            id: id.clone(),
            agent: agent.to_string(),
            title,
            options,
            tool_call,
            why,
        };
        let (tx, rx) = oneshot::channel();
        // The title and reason can quote what an agent wants to run, so the log gets their sizes; the rule is the one program the person is asked to allow.
        tracing::info!(perm = %id, agent, tool_call = info.tool_call.as_deref().unwrap_or("-"), title_chars = info.title.chars().count(), reason_chars = info.why.reason.as_deref().map_or(0, |r| r.chars().count()), always_rule = info.why.always_rule.as_deref().unwrap_or("-"), "perm requested");
        self.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                info: info.clone(),
                answer: tx,
            },
        );
        let _ = self.events.send(ServerMsg::PermRequested { perm: info });
        (id, rx)
    }

    /// Resolve a request. `choice` is an option id, or `allow` / `deny` for the first
    /// allow-once / reject-once option. Returns the option id chosen.
    pub fn answer(&self, id: &str, choice: &str, by: &str) -> Result<String, String> {
        let mut pending = self.pending.lock().unwrap();
        let Some(p) = pending.get(id) else {
            return Err(format!(
                "no waiting permission {id}; it may already be answered"
            ));
        };
        let wanted = match choice {
            "allow" => Some("allow_once"),
            "deny" => Some("reject_once"),
            _ => None,
        };
        let option = p
            .info
            .options
            .iter()
            .find(|o| match wanted {
                Some(kind) => o.kind == kind,
                None => o.id == choice,
            })
            .map(|o| o.id.clone())
            .ok_or_else(|| {
                format!(
                    "{id} has no option {choice:?}; options: {}",
                    p.info
                        .options
                        .iter()
                        .map(|o| o.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let p = pending.remove(id).expect("checked above");
        drop(pending);
        if p.answer.send(option.clone()).is_err() {
            // The turn that asked is already over; the agent got Cancelled.
            let _ = self.events.send(ServerMsg::PermResolved {
                id: id.to_string(),
                choice: "withdrawn".into(),
            });
            return Err(format!(
                "{id} was withdrawn: the agent no longer waits for it"
            ));
        }
        tracing::info!(perm = %id, choice = %option, by, "perm answered");
        let _ = self.events.send(ServerMsg::PermResolved {
            id: id.to_string(),
            choice: option.clone(),
        });
        Ok(option)
    }

    /// A request the agent withdrew (its turn was cancelled or its process died).
    pub fn drop_agent(&self, agent: &str) {
        let mut pending = self.pending.lock().unwrap();
        let gone: Vec<String> = pending
            .iter()
            .filter(|(_, p)| p.info.agent == agent)
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            pending.remove(&id);
            self.withdrawn(id, agent);
        }
    }

    /// Withdraw one request if it is still waiting.
    fn withdraw(&self, id: &str) {
        let removed = self.pending.lock().unwrap().remove(id);
        if let Some(p) = removed {
            self.withdrawn(id.to_string(), &p.info.agent);
        }
    }

    fn withdrawn(&self, id: String, agent: &str) {
        tracing::info!(perm = %id, agent, "perm withdrawn");
        let _ = self.events.send(ServerMsg::PermResolved {
            id,
            choice: "withdrawn".into(),
        });
    }

    /// The daemon-wide events, for a caller that follows requests being asked and answered.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerMsg> {
        self.events.subscribe()
    }

    /// Whether a request of `agent` is waiting for an answer: its running turn is stopped on it.
    pub fn has_pending(&self, agent: &str) -> bool {
        self.pending.lock().unwrap().values().any(|p| p.info.agent == agent)
    }

    pub fn list(&self) -> Vec<PermInfo> {
        let mut list: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .map(|p| p.info.clone())
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }
}

struct WithdrawOnDrop<'a> {
    perms: &'a Perms,
    id: String,
}

impl Drop for WithdrawOnDrop<'_> {
    fn drop(&mut self) {
        self.perms.withdraw(&self.id);
    }
}

/// What wheeld answers when the global orchestrator asks to run a tool: it makes no change itself,
/// so it may write only documents and run only `wheel`.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
    /// The rule covers no such tool; the request goes to the person.
    Person,
}

/// The rule for the global orchestrator's own tool call `tool` with `input`: `Edit`, `Write` and
/// `NotebookEdit` only on a path under one of `roots`, `Bash` only for a plain `wheel` command.
/// A relative path is taken from `cwd`.
pub(crate) fn orchestrator_verdict(tool: &str, input: &serde_json::Value, roots: &[std::path::PathBuf], cwd: &std::path::Path) -> Verdict {
    let key = match tool {
        "Edit" | "Write" => "file_path",
        "NotebookEdit" => "notebook_path",
        "Bash" => {
            let command = input["command"].as_str().unwrap_or_default();
            return if plain_wheel(command) {
                Verdict::Allow
            } else {
                Verdict::Deny(
                    "wheeld refused this: the global orchestrator runs no shell command but a plain `wheel …` \
                     (no ; & | < > ` $( or newline outside quotes). Send the work to an agent with `wheel agent send`."
                        .into(),
                )
            };
        }
        _ => return Verdict::Person,
    };
    let path = input[key].as_str().map(|p| cwd.join(p));
    let inside = path.and_then(|p| resolved(&p)).is_some_and(|p| {
        !runs_code(&p) && roots.iter().filter_map(|r| resolved(r)).any(|r| p.starts_with(r))
    });
    if inside {
        return Verdict::Allow;
    }
    let roots: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    Verdict::Deny(format!(
        "wheeld refused this: the global orchestrator makes no change itself, so {tool} is allowed only under {}, and never on a file that runs commands (.claude/, .git/, .wheel/ outside .wheel/artifacts/, .mcp.json, wheel.toml, .envrc). \
         Send the change to an agent with `wheel agent send`.",
        roots.join(", ")
    ))
}

/// Whether a file at `path` is one a program reads as commands to run (Claude Code's project
/// settings and hooks, an MCP server list, a `wheel.toml`'s refresh commands, git hooks, direnv),
/// so writing it would run a change through another door. A page under `.wheel/artifacts/` runs
/// nothing; the rest of `.wheel/` holds plugins.
fn runs_code(path: &std::path::Path) -> bool {
    let names: Vec<&str> = path.components().filter_map(|c| c.as_os_str().to_str()).collect();
    names.iter().enumerate().any(|(i, n)| match *n {
        ".claude" | ".git" => true,
        ".wheel" => names.get(i + 1) != Some(&"artifacts"),
        _ => false,
    }) || matches!(names.last(), Some(&(".mcp.json" | "wheel.toml" | ".envrc")))
}

/// `path` with its deepest existing ancestor made canonical and the rest appended, so neither a
/// symlink nor `..` reaches outside; None for a path that names `..` below that ancestor or a
/// dangling symlink.
fn resolved(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut base = path;
    let mut rest = Vec::new();
    while base.symlink_metadata().is_err() {
        rest.push(base.file_name()?);
        base = base.parent()?;
    }
    if !path.strip_prefix(base).ok()?.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut out = base.canonicalize().ok()?;
    out.extend(rest.into_iter().rev());
    Some(out)
}

/// Whether `command` runs `wheel` and nothing else: its first word is `wheel`, and no character
/// that ends, chains, redirects or substitutes a command appears outside quotes (inside double
/// quotes, only a substitution).
fn plain_wheel(command: &str) -> bool {
    let command = command.trim();
    if command.split_whitespace().next() != Some("wheel") {
        return false;
    }
    #[derive(PartialEq)]
    enum Quote { None, Single, Double }
    let mut quote = Quote::None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (&quote, c) {
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::Single, _) => {}
            (_, '\\') => {
                chars.next();
            }
            (_, '`') => return false,
            (_, '$') if chars.peek() == Some(&'(') => return false,
            // `$'…'` lets a backslash escape its quote, so the scan would lose its place.
            (Quote::None, '$') if matches!(chars.peek(), Some('\'' | '"')) => return false,
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::Double, _) => {}
            (Quote::None, '\'') => quote = Quote::Single,
            (Quote::None, '"') => quote = Quote::Double,
            (Quote::None, ';' | '&' | '|' | '<' | '>' | '\n' | '\r') => return false,
            (Quote::None, _) => {}
        }
    }
    quote == Quote::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn only_a_plain_wheel_command_runs() {
        for ok in [
            "wheel status",
            "  wheel agent send a1 \"fix it; then test | report\"",
            "wheel agent send a1 'no $(sub) here; `x`'",
            "wheel agent send a1 \"two\nlines\"",
        ] {
            assert!(plain_wheel(ok), "{ok}");
        }
        for bad in [
            "ls",
            "wheelx status",
            "FOO=1 wheel status",
            "wheel status; rm -rf /",
            "wheel status && ls",
            "wheel status | sh",
            "wheel status > /tmp/x",
            "wheel status\nls",
            "wheel agent send a1 \"$(cat /etc/passwd)\"",
            "wheel agent send a1 `id`",
            "wheel x \\'; ls; \\'",
            "wheel x \"'\"; ls",
            "wheel x 'unclosed",
            "wheel x $'\\'' ; ls #'",
            "wheel x $\"a\" ; ls",
        ] {
            assert!(!plain_wheel(bad), "{bad}");
        }
    }

    #[test]
    fn edits_land_only_under_the_roots() {
        let dir = std::env::temp_dir().join(format!("wheel-perms-verdict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let docs = dir.join("project/docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(dir.join("outside")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("outside"), docs.join("link")).unwrap();
        let roots = vec![docs.clone()];
        let at = |tool: &str, key: &str, path: PathBuf| orchestrator_verdict(tool, &json!({ key: path }), &roots, &dir);
        assert_eq!(at("Write", "file_path", docs.join("new/adr.md")), Verdict::Allow);
        assert_eq!(at("Edit", "file_path", docs.join("a.md")), Verdict::Allow);
        assert_eq!(at("NotebookEdit", "notebook_path", docs.join("n.ipynb")), Verdict::Allow);
        assert_eq!(orchestrator_verdict("Write", &json!({"file_path": "project/docs/rel.md"}), &roots, &dir), Verdict::Allow);
        for escape in [
            dir.join("outside/x"),
            docs.join("../x"),
            docs.join("new/../../x"),
            docs.join("link/x"),
            PathBuf::from("/tmp/x"),
        ] {
            assert!(matches!(at("Write", "file_path", escape.clone()), Verdict::Deny(m) if m.contains("Write is allowed only under")), "{escape:?}");
        }
        assert_eq!(at("Write", "file_path", docs.join(".wheel/artifacts/page/index.md")), Verdict::Allow);
        for code in [".claude/settings.json", ".mcp.json", "wheel.toml", ".git/hooks/pre-commit", ".wheel/plugins/x/init.luau", "sub/.envrc"] {
            assert!(matches!(at("Write", "file_path", docs.join(code)), Verdict::Deny(_)), "{code}");
        }
        assert!(matches!(orchestrator_verdict("Write", &json!({}), &roots, &dir), Verdict::Deny(_)));
        assert_eq!(orchestrator_verdict("WebFetch", &json!({}), &roots, &dir), Verdict::Person);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
