//! The `wheel-page` scheme an artifact's page is opened under (ADR-0048): each request the page
//! makes is fetched from wheeld with the token in a header, for the pane's own folder only, and
//! handed to WebKit as the scheme task's answer. The page's origin is the scheme and a fixed host,
//! so its storage stays with it when wheeld comes back on another port.
use super::main_thread;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message as _, define_class, msg_send,
    rc::{Retained, Weak as ObjcWeak},
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSInputStream, NSMutableURLRequest, NSOperationQueue, NSString, NSURL, NSURLRequestCachePolicy,
    NSURLResponse, NSURLSession, NSURLSessionConfiguration, NSURLSessionDataDelegate, NSURLSessionDataTask, NSURLSessionDelegate,
    NSURLSessionResponseDisposition, NSURLSessionTask, NSURLSessionTaskDelegate,
};
use objc2_web_kit::{WKURLSchemeHandler, WKURLSchemeTask, WKWebView};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Weak,
};
use wheel_gpui_core::artifact_page;

use super::web::Pages;

/// Page requests in flight across every pane. wheeld's remote listener refuses a client its 16th
/// open connection, and a page opens one per file; the event streams never count, since they stay open.
const LIMIT: usize = 4;

type Task = Retained<ProtocolObject<dyn WKURLSchemeTask>>;

/// The requests let through and the ones waiting their turn, shared by every pane.
struct Gate {
    free: usize,
    waiting: VecDeque<(ObjcWeak<PageScheme>, usize)>,
}
thread_local! {
    static GATE: RefCell<Gate> = const { RefCell::new(Gate { free: LIMIT, waiting: VecDeque::new() }) };
    /// Every live handler by the id its session's delegate carries.
    static SCHEMES: RefCell<HashMap<usize, ObjcWeak<PageScheme>>> = RefCell::default();
}
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

/// Hand the place a finished request held to the next one still waiting, or free it.
fn leave() {
    loop {
        let next = GATE.with_borrow_mut(|g| {
            let next = g.waiting.pop_front();
            if next.is_none() {
                g.free += 1;
            }
            next
        });
        let Some((handler, key)) = next else { return };
        if handler.load().is_some_and(|h| h.begin(key)) {
            return;
        }
    }
}

/// One request the page made.
struct Request {
    task: Task,
    /// The page address WebKit asked for, the answer's URL.
    page: Retained<NSURL>,
    /// What goes to wheeld once the request has its turn.
    sent: Retained<NSMutableURLRequest>,
    /// The page's event stream (or any `text/event-stream`), handed over as it arrives and never gated.
    streaming: bool,
    /// The main frame's document, whose status the pane reports.
    document: bool,
    /// The session task fetching it, once started; a gated one holds a place until it ends.
    fetch: Option<Retained<NSURLSessionDataTask>>,
    answer: Option<Retained<NSURLResponse>>,
    /// A whole body held until it ends, or a stream's bytes after its last newline.
    held: Vec<u8>,
}

pub struct SchemeIvars {
    id: usize,
    pane: String,
    pages: Weak<Pages>,
    session: RefCell<Option<Retained<NSURLSession>>>,
    requests: RefCell<HashMap<usize, Request>>,
    /// Each session task's request, by its task identifier.
    fetches: RefCell<HashMap<usize, usize>>,
    /// `close` ran: the web view is going away, and a request it still starts is refused.
    closed: Cell<bool>,
}

define_class!(
    /// One artifact pane's `wheel-page` handler and the URLSession delegate that fetches its requests.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelPageScheme"]
    #[ivars = SchemeIvars]
    pub struct PageScheme;

    unsafe impl NSObjectProtocol for PageScheme {}

    unsafe impl WKURLSchemeHandler for PageScheme {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.start(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.stop(key_of(task));
        }
    }
);

define_class!(
    /// A handler's URLSession delegate. URLSession wants a delegate any thread may hold, so it carries
    /// only its handler's id; its session calls it on the main queue, where the handler lives.
    #[unsafe(super(NSObject))]
    #[name = "WheelPageFetch"]
    #[ivars = usize]
    struct PageFetch;

    unsafe impl NSObjectProtocol for PageFetch {}
    unsafe impl NSURLSessionDelegate for PageFetch {}
    unsafe impl NSURLSessionTaskDelegate for PageFetch {
        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn did_complete(&self, _session: &NSURLSession, fetch: &NSURLSessionTask, error: Option<&NSError>) {
            if let Some(h) = self.handler() {
                h.completed(fetch.taskIdentifier(), error);
            }
        }
    }
    unsafe impl NSURLSessionDataDelegate for PageFetch {
        #[unsafe(method(URLSession:dataTask:didReceiveResponse:completionHandler:))]
        fn did_receive_response(&self, _session: &NSURLSession, fetch: &NSURLSessionDataTask, response: &NSURLResponse, handler: &block2::DynBlock<dyn Fn(NSURLSessionResponseDisposition)>) {
            handler.call((NSURLSessionResponseDisposition::Allow,));
            if let Some(h) = self.handler() {
                h.answered(fetch.taskIdentifier(), response);
            }
        }

        #[unsafe(method(URLSession:dataTask:didReceiveData:))]
        fn did_receive_data(&self, _session: &NSURLSession, fetch: &NSURLSessionDataTask, data: &NSData) {
            if let Some(h) = self.handler() {
                h.received(fetch.taskIdentifier(), &data.to_vec());
            }
        }
    }
);

impl PageFetch {
    fn new(id: usize) -> Retained<Self> {
        let this = Self::alloc().set_ivars(id);
        unsafe { msg_send![super(this), init] }
    }

    /// The handler, on the main queue its session calls back on; None anywhere else or once it is gone.
    fn handler(&self) -> Option<Retained<PageScheme>> {
        MainThreadMarker::new()?;
        SCHEMES.with_borrow(|m| m.get(self.ivars()).and_then(ObjcWeak::load))
    }
}

fn key_of(task: &ProtocolObject<dyn WKURLSchemeTask>) -> usize {
    std::ptr::from_ref(task) as *const () as usize
}

/// Headers the page's own request carries that wheeld must not see: any `Origin` makes it refuse
/// the request as a browser's, and the token goes in its own header.
const DROPPED: [&str; 5] = ["origin", "referer", "host", "cookie", "authorization"];

/// An `NSURLErrorDomain` error with `code` for a request refused here.
fn url_error(code: isize) -> Retained<NSError> {
    unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str("NSURLErrorDomain"), code, None) }
}

/// The address without its query, so no log line holds a token or a page's parameters.
fn shown(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// A request body WebKit hands over as a stream.
fn drain(stream: &NSInputStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = [0u8; 16384];
    stream.open();
    while stream.hasBytesAvailable() {
        let n = unsafe { stream.read_maxLength(std::ptr::NonNull::new(buffer.as_mut_ptr()).unwrap(), buffer.len()) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buffer[..n as usize]);
    }
    stream.close();
    out
}

impl PageScheme {
    pub fn new(pane: &str, pages: Weak<Pages>) -> Retained<Self> {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let this = main_thread().alloc::<Self>().set_ivars(SchemeIvars {
            id,
            pane: pane.to_owned(),
            pages,
            session: RefCell::default(),
            requests: RefCell::default(),
            fetches: RefCell::default(),
            closed: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        SCHEMES.with_borrow_mut(|m| m.insert(id, ObjcWeak::from_retained(&this)));
        this
    }

    fn log(&self, line: String) {
        if let Some(p) = self.ivars().pages.upgrade() {
            p.log(line);
        }
    }

    /// The session every request of this pane goes over: no cookie jar and no cache, since the token
    /// travels in a header and a reload after a file changed must reach wheeld. Its delegate calls
    /// come on the main queue.
    fn session(&self) -> Retained<NSURLSession> {
        if let Some(s) = self.ivars().session.borrow().clone() {
            return s;
        }
        let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
        config.setHTTPCookieStorage(None);
        config.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
        let session = unsafe {
            NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                &config,
                Some(ProtocolObject::from_ref(&*PageFetch::new(self.ivars().id))),
                Some(&NSOperationQueue::mainQueue()),
            )
        };
        *self.ivars().session.borrow_mut() = Some(session.clone());
        session
    }

    /// Ends every request this handler answers and its session, for a web view going away: WebKit
    /// stops a task only when the page itself does, so a removed view's event stream would stay
    /// open to wheeld for as long as the app runs.
    pub fn close(&self) {
        self.ivars().closed.set(true);
        let keys: Vec<usize> = self.ivars().requests.borrow().keys().copied().collect();
        for key in keys {
            self.stop(key);
        }
        if let Some(session) = self.ivars().session.borrow_mut().take() {
            session.invalidateAndCancel();
        }
        SCHEMES.with_borrow_mut(|m| m.remove(&self.ivars().id));
    }

    fn start(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        let pane = &self.ivars().pane;
        if self.ivars().closed.get() {
            unsafe { task.didFailWithError(&url_error(-999)) };
            return;
        }
        let request = unsafe { task.request() };
        let Some(page) = request.URL() else { return unsafe { task.didFailWithError(&url_error(-1000)) } };
        let address = page.absoluteString().map(|s| s.to_string()).unwrap_or_default();
        let Some((ws, token)) = self.ivars().pages.upgrade().and_then(|p| p.daemon()) else {
            self.log(format!("web pane {pane} request refused: no daemon address or token yet"));
            return unsafe { task.didFailWithError(&url_error(-1004)) };
        };
        // An artifact's page reaches its own folder; nothing else the token opens.
        let Some(target) = artifact_page::daemon_url(&address, &ws, pane).and_then(|t| NSURL::URLWithString(&NSString::from_str(&t))) else {
            self.log(format!("web pane {pane} request refused outside its folder: {}", shown(&address)));
            return unsafe { task.didFailWithError(&url_error(-1102)) };
        };
        let sent = NSMutableURLRequest::requestWithURL(&target);
        if let Some(method) = request.HTTPMethod() {
            sent.setHTTPMethod(&method);
        }
        if let Some(body) = request.HTTPBody() {
            sent.setHTTPBody(Some(&body));
        } else if let Some(stream) = request.HTTPBodyStream() {
            sent.setHTTPBody(Some(&NSData::with_bytes(&drain(&stream))));
        }
        let mut accept = String::new();
        if let Some(headers) = request.allHTTPHeaderFields() {
            let (names, values) = headers.to_vecs();
            for (name, value) in names.into_iter().zip(values) {
                let lower = name.to_string().to_lowercase();
                if lower == "accept" {
                    accept = value.to_string();
                }
                if !DROPPED.contains(&lower.as_str()) {
                    sent.setValue_forHTTPHeaderField(Some(&value), &name);
                }
            }
        }
        sent.setValue_forHTTPHeaderField(Some(&NSString::from_str(&format!("Bearer {token}"))), &NSString::from_str("Authorization"));
        let path = target.path().map(|p| p.to_string()).unwrap_or_default();
        let streaming = path.ends_with("/.wheel/events") || accept.contains("text/event-stream");
        let document = request.mainDocumentURL().is_some_and(|d| d.isEqual(Some(&page)));
        let key = key_of(task);
        self.ivars()
            .requests
            .borrow_mut()
            .insert(key, Request { task: task.retain(), page, sent, streaming, document, fetch: None, answer: None, held: vec![] });
        if streaming {
            self.begin(key);
            return;
        }
        let now = GATE.with_borrow_mut(|g| {
            if g.free > 0 {
                g.free -= 1;
                true
            } else {
                g.waiting.push_back((ObjcWeak::from_retained(&self.retain()), key));
                false
            }
        });
        if now {
            self.begin(key);
        }
    }

    /// Send request `key` to wheeld; false when it was stopped while it waited, so its place passes on.
    fn begin(&self, key: usize) -> bool {
        if self.ivars().closed.get() || !self.ivars().requests.borrow().contains_key(&key) {
            return false;
        }
        let session = self.session();
        let mut requests = self.ivars().requests.borrow_mut();
        let Some(r) = requests.get_mut(&key) else { return false };
        let fetch = session.dataTaskWithRequest(&r.sent);
        self.ivars().fetches.borrow_mut().insert(fetch.taskIdentifier(), key);
        self.log(format!("web pane {} page {} requested over URLSession", self.ivars().pane, shown(&r.page.absoluteString().map(|s| s.to_string()).unwrap_or_default())));
        fetch.resume();
        r.fetch = Some(fetch);
        true
    }

    /// WebKit stopped task `key` (or the view is closing): its fetch ends, and a gated one's place passes on.
    fn stop(&self, key: usize) {
        let Some(r) = self.ivars().requests.borrow_mut().remove(&key) else { return };
        // A request still waiting its turn leaves the line, so a later task at the same address is not started in its place.
        let me = self as *const Self;
        GATE.with_borrow_mut(|g| g.waiting.retain(|(h, k)| !(*k == key && h.load().is_some_and(|h| std::ptr::eq(&*h, me)))));
        if let Some(fetch) = &r.fetch {
            self.ivars().fetches.borrow_mut().remove(&fetch.taskIdentifier());
            fetch.cancel();
            if !r.streaming {
                leave();
            }
        }
    }

    fn request_of(&self, fetch: usize) -> Option<usize> {
        self.ivars().fetches.borrow().get(&fetch).copied()
    }

    /// The answer WebKit gets: wheeld's status and headers under the page's own address, the body
    /// inflated and unframed, so its encoding and framing no longer apply.
    fn page_answer(r: &Request, length: Option<usize>) -> Option<Retained<NSHTTPURLResponse>> {
        let http = r.answer.as_ref()?.downcast_ref::<NSHTTPURLResponse>()?;
        let (all_names, all_values) = http.allHeaderFields().to_vecs();
        let mut names = vec![];
        let mut values = vec![];
        for (name, value) in all_names.iter().zip(&all_values) {
            let (Some(name), Some(value)) = (name.downcast_ref::<NSString>(), value.downcast_ref::<NSString>()) else { continue };
            if ["content-encoding", "transfer-encoding", "connection", "content-length"].contains(&name.to_string().to_lowercase().as_str()) {
                continue;
            }
            names.push(name.retain());
            values.push(value.retain());
        }
        if let Some(n) = length {
            names.push(NSString::from_str("Content-Length"));
            values.push(NSString::from_str(&n.to_string()));
        }
        let keys: Vec<&NSString> = names.iter().map(|n| &**n).collect();
        let headers = NSDictionary::from_retained_objects(&keys, &values);
        NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(NSHTTPURLResponse::alloc(), &r.page, http.statusCode(), Some(&NSString::from_str("HTTP/1.1")), Some(&headers))
    }

    fn answered(&self, fetch: usize, response: &NSURLResponse) {
        let Some(key) = self.request_of(fetch) else { return };
        let mut requests = self.ivars().requests.borrow_mut();
        let Some(r) = requests.get_mut(&key) else { return };
        r.answer = Some(response.retain());
        let status = response.downcast_ref::<NSHTTPURLResponse>().map(|h| h.statusCode());
        if r.document
            && let (Some(code), Some(pages)) = (status, self.ivars().pages.upgrade())
        {
            pages.document_status(&self.ivars().pane, code);
        }
        if r.streaming {
            match Self::page_answer(r, None) {
                Some(answer) => unsafe { r.task.didReceiveResponse(&answer) },
                None => {
                    let r = requests.remove(&key).unwrap();
                    drop(requests);
                    self.ivars().fetches.borrow_mut().remove(&fetch);
                    if let Some(f) = &r.fetch {
                        f.cancel();
                    }
                    unsafe { r.task.didFailWithError(&url_error(-1011)) };
                    return;
                }
            }
            self.log(format!("web pane {} page {} -> {}, streaming", self.ivars().pane, shown(&r.page.absoluteString().map(|s| s.to_string()).unwrap_or_default()), status.unwrap_or(0)));
        }
    }

    fn received(&self, fetch: usize, data: &[u8]) {
        let Some(key) = self.request_of(fetch) else { return };
        let mut requests = self.ivars().requests.borrow_mut();
        let Some(r) = requests.get_mut(&key) else { return };
        r.held.extend_from_slice(data);
        // A stream is flushed at its last newline, so a page never sees half of a line.
        if r.streaming
            && let Some(last) = r.held.iter().rposition(|b| *b == b'\n')
        {
            let lines: Vec<u8> = r.held.drain(..=last).collect();
            unsafe { r.task.didReceiveData(&NSData::with_bytes(&lines)) };
        }
    }

    fn completed(&self, fetch: usize, error: Option<&NSError>) {
        let Some(key) = self.ivars().fetches.borrow_mut().remove(&fetch) else { return };
        let Some(r) = self.ivars().requests.borrow_mut().remove(&key) else { return };
        if !r.streaming {
            leave();
        }
        let pane = &self.ivars().pane;
        let address = r.page.absoluteString().map(|s| s.to_string()).unwrap_or_default();
        if let Some(error) = error {
            self.log(format!("web pane {pane} page {} failed: {} {}", shown(&address), error.domain(), error.code()));
            unsafe { r.task.didFailWithError(error) };
            return;
        }
        unsafe {
            if r.streaming {
                if !r.held.is_empty() {
                    r.task.didReceiveData(&NSData::with_bytes(&r.held));
                }
            } else {
                let Some(answer) = Self::page_answer(&r, Some(r.held.len())) else { return r.task.didFailWithError(&url_error(-1011)) };
                self.log(format!("web pane {pane} page {} -> {}, {} bytes", shown(&address), answer.statusCode(), r.held.len()));
                r.task.didReceiveResponse(&answer);
                r.task.didReceiveData(&NSData::with_bytes(&r.held));
            }
            r.task.didFinish();
        }
    }
}
