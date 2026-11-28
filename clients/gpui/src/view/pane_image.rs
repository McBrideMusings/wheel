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
    /// Panes the window draws itself, to be cut from the next frame.
    window_shots: Vec<String>,
    /// `render_pane` requests from wheeld for panes the window draws itself: the id and the pane.
    wheeld_shots: Vec<(u64, String)>,
    /// Answers waiting for a render: the pane and its PNG or the reason it was refused.
    ready: Vec<(String, Result<Vec<u8>, String>)>,
    until: Option<u64>,
    /// Renders the debug window snapshot waits on, each answered down its channel.
    snapshots: HashMap<u64, futures::channel::oneshot::Sender<Result<Vec<u8>, String>>>,
}

impl PaneImages {
    /// Answer wheeld's render `id` of `pane` with a cut of the window's next frame.
    pub fn render_for_wheeld(&mut self, id: u64, pane: &str) {
        self.wheeld_shots.push((id, pane.to_owned()));
    }
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
            let entry = &status["panes"][pane.as_str()];
            if entry["hidden"] == true || entry["alpha"].as_f64().unwrap_or(1.) <= 0. {
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
    /// Draw `pane` and put its PNG on the clipboard once the bridge answers.
    pub fn copy_pane_image(&mut self, pane: &str) {
        self.log(format!("pane {pane}: copy image asked"));
        if !self.native_panes.iter().any(|p| p == pane) {
            // The window draws this pane itself (a chat, or a pane whose view went away when it
            // switched kind, which stays opened but hidden): the bridge has no picture of it to give.
            self.pane_images.window_shots.push(pane.to_owned());
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
        for pane in std::mem::take(&mut self.pane_images.window_shots) {
            // After this render, drawn the way the debug snapshot is: a hidden window has no display
            // link to call a next-frame callback, and render_to_image needs a current frame.
            cx.spawn_in(window, async move |this, cx| {
                cx.update(|window, cx| window.draw(cx).clear()).ok();
                this.update_in(cx, |this, window, cx| {
                    let result = this.cut_pane(&pane, window);
                    this.pane_images.ready.push((pane, result));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        for (id, pane) in std::mem::take(&mut self.pane_images.wheeld_shots) {
            cx.spawn_in(window, async move |this, cx| {
                cx.update(|window, cx| window.draw(cx).clear()).ok();
                this.update_in(cx, |this, window, _| {
                    let msg = match this.cut_pane(&pane, window) {
                        Ok(png) => ClientMsg::Rendered { id, png: base64::engine::general_purpose::STANDARD.encode(png) },
                        Err(message) => ClientMsg::RenderRefused { id, message },
                    };
                    this.send(&msg);
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
