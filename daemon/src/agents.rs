//! Verbs on an agent pane's session, addressed by pane id or session name. The lead's tools and
//! the `wheel agent` commands both reach a session through `resolve_agent`, so one resolver and
//! one set of rules (what a session in `gave_up` accepts, what a cancel with no turn does) serve
//! both.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::broadcast;
use wheel_proto::{PaneKind, SessionEvent, SessionState, StampedEvent, SESSION_ENDED};

use crate::perms::Perms;
use crate::server::AppState;
use crate::session::Session;
use crate::spaces::{agent_panes, find_pane};

/// The most transcript `read` returns; a longer conversation keeps its end.
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

/// An agent session and the pane that owns it; `pane` is unset for the lead, which has none.
pub(crate) struct AgentRef {
    pub pane: Option<String>,
    pub session: Arc<Session>,
}

/// What `read` returns.
#[derive(Debug, Serialize)]
pub struct AgentRead {
    pub pane: String,
    pub session: String,
    pub state: SessionState,
    pub transcript: String,
    /// How many turns the conversation has that were started; a message still queued behind a
    /// running turn is not one yet.
    pub turns: usize,
    /// The turns `transcript` holds text from, oldest first: the span asked for, or with a grep,
    /// the turns that matched.
    pub shown: Vec<u64>,
    /// With a grep, how many messages and tool lines matched before any were cut for size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<usize>,
}

/// Which part of a conversation `read` returns.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadSpan {
    /// The last `n` turns (n is at least 1).
    Last(usize),
    /// Every message and tool line containing the text, case ignored, each tagged with its turn.
    Grep(String),
    /// The whole stored transcript.
    All,
}

impl ReadSpan {
    /// The span the lead's arguments name; the default is the last turn. At most one of the three
    /// may be given.
    pub fn from_args(
        last_turns: Option<u64>,
        grep: Option<String>,
        all: Option<bool>,
    ) -> Result<ReadSpan, String> {
        let all = all.unwrap_or(false);
        let given = [last_turns.is_some(), grep.is_some(), all]
            .iter()
            .filter(|g| **g)
            .count();
        if given > 1 {
            return Err("give at most one of last_turns, grep and all".into());
        }
        if let Some(n) = last_turns {
            return match usize::try_from(n) {
                Ok(n) if n >= 1 => Ok(ReadSpan::Last(n)),
                _ => Err(format!("last_turns is {n}; use 1 or more")),
            };
        }
        if let Some(text) = grep {
            return if text.is_empty() {
                Err("grep is empty; give the text to look for".into())
            } else {
                Ok(ReadSpan::Grep(text))
            };
        }
        Ok(if all {
            ReadSpan::All
        } else {
            ReadSpan::Last(1)
        })
    }
}

/// What `message` returns: the turn the message got, and when it waited, the reply.
#[derive(Debug, Serialize)]
pub struct AgentMessaged {
    pub pane: String,
    pub session: String,
    pub turn: u64,
    /// Present only when the call waited. False when the timeout passed first; the message still
    /// runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<bool>,
    /// The agent's reply text so far, when the call waited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// What `wait` returns.
#[derive(Debug, Serialize)]
pub struct AgentWaited {
    pub pane: String,
    pub session: String,
    pub state: SessionState,
    /// False when the timeout passed with the session not ready.
    pub ready: bool,
}

/// What `cancel` returns.
#[derive(Debug, Serialize)]
pub struct AgentCancelled {
    pub pane: String,
    pub session: String,
    pub turn: u64,
}

/// The phase name a state is reported under.
fn phase(state: &SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Ready => "ready",
        SessionState::Busy { .. } => "busy",
        SessionState::Failed { .. } => "failed",
        SessionState::GaveUp { .. } => "gave_up",
    }
}

impl AppState {
    /// The session `target` names: an agent pane's id, or a session name (`lead` included).
    pub(crate) fn resolve_agent(&self, target: &str) -> Result<AgentRef, String> {
        let pane_session = {
            let spaces = self.spaces.lock().unwrap();
            match spaces
                .spaces
                .iter()
                .find_map(|s| find_pane(&s.root, target))
            {
                Some(PaneKind::Agent { session, .. } | PaneKind::Worker { session, .. }) => {
                    Some(Ok(session.clone()))
                }
                Some(other) => {
                    let kind = serde_json::to_value(other)
                        .ok()
                        .and_then(|v| v["kind"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    Some(Err(format!(
                        "{target} is not an agent pane; its kind is {kind}"
                    )))
                }
                None => None,
            }
        };
        if let Some(session) = pane_session {
            let session = session?;
            return Ok(AgentRef {
                pane: Some(target.to_string()),
                session: self.session(&session)?,
            });
        }
        let session = self.session(target)?;
        let pane = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .flat_map(|s| agent_panes(&s.root))
                .find(|(_, name, _)| name == target)
                .map(|(pane, _, _)| pane)
        };
        Ok(AgentRef { pane, session })
    }

    /// Like `resolve_agent`, for a verb that needs the pane too.
    pub(crate) fn resolve_agent_pane(
        &self,
        target: &str,
    ) -> Result<(String, Arc<Session>), String> {
        let found = self.resolve_agent(target)?;
        match found.pane {
            Some(pane) => Ok((pane, found.session)),
            None => Err(format!(
                "session {target} has no pane; give the id of an agent pane or its session name"
            )),
        }
    }

    /// The part of the conversation `span` names, as text.
    pub fn agent_read(&self, target: &str, span: ReadSpan) -> Result<AgentRead, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        let attached = session.attach();
        let items = items(&attached.events);
        let turns: Vec<u64> = items
            .iter()
            .filter(|i| !i.queued)
            .map(|i| i.turn)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let (lines, shown, matches) = match &span {
            ReadSpan::All => (
                items.iter().map(|i| i.text.clone()).collect::<Vec<_>>(),
                turns.clone(),
                None,
            ),
            ReadSpan::Last(n) => {
                let from = turns.len().saturating_sub(*n);
                let shown = turns[from..].to_vec();
                let lines = items
                    .iter()
                    .filter(|i| i.queued || shown.contains(&i.turn))
                    .map(|i| i.text.clone())
                    .collect();
                (lines, shown, None)
            }
            ReadSpan::Grep(text) => {
                let needle = text.to_lowercase();
                let hits: Vec<&Item> = items
                    .iter()
                    .filter(|i| i.text.to_lowercase().contains(&needle))
                    .collect();
                let shown: Vec<u64> = hits
                    .iter()
                    .map(|i| i.turn)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let lines = hits
                    .iter()
                    .map(|i| format!("[turn {}] {}", i.turn, i.text))
                    .collect();
                (lines, shown, Some(hits.len()))
            }
        };
        let transcript = fit(lines);
        tracing::info!(
            %pane, session = %session.name, events = attached.events.len(), ?span,
            shown = shown.len(), "agent read"
        );
        Ok(AgentRead {
            pane,
            session: session.name.clone(),
            state: attached.state,
            transcript,
            turns: turns.len(),
            shown,
            matches,
        })
    }

    /// Queue `text` for the agent. A session that gave up or failed takes nothing, and the error
    /// names its state. With `wait`, the call returns when the message's turn ends, or after that
    /// many seconds.
    pub async fn agent_message(
        &self,
        target: &str,
        text: String,
        wait: Option<Duration>,
    ) -> Result<AgentMessaged, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        let name = session.name.clone();
        let Some(wait) = wait else {
            let turn = session.send(text)?;
            return Ok(AgentMessaged {
                pane,
                session: name,
                turn,
                finished: None,
                reply: None,
            });
        };
        let done = session_turn(&session, text, Wait::Total(wait), None).await?;
        tracing::info!(%pane, session = %name, turn = done.turn, finished = done.finished, "agent message waited");
        Ok(AgentMessaged {
            pane,
            session: name,
            turn: done.turn,
            finished: Some(done.finished),
            reply: Some(done.reply),
        })
    }

    /// Wait until the session is `ready`, up to `timeout`. A session that gave up never will be,
    /// so that is an error at once.
    pub async fn agent_wait(&self, target: &str, timeout: Duration) -> Result<AgentWaited, String> {
        let found = self.resolve_agent(target)?;
        let session = found.session;
        let pane = found.pane.unwrap_or_default();
        // Subscribed before the first look at the state, so a change in between is not missed.
        let mut global = self.global.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        let state = loop {
            let state = session.state();
            match &state {
                SessionState::Ready => break (state, true),
                SessionState::GaveUp { tries, message } => {
                    return Err(format!(
                        "session {} gave up after {tries} tries and will not be ready: {message}",
                        session.name
                    ))
                }
                _ => {}
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            // Any message, or a lag that dropped some, is a reason to look at the state again.
            if tokio::time::timeout(left, global.recv()).await.is_err() {
                break (session.state(), false);
            }
        };
        tracing::info!(%pane, session = %session.name, state = phase(&state.0), ready = state.1, "agent wait");
        Ok(AgentWaited {
            pane,
            session: session.name.clone(),
            state: state.0,
            ready: state.1,
        })
    }

    /// Stop the session's current turn; an error when none is running.
    pub fn agent_cancel(&self, target: &str) -> Result<AgentCancelled, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        let turn = session.cancel()?;
        Ok(AgentCancelled {
            pane,
            session: session.name.clone(),
            turn,
        })
    }
}

/// What a message sent with [`session_turn`] got back.
pub(crate) struct TurnReply {
    pub turn: u64,
    /// False when the wait passed before the turn ended; the message still runs.
    pub finished: bool,
    /// The agent's reply text so far.
    pub reply: String,
}

/// How long [`session_turn`] waits for a turn to end.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Wait {
    /// This long in all.
    Total(Duration),
    /// This long without the session saying anything: every event of the session starts the
    /// count again, so a long turn that keeps working is not cut off. Time the session spends
    /// waiting on a person's permission answer does not count, when the caller passes the
    /// permissions to look in.
    Idle(Duration),
}

/// Queue `text` for `session` and wait for its turn to end. An error when the session takes no
/// messages or the turn fails. The one place a caller sends a message and reads the reply: the
/// lead's `message_agent`, a supervisor's planning turn and a worker's turns all come through it.
pub(crate) async fn session_turn(
    session: &Session,
    text: String,
    wait: Wait,
    perms: Option<&Perms>,
) -> Result<TurnReply, String> {
    turn_in(session, text, wait, false, perms).await
}

/// [`session_turn`] for a message wheeld wrote itself: the turn stays out of the session's
/// transcript and out of every client's chat.
pub(crate) async fn quiet_turn(
    session: &Session,
    text: String,
    wait: Wait,
) -> Result<TurnReply, String> {
    turn_in(session, text, wait, true, None).await
}

async fn turn_in(
    session: &Session,
    text: String,
    wait: Wait,
    quiet: bool,
    perms: Option<&Perms>,
) -> Result<TurnReply, String> {
    // Subscribe before sending, so no event of the new turn is missed.
    let (mut rx, turn) = if quiet {
        let rx = session.quiet_events();
        (rx, session.send_quiet(text)?)
    } else {
        let rx = session.attach().rx;
        (rx, session.send(text)?)
    };
    let mut perm_rx = perms.map(|p| p.subscribe());
    let name = session.name.clone();
    let mut reply = String::new();
    let mut started = false;
    let mut was_waiting = false;
    let (Wait::Total(limit) | Wait::Idle(limit)) = wait;
    let mut deadline = tokio::time::Instant::now() + limit;
    let finished = loop {
        let received = tokio::select! {
            received = rx.recv() => received,
            // A permission event: when this session's own wait on a person starts or ends, the
            // idle count starts again from it. Other sessions' requests change nothing here.
            Some(()) = async {
                match perm_rx.as_mut() {
                    Some(rx) => loop {
                        match rx.recv().await {
                            Ok(wheel_proto::ServerMsg::PermRequested { .. } | wheel_proto::ServerMsg::PermResolved { .. })
                            | Err(broadcast::error::RecvError::Lagged(_)) => break Some(()),
                            Ok(_) => {}
                            Err(broadcast::error::RecvError::Closed) => break None,
                        }
                    },
                    None => None,
                }
            } => {
                let waiting = perms.is_some_and(|p| p.waiting_on_person(&name));
                if waiting != was_waiting {
                    was_waiting = waiting;
                    if matches!(wait, Wait::Idle(_)) {
                        deadline = tokio::time::Instant::now() + limit;
                    }
                }
                continue;
            }
            _ = tokio::time::sleep_until(deadline) => {
                // A session waiting on a person sends no events; its silence is not a stall.
                if matches!(wait, Wait::Idle(_)) && perms.is_some_and(|p| p.waiting_on_person(&name)) {
                    deadline = tokio::time::Instant::now() + limit;
                    continue;
                }
                break Ok(false);
            }
        };
        if matches!(wait, Wait::Idle(_)) {
            deadline = tokio::time::Instant::now() + limit;
        }
        match received.map(|e| e.event) {
            Ok(SessionEvent::TurnStart { turn: t }) if t == turn => started = true,
            Ok(SessionEvent::Text { text }) if started => reply.push_str(&text),
            Ok(SessionEvent::TurnEnd { turn: t, .. }) if t == turn => break Ok(true),
            Ok(SessionEvent::Error {
                turn: Some(t),
                message,
            }) if t == turn => break Err(message),
            Ok(SessionEvent::Error {
                turn: None,
                message,
            }) if started || message == SESSION_ENDED => break Err(message),
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break Err(SESSION_ENDED.into()),
        }
    };
    match finished {
        Ok(finished) => Ok(TurnReply {
            turn,
            finished,
            reply,
        }),
        Err(message) => Err(format!("{name} failed the turn: {message}")),
    }
}

/// One message, reply, thought or tool line of a conversation, with the turn it belongs to.
struct Item {
    turn: u64,
    text: String,
    /// A message sent that its turn has not started on yet: pending, not part of any started turn.
    queued: bool,
}

/// The conversation as items: a message sent (`> `, `(queued)` until its turn starts), the
/// reply chunks joined into one, a thought, a block, a tool call, an error. A message belongs to
/// the turn it was queued as; everything else to the turn running when it arrived, or turn 0
/// before any started.
fn items(events: &[StampedEvent]) -> Vec<Item> {
    let queued = wheel_proto::queued_turns(events);
    let mut out = Vec::new();
    let mut running = 0;
    let mut reply: Option<Item> = None;
    for event in events.iter().map(|e| &e.event) {
        if let SessionEvent::Text { text } = event {
            reply
                .get_or_insert_with(|| Item {
                    turn: running,
                    text: String::new(),
                    queued: false,
                })
                .text
                .push_str(text);
            continue;
        }
        out.extend(reply.take());
        let mut queued_now = false;
        let (turn, text) = match event {
            SessionEvent::TurnStart { turn } => {
                running = *turn;
                continue;
            }
            SessionEvent::User { turn, text } => {
                queued_now = queued.contains(turn);
                let marker = if queued_now { " (queued)" } else { "" };
                (*turn, format!("> {text}{marker}"))
            }
            SessionEvent::Thought { text } => (running, format!("[thought] {text}")),
            SessionEvent::Block { block_kind, name } => (
                running,
                format!("[{block_kind}] {}", name.as_deref().unwrap_or("-")),
            ),
            SessionEvent::ToolCall { title, status, .. } => {
                (running, format!("[tool] {title} ({status})"))
            }
            SessionEvent::Error { turn, message } => {
                (turn.unwrap_or(running), format!("[error] {message}"))
            }
            _ => continue,
        };
        out.push(Item {
            turn,
            text,
            queued: queued_now,
        });
    }
    out.extend(reply);
    out
}

/// `lines` joined, one per line, within `MAX_TRANSCRIPT_BYTES`: the oldest lines go first, whole,
/// behind a marker saying how many. A last line that is alone over the budget loses its start
/// instead.
fn fit(mut lines: Vec<String>) -> String {
    let size = |lines: &[String]| lines.iter().map(|l| l.len() + 1).sum::<usize>();
    let mut left_out = 0;
    while lines.len() > 1 && size(&lines) > MAX_TRANSCRIPT_BYTES {
        lines.remove(0);
        left_out += 1;
    }
    if let [only] = lines.as_mut_slice() {
        if only.len() + 1 > MAX_TRANSCRIPT_BYTES {
            let mut cut = only.len() + 1 - MAX_TRANSCRIPT_BYTES;
            while !only.is_char_boundary(cut) {
                cut += 1;
            }
            *only = format!("[first {cut} bytes left out]\n{}", &only[cut..]);
        }
    }
    let mut out = String::new();
    if left_out > 0 {
        out.push_str(&format!("[first {left_out} messages left out]\n"));
    }
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}
