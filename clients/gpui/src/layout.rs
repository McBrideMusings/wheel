//! wheeld's split tree (`Node` in proto/src/lib.rs) and its placement into rectangles.
use serde_json::Value;
use std::collections::HashMap;

/// Width or height of a divider between two panes, in points.
pub const GAP: f32 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub term: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    Pane(Leaf),
    Split { id: String, horizontal: bool, ratio: f64, a: Box<Tree>, b: Box<Tree> },
}

/// One space as the daemon lists it.
#[derive(Clone, Debug)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub root: Tree,
}

pub fn parse(v: &Value) -> Tree {
    let text = |k: &str| v[k].as_str().map(str::to_owned);
    if v["node"] == "split" {
        Tree::Split {
            id: text("id").unwrap_or_default(),
            horizontal: v["axis"] == "horizontal",
            ratio: v["ratio"].as_f64().unwrap_or(0.5),
            a: Box::new(parse(&v["a"])),
            b: Box::new(parse(&v["b"])),
        }
    } else {
        Tree::Pane(Leaf {
            id: text("id").unwrap_or_default(),
            kind: text("kind").unwrap_or_else(|| "unknown".into()),
            label: text("name").or_else(|| text("url")).or_else(|| text("cwd")).unwrap_or_default(),
            term: text("term"),
        })
    }
}

/// The spaces in a `status` reply or a `spaces` broadcast.
pub fn parse_spaces(message: &Value) -> Option<Vec<Space>> {
    if !matches!(message["ev"].as_str(), Some("status" | "spaces")) {
        return None;
    }
    Some(
        message["spaces"].as_array().into_iter().flatten()
            .map(|s| Space {
                id: s["id"].as_str().unwrap_or_default().to_owned(),
                name: s["name"].as_str().unwrap_or_default().to_owned(),
                root: parse(&s["root"]),
            })
            .collect(),
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
pub struct Divider {
    pub split: String,
    pub horizontal: bool,
    pub rect: Rect,
    /// The area of the split this divider belongs to; a drag measures its ratio against it.
    pub parent: Rect,
}
pub struct Placed {
    pub leaf: Leaf,
    pub rect: Rect,
}

/// Binary split tree to rectangles. `ratio` is the share of the area less one gap that `a`
/// takes; `overrides` holds ratios being dragged and not yet confirmed by the daemon.
pub fn place(tree: &Tree, rect: Rect, overrides: &HashMap<String, f64>, leaves: &mut Vec<Placed>, dividers: &mut Vec<Divider>) {
    match tree {
        Tree::Pane(leaf) => leaves.push(Placed { leaf: leaf.clone(), rect }),
        Tree::Split { id, horizontal, ratio, a, b } => {
            let ratio = overrides.get(id).copied().unwrap_or(*ratio) as f32;
            if *horizontal {
                let aw = ((rect.w - GAP) * ratio).round();
                place(a, Rect { w: aw, ..rect }, overrides, leaves, dividers);
                dividers.push(Divider { split: id.clone(), horizontal: true, rect: Rect { x: rect.x + aw, w: GAP, ..rect }, parent: rect });
                place(b, Rect { x: rect.x + aw + GAP, w: rect.w - aw - GAP, ..rect }, overrides, leaves, dividers);
            } else {
                let ah = ((rect.h - GAP) * ratio).round();
                place(a, Rect { h: ah, ..rect }, overrides, leaves, dividers);
                dividers.push(Divider { split: id.clone(), horizontal: false, rect: Rect { y: rect.y + ah, h: GAP, ..rect }, parent: rect });
                place(b, Rect { y: rect.y + ah + GAP, h: rect.h - ah - GAP, ..rect }, overrides, leaves, dividers);
            }
        }
    }
}

/// The ratio a pointer at `(x, y)` asks of a split, within the daemon's 0.1 to 0.9 range.
pub fn drag_ratio(horizontal: bool, parent: Rect, x: f32, y: f32) -> f64 {
    let ratio = if horizontal { (x - parent.x) / (parent.w - GAP) } else { (y - parent.y) / (parent.h - GAP) };
    f64::from(ratio).clamp(0.1, 0.9)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Tree {
        parse(&json!({"node":"split","id":"x1","axis":"horizontal","ratio":0.5,
            "a":{"node":"pane","id":"p1","kind":"web","name":"page"},
            "b":{"node":"split","id":"x2","axis":"vertical","ratio":0.25,
                "a":{"node":"pane","id":"p2","kind":"terminal","term":"t1"},
                "b":{"node":"pane","id":"p3","kind":"browser","url":"about:blank"}}}))
    }
    fn scene(overrides: &HashMap<String, f64>) -> (Vec<Placed>, Vec<Divider>) {
        let (mut leaves, mut dividers) = (vec![], vec![]);
        place(&tree(), Rect { x: 0., y: 10., w: 1006., h: 506. }, overrides, &mut leaves, &mut dividers);
        (leaves, dividers)
    }

    #[test]
    fn places_every_leaf_and_divider() {
        let (leaves, dividers) = scene(&HashMap::new());
        let rects: Vec<_> = leaves.iter().map(|p| (p.leaf.id.as_str(), p.rect)).collect();
        assert_eq!(rects[0], ("p1", Rect { x: 0., y: 10., w: 500., h: 506. }));
        assert_eq!(rects[1], ("p2", Rect { x: 506., y: 10., w: 500., h: 125. }));
        assert_eq!(rects[2], ("p3", Rect { x: 506., y: 141., w: 500., h: 375. }));
        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].rect, Rect { x: 500., y: 10., w: GAP, h: 506. });
        assert_eq!(dividers[1].rect, Rect { x: 506., y: 135., w: 500., h: GAP });
    }

    #[test]
    fn leaves_and_gaps_tile_the_area_without_overlap() {
        let (leaves, _) = scene(&HashMap::new());
        let (a, b) = (leaves[0].rect, leaves[1].rect);
        assert_eq!(a.x + a.w + GAP, b.x);
    }

    #[test]
    fn an_override_replaces_the_daemon_ratio() {
        let (leaves, _) = scene(&HashMap::from([("x1".to_owned(), 0.8)]));
        assert_eq!(leaves[0].rect.w, 800.);
        assert_eq!(leaves[1].rect.x, 806.);
    }

    #[test]
    fn a_drag_stays_inside_the_daemons_range() {
        let parent = Rect { x: 0., y: 0., w: 1006., h: 506. };
        assert_eq!(drag_ratio(true, parent, -50., 0.), 0.1);
        assert_eq!(drag_ratio(true, parent, 5000., 0.), 0.9);
        assert_eq!(drag_ratio(true, parent, 500., 0.), 0.5);
        assert_eq!(drag_ratio(false, parent, 0., 250.), 0.5);
    }

    #[test]
    fn unknown_kinds_survive_parsing() {
        let leaf = parse(&json!({"node":"pane","id":"p9","kind":"hologram"}));
        assert_eq!(leaf, Tree::Pane(Leaf { id: "p9".into(), kind: "hologram".into(), label: String::new(), term: None }));
    }

    #[test]
    fn spaces_come_from_status_and_broadcast_only() {
        let spaces = json!({"ev":"spaces","spaces":[{"id":"s1","name":"wheel","root":{"node":"pane","id":"p1","kind":"empty"}}]});
        assert_eq!(parse_spaces(&spaces).unwrap()[0].name, "wheel");
        assert!(parse_spaces(&json!({"ev":"term_output"})).is_none());
    }
}
