//! The voice agent (ADR-0059): while voice mode talks to an agent (the voiced agent), a second
//! process of the voiced agent's harness, on that harness's voice model, with no tools, says aloud
//! what the voiced agent's conversation shows. It hears the person's message the moment it is sent
//! and the voiced agent's final reply when its turn ends, each as one turn of its own, and what it
//! says streams to the connection holding the `voice` lease as `voice_said`, never entering the
//! voiced agent's transcript. A new message from the person interrupts whatever it is still saying.
//! Dropping the [`VoiceAgent`] ends its task, which drops the process's handle and kills it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use wheel_proto::{AgentEvent, ConfigKind, ServerMsg, StampedEvent, VoiceAgentInfo};

use crate::agent::Agent;
use crate::driver::{self, Conversation, DriverEvent, DriverHandle, Launch};
use crate::resume::Harness;

/// The voice agent's brief, which replaces its harness's own instructions.
pub(crate) const VOICE_BRIEF: &str = include_str!("voice-agent.md");

/// A running voice agent, speaking for one voiced agent to one connection.
pub(crate) struct VoiceAgent {
    pub harness: Harness,
    pub model: &'static str,
    pub pid: u32,
    task: JoinHandle<()>,
}

impl Drop for VoiceAgent {
    fn drop(&mut self) {
        self.task.abort();
        tracing::info!(pid = self.pid, "voice agent stopped");
    }
}

impl VoiceAgent {
    /// Start a voice agent for `voiced`, streaming what it says to `out`. Refused for a harness
    /// with no voice model.
    pub(crate) fn start(voiced: &Arc<Agent>, out: mpsc::Sender<ServerMsg>) -> Result<VoiceAgent, String> {
        let harness = voiced.harness();
        let model = driver::voice_model(harness).ok_or_else(|| format!("{} has no voice mode", harness.name()))?;
        // A folder of its own, so no project's files or instructions reach it.
        let cwd = std::env::temp_dir().join("wheel-voice-agent");
        std::fs::create_dir_all(&cwd).map_err(|e| format!("could not make {}: {e}", cwd.display()))?;
        let handle = driver::launch(Launch {
            agent: harness,
            command: voiced.command().to_owned(),
            cwd,
            config_dir: voiced.home().map(Into::into),
            brief: Some(VOICE_BRIEF.to_owned()),
            // It is no pane, so it gets none of the `WHEEL_` variables that name one.
            env: voiced.env().iter().filter(|(k, _)| !k.starts_with("WHEEL_")).cloned().collect(),
            conversation: Conversation::New(crate::claude::new_conversation_id()),
            model: Some(model.to_owned()),
            permissions: Default::default(),
            voice: Default::default(),
            voice_agent: true,
        })?;
        let pid = handle._group.0;
        tracing::info!(agent = %voiced.name, pid, model, "voice agent started");
        let task = tokio::spawn(run(voiced.name.clone(), model, voiced.subscribe(), handle, out));
        Ok(VoiceAgent { harness, model, pid, task })
    }

    /// What `status` says of it; none once its process has ended.
    pub(crate) fn info(&self) -> Option<VoiceAgentInfo> {
        (!self.task.is_finished())
            .then(|| VoiceAgentInfo { harness: self.harness.name().to_owned(), model: self.model.to_owned(), pid: self.pid })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// What the person said to the voiced agent.
    Person,
    /// The voiced agent's final reply.
    Reply,
}

/// One turn's worth for the voice agent to speak about, for the voiced agent's turn `turn`, whose
/// message was sent at `since`.
#[derive(Debug)]
struct Feed {
    turn: u64,
    kind: Kind,
    text: String,
    since: Instant,
}

impl Feed {
    fn prompt(&self) -> String {
        match self.kind {
            Kind::Person => format!("The person said:\n{}", self.text),
            Kind::Reply => format!("The agent's reply:\n{}", self.text),
        }
    }
}

/// Reads the voiced agent's events into feeds: the person's message when it is sent, and the
/// reply's last text block (what follows its last tool call) when the turn ends.
#[derive(Default)]
struct Listener {
    reply: String,
    sent: std::collections::HashMap<u64, Instant>,
}

impl Listener {
    fn hear(&mut self, event: &AgentEvent, now: Instant) -> Option<Feed> {
        match event {
            AgentEvent::User { turn, text } => {
                self.sent.insert(*turn, now);
                Some(Feed { turn: *turn, kind: Kind::Person, text: text.clone(), since: now })
            }
            AgentEvent::TurnStart { .. } | AgentEvent::ToolCall { .. } => {
                self.reply.clear();
                None
            }
            AgentEvent::Text { text } => {
                self.reply.push_str(text);
                None
            }
            AgentEvent::TextRevised { streamed, text } => {
                if let Some(head) = self.reply.strip_suffix(streamed.as_str()) {
                    self.reply = format!("{head}{text}");
                }
                None
            }
            AgentEvent::TurnEnd { turn, stop_reason } => {
                let reply = std::mem::take(&mut self.reply);
                let since = self.sent.remove(turn).unwrap_or(now);
                (stop_reason != "cancelled" && !reply.trim().is_empty())
                    .then(|| Feed { turn: *turn, kind: Kind::Reply, text: reply.trim().to_owned(), since })
            }
            _ => None,
        }
    }
}

/// The voice-agent turn running now.
struct Speaking {
    turn: u64,
    kind: Kind,
    since: Instant,
    fed: Instant,
    first: Option<Instant>,
    chars: usize,
    /// A newer message made it stale: it was cancelled, and nothing more of it is sent.
    dropped: bool,
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

async fn run(
    agent: String,
    model: &'static str,
    mut heard: broadcast::Receiver<StampedEvent>,
    mut voice: DriverHandle,
    out: mpsc::Sender<ServerMsg>,
) {
    let mut listener = Listener::default();
    let mut queue: VecDeque<Feed> = VecDeque::new();
    let mut speaking: Option<Speaking> = None;
    let mut ready = false;
    loop {
        tokio::select! {
            event = heard.recv() => match event {
                Ok(stamped) => {
                    let Some(feed) = listener.hear(&stamped.event, Instant::now()) else { continue };
                    if feed.kind == Kind::Person {
                        // What is still to be said about the last message is stale now.
                        queue.clear();
                        if let Some(s) = speaking.as_mut().filter(|s| !s.dropped) {
                            s.dropped = true;
                            let _ = voice.input.cancel();
                        }
                    }
                    tracing::info!(%agent, turn = feed.turn, kind = ?feed.kind, chars = feed.text.len(), "voice feed");
                    queue.push_back(feed);
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(%agent, missed, "the voice agent fell behind the voiced agent");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            event = voice.events.recv() => match event {
                Some(DriverEvent::Ready { options, .. }) => {
                    ready = true;
                    let offered = options.iter().find(|o| o.id == "model").and_then(|o| match &o.kind {
                        ConfigKind::Select { choices, .. } => Some(choices.iter().any(|c| c.value == model)),
                        _ => None,
                    });
                    if offered == Some(false) {
                        tracing::warn!(%agent, model, "the voice agent's harness does not list its voice model");
                    }
                }
                Some(DriverEvent::Transcript(AgentEvent::Text { text })) => {
                    if let Some(s) = speaking.as_mut().filter(|s| !s.dropped) {
                        s.first.get_or_insert_with(Instant::now);
                        s.chars += text.len();
                        let said = ServerMsg::VoiceSaid { agent: agent.clone(), turn: s.turn, text, ms: ms(s.since), done: false };
                        let _ = out.send(said).await;
                    }
                }
                Some(end @ (DriverEvent::TurnEnd { .. } | DriverEvent::TurnFailed { .. } | DriverEvent::TurnDropped)) => {
                    if let Some(s) = speaking.take() {
                        let first_ms = s.first.map(|f| f.duration_since(s.fed).as_millis() as u64);
                        let failed = match end { DriverEvent::TurnFailed { message } => Some(message), _ => None };
                        tracing::info!(%agent, turn = s.turn, kind = ?s.kind, first_ms, total_ms = ms(s.fed), since_sent_ms = ms(s.since), chars = s.chars, dropped = s.dropped, failed, "voice turn");
                        if !s.dropped {
                            let _ = out.send(ServerMsg::VoiceSaid { agent: agent.clone(), turn: s.turn, text: String::new(), ms: ms(s.since), done: true }).await;
                        }
                    }
                }
                Some(DriverEvent::Exited { error }) => {
                    tracing::warn!(%agent, error = ?error, "the voice agent's process ended");
                    return;
                }
                Some(_) => {}
                None => return,
            },
        }
        if ready && speaking.is_none() {
            if let Some(feed) = queue.pop_front() {
                if voice.input.prompt(feed.prompt()).is_err() {
                    return;
                }
                let now = Instant::now();
                speaking = Some(Speaking { turn: feed.turn, kind: feed.kind, since: feed.since, fed: now, first: None, chars: 0, dropped: false });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listener_feeds_the_message_then_the_last_text_block_of_the_reply() {
        let mut l = Listener::default();
        let now = Instant::now();
        let mut fed = |e: AgentEvent| l.hear(&e, now).map(|f| (f.turn, f.kind, f.text));
        assert_eq!(fed(AgentEvent::User { turn: 3, text: "why did it fail".into() }), Some((3, Kind::Person, "why did it fail".into())));
        assert_eq!(fed(AgentEvent::TurnStart { turn: 3 }), None);
        fed(AgentEvent::Text { text: "Let me look.".into() });
        fed(AgentEvent::ToolCall { id: "c".into(), title: "Read".into(), tool_kind: "read".into(), status: "pending".into(), content: vec![], detail: None });
        fed(AgentEvent::Text { text: "The test ".into() });
        fed(AgentEvent::Text { text: "timed out.".into() });
        fed(AgentEvent::TextRevised { streamed: "timed out.".into(), text: "timed out at 30 s.".into() });
        assert_eq!(fed(AgentEvent::TurnEnd { turn: 3, stop_reason: "end_turn".into() }), Some((3, Kind::Reply, "The test timed out at 30 s.".into())));
        fed(AgentEvent::Text { text: "half".into() });
        assert_eq!(fed(AgentEvent::TurnEnd { turn: 4, stop_reason: "cancelled".into() }), None, "a cancelled turn's text is no reply");
    }
}
