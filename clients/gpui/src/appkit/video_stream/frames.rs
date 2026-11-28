//! The frames on a stream socket (ADR-0053, `docs/plugin-runtime.md` "Video streams"): a type
//! byte, a 4-byte big-endian length and the payload, as WheelUI's `StreamFrames.swift` reads them.

/// The codec a stream's parameter sets name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264 = 1,
    Hevc = 2,
}

/// One frame from the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// Type 1: the codec byte, then the codec's parameter sets (SPS and PPS, with VPS first for HEVC) in Annex-B.
    ParameterSets { codec: Codec, units: Vec<Vec<u8>> },
    /// Type 2: an 8-byte big-endian time in microseconds, then one access unit in Annex-B.
    Video { time: u64, units: Vec<Vec<u8>> },
    /// Type 3: a 4-byte big-endian sample rate, a channel count byte, then interleaved s16 little-endian PCM.
    Audio { rate: u32, channels: u8, pcm: Vec<u8> },
    /// A type this client does not know, skipped.
    Unknown(u8),
}

/// What a client sends: key (16), pointer (17) and controller (18) events, each a UTF-8 JSON object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key = 16,
    Pointer = 17,
    Controller = 18,
}

/// One frame's bytes: type, big-endian length, payload.
pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// The bytes of one input frame.
pub fn input(kind: Input, event: &serde_json::Value) -> Vec<u8> {
    frame(kind as u8, event.to_string().as_bytes())
}

/// The NAL units of an Annex-B byte run, start codes (3 or 4 bytes) removed.
pub fn nal_units(b: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = vec![];
    for (k, &start) in starts.iter().enumerate() {
        let next = starts.get(k + 1).copied();
        let mut end = next.map_or(b.len(), |n| n - 3);
        // The zero before a 4-byte start code belongs to the code, not to this unit.
        while next.is_some() && end > start && b[end - 1] == 0 {
            end -= 1;
        }
        if end > start {
            units.push(b[start..end].to_vec());
        }
    }
    units
}

/// Splits what a stream socket delivers into frames, keeping a cut frame until the rest of it arrives.
#[derive(Default)]
pub struct Reader {
    buffer: Vec<u8>,
}

/// The longest payload taken; a longer length is a broken stream, not a frame to wait for.
pub const MAX_PAYLOAD: usize = 32 << 20;

impl Reader {
    /// The frames the bytes read so far complete, or why the stream is broken.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = vec![];
        let mut at = 0;
        while self.buffer.len() - at >= 5 {
            let length = u32::from_be_bytes(self.buffer[at + 1..at + 5].try_into().unwrap()) as usize;
            if length > MAX_PAYLOAD {
                return Err(format!("a frame of {length} bytes is longer than {MAX_PAYLOAD}"));
            }
            if self.buffer.len() - at - 5 < length {
                break;
            }
            frames.push(decode(self.buffer[at], &self.buffer[at + 5..at + 5 + length])?);
            at += 5 + length;
        }
        self.buffer.drain(..at);
        Ok(frames)
    }
}

fn decode(kind: u8, b: &[u8]) -> Result<Frame, String> {
    match kind {
        1 => {
            let codec = match b.first() {
                Some(1) => Codec::H264,
                Some(2) => Codec::Hevc,
                other => return Err(format!("parameter sets name no codec this client decodes ({})", other.map_or("none".into(), u8::to_string))),
            };
            Ok(Frame::ParameterSets { codec, units: nal_units(&b[1..]) })
        }
        2 => {
            if b.len() < 8 {
                return Err(format!("a video frame of {} bytes has no time", b.len()));
            }
            Ok(Frame::Video { time: u64::from_be_bytes(b[..8].try_into().unwrap()), units: nal_units(&b[8..]) })
        }
        3 => {
            if b.len() < 5 {
                return Err(format!("an audio frame of {} bytes has no format", b.len()));
            }
            let rate = u32::from_be_bytes(b[..4].try_into().unwrap());
            if rate == 0 || b[4] == 0 {
                return Err(format!("audio at {rate} Hz with {} channels", b[4]));
            }
            Ok(Frame::Audio { rate, channels: b[4], pcm: b[5..].to_vec() })
        }
        other => Ok(Frame::Unknown(other)),
    }
}

/// Where a picture of `size` draws in `bounds` (x, y, w, h): fitted inside, centred, or filling it
/// with the overflow past its edges, as `AVMakeRect` and `resizeAspectFill` place it.
pub fn video_rect(size: (f64, f64), bounds: [f64; 4], fills: bool) -> [f64; 4] {
    let scale = if fills { (bounds[2] / size.0).max(bounds[3] / size.1) } else { (bounds[2] / size.0).min(bounds[3] / size.1) };
    let (w, h) = (size.0 * scale, size.1 * scale);
    [bounds[0] + (bounds[2] - w) / 2., bounds[1] + (bounds[3] - h) / 2., w, h]
}

/// Where `point` (the view's coordinates, y up) falls in a picture of `size` drawn in `bounds`, in
/// its pixels from the top left, held to its edges.
pub fn video_point(point: (f64, f64), size: (f64, f64), bounds: [f64; 4], fills: bool) -> (f64, f64) {
    let r = video_rect(size, bounds, fills);
    let x = ((point.0 - r[0]) / r[2]).clamp(0., 1.) * size.0;
    let y = ((r[1] + r[3] - point.1) / r[3]).clamp(0., 1.) * size.1;
    (x.round(), y.round())
}

/// The names of the modifiers held, in the order the Swift view lists them.
pub fn modifier_names(shift: bool, control: bool, option: bool, command: bool, caps: bool) -> Vec<&'static str> {
    [(shift, "shift"), (control, "control"), (option, "option"), (command, "command"), (caps, "capslock")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect()
}

/// A key event: `code` is the Mac's virtual key code, `key` the characters it types without modifiers.
pub fn key_event(kind: &str, code: u16, key: &str, repeat: bool, modifiers: &[&str]) -> serde_json::Value {
    serde_json::json!({"kind": kind, "code": code, "key": key, "repeat": repeat, "modifiers": modifiers})
}

/// Interleaved samples waiting to play, shared by the read thread and the audio render thread.
#[derive(Default)]
pub struct Ring {
    samples: std::collections::VecDeque<f32>,
}

impl Ring {
    /// Adds `more`, keeping only the newest `keep` samples so sound never lags the picture.
    pub fn push(&mut self, more: &[f32], keep: usize) {
        self.samples.extend(more);
        let over = self.samples.len().saturating_sub(keep);
        self.samples.drain(..over);
    }

    /// Fills each channel's buffer with the next frames, silence where none wait.
    pub fn pull(&mut self, outs: &mut [&mut [f32]], channels: usize) {
        let frames = outs.iter().map(|o| o.len()).min().unwrap_or(0);
        let have = frames.min(self.samples.len() / channels.max(1));
        for i in 0..frames {
            for (ch, out) in outs.iter_mut().enumerate() {
                out[i] = if i < have && ch < channels { self.samples[i * channels + ch] } else { 0. };
            }
        }
        self.samples.drain(..have * channels);
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_keeps_a_cut_frame_until_the_rest_arrives() {
        let video = frame(2, &[0, 0, 0, 0, 0, 0, 0x01, 0x02, 0, 0, 0, 1, 0x65, 0xAA, 0, 0, 1, 0x41, 0xBB]);
        let audio = frame(3, &[0, 0, 0xBB, 0x80, 2, 1, 0, 2, 0]);
        let bytes = [video, audio, frame(99, &[7])].concat();
        let mut reader = Reader::default();
        let mut got = vec![];
        for chunk in bytes.chunks(3) {
            got.extend(reader.push(chunk).unwrap());
        }
        assert_eq!(got, vec![
            Frame::Video { time: 0x0102, units: vec![vec![0x65, 0xAA], vec![0x41, 0xBB]] },
            Frame::Audio { rate: 48000, channels: 2, pcm: vec![1, 0, 2, 0] },
            Frame::Unknown(99),
        ]);
    }

    #[test]
    fn a_broken_frame_is_refused() {
        assert!(Reader::default().push(&[2, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        assert!(Reader::default().push(&frame(1, &[9, 0, 0, 1, 0x67])).is_err());
        assert!(Reader::default().push(&frame(2, &[0, 1])).is_err());
        assert!(Reader::default().push(&frame(3, &[0, 0, 0, 0, 2])).is_err());
    }

    #[test]
    fn a_four_byte_start_code_leaves_no_zero_on_the_unit_before() {
        let units = nal_units(&[0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 1, 0x65]);
        assert_eq!(units, vec![vec![0x67, 0x42], vec![0x68, 0xCE], vec![0x65]]);
    }

    #[test]
    fn an_input_frame_is_its_type_length_and_json() {
        let bytes = input(Input::Key, &key_event("down", 12, "q", false, &modifier_names(true, false, false, true, false)));
        assert_eq!(bytes[0], 16);
        assert_eq!(u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize, bytes.len() - 5);
        let body: serde_json::Value = serde_json::from_slice(&bytes[5..]).unwrap();
        assert_eq!(body, serde_json::json!({"kind": "down", "code": 12, "key": "q", "repeat": false, "modifiers": ["shift", "command"]}));
    }

    #[test]
    fn a_fitted_picture_letterboxes_and_a_filling_one_crops() {
        let r = |a: [f64; 4]| a.map(|v| (v * 10.).round() / 10.);
        assert_eq!(r(video_rect((1920., 1080.), [0., 0., 400., 100.], false)), [111.1, 0., 177.8, 100.]);
        assert_eq!(r(video_rect((1920., 1080.), [0., 0., 400., 100.], true)), [0., -62.5, 400., 225.]);
    }

    #[test]
    fn a_point_lands_in_the_pictures_pixels_from_the_top_left() {
        // A 320x180 picture fitted to 640x200: drawn 355.6 wide in the middle, y up in the view.
        let bounds = [0., 0., 640., 200.];
        assert_eq!(video_point((320., 100.), (320., 180.), bounds, false), (160., 90.));
        assert_eq!(video_point((0., 200.), (320., 180.), bounds, false), (0., 0.));
        assert_eq!(video_point((640., 0.), (320., 180.), bounds, false), (320., 180.));
        // Filled, the middle is still the middle.
        assert_eq!(video_point((320., 100.), (320., 180.), bounds, true), (160., 90.));
    }

    #[test]
    fn the_audio_ring_keeps_the_newest_and_plays_silence_when_empty() {
        let mut ring = Ring::default();
        ring.push(&[1., -1., 2., -2., 3., -3.], 4);
        let (mut left, mut right) = ([9.; 3], [9.; 3]);
        ring.pull(&mut [&mut left[..], &mut right[..]], 2);
        assert_eq!(left, [2., 3., 0.]);
        assert_eq!(right, [-2., -3., 0.]);
    }
}
