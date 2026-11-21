//! A chat reply's Mermaid diagrams, laid out as SVG by `mermaid-rs-renderer` in a palette the
//! drawing client names. The GPUI Mac client lays out in its own process and draws the SVG;
//! wheeld lays out and rasterizes ([`png`], the `png` feature) for a client with no fonts the
//! crate can find (the iPhone: `fontdb` scans no iOS font folder), so every client gets the same
//! picture from one layout.

/// The colours a diagram is drawn in, each `#rrggbb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The block's ground.
    pub background: String,
    /// A node's fill.
    pub fill: String,
    /// The fill of edge labels and notes.
    pub fill_alt: String,
    /// The fill of a subgraph (cluster) and the third node colour.
    pub ground: String,
    /// A node's border.
    pub border: String,
    /// A subgraph's border.
    pub cluster_border: String,
    /// Edges and lifelines.
    pub line: String,
    /// Every label.
    pub text: String,
}

impl Palette {
    /// The first field that is not a `#rrggbb` colour, by name.
    pub fn invalid(&self) -> Option<&'static str> {
        let ok = |c: &str| c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit());
        [
            ("background", &self.background),
            ("fill", &self.fill),
            ("fill_alt", &self.fill_alt),
            ("ground", &self.ground),
            ("border", &self.border),
            ("cluster_border", &self.cluster_border),
            ("line", &self.line),
            ("text", &self.text),
        ]
        .into_iter()
        .find(|(_, c)| !ok(c))
        .map(|(name, _)| name)
    }

    fn theme(&self) -> mermaid_rs_renderer::Theme {
        let mut t = mermaid_rs_renderer::Theme::dark();
        t.font_family = "-apple-system, 'Helvetica Neue', Helvetica, sans-serif".into();
        t.font_size = 14.;
        t.background = self.background.clone();
        t.primary_color = self.fill.clone();
        t.primary_text_color = self.text.clone();
        t.text_color = self.text.clone();
        t.primary_border_color = self.border.clone();
        t.line_color = self.line.clone();
        t.secondary_color = self.fill_alt.clone();
        t.tertiary_color = self.ground.clone();
        t.edge_label_background = self.fill_alt.clone();
        t.cluster_background = self.ground.clone();
        t.cluster_border = self.cluster_border.clone();
        t.sequence_actor_fill = self.fill.clone();
        t.sequence_actor_border = self.border.clone();
        t.sequence_actor_line = self.line.clone();
        t.sequence_note_fill = self.fill_alt.clone();
        t.sequence_note_border = self.cluster_border.clone();
        t.pie_title_text_color = self.text.clone();
        t.pie_section_text_color = self.text.clone();
        t.pie_legend_text_color = self.text.clone();
        t
    }
}

/// The most source text one diagram may hold, in bytes.
pub const MAX_SOURCE: usize = 64 * 1024;

/// Whether a fenced block's language names Mermaid.
pub fn is_mermaid(language: Option<&str>) -> bool {
    language.is_some_and(|l| l.trim().eq_ignore_ascii_case("mermaid"))
}

/// `source` laid out as SVG in `palette` with its natural size, or why it could not be.
pub fn layout(source: &str, palette: &Palette) -> Result<(String, f32, f32), String> {
    if source.len() > MAX_SOURCE {
        return Err(format!("The diagram is over {} KB", MAX_SOURCE / 1024));
    }
    if let Some(field) = palette.invalid() {
        return Err(format!("The palette's {field} is not a #rrggbb colour"));
    }
    // The renderer has panicking paths (a missing node in layout); one becomes this block's error.
    std::panic::catch_unwind(|| {
        let options = mermaid_rs_renderer::RenderOptions { theme: palette.theme(), layout: Default::default() };
        let parsed = mermaid_rs_renderer::parse_mermaid_strict(source).map_err(|e| e.to_string())?;
        let layout = mermaid_rs_renderer::compute_layout(&parsed.graph, &options.theme, &options.layout);
        let size = mermaid_rs_renderer::measure_svg_dimensions(&layout, &options.layout, None);
        if !(size.width > 0. && size.height > 0.) {
            return Err("The diagram is empty".into());
        }
        Ok((mermaid_rs_renderer::render_svg(&layout, &options.theme, &options.layout), size.width, size.height))
    })
    .unwrap_or_else(|_| Err("The diagram's layout failed".into()))
}

/// The most pixels a side of a rasterized diagram may have.
#[cfg(feature = "png")]
pub const MAX_PIXELS: f32 = 4096.;

/// `svg` (a [`layout`] result) drawn as PNG at `scale` pixels per point (1 to 4), its labels set
/// in this machine's system fonts. A side over [`MAX_PIXELS`] lowers the scale to fit.
#[cfg(feature = "png")]
pub fn png(svg: &str, scale: f32) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    use std::sync::{Arc, OnceLock};
    // Scanning the system fonts takes a while: once per process.
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fontdb = FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    let options = usvg::Options { fontdb, ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| format!("The SVG did not parse: {e}"))?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.);
    let scale = scale.clamp(1., 4.).min(MAX_PIXELS / longest);
    let mut pixmap = tiny_skia::Pixmap::new((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32)
        .ok_or("The diagram has no area")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| format!("The PNG did not encode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let c = |s: &str| s.to_owned();
        Palette {
            background: c("#0b100e"),
            fill: c("#1a2621"),
            fill_alt: c("#22302a"),
            ground: c("#141d19"),
            border: c("#7fb89a"),
            cluster_border: c("#3d5247"),
            line: c("#8fa89b"),
            text: c("#d9e8e1"),
        }
    }

    #[test]
    fn a_flowchart_draws_and_garbage_says_why() {
        let (svg, width, height) = layout("flowchart LR\n  A[Start] --> B{Ready?}\n  B -->|yes| C[Ship]\n  B -->|no| A", &palette()).expect("the flowchart did not lay out");
        assert!(svg.contains("<svg") && svg.contains("#7fb89a") && width > 100. && height > 20., "{width}x{height}");
        let error = layout("flowchart LR\n  A --> \n  ((((", &palette()).expect_err("garbage laid out");
        assert!(!error.is_empty());
        assert!(is_mermaid(Some("Mermaid")) && !is_mermaid(Some("rust")) && !is_mermaid(None));
    }

    #[cfg(feature = "png")]
    #[test]
    fn a_flowchart_rasterizes_at_the_scale_asked() {
        let (svg, width, height) = layout("flowchart LR\n  A[Start] --> B[Ship]", &palette()).unwrap();
        let png = png(&svg, 2.).expect("the flowchart did not rasterize");
        assert_eq!(&png[1..4], b"PNG");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((w, h), ((width * 2.).ceil() as u32, (height * 2.).ceil() as u32));
    }

    #[test]
    fn a_palette_colour_that_is_not_hex_is_refused() {
        let mut p = palette();
        p.line = "red\" onload=\"x".into();
        assert_eq!(layout("flowchart LR\n A --> B", &p).unwrap_err(), "The palette's line is not a #rrggbb colour");
        assert!(layout(&"A".repeat(MAX_SOURCE + 1), &palette()).unwrap_err().contains("over 64 KB"));
    }
}
