//! A reply's inline images, the port of WheelUI's `ChatImageLoader`: a Markdown image of a file
//! is asked of wheeld (`file_image`, read against the agent's directory, so the file is the one on
//! the daemon's machine) and drawn at its own size up to 220 points tall; a click opens the file
//! beside the chat as a click on a link to it does. A web image stays a link to its page. `/state`
//! lists every image a reply drew under `chat_images`, by agent.
use super::*;
use crate::chat_links::{self, Target};
use base64::Engine;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

/// The tallest an image is drawn, in points (WheelKit's `chat.images.max_height` default).
pub const MAX_HEIGHT: f32 = 220.;

/// One image as far as wheeld has answered.
#[derive(Clone)]
pub enum Load {
    Waiting,
    Loaded { path: String, image: Arc<gpui::Image>, width: u32, height: u32, bytes: usize },
    Failed { path: String, error: String },
}

/// Every image the transcripts asked for, by (directory, path), and what each reply drew.
// hack: loads and failures are kept for the client's life and never asked again, so a file rewritten on disk keeps its old picture and one written after its reply stays missing; evict and re-ask when a transcript grows, as link probes do, once a reply redraws a changing image.
#[derive(Default)]
pub struct ImageState {
    loads: RefCell<HashMap<(String, String), Load>>,
    asks: RefCell<Vec<(String, String)>>,
    reqs: HashMap<u64, (String, String)>,
    /// Per agent, each (row, source) a reply drew and the directory it was read against.
    shown: RefCell<HashMap<String, BTreeMap<(usize, String), String>>>,
}

/// The file a Markdown image's address names, or None for a web or other-scheme address, which
/// stays a link. `file://` and percent escapes are undone, as WheelUI reads a scheme-less URL.
pub fn file_of(src: &str) -> Option<String> {
    let src = src.trim();
    if let Some(rest) = src.strip_prefix("file://") {
        return Some(chat_links::unescape(rest));
    }
    let scheme = src.split_once(':').is_some_and(|(s, _)| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && s.len() > 1);
    (!src.is_empty() && !scheme).then(|| chat_links::unescape(src))
}

/// wheeld's `mime` as the format GPUI decodes.
fn format_of(mime: Option<&str>) -> Option<gpui::ImageFormat> {
    Some(match mime? {
        "image/png" => gpui::ImageFormat::Png,
        "image/jpeg" => gpui::ImageFormat::Jpeg,
        "image/gif" => gpui::ImageFormat::Gif,
        "image/webp" => gpui::ImageFormat::Webp,
        _ => return None,
    })
}

/// The drawn size of a `width` by `height` image: its own, scaled down to `MAX_HEIGHT` tall.
pub fn fitted(width: u32, height: u32) -> (f32, f32) {
    let (w, h) = (width as f32, height as f32);
    let scale = (MAX_HEIGHT / h.max(1.)).min(1.);
    (w * scale, h * scale)
}

impl ImageState {
    /// What the image `src` in reply `row` of `agent`, read against `dir`, shows now; a file not
    /// asked about yet is queued for wheeld.
    pub fn shown(&self, agent: &str, row: usize, dir: &str, path: &str) -> Load {
        self.shown.borrow_mut().entry(agent.to_owned()).or_default().insert((row, path.to_owned()), dir.to_owned());
        let key = (dir.to_owned(), path.to_owned());
        if let Some(load) = self.loads.borrow().get(&key) {
            return load.clone();
        }
        self.loads.borrow_mut().insert(key.clone(), Load::Waiting);
        self.asks.borrow_mut().push(key);
        Load::Waiting
    }

    /// Every image each agent's replies drew, as `/state` lists them (WheelUI's `images`).
    pub fn json(&self) -> Value {
        let loads = self.loads.borrow();
        let agents: serde_json::Map<String, Value> = self.shown.borrow().iter().map(|(agent, shown)| {
            let list: Vec<Value> = shown.iter().map(|((row, source), dir)| {
                let mut o = json!({"row": row, "source": source, "loaded": false, "remote": false, "width": 0, "height": 0, "bytes": 0, "path": null, "error": null});
                match loads.get(&(dir.clone(), source.clone())) {
                    Some(Load::Loaded { path, width, height, bytes, .. }) => {
                        let (w, h) = fitted(*width, *height);
                        o["loaded"] = json!(true);
                        o["path"] = json!(path);
                        o["width"] = json!(width);
                        o["height"] = json!(height);
                        o["bytes"] = json!(bytes);
                        o["drawn"] = json!([w, h]);
                    }
                    Some(Load::Failed { path, error }) => {
                        o["path"] = json!(path);
                        o["error"] = json!(error);
                    }
                    _ => {}
                }
                o
            }).collect();
            (agent.clone(), json!(list))
        }).collect();
        Value::Object(agents)
    }
}

impl ImageState {
    /// Forget the asks a dropped connection will never answer, so the next frame asks again.
    pub fn reconnected(&mut self) {
        self.reqs.clear();
        self.loads.get_mut().retain(|_, l| !matches!(l, Load::Waiting));
    }
}

impl Client {
    /// Ask wheeld for the images the last frame found.
    pub(super) fn ask_images(&mut self) {
        if self.link != Link::Up {
            // Asked again once the link is up.
            let asks: Vec<(String, String)> = self.images.asks.borrow_mut().drain(..).collect();
            let mut loads = self.images.loads.borrow_mut();
            for key in asks {
                loads.remove(&key);
            }
            return;
        }
        // Asked once wheeld's `status` says what it handles.
        if self.server_offer.is_none() {
            return;
        }
        let asks: Vec<(String, String)> = self.images.asks.borrow_mut().drain(..).collect();
        if !self.server_has("file_image") {
            let mut loads = self.images.loads.borrow_mut();
            for key in asks {
                loads.insert(key, Load::Failed { path: String::new(), error: "This wheeld cannot send images".into() });
            }
            return;
        }
        for (dir, path) in asks {
            let req = self.req();
            self.images.reqs.insert(req, (dir.clone(), path.clone()));
            self.send(&ClientMsg::FileImage { req: Some(req), path, cwd: (!dir.is_empty()).then_some(dir) });
        }
    }

    /// wheeld's answer to an image asked here; false for any other `file_image`.
    pub(super) fn on_file_image(&mut self, req: u64, path: &str, found: bool, mime: Option<&str>, data: Option<&str>, error: Option<&str>) -> bool {
        let Some(key) = self.images.reqs.remove(&req) else { return false };
        let failed = |error: String| Load::Failed { path: path.to_owned(), error };
        let load = if !found {
            failed(error.map(str::to_owned).unwrap_or_else(|| format!("No image at {path}")))
        } else {
            match (format_of(mime), data.map(|d| base64::engine::general_purpose::STANDARD.decode(d))) {
                (Some(format), Some(Ok(bytes))) => match image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok()) {
                    Some((width, height)) if width > 0 && height > 0 => {
                        let size = bytes.len();
                        Load::Loaded { path: path.to_owned(), image: Arc::new(gpui::Image::from_bytes(format, bytes)), width, height, bytes: size }
                    }
                    _ => failed("Cannot read the image".into()),
                },
                _ => failed("Cannot read the image".into()),
            }
        };
        match &load {
            Load::Loaded { bytes, width, height, .. } => self.log(format!("chat image {} in {}: {width}x{height}, {bytes} bytes", key.1, key.0)),
            Load::Failed { error, .. } => self.log(format!("chat image {} in {}: {error}", key.1, key.0)),
            Load::Waiting => {}
        }
        self.images.loads.borrow_mut().insert(key, load);
        // The row grows to the picture's height: every column is measured again.
        self.links.moved();
        true
    }
}

/// The element a reply's image draws: the picture, a click on it opening the file beside the
/// chat, or a line saying why there is none. A web image is its alt text linked to its page.
pub(super) fn image_el(src: &str, alt: &str, texts: &mut Texts, cx: &mut Context<Client>) -> gpui::Div {
    let Some(path) = file_of(src) else {
        let text = if alt.is_empty() { src.to_owned() } else { alt.to_owned() };
        let runs = vec![crate::markdown::Run { range: 0..text.len(), mark: Mark { link: Some(src.to_owned()), ..Mark::default() } }];
        return div().child(texts.styled("reply", &text, &runs, 0xd9e8e1, None, false));
    };
    match texts.images.shown(&texts.agent, texts.row, &texts.dir, &path) {
        Load::Loaded { path: full, image, width, height, .. } => {
            let (w, h) = fitted(width, height);
            let (agent, address) = (texts.agent.clone(), Target::File { path: full, line: None }.address());
            div().child(div().id(gpui::ElementId::Name(format!("chat-image-{}-{}-{src}", texts.agent, texts.row).into())).cursor_pointer()
                .when(!alt.is_empty(), |d| d.tooltip(crate::tooltip::tip(alt)))
                .child(gpui::img(gpui::ImageSource::Image(image)).w(px(w)).h(px(h)).max_w_full().object_fit(gpui::ObjectFit::Contain))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_link(&agent, &address, false, cx);
                    cx.notify();
                })))
        }
        Load::Failed { error, .. } => div().text_xs().text_color(rgb(0x8fa89b)).child(format!("\u{1F5BC} {error}")),
        Load::Waiting => div().text_xs().text_color(rgb(0x6f8a7c)).child(if alt.is_empty() { "Loading image\u{2026}".to_owned() } else { format!("Loading {alt}\u{2026}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_address_is_a_path_and_a_web_one_is_not() {
        assert_eq!(file_of("out/chart.png").as_deref(), Some("out/chart.png"));
        assert_eq!(file_of("/tmp/a%20b.png").as_deref(), Some("/tmp/a b.png"));
        assert_eq!(file_of("file:///tmp/x.png").as_deref(), Some("/tmp/x.png"));
        assert_eq!(file_of("~/x.png").as_deref(), Some("~/x.png"));
        assert_eq!(file_of("https://a.b/x.png"), None);
        assert_eq!(file_of("data:image/png;base64,AA"), None);
        assert_eq!(file_of(""), None);
    }

    #[test]
    fn a_tall_image_fits_the_height_and_a_small_one_keeps_its_size() {
        assert_eq!(fitted(100, 50), (100., 50.));
        assert_eq!(fitted(880, 440), (440., 220.));
    }
}
