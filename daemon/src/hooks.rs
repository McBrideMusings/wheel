//! Event hooks (ADR-0054): `[[hook]]` entries in this machine's `wheel.toml` and in a project's.
//!
//! ```toml
//! [[hook]]
//! on = ["turn_end", "permission", "fail"]
//! run = "tools/on-event"
//! inbox = "inbox"        # optional: each event is first written here as one Maildir file
//! decide = false         # on `permission`, machine file only: the command prints allow, deny or ask
//! ```
//!
//! A machine hook takes every agent's events, a project hook those of the agents in spaces open on
//! that project. `run` is a shell command run in the folder of the file that names it (`~/.wheel`
//! for the machine's, the project's root for a project's), with the event's JSON on stdin. Both
//! files are read at each event, so an edit applies to the next one. Each hook runs its events one
//! at a time, in the order they happened. Every event also goes to each `agent_events_follow`
//! stream (`wheel agent events --follow`), as the same JSON on one line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wheel_proto::{PermInfo, ServerMsg};

use crate::agent_events::{Event, Filter};
use crate::server::AppState;

/// How long one agent turn runs before its `over_time` event.
const TURN_LIMIT: Duration = Duration::from_secs(1800);

/// How long one run of a hook's command may take before it is killed and logged as failed.
const RUN_LIMIT: Duration = Duration::from_secs(120);

/// The events a hook's `on` can name.
pub(crate) const EVENTS: &[&str] = &["turn_start", "turn_end", "fail", "permission", "answered", "over_time", "build_failed"];


/// One `[[hook]]` entry as a file names it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Hook {
    /// The file that names it.
    pub file: PathBuf,
    /// Where `run` runs and a relative `inbox` starts from: the file's folder.
    pub dir: PathBuf,
    pub on: Vec<String>,
    pub run: String,
    pub inbox: Option<PathBuf>,
    pub decide: bool,
}

/// The `[[hook]]` entries of the `wheel.toml` at `file`, run in `dir`. A file that is missing names
/// none; one that is not TOML, and an entry with no command or no event wheeld sends, are logged
/// and name none.
pub(crate) fn hooks_in(file: &Path, dir: &Path) -> Vec<Hook> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let table: toml::Table = match toml::from_str(&text) {
        Ok(table) => table,
        Err(error) => {
            tracing::warn!(path = %file.display(), %error, "a wheel.toml is not valid TOML; it names no hooks");
            return Vec::new();
        }
    };
    let Some(entries) = table.get("hook") else { return Vec::new() };
    let Some(entries) = entries.as_array() else {
        tracing::warn!(path = %file.display(), "hook in a wheel.toml is not an array of tables ([[hook]]); it names no hooks");
        return Vec::new();
    };
    let mut hooks = Vec::new();
    for (n, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_table() else {
            tracing::warn!(path = %file.display(), n, "a [[hook]] entry is not a table; it is ignored");
            continue;
        };
        let run = entry.get("run").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
        let Some(run) = run else {
            tracing::warn!(path = %file.display(), n, "a [[hook]] has no run command; it is ignored");
            continue;
        };
        let named: Vec<String> = match entry.get("on") {
            Some(toml::Value::String(one)) => vec![one.clone()],
            Some(toml::Value::Array(all)) => all.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            _ => Vec::new(),
        };
        let (on, unknown): (Vec<String>, Vec<String>) = named.into_iter().partition(|e| EVENTS.contains(&e.as_str()));
        if !unknown.is_empty() {
            tracing::warn!(path = %file.display(), n, ?unknown, events = ?EVENTS, "a [[hook]] names events wheeld does not send; they are ignored");
        }
        if on.is_empty() {
            tracing::warn!(path = %file.display(), n, %run, "a [[hook]] names no event wheeld sends; it is ignored");
            continue;
        }
        let inbox = entry.get("inbox").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| {
            let path = PathBuf::from(crate::dirs::expand_home(s));
            if path.is_absolute() { path } else { dir.join(path) }
        });
        let decide = entry.get("decide").and_then(|v| v.as_bool()).unwrap_or(false);
        if decide && !on.iter().any(|e| e == "permission") {
            tracing::warn!(path = %file.display(), n, %run, "a [[hook]] with decide = true takes no permission event; it decides nothing");
        }
        hooks.push(Hook { file: file.to_path_buf(), dir: dir.to_path_buf(), on, run: run.to_string(), inbox, decide });
    }
    hooks
}

/// The JSON a hook's command reads on stdin, and the file `inbox` holds.
#[derive(serde::Serialize)]
struct Payload<'a> {
    event: &'static str,
    agent: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    space: Option<&'a str>,
    text: &'a str,
    at_ms: u64,
    /// The permission request a `permission` or `answered` event is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    perm: Option<&'a str>,
    /// A `permission` event's whole request.
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<&'a PermInfo>,
}

/// One event queued for one hook.
pub(crate) struct Job {
    hook: Hook,
    event: &'static str,
    agent: String,
    json: Vec<u8>,
    /// Its file in the hook's inbox.
    file: Option<PathBuf>,
    /// The request a decide hook answers.
    perm: Option<String>,
}

/// What a decide hook's output asks for: `allow` or `deny`, from its first word; anything else
/// leaves the request to the person.
fn decision(stdout: &str) -> Option<&'static str> {
    match stdout.split_whitespace().next().map(str::to_ascii_lowercase).as_deref() {
        Some("allow") => Some("allow"),
        Some("deny") => Some("deny"),
        _ => None,
    }
}

/// Turn wheeld's broadcast and its failed builds into agent events and hand each to the hooks
/// that name it, for as long as wheeld runs.
pub(crate) fn follow(state: Arc<AppState>) {
    let mut global = state.global.subscribe();
    let mut builds = state.build_failures.subscribe();
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        let mut filter = Filter::default();
        loop {
            let wake = filter.next_overdue(TURN_LIMIT).unwrap_or(Instant::now() + Duration::from_secs(86_400));
            let events: Vec<(Event, Option<PermInfo>)> = tokio::select! {
                msg = global.recv() => match msg {
                    Ok(msg) => {
                        let request = match &msg {
                            ServerMsg::PermRequested { perm } => Some(perm.clone()),
                            _ => None,
                        };
                        match filter.event(&msg, Instant::now()) {
                            Some(event) => vec![(event, request)],
                            None => continue,
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Events were dropped: take the waiting permission requests from the
                        // source, so none goes unreported.
                        tracing::warn!(missed = n, "hooks lagged the broadcast; reading the waiting requests again");
                        state
                            .perms
                            .list()
                            .into_iter()
                            .filter_map(|perm| {
                                let event = filter.event(&ServerMsg::PermRequested { perm: perm.clone() }, Instant::now())?;
                                Some((event, Some(perm)))
                            })
                            .collect()
                    }
                    Err(RecvError::Closed) => break,
                },
                failed = builds.recv() => match failed {
                    Ok((agent, command)) => vec![(crate::agent_events::build_failed(&agent, &command), None)],
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "hooks lagged the failed builds");
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                },
                _ = tokio::time::sleep_until(wake.into()) => {
                    filter.forget(|agent| state.agent(agent).is_err());
                    filter.overdue(Instant::now(), TURN_LIMIT).into_iter().map(|e| (e, None)).collect()
                }
            };
            for (event, request) in events {
                state.hook_event(event, request.as_ref());
            }
        }
        tracing::info!("hooks stop: the broadcast closed");
    });
}

/// Orders the Maildir files written in one millisecond.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Write `bytes` as one file in the Maildir `maildir`, made when missing: into `tmp/`, then
/// renamed into `new/`, so a reader never sees a partial file. Returns its path in `new/`. The
/// name leads with the write time and a counter, so names sort in the order they were written.
fn maildir_write(maildir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    for sub in ["tmp", "new", "cur"] {
        std::fs::create_dir_all(maildir.join(sub))?;
    }
    let name = format!("{:013}.{:010}.{}.json", now_ms(), SEQ.fetch_add(1, Ordering::Relaxed), std::process::id());
    let tmp = maildir.join("tmp").join(&name);
    let new = maildir.join("new").join(&name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &new)?;
    Ok(new)
}

impl AppState {
    /// The hooks that take the events of an agent in a space open on `cwd` (none for an agent in
    /// no space): this machine's, then the project's.
    pub(crate) fn hooks_for(&self, cwd: Option<&str>) -> Vec<Hook> {
        let machine = self.machine_file();
        let mut hooks = hooks_in(&machine, machine.parent().unwrap_or(Path::new("/")));
        if let Some(cwd) = cwd {
            let project = Path::new(cwd).join(crate::settings::FILE);
            if project != machine {
                // The agents in a space can write its project's file, so only the machine's may
                // answer their permission requests.
                hooks.extend(hooks_in(&project, Path::new(cwd)).into_iter().map(|mut hook| {
                    if hook.decide {
                        tracing::warn!(path = %project.display(), hook = %hook.run, "decide = true is taken only from this machine's wheel.toml; the project's hook runs without deciding");
                        hook.decide = false;
                    }
                    hook
                }));
            }
        }
        hooks
    }

    /// The space, pane and space folder of the pane agent `agent` runs in.
    fn agent_place(&self, agent: &str) -> Option<(String, String, String)> {
        let spaces = self.spaces.lock().unwrap();
        spaces.spaces.iter().find_map(|s| {
            crate::spaces::agent_panes(s).into_iter().find(|p| p.agent == agent).map(|p| (s.id.clone(), p.pane, s.cwd.clone()))
        })
    }

    /// Send `event` to every `agent_events_follow` stream, then hand it to every hook that names
    /// it: write it into the hook's inbox, when it has one, then queue its command on it.
    fn hook_event(&self, event: Event, request: Option<&PermInfo>) {
        let name = event.kind.name();
        let place = self.agent_place(&event.agent);
        let hooks: Vec<Hook> = self.hooks_for(place.as_ref().map(|p| p.2.as_str())).into_iter().filter(|h| h.on.iter().any(|e| e == name)).collect();
        let followed = self.agent_events.receiver_count() > 0;
        if hooks.is_empty() && !followed {
            return;
        }
        let payload = Payload {
            event: name,
            agent: &event.agent,
            pane: place.as_ref().map(|p| p.1.as_str()),
            space: place.as_ref().map(|p| p.0.as_str()),
            text: &event.text,
            at_ms: now_ms(),
            perm: event.perm.as_deref(),
            request,
        };
        if followed {
            match serde_json::to_value(&payload) {
                Ok(value) => {
                    let _ = self.agent_events.send(Arc::new(value));
                }
                Err(error) => tracing::warn!(event = name, %error, "agent event not serialized for its followers"),
            }
        }
        if hooks.is_empty() {
            return;
        }
        let json = match serde_json::to_vec_pretty(&payload) {
            Ok(json) => json,
            Err(error) => return tracing::warn!(event = name, %error, "hook event not serialized"),
        };
        for hook in hooks {
            let file = match &hook.inbox {
                Some(inbox) => match maildir_write(inbox, &json) {
                    Ok(path) => Some(path),
                    Err(error) => {
                        tracing::warn!(hook = %hook.run, inbox = %inbox.display(), event = name, %error, "hook event not written; its command does not run");
                        continue;
                    }
                },
                None => None,
            };
            let perm = (hook.decide && name == "permission").then(|| event.perm.clone()).flatten();
            self.hook_queue(Job { hook, event: name, agent: event.agent.clone(), json: json.clone(), file, perm });
        }
    }

    /// Queue `job` behind the jobs its hook has waiting, so each hook runs its events in order.
    /// The decide jobs for one permission request share a queue, so the hooks deciding it run in
    /// file order and the first that answers it wins; different requests have different queues,
    /// so a slow hook holds up only the request it is deciding. A queue ends once it is empty.
    fn hook_queue(&self, job: Job) {
        let key = match &job.perm {
            Some(perm) => format!("decide\n{perm}"),
            None => format!("{}\n{}\n{:?}", job.hook.file.display(), job.hook.run, job.hook.inbox),
        };
        let mut runs = self.hook_runs.lock().unwrap();
        let job = match runs.get(&key) {
            Some(queue) => match queue.send(job) {
                Ok(()) => return,
                Err(tokio::sync::mpsc::error::SendError(job)) => job,
            },
            None => job,
        };
        let Some(state) = self.me.upgrade() else { return };
        let (queue, mut queued) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let _ = queue.send(job);
        runs.insert(key.clone(), queue);
        tokio::spawn(async move {
            while let Some(job) = queued.recv().await {
                state.hook_run(job).await;
                // Empty under the lock jobs are queued under, so none is sent to a queue that ends.
                let mut runs = state.hook_runs.lock().unwrap();
                if queued.is_empty() {
                    runs.remove(&key);
                    return;
                }
            }
        });
    }

    /// Run one job's command and log how it went; a decide hook's `allow` or `deny` answers its
    /// request.
    async fn hook_run(&self, job: Job) {
        let started = Instant::now();
        let (hook, event, agent) = (&job.hook, job.event, job.agent.as_str());
        let out = match self.hook_command(&job).await {
            Ok(out) => out,
            Err(why) => return tracing::warn!(file = %hook.file.display(), hook = %hook.run, event, agent, %why, "hook failed"),
        };
        let ms = started.elapsed().as_millis() as u64;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: String = stderr.trim().chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
            return tracing::warn!(file = %hook.file.display(), hook = %hook.run, event, agent, ms, status = %out.status, stderr = %tail, "hook failed");
        }
        let Some(perm) = &job.perm else {
            return tracing::info!(file = %hook.file.display(), hook = %hook.run, event, agent, ms, "hook ran");
        };
        let said = String::from_utf8_lossy(&out.stdout);
        let Some(choice) = decision(&said) else {
            let said: String = said.trim().chars().take(80).collect();
            return tracing::info!(file = %hook.file.display(), hook = %hook.run, %perm, agent, ms, %said, "decide hook left the request to the person");
        };
        match self.perms.answer(perm, choice, &format!("hook {}", hook.run)) {
            Ok(option) => tracing::info!(file = %hook.file.display(), hook = %hook.run, %perm, agent, ms, choice, %option, "decide hook answered"),
            Err(why) => tracing::info!(file = %hook.file.display(), hook = %hook.run, %perm, agent, ms, choice, %why, "decide hook's answer not taken"),
        }
    }

    /// `sh -c <run>` in the hook's folder, the event on stdin and its inbox file as `$1`, with the
    /// environment `wheel` needs.
    async fn hook_command(&self, job: &Job) -> Result<std::process::Output, String> {
        let env = crate::shell::command_env(self.control.lock().unwrap().as_ref(), &self.instance_dir, &self.path());
        let mut command = tokio::process::Command::new("/bin/sh");
        command.arg("-c").arg(&job.hook.run).arg("wheel-hook");
        if let Some(file) = &job.file {
            command.arg(file).env("WHEEL_EVENT_FILE", file);
        }
        let mut child = command
            .current_dir(&job.hook.dir)
            .envs(env)
            .env("WHEEL_EVENT", job.event)
            .stdin(Stdio::piped())
            .stdout(if job.perm.is_some() { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::piped())
            // Its own group, so a run past the limit is killed with everything it started.
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot run in {}: {e}", job.hook.dir.display()))?;
        let group = child.id();
        let stdin = child.stdin.take();
        let json = job.json.clone();
        // Written beside the wait, so a command that never reads a payload bigger than the pipe
        // still meets the limit.
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            if let Some(mut stdin) = stdin {
                // A command that never reads its stdin closes it early; that is no failure of its own.
                let _ = stdin.write_all(&json).await;
            }
        });
        let out = tokio::time::timeout(RUN_LIMIT, child.wait_with_output()).await;
        writer.abort();
        match out {
            Ok(out) => out.map_err(|e| e.to_string()),
            Err(_) => {
                if let Some(group) = group {
                    // SAFETY: killpg only sends a signal to the group this run made.
                    unsafe { libc::killpg(group as libc::pid_t, libc::SIGKILL) };
                }
                Err(format!("still running after {} seconds; killed with its process group", RUN_LIMIT.as_secs()))
            }
        }
    }
}

/// Holds the queues [`AppState::hook_queue`] keeps, one per hook.
pub(crate) type Runs = std::sync::Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<Job>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_events::Kind;

    #[test]
    fn a_file_names_its_hooks_and_skips_what_it_cannot_run() {
        let dir = crate::test_support::scratch();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("wheel.toml");
        std::fs::write(
            &file,
            r#"
[[hook]]
on = ["turn_end", "permission", "no_such_event"]
run = "tools/on-event"
inbox = "inbox"

[[hook]]
on = "permission"
run = "tools/decide"
decide = true

[[hook]]
on = ["turn_end"]

[[hook]]
on = ["nothing"]
run = "echo"

[[hook]]
on = ["fail"]
run = "true"
inbox = "/abs/box"
"#,
        )
        .unwrap();
        let hooks = hooks_in(&file, &dir);
        assert_eq!(hooks.len(), 3, "{hooks:?}");
        assert_eq!(hooks[0].on, ["turn_end", "permission"]);
        assert_eq!(hooks[0].inbox.as_deref(), Some(dir.join("inbox").as_path()));
        assert!(!hooks[0].decide);
        assert_eq!((hooks[1].on.as_slice(), hooks[1].run.as_str(), hooks[1].decide), (&["permission".to_string()][..], "tools/decide", true));
        assert_eq!(hooks[2].inbox.as_deref(), Some(Path::new("/abs/box")));
        assert!(hooks_in(&dir.join("missing.toml"), &dir).is_empty());
        std::fs::write(&file, "not = [toml").unwrap();
        assert!(hooks_in(&file, &dir).is_empty());
    }

    #[test]
    fn a_decision_is_the_first_word_and_anything_else_is_the_persons() {
        assert_eq!(decision("allow\n"), Some("allow"));
        assert_eq!(decision("  Deny because\n"), Some("deny"));
        assert_eq!(decision("ask"), None);
        assert_eq!(decision(""), None);
        assert_eq!(decision("allowed"), None);
    }

    #[test]
    fn every_event_on_names_has_a_kind() {
        let kinds = [Kind::Started, Kind::Finished, Kind::Failed, Kind::Permission, Kind::Answered, Kind::OverTime, Kind::BuildFailed];
        let names: Vec<&str> = kinds.into_iter().map(Kind::name).collect();
        assert_eq!(names, EVENTS);
    }
}
