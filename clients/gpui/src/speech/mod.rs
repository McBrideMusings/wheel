//! Speech on this Mac, for every window: the composer's dictation and voice mode's audio, in Rust
//! (ADR-0056). One app-wide entity holds their state (`wheel_gpui_core::dictation` and `::voice`,
//! where the rules and their tests live) and runs what the state asks for: the microphone and the
//! system voice through AVFoundation (`audio`), Parakeet and Silero VAD through sherpa-onnx on a
//! worker thread (`worker`), and the models' download (`download`). Windows send it commands
//! through `send` and observe the entity; only the window that pressed takes dictated text, and
//! only the window holding the voice session acts on what the person said.
mod audio;
mod download;
mod worker;

use crate::dictation::{self, Dictation};
use crate::speech_models::{self, SILERO_VAD};
use crate::speech_settings::{self, Manage};
use crate::voice::{self, Owner, Voice};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, AppContext as _, Entity, Global};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// What a window asks of speech.
#[derive(Debug)]
pub enum Cmd {
    /// The microphone in `agent`'s composer, pressed in `window`.
    DictationToggle { agent: String, window: String },
    DictationDownload,
    DictationCancel,
    /// Load Parakeet ahead of the first press, when it is on the Mac.
    DictationPrepare,
    VoiceStart(Owner),
    VoiceStop(Owner),
    VoiceDownload,
    VoiceMute(bool),
    VoiceInterrupt,
    /// The window sent the turn the person said.
    VoiceSent,
    /// The agent's finished reply, as Markdown.
    VoiceSpeak(String),
    /// Settings › Speech: download (or redownload), remove, cancel, and pick the speech-to-text
    /// model, each naming a model by its folder.
    ModelDownload(&'static str),
    ModelRemove(&'static str),
    ModelCancel,
    ModelChoose(&'static str),
    /// Settings › Speech: speak a sample reply in voice mode's voice.
    VoicePreview,
}

/// What a download is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Dictation,
    Voice,
    /// One model, from Settings › Speech.
    Settings,
}

/// What the audio thread, the worker and the download tell the entity.
pub enum Event {
    Permission { granted: bool, purpose: Purpose, run: u64 },
    Progress { purpose: Purpose, fraction: f64 },
    Downloaded { purpose: Purpose, result: Result<(), String> },
    Dictated { take: u64, result: Result<String, String>, ms: u128, seconds: f32 },
    /// The models for voice `session` loaded, or did not.
    VoiceReady { session: u64, result: Result<u128, String> },
    InputLevel(f32),
    OutputLevel(f32),
    HeardStart { session: u64 },
    TurnEnded { session: u64 },
    Said { session: u64, result: Result<String, String>, ms: u128, seconds: f32 },
    /// A buffer of speech `speech` the synthesizer wrote, to play on the main thread.
    SpeechBuffer { speech: u64, buffer: audio::SendBuffer },
    BufferPlayed { speech: u64 },
}

pub enum Msg {
    Cmd(Cmd),
    Event(Event),
}

static SENDER: OnceLock<UnboundedSender<Msg>> = OnceLock::new();

/// Ask speech for `cmd`. Before `init` (no window yet) it is dropped.
pub fn send(cmd: Cmd) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.unbounded_send(Msg::Cmd(cmd));
    }
}

fn log(line: String) {
    eprintln!("wheel-gpui: speech: {line}");
}

/// The app's speech entity, which windows observe.
pub struct SpeechGlobal(pub Entity<Speech>);
impl Global for SpeechGlobal {}

/// Make the entity and the task that feeds it, once.
pub fn init(cx: &mut App) {
    if cx.has_global::<SpeechGlobal>() {
        return;
    }
    let (tx, mut rx) = unbounded::<Msg>();
    let _ = SENDER.set(tx.clone());
    let entity = cx.new(|_| Speech::new(tx));
    cx.set_global(SpeechGlobal(entity.clone()));
    let weak = entity.downgrade();
    cx.spawn(async move |cx| {
        while let Some(msg) = rx.next().await {
            let open = weak.update(cx, |speech, cx| {
                if speech.handle(msg) {
                    cx.notify();
                }
            });
            if open.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// The entity, once `init` made it.
pub fn entity(cx: &App) -> Option<Entity<Speech>> {
    cx.try_global::<SpeechGlobal>().map(|g| g.0.clone())
}

pub struct Speech {
    pub dictation: Dictation,
    pub voice: Voice,
    pub manage: Manage,
    tx: UnboundedSender<Msg>,
    worker: worker::Worker,
    mic: Option<audio::Mic>,
    voice_audio: Option<audio::VoiceAudio>,
    downloading: Option<(Purpose, Arc<AtomicBool>)>,
    preview: Option<audio::Preview>,
    epoch: Instant,
}

impl Speech {
    fn new(tx: UnboundedSender<Msg>) -> Self {
        let worker = worker::Worker::start(tx.clone());
        let manage = Manage { chosen: speech_models::chosen(&speech_models::root()), ..Default::default() };
        Speech { dictation: Dictation::default(), voice: Voice::default(), manage, tx, worker, mic: None, voice_audio: None, downloading: None, preview: None, epoch: Instant::now() }
    }

    /// Take one message; true when what windows show changed.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Cmd(cmd) => self.command(cmd),
            Msg::Event(event) => self.event(event),
        }
    }

    fn command(&mut self, cmd: Cmd) -> bool {
        let root = speech_models::root();
        let stt = self.manage.chosen;
        match cmd {
            Cmd::DictationToggle { agent, window } => {
                let effects = self.dictation.toggle(&agent, &window, stt.on_disk(&root));
                self.run_dictation(effects);
            }
            Cmd::DictationDownload => {
                let effects = self.dictation.download();
                self.run_dictation(effects);
            }
            Cmd::DictationCancel => {
                let effects = self.dictation.cancel();
                self.run_dictation(effects);
            }
            Cmd::DictationPrepare => {
                if stt.on_disk(&root) {
                    self.worker.load(root, stt);
                }
                return false;
            }
            Cmd::VoiceStart(owner) => {
                log(format!("voice: open for {} in window {}", owner.agent, owner.window));
                let effects = self.voice.start(owner, stt.on_disk(&root) && SILERO_VAD.on_disk(&root));
                self.run_voice(effects);
            }
            Cmd::VoiceStop(owner) => {
                let effects = self.voice.stop(&owner);
                if effects.is_empty() {
                    log(format!("voice: a stop from window {} for {} left voice with {:?}", owner.window, owner.agent, self.voice.owner));
                }
                self.run_voice(effects);
            }
            Cmd::VoiceDownload => {
                let effects = self.voice.download();
                self.run_voice(effects);
            }
            Cmd::VoiceMute(on) => {
                let effects = self.voice.set_muted(on);
                self.run_voice(effects);
            }
            Cmd::VoiceInterrupt => {
                let effects = self.voice.interrupt();
                if !effects.is_empty() {
                    log("voice: interrupted".into());
                }
                self.run_voice(effects);
            }
            Cmd::VoiceSent => {
                let effects = self.voice.sent();
                self.run_voice(effects);
            }
            Cmd::VoiceSpeak(reply) => {
                let effects = self.voice.speak(&reply);
                self.run_voice(effects);
            }
            Cmd::ModelDownload(folder) | Cmd::ModelRemove(folder) | Cmd::ModelChoose(folder) if speech_models::named(folder).is_none() => {
                log(format!("settings: no model in folder {folder}"));
                return false;
            }
            Cmd::ModelDownload(folder) => {
                let effects = self.manage.download(speech_models::named(folder).expect("checked above"), &self.dictation, &self.voice);
                self.run_manage(effects);
            }
            Cmd::ModelRemove(folder) => {
                let effects = self.manage.remove(speech_models::named(folder).expect("checked above"), &self.dictation, &self.voice);
                self.run_manage(effects);
            }
            Cmd::VoicePreview => {
                if self.voice.phase != voice::Phase::Off {
                    log("voice preview refused: voice mode is on".into());
                    return false;
                }
                log("voice preview: speaking the sample reply".into());
                self.preview.get_or_insert_with(audio::Preview::new).speak(speech_settings::SAMPLE_REPLY);
                return false;
            }
            Cmd::ModelCancel => {
                let effects = self.manage.cancel();
                self.run_manage(effects);
            }
            Cmd::ModelChoose(folder) => {
                let model = speech_models::named(folder).expect("checked above");
                let effects = self.manage.choose(model, model.on_disk(&root), &self.dictation, &self.voice);
                self.run_manage(effects);
            }
        }
        if let Some(problem) = &self.manage.problem {
            log(format!("settings: {problem}"));
        }
        true
    }

    fn event(&mut self, event: Event) -> bool {
        match event {
            Event::Permission { granted, purpose: Purpose::Dictation, run } => {
                if !granted {
                    let effects = self.dictation.failed(run, audio::DENIED);
                    self.run_dictation(effects);
                } else if self.dictation.phase == dictation::Phase::Recording {
                    self.open_mic(run);
                }
            }
            Event::Permission { granted, purpose: Purpose::Voice, run } => {
                if granted {
                    self.open_voice_audio(run);
                } else {
                    let effects = self.voice.start_failed(run, audio::DENIED);
                    self.run_voice(effects);
                }
            }
            // Settings downloads ask for no microphone.
            Event::Permission { purpose: Purpose::Settings, .. } => return false,
            Event::Progress { purpose, fraction } => match purpose {
                Purpose::Dictation => self.dictation.progress(fraction),
                Purpose::Voice => self.voice.progress(fraction),
                Purpose::Settings => self.manage.progress(fraction),
            },
            Event::Downloaded { purpose, result } => {
                if self.downloading.as_ref().is_some_and(|(p, _)| *p == purpose) {
                    self.downloading = None;
                }
                log(format!("download for {purpose:?}: {}", result.as_ref().map_or_else(|e| e.clone(), |()| "done".into())));
                match purpose {
                    Purpose::Dictation => {
                        let effects = self.dictation.downloaded(result);
                        self.run_dictation(effects);
                    }
                    Purpose::Voice => {
                        let effects = self.voice.downloaded(result);
                        self.run_voice(effects);
                    }
                    Purpose::Settings => {
                        let effects = self.manage.downloaded(result, &self.dictation, &self.voice);
                        self.run_manage(effects);
                    }
                }
            }
            Event::Dictated { take, result, ms, seconds } => {
                match &result {
                    Ok(text) => log(format!("dictation: {} character(s) from {seconds:.1} s of audio in {ms} ms", text.trim().chars().count())),
                    Err(why) => log(format!("dictation: could not transcribe: {why}")),
                }
                self.dictation.transcribed(take, result);
            }
            Event::VoiceReady { session, result } => match result {
                Ok(ms) => {
                    log(format!("voice: models ready for session {session} in {ms} ms"));
                    self.ask_voice_audio(session);
                }
                Err(why) => {
                    let effects = self.voice.start_failed(session, &why);
                    self.run_voice(effects);
                }
            },
            Event::InputLevel(rms) => return self.voice.input_level(rms, self.epoch.elapsed().as_millis() as u64),
            Event::OutputLevel(rms) => return self.voice.output_level(rms, self.epoch.elapsed().as_millis() as u64),
            Event::HeardStart { session } => {
                let effects = self.voice.heard_start(session);
                self.run_voice(effects);
            }
            Event::TurnEnded { session } => self.voice.turn_ended(session),
            Event::Said { session, result, ms, seconds } => {
                match &result {
                    Ok(text) => log(format!("voice: heard {} character(s) from {seconds:.1} s of audio, transcribed in {ms} ms", text.trim().chars().count())),
                    Err(why) => log(format!("voice: could not transcribe: {why}")),
                }
                let effects = self.voice.transcribed(session, result);
                self.run_voice(effects);
            }
            Event::SpeechBuffer { speech, buffer } => {
                let done = self.voice_audio.as_mut().and_then(|a| a.play(speech, buffer, &self.tx));
                if let Some(speech) = done {
                    self.voice.spoke(speech);
                } else {
                    return false;
                }
            }
            Event::BufferPlayed { speech } => {
                match self.voice_audio.as_mut().and_then(|a| a.played(speech)) {
                    Some(speech) => self.voice.spoke(speech),
                    None => return false,
                }
            }
        }
        true
    }

    fn run_dictation(&mut self, effects: Vec<dictation::Effect>) {
        let root = speech_models::root();
        for effect in effects {
            match effect {
                dictation::Effect::Record { take } => self.ask_mic(take),
                dictation::Effect::Finish { take } => {
                    let samples = self.mic.take().map(audio::Mic::stop).unwrap_or_default();
                    log(format!("dictation: {} samples ({:.1} s) to transcribe", samples.len(), samples.len() as f32 / 16_000.));
                    self.worker.transcribe(take, samples, root.clone(), self.manage.chosen);
                }
                dictation::Effect::Discard => {
                    if let Some(mic) = self.mic.take() {
                        mic.stop();
                    }
                }
                dictation::Effect::Download => self.download(Purpose::Dictation, vec![self.manage.chosen]),
                dictation::Effect::CancelDownload => self.cancel_download(Purpose::Dictation),
                dictation::Effect::Load => self.worker.load(root.clone(), self.manage.chosen),
            }
        }
    }

    fn run_voice(&mut self, effects: Vec<voice::Effect>) {
        for effect in effects {
            match effect {
                voice::Effect::StartAudio { session } => self.worker.voice_begin(session, speech_models::root(), self.manage.chosen),
                voice::Effect::StopAudio => {
                    if let Some(audio) = self.voice_audio.take() {
                        audio.stop();
                        log("voice: microphone closed".into());
                    }
                    self.worker.voice_end();
                }
                voice::Effect::Download => {
                    let missing = [self.manage.chosen, SILERO_VAD].into_iter().filter(|m| !m.on_disk(&speech_models::root())).collect();
                    self.download(Purpose::Voice, missing);
                }
                voice::Effect::CancelDownload => self.cancel_download(Purpose::Voice),
                voice::Effect::Speak { text, speech } => {
                    log(format!("voice: speaking {} character(s)", text.chars().count()));
                    if let Some(audio) = self.voice_audio.as_mut() {
                        audio.speak(&text, speech, &self.tx);
                    }
                }
                voice::Effect::Silence => {
                    if let Some(audio) = self.voice_audio.as_mut() {
                        audio.silence();
                    }
                }
                voice::Effect::Mute(on) => {
                    if let Some(audio) = self.voice_audio.as_ref() {
                        audio.set_muted(on);
                    }
                    self.worker.voice_reset();
                }
            }
        }
    }

    fn run_manage(&mut self, effects: Vec<speech_settings::Effect>) {
        let root = speech_models::root();
        for effect in effects {
            match effect {
                speech_settings::Effect::Download(model) => self.download(Purpose::Settings, vec![model]),
                speech_settings::Effect::CancelDownload => self.cancel_download(Purpose::Settings),
                speech_settings::Effect::Remove(model) => {
                    if model == self.manage.chosen {
                        self.worker.unload();
                    }
                    let result = speech_models::remove(&root, &model);
                    log(format!("settings: removed {} from {}: {}", model.name, model.dir(&root).display(), result.as_ref().map_or_else(|e| e.clone(), |()| "done".into())));
                    self.manage.problem = result.err();
                }
                speech_settings::Effect::Save(model) => {
                    let result = speech_models::choose(&root, &model);
                    log(format!("settings: speech to text is {} ({})", model.name, result.as_ref().map_or_else(|e| e.clone(), |()| "saved".into())));
                    self.manage.problem = result.err();
                    // The model chosen before is let go; the next press loads this one.
                    self.worker.unload();
                }
            }
        }
    }

    /// Record `take`: from the stand-in file a check names, else from the microphone once the
    /// person allows it.
    fn ask_mic(&mut self, take: u64) {
        if stand_in("WHEEL_DICTATION_FILE").is_some() {
            return self.open_mic(take);
        }
        match audio::permission() {
            audio::Permission::Granted => self.open_mic(take),
            audio::Permission::Denied => {
                let effects = self.dictation.failed(take, audio::DENIED);
                self.run_dictation(effects);
            }
            audio::Permission::Undetermined => audio::request_permission(self.tx.clone(), Purpose::Dictation, take),
        }
    }

    fn open_mic(&mut self, take: u64) {
        let opened = match stand_in("WHEEL_DICTATION_FILE") {
            Some(file) => audio::Mic::from_file(&file),
            None => audio::Mic::open(),
        };
        match opened {
            Ok(mic) => {
                log(format!("dictation: recording take {take}"));
                self.mic = Some(mic);
            }
            Err(why) => {
                let effects = self.dictation.failed(take, &why);
                self.run_dictation(effects);
            }
        }
    }

    /// The models for `session` are loaded: open the audio, asking for the microphone first.
    fn ask_voice_audio(&mut self, session: u64) {
        if stand_in("WHEEL_VOICE_FILE").is_some() {
            return self.open_voice_audio(session);
        }
        match audio::permission() {
            audio::Permission::Granted => self.open_voice_audio(session),
            audio::Permission::Denied => {
                let effects = self.voice.start_failed(session, audio::DENIED);
                self.run_voice(effects);
            }
            audio::Permission::Undetermined => audio::request_permission(self.tx.clone(), Purpose::Voice, session),
        }
    }

    fn open_voice_audio(&mut self, session: u64) {
        if self.voice.phase != voice::Phase::Starting {
            return;
        }
        let silent = std::env::var("WHEEL_VOICE_SILENT").is_ok_and(|v| v == "1");
        match audio::VoiceAudio::start(stand_in("WHEEL_VOICE_FILE"), silent, session, self.tx.clone(), self.worker.jobs()) {
            Ok(audio) => {
                self.voice_audio = Some(audio);
                self.voice.started(session);
                log(format!("voice: listening, session {session}"));
            }
            Err(why) => {
                let effects = self.voice.start_failed(session, &why);
                self.run_voice(effects);
            }
        }
    }

    fn download(&mut self, purpose: Purpose, models: Vec<speech_models::Model>) {
        if let Some((running, _)) = &self.downloading {
            log(format!("download for {purpose:?} refused: the one for {running:?} is running"));
            let _ = self.tx.unbounded_send(Msg::Event(Event::Downloaded { purpose, result: Err("another download is running".into()) }));
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.downloading = Some((purpose, cancel.clone()));
        let root = speech_models::root();
        log(format!("downloading {} into {}", models.iter().map(|m| m.described()).collect::<Vec<_>>().join(" and "), root.display()));
        download::spawn(models, root, purpose, self.tx.clone(), cancel);
    }

    fn cancel_download(&mut self, purpose: Purpose) {
        if let Some((p, cancel)) = &self.downloading
            && *p == purpose
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// A WAV file a check names to stand in for the microphone.
fn stand_in(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc::UnboundedReceiver;
    use std::time::Duration;

    /// The next event, waiting up to `secs`.
    fn next(rx: &mut UnboundedReceiver<Msg>, secs: u64) -> Option<Event> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(Msg::Event(e)) = rx.try_recv() {
                return Some(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// The Mac audio path against the real models: a spoken clip transcribed for dictation, and the
    /// same clip fed through a voice engine as a turn the VAD hears and Parakeet transcribes. The
    /// reply's speech answers on the main queue, which a test thread does not run: the hidden
    /// check's `voice_mode` step speaks one. Needs `WHEEL_TEST_SPEECH_MODELS` (the models) and `WHEEL_TEST_CLIP` (a
    /// 16 kHz `say` clip); `cargo test --lib -p wheel-gpui speech -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_models_hear_a_clip_as_dictation_and_as_a_voice_turn() {
        let root = PathBuf::from(std::env::var("WHEEL_TEST_SPEECH_MODELS").expect("WHEEL_TEST_SPEECH_MODELS"));
        let clip = PathBuf::from(std::env::var("WHEEL_TEST_CLIP").expect("WHEEL_TEST_CLIP"));
        let (tx, mut rx) = unbounded::<Msg>();
        let worker = worker::Worker::start(tx.clone());
        let samples = audio::Mic::from_file(&clip).unwrap().stop();
        worker.transcribe(1, samples, root.clone(), speech_models::PARAKEET);
        match next(&mut rx, 120) {
            Some(Event::Dictated { take: 1, result: Ok(text), ms, seconds }) => {
                eprintln!("dictated {text:?} from {seconds:.1} s in {ms} ms");
                assert!(!text.is_empty());
            }
            _ => panic!("no transcription"),
        }
        worker.voice_begin(7, root.clone(), speech_models::PARAKEET);
        assert!(matches!(next(&mut rx, 60), Some(Event::VoiceReady { session: 7, result: Ok(_) })));
        let audio = audio::VoiceAudio::start(Some(clip), true, 7, tx.clone(), worker.jobs()).unwrap();
        let (mut started, mut ended, mut said) = (false, false, None);
        let end = Instant::now() + Duration::from_secs(60);
        while said.is_none() && Instant::now() < end {
            match next(&mut rx, 1) {
                Some(Event::HeardStart { session: 7 }) => started = true,
                Some(Event::TurnEnded { session: 7 }) => ended = true,
                Some(Event::Said { session: 7, result, ms, seconds }) => {
                    eprintln!("voice heard {result:?} from {seconds:.1} s in {ms} ms");
                    said = result.ok();
                }
                _ => {}
            }
        }
        assert!(started && ended && said.is_some_and(|s| !s.is_empty()), "the VAD heard no turn");
        audio.stop();
    }
}
