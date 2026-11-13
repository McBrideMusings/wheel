//! The header's Copy Pane Image (AppModel.copyPaneImage): the bridge draws the pane as `wheel pane
//! shot` does, with its native terminal or page contents, the PNG goes on the clipboard, and a
//! notice says which pane; a later copy replaces the notice and restarts its 3 s.
use super::*;
use super::settings::now_ms;
use crate::toasts::{Level, Toast};
use base64::Engine;

/// The notice's id, and how long it stays.
const NOTICE: &str = "pane-image";
const NOTICE_MS: u64 = 3000;
/// The bit that marks a render request as this client's own, so its answer never goes to wheeld
/// as the answer to a `render_pane` of the same number.
const OWN: u64 = 1 << 63;

/// The renders asked for the clipboard, their answers, and when the notice goes.
#[derive(Default)]
pub struct PaneImages {
    next: u64,
    asked: HashMap<u64, String>,
    /// Answers waiting for a render: the pane and its PNG (base64) or the reason it was refused.
    ready: Vec<(String, Result<String, String>)>,
    until: Option<u64>,
}

impl PaneImages {
    /// Keep the answer to render `id` when it was asked here; false when it is wheeld's.
    pub fn answered(&mut self, id: u64, result: Result<String, String>) -> bool {
        let Some(pane) = self.asked.remove(&id) else { return false };
        self.ready.push((pane, result));
        true
    }
}

impl Client {
    /// Draw `pane` and put its PNG on the clipboard once the bridge answers.
    pub fn copy_pane_image(&mut self, pane: &str) {
        self.log(format!("pane {pane}: copy image asked"));
        if !self.opened.contains_key(pane) {
            // The window draws this pane itself: the bridge has no picture of it to give.
            self.pane_images.ready.push((pane.to_owned(), Err("the GPUI client draws this pane itself; its picture is not served yet".into())));
            return;
        }
        let id = OWN | self.pane_images.next;
        self.pane_images.next += 1;
        self.pane_images.asked.insert(id, pane.to_owned());
        self.native.render(id, pane, false);
    }
    /// Each render: put what the bridge drew on the clipboard with its notice, and take the notice
    /// down 3 s after the last copy.
    pub fn tick_pane_image(&mut self, cx: &mut Context<Self>) {
        for (pane, result) in std::mem::take(&mut self.pane_images.ready) {
            let png = result.and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("the picture was not a PNG ({e})")));
            let toast = match png {
                Ok(png) => {
                    self.log(format!("pane {pane}: image copied ({} bytes)", png.len()));
                    cx.write_to_clipboard(gpui::ClipboardItem { entries: vec![gpui::ClipboardEntry::Image(gpui::Image::from_bytes(gpui::ImageFormat::Png, png))] });
                    Toast::posted(NOTICE.into(), Level::Notice, format!("Copied {pane} as an image"), None)
                }
                Err(message) => Toast::posted(NOTICE.into(), Level::Error, format!("Could not copy {pane}: {message}"), None),
            };
            self.post(toast);
            self.pane_images.until = Some(now_ms() + NOTICE_MS);
        }
        if let Some(until) = self.pane_images.until {
            let now = now_ms();
            if now >= until {
                self.remove_toast(NOTICE);
                self.pane_images.until = None;
            } else {
                self.redraw_in(until - now, cx);
            }
        }
    }
}
