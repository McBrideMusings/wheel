//! `wheel pane record` on a browser pane: the web view's own snapshots taken back to back for a
//! number of seconds and written to an H.264 QuickTime movie, each frame stamped with the time it
//! was taken, so the movie plays at the real speed. No Screen Recording permission is involved.
use dispatch2::MainThreadBound;
use objc2::{AnyThread as _, Message as _, rc::Retained, runtime::AnyObject};
use objc2_app_kit::NSImage;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus, AVFileTypeQuickTimeMovie, AVMediaTypeVideo,
    AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGImageByteOrderInfo, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSPoint, NSString, NSURL};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
use std::{cell::RefCell, path::PathBuf, ptr::NonNull, rc::Rc, time::Instant};

/// What a recording made: the movie's bytes, its frame count and its length.
pub struct Film {
    pub mov: Vec<u8>,
    pub frames: u64,
    pub millis: u64,
}

type Done = Box<dyn FnOnce(Result<Film, String>)>;

struct Recording {
    view: Retained<WKWebView>,
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    path: PathBuf,
    width: usize,
    height: usize,
    seconds: f64,
    start: Instant,
    frames: u64,
    last_millis: i64,
    done: Option<Done>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        // A recording that stops before it finished leaves the writer mid-file: cancelling stops it before the file goes.
        if self.done.is_some() {
            unsafe { self.writer.cancelWriting() };
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn ns_key(key: &objc2_core_foundation::CFString) -> &NSString {
    // SAFETY: a CFString is toll-free bridged to NSString.
    unsafe { &*(key as *const objc2_core_foundation::CFString).cast::<NSString>() }
}

fn dictionary(pairs: &[(&NSString, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (keys, values): (Vec<&NSString>, Vec<&AnyObject>) = pairs.iter().copied().unzip();
    NSDictionary::from_slices(&keys, &values)
}

/// One snapshot of the page at its own width in points.
fn frame(view: &WKWebView, done: impl FnOnce(Result<Retained<CGImage>, String>) + 'static) {
    let configuration = unsafe { WKSnapshotConfiguration::new(super::main_thread()) };
    unsafe { configuration.setSnapshotWidth(Some(&NSNumber::new_f64(view.bounds().size.width))) };
    super::web::snapshot_image(view, Some(&configuration), move |r| {
        done(r.and_then(|image: Retained<NSImage>| {
            let mut rect = objc2_foundation::NSRect::new(NSPoint::ZERO, image.size());
            unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }.ok_or_else(|| "the snapshot has no bitmap".to_owned())
        }))
    });
}

/// Record `view` for `seconds`; `done` gets the movie or why it failed.
pub fn record(view: &WKWebView, seconds: f64, done: impl FnOnce(Result<Film, String>) + 'static) {
    let view = view.retain();
    let view2 = view.clone();
    frame(&view, move |first| match first.and_then(|image| start(view2, seconds, &image).map(|r| (r, image))) {
        Ok((recording, image)) => {
            let recording = Rc::new(RefCell::new(recording));
            recording.borrow_mut().done = Some(Box::new(done));
            next(recording, image);
        }
        Err(message) => done(Err(message)),
    });
}

fn start(view: Retained<WKWebView>, seconds: f64, first: &CGImage) -> Result<Recording, String> {
    let (width, height) = (CGImage::width(Some(first)) & !1, CGImage::height(Some(first)) & !1);
    if width == 0 || height == 0 {
        return Err("the pane has no size".into());
    }
    let path = std::env::temp_dir().join(format!("wheel-record-{}.mov", uuid::Uuid::new_v4()));
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let file_type = unsafe { AVFileTypeQuickTimeMovie }.ok_or("no QuickTime file type")?;
    let writer = unsafe { AVAssetWriter::initWithURL_fileType_error(AVAssetWriter::alloc(), &url, file_type) }.map_err(|e| e.localizedDescription().to_string())?;
    let (w, h) = (NSNumber::new_usize(width), NSNumber::new_usize(height));
    let settings = unsafe {
        dictionary(&[
            (AVVideoCodecKey.unwrap(), AVVideoCodecTypeH264.unwrap()),
            (AVVideoWidthKey.unwrap(), &w),
            (AVVideoHeightKey.unwrap(), &h),
        ])
    };
    let media = unsafe { AVMediaTypeVideo }.ok_or("no video media type")?;
    let input = unsafe { AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(media, Some(&settings)) };
    unsafe { input.setExpectsMediaDataInRealTime(true) };
    let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let attributes = unsafe {
        dictionary(&[(ns_key(kCVPixelBufferPixelFormatTypeKey), &format), (ns_key(kCVPixelBufferWidthKey), &w), (ns_key(kCVPixelBufferHeightKey), &h)])
    };
    let adaptor = unsafe { AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input, Some(&attributes)) };
    unsafe { writer.addInput(&input) };
    if !unsafe { writer.startWriting() } {
        let _ = std::fs::remove_file(&path);
        return Err(unsafe { writer.error() }.map_or("the movie file could not be started".into(), |e| e.localizedDescription().to_string()));
    }
    unsafe { writer.startSessionAtSourceTime(CMTime::new(0, 1000)) };
    Ok(Recording { view, writer, input, adaptor, path, width, height, seconds, start: Instant::now(), frames: 0, last_millis: -1, done: None })
}

/// Write `image` as the frame at this moment, then take the next, until the time is up.
fn next(recording: Rc<RefCell<Recording>>, image: Retained<CGImage>) {
    let millis = recording.borrow().start.elapsed().as_millis() as i64;
    if millis as f64 >= recording.borrow().seconds * 1000. {
        return finish(recording);
    }
    {
        let mut r = recording.borrow_mut();
        // A frame that arrives in the same millisecond as the last would not move the clock.
        if millis > r.last_millis
            && unsafe { r.input.isReadyForMoreMediaData() }
            && let Some(buffer) = pixel_buffer(&r, &image)
        {
            if !unsafe { r.adaptor.appendPixelBuffer_withPresentationTime(&buffer, CMTime::new(millis, 1000)) } {
                let message = unsafe { r.writer.error() }.map_or("a frame could not be written".into(), |e| e.localizedDescription().to_string());
                let done = r.done.take();
                unsafe { r.writer.cancelWriting() };
                drop(r);
                if let Some(done) = done {
                    done(Err(message));
                }
                return;
            }
            r.frames += 1;
            r.last_millis = millis;
        }
    }
    let view = recording.borrow().view.clone();
    frame(&view, move |r| match r {
        Ok(image) => next(recording, image),
        Err(message) => {
            let done = recording.borrow_mut().done.take();
            if let Some(done) = done {
                unsafe { recording.borrow().writer.cancelWriting() };
                done(Err(message));
            }
        }
    });
}

fn finish(recording: Rc<RefCell<Recording>>) {
    let (writer, total) = {
        let r = recording.borrow();
        let total = r.start.elapsed().as_millis() as i64;
        unsafe {
            r.input.markAsFinished();
            r.writer.endSessionAtSourceTime(CMTime::new(total.max(r.last_millis + 1), 1000));
        }
        (r.writer.clone(), total)
    };
    let cell = RefCell::new(Some(recording));
    let block = block2::RcBlock::new(move || {
        let Some(recording) = cell.borrow_mut().take() else { return };
        let mut r = recording.borrow_mut();
        let Some(done) = r.done.take() else { return };
        let outcome = if unsafe { r.writer.status() } == AVAssetWriterStatus::Completed {
            std::fs::read(&r.path).map(|mov| Film { mov, frames: r.frames, millis: total as u64 }).map_err(|e| e.to_string())
        } else {
            Err(unsafe { r.writer.error() }.map_or("the movie file could not be finished".into(), |e| e.localizedDescription().to_string()))
        };
        drop(r);
        done(outcome);
    });
    // The writer finishes on a queue of its own; the answer is sent from the main thread, where the recording lives.
    let finished = std::sync::Mutex::new(Some(MainThreadBound::new(block, super::main_thread())));
    let block = block2::RcBlock::new(move || {
        if let Some(bound) = finished.lock().unwrap().take() {
            dispatch2::DispatchQueue::main().exec_async(move || {
                let mtm = objc2::MainThreadMarker::new().expect("the main queue runs on the main thread");
                bound.get(mtm).call(());
            });
        }
    });
    unsafe { writer.finishWritingWithCompletionHandler(&block) };
}

/// `image` drawn into a fresh BGRA pixel buffer from the adaptor's pool.
fn pixel_buffer(r: &Recording, image: &CGImage) -> Option<CFRetained<CVPixelBuffer>> {
    let pool = unsafe { r.adaptor.pixelBufferPool() }?;
    let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
    unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut buffer)) };
    let buffer = unsafe { CFRetained::from_raw(NonNull::new(buffer)?) };
    unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) };
    let space = CGColorSpace::new_device_rgb();
    let context: Option<CFRetained<CGContext>> = unsafe {
        CGBitmapContextCreate(
            CVPixelBufferGetBaseAddress(&buffer),
            r.width,
            r.height,
            8,
            CVPixelBufferGetBytesPerRow(&buffer),
            space.as_deref(),
            CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
        )
    };
    if let Some(context) = &context {
        CGContext::draw_image(Some(context), CGRect::new(CGPoint::ZERO, CGSize::new(r.width as f64, r.height as f64)), Some(image));
    }
    unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) };
    context.map(|_| buffer)
}
