//! A browser pane's chrome, drawn by GPUI above the bridge's web view (the Swift client's
//! `BrowserBar`, `AddressField` and `BrowserFailure` in BrowserPane.swift): back, forward,
//! reload or stop, home and the address field; under 360 points only back and the field stay.
//! The page itself is WheelUI's `BrowserPane` in the bridge; the view here draws from the state
//! the bridge emits and sends it the presses.
use crate::{native::BrowserState, view::{Client, Control}};
use gpui::{Context, MouseButton, div, prelude::*, px, rgb};
use std::{cell::RefCell, rc::Rc};

/// The chrome row's height, in points.
pub const CHROME: f32 = 34.0;
/// Narrower than this, the row keeps back and the field.
pub const FULL_WIDTH: f32 = 360.0;
pub const PLACEHOLDER: &str = "Enter a web address or a file path";

/// How the address field shows a page it is not editing (WheelKit's `BrowserPanes.shown`): a
/// blank page as the placeholder, a web page as its host in full and the rest dimmed, a file as
/// its folder dimmed and its name in full, anything else as it is.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Blank,
    Web { secure: bool, host: String, rest: String },
    File { folder: String, name: String },
    Other(String),
}
pub fn shown(url: &str) -> Shown {
    // A failed file is shown by the path the person typed, which has no scheme.
    if let Some(path) = url.strip_prefix('/') {
        let name = path.rsplit('/').next().unwrap_or_default();
        return Shown::File { folder: url[..url.len() - name.len()].to_owned(), name: name.to_owned() };
    }
    let Some((scheme, rest)) = url.split_once(':') else { return Shown::Other(url.to_owned()) };
    match scheme.to_ascii_lowercase().as_str() {
        "about" => if url.eq_ignore_ascii_case("about:blank") { Shown::Blank } else { Shown::Other(url.to_owned()) },
        "file" => {
            let path = rest.strip_prefix("//").unwrap_or(rest);
            let path = percent_decode(path);
            let name = path.rsplit('/').next().unwrap_or_default().to_owned();
            Shown::File { folder: path[..path.len() - name.len()].to_owned(), name }
        }
        s @ ("http" | "https") => {
            let Some(after) = rest.strip_prefix("//") else { return Shown::Other(url.to_owned()) };
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let (host, tail) = after.split_at(end);
            if host.is_empty() {
                return Shown::Other(url.to_owned());
            }
            let tail = if tail == "/" { "" } else { tail };
            Shown::Web { secure: s == "https", host: host.to_owned(), rest: tail.to_owned() }
        }
        _ => Shown::Other(url.to_owned()),
    }
}
/// The address as the person edits it: a file as its path, a blank page as nothing.
pub fn editable(address: &str) -> String {
    match shown(address) {
        Shown::Blank => String::new(),
        Shown::File { folder, name } => format!("{folder}{name}"),
        _ => address.to_owned(),
    }
}
/// The pane's title: the page's own, else its host or file name; "Browser" on a blank page.
pub fn title(state: &BrowserState) -> String {
    if !state.title.is_empty() && state.failure.is_none() {
        return state.title.clone();
    }
    match shown(&state.address) {
        Shown::Blank => "Browser".into(),
        Shown::Web { host, .. } => host,
        Shown::File { name, .. } => name,
        Shown::Other(text) => text,
    }
}
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi * 16 + lo);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// What a press on the chrome asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    Back,
    Forward,
    Reload,
    Stop,
    Home,
    /// A click on the address field: start editing it.
    Edit,
    /// Try Again on the failure card.
    Retry,
}

/// The chrome row for `pane`, `width` points wide. `editing` is the address draft while the
/// field is being edited. Each control records its bounds under `browser:<name>` for the check.
pub fn chrome(pane: &str, state: &BrowserState, width: f32, editing: Option<&str>, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let narrow = width < FULL_WIDTH;
    let button = |label: &'static str, name: &'static str, enabled: bool, press: Press, cx: &mut Context<Client>| {
        let pane = pane.to_owned();
        let mut b = div().relative().w(px(26.)).h(px(24.)).rounded_sm().flex().items_center().justify_center().flex_none()
            .text_color(rgb(if enabled { 0xd9e8e1 } else { 0x4d5f57 })).child(label)
            .child(crate::view::recorded(&pane, &format!("browser:{name}"), controls));
        if enabled {
            b = b.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.browser_press(&pane, press.clone());
                cx.stop_propagation();
                cx.notify();
            }));
        }
        b
    };
    let mut row = div().w(px(width)).h(px(CHROME)).flex_none().px_2().flex().items_center().gap(px(2.)).relative().overflow_hidden().whitespace_nowrap()
        .child(button("‹", "back", state.can_go_back, Press::Back, cx));
    if !narrow {
        row = row.child(button("›", "forward", state.can_go_forward, Press::Forward, cx));
        row = row.child(if state.loading { button("×", "stop", true, Press::Stop, cx) } else { button("↻", "reload", true, Press::Reload, cx) });
        row = row.child(button("⌂", "home", true, Press::Home, cx));
    }
    let field = {
        let pane = pane.to_owned();
        // The field takes what the buttons leave and clips a long address at its end, as the
        // Swift field truncates its tail; it never widens the row.
        let mut field = div().relative().flex_1().min_w(px(0.)).ml(px(6.)).px(px(6.)).h(px(22.)).rounded_md().border_1().border_color(rgb(0x3d5247)).bg(rgb(0x0e1311))
            .flex().items_center().gap_1().overflow_hidden().whitespace_nowrap().text_xs().cursor_text()
            .child(crate::view::recorded(&pane, "browser:address", controls));
        field = match editing {
            Some(draft) => field.border_color(rgb(0x7fb89a)).child(div().text_color(rgb(0xd9e8e1)).child(format!("{draft}▏"))),
            None => match shown(&state.address) {
                Shown::Blank => field.child(div().text_color(rgb(0x4d5f57)).child(PLACEHOLDER)),
                Shown::Web { secure, host, rest } => field.children(secure.then(|| div().flex_none().text_color(rgb(0x8fa89b)).child("🔒")))
                    .child(div().flex_none().text_color(rgb(0xd9e8e1)).child(host)).child(div().min_w(px(0.)).overflow_hidden().text_color(rgb(0x8fa89b)).child(rest)),
                Shown::File { folder, name } => field.child(div().min_w(px(0.)).overflow_hidden().text_color(rgb(0x8fa89b)).child(format!("▭ {folder}"))).child(div().flex_none().text_color(rgb(0xd9e8e1)).child(name)),
                Shown::Other(text) => field.child(div().min_w(px(0.)).overflow_hidden().text_color(rgb(0xd9e8e1)).child(text)),
            },
        };
        if editing.is_none() {
            field = field.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.browser_press(&pane, Press::Edit);
                cx.stop_propagation();
                cx.notify();
            }));
        }
        field
    };
    row = row.child(field);
    if state.loading {
        let w = (width - 16.) * state.progress.max(0.05) as f32;
        row = row.child(div().absolute().left(px(8.)).bottom_0().h(px(2.)).w(px(w.max(0.))).bg(rgb(0x7fb89a)));
    }
    row
}

/// Why the page did not load, in place of the page, with a way to try the same address again
/// (the Swift client's `BrowserFailure`).
pub fn failure_card(pane: &str, failure: &crate::native::Failure, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let pane = pane.to_owned();
    div().size_full().min_h(px(0.)).overflow_hidden().flex().flex_col().items_center().justify_center().gap(px(6.)).p(px(12.)).bg(rgb(0x17201c)).text_color(rgb(0x8fa89b))
        .child(div().flex_none().text_color(rgb(0xd9e8e1)).child("Can’t open this page"))
        .child(div().flex_none().text_xs().text_center().child(failure.message.clone()))
        .child(div().flex_none().text_xs().overflow_hidden().whitespace_nowrap().w_full().text_center().text_ellipsis_middle().child(failure.address.clone()))
        .child(div().relative().mt(px(8.)).px_2().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e)).text_color(rgb(0xd9e8e1)).child("Try Again")
            .child(crate::view::recorded(&pane, "browser:retry", controls))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.browser_press(&pane, Press::Retry);
                cx.stop_propagation();
                cx.notify();
            })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_splits_pages_as_the_swift_field_does() {
        assert_eq!(shown("about:blank"), Shown::Blank);
        assert_eq!(shown("https://example.com/"), Shown::Web { secure: true, host: "example.com".into(), rest: String::new() });
        assert_eq!(shown("http://localhost:7421/a?b=1#c"), Shown::Web { secure: false, host: "localhost:7421".into(), rest: "/a?b=1#c".into() });
        assert_eq!(shown("file:///Users/p/notes%20two.html"), Shown::File { folder: "/Users/p/".into(), name: "notes two.html".into() });
        assert_eq!(shown("file:///tmp/%aé%zz.html"), Shown::File { folder: "/tmp/".into(), name: "%aé%zz.html".into() });
        assert_eq!(shown("file:///tmp/caf%C3%A9.html"), Shown::File { folder: "/tmp/".into(), name: "café.html".into() });
        assert_eq!(shown("/Users/p/x.html"), Shown::File { folder: "/Users/p/".into(), name: "x.html".into() });
        assert_eq!(shown("data:text/html,hi"), Shown::Other("data:text/html,hi".into()));
        assert_eq!(editable("about:blank"), "");
        assert_eq!(editable("file:///tmp/a.html"), "/tmp/a.html");
        assert_eq!(editable("https://example.com/"), "https://example.com/");
    }

    #[test]
    fn title_prefers_the_page_name() {
        let mut s = BrowserState { address: "https://example.com/x".into(), ..Default::default() };
        assert_eq!(title(&s), "example.com");
        s.title = "Example".into();
        assert_eq!(title(&s), "Example");
        s.address = "about:blank".into();
        s.title.clear();
        assert_eq!(title(&s), "Browser");
    }
}
