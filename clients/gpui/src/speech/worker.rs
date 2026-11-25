//! The thread that runs the models: the chosen Parakeet TDT 0.6b (v3 or v2) through sherpa-onnx's transducer for
//! speech to text, and Silero VAD for where a voice turn ends. One thread holds both, so a model is
//! loaded once and never runs twice at once; the main thread and the audio threads hand it work
//! over a channel and hear back through the speech entity's.
use super::{Event, Msg};
use crate::speech_models::{Model, SILERO_VAD};
use futures::channel::mpsc::UnboundedSender;
use sherpa_rs::silero_vad::{SileroVad, SileroVadConfig};
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

const RATE: u32 = 16_000;
/// The threads one transcription uses: on this Mac two beat one and four (`docs/voice-mode/index.md` §5.9).
const THREADS: i32 = 2;
/// ONNX Runtime's CPU provider: with this int8 model its Core ML provider loaded in 8.3–9.3 s against 2.8–4.3 s and
/// was no faster per turn (§5.9).
const PROVIDER: &str = "cpu";
/// The samples Silero VAD judges at a time.
const VAD_WINDOW: usize = 512;
/// Audio kept from before the VAD said speech started: it says so a few windows into the first
/// word, which the turn would otherwise lose.
const PREROLL: usize = RATE as usize * 4 / 10;
/// The most audio kept while nobody speaks.
const KEPT: usize = RATE as usize * 2;

pub enum Job {
    Load { root: PathBuf, model: Model },
    Transcribe { take: u64, samples: Vec<f32>, root: PathBuf, model: Model },
    VoiceBegin { session: u64, root: PathBuf, model: Model },
    /// Let go of a loaded speech-to-text model, whose folder is going.
    Unload,
    VoiceAudio { session: u64, samples: Vec<f32> },
    /// Drop a turn half heard (mute).
    VoiceReset,
    VoiceEnd,
}

pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    pub fn start(events: UnboundedSender<Msg>) -> Self {
        let (jobs, rx) = channel();
        std::thread::Builder::new()
            .name("wheel-speech".into())
            .spawn(move || run(rx, events))
            .expect("the speech thread starts");
        Worker { jobs }
    }
    pub fn jobs(&self) -> Sender<Job> {
        self.jobs.clone()
    }
    pub fn load(&self, root: PathBuf, model: Model) {
        let _ = self.jobs.send(Job::Load { root, model });
    }
    pub fn transcribe(&self, take: u64, samples: Vec<f32>, root: PathBuf, model: Model) {
        let _ = self.jobs.send(Job::Transcribe { take, samples, root, model });
    }
    pub fn voice_begin(&self, session: u64, root: PathBuf, model: Model) {
        let _ = self.jobs.send(Job::VoiceBegin { session, root, model });
    }
    pub fn unload(&self) {
        let _ = self.jobs.send(Job::Unload);
    }
    pub fn voice_reset(&self) {
        let _ = self.jobs.send(Job::VoiceReset);
    }
    pub fn voice_end(&self) {
        let _ = self.jobs.send(Job::VoiceEnd);
    }
}

#[derive(Default)]
struct State {
    recognizer: Option<(PathBuf, Model, TransducerRecognizer)>,
    /// The voice session the VAD listens for, the VAD, samples short of a window, and whether the
    /// VAD heard speech in the last window.
    session: Option<u64>,
    vad: Option<(PathBuf, SileroVad)>,
    partial: Vec<f32>,
    speaking: bool,
    /// What the VAD was fed since its last turn, and the VAD's sample index of its first sample.
    kept: Vec<f32>,
    kept_from: usize,
}

fn run(jobs: Receiver<Job>, events: UnboundedSender<Msg>) {
    let mut state = State::default();
    let send = |event: Event| {
        let _ = events.unbounded_send(Msg::Event(event));
    };
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Load { root, model } => {
                if let Err(why) = state.recognizer(&root, &model) {
                    eprintln!("wheel-gpui: speech: {why}");
                }
            }
            Job::Transcribe { take, samples, root, model } => {
                let seconds = samples.len() as f32 / RATE as f32;
                let started = Instant::now();
                let result = state.recognizer(&root, &model).map(|r| transcribe(r, &samples));
                send(Event::Dictated { take, result, ms: started.elapsed().as_millis(), seconds });
            }
            Job::VoiceBegin { session, root, model } => {
                let started = Instant::now();
                let result = state.recognizer(&root, &model).map(|_| ()).and_then(|()| vad(&root)).map(|vad| {
                    state.vad = Some((root, vad));
                    state.session = Some(session);
                    state.listen_afresh();
                    started.elapsed().as_millis()
                });
                send(Event::VoiceReady { session, result });
            }
            Job::VoiceAudio { session, samples } => {
                if state.session == Some(session) {
                    state.hear(session, samples, &send);
                }
            }
            Job::VoiceReset => {
                // A fresh VAD, so its sample count starts again with what is kept.
                if let Some((root, _)) = state.vad.take() {
                    state.vad = vad(&root).ok().map(|v| (root, v));
                }
                state.listen_afresh();
            }
            Job::Unload => state.recognizer = None,
            Job::VoiceEnd => {
                state.session = None;
                state.vad = None;
                state.listen_afresh();
            }
        }
    }
}

impl State {
    fn listen_afresh(&mut self) {
        self.partial.clear();
        self.speaking = false;
        self.kept.clear();
        self.kept_from = 0;
    }

    /// `model`, loaded from `root` (again when the folder or the model changed).
    fn recognizer(&mut self, root: &Path, model: &Model) -> Result<&mut TransducerRecognizer, String> {
        if self.recognizer.as_ref().is_none_or(|(from, m, _)| from != root || m != model) {
            self.recognizer = None;
            if !model.on_disk(root) {
                return Err(format!("{} is not in {}", model.name, model.dir(root).display()));
            }
            let started = Instant::now();
            let path = |file: &str| model.path(root, file).to_string_lossy().into_owned();
            let recognizer = TransducerRecognizer::new(TransducerConfig {
                encoder: path("encoder.int8.onnx"),
                decoder: path("decoder.int8.onnx"),
                joiner: path("joiner.int8.onnx"),
                tokens: path("tokens.txt"),
                model_type: "nemo_transducer".into(),
                num_threads: THREADS,
                sample_rate: RATE as i32,
                feature_dim: 80,
                decoding_method: "greedy_search".into(),
                provider: Some(PROVIDER.into()),
                ..Default::default()
            })
            .map_err(|e| format!("{} did not load: {e}", model.name))?;
            eprintln!("wheel-gpui: speech: {} loaded in {} ms ({THREADS} threads)", model.name, started.elapsed().as_millis());
            self.recognizer = Some((root.to_owned(), *model, recognizer));
        }
        Ok(&mut self.recognizer.as_mut().expect("loaded above").2)
    }

    /// Feed the VAD; a start of speech and each finished turn go back, the turn transcribed.
    fn hear(&mut self, session: u64, samples: Vec<f32>, send: &impl Fn(Event)) {
        self.partial.extend(samples);
        let whole = self.partial.len() / VAD_WINDOW * VAD_WINDOW;
        let windows: Vec<f32> = self.partial.drain(..whole).collect();
        for window in windows.chunks(VAD_WINDOW) {
            let Some((_, vad)) = self.vad.as_mut() else { return };
            vad.accept_waveform(window.to_vec());
            self.kept.extend_from_slice(window);
            let speaking = vad.is_speech();
            if speaking && !self.speaking {
                send(Event::HeardStart { session });
            }
            self.speaking = speaking;
            while !vad.is_empty() {
                let segment = vad.front();
                vad.pop();
                send(Event::TurnEnded { session });
                let turn = take_turn(&mut self.kept, &mut self.kept_from, segment.start.max(0) as usize, segment.samples.len());
                let seconds = turn.len() as f32 / RATE as f32;
                let started = Instant::now();
                let result = match self.recognizer.as_mut() {
                    Some((_, _, r)) => Ok(transcribe(r, &turn)),
                    None => Err("the speech model is not loaded".into()),
                };
                send(Event::Said { session, result, ms: started.elapsed().as_millis(), seconds });
            }
            if !self.speaking && self.kept.len() > KEPT {
                let drop = self.kept.len() - PREROLL;
                self.kept.drain(..drop);
                self.kept_from += drop;
            }
        }
    }
}

/// The turn the VAD found at `start` for `len` samples, from what was kept (`kept[0]` being sample
/// `kept_from`), with up to `PREROLL` before it; what came before its end is dropped.
fn take_turn(kept: &mut Vec<f32>, kept_from: &mut usize, start: usize, len: usize) -> Vec<f32> {
    let from = start.saturating_sub(PREROLL).max(*kept_from) - *kept_from;
    let to = (start + len).saturating_sub(*kept_from).min(kept.len());
    let turn = kept.get(from..to).map(<[f32]>::to_vec).unwrap_or_default();
    kept.drain(..to);
    *kept_from += to;
    turn
}

/// Parakeet refuses less than a second of audio; silence after the words changes nothing.
fn transcribe(recognizer: &mut TransducerRecognizer, samples: &[f32]) -> String {
    let mut padded = samples.to_vec();
    if padded.len() < RATE as usize {
        padded.resize(RATE as usize, 0.);
    }
    recognizer.transcribe(RATE, &padded).trim().to_owned()
}

fn vad(root: &Path) -> Result<SileroVad, String> {
    if !SILERO_VAD.on_disk(root) {
        return Err(format!("{} is not in {}", SILERO_VAD.name, SILERO_VAD.dir(root).display()));
    }
    SileroVad::new(
        SileroVadConfig {
            model: SILERO_VAD.path(root, "silero_vad.onnx").to_string_lossy().into_owned(),
            threshold: 0.5,
            min_silence_duration: 0.6,
            min_speech_duration: 0.25,
            // The default ends a turn after half a second of speech.
            max_speech_duration: 30.,
            sample_rate: RATE,
            window_size: VAD_WINDOW as i32,
            provider: Some(PROVIDER.into()),
            ..Default::default()
        },
        60.,
    )
    .map_err(|e| format!("{} did not load: {e}", SILERO_VAD.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_keeps_the_audio_just_before_the_vad_heard_it() {
        // Sample i holds i; the first 1000 samples were dropped earlier.
        let mut kept: Vec<f32> = (1000..30_000).map(|i| i as f32).collect();
        let mut from = 1000;
        let turn = take_turn(&mut kept, &mut from, 10_000, 5_000);
        assert_eq!(turn.first(), Some(&((10_000 - PREROLL) as f32)));
        assert_eq!(turn.last(), Some(&14_999.));
        assert_eq!((from, kept.first()), (15_000, Some(&15_000.)));
        // A turn starting inside the pre-roll of what is left starts at what is left.
        let turn = take_turn(&mut kept, &mut from, 15_100, 100);
        assert_eq!((turn.first(), turn.len()), (Some(&15_000.), 200));
    }
}
