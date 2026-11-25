//! Voice mode (wheel-e1xk.11, `docs/voice-mode/index.md` §3 and §5): the app's one voice audio
//! (WheelUI's `Voice`) and a window's session over one agent pane. The microphone stays open,
//! Silero VAD hears when the person stops, Parakeet turns the turn into text the window sends with
//! `agent_send`, and the agent's reply is spoken back. This is the state and its rules; `speech` in
//! the app runs the audio and the models they ask for.
use wheel_proto::CardRef;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Off,
    /// A model is missing: the layer names what the download fetches and asks.
    Asking,
    Downloading,
    /// The models load and the microphone starts.
    Starting,
    Listening,
    /// Speech is under way.
    Hearing,
    Transcribing,
    /// The turn went to the agent; its reply has not come.
    Thinking,
    Speaking,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Asking => "asking",
            Phase::Downloading => "downloading",
            Phase::Starting => "starting",
            Phase::Listening => "listening",
            Phase::Hearing => "hearing",
            Phase::Transcribing => "transcribing",
            Phase::Thinking => "thinking",
            Phase::Speaking => "speaking",
        }
    }
}

/// Whose voice mode it is: the window that opened it and the conversation it talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub window: String,
    pub agent: String,
}

/// One turn the person spoke. `id` grows with each.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub owner: Owner,
    pub text: String,
    pub id: u64,
}

/// What the app's audio side does for a change of state.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Load the models, then open the microphone for `session`.
    StartAudio { session: u64 },
    /// Close the microphone and stop speaking.
    StopAudio,
    Download,
    CancelDownload,
    /// Say `text`; `speech` names it, so the end of an older one changes nothing.
    Speak { text: String, speech: u64 },
    /// Stop speaking at once.
    Silence,
    /// Stop or resume hearing; a turn half heard is dropped.
    Mute(bool),
}

/// The fastest the level that moves the orb changes: fifteen times a second.
const LEVEL_EVERY_MS: u64 = 1000 / 15;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voice {
    pub phase: Phase,
    pub owner: Option<Owner>,
    pub muted: bool,
    /// The microphone's level while listening, the speech's while speaking, `0..=1`.
    pub level: f32,
    pub said: Option<Said>,
    /// What is being spoken, for the caption.
    pub caption: Option<String>,
    pub problem: Option<String>,
    /// How far the models' download has got, `0..=1`.
    pub progress: f64,
    /// A reply that came while the person was mid-turn, spoken once that turn is sent.
    waiting: Option<String>,
    session: u64,
    speech: u64,
    said_count: u64,
    level_at: Option<u64>,
}

impl Voice {
    /// Open voice mode for `owner`: ask for the models when they are missing, else load them and
    /// start listening. A start for another owner moves voice there.
    pub fn start(&mut self, owner: Owner, on_disk: bool) -> Vec<Effect> {
        let mut effects = if self.phase == Phase::Off { vec![] } else { vec![Effect::StopAudio] };
        if self.phase == Phase::Downloading {
            effects.push(Effect::CancelDownload);
        }
        self.reset();
        self.owner = Some(owner);
        self.problem = None;
        if on_disk {
            self.phase = Phase::Starting;
            self.session += 1;
            effects.push(Effect::StartAudio { session: self.session });
        } else {
            self.phase = Phase::Asking;
        }
        effects
    }

    /// Fetch what the layer named.
    pub fn download(&mut self) -> Vec<Effect> {
        if self.phase != Phase::Asking {
            return vec![];
        }
        self.phase = Phase::Downloading;
        self.progress = 0.;
        vec![Effect::Download]
    }

    pub fn progress(&mut self, fraction: f64) {
        if self.phase == Phase::Downloading {
            self.progress = fraction.clamp(0., 1.);
        }
    }

    /// The models' download ended: start, or say why it stopped.
    pub fn downloaded(&mut self, result: Result<(), String>) -> Vec<Effect> {
        if self.phase != Phase::Downloading {
            return vec![];
        }
        match result {
            Ok(()) => {
                self.phase = Phase::Starting;
                self.session += 1;
                vec![Effect::StartAudio { session: self.session }]
            }
            Err(why) => self.fail(format!("The download stopped: {why}")),
        }
    }

    /// The audio for `session` is open: listening.
    pub fn started(&mut self, session: u64) {
        if session == self.session && self.phase == Phase::Starting {
            self.phase = Phase::Listening;
        }
    }

    /// The audio for `session` could not open (no permission, no microphone, a model that failed).
    pub fn start_failed(&mut self, session: u64, why: &str) -> Vec<Effect> {
        if session != self.session || self.phase == Phase::Off {
            return vec![];
        }
        self.fail(why.to_owned())
    }

    /// End voice mode for `owner`: the microphone closes and speech stops. A stop from a window or
    /// conversation voice has since moved away from changes nothing.
    pub fn stop(&mut self, owner: &Owner) -> Vec<Effect> {
        if self.owner.as_ref() != Some(owner) {
            return vec![];
        }
        let mut effects = vec![Effect::StopAudio];
        if self.phase == Phase::Downloading {
            effects.push(Effect::CancelDownload);
        }
        self.reset();
        self.phase = Phase::Off;
        self.owner = None;
        effects
    }

    pub fn set_muted(&mut self, on: bool) -> Vec<Effect> {
        if self.phase == Phase::Off {
            return vec![];
        }
        self.muted = on;
        self.level = 0.;
        if on && self.phase == Phase::Hearing {
            self.phase = Phase::Listening;
        }
        vec![Effect::Mute(on)]
    }

    /// The window sent the turn: the orb thinks until the reply is spoken, and a reply that came
    /// while the person spoke is spoken now.
    pub fn sent(&mut self) -> Vec<Effect> {
        if matches!(self.phase, Phase::Listening | Phase::Transcribing) {
            self.phase = Phase::Thinking;
        }
        match self.waiting.take() {
            Some(reply) => self.speak(&reply),
            None => vec![],
        }
    }

    /// Say the agent's reply (Markdown). A reply that comes while the person is mid-turn waits
    /// until that turn is sent, so neither is lost.
    pub fn speak(&mut self, reply: &str) -> Vec<Effect> {
        match self.phase {
            Phase::Off | Phase::Asking | Phase::Downloading | Phase::Starting => return vec![],
            Phase::Hearing | Phase::Transcribing => {
                self.waiting = Some(reply.to_owned());
                return vec![];
            }
            _ => {}
        }
        let text = crate::spoken::from(reply);
        if text.is_empty() {
            if self.phase == Phase::Thinking {
                self.phase = Phase::Listening;
            }
            return vec![];
        }
        self.caption = Some(text.clone());
        self.phase = Phase::Speaking;
        self.speech += 1;
        vec![Effect::Speak { text, speech: self.speech }]
    }

    /// Speech `speech` played to its end.
    pub fn spoke(&mut self, speech: u64) {
        if self.phase == Phase::Speaking && speech == self.speech {
            self.phase = Phase::Listening;
            self.level = 0.;
        }
    }

    /// A press on the orb, or speech while it speaks: silence at once, and listen. The unspoken
    /// rest is dropped from audio only; the chat keeps the whole reply.
    pub fn interrupt(&mut self) -> Vec<Effect> {
        if self.phase != Phase::Speaking {
            return vec![];
        }
        self.speech += 1;
        self.phase = Phase::Listening;
        self.level = 0.;
        vec![Effect::Silence]
    }

    /// The VAD heard speech start in `session`.
    pub fn heard_start(&mut self, session: u64) -> Vec<Effect> {
        if session != self.session || self.muted {
            return vec![];
        }
        let effects = self.interrupt();
        if matches!(self.phase, Phase::Listening | Phase::Thinking) {
            self.phase = Phase::Hearing;
        }
        effects
    }

    /// The VAD heard the turn end in `session`; the model is transcribing it.
    pub fn turn_ended(&mut self, session: u64) {
        if session == self.session && self.phase == Phase::Hearing {
            self.phase = Phase::Transcribing;
        }
    }

    /// The model turned the turn into `result`: a turn to send, or back to listening.
    pub fn transcribed(&mut self, session: u64, result: Result<String, String>) -> Vec<Effect> {
        if session != self.session || self.phase != Phase::Transcribing {
            return vec![];
        }
        let Some(owner) = self.owner.clone() else { return vec![] };
        match result.map(|t| t.trim().to_owned()) {
            Ok(text) if !text.is_empty() => {
                self.said_count += 1;
                self.said = Some(Said { owner, text, id: self.said_count });
                self.phase = Phase::Thinking;
                vec![]
            }
            // Nothing to send: a reply that waited is spoken now.
            _ => {
                self.phase = Phase::Listening;
                match self.waiting.take() {
                    Some(reply) => self.speak(&reply),
                    None => vec![],
                }
            }
        }
    }

    /// The microphone's level at `now_ms`; true when the orb's level changed.
    pub fn input_level(&mut self, rms: f32, now_ms: u64) -> bool {
        matches!(self.phase, Phase::Listening | Phase::Hearing) && !self.muted && self.publish(rms, now_ms)
    }

    /// The speech's level at `now_ms`; true when the orb's level changed.
    pub fn output_level(&mut self, rms: f32, now_ms: u64) -> bool {
        self.phase == Phase::Speaking && self.publish(rms, now_ms)
    }

    fn publish(&mut self, rms: f32, now_ms: u64) -> bool {
        if self.level_at.is_some_and(|at| now_ms.saturating_sub(at) < LEVEL_EVERY_MS) {
            return false;
        }
        self.level_at = Some(now_ms);
        self.level = (rms * 8.).min(1.);
        true
    }

    /// Whether `other` differs from this only in the orb's level.
    pub fn same_but_level(&self, other: &Voice) -> bool {
        let still = |v: &Voice| Voice { level: 0., level_at: None, ..v.clone() };
        still(self) == still(other)
    }

    /// Whether a reply waits for the person's turn to be sent.
    pub fn reply_waiting(&self) -> bool {
        self.waiting.is_some()
    }

    fn reset(&mut self) {
        self.waiting = None;
        self.muted = false;
        self.level = 0.;
        self.caption = None;
        self.progress = 0.;
    }

    fn fail(&mut self, why: String) -> Vec<Effect> {
        self.reset();
        self.problem = Some(why);
        self.phase = Phase::Off;
        vec![Effect::StopAudio]
    }
}

/// One window's voice session: the agent pane it covers and what it has done with wheeld.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub agent: String,
    pub pane: String,
    /// Shrunk to the pill over the transcript (the panes button, Type instead…, a card).
    pub pill: bool,
    /// The `lease_claim` request, until wheeld answers it.
    pub claim_req: Option<u64>,
    /// This connection's id as the `voice` lease names its holder, once claimed.
    pub holder: Option<String>,
    /// The last spoken turn already sent, so the same turn is never sent twice.
    pub sent: u64,
    /// The transcript's row count when voice opened: a card after it shows above the orb.
    pub rows_at_open: usize,
}

impl Session {
    pub fn owner(&self, window: &str) -> Owner {
        Owner { window: window.to_owned(), agent: self.agent.clone() }
    }

    /// Whether the app's voice now belongs to another window or conversation: this session ends
    /// without stopping it.
    pub fn moved_away(&self, window: &str, voice: &Voice) -> bool {
        voice.owner.as_ref().is_some_and(|o| *o != self.owner(window))
    }

    /// A turn the person said to this session that it has not sent yet, once.
    pub fn take_said(&mut self, window: &str, voice: &Voice) -> Option<String> {
        let said = voice.said.as_ref().filter(|s| s.owner == self.owner(window) && s.id > self.sent)?;
        self.sent = said.id;
        Some(said.text.clone())
    }

    /// Whether its pane is among `shown`: voice ends once it is not, since nothing on screen would
    /// say the microphone is open.
    pub fn shown<'a>(&self, mut shown: impl Iterator<Item = &'a str>) -> bool {
        shown.any(|p| p == self.pane)
    }
}

/// How the orb looks: its colour, its size against the full orb (`0..=1.2`), and the word under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orb {
    pub color: u32,
    pub scale: f32,
    pub word: &'static str,
}

/// The orb for `phase`, `muted` and the `level` moving it (design §5 Orb states): Listening blue
/// with rings at the microphone's level, Thinking grey, Speaking bright with the speech's level,
/// Muted dimmed.
pub fn orb(phase: Phase, muted: bool, level: f32) -> Orb {
    let level = level.clamp(0., 1.);
    if muted {
        return Orb { color: 0x3a4a63, scale: 0.9, word: "Muted" };
    }
    match phase {
        Phase::Listening => Orb { color: 0x5b8def, scale: 0.92 + 0.2 * level, word: "Listening" },
        Phase::Hearing => Orb { color: 0x5b8def, scale: 0.95 + 0.25 * level, word: "Listening" },
        Phase::Transcribing | Phase::Thinking => Orb { color: 0x7d8794, scale: 0.9, word: "Thinking" },
        Phase::Speaking => Orb { color: 0x9cc2ff, scale: 0.95 + 0.25 * level, word: "Speaking · tap to interrupt" },
        Phase::Asking | Phase::Downloading => Orb { color: 0x3a4a63, scale: 0.85, word: "Needs the speech models" },
        Phase::Starting => Orb { color: 0x3a4a63, scale: 0.85, word: "Starting…" },
        Phase::Off => Orb { color: 0x3a4a63, scale: 0.85, word: "Off" },
    }
}

/// The name a card's chip above the orb shows.
pub fn card_title(card: &CardRef) -> String {
    match card {
        CardRef::Pane { pane } => format!("Pane {pane}"),
        CardRef::Artifact { name, .. } => name.clone(),
        CardRef::Unknown => "A card".into(),
    }
}

/// The caption under the orb: what is being said, cut to its last `max` characters so the newest
/// words stay in view.
pub fn caption(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - max + 1).collect();
    format!("…{}", tail.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(window: &str, agent: &str) -> Owner {
        Owner { window: window.into(), agent: agent.into() }
    }

    /// Voice open for w1/a1 and listening.
    fn listening() -> Voice {
        let mut v = Voice::default();
        assert_eq!(v.start(owner("w1", "a1"), true), vec![Effect::StartAudio { session: 1 }]);
        v.started(1);
        assert_eq!(v.phase, Phase::Listening);
        v
    }

    #[test]
    fn a_turn_is_heard_sent_and_its_reply_spoken() {
        let mut v = listening();
        v.heard_start(1);
        assert_eq!(v.phase, Phase::Hearing);
        v.turn_ended(1);
        assert_eq!(v.transcribed(1, Ok(" what is two plus two ".into())), vec![]);
        assert_eq!(v.said, Some(Said { owner: owner("w1", "a1"), text: "what is two plus two".into(), id: 1 }));
        assert_eq!(v.phase, Phase::Thinking);
        assert_eq!(v.sent(), vec![]);
        assert_eq!(v.speak("**Four.**"), vec![Effect::Speak { text: "Four.".into(), speech: 1 }]);
        assert_eq!((v.phase, v.caption.as_deref()), (Phase::Speaking, Some("Four.")));
        v.spoke(0);
        assert_eq!(v.phase, Phase::Speaking, "an older speech's end changes nothing");
        v.spoke(1);
        assert_eq!(v.phase, Phase::Listening);
    }

    #[test]
    fn a_reply_that_lands_mid_turn_waits_and_the_turn_is_still_sent() {
        let mut v = listening();
        v.heard_start(1);
        // The reply to an earlier turn arrives while the person speaks.
        assert_eq!(v.speak("The earlier answer."), vec![]);
        assert_eq!(v.phase, Phase::Hearing);
        assert!(v.reply_waiting());
        v.turn_ended(1);
        assert_eq!(v.speak("Still waiting."), vec![]);
        assert_eq!(v.transcribed(1, Ok("and the next question".into())), vec![]);
        assert_eq!(v.said.as_ref().map(|s| s.text.as_str()), Some("and the next question"), "the turn was not dropped");
        // Spoken once the turn is sent, the newest reply that waited.
        assert_eq!(v.sent(), vec![Effect::Speak { text: "Still waiting.".into(), speech: 1 }]);
        assert!(!v.reply_waiting());
    }

    #[test]
    fn a_waiting_reply_is_spoken_when_the_turn_had_no_words() {
        let mut v = listening();
        v.heard_start(1);
        v.speak("Here it is.");
        v.turn_ended(1);
        assert_eq!(v.transcribed(1, Ok("  ".into())), vec![Effect::Speak { text: "Here it is.".into(), speech: 1 }]);
        assert_eq!(v.said, None);
    }

    #[test]
    fn a_stop_from_another_owner_changes_nothing() {
        let mut v = listening();
        assert_eq!(v.stop(&owner("w2", "a1")), vec![], "another window's stop");
        assert_eq!(v.stop(&owner("w1", "a2")), vec![], "another conversation's stop");
        assert_eq!(v.phase, Phase::Listening);
        assert_eq!(v.stop(&owner("w1", "a1")), vec![Effect::StopAudio]);
        assert_eq!((v.phase, v.owner.clone()), (Phase::Off, None));
    }

    #[test]
    fn a_start_elsewhere_moves_voice_and_ends_the_old_session() {
        let mut v = listening();
        let mut s = Session { agent: "a1".into(), pane: "p1".into(), ..Default::default() };
        assert!(!s.moved_away("w1", &v));
        assert_eq!(v.start(owner("w2", "a3"), true), vec![Effect::StopAudio, Effect::StartAudio { session: 2 }]);
        assert!(s.moved_away("w1", &v));
        // The old session's audio events are stale.
        v.started(1);
        assert_eq!(v.phase, Phase::Starting);
        v.heard_start(1);
        assert_eq!(v.phase, Phase::Starting);
        assert_eq!(s.take_said("w1", &v), None);
    }

    #[test]
    fn a_session_sends_each_turn_once_and_ends_when_its_pane_goes() {
        let mut v = listening();
        let mut s = Session { agent: "a1".into(), pane: "p1".into(), ..Default::default() };
        v.heard_start(1);
        v.turn_ended(1);
        v.transcribed(1, Ok("hello".into()));
        assert_eq!(s.take_said("w2", &v), None, "another window's session takes nothing");
        assert_eq!(s.take_said("w1", &v).as_deref(), Some("hello"));
        assert_eq!(s.take_said("w1", &v), None);
        assert!(s.shown(["p0", "p1"].into_iter()));
        assert!(!s.shown(["p0"].into_iter()));
    }

    #[test]
    fn talking_over_speech_interrupts_it_and_mute_stops_hearing() {
        let mut v = listening();
        v.speak("A long answer.");
        assert_eq!(v.heard_start(1), vec![Effect::Silence]);
        assert_eq!(v.phase, Phase::Hearing);
        assert_eq!(v.set_muted(true), vec![Effect::Mute(true)]);
        assert_eq!(v.phase, Phase::Listening);
        assert_eq!(v.heard_start(1), vec![], "muted, nothing is heard");
        assert!(!v.input_level(0.5, 1_000));
        v.set_muted(false);
        v.speak("Again.");
        assert_eq!(v.interrupt(), vec![Effect::Silence]);
        assert_eq!(v.interrupt(), vec![]);
    }

    #[test]
    fn with_no_models_it_asks_and_starts_once_they_are_here() {
        let mut v = Voice::default();
        assert_eq!(v.start(owner("w1", "a1"), false), vec![]);
        assert_eq!(v.phase, Phase::Asking);
        assert_eq!(v.download(), vec![Effect::Download]);
        v.progress(0.5);
        assert_eq!(v.downloaded(Ok(())), vec![Effect::StartAudio { session: 1 }]);
        assert_eq!(v.start_failed(1, "Wheel may not use the microphone."), vec![Effect::StopAudio]);
        assert_eq!((v.phase, v.problem.as_deref()), (Phase::Off, Some("Wheel may not use the microphone.")));
    }

    #[test]
    fn the_level_changes_at_most_fifteen_times_a_second() {
        let mut v = listening();
        assert!(v.input_level(0.0625, 1_000));
        assert_eq!(v.level, 0.5);
        assert!(!v.input_level(0.1, 1_030));
        assert!(v.input_level(1., 1_070));
        assert_eq!(v.level, 1.);
        assert!(!v.output_level(0.1, 2_000), "not speaking");
        let before = v.clone();
        v.input_level(0.01, 3_000);
        assert!(v.same_but_level(&before) && v != before);
    }

    #[test]
    fn the_orb_follows_the_phase_and_mute_wins() {
        assert_eq!(orb(Phase::Listening, false, 0.).word, "Listening");
        assert!(orb(Phase::Listening, false, 1.).scale > orb(Phase::Listening, false, 0.).scale);
        assert_eq!(orb(Phase::Thinking, false, 1.).scale, 0.9);
        assert_eq!(orb(Phase::Speaking, false, 0.).word, "Speaking · tap to interrupt");
        assert_eq!(orb(Phase::Speaking, true, 0.5).word, "Muted");
        assert_eq!(orb(Phase::Listening, false, 7.).scale, orb(Phase::Listening, false, 1.).scale);
    }

    #[test]
    fn a_long_caption_keeps_its_end() {
        assert_eq!(caption("short", 10), "short");
        assert_eq!(caption("one two three four", 10), "…hree four");
        assert_eq!(caption("one two three four", 10).chars().count(), 10);
    }

    #[test]
    fn a_card_is_named_by_what_it_shows() {
        assert_eq!(card_title(&CardRef::Pane { pane: "p5".into() }), "Pane p5");
        assert_eq!(card_title(&CardRef::Unknown), "A card");
    }
}
