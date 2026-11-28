//! One stream's access units decoded with VideoToolbox (ADR-0053), each decoded frame handed on as a
//! sample buffer marked to show at once, as WheelUI's `StreamDecoder.swift` does. Used from the
//! stream's read thread only.
use super::frames::Codec;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFDictionary, CFMutableDictionary, CFRetained, CFType};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo, CMTime, CMVideoFormatDescriptionCreateForImageBuffer,
    CMVideoFormatDescriptionCreateFromH264ParameterSets, CMVideoFormatDescriptionCreateFromHEVCParameterSets, CMVideoFormatDescriptionGetDimensions,
    kCMBlockBufferAssureMemoryNowFlag, kCMSampleAttachmentKey_DisplayImmediately, kCMTimeInvalid,
};
use objc2_core_video::{CVImageBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_video_toolbox::{VTDecodeFrameFlags, VTDecompressionSession, VTSessionSetProperty, kVTDecompressionPropertyKey_RealTime, kVTInvalidSessionErr};
use std::{ffi::c_void, ptr::NonNull, sync::Arc};

/// Takes each decoded frame, on VideoToolbox's thread.
pub type OnFrame = Arc<dyn Fn(&CMSampleBuffer) + Send + Sync>;

pub struct Decoder {
    on_frame: OnFrame,
    format: Option<CFRetained<CMFormatDescription>>,
    codec: Codec,
    session: Option<CFRetained<VTDecompressionSession>>,
    pub last_error: Option<String>,
}

fn cf<T: ?Sized>(object: &T) -> &CFType {
    // SAFETY: every Foundation object here is toll-free bridged to its CF type.
    unsafe { &*(object as *const T).cast::<CFType>() }
}

impl Decoder {
    pub fn new(on_frame: OnFrame) -> Self {
        Self { on_frame, format: None, codec: Codec::H264, session: None, last_error: None }
    }

    /// The video's size in pixels, once parameter sets came.
    pub fn dimensions(&self) -> Option<(i32, i32)> {
        self.format.as_ref().map(|f| {
            // SAFETY: a video format description.
            let d = unsafe { CMVideoFormatDescriptionGetDimensions(f) };
            (d.width, d.height)
        })
    }

    /// New parameter sets: the next frames decode with them. False, with `last_error` set, when they make no format.
    pub fn parameter_sets(&mut self, codec: Codec, units: &[Vec<u8>]) -> bool {
        let pointers: Vec<NonNull<u8>> = units.iter().map(|u| NonNull::new(u.as_ptr().cast_mut()).unwrap_or(NonNull::dangling())).collect();
        let sizes: Vec<usize> = units.iter().map(Vec::len).collect();
        let mut made: *const CMFormatDescription = std::ptr::null();
        let out = NonNull::from(&mut made);
        let (p, s) = (NonNull::new(pointers.as_ptr().cast_mut()).unwrap_or(NonNull::dangling()), NonNull::new(sizes.as_ptr().cast_mut()).unwrap_or(NonNull::dangling()));
        // SAFETY: `pointers` and `sizes` hold `units.len()` entries that live past the call.
        let status = unsafe {
            match codec {
                Codec::H264 => CMVideoFormatDescriptionCreateFromH264ParameterSets(None, units.len(), p, s, 4, out),
                Codec::Hevc => CMVideoFormatDescriptionCreateFromHEVCParameterSets(None, units.len(), p, s, 4, None, out),
            }
        };
        let Some(made) = NonNull::new(made.cast_mut()).filter(|_| status == 0) else {
            self.last_error = Some(format!("the {} parameter sets make no format ({status})", if codec == Codec::H264 { "H.264" } else { "HEVC" }));
            return false;
        };
        // SAFETY: a Create function's result, owned by the caller.
        let made = unsafe { CFRetained::from_raw(made) };
        // The same sets again (a backend repeats them before each key frame) keep the decoder.
        if self.session.is_some() && self.format.as_ref().is_some_and(|f| unsafe { CMFormatDescription::equal(Some(f), Some(&made)) }) {
            return true;
        }
        self.codec = codec;
        self.format = Some(made);
        if let Some(session) = self.session.take() {
            // SAFETY: a session this decoder made.
            unsafe { session.invalidate() };
        }
        true
    }

    /// Decode one access unit shown at `time` microseconds. Parameter set units inside it are
    /// skipped: the stream's own parameter set frames carry them.
    pub fn decode(&mut self, time: u64, units: &[Vec<u8>]) {
        let Some(format) = self.format.clone() else {
            self.last_error = Some("a video frame came before any parameter sets".into());
            return;
        };
        if self.session.is_none() && !self.open(&format) {
            return;
        }
        let mut avcc = vec![];
        for unit in units.iter().filter(|u| !self.is_parameter_set(u)) {
            avcc.extend_from_slice(&(unit.len() as u32).to_be_bytes());
            avcc.extend_from_slice(unit);
        }
        let Some(sample) = (!avcc.is_empty()).then(|| sample(&avcc, &format, time)).flatten() else { return };
        let on_frame = self.on_frame.clone();
        let handler = block2::RcBlock::new(move |status: i32, _flags, image: *mut CVImageBuffer, pts: CMTime, _duration: CMTime| {
            if status != 0 {
                return;
            }
            // SAFETY: VideoToolbox hands the handler a live image for the length of the call, or null.
            if let Some(shown) = unsafe { image.as_ref() }.and_then(|image| shown(image, pts)) {
                on_frame(&shown);
            }
        });
        let session = self.session.as_ref().unwrap();
        // SAFETY: a sample of this session's format; VideoToolbox copies the handler.
        let status = unsafe {
            session.decode_frame_with_output_handler(&sample, VTDecodeFrameFlags::Frame_1xRealTimePlayback, std::ptr::null_mut(), block2::RcBlock::as_ptr(&handler))
        };
        if status == kVTInvalidSessionErr {
            unsafe { session.invalidate() };
            self.session = None;
        }
        if status != 0 {
            self.last_error = Some(format!("VideoToolbox refused a frame ({status})"));
        }
    }

    fn is_parameter_set(&self, unit: &[u8]) -> bool {
        let Some(&first) = unit.first() else { return true };
        match self.codec {
            Codec::H264 => matches!(first & 0x1F, 7..=9),
            Codec::Hevc => matches!((first >> 1) & 0x3F, 32..=35),
        }
    }

    fn open(&mut self, format: &CMFormatDescription) -> bool {
        let pixel = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
        let surface = NSDictionary::<NSString, AnyObject>::new();
        // SAFETY: CFStrings are toll-free bridged to NSString.
        let key = |k: &objc2_core_foundation::CFString| unsafe { &*(k as *const objc2_core_foundation::CFString).cast::<NSString>() };
        let attributes: Retained<NSDictionary<NSString, AnyObject>> =
            unsafe { NSDictionary::from_slices(&[key(kCVPixelBufferPixelFormatTypeKey), key(kCVPixelBufferIOSurfacePropertiesKey)], &[pixel.as_ref() as &AnyObject, surface.as_ref()]) };
        let attributes: &CFDictionary = unsafe { &*(Retained::as_ptr(&attributes) as *const CFDictionary) };
        let mut made: *mut VTDecompressionSession = std::ptr::null_mut();
        // SAFETY: a null callback record, since every frame decodes with an output handler.
        let status = unsafe { VTDecompressionSession::create(None, format, None, Some(attributes), std::ptr::null(), NonNull::from(&mut made)) };
        let Some(made) = NonNull::new(made).filter(|_| status == 0) else {
            self.last_error = Some(format!("VideoToolbox opened no decoder ({status})"));
            return false;
        };
        // SAFETY: a Create function's result, owned by the caller.
        let session = unsafe { CFRetained::from_raw(made) };
        let yes = NSNumber::new_bool(true);
        // SAFETY: a decompression session is a VTSession; the value is a CFBoolean.
        unsafe { VTSessionSetProperty(cf(&*session), kVTDecompressionPropertyKey_RealTime, Some(cf(&*yes))) };
        self.session = Some(session);
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            // SAFETY: a session this decoder made.
            unsafe { session.invalidate() };
        }
    }
}

/// A compressed sample of `avcc` (length-prefixed units) at `time` microseconds.
fn sample(avcc: &[u8], format: &CMFormatDescription, time: u64) -> Option<CFRetained<CMSampleBuffer>> {
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    // SAFETY: a block buffer that allocates its own memory now, filled from `avcc` below.
    let status = unsafe {
        CMBlockBuffer::create_with_memory_block(None, std::ptr::null_mut(), avcc.len(), None, std::ptr::null(), 0, avcc.len(), kCMBlockBufferAssureMemoryNowFlag, NonNull::from(&mut block))
    };
    let block = unsafe { CFRetained::from_raw(NonNull::new(block).filter(|_| status == 0)?) };
    let copied = unsafe { CMBlockBuffer::replace_data_bytes(NonNull::new(avcc.as_ptr().cast_mut().cast::<c_void>())?, &block, 0, avcc.len()) };
    if copied != 0 {
        return None;
    }
    let timing = CMSampleTimingInfo { duration: unsafe { kCMTimeInvalid }, presentationTimeStamp: unsafe { CMTime::new(time as i64, 1_000_000) }, decodeTimeStamp: unsafe { kCMTimeInvalid } };
    let size = avcc.len();
    let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
    // SAFETY: one sample, one timing entry and one size entry, all living past the call.
    let status = unsafe { CMSampleBuffer::create_ready(None, Some(&block), Some(format), 1, 1, &timing, 1, &size, NonNull::from(&mut sample)) };
    Some(unsafe { CFRetained::from_raw(NonNull::new(sample).filter(|_| status == 0)?) })
}

/// A decoded image as a sample buffer marked to show at once, since a live stream has no clock to wait for.
fn shown(image: &CVImageBuffer, pts: CMTime) -> Option<CFRetained<CMSampleBuffer>> {
    let mut format: *const CMFormatDescription = std::ptr::null();
    // SAFETY: a live image buffer; the format is the caller's.
    let status = unsafe { CMVideoFormatDescriptionCreateForImageBuffer(None, image, NonNull::from(&mut format)) };
    let format = unsafe { CFRetained::from_raw(NonNull::new(format.cast_mut()).filter(|_| status == 0)?) };
    let mut timing = CMSampleTimingInfo { duration: unsafe { kCMTimeInvalid }, presentationTimeStamp: pts, decodeTimeStamp: unsafe { kCMTimeInvalid } };
    let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
    let status = unsafe { CMSampleBuffer::create_ready_with_image_buffer(None, image, &format, NonNull::from(&mut timing), NonNull::from(&mut sample)) };
    let sample = unsafe { CFRetained::from_raw(NonNull::new(sample).filter(|_| status == 0)?) };
    if let Some(attachments) = unsafe { sample.sample_attachments_array(true) }.filter(|a| a.count() > 0) {
        // SAFETY: a sample's attachments are mutable dictionaries, the first this sample's own.
        unsafe {
            let first = attachments.value_at_index(0).cast::<CFMutableDictionary>();
            let yes = NSNumber::new_bool(true);
            CFMutableDictionary::set_value(first.as_ref(), (kCMSampleAttachmentKey_DisplayImmediately as *const objc2_core_foundation::CFString).cast(), Retained::as_ptr(&yes).cast());
        }
    }
    Some(sample)
}

#[cfg(test)]
mod tests {
    use super::super::frames::nal_units;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn decodes(path: &str, codec: Codec, params: &[u8], frame_end: impl Fn(u8) -> bool, kind: impl Fn(u8) -> u8, frames: usize) {
        let bytes = std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let units = nal_units(&bytes);
        let sets: Vec<Vec<u8>> = params.iter().filter_map(|t| units.iter().find(|u| kind(u[0]) == *t).cloned()).collect();
        let mut access = vec![];
        let mut pending = vec![];
        for unit in units.into_iter().filter(|u| !params.contains(&kind(u[0]))) {
            let end = frame_end(kind(unit[0]));
            pending.push(unit);
            if end {
                access.push(std::mem::take(&mut pending));
            }
        }
        assert_eq!(access.len(), frames);
        let decoded = Arc::new(AtomicUsize::new(0));
        let count = decoded.clone();
        let mut decoder = Decoder::new(Arc::new(move |s: &CMSampleBuffer| {
            if unsafe { s.image_buffer() }.is_some() {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }));
        assert!(decoder.parameter_sets(codec, &sets), "{:?}", decoder.last_error);
        assert_eq!(decoder.dimensions(), Some((320, 180)));
        for (i, units) in access.iter().enumerate() {
            decoder.decode(i as u64 * 33_333, units);
        }
        for _ in 0..100 {
            if decoded.load(Ordering::SeqCst) >= frames {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(decoder.last_error, None);
        assert_eq!(decoded.load(Ordering::SeqCst), frames);
    }

    #[test]
    fn video_toolbox_decodes_every_frame_of_the_gallery_clip() {
        decodes("plugins/gallery/demo.h264", Codec::H264, &[7, 8], |k| matches!(k, 1 | 5), |b| b & 0x1F, 60);
    }

    #[test]
    fn video_toolbox_decodes_an_hevc_clip() {
        decodes("tools/fixtures/streams/demo.hevc", Codec::Hevc, &[32, 33, 34], |k| k < 32, |b| (b >> 1) & 0x3F, 30);
    }
}
