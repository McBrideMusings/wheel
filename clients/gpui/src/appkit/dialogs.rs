//! Context menus, alerts and the folder picker (the Swift bridge's AppNative and Open Folder).
//! Each opens from the run loop after the call that asked returns, so AppKit's tracking and modal
//! loops never run inside the client's update; an answer arrives as an [`Answer`] on the channel
//! `Dialogs::new` is given. A hidden app opens nothing: a menu is only built, and an alert is kept
//! for the check to answer.
use block2::RcBlock;
use futures::channel::{mpsc::UnboundedSender, oneshot};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::{AnyObject, NSObject}, sel};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSEvent, NSMenu, NSMenuItem, NSModalResponseOK, NSOpenPanel};
use objc2_foundation::NSString;
use serde_json::{Value, json};
use std::cell::RefCell;

/// A pick from a context menu (the item's id), or an alert answered with the index of its button.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Menu(String),
    Alert { id: String, button: i64 },
}

/// One entry of a context menu: `id` "-" is a separator.
pub struct MenuItem<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub enabled: bool,
}

/// A question with its buttons, the first the default; `id` comes back with the answer.
#[derive(Debug, Clone)]
pub struct Alert {
    pub id: String,
    pub message: String,
    pub informative: String,
    pub buttons: Vec<String>,
}

pub struct Ivars {
    answers: UnboundedSender<Answer>,
    /// The open menu's ids by item tag, the menu waiting to pop up, and the alerts waiting to run,
    /// each once, in the order asked.
    ids: RefCell<Vec<String>>,
    menu: RefCell<Option<Retained<NSMenu>>>,
    alerts: RefCell<std::collections::VecDeque<(Alert, bool)>>,
}

define_class!(
    /// The menus' target and the run loop's callee: it turns AppKit's answers into [`Answer`]s.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelDialogsTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(chose:))]
        fn chose(&self, sender: &NSMenuItem) {
            let id = self.ivars().ids.borrow().get(sender.tag() as usize).cloned().unwrap_or_default();
            let _ = self.ivars().answers.unbounded_send(Answer::Menu(id));
        }

        /// Pop the waiting menu up at the pointer, as SwiftUI's `.contextMenu` opens one.
        #[unsafe(method(openMenu:))]
        fn open_menu(&self, _sender: Option<&AnyObject>) {
            let Some(menu) = self.ivars().menu.borrow_mut().take() else { return };
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSEvent::mouseLocation(), None);
        }

        #[unsafe(method(runAlert:))]
        fn run_alert(&self, _sender: Option<&AnyObject>) {
            let Some((asked, activate)) = self.ivars().alerts.borrow_mut().pop_front() else { return };
            let mtm = self.mtm();
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(&asked.message));
            alert.setInformativeText(&NSString::from_str(&asked.informative));
            alert.setAlertStyle(NSAlertStyle::Warning);
            for title in &asked.buttons {
                alert.addButtonWithTitle(&NSString::from_str(title));
            }
            if activate {
                NSApplication::sharedApplication(mtm).activate();
            }
            let button = (alert.runModal() - NSAlertFirstButtonReturn) as i64;
            let _ = self.ivars().answers.unbounded_send(Answer::Alert { id: asked.id, button });
        }
    }
);

impl Target {
    /// Run `selector` from the run loop once the current call returns.
    fn later(&self, selector: objc2::runtime::Sel) {
        // SAFETY: `selector` is one of the target's methods taking an optional sender; a zero
        // delay runs it from the run loop.
        unsafe {
            let _: () = msg_send![self, performSelector: selector, withObject: None::<&AnyObject>, afterDelay: 0f64];
        }
    }
}

pub struct Dialogs {
    target: Retained<Target>,
    hidden: bool,
    /// The context menu as last built, each item's title and whether it is enabled ("-" a separator).
    built: Vec<Value>,
    /// The question a hidden app keeps for the check.
    pending: Option<Alert>,
}

impl Dialogs {
    pub fn new(hidden: bool, answers: UnboundedSender<Answer>) -> Self {
        let mtm = super::main_thread();
        let ivars = Ivars { answers, ids: RefCell::default(), menu: RefCell::default(), alerts: RefCell::default() };
        let target: Retained<Target> = unsafe { msg_send![super(Target::alloc(mtm).set_ivars(ivars)), init] };
        Self { target, hidden, built: vec![], pending: None }
    }

    /// A context menu of `items`, opened at the pointer after this call returns; a pick arrives as
    /// [`Answer::Menu`]. Returns the menu as built, for the check.
    pub fn context_menu(&mut self, items: &[MenuItem]) -> Vec<Value> {
        let mtm = super::main_thread();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let mut ids = vec![];
        for entry in items {
            if entry.id == "-" {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            }
            // SAFETY: `chose:` is the target's method, taking the menu item that sent it.
            let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(entry.title), Some(sel!(chose:)), &NSString::new()) };
            // SAFETY: the target lives as long as this struct, which the app holds for its life.
            unsafe { item.setTarget(Some(&self.target)) };
            item.setTag(ids.len() as isize);
            item.setEnabled(entry.enabled);
            ids.push(entry.id.to_owned());
            menu.addItem(&item);
        }
        *self.target.ivars().ids.borrow_mut() = ids;
        self.built = menu
            .itemArray()
            .iter()
            .map(|i| if i.isSeparatorItem() { json!({"title": "-", "enabled": false}) } else { json!({"title": i.title().to_string(), "enabled": i.isEnabled()}) })
            .collect();
        if !self.hidden {
            *self.target.ivars().menu.borrow_mut() = Some(menu);
            self.target.later(sel!(openMenu:));
        }
        self.built.clone()
    }

    /// Pick `id` from the context menu as its action does (the check's stand-in for the person).
    pub fn choose(&self, id: &str) {
        let _ = self.target.ivars().answers.unbounded_send(Answer::Menu(id.to_owned()));
    }

    /// Ask `alert` after this call returns, bringing the app front when `activate`; the answer
    /// arrives as [`Answer::Alert`]. A hidden app keeps the question for [`Dialogs::answer`].
    pub fn alert(&mut self, alert: Alert, activate: bool) {
        if self.hidden {
            self.pending = Some(alert);
            return;
        }
        self.target.ivars().alerts.borrow_mut().push_back((alert, activate));
        self.target.later(sel!(runAlert:));
    }

    /// Answer the kept question with `button`, as a press on it does (the check's stand-in).
    pub fn answer(&mut self, button: i64) {
        if let Some(asked) = self.pending.take() {
            let _ = self.target.ivars().answers.unbounded_send(Answer::Alert { id: asked.id, button });
        }
    }

    /// The menu as last built and the kept question, as the check reads them.
    pub fn state(&self) -> Value {
        let alert = self.pending.as_ref().map(|a| json!({"id": a.id, "message": a.message, "informative": a.informative, "buttons": a.buttons}));
        json!({"context_menu": self.built, "alert": alert})
    }
}

/// The system folder picker; the folder chosen arrives on the returned channel, which closes when
/// the person cancels.
pub fn choose_folder() -> oneshot::Receiver<String> {
    let (tx, rx) = oneshot::channel();
    let panel = NSOpenPanel::openPanel(super::main_thread());
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    let tx = RefCell::new(Some(tx));
    let held = panel.clone();
    let done = RcBlock::new(move |response| {
        let path = (response == NSModalResponseOK).then(|| held.URL().and_then(|u| u.path())).flatten();
        if let (Some(path), Some(tx)) = (path, tx.borrow_mut().take()) {
            let _ = tx.send(path.to_string());
        }
    });
    panel.beginWithCompletionHandler(&done);
    rx
}
