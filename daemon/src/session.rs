//! ACP agent sessions. Each one belongs to an agent pane, is named `w<n>` and runs in its space's
//! directory with the person's Claude configuration and Wheel's brief (ADR-0002). Messages queue
//! and run one turn at a time; the conversation is kept so a client that attaches later sees it;
//! when the agent process stops, the daemon starts a new one and the queue carries on. A session
//! ends when its pane closes or changes kind, or gives up when its agent stops `Restart::tries`
//! times in a row before it is ready.
//!
//! A session remembers its agent's own id for the conversation, and an agent pane saves it. A
//! session restored from a saved layout loads that conversation when the agent can (ACP
//! `loadSession`), and the history the agent replays becomes the transcript; when it cannot, the
//! session says so and starts afresh.
//!
//! A session can also move between conversations while it runs: `conversations` lists what the
//! agent holds for its directory (ACP `session/list`), and `switch_conversation` opens a fresh one
//! or reopens a past one on the agent it is already talking to. Either empties the transcript and
//! bumps `conversation_changes`, which makes every attached client re-attach and take the new
//! transcript whole. Both are refused while a turn is running or a message is queued. The
//! conversation left behind is closed (ACP `session/close`) when the agent can close one.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, CloseSessionRequest, ContentBlock,
    EmbeddedResourceResource, InitializeRequest, ListSessionsRequest, LoadSessionRequest, Meta,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionConfigId, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionValue, SessionConfigSelectOption,
    SessionConfigSelectOptions, SessionId, SessionInfo, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, TextContent, ToolCallContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use wheel_proto::{
    ConfigChoice, ConfigKind, ConfigOption, ContextUse, ConversationInfo, PermOption, PlanItem,
    ServerMsg, SessionEvent, SessionState, StampedEvent, ToolContent,
};

use crate::perms::Perms;

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
    /// What conversations the agent holds for the session's directory.
    ListConversations {
        reply: oneshot::Sender<Result<Vec<ConversationInfo>, String>>,
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
    acp_session: Mutex<Option<String>>,
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
    /// Gathers the history while the agent replays a loaded session.
    replay: Mutex<Option<Replay>>,
    /// Set when the running agent process reaches ready, so the runner tells a failed start from
    /// a crash.
    ready_this_run: AtomicBool,
    started: watch::Sender<Started>,
    /// What the running agent process said it can do with past conversations: list them
    /// (`session/list`), load one (`session/load`) and close the one it leaves (`session/close`).
    /// Unset until a process initializes.
    lists_conversations: AtomicBool,
    loads_conversations: AtomicBool,
    closes_conversations: AtomicBool,
    /// Bumped whenever the session moves to another conversation, so every attached client
    /// re-attaches and takes the new transcript whole — the same resync a client that fell behind
    /// does.
    conversation: watch::Sender<u64>,
}

/// How a session's agent is started.
#[derive(Clone)]
pub struct SessionConfig {
    /// The agent command line, e.g. `claude-agent-acp`.
    pub command: String,
    pub cwd: PathBuf,
    /// The Claude config directory an agent pane's agent runs with, as `CLAUDE_CONFIG_DIR`, when
    /// it is not the daemon's own: a conversation Claude Code kept somewhere else is found there.
    pub claude_dir: Option<PathBuf>,
    /// Appended to the agent's system prompt as `_meta.systemPrompt.append`: how to use Wheel
    /// (`crate::brief`). None leaves the agent's prompt as its configuration makes it.
    pub brief: Option<String>,
    /// Added to the agent process's environment: where `wheel` reaches the daemon, and the pane.
    pub env: Vec<(String, String)>,
    /// The session comes back from a saved layout: its first agent process loads `acp_session`,
    /// the pane's saved conversation, and the agent's replay of it becomes the transcript.
    pub restored: bool,
    pub acp_session: Option<String>,
    /// Called each time the session's saved state changes — the agent gives it a new conversation
    /// id, or reports a new context use — so its pane saves it.
    pub on_acp_session: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The context use saved with the session, shown until the agent reports a fresh one. Only
    /// read for a restored session that loads its conversation.
    pub context: Option<ContextUse>,
    pub restart: Restart,
}

/// A loaded session's history as the agent replays it, kept apart until the load succeeds so a
/// failed load leaves nothing behind. Each replayed message becomes a whole turn (`user`,
/// `turn_start`, the agent's output, `turn_end`); its chunks are joined first, since a `user` event
/// is sent whole.
#[derive(Default)]
struct Replay {
    /// The conversation being replayed. A conversation the session left can still speak on the
    /// same agent, and its output is not this history.
    conversation: String,
    /// Agent output before the first replayed message.
    before: Vec<SessionEvent>,
    /// Each replayed message with the agent's output after it.
    turns: Vec<(String, Vec<SessionEvent>)>,
    /// Set while user chunks arrive: the message they belong to, when the agent names it.
    user_message: Option<Option<String>>,
}

impl Replay {
    fn new(conversation: String) -> Replay {
        Replay {
            conversation,
            ..Replay::default()
        }
    }

    fn add(&mut self, session: &str, update: SessionUpdate) {
        if let SessionUpdate::UserMessageChunk(chunk) = update {
            let ContentBlock::Text(t) = chunk.content else {
                return;
            };
            let id = chunk.message_id.map(|m| m.to_string());
            // Chunks of one message are consecutive; a new message id starts another.
            let same = matches!(&self.user_message, Some(last) if id.is_none() || *last == id);
            match self.turns.last_mut() {
                Some((text, _)) if same => text.push_str(&t.text),
                _ => self.turns.push((t.text, Vec::new())),
            }
            self.user_message = Some(id);
            return;
        }
        let Some(event) = map_update(session, update) else {
            return;
        };
        self.user_message = None;
        match self.turns.last_mut() {
            Some((_, output)) => output.push(event),
            None => self.before.push(event),
        }
    }
}

/// Messages waiting for a turn. Lives outside any one agent process, so a restart keeps them.
type Queue = VecDeque<(u64, String)>;

/// Holds a session's `switching` flag for as long as one conversation change is in flight, and
/// clears it however that change ends — answered, refused, or its caller dropped.
pub struct Switching<'a>(&'a Session);

impl Drop for Switching<'_> {
    fn drop(&mut self) {
        self.0.inner.lock().unwrap().switching = false;
    }
}

/// Why no agent is there to answer a command, or `None` when one is.
fn without_agent(state: &SessionState) -> Option<String> {
    match state {
        SessionState::Ready | SessionState::Busy { .. } => None,
        SessionState::Starting => Some("is still starting".to_string()),
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
                context: config.context.filter(|_| config.restored),
            }),
            tx,
            cmds,
            next_turn: AtomicU64::new(1),
            global,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
            acp_session: Mutex::new(config.acp_session.clone()),
            restored: AtomicBool::new(config.restored),
            saved_id: Mutex::new(config.acp_session.clone()),
            held: AtomicBool::new(false),
            replay: Mutex::new(None),
            ready_this_run: AtomicBool::new(false),
            started: watch::Sender::new(None),
            lists_conversations: AtomicBool::new(false),
            loads_conversations: AtomicBool::new(false),
            closes_conversations: AtomicBool::new(false),
            conversation: watch::Sender::new(0),
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
        drop(self.halt(perms));
    }

    /// [`Session::stop`], returning once the agent's process group has been killed, so another
    /// program can open the conversation without the two writing its store at once. The runner
    /// holds the process and kills its group when dropped, which the abort does.
    pub async fn stop_and_wait(&self, perms: &Perms) {
        if let Some(runner) = self.halt(perms) {
            let _ = runner.await;
        }
        tracing::info!(session = %self.name, "session agent process gone");
    }

    fn halt(&self, perms: &Perms) -> Option<JoinHandle<()>> {
        self.stopped.store(true, Ordering::SeqCst);
        let runner = self.runner.lock().unwrap().take();
        if let Some(runner) = &runner {
            runner.abort();
        }
        perms.drop_session(&self.name);
        self.push(SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_ENDED.into(),
        });
        self.settle(Err(wheel_proto::SESSION_ENDED.into()));
        tracing::info!(session = %self.name, "session ended");
        runner
    }

    /// Hold the session between turns while its conversation is handed to another program: refused
    /// while a turn runs or a message waits, and every message sent while held is refused, as during
    /// a conversation change. The hold lasts until the guard drops.
    pub fn hold_for_handoff(&self) -> Result<Switching<'_>, String> {
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
        inner.switching = true;
        drop(inner);
        Ok(Switching(self))
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
    pub fn acp_session(&self) -> Option<String> {
        self.acp_session.lock().unwrap().clone()
    }

    /// The conversation id to save so the next start reloads it, none while the conversation the
    /// session is in is still empty.
    pub fn saved_conversation(&self) -> Option<String> {
        self.saved_id.lock().unwrap().clone()
    }

    /// The conversations the agent holds for this session's directory, newest first, with the one
    /// the session is in flagged. The agent's store belongs to the directory rather than to wheel,
    /// so this lists whatever else ran the same harness there too.
    pub async fn conversations(&self) -> Result<Vec<ConversationInfo>, String> {
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(Cmd::ListConversations { reply })
            .map_err(|_| format!("session {} is not running", self.name))?;
        answer
            .await
            .map_err(|_| format!("session {} stopped before it answered", self.name))?
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

    /// The agent has put the session in conversation `id`. Stored before the transcript is
    /// replaced, so an update arriving in between is judged against the conversation it was sent
    /// for; `took_conversation` then saves the id and tells every client.
    fn entered_conversation(&self, id: &str, on_disk: bool) {
        *self.acp_session.lock().unwrap() = Some(id.to_string());
        *self.saved_id.lock().unwrap() = on_disk.then(|| id.to_string());
        self.held.store(false, Ordering::SeqCst);
    }

    /// The session is now in conversation `id`: remember it, let its owner save it, and send every
    /// attached client the transcript that goes with it.
    fn took_conversation(&self, id: String, save: Option<&Arc<dyn Fn() + Send + Sync>>) {
        *self.acp_session.lock().unwrap() = Some(id);
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

    /// Record a mode the agent changed on its own. Only the option the agent put in the `mode`
    /// category moves. The whole read-modify-write runs under one lock, so a set landing at the
    /// same moment cannot be overwritten by a stale copy.
    ///
    /// Returns whether there was a `mode` option to fold it into; when there is none the caller
    /// records the change in the transcript instead, so it is not lost.
    fn set_current_mode(&self, mode: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(option) = inner
            .config
            .iter_mut()
            .find(|option| option.category.as_deref() == Some("mode"))
        else {
            return false;
        };
        let ConfigKind::Select { current, .. } = &mut option.kind else {
            return false;
        };
        if current != mode {
            *current = mode.to_string();
            self.announce_config(&inner);
        }
        true
    }

    /// Replace the stored options and tell every client. Called with whatever the agent last sent:
    /// `session/new`, a set's reply, or a mode it changed itself.
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

    /// Add a loaded session's history to the transcript, numbering its turns now. A message sent
    /// while the agent was loading already holds a turn id, so it can carry a lower id than the
    /// history; ids stay unique, and every client matches turns by id. Returns how many events the
    /// transcript holds.
    ///
    /// Recorded without broadcasting: a replay is a whole transcript arriving at once, not a
    /// conversation happening. A client that was sent each event live would show the history
    /// appended to whatever it already had. `took_conversation` tells it instead, and it takes the
    /// transcript whole.
    fn commit_replay(&self, replay: Replay) -> usize {
        let mut inner = self.inner.lock().unwrap();
        // The agent replays a conversation with no times, so its events carry none.
        let mut record = |event| Self::record(&mut inner, StampedEvent::unstamped(event));
        for event in replay.before {
            record(event);
        }
        for (text, output) in replay.turns {
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

    /// One agent process, from spawn until it stops.
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
        // before this one opens a session, so an agent that names none — or one that never gets
        // that far — cannot leave a dead process's controls on screen for a client to set.
        self.set_config_options(Vec::new());
        // So is a half-gathered replay: a process that dies during a `session/load` leaves one
        // behind, and an agent whose updates were still being diverted into it would reach no
        // client at all. Dropping it here is the one place that cannot be skipped.
        *self.replay.lock().unwrap() = None;
        let mut agent = AcpAgent::from_str(&config.command)
            .map_err(|e| format!("bad agent command {:?}: {e}", config.command))?
            .into_config();
        if let Some(dir) = &config.claude_dir {
            agent = agent.env("CLAUDE_CONFIG_DIR", dir.display().to_string());
            tracing::info!(session = %self.name, config_dir = %dir.display(), "agent runs with the conversation's config directory");
        }
        for (key, value) in &config.env {
            agent = agent.env(key, value);
        }
        let agent = AcpAgent::new(agent);
        let on_update = self.clone();
        let on_perm = perms.clone();
        let alive = self.clone();
        let main = self.clone();
        let name = self.name.clone();
        let asker = self.name.clone();
        let cwd = config.cwd.clone();
        let brief = config.brief.clone();
        let on_acp_session = config.on_acp_session.clone();
        // The update handler below moves its own copy of the same callback.
        let save_on_update = config.on_acp_session.clone();
        Client
            .builder()
            .on_receive_notification(
                async move |n: SessionNotification, _cx| {
                    // A conversation the session has moved on from can still speak: an update
                    // already in flight when it was closed, or one from an agent that cannot close
                    // it. Its output belongs to no transcript anyone is looking at, and least of all
                    // to the history being replayed for another conversation.
                    let from = n.session_id.to_string();
                    if let Some(replay) = on_update.replay.lock().unwrap().as_mut() {
                        if replay.conversation == from {
                            replay.add(&on_update.name, n.update);
                        } else {
                            tracing::info!(session = %on_update.name, acp_session = %from, replaying = %replay.conversation, "update from another conversation while replaying");
                        }
                        return Ok(());
                    }
                    if on_update.acp_session().is_some_and(|id| id != from) {
                        tracing::info!(session = %on_update.name, acp_session = %from, "update from a conversation this session has left");
                        return Ok(());
                    }
                    // The agent can change its own permission mode — leaving plan mode, say — and
                    // says so here rather than in a config option. Fold it into the `mode` option
                    // so a client has one place to read the mode from. With no such option to fold it
                    // into, it goes to the transcript like any other update.
                    if let SessionUpdate::CurrentModeUpdate(update) = &n.update {
                        if on_update.set_current_mode(&update.current_mode_id.to_string()) {
                            return Ok(());
                        }
                    }
                    // How full the window is: not part of the transcript, so it never reaches
                    // `map_update`. Its owner saves it with the pane.
                    if let SessionUpdate::UsageUpdate(update) = &n.update {
                        let context = ContextUse { used: update.used, size: update.size };
                        tracing::info!(session = %on_update.name, used = context.used, size = context.size, "context use reported");
                        if on_update.set_context(Some(context)) {
                            if let Some(saved) = &save_on_update {
                                saved();
                            }
                        }
                        return Ok(());
                    }
                    if let Some(event) = map_update(&on_update.name, n.update) {
                        on_update.push(event);
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |req: RequestPermissionRequest, responder, cx| {
                    let title = req.tool_call.fields.title.clone().unwrap_or_else(|| req.tool_call.tool_call_id.to_string());
                    let options: Vec<PermOption> =
                        req.options.iter().map(|o| PermOption { id: o.option_id.to_string(), name: o.name.clone(), kind: wire(&o.kind) }).collect();
                    let tool_call = Some(req.tool_call.tool_call_id.to_string());
                    let (_, answer) = on_perm.open(&asker, title, options, tool_call);
                    // A runner still finishing a poll after `stop` must not leave a request nobody can
                    // answer: `stop` sets the flag before it withdraws, so one of the two withdraws it.
                    if alive.stopped.load(Ordering::SeqCst) {
                        on_perm.drop_session(&asker);
                    }
                    // Answering waits on a person; never block the dispatch loop for it. A request
                    // withdrawn at the end of its turn resolves as Cancelled.
                    cx.spawn(async move {
                        let outcome = match answer.await {
                            Ok(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option)),
                            Err(_) => RequestPermissionOutcome::Cancelled,
                        };
                        responder.respond(RequestPermissionResponse::new(outcome))
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, async move |conn: ConnectionTo<Agent>| {
                let init = conn.send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(ClientCapabilities::new())).block_task().await?;
                let caps = &init.agent_capabilities;
                main.loads_conversations.store(caps.load_session, Ordering::SeqCst);
                main.lists_conversations.store(caps.session_capabilities.list.is_some(), Ordering::SeqCst);
                main.closes_conversations.store(caps.session_capabilities.close.is_some(), Ordering::SeqCst);
                tracing::info!(
                    session = %name,
                    load = caps.load_session,
                    list = caps.session_capabilities.list.is_some(),
                    close = caps.session_capabilities.close.is_some(),
                    "agent capabilities"
                );
                let meta = brief.as_ref().map(|brief| {
                    let mut meta = serde_json::Map::new();
                    meta.insert("systemPrompt".into(), serde_json::json!({ "append": brief }));
                    // The agent runs `wheel` without asking: the person answers for what it does
                    // through Wheel's own permission handling, not through a prompt per command.
                    meta.insert("claudeCode".into(), serde_json::json!({ "options": { "allowedTools": ["Bash(wheel:*)"] } }));
                    meta
                });
                // Only a restored agent pane's first agent process loads; any later one starts afresh.
                let saved = main.restored.swap(false, Ordering::SeqCst).then(|| main.acp_session()).flatten();
                let mut loaded = None;
                // Why the conversation did not come back, so the person is told the true reason:
                // an agent that cannot load at all, or one that no longer has this conversation.
                let mut load_failed = false;
                if let Some(id) = saved.clone().filter(|_| caps.load_session) {
                    *main.replay.lock().unwrap() = Some(Replay::new(id.clone()));
                    let request = LoadSessionRequest::new(id.clone(), cwd.clone()).meta(meta.clone());
                    let load_started = tokio::time::Instant::now();
                    let result = conn.send_request(request).block_task().await;
                    let load_ms = load_started.elapsed().as_millis() as u64;
                    let replay = main.replay.lock().unwrap().take().unwrap_or_default();
                    match result {
                        Ok(response) => {
                            let events = main.commit_replay(replay);
                            tracing::info!(session = %name, acp_session = %id, events, load_ms, "session resumed");
                            *main.saved_id.lock().unwrap() = Some(id.clone());
                            main.set_config_options(config_options(response.config_options.unwrap_or_default()));
                            // A client can attach while the agent is still loading, so it is told
                            // the replayed transcript the same way a switch tells it. Its owner
                            // saved this id already — it is the one that was read back.
                            main.took_conversation(id.clone(), None);
                            loaded = Some(id);
                        }
                        Err(e) => {
                            tracing::warn!(session = %name, acp_session = %id, error = %e, load_ms, "session load failed");
                            load_failed = true;
                            main.held.store(true, Ordering::SeqCst);
                        }
                    }
                }
                let mut session = match loaded {
                    Some(id) => SessionId::new(id),
                    None => {
                        if saved.is_some() {
                            let message = if load_failed { wheel_proto::SESSION_NOT_REOPENED } else { wheel_proto::SESSION_NOT_RESUMED };
                            tracing::info!(session = %name, load_failed, "session not resumable");
                            main.set_notice(Some(message.into()));
                        }
                        if !load_failed {
                            *main.saved_id.lock().unwrap() = None;
                        }
                        let request = NewSessionRequest::new(cwd.clone()).meta(meta.clone());
                        let response = conn.send_request(request).block_task().await?;
                        main.set_config_options(config_options(response.config_options.unwrap_or_default()));
                        // A saved figure belongs to the conversation that did not come back.
                        main.set_context(None);
                        let session = response.session_id;
                        *main.acp_session.lock().unwrap() = Some(session.to_string());
                        // A load that failed keeps the saved conversation: this cannot tell a
                        // deleted one from a timeout, and overwriting would turn one flaky start
                        // into the permanent loss of a conversation the agent still has. The new
                        // one runs this session; the next start tries the saved one again.
                        if let (Some(saved), false) = (&on_acp_session, load_failed) {
                            saved();
                        }
                        session
                    }
                };
                tracing::info!(session = %name, acp_session = %session, "session ready");
                main.ready_this_run.store(true, Ordering::SeqCst);
                main.settle(Ok(()));
                main.set_state(if queue.is_empty() { SessionState::Ready } else { SessionState::Busy { queued: queue.len() } });

                loop {
                    let (turn, text) = match queue.pop_front() {
                        Some(next) => next,
                        None => match cmd_rx.recv().await {
                            Some(Cmd::Prompt { turn, text }) => (turn, text),
                            // A cancel for a turn that has already ended.
                            Some(Cmd::Cancel { .. }) => continue,
                            Some(Cmd::SetConfig { config, value, reply }) => {
                                spawn_config_set(conn.clone(), session.clone(), main.clone(), config, value, reply);
                                continue;
                            }
                            Some(Cmd::ListConversations { reply }) => {
                                spawn_conversations(conn.clone(), main.clone(), cwd.clone(), reply);
                                continue;
                            }
                            // Awaited here rather than spawned: a switch replaces the conversation
                            // this loop prompts in, so it must be done before the next message is
                            // read, and there is no turn running to starve.
                            Some(Cmd::Switch { load, reply }) => {
                                let switched = switch_conversation(&conn, &main, &cwd, &meta, load).await;
                                if let Ok(id) = &switched {
                                    let left = std::mem::replace(&mut session, SessionId::new(id.clone()));
                                    main.set_notice(None);
                                    main.took_conversation(id.clone(), on_acp_session.as_ref());
                                    // Reopening the conversation it is already in leaves nothing.
                                    if left != session {
                                        spawn_close(conn.clone(), main.clone(), left);
                                    }
                                }
                                let _ = reply.send(switched);
                                continue;
                            }
                            None => return Ok(()),
                        },
                    };
                    let cancelled = main.start_turn(turn);
                    main.set_state(SessionState::Busy { queued: queue.len() });
                    if cancelled {
                        tracing::info!(session = %name, turn, "prompt cancelled before it started");
                        main.end_turn(SessionEvent::TurnEnd { turn, stop_reason: "cancelled".into() });
                        main.set_state(if queue.is_empty() { SessionState::Ready } else { SessionState::Busy { queued: queue.len() } });
                        continue;
                    }
                    // The agent writes the conversation out when this prompt reaches it, so from
                    // here its id is worth saving.
                    if !main.held.load(Ordering::SeqCst) {
                        let current = main.acp_session();
                        let changed = {
                            let mut saved_id = main.saved_id.lock().unwrap();
                            let changed = *saved_id != current;
                            *saved_id = current;
                            changed
                        };
                        if let (true, Some(saved)) = (changed, &on_acp_session) {
                            saved();
                        }
                    }
                    tracing::info!(session = %name, turn, chars = text.len(), "prompt start");
                    let request = conn.send_request(PromptRequest::new(session.clone(), vec![ContentBlock::Text(TextContent::new(text.clone()))])).block_task();
                    tokio::pin!(request);
                    let result = loop {
                        tokio::select! {
                            result = &mut request => break result,
                            Some(cmd) = cmd_rx.recv() => match cmd {
                                Cmd::Prompt { turn, text } => {
                                    queue.push_back((turn, text));
                                    main.set_state(SessionState::Busy { queued: queue.len() });
                                }
                                Cmd::Cancel { turn: stale } if stale != turn => {
                                    tracing::info!(session = %name, turn = stale, "cancel dropped; that turn already ended");
                                }
                                Cmd::Cancel { .. } => {
                                    tracing::info!(session = %name, turn, "prompt cancel");
                                    // ACP: a client that cancels answers its open permission requests as cancelled.
                                    perms.drop_session(&name);
                                    conn.send_notification(CancelNotification::new(session.clone()))?;
                                }
                                // Mid-turn is exactly when a mode or model change is worth making, so
                                // this does not wait for the turn to end.
                                Cmd::SetConfig { config, value, reply } => {
                                    spawn_config_set(conn.clone(), session.clone(), main.clone(), config, value, reply);
                                }
                                // Reading the list takes an agent round-trip and changes nothing,
                                // so it answers mid-turn like a config set.
                                Cmd::ListConversations { reply } => {
                                    spawn_conversations(conn.clone(), main.clone(), cwd.clone(), reply);
                                }
                                // Unreachable by any route a client has: `switch_conversation`
                                // refuses while a turn is running. The arm answers rather than
                                // dropping the reply, so a caller could never hang on it.
                                Cmd::Switch { reply, .. } => {
                                    let _ = reply.send(Err(format!("session {name} is working; cancel its turn first")));
                                }
                            },
                        }
                    };
                    // Nothing this turn asked for can still be answered once it is over.
                    perms.drop_session(&name);
                    match result {
                        Ok(response) => {
                            let stop_reason = wire(&response.stop_reason);
                            tracing::info!(session = %name, turn, %stop_reason, "prompt end");
                            main.end_turn(SessionEvent::TurnEnd { turn, stop_reason });
                        }
                        Err(e) => {
                            tracing::warn!(session = %name, turn, error = %e, "prompt failed");
                            main.end_turn(SessionEvent::Error { turn: Some(turn), message: e.to_string() });
                        }
                    }
                    main.set_state(if queue.is_empty() { SessionState::Ready } else { SessionState::Busy { queued: queue.len() } });
                }
            })
            .await
            .map_err(|e| e.to_string())
    }
}

/// An agent's config options as the wire carries them.
fn config_options(options: Vec<SessionConfigOption>) -> Vec<ConfigOption> {
    options
        .into_iter()
        .map(|option| ConfigOption {
            id: option.id.to_string(),
            name: option.name,
            description: option.description,
            category: option.category.as_ref().map(wire),
            kind: config_kind(option.kind),
        })
        .collect()
}

/// One control as the wire carries it. A select's choices are flattened out of whatever groups the
/// agent put them in, keeping their order; a kind this wheel does not know still gets a row, so a
/// person can see the control exists.
fn config_kind(kind: SessionConfigKind) -> ConfigKind {
    match kind {
        SessionConfigKind::Select(select) => {
            // A shape of select this wheel does not know carries choices it cannot list, so it is
            // Unknown rather than a select with none: an empty select would look settable and
            // offer nothing to set it to.
            let choices = match select.options {
                SessionConfigSelectOptions::Ungrouped(choices) => choices,
                SessionConfigSelectOptions::Grouped(groups) => {
                    groups.into_iter().flat_map(|group| group.options).collect()
                }
                _ => return ConfigKind::Unknown,
            };
            ConfigKind::Select {
                current: select.current_value.to_string(),
                choices: choices.into_iter().map(config_choice).collect(),
            }
        }
        SessionConfigKind::Boolean(boolean) => ConfigKind::Boolean {
            current: boolean.current_value,
        },
        _ => ConfigKind::Unknown,
    }
}

/// Run a config set beside the loop that reads commands rather than inside it. The set waits on
/// the agent, and a slow agent must not stop a cancel from being heard: awaiting it in the
/// `select!` arm would leave the next `Cmd` unread for as long as the agent took. The agent
/// answers its own requests in order, so two sets still cannot cross.
fn spawn_config_set(
    conn: ConnectionTo<Agent>,
    acp_session: SessionId,
    session: Arc<Session>,
    config: String,
    value: String,
    reply: oneshot::Sender<Result<(), String>>,
) {
    tokio::spawn(async move {
        let _ = reply.send(apply_config(&conn, &acp_session, &session, config, value).await);
    });
}

/// Read the agent's conversations for `cwd` beside the loop that reads commands, so a slow answer
/// cannot stop a cancel from being heard.
fn spawn_conversations(
    conn: ConnectionTo<Agent>,
    session: Arc<Session>,
    cwd: PathBuf,
    reply: oneshot::Sender<Result<Vec<ConversationInfo>, String>>,
) {
    tokio::spawn(async move {
        let _ = reply.send(list_conversations(&conn, &session, cwd).await);
    });
}

/// How many pages of `session/list` to take. A listing is a menu for a person; an agent that keeps
/// handing out cursors past this has more than anyone will read.
const CONVERSATION_PAGES: usize = 20;

/// Every conversation the agent holds for `cwd`, newest first, flagging the one this session is in.
async fn list_conversations(
    conn: &ConnectionTo<Agent>,
    session: &Session,
    cwd: PathBuf,
) -> Result<Vec<ConversationInfo>, String> {
    if !session.lists_conversations.load(Ordering::SeqCst) {
        return Err(format!(
            "session {}'s agent cannot list past conversations",
            session.name
        ));
    }
    let mut listed: Vec<SessionInfo> = Vec::new();
    let mut cursor = None;
    for page in 0..CONVERSATION_PAGES {
        let request = ListSessionsRequest::new().cwd(cwd.clone()).cursor(cursor);
        let response = conn
            .send_request(request)
            .block_task()
            .await
            .map_err(|e| e.to_string())?;
        listed.extend(response.sessions);
        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
        if page + 1 == CONVERSATION_PAGES {
            tracing::warn!(session = %session.name, pages = CONVERSATION_PAGES, listed = listed.len(), "the agent has more conversations than this lists");
        }
    }
    tracing::info!(session = %session.name, cwd = %cwd.display(), listed = listed.len(), "conversations listed");
    // The agent gives each one an ISO 8601 time, which sorts as text; one with no time at all goes
    // last rather than being guessed at.
    listed.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    // Read now rather than before the agent answered: a switch can land while a listing is in
    // flight, and the flag has to name the conversation the session is in when the list is made.
    let current = session.acp_session();
    Ok(listed
        .into_iter()
        .map(|info| {
            let id = info.session_id.to_string();
            ConversationInfo {
                current: current.as_deref() == Some(id.as_str()),
                id,
                cwd: info.cwd.display().to_string(),
                title: info.title,
                updated_at: info.updated_at,
            }
        })
        .collect())
}

/// Move the session into another conversation on the agent it is already talking to: a fresh one,
/// or the past one `load` names, whose replayed history becomes the transcript. Returns the agent's
/// id for it; a load the agent refuses leaves the transcript and the stored id untouched, so the
/// session stays in the conversation it was in.
async fn switch_conversation(
    conn: &ConnectionTo<Agent>,
    session: &Arc<Session>,
    cwd: &Path,
    meta: &Option<Meta>,
    load: Option<String>,
) -> Result<String, String> {
    let Some(id) = load else {
        let request = NewSessionRequest::new(cwd.to_path_buf()).meta(meta.clone());
        let response = conn
            .send_request(request)
            .block_task()
            .await
            .map_err(|e| e.to_string())?;
        let id = response.session_id.to_string();
        // Stored before anything else: from the agent's answer onwards every update belongs to
        // this conversation, and an update is kept or dropped by comparing it to this id.
        session.entered_conversation(&id, false);
        session.reset();
        session.set_config_options(config_options(response.config_options.unwrap_or_default()));
        tracing::info!(session = %session.name, acp_session = %id, "conversation opened");
        return Ok(id);
    };
    if !session.loads_conversations.load(Ordering::SeqCst) {
        return Err(format!(
            "session {}'s agent cannot reopen a past conversation",
            session.name
        ));
    }
    *session.replay.lock().unwrap() = Some(Replay::new(id.clone()));
    let request = LoadSessionRequest::new(id.clone(), cwd.to_path_buf()).meta(meta.clone());
    let load_started = tokio::time::Instant::now();
    let result = conn.send_request(request).block_task().await;
    let load_ms = load_started.elapsed().as_millis() as u64;
    if result.is_ok() {
        // Before the replay buffer closes, for the same reason as a fresh conversation: an update
        // arriving between the two is judged against the conversation it belongs to.
        session.entered_conversation(&id, true);
    }
    // Taken whether the load worked or not: a failed load leaves no half-replayed history behind.
    let replay = session.replay.lock().unwrap().take().unwrap_or_default();
    let response = result.map_err(|e| {
        tracing::warn!(session = %session.name, acp_session = %id, error = %e, load_ms, "conversation load failed");
        e.to_string()
    })?;
    session.reset();
    let events = session.commit_replay(replay);
    session.set_config_options(config_options(response.config_options.unwrap_or_default()));
    tracing::info!(session = %session.name, acp_session = %id, events, load_ms, "conversation resumed");
    Ok(id)
}

/// End the conversation the session has just left, so nothing it was still doing — a tool call
/// finishing after the switch — runs on unseen. Notifications already in flight when it closes are
/// still dropped by the update filter, which judges each one against the conversation the session
/// is in. It runs beside the loop that reads commands: the session is already in the new
/// conversation, so a close the agent is slow to answer, or refuses, must not hold up the switch's
/// reply or the next message.
fn spawn_close(conn: ConnectionTo<Agent>, session: Arc<Session>, left: SessionId) {
    if !session.closes_conversations.load(Ordering::SeqCst) {
        tracing::info!(session = %session.name, acp_session = %left, "conversation left open; the agent cannot close one");
        return;
    }
    // Sent here, on the loop, so the close is on the wire ahead of any later command: a resume of
    // this same conversation can never reach the agent before the close and be closed by it.
    let sent = conn.send_request(CloseSessionRequest::new(left.clone()));
    tokio::spawn(async move {
        match sent.block_task().await {
            Ok(_) => {
                tracing::info!(session = %session.name, acp_session = %left, "conversation closed")
            }
            Err(e) => {
                tracing::warn!(session = %session.name, acp_session = %left, error = %e, "conversation close failed")
            }
        }
    });
}

/// Ask the running agent to change one option and store whatever it answers with. The agent
/// replies with its whole list, so this never patches a value by hand: the stored options are
/// always the agent's own, right down to an option its change moved somewhere else.
async fn apply_config(
    conn: &ConnectionTo<Agent>,
    acp_session: &SessionId,
    session: &Session,
    config: String,
    value: String,
) -> Result<(), String> {
    let kind = session
        .inner
        .lock()
        .unwrap()
        .config
        .iter()
        .find(|option| option.id == config)
        .map(|option| option.kind.clone());
    let Some(kind) = kind else {
        return Err(format!(
            "session {} has no config option {config}",
            session.name
        ));
    };
    let request = SetSessionConfigOptionRequest::new(
        acp_session.clone(),
        SessionConfigId::new(config),
        config_value(&kind, value)?,
    );
    let response = conn
        .send_request(request)
        .block_task()
        .await
        .map_err(|e| e.to_string())?;
    // This task outlives the agent process it is talking to. If that process has been replaced
    // while the reply was in flight, the list it answers with belongs to a conversation nobody is
    // in any more: storing it would overwrite the running agent's own options with a dead one's.
    if session.acp_session() != Some(acp_session.to_string()) {
        return Err(format!(
            "session {} restarted before the agent answered",
            session.name
        ));
    }
    session.set_config_options(config_options(response.config_options));
    Ok(())
}

fn config_choice(choice: SessionConfigSelectOption) -> ConfigChoice {
    ConfigChoice {
        value: choice.value.to_string(),
        name: choice.name,
        description: choice.description,
    }
}

/// What to send the agent for a typed value. A boolean option takes `true` or `false` and nothing
/// else — anything else would quietly read as `false` and turn the option off. A select takes the
/// choice's id, which the agent compares against its own list, so an unlisted value is the agent's
/// to refuse rather than this wheel's to guess at.
fn config_value(kind: &ConfigKind, value: String) -> Result<SessionConfigOptionValue, String> {
    match kind {
        ConfigKind::Boolean { .. } => match value.as_str() {
            "true" => Ok(SessionConfigOptionValue::Boolean { value: true }),
            "false" => Ok(SessionConfigOptionValue::Boolean { value: false }),
            other => Err(format!("{other:?} is not true or false")),
        },
        ConfigKind::Select { .. } => Ok(SessionConfigOptionValue::ValueId {
            value: value.into(),
        }),
        ConfigKind::Unknown => Err("this wheel cannot set a control of that kind".into()),
    }
}

/// An ACP enum as the schema itself names it on the wire (`end_turn`, `in_progress`, `allow_once`).
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

/// The parts of an ACP session update a client renders; everything else is dropped.
fn map_update(session: &str, update: SessionUpdate) -> Option<SessionEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(t) => Some(SessionEvent::Text { text: t.text }),
            other => block_event(session, other),
        },
        SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
            ContentBlock::Text(t) => Some(SessionEvent::Thought { text: t.text }),
            _ => {
                tracing::info!(session, "thought block dropped: not text");
                None
            }
        },
        SessionUpdate::ToolCall(call) => {
            let (content, dropped) = tool_content(call.content);
            let id = call.tool_call_id.to_string();
            let status = wire(&call.status);
            tracing::info!(
                session,
                id,
                %status,
                items = content.len(),
                dropped,
                bytes = content_bytes(&content),
                "tool call"
            );
            Some(SessionEvent::ToolCall {
                id,
                tool_kind: wire(&call.kind),
                status,
                title: call.title,
                content,
            })
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let (content, dropped) = match update.fields.content.map(tool_content) {
                Some((content, dropped)) => (Some(content), dropped),
                None => (None, 0),
            };
            let id = update.tool_call_id.to_string();
            let status = update.fields.status.as_ref().map(wire);
            tracing::info!(
                session,
                id,
                status = %status.as_deref().unwrap_or("-"),
                items = content.as_ref().map(Vec::len),
                dropped,
                bytes = content.as_deref().map(content_bytes),
                "tool update"
            );
            Some(SessionEvent::ToolUpdate {
                id,
                title: update.fields.title,
                status,
                content,
            })
        }
        SessionUpdate::Plan(plan) => Some(SessionEvent::Plan {
            entries: plan
                .entries
                .into_iter()
                .map(|e| PlanItem {
                    status: wire(&e.status),
                    content: e.content,
                })
                .collect(),
        }),
        SessionUpdate::CurrentModeUpdate(update) => Some(SessionEvent::Mode {
            mode: update.current_mode_id.to_string(),
        }),
        _ => None,
    }
}

/// A non-text block of the agent's reply as a placeholder naming what it sent; its bytes stay out.
fn block_event(session: &str, block: ContentBlock) -> Option<SessionEvent> {
    let (block_kind, name) = match block {
        ContentBlock::Image(_) => ("image", None),
        ContentBlock::Audio(_) => ("audio", None),
        ContentBlock::ResourceLink(link) => ("resource_link", Some(link.name)),
        ContentBlock::Resource(resource) => (
            "resource",
            Some(match resource.resource {
                EmbeddedResourceResource::TextResourceContents(t) => t.uri,
                EmbeddedResourceResource::BlobResourceContents(b) => b.uri,
                _ => {
                    tracing::info!(session, "message block dropped: unknown resource kind");
                    return None;
                }
            }),
        ),
        _ => {
            tracing::info!(session, "message block dropped: unknown block kind");
            return None;
        }
    };
    tracing::info!(
        session,
        block_kind,
        name = name.as_deref().unwrap_or("-"),
        "message block"
    );
    Some(SessionEvent::Block {
        block_kind: block_kind.into(),
        name,
    })
}

/// The tool call content a client renders, and how many items were dropped: content blocks other
/// than text (images, resources) and kinds this build does not know.
fn tool_content(content: Vec<ToolCallContent>) -> (Vec<ToolContent>, usize) {
    let total = content.len();
    let kept: Vec<_> = content
        .into_iter()
        .filter_map(|c| match c {
            ToolCallContent::Content(c) => match c.content {
                ContentBlock::Text(t) => Some(ToolContent::Text { text: t.text }),
                _ => None,
            },
            ToolCallContent::Diff(d) => Some(ToolContent::Diff {
                path: d.path.display().to_string(),
                old_text: d.old_text,
                new_text: d.new_text,
            }),
            ToolCallContent::Terminal(t) => Some(ToolContent::Terminal {
                id: t.terminal_id.to_string(),
            }),
            _ => None,
        })
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
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
        SessionEvent::Block { name, .. } => name.as_ref().map_or(0, String::len),
        SessionEvent::TurnStart { .. }
        | SessionEvent::TurnEnd { .. }
        | SessionEvent::Mode { .. } => 0,
    }
}

/// Bytes of text and diff text, for the log, which never carries the text itself: it can hold
/// file contents.
fn content_bytes(content: &[ToolContent]) -> usize {
    content
        .iter()
        .map(|c| match c {
            ToolContent::Text { text } => text.len(),
            ToolContent::Diff {
                old_text, new_text, ..
            } => old_text.as_ref().map_or(0, String::len) + new_text.len(),
            ToolContent::Terminal { .. } | ToolContent::Unknown => 0,
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
                context: None,
            }),
            tx: broadcast::channel(16).0,
            cmds,
            next_turn: AtomicU64::new(1),
            global: broadcast::channel(16).0,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
            acp_session: Mutex::new(None),
            restored: AtomicBool::new(false),
            saved_id: Mutex::new(None),
            held: AtomicBool::new(false),
            replay: Mutex::new(None),
            ready_this_run: AtomicBool::new(false),
            started: watch::Sender::new(None),
            lists_conversations: AtomicBool::new(false),
            loads_conversations: AtomicBool::new(false),
            closes_conversations: AtomicBool::new(false),
            conversation: watch::Sender::new(0),
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
                Cmd::Prompt { .. }
                | Cmd::SetConfig { .. }
                | Cmd::ListConversations { .. }
                | Cmd::Switch { .. } => None,
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

    use agent_client_protocol::schema::v1::{
        SessionConfigSelect, SessionConfigSelectGroup, SessionConfigSelectOption,
    };

    fn choice(value: &'static str) -> SessionConfigSelectOption {
        SessionConfigSelectOption::new(value, value.to_uppercase())
    }

    /// An agent may group a select's choices under headers. Wheel draws one list, so the groups
    /// flatten in the order they arrived.
    #[test]
    fn a_grouped_select_flattens_into_one_list_of_choices() {
        let grouped = SessionConfigSelectOptions::Grouped(vec![
            SessionConfigSelectGroup::new("fast", "Fast", vec![choice("haiku")]),
            SessionConfigSelectGroup::new("slow", "Slow", vec![choice("opus"), choice("sonnet")]),
        ]);
        let option = SessionConfigOption::new(
            SessionConfigId::new("model"),
            "Model",
            SessionConfigKind::Select(SessionConfigSelect::new("sonnet", grouped)),
        );

        let mapped = config_options(vec![option]);

        let [mapped] = &mapped[..] else {
            panic!("expected one option, got {mapped:?}");
        };
        let ConfigKind::Select { current, choices } = &mapped.kind else {
            panic!("expected a select, got {:?}", mapped.kind);
        };
        assert_eq!(current, "sonnet");
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, ["haiku", "opus", "sonnet"]);
    }

    /// A boolean takes only `true` or `false`; an unknown control cannot be set at all. Anything
    /// else would read as `false` and quietly turn an option off.
    #[test]
    fn a_boolean_takes_only_true_or_false_and_unknown_takes_nothing() {
        let boolean = ConfigKind::Boolean { current: false };
        assert!(matches!(
            config_value(&boolean, "true".into()),
            Ok(SessionConfigOptionValue::Boolean { value: true })
        ));
        assert!(matches!(
            config_value(&boolean, "false".into()),
            Ok(SessionConfigOptionValue::Boolean { value: false })
        ));
        assert_eq!(
            config_value(&boolean, "yes".into()),
            Err("\"yes\" is not true or false".into())
        );
        assert!(config_value(&ConfigKind::Unknown, "anything".into()).is_err());
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
