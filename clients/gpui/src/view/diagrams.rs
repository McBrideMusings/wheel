//! A reply's Mermaid diagrams: a ```mermaid block is laid out as SVG by `mermaid-rs-renderer`
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

/// Whether a fenced block's language names Mermaid.
pub fn is_mermaid(language: Option<&str>) -> bool {
    language.is_some_and(|l| l.trim().eq_ignore_ascii_case("mermaid"))
}

/// The chat's palette (`block_el`'s colours) as a Mermaid theme.
fn theme() -> mermaid_rs_renderer::Theme {
    let mut t = mermaid_rs_renderer::Theme::dark();
    t.font_family = "-apple-system, 'Helvetica Neue', Helvetica, sans-serif".into();
    t.font_size = 14.;
    t.background = format!("#{CODE_BG:06x}");
    t.primary_color = "#1a2621".into();
    t.primary_text_color = "#d9e8e1".into();
    t.text_color = "#d9e8e1".into();
    t.primary_border_color = "#7fb89a".into();
    t.line_color = "#8fa89b".into();
    t.secondary_color = "#22302a".into();
    t.tertiary_color = "#141d19".into();
    t.edge_label_background = "#22302a".into();
    t.cluster_background = "#141d19".into();
    t.cluster_border = "#3d5247".into();
    t.sequence_actor_fill = "#1a2621".into();
    t.sequence_actor_border = "#7fb89a".into();
    t.sequence_actor_line = "#8fa89b".into();
    t.sequence_note_fill = "#22302a".into();
    t.sequence_note_border = "#3d5247".into();
    t.pie_title_text_color = "#d9e8e1".into();
    t.pie_section_text_color = "#d9e8e1".into();
    t.pie_legend_text_color = "#d9e8e1".into();
    t
}

/// `source` laid out as SVG with its natural size, or the parse error.
pub fn layout(source: &str) -> Result<(String, f32, f32), String> {
    let options = mermaid_rs_renderer::RenderOptions { theme: theme(), layout: Default::default() };
    let parsed = mermaid_rs_renderer::parse_mermaid_strict(source).map_err(|e| e.to_string())?;
    let layout = mermaid_rs_renderer::compute_layout(&parsed.graph, &options.theme, &options.layout);
    let size = mermaid_rs_renderer::measure_svg_dimensions(&layout, &options.layout, None);
    if !(size.width > 0. && size.height > 0.) {
        return Err("The diagram is empty".into());
    }
    Ok((mermaid_rs_renderer::render_svg(&layout, &options.theme, &options.layout), size.width, size.height))
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
