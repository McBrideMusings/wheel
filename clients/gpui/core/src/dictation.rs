//! The composer's dictation (WheelUI's `Dictation`): one recorder for the app, which a window's
//! microphone press starts for one conversation and a second press stops and transcribes. The
//! text goes into the draft of the window that pressed, as typed text the person sends. This is
//! the state and its rules; `speech` in the app runs the microphone and the model it asks for.
use crate::speech_models::Model;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// The model is not on this Mac: the composer names what the download fetches and asks.
    Asking,
    /// The model downloads, `0..=1`.
    Downloading(f64),
    Recording,
    Transcribing,
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Asking => "asking",
            Phase::Downloading(_) => "downloading",
            Phase::Recording => "recording",
            Phase::Transcribing => "transcribing",
        }
    }
}

/// What one recording turned into, for the conversation and the window that recorded it. `id`
/// grows with each, so the same words twice reach the draft twice.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub owner: String,
    pub window: String,
    pub text: String,
    pub id: u64,
}

/// What the app's audio side does for a change of state.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Open the microphone (or the stand-in file) for recording `take`.
    Record { take: u64 },
    /// Close it and transcribe what `take` heard.
    Finish { take: u64 },
    /// Close it and throw away what it heard.
    Discard,
    Download,
    CancelDownload,
    /// Load the model ahead of the next press.
    Load,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dictation {
    pub phase: Phase,
    /// The conversation recording, asked or downloading for; None while idle.
    pub owner: Option<String>,
    /// The window whose press started or last stopped it: only that window takes the text.
    pub window: Option<String>,
    /// Why the last attempt failed, and whose it was, until the next one starts.
    pub problem: Option<String>,
    pub problem_owner: Option<String>,
    pub heard: Option<Heard>,
    take: u64,
    heard_count: u64,
}

impl Dictation {
    /// The microphone in conversation `owner`, pressed in `window`: start recording, or stop and
    /// transcribe when `owner` is the one recording. With no model on the Mac it asks first. A
    /// press in another conversation while one records or transcribes changes nothing.
    pub fn toggle(&mut self, owner: &str, window: &str, on_disk: bool) -> Vec<Effect> {
        match self.phase {
            Phase::Recording if self.owner.as_deref() == Some(owner) => {
                self.phase = Phase::Transcribing;
                self.window = Some(window.to_owned());
                vec![Effect::Finish { take: self.take }]
            }
            Phase::Idle | Phase::Asking if !on_disk => {
                self.owner = Some(owner.to_owned());
                self.window = Some(window.to_owned());
                self.phase = Phase::Asking;
                self.problem = None;
                vec![]
            }
            Phase::Idle | Phase::Asking => {
                self.problem = None;
                self.owner = Some(owner.to_owned());
                self.window = Some(window.to_owned());
                self.phase = Phase::Recording;
                self.take += 1;
                vec![Effect::Record { take: self.take }, Effect::Load]
            }
            _ => vec![],
        }
    }

    /// Download what the prompt named.
    pub fn download(&mut self) -> Vec<Effect> {
        if self.phase != Phase::Asking {
            return vec![];
        }
        self.phase = Phase::Downloading(0.);
        vec![Effect::Download]
    }

    pub fn progress(&mut self, fraction: f64) {
        if let Phase::Downloading(_) = self.phase {
            self.phase = Phase::Downloading(fraction.clamp(0., 1.));
        }
    }

    /// The download ended: back to idle, so the next press records; a failure says why.
    pub fn downloaded(&mut self, result: Result<(), String>) -> Vec<Effect> {
        if !matches!(self.phase, Phase::Downloading(_)) {
            return vec![];
        }
        match result {
            Ok(()) => {
                self.phase = Phase::Idle;
                self.owner = None;
                vec![Effect::Load]
            }
            Err(why) => {
                self.fail(format!("The download stopped: {why}"));
                vec![]
            }
        }
    }

    /// Cancel: closes the prompt, stops a download or throws the recording away.
    pub fn cancel(&mut self) -> Vec<Effect> {
        let effects = match self.phase {
            Phase::Asking => vec![],
            Phase::Downloading(_) => vec![Effect::CancelDownload],
            Phase::Recording => vec![Effect::Discard],
            Phase::Idle | Phase::Transcribing => return vec![],
        };
        self.phase = Phase::Idle;
        self.owner = None;
        effects
    }

    /// Recording `take` could not start (no permission, no microphone).
    pub fn failed(&mut self, take: u64, why: &str) -> Vec<Effect> {
        if take != self.take || !matches!(self.phase, Phase::Recording | Phase::Transcribing) {
            return vec![];
        }
        self.fail(why.to_owned());
        vec![Effect::Discard]
    }

    /// The model turned `take` into `result`.
    pub fn transcribed(&mut self, take: u64, result: Result<String, String>) {
        if take != self.take || self.phase != Phase::Transcribing {
            return;
        }
        let (Some(owner), Some(window)) = (self.owner.take(), self.window.clone()) else { return };
        self.phase = Phase::Idle;
        match result.map(|t| t.trim().to_owned()) {
            Ok(text) if text.is_empty() => {
                self.problem = Some("Heard nothing.".into());
                self.problem_owner = Some(owner);
            }
            Ok(text) => {
                self.heard_count += 1;
                self.heard = Some(Heard { owner, window, text, id: self.heard_count });
            }
            Err(why) => {
                self.problem = Some(format!("Could not transcribe: {why}"));
                self.problem_owner = Some(owner);
            }
        }
    }

    fn fail(&mut self, why: String) {
        self.problem = Some(why);
        self.problem_owner = self.owner.take();
        self.phase = Phase::Idle;
    }

    /// Whether `agent`'s microphone is the one recording, for its red stop.
    pub fn recording_for(&self, agent: &str) -> bool {
        self.phase == Phase::Recording && self.owner.as_deref() == Some(agent)
    }

    /// Whether `agent`'s microphone waits on the model (downloading or transcribing): dimmed.
    pub fn busy_for(&self, agent: &str) -> bool {
        self.owner.as_deref() == Some(agent) && matches!(self.phase, Phase::Downloading(_) | Phase::Transcribing)
    }

    /// Whether the prompt above `agent`'s composer is up: asking, or downloading with Cancel.
    pub fn prompt_for(&self, agent: &str) -> bool {
        self.owner.as_deref() == Some(agent) && matches!(self.phase, Phase::Asking | Phase::Downloading(_))
    }

    /// The line under `agent`'s composer while dictation works for it, or its failure there.
    pub fn note(&self, agent: &str, model: &Model) -> Option<String> {
        if self.owner.as_deref() == Some(agent) {
            return match self.phase {
                Phase::Downloading(p) => Some(format!("Downloading {}, {}%", model.name, (p * 100.).floor() as u32)),
                Phase::Recording => Some("Listening…".into()),
                Phase::Transcribing => Some("Transcribing…".into()),
                _ => None,
            };
        }
        self.problem.clone().filter(|_| self.problem_owner.as_deref() == Some(agent))
    }
}

/// The draft after dictation hands it `text`: after what is there, with a space between unless the
/// draft is empty or already ends in whitespace.
pub fn dictated(draft: &str, text: &str) -> String {
    let gap = if draft.is_empty() || draft.ends_with(char::is_whitespace) { "" } else { " " };
    format!("{draft}{gap}{text}")
}

/// The prompt above the composer the first time the microphone is pressed with no model here: what
/// the download fetches, how big, from where, and where it is kept (ADR-0013).
pub fn prompt(model: &Model, root: &Path) -> String {
    format!(
        "Dictation needs {} ({}), downloaded once from {} into {}. Your voice never leaves this Mac.",
        model.name,
        model.size(),
        model.source,
        model.dir(root).display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech_models::PARAKEET;

    #[test]
    fn with_no_model_the_press_asks_and_cancel_closes_it() {
        let mut d = Dictation::default();
        assert_eq!(d.toggle("a1", "w1", false), vec![]);
        assert_eq!((d.phase.clone(), d.owner.as_deref()), (Phase::Asking, Some("a1")));
        assert!(d.prompt_for("a1") && !d.prompt_for("a2"));
        assert_eq!(d.download(), vec![Effect::Download]);
        d.progress(0.426);
        assert_eq!(d.note("a1", &PARAKEET).as_deref(), Some("Downloading Parakeet TDT 0.6b v3, 42%"));
        assert_eq!(d.cancel(), vec![Effect::CancelDownload]);
        assert_eq!((d.phase.clone(), d.owner.clone()), (Phase::Idle, None));
        // The download's own end, after the cancel, changes nothing.
        assert_eq!(d.downloaded(Err("cancelled".into())), vec![]);
        assert_eq!(d.problem, None);
    }

    #[test]
    fn two_presses_record_then_transcribe_into_the_window_that_pressed() {
        let mut d = Dictation::default();
        assert_eq!(d.toggle("a1", "w1", true), vec![Effect::Record { take: 1 }, Effect::Load]);
        assert!(d.recording_for("a1") && !d.recording_for("a2"));
        // Another conversation's press waits its turn.
        assert_eq!(d.toggle("a2", "w2", true), vec![]);
        assert_eq!(d.note("a1", &PARAKEET).as_deref(), Some("Listening…"));
        assert_eq!(d.note("a2", &PARAKEET), None);
        assert_eq!(d.toggle("a1", "w3", true), vec![Effect::Finish { take: 1 }]);
        assert!(d.busy_for("a1"));
        // An old take's text is not this one's.
        d.transcribed(0, Ok("stale".into()));
        assert_eq!(d.phase, Phase::Transcribing);
        d.transcribed(1, Ok(" run the tests ".into()));
        assert_eq!(d.heard, Some(Heard { owner: "a1".into(), window: "w3".into(), text: "run the tests".into(), id: 1 }));
        assert_eq!(d.phase, Phase::Idle);
        d.toggle("a1", "w1", true);
        d.toggle("a1", "w1", true);
        d.transcribed(2, Ok("".into()));
        assert_eq!((d.problem.as_deref(), d.note("a1", &PARAKEET).as_deref(), d.note("a2", &PARAKEET)), (Some("Heard nothing."), Some("Heard nothing."), None));
        assert_eq!(d.heard.as_ref().map(|h| h.id), Some(1));
    }

    #[test]
    fn a_recording_that_cannot_start_says_why_to_its_own_conversation() {
        let mut d = Dictation::default();
        d.toggle("a1", "w1", true);
        assert_eq!(d.failed(1, "Wheel may not use the microphone."), vec![Effect::Discard]);
        assert_eq!((d.phase.clone(), d.problem_owner.as_deref()), (Phase::Idle, Some("a1")));
        assert_eq!(d.failed(1, "again"), vec![], "a failure after the take ended changes nothing");
    }

    #[test]
    fn dictated_text_follows_the_draft() {
        assert_eq!(dictated("", "run the tests"), "run the tests");
        assert_eq!(dictated("then", "run the tests"), "then run the tests");
        assert_eq!(dictated("then\n", "run the tests"), "then\nrun the tests");
    }

    #[test]
    fn the_prompt_names_the_model_its_size_source_and_folder() {
        let text = prompt(&PARAKEET, Path::new("/m"));
        for named in ["Parakeet TDT 0.6b v3", "670.6 MB", "huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", "/m/parakeet"] {
            assert!(text.contains(named), "{named} missing from {text}");
        }
    }
}
