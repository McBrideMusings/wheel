//! A plugin page's `ui.VideoStream` (ADR-0053) in its embed slot, as WheelUI's `VideoStreams.swift`
//! shows it on the Mac: a thread reads the backend's frames from the unix socket wheeld named
//! (`plugin_stream`), VideoToolbox decodes the video onto an `AVSampleBufferDisplayLayer`,
//! `AVAudioEngine` plays the audio, and the keys, pointer and controller events on the view go
//! back as input frames. A note over the picture says why it shows none: connecting, refused,
//! ended. When a connection ends, the client hears of it as a `stream_ended` frame and asks wheeld
//! again; the stream's counts last across connections, for the check's state.
pub mod frames;

mod audio;
mod controllers;
mod decoder;

use super::main_thread;
use dispatch2::DispatchQueue;
use frames::{Frame, Input};
use futures::channel::mpsc::UnboundedSender;
use objc2::{AnyThread as _, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSEvent, NSEventModifierFlags, NSFont, NSResponder, NSTextAlignment, NSTextField, NSTrackingArea,
    NSTrackingAreaOptions, NSView,
};
use objc2_av_foundation::{
    AVLayerVideoGravityResizeAspect, AVLayerVideoGravityResizeAspectFill, AVQueuedSampleBufferRendering, AVQueuedSampleBufferRenderingStatus,
    AVSampleBufferDisplayLayer, AVSampleBufferVideoRenderer,
};
use objc2_core_image::CIImage;
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CATransaction;
use serde_json::{Value, json};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashMap,
    io::{Read as _, Write as _},
    os::unix::net::UnixStream,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
};
use wheel_gpui_core::native::Embedded;

/// What a stream's reader counts, read on the main thread for the check's state.
#[derive(Default)]
struct Stats {
    frames: AtomicU64,
    audio: AtomicU64,
    inputs: AtomicU64,
    bytes: AtomicU64,
}

/// The display layer's renderer, which takes sample buffers from any thread.
struct Renderer(Retained<AVSampleBufferVideoRenderer>);
// SAFETY: AVSampleBufferVideoRenderer's enqueue and flush are documented as callable from any thread.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

/// The last frame decoded, kept for the check's picture of the stream.
struct Picture(objc2_core_foundation::CFRetained<objc2_core_video::CVImageBuffer>);
// SAFETY: a CVPixelBuffer is a reference-counted CF object, read here only after decoding wrote it.
unsafe impl Send for Picture {}

/// One open connection's state, shared by its read thread, its write thread and the main thread.
struct Shared {
    stats: Arc<Stats>,
    renderer: Renderer,
    socket: OnceLock<UnixStream>,
    closed: AtomicBool,
    showing: AtomicBool,
    size: Mutex<Option<(i32, i32)>>,
    /// The audio's gain, 0 to 1, as `f32` bits; read on the read thread before each chunk plays.
    gain: AtomicU32,
    last: Mutex<Option<Picture>>,
}

/// What a read thread tells the main thread about connection `link` of embed `embed`.
enum Event {
    FirstFrame,
    Ended(String),
}

thread_local! {
    /// Every stream embed by id, for what the read threads say on the main queue.
    static EMBEDS: RefCell<HashMap<u64, Weak<StreamEmbed>>> = RefCell::default();
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn on_main(embed: u64, link: u64, event: Event) {
    DispatchQueue::main().exec_async(move || {
        if let Some(e) = EMBEDS.with_borrow(|m| m.get(&embed).and_then(Weak::upgrade)) {
            e.heard(link, event);
        }
    });
}

/// One open connection to a stream socket: a thread reads and decodes, another writes.
struct Link {
    no: u64,
    path: String,
    shared: Arc<Shared>,
    writes: mpsc::Sender<Vec<u8>>,
}

impl Link {
    fn start(embed: u64, path: &str, stats: Arc<Stats>, renderer: Retained<AVSampleBufferVideoRenderer>, gain: f32) -> Self {
        let no = NEXT.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            stats,
            renderer: Renderer(renderer),
            socket: OnceLock::new(),
            closed: AtomicBool::new(false),
            showing: AtomicBool::new(false),
            size: Mutex::new(None),
            gain: AtomicU32::new(gain.to_bits()),
            last: Mutex::new(None),
        });
        let (writes, queue) = mpsc::channel::<Vec<u8>>();
        let (s, p) = (shared.clone(), path.to_owned());
        let _ = std::thread::Builder::new().name("wheel.videostream.read".into()).spawn(move || {
            let why = read(&s, &p, embed, no);
            if let Some(socket) = s.socket.get() {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            // A link its slot closed takes its last picture with it, so another stream starts on black.
            // SAFETY: the renderer takes a flush from any thread.
            unsafe { s.renderer.0.flushWithRemovalOfDisplayedImage_completionHandler(s.closed.load(Ordering::SeqCst), None) };
            if !s.closed.load(Ordering::SeqCst) {
                on_main(embed, no, Event::Ended(why));
            }
        });
        let w = shared.clone();
        let _ = std::thread::Builder::new().name("wheel.videostream.write".into()).spawn(move || {
            for bytes in queue {
                let Some(mut socket) = w.socket.get().filter(|_| !w.closed.load(Ordering::SeqCst)) else { continue };
                if socket.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        Self { no, path: path.to_owned(), shared, writes }
    }

    /// Ends the connection; the read thread sees the end and returns without reporting it.
    fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        if let Some(socket) = self.shared.socket.get() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }

    fn size(&self) -> Option<(i32, i32)> {
        *self.shared.size.lock().unwrap()
    }
}

/// An I/O error's words without Rust's `(os error N)`, as `strerror` gives them.
fn words(e: &std::io::Error) -> String {
    let s = e.to_string();
    s.split(" (os error").next().unwrap_or(&s).to_owned()
}

/// Connects and reads until the stream ends; returns why it ended.
fn read(s: &Arc<Shared>, path: &str, embed: u64, no: u64) -> String {
    if path.len() >= 104 {
        return format!("the stream socket path is too long: {path}");
    }
    let socket = match UnixStream::connect(path) {
        Ok(socket) => socket,
        Err(e) => {
            let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return format!("The backend does not serve {name} yet ({})", words(&e));
        }
    };
    let _ = s.socket.set(socket);
    if s.closed.load(Ordering::SeqCst) {
        return "closed".into();
    }
    let mut socket = s.socket.get().unwrap();
    let mut audio = audio::Audio::new();
    let shown = s.clone();
    let mut decoder = decoder::Decoder::new(Arc::new(move |sample: &CMSampleBuffer| {
        if shown.closed.load(Ordering::SeqCst) {
            return;
        }
        // SAFETY: a decoded sample, onto the layer at once from VideoToolbox's thread.
        unsafe {
            let r = &shown.renderer.0;
            if r.status() == AVQueuedSampleBufferRenderingStatus::Failed {
                r.flush();
            }
            r.enqueueSampleBuffer(sample);
            *shown.last.lock().unwrap() = sample.image_buffer().map(Picture);
        }
        shown.stats.frames.fetch_add(1, Ordering::Relaxed);
        if !shown.showing.swap(true, Ordering::SeqCst) {
            on_main(embed, no, Event::FirstFrame);
        }
    }));
    let mut reader = frames::Reader::default();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = match socket.read(&mut buffer) {
            Ok(0) => return "The stream ended".into(),
            Ok(n) => n,
            Err(e) => return format!("The stream ended: {}", words(&e)),
        };
        s.stats.bytes.fetch_add(n as u64, Ordering::Relaxed);
        let frames = match reader.push(&buffer[..n]) {
            Ok(frames) => frames,
            Err(why) => return format!("The stream broke: {why}"),
        };
        for frame in frames {
            match frame {
                Frame::ParameterSets { codec, units } => {
                    if !decoder.parameter_sets(codec, &units) {
                        return decoder.last_error.take().unwrap_or_else(|| "bad parameter sets".into());
                    }
                    if let Some(d) = decoder.dimensions() {
                        *s.size.lock().unwrap() = Some(d);
                    }
                }
                Frame::Video { time, units } => decoder.decode(time, &units),
                Frame::Audio { rate, channels, pcm } => {
                    audio.set_gain(f32::from_bits(s.gain.load(Ordering::Relaxed)));
                    if audio.play(f64::from(rate), usize::from(channels), &pcm) {
                        s.stats.audio.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Frame::Unknown(_) => {}
            }
        }
    }
}

#[derive(Default)]
pub struct ViewIvars {
    display: OnceCell<Retained<AVSampleBufferDisplayLayer>>,
    note: OnceCell<Retained<NSTextField>>,
    takes_input: Cell<bool>,
    fills: Cell<bool>,
    owner: RefCell<Weak<StreamEmbed>>,
}

define_class!(
    /// The view a stream draws in: the display layer, fitted to the view with the video's aspect (or
    /// filling it, cropped, when `fills`), the note over it, and the keys and pointer events on it,
    /// sent as input frames. A pointer's `x` and `y` are in the video's pixels, from its top left.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelStreamView"]
    #[ivars = ViewIvars]
    pub struct StreamView;

    impl StreamView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().takes_input.get()
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            self.ivars().takes_input.get()
        }

        /// The note is drawn over the picture, never hit: a press anywhere is the stream's.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: NSView's own hitTest:, which returns an autoreleased view or nil.
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            if hit.is_null() { hit } else { (self as *const Self).cast_mut().cast() }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: NSView's own setFrameSize:.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.lay_out();
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: NSView's own updateTrackingAreas.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            for area in self.trackingAreas().iter() {
                self.removeTrackingArea(&area);
            }
            let options = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveInKeyWindow | NSTrackingAreaOptions::InVisibleRect;
            // SAFETY: a tracking area owned by this view, which outlives it.
            let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, options, Some(self), None) };
            self.addTrackingArea(&area);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), mouseDown: event] };
            }
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            self.pointer("down", event, Some(0));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.pointer("up", event, Some(0));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), rightMouseDown: event] };
            }
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            self.pointer("down", event, Some(1));
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.pointer("up", event, Some(1));
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.pointer("down", event, Some(event.buttonNumber() as i64));
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.pointer("up", event, Some(event.buttonNumber() as i64));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.pointer("move", event, None);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.pointer("move", event, None);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.pointer("move", event, None);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.pointer("move", event, None);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), scrollWheel: event] };
            }
            self.send(Input::Pointer, &json!({"kind": "scroll", "dx": event.scrollingDeltaX(), "dy": event.scrollingDeltaY(), "precise": event.hasPreciseScrollingDeltas()}));
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), keyDown: event] };
            }
            self.send(Input::Key, &key("down", event));
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), keyUp: event] };
            }
            self.send(Input::Key, &key("up", event));
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            if !self.ivars().takes_input.get() {
                return unsafe { msg_send![super(self), flagsChanged: event] };
            }
            self.send(Input::Key, &json!({"kind": "modifiers", "code": event.keyCode(), "modifiers": modifiers(event.modifierFlags())}));
        }
    }
);

fn modifiers(f: NSEventModifierFlags) -> Vec<&'static str> {
    frames::modifier_names(f.contains(NSEventModifierFlags::Shift), f.contains(NSEventModifierFlags::Control), f.contains(NSEventModifierFlags::Option), f.contains(NSEventModifierFlags::Command), f.contains(NSEventModifierFlags::CapsLock))
}

/// A key event: `code` is the Mac's virtual key code, `key` the characters it types without modifiers.
fn key(kind: &str, event: &NSEvent) -> Value {
    let chars = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
    frames::key_event(kind, event.keyCode(), &chars, event.isARepeat(), &modifiers(event.modifierFlags()))
}

impl StreamView {
    fn new() -> Retained<Self> {
        let mtm = main_thread();
        let this = mtm.alloc::<Self>().set_ivars(ViewIvars::default());
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        // SAFETY: a fresh display layer under the view's own layer.
        let display = unsafe { AVSampleBufferDisplayLayer::new() };
        if let Some(layer) = this.layer() {
            layer.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
            layer.addSublayer(&display);
        }
        let note = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        note.setFont(Some(&NSFont::systemFontOfSize(10.)));
        note.setTextColor(Some(&NSColor::colorWithWhite_alpha(1., 0.8)));
        note.setAlignment(NSTextAlignment::Center);
        this.addSubview(&note);
        let _ = this.ivars().display.set(display);
        let _ = this.ivars().note.set(note);
        this.set_fills(false);
        this
    }

    fn display(&self) -> &AVSampleBufferDisplayLayer {
        self.ivars().display.get().unwrap()
    }

    fn set_fills(&self, fills: bool) {
        self.ivars().fills.set(fills);
        // SAFETY: AVFoundation's own gravity constants.
        if let Some(gravity) = unsafe { if fills { AVLayerVideoGravityResizeAspectFill } else { AVLayerVideoGravityResizeAspect } } {
            unsafe { self.display().setVideoGravity(gravity) };
        }
    }

    /// The display layer on the view's bounds and the note centred over it, 8 points in.
    fn lay_out(&self) {
        let b = self.bounds();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.display().setFrame(b);
        CATransaction::commit();
        let note = self.ivars().note.get().unwrap();
        let width = (b.size.width - 16.).max(0.);
        note.setPreferredMaxLayoutWidth(width);
        let fit = note.fittingSize();
        let (w, h) = (fit.width.min(width), fit.height.min(b.size.height));
        note.setFrame(NSRect::new(NSPoint::new((b.size.width - w) / 2., (b.size.height - h) / 2.), NSSize::new(w, h)));
    }

    fn show_note(&self, text: Option<&str>) {
        let note = self.ivars().note.get().unwrap();
        note.setStringValue(&NSString::from_str(text.unwrap_or("")));
        note.setHidden(text.is_none());
        self.lay_out();
    }

    fn send(&self, kind: Input, event: &Value) {
        if let Some(owner) = self.ivars().owner.borrow().upgrade() {
            owner.send(kind, event);
        }
    }

    /// Where `event` falls in the video, in its pixels from the top left; nothing before the video's size is known.
    fn pointer(&self, kind: &str, event: &NSEvent, button: Option<i64>) {
        if !self.ivars().takes_input.get() {
            return;
        }
        self.pointer_at(kind, self.convertPoint_fromView(event.locationInWindow(), None), button);
    }

    /// A pointer event at `p`, in the view's own coordinates.
    fn pointer_at(&self, kind: &str, p: NSPoint, button: Option<i64>) {
        let Some(owner) = self.ivars().owner.borrow().upgrade() else { return };
        let Some((w, h)) = owner.video_size().filter(|(w, h)| *w > 0 && *h > 0) else { return };
        let b = self.bounds();
        let (x, y) = frames::video_point((p.x, p.y), (f64::from(w), f64::from(h)), [0., 0., b.size.width, b.size.height], self.ivars().fills.get());
        let mut o = json!({"kind": kind, "x": x as i64, "y": y as i64});
        if let Some(button) = button {
            o["button"] = json!(button);
        }
        owner.send(Input::Pointer, &o);
    }
}

/// One stream shown in an embed slot: its view, and the connection to the backend while one is open.
pub struct StreamEmbed {
    id: u64,
    key: String,
    pub view: Retained<StreamView>,
    stream: RefCell<String>,
    stats: RefCell<Arc<Stats>>,
    link: RefCell<Option<Link>>,
    /// The socket the open connection reads, kept after it ends until the client names another.
    socket: RefCell<Option<String>>,
    error: RefCell<Option<String>>,
    note: RefCell<Option<String>>,
    input: Cell<bool>,
    gain: Cell<f32>,
    frames: UnboundedSender<String>,
}

impl StreamEmbed {
    /// The stream embed under slot `key`; a connection's end goes to the client through `frames`.
    pub fn new(key: &str, frames: UnboundedSender<String>) -> Rc<Self> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let e = Rc::new(Self {
            id,
            key: key.to_owned(),
            view: StreamView::new(),
            stream: RefCell::default(),
            stats: RefCell::default(),
            link: RefCell::default(),
            socket: RefCell::default(),
            error: RefCell::default(),
            note: RefCell::default(),
            input: Cell::new(false),
            gain: Cell::new(1.),
            frames,
        });
        *e.view.ivars().owner.borrow_mut() = Rc::downgrade(&e);
        EMBEDS.with_borrow_mut(|m| m.insert(id, Rc::downgrade(&e)));
        controllers::add(Rc::downgrade(&e));
        e
    }

    /// Show `what`, a `Embedded::Stream`: another stream starts over, a new socket connects, and
    /// wheeld's refusal or a connection's end shows as the note.
    pub fn open(&self, what: &Embedded) {
        let Embedded::Stream { stream, socket, error, input, fills, gain } = what else { return };
        if *self.stream.borrow() != *stream {
            self.close();
            // SAFETY: the display layer's own renderer, on the main thread.
            unsafe { self.view.display().sampleBufferRenderer().flushWithRemovalOfDisplayedImage_completionHandler(true, None) };
            *self.stream.borrow_mut() = stream.clone();
            *self.stats.borrow_mut() = Arc::default();
            *self.socket.borrow_mut() = None;
            self.set_note(Some(format!("Connecting to {stream}")));
        }
        self.input.set(*input);
        self.view.ivars().takes_input.set(*input);
        if self.view.ivars().fills.get() != *fills {
            self.view.set_fills(*fills);
        }
        self.gain.set(*gain);
        if let Some(link) = self.link.borrow().as_ref() {
            link.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
        }
        if let Some(error) = error {
            *self.error.borrow_mut() = Some(error.clone());
            self.close();
            self.set_note(Some(error.clone()));
        }
        if socket.is_some() && *socket != *self.socket.borrow() {
            self.close();
            let path = socket.clone().unwrap();
            // SAFETY: the display layer's own renderer.
            let renderer = unsafe { self.view.display().sampleBufferRenderer() };
            *self.link.borrow_mut() = Some(Link::start(self.id, &path, self.stats.borrow().clone(), renderer, *gain));
            *self.socket.borrow_mut() = Some(path);
        }
    }

    fn set_note(&self, note: Option<String>) {
        self.view.show_note(note.as_deref());
        *self.note.borrow_mut() = note;
    }

    /// Ends the open connection, if any, without reporting it.
    pub fn close(&self) {
        if let Some(link) = self.link.borrow_mut().take() {
            link.close();
        }
    }

    fn heard(&self, no: u64, event: Event) {
        if self.link.borrow().as_ref().is_none_or(|l| l.no != no) {
            return;
        }
        match event {
            Event::FirstFrame => {
                *self.error.borrow_mut() = None;
                self.set_note(None);
            }
            Event::Ended(why) => {
                let (path, shown) = self.link.borrow_mut().take().map(|l| (l.path, l.shared.showing.load(Ordering::SeqCst))).unwrap_or_default();
                *self.socket.borrow_mut() = None;
                *self.error.borrow_mut() = Some(why.clone());
                self.set_note(Some(why.clone()));
                let _ = self.frames.unbounded_send(json!({"event": "stream_ended", "pane": self.key, "socket": path, "why": why, "shown": shown}).to_string());
            }
        }
    }

    fn video_size(&self) -> Option<(i32, i32)> {
        self.link.borrow().as_ref().and_then(Link::size)
    }

    fn send(&self, kind: Input, event: &Value) {
        if !self.input.get() {
            return;
        }
        let Some(link) = self.link.borrow().as_ref().map(|l| l.writes.clone()) else { return };
        if link.send(frames::input(kind, event)).is_ok() {
            self.stats.borrow().inputs.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A check's key typing `characters` on the stream, as `VideoStreams.type` hands the view a key
    /// down and up (key code 0, no modifiers); false when it takes no input.
    pub fn type_characters(&self, characters: &str) -> bool {
        if !self.input.get() {
            return false;
        }
        for kind in ["down", "up"] {
            self.send(Input::Key, &frames::key_event(kind, 0, characters, false, &[]));
        }
        true
    }

    /// A check's left click at the middle of the stream, as `VideoStreams.click` hands the view a
    /// mouse down (which takes the keyboard) and up; false when it takes no input or is in no window.
    pub fn click_middle(&self) -> bool {
        let Some(window) = self.view.window().filter(|_| self.input.get()) else { return false };
        window.makeFirstResponder(Some(&self.view));
        let b = self.view.bounds();
        let middle = NSPoint::new(b.size.width / 2., b.size.height / 2.);
        for kind in ["down", "up"] {
            self.view.pointer_at(kind, middle, Some(0));
        }
        true
    }

    /// Whether controller events go here: the stream takes input, is connected and its window is the key one.
    fn wants_controllers(&self) -> bool {
        self.input.get() && self.link.borrow().is_some() && self.view.window().is_some_and(|w| w.isKeyWindow())
    }

    /// What the check reads of the stream: its name, the frames decoded, the audio chunks played,
    /// the input events sent, the bytes read, the socket, why it shows nothing, and the video's size.
    pub fn facts(&self) -> Value {
        let s = self.stats.borrow();
        let mut o = json!({
            "stream": *self.stream.borrow(), "frames": s.frames.load(Ordering::Relaxed), "audio": s.audio.load(Ordering::Relaxed),
            "inputs": s.inputs.load(Ordering::Relaxed), "bytes": s.bytes.load(Ordering::Relaxed),
            "input": self.input.get(), "fills": self.view.ivars().fills.get(), "gain": self.gain.get(), "note": *self.note.borrow(),
            "connected": self.link.borrow().is_some(), "layer_shows": self.layer_shows(),
        });
        if let Some(socket) = self.socket.borrow().as_ref() {
            o["socket"] = json!(socket);
        }
        if let Some(error) = self.error.borrow().as_ref() {
            o["error"] = json!(error);
        }
        if let Some((w, h)) = self.video_size() {
            o["video"] = json!([w, h]);
        }
        o
    }

    /// Whether the display layer holds a picture it shows.
    fn layer_shows(&self) -> bool {
        // SAFETY: the display layer's own renderer, on the main thread.
        unsafe { self.view.display().sampleBufferRenderer().copyDisplayedPixelBuffer() }.is_some()
    }

    /// The stream's picture as a PNG in the video's pixels: the frame the layer shows, else the last one decoded.
    pub fn png(&self) -> Option<Vec<u8>> {
        // SAFETY: the display layer's own renderer, on the main thread; a decoded frame is never written again.
        let image = match unsafe { self.view.display().sampleBufferRenderer().copyDisplayedPixelBuffer() } {
            Some(buffer) => unsafe { CIImage::imageWithCVPixelBuffer(&buffer) },
            None => {
                let link = self.link.borrow();
                let last = link.as_ref()?.shared.last.lock().unwrap();
                unsafe { CIImage::imageWithCVImageBuffer(&last.as_ref()?.0) }
            }
        };
        let rep = NSBitmapImageRep::initWithCIImage(NSBitmapImageRep::alloc(), &image);
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        Some(data.to_vec())
    }
}

impl Drop for StreamEmbed {
    fn drop(&mut self) {
        self.close();
        let id = self.id;
        let _ = EMBEDS.try_with(|m| m.borrow_mut().remove(&id));
        self.view.removeFromSuperview();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A backend serving the gallery's clip on a socket: the link reads and decodes every frame,
    /// learns the picture's size, and its input frame reaches the backend as written.
    #[test]
    fn a_link_decodes_the_backends_frames_and_writes_input_back() {
        let dir = std::env::temp_dir().join(format!("wheel-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let clip = std::fs::read(format!("{}/../../plugins/gallery/demo.h264", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let units = frames::nal_units(&clip);
        let annex = |us: &[Vec<u8>]| us.iter().flat_map(|u| [&[0u8, 0, 0, 1][..], u].concat()).collect::<Vec<u8>>();
        let sets: Vec<Vec<u8>> = [7u8, 8].iter().filter_map(|t| units.iter().find(|u| u[0] & 0x1F == *t).cloned()).collect();
        let mut access = vec![];
        let mut pending = vec![];
        for unit in units.into_iter().filter(|u| !matches!(u[0] & 0x1F, 7 | 8)) {
            let end = matches!(unit[0] & 0x1F, 1 | 5);
            pending.push(unit);
            if end {
                access.push(std::mem::take(&mut pending));
            }
        }
        let (got, heard) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.write_all(&frames::frame(1, &[&[1u8][..], &annex(&sets)].concat())).unwrap();
            for (i, a) in access.iter().enumerate() {
                conn.write_all(&frames::frame(2, &[&(i as u64 * 33_333).to_be_bytes()[..], &annex(a)].concat())).unwrap();
            }
            let mut head = [0u8; 5];
            conn.read_exact(&mut head).unwrap();
            let mut body = vec![0u8; u32::from_be_bytes(head[1..].try_into().unwrap()) as usize];
            conn.read_exact(&mut body).unwrap();
            got.send((head[0], String::from_utf8(body).unwrap())).unwrap();
        });
        let stats = Arc::new(Stats::default());
        // SAFETY: a display layer used only for its renderer.
        let renderer = unsafe { AVSampleBufferDisplayLayer::new().sampleBufferRenderer() };
        let link = Link::start(0, path.to_str().unwrap(), stats.clone(), renderer, 1.);
        for _ in 0..200 {
            if stats.frames.load(Ordering::Relaxed) >= 60 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(stats.frames.load(Ordering::Relaxed), 60);
        assert_eq!(link.size(), Some((320, 180)));
        assert!(stats.bytes.load(Ordering::Relaxed) > 0);
        link.writes.send(frames::input(Input::Pointer, &json!({"kind": "up", "x": 160, "y": 90, "button": 0}))).unwrap();
        let (kind, body) = heard.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!((kind, serde_json::from_str::<Value>(&body).unwrap()), (17, json!({"kind": "up", "x": 160, "y": 90, "button": 0})));
        link.close();
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_socket_nobody_serves_says_the_backend_does_not_serve_it_yet() {
        let s = Arc::new(Shared {
            stats: Arc::default(),
            renderer: Renderer(unsafe { AVSampleBufferDisplayLayer::new().sampleBufferRenderer() }),
            socket: OnceLock::new(),
            closed: AtomicBool::new(false),
            showing: AtomicBool::new(false),
            size: Mutex::new(None),
            gain: AtomicU32::new(1f32.to_bits()),
            last: Mutex::new(None),
        });
        let why = read(&s, "/private/tmp/no-such-wheel-stream/demo.sock", 0, 0);
        assert_eq!(why, "The backend does not serve demo yet (No such file or directory)");
    }
}
