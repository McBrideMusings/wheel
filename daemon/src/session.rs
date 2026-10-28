//! Agent sessions. Each one belongs to an agent pane, is named `w<n>` and runs in its space's
//! directory with the person's Claude configuration and Wheel's brief (ADR-0002), through the agent
//! interface (`crate::agent`). Messages queue and run one turn at a time; the conversation is kept
//! so a client that attaches later sees it; when the agent process stops, the daemon starts a new
//! one and the queue carries on. A session ends when its pane closes or changes kind, or gives up
//! when its agent stops `Restart::tries` times in a row before it is ready.
//!
//! A session remembers the agent's own id for its conversation, and an agent pane saves it. A
//! session restored from a saved layout resumes that conversation, and the history the agent's
//! store holds for it becomes the transcript; when the store no longer holds it, the session says
//! so and starts afresh.
//!
//! A session can also move between conversations while it runs: `conversations` lists what the
//! agent's store holds for its directory, and `switch_conversation` opens a fresh one or reopens a
//! past one in a new agent process, which replaces the one it ran. Either empties the transcript
//! and bumps `conversation_changes`, which makes every attached client re-attach and take the new
//! transcript whole. Both are refused while a turn is running or a message is queued.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use wheel_proto::{
    ConfigKind, ConfigOption, ContextUse, ConversationInfo, ServerMsg, SessionEvent, SessionState,
    StampedEvent, ToolContent,
};

use crate::agent::{self, Agent, AgentEvent, AgentInput, Conversation, Launch};
use crate::claude::{self, History};
use crate::perms::Perms;
use crate::resume::Agent as Harness;

/// Events kept for clients that attach later; the oldest go first.
const TRANSCRIPT_CAP: usize = 2000;

/// Bytes of text the stored transcript may hold, counted by `event_bytes`; the oldest events go
/// first. Tool calls carry whole-file diffs, so the event count alone does not bound the memory
/// or what every attach replays. Live events are never trimmed. The newest event stays even when
/// it alone is over the budget, unless it is a reply or thought: that loses the start of its text
/// instead, behind `TEXT_TRUNCATED`.
const TRANSCRIPT_BYTES: usize = 4 * 1024 * 1024;

/// What a reply or thought starts with once its beginning has been cut to fit the byte budget.
const TEXT_TRUNCATED: &str = "[earlier text truncated]\n";

/// An agent pane whose agent stops this many times in a row before it is ready gives up. With the
/// restart backoff that is about 15 seconds after the first failure.
const AGENT_TRIES: u32 = 5;

/// How a session's agent is started again after it stops. The defaults are the product's;
/// tests shorten the waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restart {
    /// Stops in a row before ready after which an agent pane's session gives up.
    pub tries: u32,
    /// The wait before the first restart; it doubles up to `backoff_max`.
    pub backoff: Duration,
    pub backoff_max: Duration,
    /// A run that lasted longer than this starts the backoff over.
    pub steady: Duration,
}

impl Default for Restart {
    fn default() -> Self {
        Restart {
            tries: AGENT_TRIES,
            backoff: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            steady: Duration::from_secs(60),
        }
    }
}

/// How a session's start came out: unset until its agent is first ready, the session gives up,
/// or it ends.
type Started = Option<Result<(), String>>;

/// What a client sees on attach, taken at one instant.
pub struct SessionAttachment {
    pub state: SessionState,
    pub events: Vec<StampedEvent>,
    pub config: Vec<ConfigOption>,
    pub context: Option<ContextUse>,
    pub rx: broadcast::Receiver<StampedEvent>,
}

enum Cmd {
    Prompt {
        turn: u64,
        text: String,
    },
    /// Stop `turn` if it is still the one running; a cancel for a turn that already ended is
    /// dropped, never applied to the next.
    Cancel {
        turn: u64,
    },
    /// Change one config option. The agent answers with the whole new list, so the reply says only
    /// whether it took it; the values reach clients as a `session_config` broadcast.
    SetConfig {
        config: String,
        value: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Put the session in another conversation: a fresh one, or the past one `load` names. Only
    /// read between turns, so the conversation never changes under a running prompt.
    Switch {
        load: Option<String>,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

struct Inner {
    /// The conversation so far, capped at `TRANSCRIPT_CAP`.
    events: VecDeque<StampedEvent>,
    /// `event_bytes` summed over `events`.
    event_bytes: usize,
    /// The `user` events of messages whose turn has not started, in turn order. Kept apart from
    /// `events` so the cap never drops a message that is still waiting to run.
    queued: VecDeque<StampedEvent>,
    /// The turn the agent is working on, from its `turn_start` until its end.
    running: Option<u64>,
    /// Queued turns cancelled before they started; each ends as `cancelled` when its turn comes.
    cancelled: HashSet<u64>,
    state: SessionState,
    /// The controls the running agent offers, as it last named them. Empty until a session opens;
    /// a new agent process replaces the list wholesale, since the options are the agent's, not the
    /// conversation's.
    config: Vec<ConfigOption>,
    /// A conversation change asked for and not yet answered. Lives here rather than beside the
    /// session's other flags so it is read and set under the one lock `send` takes, and so two
    /// switches cannot both believe they are the only one in flight.
    switching: bool,
    /// A status the person is told about this session — a conversation that would not reopen —
    /// held for as long as it is up. Never a transcript event and never saved: a client draws it
    /// and the person dismisses it, a conversation change clears it, and a daemon restart forgets it.
    notice: Option<String>,
    /// How full the conversation's context window is, as the agent last reported it. Unset until
    /// it reports one, and again when the session moves to another conversation. Saved with the
    /// pane so a restart shows it before the agent speaks.
    context: Option<ContextUse>,
    /// The slash commands the agent listed when its process started; none before it has.
    commands: Option<Vec<String>>,
}

pub struct Session {
    pub name: String,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<StampedEvent>,
    cmds: mpsc::UnboundedSender<Cmd>,
    next_turn: AtomicU64,
    /// Daemon-wide events, for state changes every client should see.
    global: broadcast::Sender<ServerMsg>,
    /// Runs the agent process; aborting it kills the process.
    runner: Mutex<Option<JoinHandle<()>>>,
    /// Set by `stop`, before it withdraws the session's permission requests.
    stopped: AtomicBool,
    /// The agent's id for this conversation, once it has one.
    conversation_id: Mutex<Option<String>>,
    /// Set until the first agent process decides whether a restored session resumes.
    restored: AtomicBool,
    /// The conversation id the owner should save so the next start reloads it: one the agent holds
    /// on disk. It writes a conversation out when the first prompt reaches it, so a conversation
    /// that never took a turn cannot be loaded again, and saving its id would make the next start
    /// fail to reopen it.
    saved_id: Mutex<Option<String>>,
    /// Set when a saved conversation failed to load: the id stays saved, and the new conversation
    /// the session works in meanwhile does not replace it until a switch.
    held: AtomicBool,
    /// Set when the running agent process reaches ready, so the runner tells a failed start from
    /// a crash.
    ready_this_run: AtomicBool,
    started: watch::Sender<Started>,
    /// Bumped whenever the session moves to another conversation, so every attached client
    /// re-attaches and takes the new transcript whole — the same resync a client that fell behind
    /// does.
    conversation: watch::Sender<u64>,
    /// Where the agent runs, and the config directory it runs with, which together name the
    /// store its conversations are read from.
    cwd: PathBuf,
    home: Option<PathBuf>,
    harness: Harness,
}

/// How a session's agent is started.
#[derive(Clone)]
pub struct SessionConfig {
    /// Which program `command` is.
    pub agent: Harness,
    /// The agent's program, e.g. `claude`, with any arguments of its own.
    pub command: String,
    pub cwd: PathBuf,
    /// The configuration directory the agent runs with (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), when
    /// it is not the agent's own default: a conversation Claude Code kept somewhere else is found there.
    pub home: Option<PathBuf>,
    /// Appended to the agent's system prompt: how to use Wheel (`crate::brief`). None leaves the
    /// agent's prompt as its configuration makes it.
    pub brief: Option<String>,
    /// Added to the agent process's environment: where `wheel` reaches the daemon, and the pane.
    pub env: Vec<(String, String)>,
    /// The session comes back from a saved layout: its first agent process resumes
    /// `conversation`, the pane's saved conversation, and its history becomes the transcript.
    pub restored: bool,
    pub conversation: Option<String>,
    /// Called each time the session's saved state changes — a new conversation id worth saving, or
    /// a new context use — so its pane saves it.
    pub on_saved: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The context use saved with the session, shown until the agent reports a fresh one. Only
    /// read for a restored session that resumes its conversation.
    pub context: Option<ContextUse>,
    pub restart: Restart,
    /// The permission mode and Codex sandbox every process of this session starts with.
    pub permission_mode: Option<String>,
    pub sandbox: Option<String>,
}

/// Messages waiting for a turn. Lives outside any one agent process, so a restart keeps them.
type Queue = VecDeque<(u64, String)>;

/// Holds a session's `switching` flag for as long as one conversation change is in flight, and
/// clears it however that change ends — answered, refused, or its caller dropped.
pub struct Switching<'a>(&'a Session);

/// Holds a session in `SessionState::Switching` while its conversation is handed to another
/// program. Dropping it puts the previous state back unless the session was stopped meanwhile.
pub struct Handoff<'a> {
    switching: Switching<'a>,
    previous: SessionState,
}

impl Drop for Switching<'_> {
    fn drop(&mut self) {
        self.0.inner.lock().unwrap().switching = false;
    }
}

impl Drop for Handoff<'_> {
    fn drop(&mut self) {
        let session = self.switching.0;
        // Only while nothing else has moved the state on: an agent that died during the hold
        // is `Failed`, and the snapshot taken before it would hide that.
        let still_switching = session.state() == SessionState::Switching;
        if still_switching && !session.stopped.load(Ordering::SeqCst) {
            session.set_state(self.previous.clone());
        }
    }
}

/// Why no agent is there to answer a command, or `None` when one is.
fn without_agent(state: &SessionState) -> Option<String> {
    match state {
        SessionState::Ready | SessionState::Busy { .. } => None,
        SessionState::Starting => Some("is still starting".to_string()),
        SessionState::Switching => Some("is switching to the terminal view".to_string()),
        SessionState::Failed { message } => {
            Some(format!("is failed (its agent stopped): {message}"))
        }
        SessionState::GaveUp { tries, message } => {
            Some(format!("gave up after {tries} tries: {message}"))
        }
    }
}

impl Session {
    /// Start the session's agent, restarting it whenever it stops, until `stop`. `pane` names
    /// the agent pane that owns a session, for the log.
    pub fn start(
        name: String,
        pane: Option<String>,
        config: SessionConfig,
        perms: Arc<Perms>,
        global: broadcast::Sender<ServerMsg>,
    ) -> Arc<Session> {
        let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
        let (tx, _) = broadcast::channel(4096);
        let session = Arc::new(Session {
            name,
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                event_bytes: 0,
                queued: VecDeque::new(),
                running: None,
                cancelled: HashSet::new(),
                state: SessionState::Starting,
                config: Vec::new(),
                switching: false,
                notice: None,
                commands: None,
                context: config.context.filter(|_| config.restored),
            }),
            tx,
            cmds,
            next_turn: AtomicU64::new(1),
            global,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
            conversation_id: Mutex::new(config.conversation.clone()),
            restored: AtomicBool::new(config.restored),
            saved_id: Mutex::new(config.conversation.clone()),
            held: AtomicBool::new(false),
            ready_this_run: AtomicBool::new(false),
            started: watch::Sender::new(None),
            conversation: watch::Sender::new(0),
            cwd: config.cwd.clone(),
            home: config.home.clone(),
            harness: config.agent,
        });
        let run = session.clone();
        let pane = pane.unwrap_or_else(|| "-".into());
        let runner = tokio::spawn(async move {
            let mut queue = Queue::new();
            let restart = config.restart;
            let mut backoff = restart.backoff;
            let name = run.name.clone();
            // Agent processes in a row that stopped before they were ready.
            let mut failures = 0;
            loop {
                run.set_state(SessionState::Starting);
                let started = tokio::time::Instant::now();
                tracing::info!(session = %name, %pane, command = %config.command, cwd = %config.cwd.display(), "session spawned");
                let result = run
                    .clone()
                    .run(&config, perms.clone(), &mut cmd_rx, &mut queue)
                    .await;
                perms.drop_session(&name);
                // A turn the agent was on when it stopped never ends; it is not running any more.
                run.inner.lock().unwrap().running = None;
                let message = match result {
                    Ok(()) => {
                        tracing::info!(session = %name, "session agent exited");
                        "the agent exited".to_string()
                    }
                    Err(e) => {
                        tracing::error!(session = %name, error = %e, "session failed");
                        e
                    }
                };
                failures = if run.ready_this_run.swap(false, Ordering::SeqCst) {
                    0
                } else {
                    failures + 1
                };
                if failures >= restart.tries {
                    tracing::error!(session = %name, %pane, tries = failures, error = %message, "session gave up");
                    let gave_up = format!("gave up after {failures} tries: {message}");
                    // Nothing will run the messages still waiting; a client attaching later sees
                    // them gone, not queued forever.
                    let mut inner = run.inner.lock().unwrap();
                    inner.queued.clear();
                    inner.cancelled.clear();
                    drop(inner);
                    run.push(SessionEvent::Error {
                        turn: None,
                        message: gave_up.clone(),
                    });
                    run.set_state(SessionState::GaveUp {
                        tries: failures,
                        message,
                    });
                    run.settle(Err(gave_up));
                    // Returning drops the command channel, so a later send says the session is
                    // not running.
                    return;
                }
                run.push(SessionEvent::Error {
                    turn: None,
                    message: format!("{message}; restarting it"),
                });
                run.set_state(SessionState::Failed { message });
                if started.elapsed() > restart.steady {
                    backoff = restart.backoff;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(restart.backoff_max);
            }
        });
        *session.runner.lock().unwrap() = Some(runner);
        session
    }

    /// End the session for good: kill its agent and withdraw its permission requests. Clients
    /// attached to it see an `error` with no turn, then their stream ends.
    pub fn stop(&self, perms: &Perms) {
        drop(self.halt(perms, true));
    }

    /// [`Session::stop`], returning once the agent's process group has been killed, so another
    /// program can open the conversation without the two writing its store at once. The runner
    /// holds the process and kills its group when dropped, which the abort does. Attached clients
    /// are not sent the `session ended` error: the conversation moves to the terminal, it does not end.
    pub async fn stop_and_wait(&self, perms: &Perms) {
        if let Some(runner) = self.halt(perms, false) {
            let _ = runner.await;
        }
        tracing::info!(session = %self.name, "session agent process gone");
    }

    /// `announce` pushes the `session ended` error to attached clients; without it the session
    /// goes `Failed` instead, and the pane's next view replaces it.
    fn halt(&self, perms: &Perms, announce: bool) -> Option<JoinHandle<()>> {
        self.stopped.store(true, Ordering::SeqCst);
        let runner = self.runner.lock().unwrap().take();
        if let Some(runner) = &runner {
            runner.abort();
        }
        perms.drop_session(&self.name);
        if announce {
            self.push(SessionEvent::Error {
                turn: None,
                message: wheel_proto::SESSION_ENDED.into(),
            });
        } else {
            // The handoff's `Switching` must not outlive the agent if the switch then fails.
            self.set_state(SessionState::Failed {
                message: wheel_proto::SESSION_ENDED.into(),
            });
        }
        self.settle(Err(wheel_proto::SESSION_ENDED.into()));
        tracing::info!(session = %self.name, announce, "session ended");
        runner
    }

    /// Hold the session between turns while its conversation is handed to another program: refused
    /// while a turn runs or a message waits, and every message sent while held is refused, as during
    /// a conversation change. The hold lasts until the guard drops.
    pub fn hold_for_handoff(&self) -> Result<Handoff<'_>, String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.switching {
            return Err(format!(
                "session {} is changing conversation; try again once that is done",
                self.name
            ));
        }
        if inner.running.is_some() || !inner.queued.is_empty() {
            return Err(format!(
                "session {} is working on a turn; switch once it ends, or cancel it",
                self.name
            ));
        }
        if inner.state == SessionState::Starting {
            return Err(format!(
                "session {} is still starting; switch once it is ready",
                self.name
            ));
        }
        inner.switching = true;
        let previous = inner.state.clone();
        drop(inner);
        self.set_state(SessionState::Switching);
        Ok(Handoff {
            switching: Switching(self),
            previous,
        })
    }

    /// Record how the start came out; only the first outcome counts.
    fn settle(&self, outcome: Result<(), String>) {
        self.started.send_if_modified(|started| {
            let first = started.is_none();
            if first {
                *started = Some(outcome);
            }
            first
        });
    }

    /// Wait until the agent is first ready (`Ok`), or the session gives up or ends (`Err` with
    /// why).
    pub async fn started(&self) -> Result<(), String> {
        let mut rx = self.started.subscribe();
        // The session holds the sender, so the channel cannot close while `self` lives.
        let outcome = rx
            .wait_for(Option::is_some)
            .await
            .map_err(|e| e.to_string())?;
        outcome.clone().unwrap_or(Ok(()))
    }

    /// Queue a message and show it to every client at once; returns the turn id its events will
    /// carry. The turn id, the queue and the `user` event are ordered under one lock, so `user`
    /// events appear in the order their turns will run and always before their `turn_start`.
    pub fn send(&self, text: String) -> Result<u64, String> {
        let mut inner = self.inner.lock().unwrap();
        // A message written while the session is moving to another conversation was written for
        // the one on screen. Queueing it would show it under that conversation and then run it in
        // the next one, so it is refused and the person still has their text.
        if inner.switching {
            return Err(format!(
                "session {} is changing conversation; send it again once that is done",
                self.name
            ));
        }
        match &inner.state {
            SessionState::GaveUp { tries, message } => {
                return Err(format!(
                    "session {} is gave_up after {tries} tries and takes no messages: {message}",
                    self.name
                ))
            }
            SessionState::Failed { message } => {
                return Err(format!(
                    "session {} is failed (its agent stopped) and takes no messages until it is ready again: {message}",
                    self.name
                ))
            }
            _ => {}
        }
        let turn = self.next_turn.fetch_add(1, Ordering::SeqCst);
        self.cmds
            .send(Cmd::Prompt {
                turn,
                text: text.clone(),
            })
            .map_err(|_| format!("session {} is not running", self.name))?;
        tracing::info!(session = %self.name, turn, chars = text.len(), "prompt queued");
        let event = StampedEvent::now(SessionEvent::User { turn, text });
        inner.queued.push_back(event.clone());
        let _ = self.tx.send(event);
        Ok(turn)
    }

    /// The runner took up `turn`: its `user` event joins the transcript, then `turn_start`.
    /// Returns whether the turn was cancelled while it waited, so it must not reach the agent.
    fn start_turn(&self, turn: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let position = inner
            .queued
            .iter()
            .position(|e| matches!(e.event, SessionEvent::User { turn: t, .. } if t == turn));
        if let Some(user) = position.and_then(|i| inner.queued.remove(i)) {
            Self::record(&mut inner, user);
        }
        inner.running = Some(turn);
        self.push_locked(&mut inner, SessionEvent::TurnStart { turn });
        inner.cancelled.remove(&turn)
    }

    /// The turn is over: `event` is its `turn_end`, or the `error` it failed with.
    fn end_turn(&self, event: SessionEvent) {
        let mut inner = self.inner.lock().unwrap();
        inner.running = None;
        self.push_locked(&mut inner, event);
    }

    /// Stop one turn: the one in progress, or else the next queued one, which then ends as
    /// `cancelled` without reaching the agent. Messages behind it still run, so stopping
    /// everything takes one cancel per turn. Returns the turn it stops; fails when no turn is
    /// running or queued, so a caller knows the cancel did nothing.
    pub fn cancel(&self) -> Result<u64, String> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(turn) = inner.running {
            tracing::info!(session = %self.name, turn, "cancel requested");
            self.cmds
                .send(Cmd::Cancel { turn })
                .map_err(|_| format!("session {} is not running", self.name))?;
            return Ok(turn);
        }
        let next = inner.queued.iter().find_map(|e| match &e.event {
            SessionEvent::User { turn, .. } if !inner.cancelled.contains(turn) => Some(*turn),
            _ => None,
        });
        let Some(turn) = next else {
            return Err(format!("session {} has no turn to cancel", self.name));
        };
        tracing::info!(session = %self.name, turn, "cancel requested before the turn started");
        inner.cancelled.insert(turn);
        Ok(turn)
    }

    /// The agent's id for this conversation, once it has one.
    pub fn conversation_id(&self) -> Option<String> {
        self.conversation_id.lock().unwrap().clone()
    }

    /// The conversation id to save so the next start reloads it, none while the conversation the
    /// session is in is still empty.
    pub fn saved_conversation(&self) -> Option<String> {
        self.saved_id.lock().unwrap().clone()
    }

    /// The conversations the agent's store holds for this session's directory, newest first, with
    /// the one the session is in flagged. The store belongs to the directory rather than to wheel,
    /// so this lists whatever else ran the same harness there too.
    pub async fn conversations(&self) -> Result<Vec<ConversationInfo>, String> {
        if self.harness != Harness::Claude {
            return Err(format!(
                "session {} runs {}, which does not list its conversations here",
                self.name,
                self.harness.name()
            ));
        }
        let home = claude::store(self.home.as_deref())
            .ok_or("no home directory to find the agent's conversations in")?;
        let cwd = self.cwd.clone();
        let mut listed = tokio::task::spawn_blocking(move || claude::conversations(&home, &cwd))
            .await
            .map_err(|e| e.to_string())?;
        // Read now rather than before the store was: a switch can land while a listing is in
        // flight, and the flag has to name the conversation the session is in when the list is made.
        let current = self.conversation_id();
        for info in &mut listed {
            info.current = current.as_deref() == Some(info.id.as_str());
        }
        tracing::info!(session = %self.name, cwd = %self.cwd.display(), listed = listed.len(), "conversations listed");
        Ok(listed)
    }

    /// Put the session in a fresh conversation, or in the past one `load` names, and return the
    /// agent's id for it. The transcript becomes that conversation's and every attached client is
    /// sent it; a load the agent refuses leaves the session where it was.
    pub async fn switch_conversation(&self, load: Option<String>) -> Result<String, String> {
        let (reply, answer) = oneshot::channel();
        let switching = {
            // The runner reads commands only between turns, and reads nothing at all while its
            // queue has messages in it, so a switch sent now would sit unanswered until the queue
            // drained — and would land the queued messages in a conversation they were not
            // written for. Both are the same refusal.
            let mut inner = self.inner.lock().unwrap();
            // One at a time: two switches in flight together would each clear the other's flag,
            // and the second would replace the conversation the first had just reported.
            if inner.switching {
                return Err(format!(
                    "session {} is already changing conversation",
                    self.name
                ));
            }
            if inner.running.is_some() || !inner.queued.is_empty() {
                return Err(format!(
                    "session {} is working; cancel its turn first",
                    self.name
                ));
            }
            // An agent that is starting, failed or has given up has no conversation to change. The
            // command would otherwise wait in the channel and change it whenever some later agent
            // process came up, long after the caller was told nothing happened.
            let why = without_agent(&inner.state);
            if let Some(why) = why {
                return Err(format!(
                    "session {} {why}, so it has no conversation to change",
                    self.name
                ));
            }
            // Set under the same lock `send` takes, so a message cannot slip in behind the checks
            // above. The guard clears it however this ends — a dropped future included, which
            // would otherwise leave the session refusing every message from then on.
            inner.switching = true;
            // Let the lock go before the guard exists: the guard takes the same lock to clear the
            // flag, and a failed send below drops it right here.
            drop(inner);
            let switching = Switching(self);
            self.cmds
                .send(Cmd::Switch { load, reply })
                .map_err(|_| format!("session {} is not running", self.name))?;
            switching
        };
        let switched = answer
            .await
            .map_err(|_| format!("session {} stopped before it answered", self.name));
        drop(switching);
        switched?
    }

    /// Fires each time the session moves to another conversation; a client's stream re-attaches on
    /// it. Subscribing marks the current conversation as seen, so a caller that subscribes before
    /// it reads the transcript cannot miss a change.
    pub fn conversation_changes(&self) -> watch::Receiver<u64> {
        self.conversation.subscribe()
    }

    /// The session is now in conversation `id`, which the agent's store already holds when
    /// `on_disk`: remember it, let its owner save it, and send every attached client the
    /// transcript that goes with it.
    fn took_conversation(
        &self,
        id: &str,
        on_disk: bool,
        save: Option<&Arc<dyn Fn() + Send + Sync>>,
    ) {
        *self.conversation_id.lock().unwrap() = Some(id.to_string());
        *self.saved_id.lock().unwrap() = on_disk.then(|| id.to_string());
        self.held.store(false, Ordering::SeqCst);
        if let Some(save) = save {
            save();
        }
        self.conversation.send_modify(|n| *n += 1);
    }

    /// Drop the conversation so far; the session is about to be in another one. Turn ids carry on
    /// climbing — every client matches turns by id, so none is ever reused.
    fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.events.clear();
        inner.event_bytes = 0;
        inner.cancelled.clear();
        drop(inner);
        self.set_context(None);
    }

    pub fn state(&self) -> SessionState {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn attach(&self) -> SessionAttachment {
        let inner = self.inner.lock().unwrap();
        SessionAttachment {
            state: inner.state.clone(),
            events: inner.events.iter().chain(&inner.queued).cloned().collect(),
            config: inner.config.clone(),
            context: inner.context,
            rx: self.tx.subscribe(),
        }
    }

    /// Change one config option. Refused at once, naming the phase, while the session has no
    /// agent; otherwise waits for the agent to take it, so a caller learns why it was refused. The new values reach every client as a `session_config` broadcast.
    pub async fn set_config(&self, config: String, value: String) -> Result<(), String> {
        // Refused while no agent is there: the command would otherwise wait in the channel until
        // one connects, which for a slow agent is tens of seconds, and answer with a stop if it
        // never does.
        if let Some(why) = without_agent(&self.inner.lock().unwrap().state) {
            return Err(format!(
                "session {} {why}, so it has no config to change",
                self.name
            ));
        }
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(Cmd::SetConfig {
                config,
                value,
                reply,
            })
            .map_err(|_| format!("session {} is not running", self.name))?;
        answer
            .await
            .map_err(|_| format!("session {} stopped before it answered", self.name))?
    }

    /// The permission mode the running agent is in: the current value of its `mode` option.
    pub fn permission_mode(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner
            .config
            .iter()
            .find_map(|option| match (&option.kind, option.category.as_deref()) {
                (ConfigKind::Select { current, .. }, Some("mode")) => Some(current.clone()),
                _ => None,
            })
    }

    /// Record a mode the agent changed on its own. Only the option the agent put in the `mode`
    /// category moves. The whole read-modify-write runs under one lock, so a set landing at the
    /// same moment cannot be overwritten by a stale copy.
    fn set_current_mode(&self, mode: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(option) = inner
            .config
            .iter_mut()
            .find(|option| option.category.as_deref() == Some("mode"))
        else {
            return;
        };
        let ConfigKind::Select { current, .. } = &mut option.kind else {
            return;
        };
        if current != mode {
            *current = mode.to_string();
            self.announce_config(&inner);
        }
    }

    /// Replace the stored options and tell every client. Called with whatever the agent last sent:
    /// its start, a set's reply, or a change it made itself.
    fn set_config_options(&self, options: Vec<ConfigOption>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.config == options {
            return;
        }
        inner.config = options;
        self.announce_config(&inner);
    }

    /// How full the conversation's context window is, as the agent last reported it.
    pub fn context(&self) -> Option<ContextUse> {
        self.inner.lock().unwrap().context
    }

    /// Store the context use the agent reported and tell every client; true when it changed.
    /// Under the lock `attach` reads, for the reason `announce_config` gives.
    fn set_context(&self, context: Option<ContextUse>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.context == context {
            return false;
        }
        inner.context = context;
        let _ = self.global.send(ServerMsg::SessionContext {
            session: self.name.clone(),
            context,
        });
        true
    }

    /// The slash commands the agent accepts, once it has listed them.
    pub fn commands(&self) -> Option<Vec<String>> {
        self.inner.lock().unwrap().commands.clone()
    }

    /// Store the agent's command list and tell every client. Under the lock `attach` reads, for the
    /// reason `announce_config` gives.
    fn set_commands(&self, commands: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.commands.as_ref() == Some(&commands) {
            return;
        }
        inner.commands = Some(commands.clone());
        let _ = self.global.send(ServerMsg::SessionCommands {
            session: self.name.clone(),
            commands,
        });
    }

    /// The notice a client shows over this session now, if one is up.
    pub fn notice(&self) -> Option<String> {
        self.inner.lock().unwrap().notice.clone()
    }

    /// Put a notice up over the session, or take it down with `None`, and tell every client. Under
    /// the lock `attach` reads, for the reason `announce_config` gives.
    pub fn set_notice(&self, notice: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.notice == notice {
            return;
        }
        inner.notice = notice.clone();
        let _ = self.global.send(ServerMsg::SessionNotice {
            session: self.name.clone(),
            notice,
        });
    }

    /// Send the stored options to every client, under the lock that holds them, so two updaters
    /// cannot store A then B and broadcast B then A — and so an `attach` cannot hand out a list
    /// older than a broadcast the same client already saw.
    fn announce_config(&self, inner: &Inner) {
        let _ = self.global.send(ServerMsg::SessionConfig {
            session: self.name.clone(),
            options: inner.config.clone(),
        });
    }

    /// Stamp an event, record it and broadcast it, under the lock attach() reads, so no attach
    /// misses or repeats one. Consecutive text chunks are merged in the stored transcript.
    fn push(&self, event: SessionEvent) {
        self.push_locked(&mut self.inner.lock().unwrap(), event);
    }

    fn push_locked(&self, inner: &mut Inner, event: SessionEvent) {
        let event = StampedEvent::now(event);
        Self::record(inner, event.clone());
        let _ = self.tx.send(event);
    }

    /// Add an event to the stored transcript only. A merged chunk keeps the first chunk's stamp,
    /// so a reply or a thought is dated from when it began.
    fn record(inner: &mut Inner, event: StampedEvent) {
        match (inner.events.back_mut().map(|e| &mut e.event), &event.event) {
            (Some(SessionEvent::Text { text: last }), SessionEvent::Text { text })
            | (Some(SessionEvent::Thought { text: last }), SessionEvent::Thought { text }) => {
                last.push_str(text);
                inner.event_bytes += text.len();
            }
            _ => {
                inner.event_bytes += event_bytes(&event.event);
                inner.events.push_back(event);
            }
        }
        while inner.events.len() > TRANSCRIPT_CAP
            || (inner.event_bytes > TRANSCRIPT_BYTES && inner.events.len() > 1)
        {
            if let Some(old) = inner.events.pop_front() {
                inner.event_bytes -= event_bytes(&old.event);
            }
        }
        Self::trim_lone_text(inner);
    }

    /// A single reply or thought that grew past the byte budget on its own: drop the start of its
    /// text and put `TEXT_TRUNCATED` there, so a client sees the cut. Trims down to seven eighths
    /// of the budget, so a reply still growing does not copy its text on every chunk.
    fn trim_lone_text(inner: &mut Inner) {
        if inner.event_bytes <= TRANSCRIPT_BYTES {
            return;
        }
        let Some(StampedEvent {
            event: SessionEvent::Text { text } | SessionEvent::Thought { text },
            ..
        }) = inner.events.back_mut()
        else {
            return;
        };
        let kept_marker = if text.starts_with(TEXT_TRUNCATED) {
            TEXT_TRUNCATED.len()
        } else {
            0
        };
        let target = TRANSCRIPT_BYTES - TRANSCRIPT_BYTES / 8;
        let mut cut = kept_marker + (inner.event_bytes - target);
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.replace_range(kept_marker..cut, "");
        if kept_marker == 0 {
            text.insert_str(0, TEXT_TRUNCATED);
        }
        inner.event_bytes = inner.events.iter().map(|e| event_bytes(&e.event)).sum();
    }

    /// Add a resumed conversation's history to the transcript, numbering its turns now. A message
    /// sent while the agent was starting already holds a turn id, so it can carry a lower id than
    /// the history; ids stay unique, and every client matches turns by id. Returns how many events
    /// the transcript holds.
    ///
    /// Recorded without broadcasting: a history is a whole transcript arriving at once, not a
    /// conversation happening. A client that was sent each event live would show the history
    /// appended to whatever it already had. `took_conversation` tells it instead, and it takes the
    /// transcript whole.
    fn commit_history(&self, history: History) -> usize {
        let mut inner = self.inner.lock().unwrap();
        // The history is read with no times the transcript keeps, so its events carry none.
        let mut record = |event| Self::record(&mut inner, StampedEvent::unstamped(event));
        for event in history.before {
            record(event);
        }
        for (text, output) in history.turns {
            let turn = self.next_turn.fetch_add(1, Ordering::SeqCst);
            record(SessionEvent::User { turn, text });
            record(SessionEvent::TurnStart { turn });
            for event in output {
                record(event);
            }
            let stop_reason = "end_turn".into();
            record(SessionEvent::TurnEnd { turn, stop_reason });
        }
        inner.events.len()
    }

    fn set_state(&self, state: SessionState) {
        self.inner.lock().unwrap().state = state.clone();
        let _ = self.global.send(ServerMsg::SessionStateChanged {
            session: self.name.clone(),
            state,
        });
    }

    /// Start one agent process for this session in `conversation`.
    fn launch(&self, config: &SessionConfig, conversation: Conversation) -> Result<Agent, String> {
        tracing::info!(session = %self.name, conversation = ?conversation, command = %config.command, "agent launch");
        agent::launch(Launch {
            agent: config.agent,
            command: config.command.clone(),
            cwd: config.cwd.clone(),
            config_dir: config.home.clone(),
            brief: config.brief.clone(),
            env: config.env.clone(),
            conversation,
            permission_mode: config.permission_mode.clone(),
            sandbox: config.sandbox.clone(),
        })
    }

    /// The agent's processes, from the first spawn until one stops. A switch to another
    /// conversation starts a new process, which replaces the running one once it is ready.
    async fn run(
        self: Arc<Self>,
        config: &SessionConfig,
        perms: Arc<Perms>,
        cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
        queue: &mut Queue,
    ) -> Result<(), String> {
        // An agent started in a missing directory fails in ways that differ per agent; say why.
        if !config.cwd.is_dir() {
            return Err(format!("{} is not a directory", config.cwd.display()));
        }
        // The options belong to the agent process, not the conversation. Drop the last one's
        // before this one starts, so one that never gets that far cannot leave a dead process's
        // controls on screen for a client to set.
        self.set_config_options(Vec::new());
        let home = claude::store(config.home.as_deref());
        // A restored agent pane's first agent process resumes the conversation the layout names;
        // every later one resumes the one the session is in, if the agent holds it on disk. The
        // flag clears once one is ready, so a start that fails tries the saved conversation again
        // rather than letting a fresh one replace it.
        let first = self.restored.load(Ordering::SeqCst);
        let saved = if first {
            self.conversation_id()
        } else if self.held.load(Ordering::SeqCst) {
            // The session works in a conversation the saved one did not give way to; resuming the
            // saved one would swap it for a transcript the agent never had.
            None
        } else {
            self.saved_conversation()
        };
        // Claude Code keeps its conversations where wheel reads them; Codex hands a resumed
        // conversation's history over when it opens it.
        let reads_store = config.agent == Harness::Claude;
        let mut history = match (&saved, &home) {
            (Some(id), Some(home)) if reads_store => {
                let (home, cwd, id) = (home.clone(), config.cwd.clone(), id.clone());
                let found = tokio::task::spawn_blocking(move || claude::history(&home, &cwd, &id))
                    .await
                    .map_err(|e| e.to_string())?;
                if found.is_none() {
                    tracing::warn!(session = %self.name, conversation = ?saved, "saved conversation is not in the agent's store");
                }
                found
            }
            _ => None,
        };
        let started = tokio::time::Instant::now();
        let (mut agent, options, conversation, reported) = match (
            &saved,
            history.is_some() || !reads_store,
        ) {
            (Some(id), true) => {
                let conversation = Conversation::Resume(id.clone());
                // A driver that finds the conversation gone before starting its agent (pi's)
                // refuses the launch; that is the same failure as an agent that will not open it.
                let opened = match self.launch(config, conversation.clone()) {
                    Ok(mut agent) => wait_ready(&mut agent).await.map(|ready| (agent, ready)),
                    Err(e) => Err(e),
                };
                match opened {
                    Ok((agent, ready)) => {
                        history = ready.history.or(history);
                        (
                            agent,
                            ready.options,
                            conversation.named(ready.conversation.unwrap_or_else(|| id.clone())),
                            ready.context,
                        )
                    }
                    // A conversation the store holds but the agent will not open: this cannot tell
                    // a damaged transcript from a passing failure, so the id stays saved.
                    Err(e) => {
                        tracing::warn!(session = %self.name, conversation = %id, error = %e, "session resume failed");
                        history = None;
                        let conversation = Conversation::New(claude::new_conversation_id());
                        let mut agent = self.launch(config, conversation.clone())?;
                        let ready = wait_ready(&mut agent).await?;
                        let conversation = ready
                            .conversation
                            .map_or(conversation.clone(), |id| conversation.named(id));
                        (agent, ready.options, conversation, ready.context)
                    }
                }
            }
            _ => {
                let conversation = Conversation::New(claude::new_conversation_id());
                let mut agent = self.launch(config, conversation.clone())?;
                let ready = wait_ready(&mut agent).await?;
                let conversation = ready
                    .conversation
                    .map_or(conversation.clone(), |id| conversation.named(id));
                (agent, ready.options, conversation, ready.context)
            }
        };
        let start_ms = started.elapsed().as_millis() as u64;
        self.restored.store(false, Ordering::SeqCst);
        self.set_config_options(options);
        // An agent that reports its context before it is ready (pi, on a resume) has the figure
        // shown before the first new turn; `Some(None)` clears a figure saved with the pane.
        if let Some(context) = reported {
            if self.set_context(context) {
                if let Some(saved) = &config.on_saved {
                    saved();
                }
            }
        }
        match history {
            Some(history) => {
                // A relaunch resumes the conversation the transcript already holds, so the history
                // would be a second copy of it.
                let events = if first {
                    self.commit_history(history)
                } else {
                    self.inner.lock().unwrap().events.len()
                };
                tracing::info!(session = %self.name, conversation = %conversation.id(), events, start_ms, "session resumed");
                // A client can attach while the agent is still starting, so it is told the
                // transcript the same way a switch tells it. Its owner saved this id already — it
                // is the one that was read back.
                self.took_conversation(conversation.id(), true, None);
            }
            None => {
                let load_failed = saved.is_some();
                if load_failed {
                    tracing::info!(session = %self.name, "session not resumable");
                    self.set_notice(Some(wheel_proto::SESSION_NOT_REOPENED.into()));
                }
                // A saved figure belongs to the conversation that did not come back.
                self.set_context(None);
                *self.conversation_id.lock().unwrap() = Some(conversation.id().to_string());
                // A conversation that would not reopen keeps its saved id: this cannot tell a
                // deleted one from a passing failure, and overwriting would turn one flaky start
                // into the permanent loss of a conversation the agent still has. The new one runs
                // this session; the next start tries the saved one again.
                self.held.store(load_failed, Ordering::SeqCst);
                if !load_failed {
                    *self.saved_id.lock().unwrap() = None;
                    if let Some(saved) = &config.on_saved {
                        saved();
                    }
                }
            }
        }
        tracing::info!(session = %self.name, conversation = %conversation.id(), start_ms, "session ready");
        self.ready_this_run.store(true, Ordering::SeqCst);
        self.settle(Ok(()));
        self.set_state(idle(queue));

        // The turn the agent is working on.
        let mut running: Option<u64> = None;
        loop {
            if running.is_none() {
                if let Some((turn, text)) = queue.pop_front() {
                    let cancelled = self.start_turn(turn);
                    self.set_state(SessionState::Busy {
                        queued: queue.len(),
                    });
                    if cancelled {
                        tracing::info!(session = %self.name, turn, "prompt cancelled before it started");
                        self.end_turn(SessionEvent::TurnEnd {
                            turn,
                            stop_reason: "cancelled".into(),
                        });
                        self.set_state(idle(queue));
                        continue;
                    }
                    self.save_on_first_prompt(config);
                    tracing::info!(session = %self.name, turn, chars = text.len(), "prompt start");
                    if agent.input.prompt(text).is_err() {
                        return Err(exit_reason(&mut agent).await);
                    }
                    running = Some(turn);
                    continue;
                }
            }
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    None => return Ok(()),
                    Some(Cmd::Prompt { turn, text }) => {
                        queue.push_back((turn, text));
                        if running.is_some() {
                            self.set_state(SessionState::Busy { queued: queue.len() });
                        }
                    }
                    Some(Cmd::Cancel { turn }) if running == Some(turn) => {
                        tracing::info!(session = %self.name, turn, "prompt cancel");
                        // A cancelled turn's open permission requests are answered as withdrawn.
                        perms.drop_session(&self.name);
                        if agent.input.cancel().is_err() {
                            return Err(exit_reason(&mut agent).await);
                        }
                    }
                    Some(Cmd::Cancel { turn }) => {
                        tracing::info!(session = %self.name, turn, "cancel dropped; that turn already ended");
                    }
                    // Mid-turn is exactly when a mode or model change is worth making, so this
                    // does not wait for the turn to end.
                    Some(Cmd::SetConfig { config: option, value, reply }) => {
                        spawn_config_set(agent.input.clone(), self.clone(), option, value, reply);
                    }
                    Some(Cmd::Switch { reply, .. }) if running.is_some() || !queue.is_empty() => {
                        // Unreachable by any route a client has: `switch_conversation` refuses
                        // while a turn is running. Answered rather than dropped, so a caller could
                        // never hang on it.
                        let _ = reply.send(Err(format!("session {} is working; cancel its turn first", self.name)));
                    }
                    // Awaited here rather than spawned: a switch replaces the process this loop
                    // prompts, so it must be done before the next message is read, and there is
                    // no turn running to starve.
                    Some(Cmd::Switch { load, reply }) => {
                        match self.switch(config, load).await {
                            Ok((next, id)) => {
                                // The process left behind is killed as it drops; the conversation
                                // it was in stays in the store.
                                agent = next;
                                let _ = reply.send(Ok(id));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                },
                event = agent.events.recv() => match event {
                    None => return Ok(()),
                    Some(AgentEvent::Exited { error }) => return error.map_or(Ok(()), Err),
                    Some(AgentEvent::Transcript(event)) => self.push(event),
                    Some(AgentEvent::Permission { request, title, tool_call, options }) => {
                        self.ask(&perms, agent.input.clone(), request, title, options, tool_call);
                    }
                    // How full the window is: not part of the transcript. Its owner saves it with
                    // the pane.
                    Some(AgentEvent::Context(context)) => {
                        tracing::info!(session = %self.name, used = ?context.map(|c| c.used), size = ?context.map(|c| c.size), "context use reported");
                        if self.set_context(context) {
                            if let Some(saved) = &config.on_saved {
                                saved();
                            }
                        }
                    }
                    // Folded into the `mode` option so a client has one place to read the mode
                    // from.
                    Some(AgentEvent::Mode(mode)) => self.set_current_mode(&mode),
                    Some(AgentEvent::Commands(commands)) => self.set_commands(commands),
                    // The agent wrote nothing for the new conversation yet, so its id is not
                    // saved until a prompt reaches it, and the old transcript goes.
                    Some(AgentEvent::Conversation { id }) => {
                        self.reset();
                        self.took_conversation(&id, false, config.on_saved.as_ref());
                    }
                    Some(AgentEvent::Ready { options, .. } | AgentEvent::Options(options)) => {
                        self.set_config_options(options);
                    }
                    Some(AgentEvent::TurnEnd { stop_reason }) => {
                        if let Some(turn) = running.take() {
                            // Nothing this turn asked for can still be answered once it is over.
                            perms.drop_session(&self.name);
                            tracing::info!(session = %self.name, turn, %stop_reason, "prompt end");
                            self.end_turn(SessionEvent::TurnEnd { turn, stop_reason });
                            self.set_state(idle(queue));
                        }
                    }
                    Some(AgentEvent::TurnFailed { message }) => {
                        if let Some(turn) = running.take() {
                            perms.drop_session(&self.name);
                            tracing::warn!(session = %self.name, turn, error = %message, "prompt failed");
                            self.end_turn(SessionEvent::Error { turn: Some(turn), message });
                            self.set_state(idle(queue));
                        }
                    }
                },
            }
        }
    }

    /// The agent writes the conversation out when a prompt reaches it, so from the first one its
    /// id is worth saving — unless a saved conversation that would not reopen is being held.
    fn save_on_first_prompt(&self, config: &SessionConfig) {
        if self.held.load(Ordering::SeqCst) {
            return;
        }
        let current = self.conversation_id();
        let changed = {
            let mut saved_id = self.saved_id.lock().unwrap();
            let changed = *saved_id != current;
            *saved_id = current;
            changed
        };
        if let (true, Some(saved)) = (changed, &config.on_saved) {
            saved();
        }
    }

    /// Open a permission request for every client; the first answer goes back to the agent.
    fn ask(
        &self,
        perms: &Arc<Perms>,
        input: AgentInput,
        request: String,
        title: String,
        options: Vec<wheel_proto::PermOption>,
        tool_call: Option<String>,
    ) {
        let (_, answer) = perms.open(&self.name, title, options, tool_call);
        // A runner still finishing after `stop` must not leave a request nobody can answer:
        // `stop` sets the flag before it withdraws, so one of the two withdraws it.
        if self.stopped.load(Ordering::SeqCst) {
            perms.drop_session(&self.name);
        }
        // Answering waits on a person; never block the loop for it. A request withdrawn at the
        // end of its turn resolves as withdrawn.
        tokio::spawn(async move {
            let choice = answer.await.ok();
            let _ = input.answer(request, choice);
        });
    }

    /// Move the session into a fresh conversation, or the past one `load` names, in a new agent
    /// process. Returns that process and the conversation's id once it is ready; a conversation the
    /// store does not hold, or a process that stops before it is ready, leaves the session where it
    /// was.
    async fn switch(
        &self,
        config: &SessionConfig,
        load: Option<String>,
    ) -> Result<(Agent, String), String> {
        let (conversation, history) = match load {
            None => (Conversation::New(claude::new_conversation_id()), None),
            Some(id) if config.agent == Harness::Claude => {
                let home = claude::store(config.home.as_deref());
                let (cwd, wanted) = (config.cwd.clone(), id.clone());
                let history = tokio::task::spawn_blocking(move || {
                    home.and_then(|home| claude::history(&home, &cwd, &wanted))
                })
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| {
                    format!(
                        "the agent keeps no conversation {id} for {}",
                        config.cwd.display()
                    )
                })?;
                (Conversation::Resume(id), Some(history))
            }
            // The agent hands over the history when it opens the conversation, or refuses.
            Some(id) => (Conversation::Resume(id), None),
        };
        let started = tokio::time::Instant::now();
        let mut agent = self.launch(config, conversation.clone())?;
        let ready = wait_ready(&mut agent).await.map_err(|e| {
            tracing::warn!(session = %self.name, conversation = ?conversation, error = %e, "conversation change failed");
            e
        })?;
        let load_ms = started.elapsed().as_millis() as u64;
        let id = ready
            .conversation
            .unwrap_or_else(|| conversation.id().to_string());
        let options = ready.options;
        let reported = ready.context;
        let history = history.or(ready.history);
        self.reset();
        let resumed = history.is_some();
        if let Some(history) = history {
            let events = self.commit_history(history);
            tracing::info!(session = %self.name, conversation = %id, events, load_ms, "conversation resumed");
        } else {
            tracing::info!(session = %self.name, conversation = %id, load_ms, "conversation opened");
        }
        self.set_config_options(options);
        if let Some(context) = reported {
            self.set_context(context);
        }
        self.set_notice(None);
        self.took_conversation(&id, resumed, config.on_saved.as_ref());
        Ok((agent, id))
    }
}

/// The state a session between turns is in: ready, or busy with what is queued.
fn idle(queue: &Queue) -> SessionState {
    if queue.is_empty() {
        SessionState::Ready
    } else {
        SessionState::Busy {
            queued: queue.len(),
        }
    }
}

/// Why an agent process whose input is closed stopped: its exit, read from the events it left.
async fn exit_reason(agent: &mut Agent) -> String {
    while let Some(event) = agent.events.recv().await {
        if let AgentEvent::Exited { error } = event {
            return error.unwrap_or_else(|| "the agent exited".into());
        }
    }
    "the agent exited".into()
}

/// What a new agent process reports when it is ready.
struct Ready {
    options: Vec<ConfigOption>,
    conversation: Option<String>,
    history: Option<History>,
    /// The context figure the agent reported before it was ready (`Some(None)` is "no figure").
    context: Option<Option<ContextUse>>,
}

/// Wait for a new agent process to be ready, returning what it reports, or why it stopped first.
async fn wait_ready(agent: &mut Agent) -> Result<Ready, String> {
    let mut context = None;
    loop {
        match agent.events.recv().await {
            Some(AgentEvent::Ready {
                options,
                conversation,
                history,
            }) => {
                return Ok(Ready {
                    options,
                    conversation,
                    history,
                    context,
                })
            }
            Some(AgentEvent::Context(reported)) => context = Some(reported),
            Some(AgentEvent::Exited { error }) => {
                return Err(error.unwrap_or_else(|| "the agent exited before it was ready".into()))
            }
            None => return Err("the agent exited before it was ready".into()),
            Some(other) => tracing::info!(event = ?other, "agent event before ready dropped"),
        }
    }
}

/// Run a config set beside the loop that reads commands rather than inside it. The set waits on
/// the agent, and a slow agent must not stop a cancel from being heard.
fn spawn_config_set(
    input: AgentInput,
    session: Arc<Session>,
    config: String,
    value: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    let known = session
        .inner
        .lock()
        .unwrap()
        .config
        .iter()
        .any(|option| option.id == config);
    if !known {
        let _ = reply.send(Err(format!(
            "session {} has no config option {config}",
            session.name
        )));
        return;
    }
    let conversation = session.conversation_id();
    tokio::spawn(async move {
        let answer = match input.set_option(config, value).await {
            // This task outlives the agent process it is talking to. If the session has moved to
            // another conversation while the reply was in flight, the list belongs to a process
            // nobody is talking to any more.
            Ok(_) if session.conversation_id() != conversation => Err(format!(
                "session {} restarted before the agent answered",
                session.name
            )),
            Ok(options) => {
                session.set_config_options(options);
                Ok(())
            }
            Err(e) => Err(e),
        };
        let _ = reply.send(answer);
    });
}

/// What an event adds to the transcript budget: its text, including diff text, plus a flat 64 for
/// the rest.
fn event_bytes(event: &SessionEvent) -> usize {
    64 + match event {
        SessionEvent::User { text, .. }
        | SessionEvent::Text { text }
        | SessionEvent::Thought { text } => text.len(),
        SessionEvent::ToolCall { title, content, .. } => title.len() + content_bytes(content),
        SessionEvent::ToolUpdate { title, content, .. } => {
            title.as_ref().map_or(0, String::len) + content.as_deref().map_or(0, content_bytes)
        }
        SessionEvent::Error { message, .. } => message.len(),
        SessionEvent::Plan { entries } => entries.iter().map(|e| e.content.len()).sum(),
        SessionEvent::TurnStart { .. }
        | SessionEvent::TurnEnd { .. }
        | SessionEvent::Mode { .. } => 0,
    }
}

/// Bytes of text and diff text.
fn content_bytes(content: &[ToolContent]) -> usize {
    content
        .iter()
        .map(|c| match c {
            ToolContent::Text { text } => text.len(),
            ToolContent::Diff {
                old_text, new_text, ..
            } => old_text.as_ref().map_or(0, String::len) + new_text.len(),
            ToolContent::Unknown => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session with no runner: `send` queues into a channel nobody reads.
    fn idle_session() -> (Session, mpsc::UnboundedReceiver<Cmd>) {
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let session = Session {
            name: "w1".into(),
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                event_bytes: 0,
                queued: VecDeque::new(),
                running: None,
                cancelled: HashSet::new(),
                state: SessionState::Ready,
                config: Vec::new(),
                switching: false,
                notice: None,
                commands: None,
                context: None,
            }),
            tx: broadcast::channel(16).0,
            cmds,
            next_turn: AtomicU64::new(1),
            global: broadcast::channel(16).0,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
            conversation_id: Mutex::new(None),
            restored: AtomicBool::new(false),
            saved_id: Mutex::new(None),
            held: AtomicBool::new(false),
            ready_this_run: AtomicBool::new(false),
            started: watch::Sender::new(None),
            conversation: watch::Sender::new(0),
            cwd: PathBuf::from("/"),
            home: None,
            harness: Harness::Claude,
        };
        (session, cmd_rx)
    }

    fn tool(n: usize) -> SessionEvent {
        SessionEvent::ToolCall {
            id: n.to_string(),
            title: "ls".into(),
            tool_kind: "execute".into(),
            status: "pending".into(),
            content: vec![],
        }
    }

    #[test]
    fn a_long_turn_never_drops_a_queued_message() {
        let (session, _rx) = idle_session();
        session.send("first".into()).unwrap();
        session.start_turn(1);
        session.send("second".into()).unwrap();
        for n in 0..TRANSCRIPT_CAP + 10 {
            session.push(tool(n));
        }
        let events: Vec<SessionEvent> = session
            .attach()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect();
        assert_eq!(events.len(), TRANSCRIPT_CAP + 1);
        assert_eq!(
            events.last(),
            Some(&SessionEvent::User {
                turn: 2,
                text: "second".into()
            })
        );
        session.start_turn(2);
        let events: Vec<SessionEvent> = session
            .attach()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect();
        assert_eq!(
            events[events.len() - 2..],
            [
                SessionEvent::User {
                    turn: 2,
                    text: "second".into()
                },
                SessionEvent::TurnStart { turn: 2 }
            ]
        );
    }

    #[test]
    fn one_reply_over_the_byte_budget_loses_its_start() {
        let (session, _rx) = idle_session();
        for _ in 0..6 {
            session.push(SessionEvent::Text {
                text: format!("{}é", "x".repeat(1024 * 1024 - 2)),
            });
        }
        session.push(SessionEvent::Text { text: "END".into() });
        let events: Vec<SessionEvent> = session
            .attach()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect();
        let held: usize = events.iter().map(event_bytes).sum();
        assert!(held <= TRANSCRIPT_BYTES, "{held} bytes held");
        assert_eq!(events.len(), 1);
        let SessionEvent::Text { text } = &events[0] else {
            panic!("not text: {events:?}");
        };
        assert!(text.starts_with(TEXT_TRUNCATED));
        assert_eq!(text.matches(TEXT_TRUNCATED).count(), 1);
        assert!(text.ends_with("END"));
    }

    #[test]
    fn large_diffs_keep_the_transcript_under_its_byte_budget() {
        let (session, _rx) = idle_session();
        session.send("first".into()).unwrap();
        session.start_turn(1);
        session.send("second".into()).unwrap();
        for n in 0..20 {
            session.push(SessionEvent::ToolCall {
                id: n.to_string(),
                title: "Edit".into(),
                tool_kind: "edit".into(),
                status: "completed".into(),
                content: vec![ToolContent::Diff {
                    path: "a.rs".into(),
                    old_text: Some("a".repeat(256 * 1024)),
                    new_text: "b".repeat(256 * 1024),
                }],
            });
        }
        let events: Vec<SessionEvent> = session
            .attach()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect();
        let held: usize = events.iter().map(event_bytes).sum();
        assert!(held <= TRANSCRIPT_BYTES + 64, "{held} bytes held");
        let calls = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ToolCall { .. }))
            .count();
        assert!((6..=8).contains(&calls), "{calls} tool calls held");
        assert!(events
            .iter()
            .any(|e| matches!(e, SessionEvent::ToolCall { id, .. } if id == "19")));
        assert_eq!(
            events.last(),
            Some(&SessionEvent::User {
                turn: 2,
                text: "second".into()
            })
        );
    }

    #[test]
    fn each_cancel_names_one_turn() {
        let (session, mut rx) = idle_session();
        assert_eq!(
            session.cancel(),
            Err("session w1 has no turn to cancel".into())
        );
        session.send("first".into()).unwrap();
        session.send("second".into()).unwrap();
        session.send("third".into()).unwrap();
        // Before any turn starts, a cancel takes the next queued turn not already cancelled.
        assert_eq!(session.cancel(), Ok(1));
        assert!(session.start_turn(1));
        session.end_turn(SessionEvent::TurnEnd {
            turn: 1,
            stop_reason: "cancelled".into(),
        });
        // A running turn is cancelled through the runner, by its own id.
        assert!(!session.start_turn(2));
        assert_eq!(session.cancel(), Ok(2));
        session.end_turn(SessionEvent::TurnEnd {
            turn: 2,
            stop_reason: "cancelled".into(),
        });
        // Turn 3 never saw a cancel: the one sent for turn 2 cannot reach it.
        assert!(!session.start_turn(3));
        let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|cmd| match cmd {
                Cmd::Cancel { turn } => Some(turn),
                Cmd::Prompt { .. } | Cmd::SetConfig { .. } | Cmd::Switch { .. } => None,
            })
            .collect();
        assert_eq!(sent, [2]);
        session.end_turn(SessionEvent::TurnEnd {
            turn: 3,
            stop_reason: "end_turn".into(),
        });
        assert_eq!(
            session.cancel(),
            Err("session w1 has no turn to cancel".into())
        );
    }

    /// A mode the agent changes on its own lands on the option it put in the `mode` category, and
    /// leaves every other option alone.
    #[test]
    fn a_mode_the_agent_changes_itself_moves_only_the_mode_option() {
        let (session, _rx) = idle_session();
        session.set_config_options(vec![
            ConfigOption {
                id: "mode".into(),
                name: "Mode".into(),
                description: None,
                category: Some("mode".into()),
                kind: ConfigKind::Select {
                    current: "default".into(),
                    choices: vec![],
                },
            },
            ConfigOption {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                category: Some("model".into()),
                kind: ConfigKind::Select {
                    current: "sonnet".into(),
                    choices: vec![],
                },
            },
        ]);

        session.set_current_mode("plan");

        let stored = session.inner.lock().unwrap().config.clone();
        let current = |id: &str| match &stored.iter().find(|o| o.id == id).unwrap().kind {
            ConfigKind::Select { current, .. } => current.clone(),
            other => panic!("expected a select, got {other:?}"),
        };
        assert_eq!(current("mode"), "plan");
        assert_eq!(current("model"), "sonnet");
    }
}
