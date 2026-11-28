//! Browser panes, plain web panes (a URL) and artifact pages as WKWebViews made here and put in
//! the pane host's slots (wheel-kq6t.2, wheel-kq6t.3, ADR-0056). An artifact's page opens under the
//! `wheel-page` scheme `page_scheme` answers, in its own website data store (ADR-0048). A browser is an ordinary browser on any http,
//! https or file page, with the default website data store, so a login survives a relaunch; its
//! chrome state, typed text and console go to the GPUI view as frames on the window's channel.
//! Every WKWebView in a slot (an artifact page's too) is drawn here for `render_pane` and the
//! check's snapshots; `inspect_pane` runs in a browser's page.
use super::{main_thread, page_scheme::PageScheme};
use base64::Engine as _;
use block2::{DynBlock, RcBlock};
use futures::channel::mpsc::UnboundedSender;
use objc2::{
    DefinedClass, MainThreadOnly, Message as _, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSGraphicsContext, NSImage, NSView, NSWorkspace};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSHTTPURLResponse, NSJSONSerialization, NSJSONWritingOptions, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSNumber, NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest, NSUUID,
};
use objc2_web_kit::{
    WKFrameInfo, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKNavigationType, WKScriptMessage, WKScriptMessageHandler, WKSnapshotConfiguration, WKUIDelegate,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
};
use wheel_gpui_core::artifact_page;
use serde_json::{Map, Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, VecDeque},
    ffi::c_void,
    rc::{Rc, Weak},
};

/// The name the console script posts to (`BrowserConsole.handlerName`).
const CONSOLE: &str = "wheelConsole";
/// The key paths of a browser's view that change its chrome.
const OBSERVED: [&str; 6] = ["URL", "title", "loading", "estimatedProgress", "canGoBack", "canGoForward"];

/// The console script WheelUI's `BrowserConsole.script` holds, read out of that Swift raw string,
/// so the phone's browser and this one run the same text.
fn console_script() -> String {
    const SOURCE: &str = include_str!("../../../../apple/WheelUI/Sources/WheelUI/BrowserConsole.swift");
    let start = SOURCE.find("script = #\"\"\"\n").map(|i| i + "script = #\"\"\"\n".len()).expect("BrowserConsole.swift holds the script");
    let end = start + SOURCE[start..].find("\"\"\"#").expect("the script's closing delimiter");
    // Swift drops the closing delimiter's indentation from every line, and the newline before it.
    let body = SOURCE[start..end].trim_end_matches(' ');
    let indent = SOURCE[start..end].len() - body.len();
    body.strip_suffix('\n').unwrap_or(body).lines().map(|l| l.get(indent.min(l.len())..).unwrap_or("")).collect::<Vec<_>>().join("\n")
}

/// The last load that failed, shown over the page until the next one starts.
struct Failure {
    /// The address that failed, as the field shows it.
    address: String,
    /// The page to try again; None when wheeld refused the text as no page at all.
    target: Option<String>,
    message: String,
}

/// A browser pane's model: what WheelUI's `BrowserPane` held.
struct Browser {
    /// Whether wheeld runs on another machine, so a file page names a path this Mac does not have.
    remote: bool,
    /// The pane's home as the layout last named it; the browser goes there only on `go_home`.
    home: String,
    title: String,
    loading: bool,
    progress: f64,
    can_go_back: bool,
    can_go_forward: bool,
    failure: Option<Failure>,
}

/// An artifact's page: which artifact in which space directory (another of either is another
/// view, since the store and handler are bound to both), its store and its scheme handler.
struct Artifact {
    key: String,
    /// wheeld's page id: the page opens at `/artifacts/<id>/` and stays in that folder. A web pane's is its pane id.
    id: String,
    store: String,
    scheme: Retained<PageScheme>,
}

struct Page {
    view: Retained<WKWebView>,
    delegate: Retained<PageDelegate>,
    /// A browser's model; None for a web pane.
    browser: Option<Browser>,
    /// An artifact page's; None for a browser or a web pane on a plain URL.
    artifact: Option<Artifact>,
    /// A plugin page's `WebView{url}`: it navigates only to http, https and about, a new window in place.
    embedded: bool,
    /// The page the view is on, or the one it was given before anything committed.
    url: String,
    /// Each finished load's title.
    loads: Vec<String>,
    reloads: u64,
    /// The main frame's last HTTP status.
    status: Option<isize>,
    /// The page's visible text after the last load.
    text: Option<String>,
}

/// The browser and web pages of one window, by pane id.
pub struct Pages {
    me: Weak<Pages>,
    frames: UnboundedSender<String>,
    pages: RefCell<HashMap<String, Page>>,
    /// What the browsers logged, the last 200 lines, for the check's report.
    logs: RefCell<VecDeque<String>>,
    errors: RefCell<Vec<String>>,
    /// The panes whose snapshot has not answered yet.
    waiting: RefCell<BTreeSet<String>>,
    /// wheeld's socket address and token, as the last artifact open named them, for every page's scheme handler.
    daemon: RefCell<Option<(String, String)>>,
}

pub struct DelegateIvars {
    pane: String,
    pages: Weak<Pages>,
}

define_class!(
    /// One page's navigation, UI, console and key-value observer: each call goes to its pane's model.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelPageDelegate"]
    #[ivars = DelegateIvars]
    pub struct PageDelegate;

    unsafe impl NSObjectProtocol for PageDelegate {}

    unsafe impl WKNavigationDelegate for PageDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_action(&self, _web: &WKWebView, action: &WKNavigationAction, handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>) {
            let allow = self.pages().is_none_or(|p| p.allows(&self.ivars().pane, action));
            handler.call((if allow { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(&self, _web: &WKWebView, response: &WKNavigationResponse, handler: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>) {
            let allow = self.pages().is_none_or(|p| p.shows(&self.ivars().pane, response));
            handler.call((if allow { WKNavigationResponsePolicy::Allow } else { WKNavigationResponsePolicy::Cancel },));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>) {
            if let Some(p) = self.pages() {
                p.finished(&self.ivars().pane);
            }
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(p) = self.pages() {
                p.failed(&self.ivars().pane, error, false);
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(p) = self.pages() {
                p.failed(&self.ivars().pane, error, true);
            }
        }
    }

    unsafe impl WKUIDelegate for PageDelegate {
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(&self, web: &WKWebView, _configuration: &WKWebViewConfiguration, action: &WKNavigationAction, _features: &WKWindowFeatures) -> *mut WKWebView {
            if let Some(p) = self.pages() {
                p.new_window(&self.ivars().pane, web, action);
            }
            std::ptr::null_mut()
        }
    }

    unsafe impl WKScriptMessageHandler for PageDelegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            if let Some(p) = self.pages() {
                p.console(&self.ivars().pane, &*unsafe { message.body() });
            }
        }
    }

    impl PageDelegate {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(&self, _key: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut c_void) {
            if let Some(p) = self.pages() {
                p.observed(&self.ivars().pane);
            }
        }
    }
);

impl PageDelegate {
    fn new(pane: &str, pages: Weak<Pages>) -> Retained<Self> {
        let this = main_thread().alloc::<Self>().set_ivars(DelegateIvars { pane: pane.to_owned(), pages });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    fn pages(&self) -> Option<Rc<Pages>> {
        self.ivars().pages.upgrade()
    }
}

/// `f` run once by a block WebKit may hold as `Fn`.
fn once<A, F: FnOnce(A) + 'static>(f: F) -> impl Fn(A) + 'static {
    let cell = Cell::new(Some(f));
    move |a| {
        if let Some(f) = cell.take() {
            f(a)
        }
    }
}

/// Whether a plugin page's `WebView{url}` may go to `target` (`EmbeddedWebLoader.allowed`).
fn embedded_allows(target: Option<&NSURL>) -> bool {
    target.and_then(|t| t.scheme()).is_some_and(|s| embedded_scheme(&s.to_string()))
}

fn embedded_scheme(scheme: &str) -> bool {
    matches!(scheme.to_lowercase().as_str(), "http" | "https" | "about")
}

fn url_of(text: &str) -> Option<Retained<NSURL>> {
    NSURL::URLWithString(&NSString::from_str(text))
}

fn absolute(url: &NSURL) -> String {
    url.absoluteString().map(|s| s.to_string()).unwrap_or_default()
}

/// What a file URL names as a path, or what a web one is.
fn address_of(url: &NSURL) -> String {
    if url.isFileURL() { url.path().map(|p| p.to_string()).unwrap_or_default() } else { absolute(url) }
}

/// `text` as a JavaScript string literal.
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// A script's result as JSON text (sorted keys); `undefined` and `null` are `null`.
fn json_text(value: Option<&AnyObject>) -> Option<String> {
    let Some(value) = value else { return Some("null".into()) };
    if value.downcast_ref::<objc2_foundation::NSNull>().is_some() {
        return Some("null".into());
    }
    let array = NSArray::from_slice(&[value]);
    if !unsafe { NSJSONSerialization::isValidJSONObject(&array) } {
        return None;
    }
    let data = unsafe { NSJSONSerialization::dataWithJSONObject_options_error(&array, NSJSONWritingOptions::SortedKeys) }.ok()?;
    let text = String::from_utf8(data.to_vec()).ok()?;
    Some(text.strip_prefix('[')?.strip_suffix(']')?.to_owned())
}

/// What a page threw, as the page worded it.
fn script_message(error: &NSError) -> String {
    let message = error.userInfo().objectForKey(&NSString::from_str("WKJavaScriptExceptionMessage"));
    message.and_then(|m| m.downcast::<NSString>().ok()).map_or_else(|| error.localizedDescription().to_string(), |m| m.to_string())
}

/// `script` evaluated in `view`; `done` gets the result or the page's error.
fn evaluate(view: &WKWebView, script: &str, done: impl FnOnce(Result<Option<Retained<AnyObject>>, String>) + 'static) {
    let done = once(done);
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit hands the block its result and error, each nil or alive for the call.
        let outcome = match unsafe { error.as_ref() } {
            Some(error) => Err(script_message(error)),
            None => Ok(unsafe { Retained::retain(value) }),
        };
        done(outcome);
    });
    unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&block)) };
}

/// `script`'s result as text, for a script that returns a string.
fn evaluate_text(view: &WKWebView, script: &str, done: impl FnOnce(Result<Option<String>, String>) + 'static) {
    evaluate(view, script, move |r| done(r.map(|v| v.and_then(|v| v.downcast::<NSString>().ok()).map(|s| s.to_string()))));
}

/// Every WKWebView shown inside `view`, at any depth.
fn web_views(view: &NSView, out: &mut Vec<Retained<WKWebView>>) {
    for sub in view.subviews() {
        if sub.isHiddenOrHasHiddenAncestor() || sub.bounds().size.width <= 0. || sub.bounds().size.height <= 0. {
            continue;
        }
        match sub.downcast::<WKWebView>() {
            Ok(web) => out.push(web),
            Err(sub) => web_views(&sub, out),
        }
    }
}

/// The view's own drawing of the page, or why WebKit drew none.
pub fn snapshot_image(view: &WKWebView, configuration: Option<&WKSnapshotConfiguration>, done: impl FnOnce(Result<Retained<NSImage>, String>) + 'static) {
    let done = once(done);
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        // SAFETY: WebKit hands the block an image or an error, each nil or alive for the call.
        let outcome = match unsafe { Retained::retain(image) } {
            Some(image) => Ok(image),
            None => Err(format!("WebKit drew no image: {}", unsafe { error.as_ref() }.map_or("no error".into(), |e| e.localizedDescription().to_string()))),
        };
        done(outcome);
    });
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(configuration, &block) };
}

/// `image` as a PNG, keeping its pixels (a Retina snapshot stays Retina).
pub fn image_png(image: &NSImage) -> Option<Vec<u8>> {
    let tiff = image.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    // SAFETY: an empty properties dictionary is what the PNG encoder accepts.
    unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }.map(|d| d.to_vec())
}

/// `view` drawn by AppKit, as a PNG: a view showing no page, which WebKit may never snapshot.
pub(super) fn cached_png(view: &NSView) -> Option<Vec<u8>> {
    let rep = view.bitmapImageRepForCachingDisplayInRect(view.bounds())?;
    view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &rep);
    unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }.map(|d| d.to_vec())
}

/// The page as `view` draws it, the part in view, or with `full` the whole document as one tall image, as a PNG.
pub fn shot_page(view: &WKWebView, full: bool, done: impl FnOnce(Result<Vec<u8>, String>) + 'static) {
    let encode = |r: Result<Retained<NSImage>, String>| r.and_then(|i| image_png(&i).ok_or_else(|| "the drawing could not be encoded as a PNG".to_owned()));
    if !full {
        snapshot_image(view, None, move |r| done(encode(r)));
        return;
    }
    let size = "JSON.stringify([Math.max(document.documentElement.scrollWidth, document.body ? document.body.scrollWidth : 0), Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)])";
    let view2 = view.retain();
    evaluate_text(view, size, move |r| {
        let size = r.ok().flatten().and_then(|t| serde_json::from_str::<[f64; 2]>(&t).ok()).filter(|[w, h]| *w > 0. && *h > 0.);
        let configuration = size.map(|[w, h]| {
            let c = unsafe { WKSnapshotConfiguration::new(main_thread()) };
            unsafe {
                c.setRect(NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
                c.setSnapshotWidth(Some(&NSNumber::new_f64(w)));
            }
            c
        });
        snapshot_image(&view2, configuration.as_deref(), move |r| done(encode(r)));
    });
}

impl Pages {
    pub fn new(frames: UnboundedSender<String>) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            frames,
            pages: RefCell::default(),
            logs: RefCell::default(),
            errors: RefCell::default(),
            waiting: RefCell::default(),
            daemon: RefCell::default(),
        })
    }

    pub(super) fn daemon(&self) -> Option<(String, String)> {
        self.daemon.borrow().clone()
    }

    /// The status of the document `pane`'s main frame loaded.
    pub(super) fn document_status(&self, pane: &str, code: isize) {
        if let Some(page) = self.pages.borrow_mut().get_mut(pane) {
            page.status = Some(code);
        }
    }

    pub(super) fn log(&self, line: String) {
        let mut logs = self.logs.borrow_mut();
        logs.push_back(line);
        while logs.len() > 200 {
            logs.pop_front();
        }
    }

    fn emit(&self, value: Value) {
        let _ = self.frames.unbounded_send(value.to_string());
    }

    fn view(&self, pane: &str) -> Option<Retained<WKWebView>> {
        self.pages.borrow().get(pane).map(|p| p.view.clone())
    }

    fn is_artifact(&self, pane: &str) -> bool {
        self.pages.borrow().get(pane).is_some_and(|p| p.artifact.is_some())
    }

    /// The page id an artifact page stays inside.
    fn artifact_id(&self, pane: &str) -> Option<String> {
        self.pages.borrow().get(pane).and_then(|p| p.artifact.as_ref()).map(|a| a.id.clone())
    }

    fn is_embedded(&self, pane: &str) -> bool {
        self.pages.borrow().get(pane).is_some_and(|p| p.embedded)
    }

    fn is_browser(&self, pane: &str) -> bool {
        self.pages.borrow().get(pane).is_some_and(|p| p.browser.is_some())
    }

    /// A browser on `url`, or a web pane on the plain URL `url`, in `slot`. Opening a browser
    /// again makes `url` its home; opening a web pane again on another URL loads it.
    pub fn open(&self, pane: &str, slot: &NSView, browser: bool, url: &str, remote: bool) {
        let again = self.pages.borrow().get(pane).filter(|p| p.artifact.is_none() && !p.embedded).map(|p| (p.browser.is_some(), p.url.clone()));
        if again.is_none() {
            self.close(pane);
        }
        match again {
            Some((true, _)) if browser => {
                if let Some(b) = self.pages.borrow_mut().get_mut(pane).and_then(|p| p.browser.as_mut()) {
                    b.home = url.to_owned();
                }
                return;
            }
            Some((false, was)) if !browser => {
                if was != url {
                    self.pages.borrow_mut().get_mut(pane).unwrap().url = url.to_owned();
                    if let (Some(view), Some(target)) = (self.view(pane), url_of(url)) {
                        unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&target)) };
                    }
                }
                return;
            }
            Some(_) => self.close(pane),
            None => {}
        }
        let mtm = main_thread();
        let delegate = PageDelegate::new(pane, self.me.clone());
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        if browser {
            let controller = unsafe { configuration.userContentController() };
            let script = unsafe {
                WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(&console_script()),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    false,
                )
            };
            unsafe {
                controller.addUserScript(&script);
                controller.addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), &NSString::from_str(CONSOLE));
            }
        }
        let view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &configuration) };
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            if browser {
                view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                view.setAllowsBackForwardNavigationGestures(true);
                // Shows the pane under Safari's Develop menu.
                view.setInspectable(true);
                for key in OBSERVED {
                    view.addObserver_forKeyPath_options_context(&delegate, &NSString::from_str(key), NSKeyValueObservingOptions::New, std::ptr::null_mut());
                }
            }
        }
        view.setFrame(slot.bounds());
        slot.addSubview(&view);
        let model = browser.then(|| Browser {
            remote,
            home: url.to_owned(),
            title: String::new(),
            loading: false,
            progress: 0.,
            can_go_back: false,
            can_go_forward: false,
            failure: None,
        });
        self.pages
            .borrow_mut()
            .insert(pane.to_owned(), Page { view, delegate, browser: model, artifact: None, embedded: false, url: url.to_owned(), loads: vec![], reloads: 0, status: None, text: None });
        if browser {
            self.go(pane, url);
        } else if let (Some(view), Some(target)) = (self.view(pane), url_of(url)) {
            unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&target)) };
        }
    }

    /// A plugin page's `WebView{url}` in `slot`, as WheelUI's `EmbeddedWebLoader` makes it: a WKWebView
    /// with the default configuration, loading `url` once and again only when it names another.
    pub fn open_embedded(&self, key: &str, slot: &NSView, url: &str) {
        let again = self.pages.borrow().get(key).filter(|p| p.embedded).map(|p| p.url.clone());
        match again {
            Some(was) if was == url => return,
            Some(_) => {
                self.pages.borrow_mut().get_mut(key).unwrap().url = url.to_owned();
                if let (Some(view), Some(target)) = (self.view(key), url_of(url)) {
                    unsafe { drop(view.loadRequest(&NSURLRequest::requestWithURL(&target))) };
                }
                return;
            }
            None => self.close(key),
        }
        let mtm = main_thread();
        let delegate = PageDelegate::new(key, self.me.clone());
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &configuration) };
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        view.setFrame(slot.bounds());
        slot.addSubview(&view);
        self.pages
            .borrow_mut()
            .insert(key.to_owned(), Page { view, delegate, browser: None, artifact: None, embedded: true, url: url.to_owned(), loads: vec![], reloads: 0, status: None, text: None });
        if let (Some(view), Some(target)) = (self.view(key), url_of(url)) {
            unsafe { drop(view.loadRequest(&NSURLRequest::requestWithURL(&target))) };
        }
    }

    /// Artifact `name` (`scope` project or machine) of the space at `cwd` in `slot`: its page at
    /// `wheel-page://pages/artifacts/<pane>/`, fetched from wheeld at `ws` (its token in the query).
    /// Opening the same artifact for the same space again keeps the view.
    pub fn open_artifact(&self, pane: &str, slot: &NSView, name: &str, scope: &str, cwd: &str, ws: &str) {
        self.open_artifact_page(pane, pane, slot, name, scope, cwd, ws);
    }

    /// Artifact `name`'s page `id` under `key`: a web pane's own (`id` its pane id), or a plugin page's
    /// `WebView{artifact}` (`id` the page wheeld keeps for it). Another page id is another view.
    #[allow(clippy::too_many_arguments)]
    pub fn open_artifact_page(&self, key: &str, id: &str, slot: &NSView, name: &str, scope: &str, cwd: &str, ws: &str) {
        let pane = key;
        if let Some(daemon) = artifact_page::daemon_of(ws) {
            *self.daemon.borrow_mut() = Some(daemon);
        }
        let key = format!("{scope}/{name}\n{cwd}\n{id}");
        if self.pages.borrow().get(pane).and_then(|p| p.artifact.as_ref()).is_some_and(|a| a.key == key) {
            return;
        }
        self.close(pane);
        let mtm = main_thread();
        let store = artifact_page::store_id(cwd, scope == "machine", name);
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        if let Some(id) = NSUUID::from_string(&NSString::from_str(&store)) {
            unsafe { configuration.setWebsiteDataStore(&WKWebsiteDataStore::dataStoreForIdentifier(&id, mtm)) };
        }
        let scheme = PageScheme::new(pane, id, self.me.clone());
        unsafe { configuration.setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(&*scheme)), &NSString::from_str(artifact_page::SCHEME)) };
        let delegate = PageDelegate::new(pane, self.me.clone());
        let view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &configuration) };
        unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        view.setFrame(slot.bounds());
        slot.addSubview(&view);
        let url = artifact_page::page_url(id);
        self.pages.borrow_mut().insert(
            pane.to_owned(),
            Page { view, delegate, browser: None, artifact: Some(Artifact { key, id: id.to_owned(), store, scheme }), embedded: false, url: url.clone(), loads: vec![], reloads: 0, status: None, text: None },
        );
        self.load_page(pane);
    }

    /// Open an artifact pane's page address, which also serves as its reload: `WKWebView.reload()`
    /// does nothing when the first load never committed (wheeld was down).
    fn load_page(&self, pane: &str) {
        let Some(id) = self.artifact_id(pane) else { return };
        if let (Some(view), Some(target)) = (self.view(pane), url_of(&artifact_page::page_url(&id))) {
            unsafe { drop(view.loadRequest(&NSURLRequest::requestWithURL(&target))) };
        }
    }

    /// Drop `pane`'s page: its observers, console handler, scheme handler's requests and view.
    pub fn close(&self, pane: &str) {
        let Some(page) = self.pages.borrow_mut().remove(pane) else { return };
        self.waiting.borrow_mut().remove(pane);
        unsafe {
            if page.browser.is_some() {
                for key in OBSERVED {
                    page.view.removeObserver_forKeyPath(&page.delegate, &NSString::from_str(key));
                }
                page.view.configuration().userContentController().removeScriptMessageHandlerForName(&NSString::from_str(CONSOLE));
                page.view.setUIDelegate(None);
            }
            page.view.setNavigationDelegate(None);
            if let Some(a) = &page.artifact {
                page.view.stopLoading();
                a.scheme.close();
            }
        }
        page.view.removeFromSuperview();
    }

    pub fn close_all(&self) {
        let panes: Vec<String> = self.pages.borrow().keys().cloned().collect();
        for pane in panes {
            self.close(&pane);
        }
    }

    /// What the GPUI view draws a browser's chrome from.
    fn state(&self, pane: &str) -> Option<Value> {
        let pages = self.pages.borrow();
        let page = pages.get(pane)?;
        let b = page.browser.as_ref()?;
        let mut o = json!({"event": "browser", "pane": pane, "url": page.url, "address": b.failure.as_ref().map_or(&page.url, |f| &f.address),
            "title": b.title, "loading": b.loading, "progress": b.progress, "can_go_back": b.can_go_back, "can_go_forward": b.can_go_forward});
        if let Some(f) = &b.failure {
            o["failure"] = json!({"address": f.address, "message": f.message});
        }
        Some(o)
    }

    fn changed(&self, pane: &str) {
        if let Some(state) = self.state(pane) {
            self.emit(state);
        }
    }

    fn with_browser<R>(&self, pane: &str, f: impl FnOnce(&mut Page, &mut Browser) -> R) -> Option<R> {
        let mut pages = self.pages.borrow_mut();
        let page = pages.get_mut(pane)?;
        let mut browser = page.browser.take()?;
        let out = f(page, &mut browser);
        page.browser = Some(browser);
        Some(out)
    }

    /// `back`, `forward`, `reload`, `stop`, `clear_failure`, `open` (typed text), `opened` and
    /// `refused` (wheeld's answers to typed text), `home_is` (the pane's home in the layout) and `go_home`.
    pub fn browser(&self, pane: &str, action: &str, value: &Value) {
        let (Some(view), true) = (self.view(pane), self.is_browser(pane)) else { return };
        let text = |k: &str| value[k].as_str().unwrap_or_default().to_owned();
        match action {
            "back" => unsafe { drop(view.goBack()) },
            "forward" => unsafe { drop(view.goForward()) },
            "stop" => unsafe { view.stopLoading() },
            "reload" => self.reload_browser(pane),
            "clear_failure" => {
                self.with_browser(pane, |_, b| b.failure = None);
            }
            "open" => {
                let typed = text("text");
                self.log(format!("browser pane {pane} asking the daemon what typed text means, {} chars", typed.chars().count()));
                self.emit(json!({"event": "typed", "pane": pane, "text": typed}));
            }
            "opened" => self.opened(pane, &text("url")),
            "refused" => {
                let message = text("message");
                self.log(format!("browser pane {pane} typed text refused: {message}"));
                self.with_browser(pane, |_, b| b.failure = Some(Failure { address: text("text"), target: None, message }));
            }
            "home_is" => {
                self.with_browser(pane, |_, b| b.home = text("url"));
            }
            "go_home" => {
                if let Some(home) = self.with_browser(pane, |_, b| b.home.clone()) {
                    self.log(format!("browser pane {pane} going home to {home}"));
                    self.go(pane, &home);
                }
            }
            _ => {}
        }
        self.changed(pane);
    }

    /// wheeld read typed text as `url`, or an agent's `pane page` sent the browser there: it goes
    /// there even when it is already on it, as a browser does.
    fn opened(&self, pane: &str, url: &str) {
        self.log(format!("browser pane {pane} opening {url}"));
        self.go(pane, url);
    }

    fn go(&self, pane: &str, url: &str) {
        if let Some(page) = self.pages.borrow_mut().get_mut(pane) {
            page.url = url.to_owned();
        }
        if let Some(target) = url_of(url) {
            self.load(pane, &target);
        }
        self.changed(pane);
    }

    fn load(&self, pane: &str, target: &NSURL) {
        let Some(remote) = self.with_browser(pane, |page, b| {
            b.failure = None;
            page.status = None;
            b.remote
        }) else {
            return;
        };
        let Some(view) = self.view(pane) else { return };
        if target.isFileURL() && remote {
            // The path is on wheeld's machine; a file of this Mac's at the same path is another file.
            let path = address_of(target);
            self.log(format!("browser pane {pane} refused file {path}: wheeld is on another machine"));
            let message = "This file is on the machine wheeld runs on, which this Mac can’t reach.".to_owned();
            self.with_browser(pane, |_, b| b.failure = Some(Failure { address: path, target: Some(absolute(target)), message }));
        } else if target.isFileURL() {
            // The whole disk, so a page reaches every stylesheet, script and image it names
            // wherever it sits: the person's call, knowing the page's scripts can read any file.
            unsafe { drop(view.loadFileURL_allowingReadAccessToURL(target, &NSURL::fileURLWithPath(&NSString::from_str("/")))) };
        } else {
            unsafe { drop(view.loadRequest(&NSURLRequest::requestWithURL(target))) };
        }
    }

    /// Reload the page, or try the failed address again.
    fn reload_browser(&self, pane: &str) {
        let Some(view) = self.view(pane) else { return };
        let Some((failure, url)) = self.with_browser(pane, |page, b| (b.failure.as_ref().map(|f| f.target.clone()), page.url.clone())) else { return };
        match failure {
            // Text that was never an address has nothing to try again; the page behind it shows.
            Some(None) => {
                self.with_browser(pane, |_, b| b.failure = None);
            }
            Some(Some(target)) => {
                if let Some(target) = url_of(&target) {
                    self.load(pane, &target);
                }
            }
            None if unsafe { view.URL() }.is_none() => self.go(pane, &url),
            None => {
                self.log(format!("browser pane {pane} reloading"));
                unsafe { drop(view.reload()) };
            }
        }
    }

    /// `pane_reload`: a browser reloads the page it is on, a web pane its URL.
    pub fn reload(&self, pane: &str) -> bool {
        let Some(view) = self.view(pane) else { return false };
        if let Some(page) = self.pages.borrow_mut().get_mut(pane) {
            page.reloads += 1;
        }
        if self.is_browser(pane) {
            self.reload_browser(pane);
            self.changed(pane);
        } else if self.is_artifact(pane) {
            self.load_page(pane);
        } else {
            unsafe { drop(view.reload()) };
        }
        true
    }

    /// A browser's view changed a property its chrome shows.
    fn observed(&self, pane: &str) {
        let Some(view) = self.view(pane) else { return };
        let (url, title, loading, progress, back, forward) = unsafe {
            (view.URL().map(|u| absolute(&u)), view.title().map(|t| t.to_string()).unwrap_or_default(), view.isLoading(), view.estimatedProgress(), view.canGoBack(), view.canGoForward())
        };
        self.with_browser(pane, |page, b| {
            // The view moved to a new page: a load committed, a redirect, or an in-page navigation.
            if let Some(url) = url {
                page.url = url;
            }
            b.title = title;
            b.loading = loading;
            b.progress = progress;
            b.can_go_back = back;
            b.can_go_forward = forward;
        });
        self.changed(pane);
    }

    /// Everything a web view can show opens in a browser. A link to anything else (mail, an app's
    /// own scheme) goes to whatever on this Mac handles it, but only when the person clicked it in
    /// the page itself: a script, a redirect or a frame never gets to launch an app.
    fn allows(&self, pane: &str, action: &WKNavigationAction) -> bool {
        if self.is_artifact(pane) {
            return self.artifact_allows(pane, action);
        }
        if self.is_embedded(pane) {
            return embedded_allows((unsafe { action.request() }).URL().as_deref());
        }
        if !self.is_browser(pane) {
            return true;
        }
        let Some(target) = (unsafe { action.request() }).URL() else { return false };
        let scheme = target.scheme().map(|s| s.to_string().to_lowercase()).unwrap_or_default();
        if matches!(scheme.as_str(), "http" | "https" | "file" | "about" | "data" | "blob") {
            return true;
        }
        let main = unsafe { action.targetFrame() }.is_none_or(|f: Retained<WKFrameInfo>| unsafe { f.isMainFrame() });
        if unsafe { action.navigationType() } == WKNavigationType::LinkActivated && main {
            self.log(format!("browser pane {pane} handing a clicked {scheme}: link to the system"));
            NSWorkspace::sharedWorkspace().openURL(&target);
        } else {
            self.log(format!("browser pane {pane} refused a {scheme}: navigation nobody clicked"));
        }
        false
    }

    /// An artifact page stays inside its pane's folder; a link the person clicked anywhere else
    /// opens in the default browser, and a frame may build its own inline document.
    fn artifact_allows(&self, pane: &str, action: &WKNavigationAction) -> bool {
        let Some(target) = (unsafe { action.request() }).URL() else { return false };
        let main = unsafe { action.targetFrame() }.is_none_or(|f: Retained<WKFrameInfo>| unsafe { f.isMainFrame() });
        let address = absolute(&target);
        let id = self.artifact_id(pane).unwrap_or_default();
        if artifact_page::allows_inline_frame(&address, main) || artifact_page::in_folder(&address, &id) {
            return true;
        }
        if unsafe { action.navigationType() } == WKNavigationType::LinkActivated {
            self.log(format!("web pane {pane} handing a link outside its folder to the default browser"));
            NSWorkspace::sharedWorkspace().openURL(&target);
        }
        false
    }

    /// A page a browser cannot show is a download, which a pane does not do: a web one goes to
    /// the default browser, and a file says it cannot be shown rather than doing nothing.
    fn shows(&self, pane: &str, response: &WKNavigationResponse) -> bool {
        if !self.is_browser(pane) || !unsafe { response.isForMainFrame() } {
            return true;
        }
        let answer = unsafe { response.response() };
        let status = answer.downcast_ref::<NSHTTPURLResponse>().map(|h| h.statusCode());
        self.with_browser(pane, |page, _| page.status = status);
        let target = answer.URL();
        let (false, Some(target)) = (unsafe { response.canShowMIMEType() }, target) else { return true };
        if target.isFileURL() {
            let path = address_of(&target);
            self.log(format!("browser pane {pane} cannot show {path}, {}", answer.MIMEType().map_or("no type".into(), |m| m.to_string())));
            let message = "A browser pane can’t show this kind of file.".to_owned();
            self.with_browser(pane, |_, b| b.failure = Some(Failure { address: path, target: Some(absolute(&target)), message }));
            self.changed(pane);
        } else {
            self.log(format!("browser pane {pane} handing a download to the default browser: {}", absolute(&target)));
            NSWorkspace::sharedWorkspace().openURL(&target);
        }
        false
    }

    fn finished(&self, pane: &str) {
        let Some(view) = self.view(pane) else { return };
        let title = unsafe { view.title() }.map(|t| t.to_string()).unwrap_or_default();
        if let Some(page) = self.pages.borrow_mut().get_mut(pane) {
            page.loads.push(title.clone());
            if page.browser.is_some() {
                let status = page.status.map_or("-".into(), |s| s.to_string());
                self.log(format!("browser pane {pane} loaded {}, status {status}, title \"{title}\"", page.url));
            }
        }
        let me = self.me.clone();
        let pane = pane.to_owned();
        evaluate_text(&view, "document.body ? document.body.innerText : ''", move |r| {
            if let (Some(me), Ok(text)) = (me.upgrade(), r)
                && let Some(page) = me.pages.borrow_mut().get_mut(&pane)
            {
                page.text = text;
            }
        });
    }

    fn failed(&self, pane: &str, error: &NSError, provisional: bool) {
        if !self.is_browser(pane) {
            if provisional {
                self.errors.borrow_mut().push(error.localizedDescription().to_string());
            }
            return;
        }
        let (domain, code) = (error.domain().to_string(), error.code());
        // A load the person stopped, or replaced with another, is not a failure.
        if (domain == "NSURLErrorDomain" && code == -999) || (domain == "WebKitErrorDomain" && code == 102) {
            return;
        }
        let info = error.userInfo();
        let failing = info
            .objectForKey(&NSString::from_str("NSErrorFailingURLKey"))
            .and_then(|u| u.downcast::<NSURL>().ok())
            .or_else(|| info.objectForKey(&NSString::from_str("NSErrorFailingURLStringKey")).and_then(|s| s.downcast::<NSString>().ok()).and_then(|s| url_of(&s.to_string())));
        let url = self.pages.borrow().get(pane).map(|p| p.url.clone()).unwrap_or_default();
        let address = failing.as_deref().map_or_else(|| url.clone(), address_of);
        let description = error.localizedDescription().to_string();
        self.log(format!("browser pane {pane} failed to load {address}: {domain} {code} {description}"));
        // WebKit words a missing file as a server's 404, which a file has none of.
        let missing_file = failing.as_deref().is_some_and(|f| f.isFileURL()) && domain == "NSURLErrorDomain" && code == -1100;
        let message = if missing_file { "The file couldn’t be opened because there is no such file.".to_owned() } else { description };
        let target = Some(failing.as_deref().map_or(url, absolute));
        self.with_browser(pane, |_, b| b.failure = Some(Failure { address, target, message }));
        self.changed(pane);
    }

    /// A link that asks for a new window opens in its pane: a pane has no tabs. A file opens this
    /// way only from a page that is itself a file, so a site's script cannot steer the pane onto
    /// the disk.
    fn new_window(&self, pane: &str, web: &WKWebView, action: &WKNavigationAction) {
        let request = unsafe { action.request() };
        if self.is_embedded(pane) {
            if embedded_allows(request.URL().as_deref()) {
                unsafe { drop(web.loadRequest(&request)) };
            }
            return;
        }
        let Some(target) = request.URL() else { return };
        if target.isFileURL() {
            let from = unsafe { web.URL() };
            if !from.as_deref().is_some_and(NSURL::isFileURL) {
                self.log(format!("browser pane {pane} refused a new window on a file from {}", from.as_deref().map_or("?".into(), absolute)));
                return;
            }
            self.load(pane, &target);
            self.changed(pane);
        } else {
            unsafe { drop(web.loadRequest(&request)) };
        }
    }

    /// One message from the console script: `reset` for a page load, else a console entry.
    fn console(&self, pane: &str, body: &AnyObject) {
        let Some(text) = json_text(Some(body)) else { return };
        let Ok(body) = serde_json::from_str::<Value>(&text) else { return };
        if body["reset"] == true {
            self.emit(json!({"event": "console", "pane": pane, "reset": true, "entries": []}));
            return;
        }
        let (Some(level), Some(text), Some(time)) = (body["level"].as_str(), body["text"].as_str(), body["time"].as_f64()) else { return };
        let mut entry = json!({"time": time as u64, "level": level, "text": text});
        if let Some(url) = body["url"].as_str() {
            entry["url"] = json!(url);
        }
        if let Some(line) = body["line"].as_f64() {
            entry["line"] = json!(line as u32);
        }
        self.emit(json!({"event": "console", "pane": pane, "reset": false, "entries": [entry]}));
    }

    /// `inspect_pane` `id` on a browser's page, answered with a frame: `where` reads the page it is
    /// on, `open` sends it to a page, and `eval`, `dom`, `shot` and `record` run in the page.
    pub fn inspect(&self, id: u64, pane: &str, action: &str, arg: Option<&str>) {
        let me = self.me.clone();
        let answer = move |value: Value| {
            if let Some(me) = me.upgrade() {
                me.emit(value);
            }
        };
        let text = move |text: String| json!({"answer": "inspected", "id": id, "text": text});
        let refused = move |message: String| json!({"answer": "refused", "id": id, "message": message});
        let (Some(view), true) = (self.view(pane), self.is_browser(pane)) else { return answer(refused("not shown".into())) };
        match action {
            "where" => {
                let pages = self.pages.borrow();
                let (url, title) = pages.get(pane).map(|p| (p.url.clone(), p.browser.as_ref().map(|b| b.title.clone()).unwrap_or_default())).unwrap_or_default();
                drop(pages);
                answer(text(format!("{{\"title\":{},\"url\":{}}}", literal(&title), literal(&url))));
            }
            "open" => match arg.filter(|a| !a.is_empty()) {
                Some(page) => {
                    self.opened(pane, page);
                    answer(text(String::new()));
                }
                None => answer(refused("open needs a page".into())),
            },
            "eval" => evaluate(&view, arg.unwrap_or_default(), move |r| {
                answer(match r {
                    Ok(value) => json_text(value.as_deref()).map_or_else(|| refused("the result is not JSON".into()), text),
                    Err(message) => refused(message),
                });
            }),
            "dom" => {
                let selector = arg.map(literal).unwrap_or_else(|| "null".into());
                let script = format!(
                    "(function (selector) {{ if (selector === null) return \"ok:\" + document.documentElement.outerHTML; var found = document.querySelectorAll(selector); if (!found.length) return \"none\"; return \"ok:\" + Array.prototype.map.call(found, function (e) {{ return e.outerHTML; }}).join(\"\\n\"); }})({selector})"
                );
                let wanted = arg.unwrap_or_default().to_owned();
                evaluate_text(&view, &script, move |r| {
                    answer(match r {
                        Ok(Some(reply)) => reply.strip_prefix("ok:").map_or_else(|| refused(format!("no element matches {wanted}")), |html| text(html.to_owned())),
                        Ok(None) => refused("the page returned no HTML".into()),
                        Err(message) => refused(message),
                    });
                });
            }
            "shot" => {
                let selector = arg.unwrap_or_default().to_owned();
                let script = format!(
                    "(function (selector) {{ var el = document.querySelector(selector); if (!el) return \"none\"; el.scrollIntoView({{ block: \"nearest\", inline: \"nearest\" }}); var r = el.getBoundingClientRect(); return JSON.stringify([r.x, r.y, r.width, r.height]); }})({})",
                    literal(&selector)
                );
                let view2 = view.clone();
                evaluate_text(&view, &script, move |r| {
                    let found = match r {
                        Err(message) => return answer(refused(message)),
                        Ok(found) => found.filter(|t| t != "none"),
                    };
                    let Some(found) = found else { return answer(refused(format!("no element matches {selector}"))) };
                    let Ok([x, y, w, h]) = serde_json::from_str::<[f64; 4]>(&found) else {
                        return answer(refused(format!("the page gave no box for {selector}: {found}")));
                    };
                    let b = view2.bounds();
                    let (x0, y0, x1, y1) = (x.max(0.), y.max(0.), (x + w).min(b.size.width), (y + h).min(b.size.height));
                    if x1 - x0 < 1. || y1 - y0 < 1. {
                        return answer(refused(format!("{selector} has no visible area in the pane")));
                    }
                    let c = unsafe { WKSnapshotConfiguration::new(main_thread()) };
                    unsafe { c.setRect(NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0))) };
                    snapshot_image(&view2, Some(&c), move |r| {
                        answer(match r.and_then(|i| image_png(&i).ok_or_else(|| "the drawing could not be encoded as a PNG".to_owned())) {
                            Ok(png) => json!({"answer": "rendered", "id": id, "png": base64::engine::general_purpose::STANDARD.encode(png)}),
                            Err(message) => refused(message),
                        });
                    });
                });
            }
            "record" => match arg.and_then(|a| a.parse::<f64>().ok()).filter(|s| *s > 0.) {
                Some(seconds) => super::film::record(&view, seconds, move |r| {
                    answer(match r {
                        Ok(film) => json!({"answer": "recorded", "id": id, "mov": base64::engine::general_purpose::STANDARD.encode(film.mov), "frames": film.frames, "millis": film.millis}),
                        Err(message) => refused(format!("recording failed: {message}")),
                    });
                }),
                None => answer(refused("record needs a number of seconds".into())),
            },
            other => answer(refused(format!("unknown inspect op {other}"))),
        }
    }

    /// `render_pane` `id` of the WKWebView `view` (a browser's, a web pane's or an artifact page's), answered with a frame.
    pub fn render(&self, id: u64, view: &WKWebView, full: bool) {
        let me = self.me.clone();
        shot_page(view, full, move |r| {
            let Some(me) = me.upgrade() else { return };
            me.emit(match r {
                Ok(png) => json!({"answer": "rendered", "id": id, "png": base64::engine::general_purpose::STANDARD.encode(png)}),
                Err(message) => json!({"answer": "refused", "id": id, "message": message}),
            });
        });
    }

    /// `render_pane` `id` of a plugin pane: `base` is the PNG of `view` as the bridge drew it, in
    /// which no WKWebView draws, so every WKWebView shown inside `view` is drawn over it here.
    pub fn compose(&self, id: u64, view: &NSView, base: Result<Vec<u8>, String>) {
        let refuse = move |me: &Self, message: String| me.emit(json!({"answer": "refused", "id": id, "message": message}));
        let base = match base {
            Ok(png) => png,
            Err(message) => return refuse(self, message),
        };
        let mut webs = vec![];
        web_views(view, &mut webs);
        let bounds = view.bounds();
        let flipped = view.isFlipped();
        let rects: Vec<NSRect> = webs.iter().map(|w| view.convertRect_fromView(w.bounds(), Some(w))).collect();
        let shots: Rc<RefCell<Vec<Option<Retained<NSImage>>>>> = Rc::new(RefCell::new(vec![None; webs.len()]));
        let left = Rc::new(Cell::new(webs.len()));
        let me = self.me.clone();
        let finish = Rc::new(move |shots: &[Option<Retained<NSImage>>]| {
            let Some(me) = me.upgrade() else { return };
            let Some(rep) = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&base)) else { return refuse(&me, "the bridge's picture could not be read".into()) };
            rep.setSize(bounds.size);
            if let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) {
                NSGraphicsContext::saveGraphicsState_class();
                NSGraphicsContext::setCurrentContext(Some(&context));
                for (rect, image) in rects.iter().zip(shots).filter_map(|(r, s)| s.as_ref().map(|s| (r, s))) {
                    // The bitmap's origin is bottom-left; a flipped view's frames count from the top.
                    let y = if flipped { bounds.size.height - rect.origin.y - rect.size.height } else { rect.origin.y };
                    image.drawInRect(NSRect::new(NSPoint::new(rect.origin.x, y), rect.size));
                }
                NSGraphicsContext::restoreGraphicsState_class();
            }
            // SAFETY: an empty properties dictionary is what the PNG encoder accepts.
            match unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) } {
                Some(png) => me.emit(json!({"answer": "rendered", "id": id, "png": base64::engine::general_purpose::STANDARD.encode(png.to_vec())})),
                None => refuse(&me, "the page could not be encoded as a PNG".into()),
            }
        });
        if webs.is_empty() {
            return finish(&[]);
        }
        for (i, web) in webs.iter().enumerate() {
            let (shots, left, finish) = (shots.clone(), left.clone(), finish.clone());
            snapshot_image(web, None, move |r| {
                shots.borrow_mut()[i] = r.ok();
                left.set(left.get() - 1);
                if left.get() == 0 {
                    finish(&shots.borrow());
                }
            });
        }
    }

    /// The check's snapshot of WKWebView `view` in `pane`, written to `<directory>/<pane>-web.png`
    /// (an embed's key with each `/` of its node id as `_`).
    pub fn snapshot_to(&self, pane: &str, view: &WKWebView, directory: &str) {
        let path = format!("{directory}/{}-web.png", pane.replace('/', "_"));
        let blank = unsafe { view.URL() }.is_none_or(|u| absolute(&u) == "about:blank");
        if blank {
            // A view showing no page has nothing for WebKit to snapshot, and its snapshot can go unanswered: AppKit draws it instead.
            if let Some(png) = cached_png(view) {
                let _ = std::fs::write(&path, png);
            }
            return;
        }
        self.waiting.borrow_mut().insert(pane.to_owned());
        let me = self.me.clone();
        let pane = pane.to_owned();
        snapshot_image(view, None, move |r| {
            let Some(me) = me.upgrade() else { return };
            match r {
                Ok(image) => {
                    if let Some(png) = image_png(&image) {
                        let _ = std::fs::write(&path, png);
                    }
                }
                Err(message) => me.errors.borrow_mut().push(message),
            }
            me.waiting.borrow_mut().remove(&pane);
        });
    }

    pub fn clear_waiting(&self) {
        self.waiting.borrow_mut().clear();
    }

    /// Each page's entry for the bridge's `panes` status (what the check reads), and the window's
    /// own: the snapshots still waiting, the browsers' log lines and the pages' errors.
    pub fn report(&self, first_responder: Option<&NSView>) -> (Map<String, Value>, Value) {
        let pages = self.pages.borrow();
        let panes = pages
            .iter()
            .map(|(id, p)| {
                let view: &NSView = &p.view;
                let focused = first_responder.is_some_and(|r| std::ptr::eq(r, view) || r.isDescendantOf(view));
                let mut entry = json!({"kind": if p.browser.is_some() { "browser" } else { "web" }, "focused": focused,
                    "view_identity": format!("{:p}", Retained::as_ptr(&p.view)), "url": p.url, "loads": p.loads, "reloads": p.reloads,
                    "status": p.status, "text": p.text.clone().unwrap_or_default()});
                if let Some(a) = &p.artifact {
                    entry["store"] = json!(a.store);
                    entry["page"] = json!(a.id);
                }
                if p.embedded {
                    entry["embedded"] = json!(true);
                }
                if let Some(b) = &p.browser {
                    entry["title"] = json!(b.title);
                    entry["loading"] = json!(b.loading);
                    entry["can_go_back"] = json!(b.can_go_back);
                    entry["can_go_forward"] = json!(b.can_go_forward);
                    entry["failure"] = b.failure.as_ref().map_or(Value::Null, |f| json!({"address": f.address, "message": f.message}));
                }
                (id.clone(), entry)
            })
            .collect();
        let waiting: Vec<String> = self.waiting.borrow().iter().cloned().collect();
        let window = json!({"snapshots_pending": waiting.len(), "snapshots_waiting": waiting, "logs": self.logs.borrow().iter().collect::<Vec<_>>(), "errors": *self.errors.borrow()});
        (panes, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_script_is_read_out_of_the_swift_source() {
        let script = console_script();
        assert!(script.starts_with("(function () {\n  if (window.__wheelConsole) return;"), "{script}");
        assert!(script.ends_with("})();"), "{script}");
        assert!(script.contains("window.webkit.messageHandlers.wheelConsole.postMessage(message)"));
        // The Swift literal's closing-delimiter indentation is gone: the outermost lines start at column 0.
        assert_eq!((script.lines().next(), script.lines().last()), (Some("(function () {"), Some("})();")), "{script}");
    }

    #[test]
    fn an_embedded_page_goes_only_to_web_and_about() {
        assert!(embedded_scheme("http") && embedded_scheme("HTTPS") && embedded_scheme("about"));
        assert!(!embedded_scheme("file") && !embedded_scheme("wheel-page") && !embedded_scheme("mailto"));
    }

    #[test]
    fn literals_quote_as_javascript() {
        assert_eq!(literal("a\"b\n"), "\"a\\\"b\\n\"");
    }
}
