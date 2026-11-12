//! The window's tooltips: one line of text in a dark box, shown by GPUI after the pointer rests
//! on an element that names one (`.tooltip(tip("…"))`, the Swift client's `.help`).
use gpui::{App, Context, Render, Window, div, prelude::*, rgb};
use std::cell::RefCell;

thread_local! {
    /// The text of the last tooltip drawn, so a check can read what a hover shows.
    static SHOWN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A tooltip's view.
struct Tip(String);

/// A tooltip reading `text`.
pub fn tip(text: &str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.to_owned();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

/// The text of the last tooltip drawn, cleared by the read.
pub fn take_shown() -> Option<String> {
    SHOWN.with(|s| s.borrow_mut().take())
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SHOWN.with(|s| *s.borrow_mut() = Some(self.0.clone()));
        div().px_2().py_1().rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(0x2c3d34)).text_xs().text_color(rgb(0xd9e8e1)).child(self.0.clone())
    }
}
