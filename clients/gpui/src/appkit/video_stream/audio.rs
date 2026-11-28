//! A stream's audio, as WheelUI's `StreamAudio`: interleaved s16 PCM converted to float into a ring
//! the engine's render thread pulls from through an `AVAudioSourceNode`. Nothing here calls a
//! player's `play()`, which raises an Objective-C exception when the engine has not run an I/O
//! cycle yet. The ring holds at most `BACKLOG_SECONDS`; past it the oldest samples go, so sound
//! never lags the picture. Runs on the stream's read thread.
use super::frames::Ring;
use objc2::{AnyThread as _, rc::Retained, runtime::Bool};
use objc2_avf_audio::{AVAudioCommonFormat, AVAudioEngine, AVAudioFormat, AVAudioSourceNode};
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use std::{
    ptr::NonNull,
    sync::{Arc, Mutex},
};

const BACKLOG_SECONDS: f64 = 0.25;

pub struct Audio {
    engine: Retained<AVAudioEngine>,
    source: Option<Retained<AVAudioSourceNode>>,
    rate: f64,
    channels: usize,
    running: bool,
    ring: Arc<Mutex<Ring>>,
    /// A hidden check (`WHEEL_MUTE=1`) plays through the engine at volume 0: the audio path runs, nobody hears it.
    muted: bool,
    gain: f32,
}

impl Audio {
    pub fn new() -> Self {
        Self {
            // SAFETY: a fresh engine, used from this thread only.
            engine: unsafe { AVAudioEngine::new() },
            source: None,
            rate: 0.,
            channels: 0,
            running: false,
            ring: Arc::default(),
            muted: std::env::var("WHEEL_MUTE").as_deref() == Ok("1"),
            gain: 1.,
        }
    }

    /// The mixer's output gain, 0 to 1.
    pub fn set_gain(&mut self, gain: f32) {
        if gain != self.gain {
            self.gain = gain;
            unsafe { self.engine.mainMixerNode().setOutputVolume(if self.muted { 0. } else { gain }) };
        }
    }

    /// Queues one chunk; false when no audio device took the stream.
    pub fn play(&mut self, rate: f64, channels: usize, pcm: &[u8]) -> bool {
        if rate != self.rate || channels != self.channels {
            self.start(rate, channels);
        }
        if !self.running {
            return false;
        }
        let samples: Vec<f32> = pcm.chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.).collect();
        let whole = samples.len() / channels * channels;
        self.ring.lock().unwrap().push(&samples[..whole], (rate * BACKLOG_SECONDS) as usize * channels);
        true
    }

    fn start(&mut self, rate: f64, channels: usize) {
        self.stop();
        self.rate = rate;
        self.channels = channels;
        // SAFETY: a float, non-interleaved format of `channels` at `rate`.
        let Some(format) = (unsafe { AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(AVAudioFormat::alloc(), AVAudioCommonFormat::PCMFormatFloat32, rate, channels as u32, false) }) else { return };
        let ring = self.ring.clone();
        let block = block2::RcBlock::new(move |_silence: NonNull<Bool>, _time: NonNull<AudioTimeStamp>, frames: u32, data: NonNull<AudioBufferList>| -> i32 {
            // SAFETY: the render thread's own buffer list, one buffer per channel of `frames` floats, for the length of the call.
            unsafe {
                let list = data.as_ptr();
                let count = (*list).mNumberBuffers as usize;
                let buffers = std::slice::from_raw_parts_mut((*list).mBuffers.as_mut_ptr(), count);
                let mut outs: Vec<&mut [f32]> = buffers.iter_mut().filter(|b| !b.mData.is_null()).map(|b| std::slice::from_raw_parts_mut(b.mData.cast::<f32>(), frames as usize)).collect();
                // The render thread never waits: a chunk being pushed leaves silence for one cycle.
                if let Ok(mut ring) = ring.try_lock() {
                    ring.pull(&mut outs, channels);
                } else {
                    outs.iter_mut().for_each(|o| o.fill(0.));
                }
            }
            0
        });
        // SAFETY: the engine copies the block; the format outlives the call.
        let node = unsafe { AVAudioSourceNode::initWithFormat_renderBlock(AVAudioSourceNode::alloc(), &format, block2::RcBlock::as_ptr(&block)) };
        unsafe {
            self.engine.attachNode(&node);
            self.engine.connect_to_format(&node, &self.engine.mainMixerNode(), Some(&format));
            self.engine.mainMixerNode().setOutputVolume(if self.muted { 0. } else { self.gain });
        }
        self.source = Some(node);
        self.running = unsafe { self.engine.startAndReturnError() }.is_ok();
    }

    pub fn stop(&mut self) {
        unsafe { self.engine.stop() };
        if let Some(source) = self.source.take() {
            unsafe { self.engine.detachNode(&source) };
        }
        self.running = false;
        self.ring.lock().unwrap().clear();
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop();
    }
}
