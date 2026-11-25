//! The menu bar item and the Dock tile's badge (the Swift app's MenuBarExtra and MenuBarIcon.swift).
//! The item lives while the app runs, with or without a window. A press on it, left or right, and a
//! pick from its right-click menu arrive as an [`ItemEvent`] on the channel `new` is given. In the
//! hidden check's test mode the item exists but is never in the menu bar, its hidden state is saved
//! under its own name, and the Dock tile is left alone.
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::{AnyObject, Bool, NSObject}, sel};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSEventMask, NSEventType, NSFont, NSFontAttributeName, NSFontWeightBold,
    NSFontWeightRegular, NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSMenu, NSMenuItem, NSScreen, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSAttributedStringNSStringDrawing as _,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};

/// What the item hears: a press with the item's frame on the main screen ([x, y, w, h] from that
/// screen's top left, none when it has no window), or the id of the right-click menu item picked.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemEvent {
    Press { right: bool, frame: Option<[f32; 4]> },
    Menu(String),
}

/// One entry of the right-click menu: `id` "-" is a separator, `key` the key equivalent.
pub struct MenuEntry<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub key: &'a str,
}

pub struct Ivars {
    events: UnboundedSender<ItemEvent>,
    item: Retained<NSStatusItem>,
    /// The right-click menu's ids by item tag, and the menu waiting to open under the item.
    ids: RefCell<Vec<String>>,
    menu: RefCell<Option<Retained<NSMenu>>>,
    clicks: Cell<u32>,
    right_clicks: Cell<u32>,
}

define_class!(
    /// The button's and the menu's target: it turns AppKit's actions into [`ItemEvent`]s.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelStatusItemTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(pressed:))]
        fn pressed(&self, _sender: Option<&AnyObject>) {
            let right = NSApplication::sharedApplication(self.mtm()).currentEvent().is_some_and(|e| matches!(e.r#type(), NSEventType::RightMouseDown | NSEventType::RightMouseUp));
            self.press(right);
        }

        #[unsafe(method(chose:))]
        fn chose(&self, sender: &NSMenuItem) {
            let id = self.ivars().ids.borrow().get(sender.tag() as usize).cloned().unwrap_or_default();
            let _ = self.ivars().events.unbounded_send(ItemEvent::Menu(id));
        }

        /// Open the waiting menu under the item: from the run loop, after the call that built it
        /// returned, so the menu's tracking loop never runs inside the client's update.
        #[unsafe(method(openMenu:))]
        fn open_menu(&self, _sender: Option<&AnyObject>) {
            let Some(menu) = self.ivars().menu.borrow_mut().take() else { return };
            let item = &self.ivars().item;
            let Some(button) = item.button(self.mtm()) else { return };
            item.setMenu(Some(&menu));
            // SAFETY: a click with no sender, as a person's press sends.
            unsafe { button.performClick(None) };
            item.setMenu(None);
        }
    }
);

impl Target {
    fn press(&self, right: bool) {
        let ivars = self.ivars();
        let count = if right { &ivars.right_clicks } else { &ivars.clicks };
        count.set(count.get() + 1);
        let _ = ivars.events.unbounded_send(ItemEvent::Press { right, frame: frame(&ivars.item, self.mtm()) });
    }
}

/// Where the item's button is on the main screen, in points from that screen's top left: the
/// popover opens under it.
fn frame(item: &NSStatusItem, mtm: MainThreadMarker) -> Option<[f32; 4]> {
    let window = item.button(mtm)?.window()?;
    let screen = NSScreen::screens(mtm).firstObject()?;
    let (f, s) = (window.frame(), screen.frame());
    Some([(f.origin.x - s.origin.x) as f32, (s.origin.y + s.size.height - f.origin.y - f.size.height) as f32, f.size.width as f32, f.size.height as f32])
}

/// What the item was last told, for the check.
#[derive(Default)]
struct Shown {
    badge: Option<(usize, bool)>,
    label: String,
    dock: Option<String>,
    menu: Vec<String>,
}

pub struct StatusItem {
    target: Retained<Target>,
    hidden: bool,
    shown: Shown,
}

impl StatusItem {
    pub fn new(hidden: bool, events: UnboundedSender<ItemEvent>) -> Self {
        let mtm = super::main_thread();
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        // The check's item saves its hidden state under its own name, never the app's.
        if hidden {
            item.setAutosaveName(Some(&NSString::from_str("wheel-gpui-check")));
        }
        item.setVisible(!hidden);
        let ivars = Ivars { events, item: item.clone(), ids: RefCell::default(), menu: RefCell::default(), clicks: Cell::new(0), right_clicks: Cell::new(0) };
        let target: Retained<Target> = unsafe { msg_send![super(Target::alloc(mtm).set_ivars(ivars)), init] };
        if let Some(button) = item.button(mtm) {
            // SAFETY: the target lives as long as the item: both are held by this struct for the app's life.
            unsafe {
                button.setTarget(Some(&target));
                button.setAction(Some(sel!(pressed:)));
            }
            // A right press sends the action too, so it can open the menu.
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseDown);
        }
        Self { target, hidden, shown: Shown::default() }
    }

    fn item(&self) -> &NSStatusItem {
        &self.target.ivars().item
    }

    /// Draw the item: its image counts `badge` spaces no window shows with the update dot while a
    /// restart waits, `label` is what VoiceOver reads, and `dock` the Dock tile's badge (none at zero).
    pub fn show(&mut self, badge: usize, label: &str, dock: Option<&str>, update: bool) {
        let mtm = super::main_thread();
        let button = self.item().button(mtm);
        if self.shown.badge != Some((badge, update)) {
            self.shown.badge = Some((badge, update));
            if let Some(button) = &button {
                button.setImage(Some(&icon(badge, update)));
            }
        }
        if let Some(button) = &button {
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
        }
        self.shown.label = label.to_owned();
        if !self.hidden && self.shown.dock.as_deref() != dock {
            NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(dock.map(NSString::from_str).as_deref());
        }
        self.shown.dock = dock.map(str::to_owned);
    }

    /// Build the right-click menu from `entries` and open it under the item once the current call
    /// returns; a pick arrives as [`ItemEvent::Menu`]. A hidden item builds it and opens nothing.
    /// The menu's titles, a separator as "-".
    pub fn open_menu(&mut self, entries: &[MenuEntry]) -> Vec<String> {
        let mtm = super::main_thread();
        let menu = NSMenu::new(mtm);
        let mut ids = vec![];
        for entry in entries {
            if entry.id == "-" {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            }
            // SAFETY: `chose:` is the target's method, taking the menu item that sent it.
            let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(entry.title), Some(sel!(chose:)), &NSString::from_str(entry.key)) };
            unsafe { item.setTarget(Some(&self.target)) };
            item.setTag(ids.len() as isize);
            ids.push(entry.id.to_owned());
            menu.addItem(&item);
        }
        *self.target.ivars().ids.borrow_mut() = ids;
        self.shown.menu = menu.itemArray().iter().map(|i| if i.isSeparatorItem() { "-".to_owned() } else { i.title().to_string() }).collect();
        if !self.hidden {
            *self.target.ivars().menu.borrow_mut() = Some(menu);
            // SAFETY: `openMenu:` is the target's method; a zero delay runs it from the run loop.
            unsafe {
                let _: () = msg_send![&*self.target, performSelector: sel!(openMenu:), withObject: None::<&AnyObject>, afterDelay: 0f64];
            }
        }
        self.shown.menu.clone()
    }

    /// Press the item as a click does (the check's item is never in the menu bar, so it has no
    /// window to click in): the press arrives on the channel like a person's.
    pub fn press(&self, right: bool) {
        self.target.press(right);
    }

    /// Pick an item of the right-click menu as the menu's action does.
    pub fn choose(&self, id: &str) {
        let _ = self.target.ivars().events.unbounded_send(ItemEvent::Menu(id.to_owned()));
    }

    /// Whether the pointer is on the item: a press there toggles the popover itself.
    pub fn pointer_on(&self) -> bool {
        let mtm = super::main_thread();
        let at = NSEvent::mouseLocation();
        let on = self.item().button(mtm).and_then(|b| b.window()).is_some_and(|w| {
            let f = w.frame();
            at.x >= f.origin.x && at.x < f.origin.x + f.size.width && at.y >= f.origin.y && at.y < f.origin.y + f.size.height
        });
        on && self.item().isVisible()
    }

    /// The item as the check reads it.
    pub fn state(&self) -> Value {
        let ivars = self.target.ivars();
        let image = self.item().button(super::main_thread()).and_then(|b| b.image());
        let size = image.as_ref().map(|i| i.size()).unwrap_or(NSSize::ZERO);
        let (badge, update) = self.shown.badge.unwrap_or((0, false));
        json!({
            "status_item": true, "in_menu_bar": self.item().isVisible(), "badge": badge, "update": update, "label": self.shown.label,
            "dock": self.shown.dock, "image": [size.width, size.height], "template": image.is_some_and(|i| i.isTemplate()),
            "clicks": ivars.clicks.get(), "right_clicks": ivars.right_clicks.get(), "menu": self.shown.menu,
        })
    }
}

/// The badge's text: the count, "9+" past nine.
pub fn badge_text(count: usize) -> String {
    if count > 9 { "9+".into() } else { count.to_string() }
}

/// The item's image (MenuBarIcon.swift): the steering wheel symbol at 14 pt regular, with the
/// count of spaces no window shows in a capsule at its top right, the number cut out of it, and
/// `update`'s dot at the lower right. A template image, so the system draws it in the menu bar's
/// own colour; a gap cut around the badge and the dot keeps them apart from the mark.
pub fn icon(count: usize, update: bool) -> Retained<NSImage> {
    // SAFETY: AppKit's font weight constants, read once.
    let (regular, bold) = unsafe { (NSFontWeightRegular, NSFontWeightBold) };
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(14., regular);
    let mark = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str("steeringwheel"), None).and_then(|m| m.imageWithSymbolConfiguration(&configuration));
    let mark_size = mark.as_ref().map(|m| m.size()).unwrap_or(NSSize::new(16., 16.));
    let font = NSFont::systemFontOfSize_weight(8., bold);
    // SAFETY: the attribute key is AppKit's constant, its value the font it names.
    let attributes = NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[&*font as &AnyObject]);
    let label = unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(&badge_text(count)), &attributes) };
    let badge_height = 11.;
    let badge_width = if count > 0 { f64::max(badge_height, label.size().width.ceil() + 5.) } else { 0. };
    let dot = 7.;
    let extra = f64::max(if count > 0 { badge_width * 0.45 } else { 0. }, if update { dot * 0.45 } else { 0. });
    let size = NSSize::new(mark_size.width.ceil() + extra, 18.);
    let draw = RcBlock::new(move |rect: NSRect| -> Bool {
        if let Some(mark) = &mark {
            mark.drawInRect(NSRect::new(NSPoint::new(0., (rect.size.height - mark_size.height) / 2.), mark_size));
        }
        let Some(context) = NSGraphicsContext::currentContext() else { return Bool::YES };
        if update {
            let spot = NSRect::new(NSPoint::new(rect.size.width - dot, 0.), NSSize::new(dot, dot));
            context.setCompositingOperation(NSCompositingOperation::Clear);
            NSBezierPath::bezierPathWithOvalInRect(inset(spot, -1.5)).fill();
            context.setCompositingOperation(NSCompositingOperation::SourceOver);
            NSColor::blackColor().setFill();
            NSBezierPath::bezierPathWithOvalInRect(spot).fill();
        }
        if count == 0 {
            return Bool::YES;
        }
        let badge = NSRect::new(NSPoint::new(rect.size.width - badge_width, rect.size.height - badge_height), NSSize::new(badge_width, badge_height));
        context.setCompositingOperation(NSCompositingOperation::Clear);
        let r = badge_height / 2. + 1.5;
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset(badge, -1.5), r, r).fill();
        context.setCompositingOperation(NSCompositingOperation::SourceOver);
        NSColor::blackColor().setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(badge, badge_height / 2., badge_height / 2.).fill();
        context.setCompositingOperation(NSCompositingOperation::Clear);
        let text = label.size();
        label.drawAtPoint(NSPoint::new(badge.origin.x + badge.size.width / 2. - text.width / 2., badge.origin.y + badge.size.height / 2. - text.height / 2.));
        context.setCompositingOperation(NSCompositingOperation::SourceOver);
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw);
    image.setTemplate(true);
    image
}

fn inset(rect: NSRect, by: f64) -> NSRect {
    NSRect::new(NSPoint::new(rect.origin.x + by, rect.origin.y + by), NSSize::new(rect.size.width - 2. * by, rect.size.height - 2. * by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_reads_the_count_up_to_nine_then_nine_plus() {
        assert_eq!([0, 1, 9, 10, 12].map(badge_text), ["0", "1", "9", "9+", "9+"]);
    }
}
