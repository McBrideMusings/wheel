//! The Mac's microphone and speaker for speech, through AVFoundation (objc2): an AVAudioEngine
//! whose input tap converts to 16 kHz mono samples, and for voice mode the same engine with voice
//! processing on, so the reply it plays through its AVAudioPlayerNode is cancelled from what the
//! microphone hears and the person can talk over it. The reply is AVSpeechSynthesizer's, written
//! to buffers and played on that engine. Engines are made and stopped on the main thread; the tap
//! blocks run on the audio thread and only send samples on.
use super::worker::Job;
use super::{Event, Msg, Purpose};
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::AllocAnyThread as _;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioBuffer, AVAudioCommonFormat, AVAudioConverter, AVAudioConverterInputStatus,
    AVAudioConverterOutputStatus, AVAudioEngine, AVAudioFormat, AVAudioMixing as _, AVAudioPCMBuffer, AVAudioPlayerNode, AVAudioTime, AVSpeechBoundary,
    AVSpeechSynthesizer, AVSpeechUtterance,
};
use objc2_foundation::NSString;
use std::cell::Cell;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DENIED: &str = "Wheel may not use the microphone. Allow it in System Settings › Privacy & Security › Microphone.";
const RATE: f64 = 16_000.;
/// What the voice stand-in feeds at a time, and how often: a quarter second, as spoken.
const FEED: usize = 4096;
const FEED_EVERY: Duration = Duration::from_millis(256);

/// A synthesizer buffer handed from its callback's thread to the main thread, which alone touches
/// it after.
pub struct SendBuffer(Retained<AVAudioBuffer>);
// SAFETY: the callback gives the buffer up once it sends it; only the main thread uses it after.
unsafe impl Send for SendBuffer {}

pub enum Permission {
    Granted,
    Denied,
    Undetermined,
}

pub fn permission() -> Permission {
    // SAFETY: a plain query of the shared instance.
    match unsafe { AVAudioApplication::sharedInstance().recordPermission() } {
        AVAudioApplicationRecordPermission::Granted => Permission::Granted,
        AVAudioApplicationRecordPermission::Denied => Permission::Denied,
        _ => Permission::Undetermined,
    }
}

/// Ask macOS for the microphone; the answer comes back as `Event::Permission` for `run`.
pub fn request_permission(events: UnboundedSender<Msg>, purpose: Purpose, run: u64) {
    let block = RcBlock::new(move |granted: Bool| {
        let _ = events.unbounded_send(Msg::Event(Event::Permission { granted: granted.as_bool(), purpose, run }));
    });
    // SAFETY: AVFoundation copies the block and calls it once, on any thread; it only sends.
    unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block) };
}

fn mono(rate: f64) -> Retained<AVAudioFormat> {
    // SAFETY: a standard deinterleaved float format, which AVAudioFormat always makes.
    unsafe { AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(AVAudioFormat::alloc(), AVAudioCommonFormat::PCMFormatFloat32, rate, 1, false) }
        .expect("a mono float format")
}

/// A stream of buffers in one format converted to another (rate, channels), keeping the
/// converter's state from buffer to buffer.
struct Converter {
    converter: Retained<AVAudioConverter>,
    to: Retained<AVAudioFormat>,
    ratio: f64,
}

impl Converter {
    fn new(from: &AVAudioFormat, to: Retained<AVAudioFormat>) -> Option<Self> {
        // SAFETY: both formats are valid PCM formats.
        let converter = unsafe { AVAudioConverter::initFromFormat_toFormat(AVAudioConverter::alloc(), from, &to) }?;
        let ratio = unsafe { to.sampleRate() / from.sampleRate() };
        Some(Converter { converter, to, ratio })
    }

    fn convert(&self, buffer: &AVAudioPCMBuffer) -> Option<Retained<AVAudioPCMBuffer>> {
        // SAFETY: the buffer outlives the call, and the input block hands it over once.
        unsafe {
            let capacity = (f64::from(buffer.frameLength()) * self.ratio).ceil() as u32 + 64;
            let out = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), &self.to, capacity)?;
            let given = Cell::new(false);
            let input = buffer as *const AVAudioPCMBuffer as *mut AVAudioBuffer;
            let block = RcBlock::new(move |_: u32, status: NonNull<AVAudioConverterInputStatus>| -> *mut AVAudioBuffer {
                if given.replace(true) {
                    *status.as_ptr() = AVAudioConverterInputStatus::NoDataNow;
                    std::ptr::null_mut()
                } else {
                    *status.as_ptr() = AVAudioConverterInputStatus::HaveData;
                    input
                }
            });
            let mut error = None;
            let status = self.converter.convertToBuffer_error_withInputFromBlock(&out, Some(&mut error), RcBlock::as_ptr(&block));
            (status != AVAudioConverterOutputStatus::Error).then_some(out)
        }
    }
}

/// The first channel's samples.
fn samples(buffer: &AVAudioPCMBuffer) -> Vec<f32> {
    // SAFETY: a float buffer's first channel holds `frameLength` samples.
    unsafe {
        let channels = buffer.floatChannelData();
        if channels.is_null() || buffer.frameLength() == 0 {
            return vec![];
        }
        std::slice::from_raw_parts((*channels).as_ptr(), buffer.frameLength() as usize).to_vec()
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// A 16 kHz WAV standing in for the microphone.
fn read_file(path: &Path) -> Result<Vec<f32>, String> {
    let (samples, rate) = sherpa_rs::read_audio_file(&path.to_string_lossy()).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    if rate != RATE as u32 {
        return Err(format!("{} is {rate} Hz, not 16 kHz", path.display()));
    }
    Ok(samples)
}

/// Dictation's recorder: the microphone, or a file standing in for it, as 16 kHz mono samples.
pub struct Mic {
    engine: Option<Retained<AVAudioEngine>>,
    heard: Arc<Mutex<Vec<f32>>>,
}

impl Mic {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        Ok(Mic { engine: None, heard: Arc::new(Mutex::new(read_file(path)?)) })
    }

    pub fn open() -> Result<Self, String> {
        let heard = Arc::new(Mutex::new(Vec::new()));
        // SAFETY: made and started on the main thread; the tap only converts and appends.
        unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let format = input.outputFormatForBus(0);
            if format.sampleRate() <= 0. {
                return Err("No microphone is available.".into());
            }
            let converter = Converter::new(&format, mono(RATE)).ok_or("The microphone's audio cannot be converted.")?;
            let into = heard.clone();
            let tap = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
                if let Some(out) = converter.convert(buffer.as_ref()) {
                    into.lock().unwrap().extend(samples(&out));
                }
            });
            input.installTapOnBus_bufferSize_format_block(0, 4096, Some(&format), RcBlock::as_ptr(&tap));
            engine.prepare();
            if let Err(error) = engine.startAndReturnError() {
                input.removeTapOnBus(0);
                return Err(format!("The microphone did not start: {}", error.localizedDescription()));
            }
            Ok(Mic { engine: Some(engine), heard })
        }
    }

    /// Close the microphone and hand back everything it heard.
    pub fn stop(self) -> Vec<f32> {
        if let Some(engine) = &self.engine {
            // SAFETY: on the main thread, where the engine was made.
            unsafe {
                engine.inputNode().removeTapOnBus(0);
                engine.stop();
            }
        }
        std::mem::take(&mut *self.heard.lock().unwrap())
    }
}

/// Voice mode's microphone (or a file fed at speaking pace, then silence) and speaker, on one
/// engine. Heard audio goes straight to the worker; levels and speech buffers to the entity.
pub struct VoiceAudio {
    engine: Retained<AVAudioEngine>,
    player: Retained<AVAudioPlayerNode>,
    synthesizer: Retained<AVSpeechSynthesizer>,
    speech_format: Retained<AVAudioFormat>,
    uses_mic: bool,
    muted: Arc<AtomicBool>,
    feeding: Option<Arc<AtomicBool>>,
    /// The speech playing now (0 for none): buffers of any other are dropped.
    current: Arc<AtomicU64>,
    queued: usize,
    finished: bool,
    converter: Option<(f64, u32, Converter)>,
}

impl VoiceAudio {
    pub fn start(file: Option<std::path::PathBuf>, silent: bool, session: u64, events: UnboundedSender<Msg>, worker: Sender<Job>) -> Result<Self, String> {
        let muted = Arc::new(AtomicBool::new(false));
        let speech_format = mono(22_050.);
        // SAFETY: made, wired and started on the main thread; taps only send.
        unsafe {
            let engine = AVAudioEngine::new();
            let player = AVAudioPlayerNode::new();
            engine.attachNode(&player);
            engine.connect_to_format(&player, &engine.mainMixerNode(), Some(&speech_format));
            if silent {
                player.setVolume(0.);
            }
            let level = events.clone();
            let out_tap = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
                let _ = level.unbounded_send(Msg::Event(Event::OutputLevel(rms(&samples(buffer.as_ref())))));
            });
            player.installTapOnBus_bufferSize_format_block(0, 2048, Some(&speech_format), RcBlock::as_ptr(&out_tap));
            let mut feeding = None;
            if let Some(file) = &file {
                let heard = read_file(file);
                let heard = match heard {
                    Ok(h) => h,
                    Err(why) => {
                        player.removeTapOnBus(0);
                        return Err(why);
                    }
                };
                let stop = Arc::new(AtomicBool::new(false));
                feed(heard, session, stop.clone(), muted.clone(), events.clone(), worker);
                feeding = Some(stop);
            } else {
                let input = engine.inputNode();
                if let Err(error) = input.setVoiceProcessingEnabled_error(true) {
                    player.removeTapOnBus(0);
                    return Err(format!("Echo cancellation did not start: {}", error.localizedDescription()));
                }
                let format = input.outputFormatForBus(0);
                if format.sampleRate() <= 0. {
                    player.removeTapOnBus(0);
                    return Err("No microphone is available.".into());
                }
                let converter = Converter::new(&format, mono(RATE)).ok_or("The microphone's audio cannot be converted.")?;
                let (hush, events) = (muted.clone(), events.clone());
                let tap = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
                    if hush.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Some(out) = converter.convert(buffer.as_ref()) {
                        let heard = samples(&out);
                        let _ = events.unbounded_send(Msg::Event(Event::InputLevel(rms(&heard))));
                        let _ = worker.send(Job::VoiceAudio { session, samples: heard });
                    }
                });
                input.installTapOnBus_bufferSize_format_block(0, 4096, Some(&format), RcBlock::as_ptr(&tap));
            }
            engine.prepare();
            if let Err(error) = engine.startAndReturnError() {
                if let Some(stop) = &feeding {
                    stop.store(true, Ordering::Relaxed);
                }
                player.removeTapOnBus(0);
                if file.is_none() {
                    engine.inputNode().removeTapOnBus(0);
                }
                return Err(format!("The audio did not start: {}", error.localizedDescription()));
            }
            Ok(VoiceAudio {
                engine,
                player,
                synthesizer: AVSpeechSynthesizer::new(),
                speech_format,
                uses_mic: file.is_none(),
                muted,
                feeding,
                current: Arc::new(AtomicU64::new(0)),
                queued: 0,
                finished: false,
                converter: None,
            })
        }
    }

    pub fn set_muted(&self, on: bool) {
        self.muted.store(on, Ordering::Relaxed);
    }

    /// Say `text` as speech `speech`: the synthesizer writes buffers that come back through the
    /// entity to `play`.
    pub fn speak(&mut self, text: &str, speech: u64, events: &UnboundedSender<Msg>) {
        self.silence();
        self.current.store(speech, Ordering::Relaxed);
        self.queued = 0;
        self.finished = false;
        let (current, events) = (self.current.clone(), events.clone());
        let callback = RcBlock::new(move |buffer: NonNull<AVAudioBuffer>| {
            if current.load(Ordering::Relaxed) != speech {
                return;
            }
            // SAFETY: the synthesizer hands a valid buffer; retaining it keeps it past the callback.
            let Some(buffer) = (unsafe { Retained::retain(buffer.as_ptr()) }) else { return };
            let _ = events.unbounded_send(Msg::Event(Event::SpeechBuffer { speech, buffer: SendBuffer(buffer) }));
        });
        // SAFETY: on the main thread; AVFoundation copies the callback.
        unsafe {
            let utterance = AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(text));
            self.synthesizer.writeUtterance_toBufferCallback(&utterance, RcBlock::as_ptr(&callback));
        }
    }

    /// Play one buffer of speech `speech`; the speech when this was its end and nothing is left.
    pub fn play(&mut self, speech: u64, buffer: SendBuffer, events: &UnboundedSender<Msg>) -> Option<u64> {
        if self.current.load(Ordering::Relaxed) != speech {
            return None;
        }
        let Ok(pcm) = buffer.0.downcast::<AVAudioPCMBuffer>() else { return None };
        // SAFETY: on the main thread, with buffers the synthesizer wrote.
        unsafe {
            if pcm.frameLength() == 0 {
                self.finished = true;
                return self.done(speech);
            }
            let format = pcm.format();
            let key = (format.sampleRate(), format.channelCount());
            let same = format.commonFormat() == AVAudioCommonFormat::PCMFormatFloat32 && key == (22_050., 1) && !format.isInterleaved();
            let converted = if same {
                Some(pcm)
            } else {
                if self.converter.as_ref().is_none_or(|(rate, channels, _)| (*rate, *channels) != key) {
                    self.converter = Converter::new(&format, self.speech_format.clone()).map(|c| (key.0, key.1, c));
                }
                self.converter.as_ref().and_then(|(_, _, c)| c.convert(&pcm))
            };
            let converted = converted?;
            self.queued += 1;
            let events = events.clone();
            let played = RcBlock::new(move || {
                let _ = events.unbounded_send(Msg::Event(Event::BufferPlayed { speech }));
            });
            self.player.scheduleBuffer_completionHandler(&converted, RcBlock::as_ptr(&played));
            if !self.player.isPlaying() {
                self.player.play();
            }
        }
        None
    }

    /// A buffer of speech `speech` played; the speech when it was the last.
    pub fn played(&mut self, speech: u64) -> Option<u64> {
        if self.current.load(Ordering::Relaxed) != speech {
            return None;
        }
        self.queued = self.queued.saturating_sub(1);
        self.done(speech)
    }

    fn done(&mut self, speech: u64) -> Option<u64> {
        (self.finished && self.queued == 0).then(|| {
            self.current.store(0, Ordering::Relaxed);
            speech
        })
    }

    /// Stop speaking at once; buffers of what was being said are dropped.
    pub fn silence(&mut self) {
        self.current.store(0, Ordering::Relaxed);
        self.queued = 0;
        self.finished = false;
        // SAFETY: on the main thread.
        unsafe {
            self.synthesizer.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
            self.player.stop();
        }
    }

    pub fn stop(mut self) {
        if let Some(stop) = &self.feeding {
            stop.store(true, Ordering::Relaxed);
        }
        self.silence();
        // SAFETY: on the main thread, where the engine was made.
        unsafe {
            self.player.removeTapOnBus(0);
            if self.uses_mic {
                self.engine.inputNode().removeTapOnBus(0);
            }
            self.engine.stop();
        }
    }
}

/// Feed a stand-in file to the worker a quarter second at a time, as if spoken, then silence
/// until voice mode ends, so the VAD hears the turn end.
fn feed(heard: Vec<f32>, session: u64, stop: Arc<AtomicBool>, muted: Arc<AtomicBool>, events: UnboundedSender<Msg>, worker: Sender<Job>) {
    std::thread::Builder::new()
        .name("wheel-voice-file".into())
        .spawn(move || {
            let silence = vec![0f32; FEED];
            let mut chunks = heard.chunks(FEED).map(<[f32]>::to_vec).collect::<Vec<_>>().into_iter();
            while !stop.load(Ordering::Relaxed) {
                let chunk = chunks.next().unwrap_or_else(|| silence.clone());
                if !muted.load(Ordering::Relaxed) {
                    let _ = events.unbounded_send(Msg::Event(Event::InputLevel(rms(&chunk))));
                    if worker.send(Job::VoiceAudio { session, samples: chunk }).is_err() {
                        break;
                    }
                }
                std::thread::sleep(FEED_EVERY);
            }
        })
        .expect("the voice stand-in thread starts");
}

/// What a voice-mode reply sounds like, from Settings › Speech: the same synthesizer and default
/// utterance voice mode writes its replies with, played straight out, with no microphone open.
pub struct Preview(Retained<AVSpeechSynthesizer>);

impl Preview {
    pub fn new() -> Self {
        // SAFETY: on the main thread.
        Preview(unsafe { AVSpeechSynthesizer::new() })
    }

    /// Speak `text`, cutting off a sample still playing.
    pub fn speak(&self, text: &str) {
        // SAFETY: on the main thread.
        unsafe {
            self.0.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
            self.0.speakUtterance(&AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(text)));
        }
    }
}
