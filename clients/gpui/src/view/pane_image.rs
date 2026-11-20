//! The header's Copy Pane Image (AppModel.copyPaneImage): the bridge draws the pane as `wheel pane
//! shot` does, with its native terminal or page contents, the PNG goes on the clipboard, and a
//! notice says which pane; a later copy replaces the notice and restarts its 3 s. A pane with no
//! native view is the window's own drawing: its rectangle is cut from the window's last frame.
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
/// A render the debug window snapshot asked for: its id and the channel its PNG comes down.
pub type SnapshotRender = (u64, futures::channel::oneshot::Receiver<Result<Vec<u8>, String>>);

/// The renders asked for the clipboard, their answers, and when the notice goes.
#[derive(Default)]
pub struct PaneImages {
    next: u64,
    asked: HashMap<u64, String>,
    /// Panes the window draws itself, to be cut from the next frame, each with the id of the
    /// wheeld request it answers (None for the clipboard's own).
    window_shots: Vec<(String, Option<u64>)>,
    /// Answers waiting for a render: the pane and its PNG or the reason it was refused.
    ready: Vec<(String, Result<Vec<u8>, String>)>,
    until: Option<u64>,
    /// Renders the debug window snapshot waits on, each answered down its channel.
    snapshots: HashMap<u64, futures::channel::oneshot::Sender<Result<Vec<u8>, String>>>,
}

/// Whether a native view's last placement leaves it showing on screen: not hidden under a card or
/// out of the window, and not faded out. No placement yet counts as showing (a card's page view).
fn shows_native(placement: Option<&Placement>) -> bool {
    placement.is_none_or(|(_, hidden, _, alpha)| !hidden && *alpha > 0.)
}

impl PaneImages {
    /// Keep the answer to render `id` when it was asked here; false when it is wheeld's.
    pub fn answered(&mut self, id: u64, result: Result<String, String>) -> bool {
        let decode = |result: Result<String, String>| result.and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("the picture was not a PNG ({e})")));
        if let Some(waiter) = self.snapshots.remove(&id) {
            let _ = waiter.send(decode(result));
            return true;
        }
        let Some(pane) = self.asked.remove(&id) else { return false };
        self.ready.push((pane, decode(result)));
        true
    }
}

impl Client {
    /// For the debug window snapshot: the bridge's status (where each native view is and what is
    /// cut from it), and per native pane either a render asked of it (its id, its PNG down the
    /// channel) or None when the window does not show it (hidden or faded out).
    pub fn render_native_for_snapshot(&mut self) -> (serde_json::Value, Vec<(String, Option<SnapshotRender>)>) {
        let status = self.native.status();
        // The artifact cards' page views in agent transcripts are pasted as the panes' are.
        let renders = self.native_panes.clone().into_iter().chain(self.card_views.keys().cloned()).map(|pane| {
            if !shows_native(self.placed.get(&pane)) {
                return (pane, None);
            }
            let (send, receive) = futures::channel::oneshot::channel();
            let id = OWN | self.pane_images.next;
            self.pane_images.next += 1;
            self.pane_images.snapshots.insert(id, send);
            self.native.render(id, &pane, false);
            (pane, Some((id, receive)))
        }).collect();
        (status, renders)
    }
    /// Stop waiting for snapshot renders the bridge never answered.
    pub fn forget_snapshot_renders(&mut self, ids: &[u64]) {
        for id in ids {
            self.pane_images.snapshots.remove(id);
        }
    }
    /// Whether `pane`'s picture is the bridge's render of its native view: it has one and it is on
    /// screen. Otherwise the window's own drawing is what the person sees (a chat, a browser pane
    /// whose load failed under its card, a pane whose view is hidden) and its picture is cut from it.
    pub fn native_on_screen(&self, pane: &str) -> bool {
        self.native_panes.iter().any(|p| p == pane) && shows_native(self.placed.get(pane))
    }
    /// `wheel pane shot`: draw `pane` as the person sees it. True when it is asked (the answer goes
    /// to wheeld from `tick_pane_image` or the bridge), false when this client does not show it.
    pub fn render_for_wheeld(&mut self, id: u64, pane: &str) -> bool {
        if !self.opened.contains_key(pane) {
            return false;
        }
        if self.native_on_screen(pane) {
            self.native.render(id, pane, false);
        } else {
            self.pane_images.window_shots.push((pane.to_owned(), Some(id)));
        }
        true
    }
    /// Draw `pane` and put its PNG on the clipboard once the bridge answers.
    pub fn copy_pane_image(&mut self, pane: &str) {
        self.log(format!("pane {pane}: copy image asked"));
        if !self.native_on_screen(pane) {
            self.pane_images.window_shots.push((pane.to_owned(), None));
            return;
        }
        let id = OWN | self.pane_images.next;
        self.pane_images.next += 1;
        self.pane_images.asked.insert(id, pane.to_owned());
        self.native.render(id, pane, false);
    }
    /// `pane`'s rectangle of the window's last frame as a PNG, in the frame's own pixels.
    fn cut_pane(&self, pane: &str, window: &Window) -> Result<Vec<u8>, String> {
        let rect = self.scene().0.into_iter().find(|p| p.leaf.id == pane).map(|p| p.rect).ok_or("the pane is not shown")?;
        let frame = window.render_to_image().map_err(|e| format!("the window could not be drawn ({e})"))?;
        let scale = frame.width() as f32 / f32::from(window.viewport_size().width).max(1.);
        let (x, y) = ((rect.x * scale).round().max(0.) as u32, (rect.y * scale).round().max(0.) as u32);
        let (w, h) = (((rect.w * scale).round() as u32).min(frame.width().saturating_sub(x)), ((rect.h * scale).round() as u32).min(frame.height().saturating_sub(y)));
        let cut = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(cut).write_to(&mut png, image::ImageFormat::Png).map_err(|e| format!("the drawing could not be encoded as a PNG ({e})"))?;
        Ok(png.into_inner())
    }
    /// Each render: put what the bridge drew on the clipboard with its notice, and take the notice
    /// down 3 s after the last copy.
    pub fn tick_pane_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (pane, wheeld) in std::mem::take(&mut self.pane_images.window_shots) {
            // After this render, drawn the way the debug snapshot is: a hidden window has no display
            // link to call a next-frame callback, and render_to_image needs a current frame.
            cx.spawn_in(window, async move |this, cx| {
                cx.update(|window, cx| window.draw(cx).clear()).ok();
                this.update_in(cx, |this, window, cx| {
                    let result = this.cut_pane(&pane, window);
                    match (wheeld, result) {
                        (Some(id), Ok(png)) => this.send(&ClientMsg::Rendered { id, png: base64::engine::general_purpose::STANDARD.encode(png) }),
                        (Some(id), Err(message)) => this.send(&ClientMsg::RenderRefused { id, message }),
                        (None, result) => this.pane_images.ready.push((pane, result)),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        for (pane, result) in std::mem::take(&mut self.pane_images.ready) {
            let toast = match result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_hidden_under_a_card_or_faded_out_is_not_what_is_on_screen() {
        let placed = |hidden, alpha| ([0.; 4], hidden, vec![], alpha);
        assert!(shows_native(None));
        assert!(shows_native(Some(&placed(false, 1.))));
        assert!(!shows_native(Some(&placed(true, 1.))));
        assert!(!shows_native(Some(&placed(false, 0.))));
    }
}
