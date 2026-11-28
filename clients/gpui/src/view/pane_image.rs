//! The header's Copy Pane Image (AppModel.copyPaneImage): the bridge draws the pane as `wheel pane
//! shot` does, with its native terminal or page contents, the PNG goes on the clipboard, and a
//! notice says which pane; a later copy replaces the notice and restarts its 3 s. A pane with no
//! native view is the window's own drawing: its rectangle is cut from the window's last frame, and
//! a plugin page's embeds (web pages, maps, video streams in native slots) are drawn over its cut.
use super::*;
use super::settings::now_ms;
use crate::toasts::{Level, Toast};
use base64::Engine;
use wheel_gpui_core::native::EmbedPicture;

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
    /// Plugin pages to cut from the next frame with their embeds drawn in: wheeld's render id, or
    /// None for the clipboard, and the pane.
    plugin_shots: Vec<(Option<u64>, String)>,
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
    /// Answer wheeld's render `id` (None: the clipboard's copy) of plugin pane `pane` with its page
    /// cut from the window's next frame, each embed's native view drawn in.
    pub fn render_plugin(&mut self, id: Option<u64>, pane: &str) {
        self.plugin_shots.push((id, pane.to_owned()));
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
            if !self.native.render(id, &pane, false) {
                self.pane_images.answered(id, Err("its view holds no page".into()));
            }
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
        if self.plugins.contains_key(pane) {
            self.pane_images.render_plugin(None, pane);
            return;
        }
        if !self.native_panes.iter().any(|p| p == pane) {
            // The window draws this pane itself (a chat, or a pane whose view went away when it
            // switched kind, which stays opened but hidden): the bridge has no picture of it to give.
            self.pane_images.window_shots.push(pane.to_owned());
            return;
        }
        let id = OWN | self.pane_images.next;
        self.pane_images.next += 1;
        self.pane_images.asked.insert(id, pane.to_owned());
        if !self.native.render(id, pane, false) {
            self.pane_images.answered(id, Err("its view holds no page".into()));
        }
    }
    /// `pane`'s rectangle of the window's last frame as a PNG, in the frame's own pixels.
    fn cut_pane(&self, pane: &str, window: &Window) -> Result<Vec<u8>, String> {
        let rect = self.scene().0.into_iter().find(|p| p.leaf.id == pane).map(|p| p.rect).ok_or("the pane is not shown")?;
        let (cut, _) = self.cut_rect([rect.x, rect.y, rect.w, rect.h], window)?;
        encode(cut)
    }
    /// `rect` (window points) of the window's last frame, in the frame's own pixels, and how many of them a point is.
    fn cut_rect(&self, rect: [f32; 4], window: &Window) -> Result<(image::RgbaImage, f32), String> {
        let frame = window.render_to_image().map_err(|e| format!("the window could not be drawn ({e})"))?;
        let scale = frame.width() as f32 / f32::from(window.viewport_size().width).max(1.);
        let (x, y) = ((rect[0] * scale).round().max(0.) as u32, (rect[1] * scale).round().max(0.) as u32);
        let (w, h) = (((rect[2] * scale).round() as u32).min(frame.width().saturating_sub(x)), ((rect[3] * scale).round() as u32).min(frame.height().saturating_sub(y)));
        Ok((image::imageops::crop_imm(&frame, x, y, w, h).to_image(), scale))
    }
    /// Plugin pane `pane`'s page from the window's last frame, as the `body` it recorded, with where it lies.
    fn cut_plugin(&self, pane: &str, window: &Window) -> Result<(image::RgbaImage, [f64; 4], f32), String> {
        let (x, y, w, h) = self.control_bounds(pane, "body").ok_or("the pane is not shown")?;
        let (cut, scale) = self.cut_rect([x, y, w, h], window)?;
        Ok((cut, [x, y, w, h].map(f64::from), scale))
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
        for (id, pane) in std::mem::take(&mut self.pane_images.plugin_shots) {
            cx.spawn_in(window, async move |this, cx| {
                cx.update(|window, cx| window.draw(cx).clear()).ok();
                let Ok(cut) = this.update_in(cx, |this, window, _| this.cut_plugin(&pane, window)) else { return };
                let result = match cut {
                    Ok((page, at, scale)) => {
                        let (send, receive) = futures::channel::oneshot::channel();
                        this.update(cx, |this, _| this.native.embed_pictures(&pane, Box::new(move |pictures| { let _ = send.send(pictures); }))).ok();
                        let pictures = receive.await.unwrap_or_default();
                        encode(draw_embeds(page, at, scale, &pictures))
                    }
                    Err(message) => Err(message),
                };
                this.update(cx, |this, cx| {
                    match id {
                        Some(id) => this.send(&match result {
                            Ok(png) => ClientMsg::Rendered { id, png: base64::engine::general_purpose::STANDARD.encode(png) },
                            Err(message) => ClientMsg::RenderRefused { id, message },
                        }),
                        None => this.pane_images.ready.push((pane, result)),
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

fn encode(image: image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png).map_err(|e| format!("the drawing could not be encoded as a PNG ({e})"))?;
    Ok(png.into_inner())
}

/// `page`, a cut of the window at `at` (window points, `scale` pixels a point), with each embed's
/// picture drawn over the parts of its slot not cut out: GPUI's frame has only empty slots there.
fn draw_embeds(mut page: image::RgbaImage, at: [f64; 4], scale: f32, pictures: &[EmbedPicture]) -> image::RgbaImage {
    let s = f64::from(scale);
    for picture in pictures {
        let Ok(shot) = image::load_from_memory(&picture.png) else { continue };
        let f = picture.frame;
        let shot = image::imageops::resize(&shot.to_rgba8(), (f[2] * s).round().max(1.) as u32, (f[3] * s).round().max(1.) as u32, image::imageops::FilterType::Triangle);
        for v in &picture.visible {
            // The part of this rect inside the page.
            let (x0, y0) = (v[0].max(at[0]), v[1].max(at[1]));
            let (x1, y1) = ((v[0] + v[2]).min(at[0] + at[2]), (v[1] + v[3]).min(at[1] + at[3]));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let px = |n: f64| (n * s).round().max(0.) as u32;
            let part = image::imageops::crop_imm(&shot, px(x0 - f[0]), px(y0 - f[1]), px(x1 - x0), px(y1 - y0)).to_image();
            image::imageops::replace(&mut page, &part, px(x0 - at[0]).into(), px(y0 - at[1]).into());
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(image: image::RgbaImage) -> Vec<u8> {
        encode(image).unwrap()
    }

    #[test]
    fn an_embed_draws_only_its_visible_part_inside_the_page() {
        // A 10x10 page at (100, 50), 2 pixels a point; an embed at (104, 46) 4x8, its top 4 points cut by the page's edge.
        let page = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        let red = image::RgbaImage::from_pixel(8, 16, image::Rgba([255, 0, 0, 255]));
        let picture = EmbedPicture { frame: [104., 46., 4., 8.], visible: vec![[104., 46., 4., 8.]], png: png(red) };
        let out = draw_embeds(page, [100., 50., 10., 10.], 2., &[picture]);
        assert_eq!(out.get_pixel(8, 0)[0], 255, "the embed's part inside the page draws");
        assert_eq!(out.get_pixel(8, 7)[0], 255);
        assert_eq!(out.get_pixel(8, 8)[0], 0, "below the embed the page stays");
        assert_eq!(out.get_pixel(7, 0)[0], 0, "left of the embed the page stays");
        // A hole over the embed's lower half leaves the page there.
        let page = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        let red = image::RgbaImage::from_pixel(8, 16, image::Rgba([255, 0, 0, 255]));
        let picture = EmbedPicture { frame: [104., 50., 4., 8.], visible: vec![[104., 50., 4., 4.]], png: png(red) };
        let out = draw_embeds(page, [100., 50., 10., 10.], 2., &[picture]);
        assert_eq!((out.get_pixel(8, 7)[0], out.get_pixel(8, 8)[0]), (255, 0));
    }
}
