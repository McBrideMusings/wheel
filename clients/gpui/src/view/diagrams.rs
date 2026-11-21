//! A reply's Mermaid diagrams: a ```mermaid block is laid out as SVG by `wheel-diagrams`
//! and drawn to a picture by GPUI's SVG renderer once per source, in the client, in the chat's palette, and shown in the code block's frame
//! in place of its source. A block that does not parse keeps its source with the parse error under
//! it. `/state` lists every diagram a reply drew under `chat_diagrams`, by agent.
use super::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

/// The tallest a diagram is drawn, in points; a taller one is scaled down to it.
pub const MAX_HEIGHT: f32 = 420.;

/// One block's source as drawn: its picture and natural size, or why it did not parse.
pub enum Drawing {
    Drawn { image: Arc<gpui::RenderImage>, width: f32, height: f32 },
    Failed(String),
}

/// One agent's drawn diagrams, by (row, nth Mermaid block in it).
type Shown = BTreeMap<(usize, usize), Value>;

/// Every diagram the transcripts drew, by source, and what each reply showed.
#[derive(Default)]
pub struct Diagrams {
    drawn: RefCell<HashMap<String, Rc<Drawing>>>,
    /// Per agent, each (row, nth Mermaid block in it) a reply drew: whether it is a picture, its drawn size, its error.
    shown: RefCell<HashMap<String, Shown>>,
}

pub use wheel_diagrams::is_mermaid;

/// The chat's palette (`block_el`'s colours).
fn palette() -> wheel_diagrams::Palette {
    let c = |s: &str| s.to_owned();
    wheel_diagrams::Palette {
        background: format!("#{CODE_BG:06x}"),
        fill: c("#1a2621"),
        fill_alt: c("#22302a"),
        ground: c("#141d19"),
        border: c("#7fb89a"),
        cluster_border: c("#3d5247"),
        line: c("#8fa89b"),
        text: c("#d9e8e1"),
    }
}

/// `source` laid out as SVG with its natural size, or the parse error.
pub fn layout(source: &str) -> Result<(String, f32, f32), String> {
    wheel_diagrams::layout(source, &palette())
}

/// `source` laid out and drawn to a picture now, so its first frame already shows it.
fn draw(source: &str, cx: &App) -> Drawing {
    match layout(source) {
        Ok((svg, width, height)) => match cx.svg_renderer().render_single_frame(svg.as_bytes(), 1.) {
            Ok(image) => Drawing::Drawn { image, width, height },
            Err(e) => Drawing::Failed(format!("Cannot draw the SVG: {e}")),
        },
        Err(e) => Drawing::Failed(e),
    }
}

/// The drawn size of a `width` by `height` diagram: its own, scaled down to `MAX_HEIGHT` tall.
pub fn fitted(width: f32, height: f32) -> (f32, f32) {
    let scale = (MAX_HEIGHT / height.max(1.)).min(1.);
    (width * scale, height * scale)
}

impl Diagrams {
    /// The drawing of `source`, made once per source while a reply streams and redraws.
    fn drawing(&self, source: &str, cx: &App) -> Rc<Drawing> {
        let mut drawn = self.drawn.borrow_mut();
        if drawn.len() > 64 {
            drawn.clear();
        }
        drawn.entry(source.to_owned()).or_insert_with(|| Rc::new(draw(source, cx))).clone()
    }

    /// Every diagram each agent's replies drew, as `/state` lists them.
    pub fn json(&self) -> Value {
        Value::Object(self.shown.borrow().iter().map(|(agent, shown)| (agent.clone(), Value::Array(shown.values().cloned().collect()))).collect())
    }
}

/// The element a closed Mermaid block in a reply draws (an open one, still streaming, stays code): the diagram in the code block's frame, or the
/// parse error, under which the caller draws the source as a code block.
pub(super) fn diagram_el(code: &str, texts: &mut Texts, cx: &mut Context<Client>) -> Result<gpui::Div, String> {
    let drawing = texts.diagrams.drawing(code, cx);
    let nth = texts.diagram;
    texts.diagram += 1;
    let mut entry = json!({"row": texts.row, "block": nth, "drawn": false, "width": 0, "height": 0, "error": null, "lines": code.lines().count()});
    let el = match &*drawing {
        Drawing::Drawn { image, width, height } => {
            let (w, h) = fitted(*width, *height);
            entry["drawn"] = json!(true);
            entry["width"] = json!(w);
            entry["height"] = json!(h);
            let copied = code.to_owned();
            let id = format!("copy-diagram-{}-{nth}", texts.row);
            Ok(div().rounded_sm().bg(rgb(CODE_BG)).border_1().border_color(rgb(0x2c3d34)).flex().flex_col().overflow_hidden()
                .child(div().h(px(20.)).px_2().flex().items_center().justify_between().bg(rgb(CODE_HEADER)).text_xs().text_color(rgb(0x8fa89b))
                    .child("mermaid")
                    .child(div().id(gpui::ElementId::Name(id.into())).px_1().rounded_sm().cursor_pointer().bg(rgb(0x22302a)).child("Copy")
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                            cx.stop_propagation();
                        }))))
                .child(div().p_2().child(gpui::img(gpui::ImageSource::Render(image.clone())).w(px(w)).h(px(h)).max_w_full().object_fit(gpui::ObjectFit::Contain))))
        }
        Drawing::Failed(error) => {
            entry["error"] = json!(error);
            Err(error.clone())
        }
    };
    texts.diagrams.shown.borrow_mut().entry(texts.agent.clone()).or_default().insert((texts.row, nth), entry);
    el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flowchart_draws_and_garbage_says_why() {
        let (svg, width, height) = layout("flowchart LR\n  A[Start] --> B{Ready?}\n  B -->|yes| C[Ship]\n  B -->|no| A").expect("the flowchart did not lay out");
        assert!(svg.contains("<svg") && width > 100. && height > 20., "{width}x{height}");
        let error = layout("flowchart LR\n  A --> \n  ((((").expect_err("garbage laid out");
        assert!(!error.is_empty());
        assert!(is_mermaid(Some("Mermaid")) && !is_mermaid(Some("rust")) && !is_mermaid(None));
        assert_eq!(fitted(200., 840.), (100., 420.));
    }
}
