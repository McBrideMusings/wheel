//! The Focus and Graph views: the layouts of the installed page's `graph.js`, in Rust. The layout
//! becomes a scene for the generic `Canvas` component (ADR-0029): shapes, text, hit regions and the
//! zoom levels at which each layer shows. The client draws the scene and keeps the camera (pan and
//! zoom) to itself; it knows nothing of beads. The toolbar, caption and legend are ordinary components.
//!
//! Focus puts one bead in the middle: what blocks it on the left (by hops), what it unlocks on
//! the right, its parent above, its children below. Graph puts every bead on two axes: x is the
//! dependency layer (longest path from a bead with no blockers), y is one lane per epic.

use super::*;

const NH: f64 = 28.0;
const PITCH: f64 = 34.0;
const GAP: f64 = 40.0;
const CAP: usize = 13;
const PAD: f64 = 14.0;
const CW: f64 = 150.0;
const CH: f64 = 30.0;
const LANE_HDR: f64 = 30.0;
const LANE_GAP: f64 = 14.0;

pub(super) const WORDS: [&str; 7] = ["ready", "in progress", "blocked", "needs you", "deferred", "open", "done"];

fn st_code(b: &Bead) -> u8 {
    let k = kind_of(b);
    if k == "done" {
        6
    } else if b.labels.iter().any(|l| l == "human") {
        3
    } else if b.kind == "epic" {
        5
    } else {
        match k {
            "ready" => 0,
            "in progress" => 1,
            "blocked" => 2,
            "deferred" => 4,
            _ => 5,
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub(super) struct Model<'a> {
    b: Vec<&'a Bead>,
    idx: HashMap<&'a str, usize>,
    blk: Vec<Vec<usize>>,
    unl: Vec<Vec<usize>>,
    miss: Vec<Vec<String>>,
    par: Vec<Option<usize>>,
    kids: Vec<Vec<usize>>,
    st: Vec<u8>,
    rank: Vec<i32>,
    ok: Vec<bool>,
}

struct Cap {
    shown: Vec<usize>,
    more: Vec<usize>,
    key: String,
}

fn cap(list: Vec<usize>, key: &str) -> Cap {
    if list.len() > CAP {
        Cap { shown: list[..CAP].to_vec(), more: list[CAP..].to_vec(), key: key.into() }
    } else {
        Cap { shown: list, more: vec![], key: key.into() }
    }
}

struct FNode {
    i: Option<usize>,
    ghost: Option<String>,
    more: Option<(String, Vec<usize>)>,
    kind: &'static str,
    x: f64,
    y: f64,
    w: f64,
}

impl<'a> Model<'a> {
    pub(super) fn new(snap: &'a Snapshot, ui: &Ui) -> Self {
        let mut seen = HashSet::new();
        let b: Vec<&Bead> = snap.beads.iter().filter(|b| seen.insert(b.id.as_str())).collect();
        let n = b.len();
        let idx: HashMap<&str, usize> = b.iter().enumerate().map(|(i, x)| (x.id.as_str(), i)).collect();
        let mut blk = vec![vec![]; n];
        let mut unl = vec![vec![]; n];
        let mut miss = vec![vec![]; n];
        let mut par = vec![None; n];
        let mut kids = vec![vec![]; n];
        for (i, x) in b.iter().enumerate() {
            for id in &x.blocked_by {
                match idx.get(id.as_str()) {
                    None => miss[i].push(id.clone()),
                    Some(&j) if j != i && !blk[i].contains(&j) => {
                        blk[i].push(j);
                        unl[j].push(i);
                    }
                    _ => {}
                }
            }
            if let Some(&p) = x.parent.as_deref().and_then(|p| idx.get(p)) {
                if p != i {
                    par[i] = Some(p);
                    kids[p].push(i);
                }
            }
        }
        let st: Vec<u8> = b.iter().map(|x| st_code(x)).collect();
        // Longest-path layer, cycle-safe: a back edge is ignored, so a cycle still gets layers.
        let mut rank = vec![0i32; n];
        let mut colour = vec![0u8; n];
        for s in 0..n {
            if colour[s] != 0 {
                continue;
            }
            colour[s] = 1;
            let mut stack = vec![(s, 0usize)];
            while let Some(&(u, k)) = stack.last() {
                if k < blk[u].len() {
                    stack.last_mut().unwrap().1 += 1;
                    let w = blk[u][k];
                    if colour[w] == 0 {
                        colour[w] = 1;
                        stack.push((w, 0));
                    }
                } else {
                    let mut r = 0;
                    for &w in &blk[u] {
                        if colour[w] == 2 {
                            r = r.max(rank[w] + 1);
                        }
                    }
                    rank[u] = r;
                    colour[u] = 2;
                    stack.pop();
                }
            }
        }
        // Which beads the search and filters let through. An epic is a lane or a parent, never a row.
        let ok = b.iter().map(|x| x.kind == "epic" || x.kind == "milestone" || matches(ui, x)).collect();
        Model { b, idx, blk, unl, miss, par, kids, st, rank, ok }
    }

    fn n(&self) -> usize {
        self.b.len()
    }

    fn by_pri(&self, a: usize, b: usize) -> std::cmp::Ordering {
        let pa = self.b[a].priority.unwrap_or(9);
        let pb = self.b[b].priority.unwrap_or(9);
        pa.cmp(&pb).then_with(|| natural(&self.b[a].id, &self.b[b].id))
    }

    fn sorted(&self, mut v: Vec<usize>) -> Vec<usize> {
        v.sort_by(|&a, &b| self.by_pri(a, b));
        v
    }

    fn default_focus(&self) -> Option<usize> {
        let mut best = None;
        let mut score = -1i64;
        for i in 0..self.n() {
            if self.b[i].kind == "epic" || self.st[i] == 6 {
                continue;
            }
            let s = (self.blk[i].len() + self.unl[i].len()) as i64;
            if s > score {
                score = s;
                best = Some(i);
            }
        }
        best.or(if self.n() > 0 { Some(0) } else { None })
    }

    pub(super) fn focus_idx(&self, ui: &Ui) -> Option<usize> {
        ui.sel.as_deref().and_then(|s| self.idx.get(s).copied()).or_else(|| self.default_focus())
    }

    pub(super) fn id_of(&self, i: usize) -> &str {
        &self.b[i].id
    }

    pub(super) fn has(&self, id: &str) -> bool {
        self.idx.contains_key(id)
    }

    pub(super) fn index(&self, id: &str) -> Option<usize> {
        self.idx.get(id).copied()
    }

    fn layers(c: usize, adj: &[Vec<usize>], depth: usize, placed: &mut HashSet<usize>) -> Vec<Vec<usize>> {
        let mut out = vec![vec![]];
        let mut frontier = vec![c];
        for _ in 1..=depth {
            let mut nx = vec![];
            for &u in &frontier {
                for &v in &adj[u] {
                    if placed.insert(v) {
                        nx.push(v);
                    }
                }
            }
            if nx.is_empty() {
                break;
            }
            out.push(nx.clone());
            frontier = nx;
        }
        out
    }

    /// Order each layer: the first by priority, the rest by where the beads they link to sit.
    fn place(&self, layers: &mut [Vec<usize>], link: &[Vec<usize>], c: usize) {
        let mut pos: HashMap<usize, usize> = HashMap::from([(c, 0)]);
        for k in 1..layers.len() {
            let avg = |i: usize| -> f64 {
                let (mut s, mut n) = (0.0, 0.0);
                for j in &link[i] {
                    if let Some(p) = pos.get(j) {
                        s += *p as f64;
                        n += 1.0;
                    }
                }
                if n > 0.0 { s / n } else { 1e9 }
            };
            let mut layer = std::mem::take(&mut layers[k]);
            if k == 1 {
                layer.sort_by(|&a, &b| self.by_pri(a, b));
            } else {
                layer.sort_by(|&a, &b| {
                    avg(a).partial_cmp(&avg(b)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| self.by_pri(a, b))
                });
            }
            for (n, &i) in layer.iter().enumerate() {
                pos.insert(i, n);
            }
            layers[k] = layer;
        }
    }

    fn short_title(&self, i: usize) -> &str {
        &self.b[i].title
    }

    fn item(&self, i: usize) -> Value {
        json!({ "id": self.b[i].id, "title": self.b[i].title, "st": self.st[i] })
    }

    /// The Focus view: one bead, its neighbourhood on two axes. `cw` is the width the client
    /// reported for its canvas.
    fn focus(&self, ui: &Ui, cw: f64) -> Value {
        let Some(c) = self.focus_idx(ui) else {
            return json!({ "empty": "Pick a bead to see what it is connected to.", "caption": "" });
        };
        let mut placed: HashSet<usize> = HashSet::from([c]);
        let mut l = Self::layers(c, &self.blk, ui.depth, &mut placed);
        let mut r = Self::layers(c, &self.unl, ui.depth, &mut placed);
        self.place(&mut l, &self.unl, c);
        self.place(&mut r, &self.blk, c);
        let gh = self.miss[c].clone();
        if !gh.is_empty() && l.len() < 2 {
            l.push(vec![]);
        }
        let max_l = l.len() - 1;
        let max_r = r.len() - 1;
        let cols = (max_l + 1 + max_r) as f64;
        let w = ((cw - 2.0 * PAD - (cols - 1.0) * GAP) / cols).floor().clamp(130.0, 260.0);
        // Layers spread across the width: the space the nodes leave over widens the gaps between columns.
        let gapx = if cols > 1.0 { ((cw - 2.0 * PAD - cols * w) / (cols - 1.0)).floor().clamp(GAP, 240.0) } else { GAP };
        let dep_w = cols * w + (cols - 1.0) * gapx;
        let total = dep_w.max(cw - 2.0 * PAD);
        let x0 = PAD + (total - dep_w) / 2.0;
        let col_x = |k: i64| x0 + ((max_l as i64 + k) as f64) * (w + gapx);

        struct Col {
            k: i64,
            shown: Vec<usize>,
            more: Vec<usize>,
            key: String,
            ghosts: Vec<String>,
        }
        let mut cols_data: Vec<Col> = vec![];
        for k in (1..=max_l).rev() {
            let cp = cap(l[k].clone(), &format!("L{k}"));
            cols_data.push(Col {
                k: -(k as i64),
                shown: cp.shown,
                more: cp.more,
                key: cp.key,
                ghosts: if k == 1 { gh.clone() } else { vec![] },
            });
        }
        cols_data.push(Col { k: 0, shown: vec![c], more: vec![], key: String::new(), ghosts: vec![] });
        for k in 1..=max_r {
            let cp = cap(r[k].clone(), &format!("R{k}"));
            cols_data.push(Col { k: k as i64, shown: cp.shown, more: cp.more, key: cp.key, ghosts: vec![] });
        }
        let col_h = |d: &Col| (d.shown.len() + d.ghosts.len() + usize::from(!d.more.is_empty())) as f64 * PITCH;

        // Rows from the top: the parent band, the siblings lane, the dependency area, the children lane.
        let mut y = 6.0;
        let mut anc: Vec<usize> = vec![];
        {
            let mut p = self.par[c];
            let mut n = 0;
            while let Some(pp) = p {
                if n >= 30 || anc.contains(&pp) {
                    break;
                }
                n += 1;
                anc.insert(0, pp);
                p = self.par[pp];
            }
        }
        let cut = anc.len() > 3;
        let shown_anc: Vec<usize> = anc[anc.len().saturating_sub(3)..].to_vec();
        let band_y = y;
        let band_h = 44.0;
        y += band_h + 8.0;
        let parent = self.par[c];
        let sib_all: Vec<usize> = match parent {
            Some(p) => self.sorted(self.kids[p].iter().copied().filter(|s| *s != c && !placed.contains(s)).collect()),
            None => vec![],
        };
        let sibs = cap(sib_all.clone(), "S");
        let per_row = (((total + GAP / 2.0) / (w + 10.0)).floor()).max(1.0) as usize;
        let rows_for = |n: usize| n.div_ceil(per_row);
        let mut lanes: Vec<Value> = vec![json!({ "x": PAD, "y": band_y, "w": total, "h": band_h })];
        let mut labels: Vec<Value> = vec![];
        let label = |labels: &mut Vec<Value>, x: f64, cy: f64, text: String, bold: bool| {
            labels.push(json!({ "x": x, "y": cy, "text": text, "bold": bold }));
        };
        let mut sib_lane: Option<(f64, f64)> = None;
        if let (false, Some(p)) = (sib_all.is_empty(), parent) {
            let cnt = sibs.shown.len() + usize::from(!sibs.more.is_empty());
            let h = 24.0 + rows_for(cnt) as f64 * PITCH + 4.0;
            lanes.push(json!({ "x": PAD, "y": y, "w": total, "h": h }));
            label(&mut labels, PAD + 12.0, y + 12.0, format!("Siblings in {}", self.short_title(p)), true);
            sib_lane = Some((y, h));
            y += h + 8.0;
        }
        let hdr_y = y;
        y += 18.0;
        let dep_h = cols_data.iter().map(col_h).fold(PITCH, f64::max);
        let dep_top = y;
        y += dep_h + 8.0;
        let kid_all: Vec<usize> = self.sorted(self.kids[c].iter().copied().filter(|k| !placed.contains(k)).collect());
        let kc = cap(kid_all.clone(), "C");
        let mut kid_lane: Option<(f64, f64)> = None;
        if !kid_all.is_empty() {
            let cnt = kc.shown.len() + usize::from(!kc.more.is_empty());
            let h = 24.0 + rows_for(cnt) as f64 * PITCH + 4.0;
            lanes.push(json!({ "x": PAD, "y": y, "w": total, "h": h }));
            label(&mut labels, PAD + 12.0, y + 12.0, format!("Children ({})", kid_all.len()), true);
            kid_lane = Some((y, h));
            y += h + 8.0;
        }
        let height = y + 4.0;

        // the band's own words
        let aw = (w + 40.0).min(260.0);
        let anc_x = |n: usize| PAD + 10.0 + n as f64 * (aw + 26.0) + if cut { 20.0 } else { 0.0 };
        if shown_anc.is_empty() {
            label(&mut labels, PAD + 12.0, band_y + 22.0, "No parent: this bead belongs to no epic.".into(), false);
        } else {
            if cut {
                label(&mut labels, PAD + 10.0, band_y + 22.0, "…".into(), false);
            }
            for n in 1..shown_anc.len() {
                label(&mut labels, anc_x(n) - 17.0, band_y + 22.0, "›".into(), false);
            }
            let last_x = anc_x(shown_anc.len() - 1) + aw + 12.0;
            label(
                &mut labels,
                last_x,
                band_y + 22.0,
                format!("parent{} above", if shown_anc.len() > 1 { "s" } else { "" }),
                false,
            );
        }

        let mut nodes: Vec<FNode> = vec![];
        let mut at: HashMap<usize, usize> = HashMap::new();
        let mut add = |nodes: &mut Vec<FNode>, n: FNode| {
            if let Some(i) = n.i {
                at.insert(i, nodes.len());
            }
            nodes.push(n);
        };
        for (n, &a) in shown_anc.iter().enumerate() {
            add(&mut nodes, FNode { i: Some(a), ghost: None, more: None, kind: "parent", x: anc_x(n), y: band_y + 8.0, w: aw });
        }
        let mid_y = dep_top + dep_h / 2.0;
        for d in &cols_data {
            let m = (d.shown.len() + d.ghosts.len() + usize::from(!d.more.is_empty())) as f64;
            let mut yy = mid_y - m * PITCH / 2.0 + (PITCH - NH) / 2.0;
            let x = col_x(d.k);
            for &i in &d.shown {
                add(&mut nodes, FNode { i: Some(i), ghost: None, more: None, kind: if d.k == 0 { "center" } else { "dep" }, x, y: yy, w });
                yy += PITCH;
            }
            for g in &d.ghosts {
                add(&mut nodes, FNode { i: None, ghost: Some(g.clone()), more: None, kind: "ghost", x, y: yy, w });
                yy += PITCH;
            }
            if !d.more.is_empty() {
                add(&mut nodes, FNode { i: None, ghost: None, more: Some((d.key.clone(), d.more.clone())), kind: "more", x, y: yy, w });
            }
        }
        let flow = |nodes: &mut Vec<FNode>, add: &mut dyn FnMut(&mut Vec<FNode>, FNode), lane: (f64, f64), list: &Cap, kind: &'static str| {
            let mut items: Vec<FNode> = list
                .shown
                .iter()
                .map(|&i| FNode { i: Some(i), ghost: None, more: None, kind, x: 0.0, y: 0.0, w })
                .collect();
            if !list.more.is_empty() {
                items.push(FNode { i: None, ghost: None, more: Some((list.key.clone(), list.more.clone())), kind: "more", x: 0.0, y: 0.0, w });
            }
            for (k, mut n) in items.into_iter().enumerate() {
                n.x = PAD + (k % per_row) as f64 * (w + 10.0);
                n.y = lane.0 + 24.0 + (k / per_row) as f64 * PITCH + 3.0;
                add(nodes, n);
            }
        };
        if let Some(lane) = sib_lane {
            flow(&mut nodes, &mut add, lane, &sibs, "sib");
        }
        if let Some(lane) = kid_lane {
            flow(&mut nodes, &mut add, lane, &kc, "child");
        }

        // edges: every dependency between two beads on the page, a dotted line for each parent link
        struct Edge {
            a: usize,
            b: usize,
            t: &'static str,
        }
        let mut edges: Vec<Edge> = vec![];
        let mut cyc = false;
        let plain = |k: &str| !matches!(k, "child" | "sib" | "parent");
        for (ni, n) in nodes.iter().enumerate() {
            let Some(i) = n.i else { continue };
            for &v in &self.unl[i] {
                let Some(&mi) = at.get(&v) else { continue };
                let m = &nodes[mi];
                let back = plain(n.kind) && plain(m.kind) && m.x < n.x - 4.0;
                cyc |= back;
                edges.push(Edge { a: ni, b: mi, t: if back { "cyc" } else { "dep" } });
            }
        }
        let cn = at[&c];
        if let Some(p) = parent {
            if let Some(&pn) = at.get(&p) {
                edges.push(Edge { a: pn, b: cn, t: "par" });
            }
        }
        for (ni, n) in nodes.iter().enumerate() {
            if n.kind == "child" {
                edges.push(Edge { a: cn, b: ni, t: "par" });
            }
        }
        let edge_json: Vec<Value> = edges
            .iter()
            .map(|e| {
                let (a, b) = (&nodes[e.a], &nodes[e.b]);
                if e.t == "par" {
                    let down = b.y > a.y;
                    let ax = a.x + a.w / 2.0;
                    let bx = b.x + b.w / 2.0;
                    let ay = if down { a.y + NH } else { a.y };
                    let by = if down { b.y } else { b.y + NH };
                    let my = (ay + by) / 2.0;
                    return json!({ "t": "par", "p": [ax, ay, ax, my, bx, my, bx, by] });
                }
                if (a.x - b.x).abs() < 4.0 {
                    // the same column: loop out of the right sides
                    let x = a.x + a.w;
                    let (o, ay, by) = (26.0, a.y + NH / 2.0, b.y + NH / 2.0);
                    return json!({ "t": e.t, "p": [x, ay, x + o, ay, x + o, by, x, by], "s": -1 });
                }
                let fwd = b.x > a.x;
                let ax = if fwd { a.x + a.w } else { a.x };
                let bx = if fwd { b.x } else { b.x + b.w };
                let (ay, by) = (a.y + NH / 2.0, b.y + NH / 2.0);
                let dx = ((bx - ax).abs() * 0.5).max(24.0);
                let s = if fwd { 1.0 } else { -1.0 };
                json!({ "t": e.t, "p": [ax, ay, ax + s * dx, ay, bx - s * dx, by, bx, by], "s": s })
            })
            .collect();

        let node_json: Vec<Value> = nodes
            .iter()
            .map(|n| {
                let mut v = json!({ "kind": n.kind, "x": n.x, "y": n.y, "w": n.w, "h": NH });
                if let Some(i) = n.i {
                    let dim = if !self.ok[i] && n.kind != "center" { 0.28 } else if n.kind == "sib" { 0.5 } else { 1.0 };
                    v["id"] = json!(self.b[i].id);
                    v["title"] = json!(self.b[i].title);
                    v["st"] = json!(self.st[i]);
                    v["word"] = json!(WORDS[self.st[i] as usize]);
                    v["opacity"] = json!(dim);
                }
                if let Some(g) = &n.ghost {
                    v["ghost"] = json!(g);
                }
                if let Some((key, list)) = &n.more {
                    v["more"] = json!({ "key": key, "count": list.len() });
                }
                v
            })
            .collect();

        for d in &cols_data {
            let n = d.shown.len() + d.more.len() + d.ghosts.len();
            let hops = d.k.unsigned_abs() as usize;
            let mut text = if d.k == 0 {
                "this bead".to_string()
            } else {
                format!("{} · {}", if d.k < 0 { "blocked by" } else { "unlocks" }, plural(hops, "hop", "hops"))
            };
            if n > 1 {
                text.push_str(&format!(" · {n}"));
            }
            label(&mut labels, col_x(d.k), hdr_y + 8.0, text, true);
        }

        let pop = ui.more.as_deref().and_then(|key| {
            let n = nodes.iter().find(|n| n.more.as_ref().is_some_and(|(k, _)| k == key))?;
            let list = &n.more.as_ref()?.1;
            let wide = n.x + n.w + 306.0 < total + 2.0 * PAD;
            Some(json!({ "key": key, "x": if wide { n.x + n.w + 6.0 } else { (n.x - 306.0).max(4.0) },
                "y": (n.y - 6.0).max(4.0), "items": list.iter().map(|&i| self.item(i)).collect::<Vec<_>>() }))
        });

        let up = self.blk[c].len();
        let dn = self.unl[c].len();
        let m = self.miss[c].len();
        let mut caption = if up == 0 && dn == 0 && m == 0 {
            format!("{} has no dependencies: nothing blocks it and it blocks nothing, so it has only its parent and children.", self.b[c].id)
        } else {
            format!(
                "x: dependency direction, blocked by on the left and unlocks on the right, by hops. y: grouping, parent above, children below. {}, {}{}.",
                plural(up, "blocker", "blockers"),
                plural(dn, "unlock", "unlocks"),
                if m > 0 { format!(", {m} blocker{} missing from this repo", if m > 1 { "s" } else { "" }) } else { String::new() }
            )
        };
        if cyc {
            caption = format!("Dependency cycle: the red arrow runs against the flow. {caption}");
        }
        let cnode = &nodes[cn];
        json!({
            "mode": "focus", "width": total + 2.0 * PAD, "height": height, "lanes": lanes, "labels": labels,
            "edges": edge_json, "nodes": node_json, "pop": pop,
            "center": { "id": self.b[c].id, "x": cnode.x, "y": cnode.y, "w": cnode.w, "h": NH },
            "caption": caption, "captionBad": cyc, "cyc": cyc,
        })
    }

    fn lane_name(&self, epic: Option<usize>, depth: usize) -> String {
        match epic {
            Some(e) => format!("{}{}", if depth > 0 { "↳ " } else { "" }, self.b[e].title),
            None => "No epic".into(),
        }
    }

    /// The Graph view: every bead on two axes.
    fn whole(&self, ui: &Ui) -> Value {
        let n = self.n();
        let is_e = |i: usize| self.b[i].kind == "epic";
        let epic_of = |i: usize| -> Option<usize> {
            let mut p = self.par[i];
            let mut k = 0;
            while let Some(pp) = p {
                if k >= 60 {
                    break;
                }
                k += 1;
                if is_e(pp) {
                    return Some(pp);
                }
                p = self.par[pp];
            }
            None
        };
        let mut direct: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for i in 0..n {
            if !is_e(i) {
                direct.entry(epic_of(i)).or_default().push(i);
            }
        }
        // Lane order: epic trees by priority, a nested feature right after its parent epic.
        let mut e_kids: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut roots: Vec<usize> = vec![];
        for i in 0..n {
            if is_e(i) {
                match self.par[i] {
                    Some(p) if is_e(p) => e_kids.entry(p).or_default().push(i),
                    _ => roots.push(i),
                }
            }
        }
        let mut order: Vec<(usize, usize)> = vec![];
        let mut seen_e: HashSet<usize> = HashSet::new();
        fn dfs(m: &Model, e: usize, depth: usize, kids: &HashMap<usize, Vec<usize>>, seen: &mut HashSet<usize>, order: &mut Vec<(usize, usize)>) {
            if !seen.insert(e) {
                return;
            }
            order.push((e, depth));
            for k in m.sorted(kids.get(&e).cloned().unwrap_or_default()) {
                dfs(m, k, depth + 1, kids, seen, order);
            }
        }
        for e in self.sorted(roots) {
            dfs(self, e, 0, &e_kids, &mut seen_e, &mut order);
        }
        for i in 0..n {
            if is_e(i) && !seen_e.contains(&i) {
                dfs(self, i, 0, &e_kids, &mut seen_e, &mut order);
            }
        }
        struct Lane {
            epic: Option<usize>,
            depth: usize,
            beads: Vec<usize>,
        }
        let mut lanes: Vec<Lane> = vec![];
        for (e, depth) in order {
            if let Some(b) = direct.get(&Some(e)).filter(|b| !b.is_empty()) {
                lanes.push(Lane { epic: Some(e), depth, beads: b.clone() });
            }
        }
        if let Some(b) = direct.get(&None).filter(|b| !b.is_empty()) {
            lanes.push(Lane { epic: None, depth: 0, beads: b.clone() });
        }
        let mut lane_of: Vec<Option<usize>> = vec![None; n];
        let mut max_layer = 0usize;
        for (li, l) in lanes.iter().enumerate() {
            for &i in &l.beads {
                lane_of[i] = Some(li);
                max_layer = max_layer.max(self.rank[i] as usize);
            }
        }
        // Per lane: rows from its size, the beads of a layer wrapped into sub-columns of that many rows.
        let mut subw = vec![1usize; max_layer + 1];
        struct Shape {
            rows: usize,
            per: BTreeMap<usize, Vec<usize>>,
            max_layer: usize,
        }
        let mut shapes: Vec<Shape> = vec![];
        for l in &lanes {
            let mut per: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for &i in &l.beads {
                per.entry(self.rank[i] as usize).or_default().push(i);
            }
            let maxc = per.values().map(Vec::len).max().unwrap_or(0);
            let rows = maxc.min(2usize.max(((l.beads.len() as f64) / 2.5).sqrt().ceil() as usize)).max(1);
            let mut ml = 0;
            for (r, a) in &per {
                subw[*r] = subw[*r].max(a.len().div_ceil(rows));
                ml = ml.max(*r);
            }
            shapes.push(Shape { rows, per, max_layer: ml });
        }
        let mut lx = vec![0usize; max_layer + 2];
        for r in 0..=max_layer {
            lx[r + 1] = lx[r] + subw[r];
        }
        // node numbering: every bead that has a lane, in bead order
        let mut node_of: Vec<Option<usize>> = vec![None; n];
        let mut members: Vec<usize> = vec![];
        for i in 0..n {
            if lane_of[i].is_some() {
                node_of[i] = Some(members.len());
                members.push(i);
            }
        }
        let mut xs = vec![0.0f64; members.len()];
        let mut ys = vec![0.0f64; members.len()];
        let mut y = 0.0;
        let mut lane_json: Vec<Value> = vec![];
        for (l, s) in lanes.iter().zip(shapes.iter_mut()) {
            let h = LANE_HDR + s.rows as f64 * CH;
            let w = lx[s.max_layer + 1] as f64 * CW;
            let mut counts = [0usize; 7];
            for (r, a) in s.per.iter_mut() {
                a.sort_by(|&p, &q| self.by_pri(p, q));
                for (k, &i) in a.iter().enumerate() {
                    let ni = node_of[i].unwrap();
                    xs[ni] = (lx[*r] + k / s.rows) as f64 * CW;
                    ys[ni] = y + LANE_HDR + (k % s.rows) as f64 * CH;
                }
            }
            for &i in &l.beads {
                counts[self.st[i] as usize] += 1;
            }
            lane_json.push(json!({ "name": self.lane_name(l.epic, l.depth), "y0": y, "h": h, "w": w,
                "counts": counts, "n": l.beads.len() }));
            y += h + LANE_GAP;
        }
        let (mut ea, mut eb) = (vec![], vec![]);
        for &i in &members {
            for &j in &self.blk[i] {
                if let (Some(a), Some(b)) = (node_of[j], node_of[i]) {
                    ea.push(a);
                    eb.push(b);
                }
            }
        }
        let w_total = (lx[max_layer + 1] as f64 * CW).max(CW);
        let h_total = (y - LANE_GAP).max(CH);

        let sel = ui.sel.as_deref().and_then(|s| self.idx.get(s).copied()).filter(|i| lane_of[*i].is_some());
        let marks: Vec<usize> = match sel {
            None => vec![],
            Some(s) => {
                let mut m = vec![false; n];
                m[s] = true;
                for adj in [&self.blk, &self.unl] {
                    let mut q = vec![s];
                    let mut seen = vec![false; n];
                    seen[s] = true;
                    let mut h = 0;
                    while h < q.len() {
                        for &v in &adj[q[h]] {
                            if !seen[v] {
                                seen[v] = true;
                                m[v] = true;
                                q.push(v);
                            }
                        }
                        h += 1;
                    }
                }
                (0..n).filter(|&i| m[i]).filter_map(|i| node_of[i]).collect()
            }
        };
        let base = format!(
            "x: dependency layer (0 = no blockers), {}. y: one lane per epic, {}.",
            plural(max_layer + 1, "layer", "layers"),
            plural(lanes.len(), "lane", "lanes")
        );
        let caption = match sel {
            Some(s) => {
                if self.blk[s].is_empty() && self.unl[s].is_empty() {
                    format!("{} has no dependencies: it sits in its lane at layer {}.", self.b[s].id, self.rank[s])
                } else {
                    format!(
                        "{}: {} beads upstream and downstream highlighted, the rest dimmed. Return opens it.",
                        self.b[s].id,
                        marks.len().saturating_sub(1)
                    )
                }
            }
            None if ea.len() > 3500 => format!("{base} Dependency lines are hidden above 3500; select a bead to see its own."),
            None => base,
        };
        let filtered = self.ok.iter().any(|o| !o);
        json!({
            "mode": "graph", "W": w_total, "H": h_total, "lanes": lane_json,
            "ids": members.iter().map(|&i| &self.b[i].id).collect::<Vec<_>>(),
            "titles": members.iter().map(|&i| &self.b[i].title).collect::<Vec<_>>(),
            "x": xs, "y": ys,
            "st": members.iter().map(|&i| self.st[i]).collect::<Vec<_>>(),
            "lane": members.iter().map(|&i| lane_of[i].unwrap()).collect::<Vec<_>>(),
            "rank": members.iter().map(|&i| self.rank[i]).collect::<Vec<_>>(),
            "dim": members.iter().map(|&i| u8::from(!self.ok[i])).collect::<Vec<_>>(),
            "ea": ea, "eb": eb, "filtered": filtered,
            "sel": sel.and_then(|s| node_of[s]).map(|v| v as i64).unwrap_or(-1),
            "marks": marks, "caption": caption, "captionBad": false,
            "selected": sel.is_some(),
        })
    }

    fn layout(&self, ui: &Ui) -> Value {
        if ui.view == "graph" { self.whole(ui) } else { self.focus(ui, ui.cw) }
    }

    /// The Focus or Graph view as ordinary components: the toolbar, the caption, the generic Canvas
    /// holding the drawing, the legend and, in Focus, a sheet listing what a "+ N more" node hides.
    /// Returns the ids to put on the page, top to bottom.
    pub(super) fn components(&self, p: &mut Page, ui: &Ui) -> Vec<String> {
        let focus = ui.view != "graph";
        let layout = self.layout(ui);
        let mut ids = vec![];
        let mut bar: Vec<String> = vec![];
        if focus {
            bar.push(p.button("graph.back", "‹ Back", "focus_back", json!({}), json!({ "disabled": ui.fhist.is_empty() })));
            p.choose("graph.depth", "/set_depth", &ui.depth.to_string(), vec![
                json!({ "value": "1", "text": "1 hop" }), json!({ "value": "2", "text": "2 hops" }), json!({ "value": "3", "text": "3 hops" })], true, false);
            bar.push("graph.depth".into());
        }
        if focus {
            ids.push(p.boxed("graph.bar", "h", bar, json!({ "spacing": 8, "padding": [10, 0, 4, 0] })));
        }
        let caption = layout["caption"].as_str().unwrap_or("");
        let tone = if layout["captionBad"].as_bool() == Some(true) { "bad" } else { "muted" };
        ids.push(p.label("graph.caption", caption, json!({ "tone": tone, "size": 12, "padding": [0, 2, 8, 2] })));
        if self.n() == 0 || layout["empty"].is_string() {
            let text = if self.n() == 0 { "No beads yet" } else { layout["empty"].as_str().unwrap_or("") };
            ids.push(p.label("graph.empty", text, json!({ "tone": "muted", "padding": [12, 12, 12, 12] })));
        } else {
            let mut canvas = if focus { focus_scene(&layout) } else { whole_scene(&layout, ui.centre) };
            let o = canvas.as_object_mut().unwrap();
            o.insert("id".into(), json!("graph.canvas"));
            o.insert("component".into(), json!("Canvas"));
            o.insert("minHeight".into(), json!(380));
            o.insert("reserve".into(), json!(400));
            o.insert("escapeAction".into(), json!("escape"));
            o.insert("fitKey".into(), json!(if focus { "focus" } else { "graph" }));
            if focus {
                o.insert("sizeAction".into(), json!("set_width"));
                o.insert("sizeKnown".into(), json!(ui.cw));
            }
            p.put(canvas);
            ids.push("graph.canvas".into());
        }
        let mut runs: Vec<Value> = vec![];
        for c in 0..7 {
            runs.push(json!({ "text": format!("{} ", GLYPH[c]), "tone": TONE[c] }));
            runs.push(json!({ "text": format!("{}    ", WORDS[c]), "tone": TONE[c], "weight": "semibold" }));
        }
        let hint = if focus {
            format!("solid arrow: blocks · dotted line: parent and child{}", if layout["cyc"].as_bool() == Some(true) { " · red: dependency cycle" } else { "" })
        } else {
            "one dot per bead · drag to pan, wheel or pinch to zoom".into()
        };
        let l = p.runs("graph.words", json!(runs), json!({ "size": 12 }));
        let h = p.label("graph.hint", &hint, json!({ "tone": "muted", "size": 12 }));
        ids.push(p.boxed("graph.legend", "h", vec![l, h], json!({ "spacing": 8, "padding": [10, 2, 0, 2], "justify": "between" })));
        if let Some(items) = layout["pop"]["items"].as_array() {
            self.more_sheet(p, items);
            ids.push("graph.more".into());
        } else {
            p.put(json!({ "id": "graph.more", "component": "Modal", "presented": false, "dismissAction": "escape" }));
        }
        ids
    }

    /// The sheet behind a "+ N more" node: one row per hidden bead, a pick centres Focus on it.
    fn more_sheet(&self, p: &mut Page, items: &[Value]) {
        let rows: Vec<Value> = items
            .iter()
            .filter_map(|it| it["id"].as_str())
            .filter_map(|id| self.index(id))
            .map(|i| {
                let b = self.b[i];
                json!({ "id": b.id, "path": b.id, "depth": 0, "kids": 0, "collapsed": false, "dim": false, "sel": false,
                    "cells": [
                        { "key": "id", "text": b.id, "mono": true, "tone": "muted" },
                        { "key": "title", "text": b.title },
                        { "key": "status", "text": WORDS[self.st[i] as usize], "chip": true, "tone": tone_of(kind_of(b)) },
                    ], "menu": [] })
            })
            .collect();
        let columns = json!([
            { "key": "id", "label": "ID", "width": 150, "tree": false, "wide": false },
            { "key": "title", "label": "Title", "width": null, "tree": false, "wide": false },
            { "key": "status", "label": "Status", "width": 110, "tree": false, "wide": false },
        ]);
        let table = json!({ "id": "graph.more.t", "component": "Table", "rowAction": "focus_on", "foldAction": "", "columns": columns,
            "sort": { "key": "", "asc": true }, "sections": [{ "title": "", "count": rows.len(), "rows": rows }], "empty": "" });
        let title = json!({ "id": "graph.more.h", "component": "Text", "text": format!("{} more beads", rows.len()), "modifiers": [{ "fontWeight": "semibold" }] });
        p.put(table);
        p.put(title);
        p.put(json!({ "id": "graph.more.b", "component": "Column", "spacing": 8, "children": ["graph.more.h", "graph.more.t"] }));
        p.put(json!({ "id": "graph.more", "component": "Modal", "presented": true, "dismissAction": "escape", "children": ["graph.more.b"] }));
    }

    /// The pane's focus history, cut to beads that still exist.
    pub(super) fn trim(&self, ui: &mut Ui) {
        ui.fhist.retain(|h| self.idx.contains_key(h.as_str()));
        if ui.sel.as_deref().is_some_and(|s| !self.idx.contains_key(s)) {
            ui.sel = None;
        }
    }
}

const GLYPH: [&str; 7] = ["●", "◐", "⊘", "◆", "◌", "○", "✓"];
const TONE: [&str; 7] = ["ok", "accent", "warn", "purple", "faint", "muted", "muted"];
/// A graph cell's node box and dot, in scene units: the cell is CW x CH, the box leaves 14 and 5 of it.
const BW: f64 = CW - 14.0;
const BH: f64 = CH - 5.0;
const DOT: f64 = CH * 0.26;
/// The zoom below which a lane is one bar, and below which a bead is a dot rather than a box.
const LANES_BELOW: f64 = 0.05;
const DOTS_BELOW: f64 = 0.75;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array().map(|a| a.iter().map(num).collect()).unwrap_or_default()
}

fn svg_curve(p: [f64; 8]) -> String {
    format!("M{} {} C{} {} {} {} {} {}", r1(p[0]), r1(p[1]), r1(p[2]), r1(p[3]), r1(p[4]), r1(p[5]), r1(p[6]), r1(p[7]))
}

fn arrow_head(x: f64, y: f64, s: f64, len: f64) -> String {
    format!("M{} {} L{} {} L{} {} Z", r1(x), r1(y), r1(x - len * s), r1(y - 3.5), r1(x - len * s), r1(y + 3.5))
}

fn tip_bead(id: &str, st: usize, title: &str, last: Option<String>) -> Value {
    let mut tip = vec![
        json!([{ "t": id, "mono": true, "tone": "muted" }, { "t": format!("  {}", WORDS[st]), "bold": true, "tone": TONE[st] }]),
        json!(title),
    ];
    tip.extend(last.map(|l| json!([{ "t": l, "tone": "muted" }])));
    Value::Array(tip)
}

/// The Focus layout as a scene for the Canvas: scene units are points, drawn at zoom 1 and moved by pan.
fn focus_scene(l: &Value) -> Value {
    let (width, height) = (num(&l["width"]), num(&l["height"]));
    let mut ops: Vec<Value> = vec![];
    let mut hits: Vec<Value> = vec![];
    let lanes: Vec<Value> = l["lanes"].as_array().into_iter().flatten().map(|x| json!([x["x"], x["y"], x["w"], x["h"]])).collect();
    ops.push(json!({ "op": "rect", "r": lanes, "fill": "fill", "opacity": 0.5, "radius": 8 }));
    for bold in [true, false] {
        let t: Vec<Value> = l["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["bold"].as_bool().unwrap_or(false) == bold)
            .map(|x| {
                let text = x["text"].as_str().unwrap_or("");
                json!([x["x"], x["y"], if bold { text.to_uppercase() } else { text.to_string() }])
            })
            .collect();
        ops.push(json!({ "op": "text", "t": t, "size": 11, "px": true, "fill": "muted", "weight": if bold { "semibold" } else { "regular" } }));
    }
    // edges: a curve each, with an arrowhead for a dependency
    let mut paths: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut heads: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for e in l["edges"].as_array().into_iter().flatten() {
        let p = nums(&e["p"]);
        if p.len() != 8 {
            continue;
        }
        let t = match e["t"].as_str() { Some("par") => "par", Some("cyc") => "cyc", _ => "dep" };
        paths.entry(t).or_default().push(svg_curve([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]]));
        if t != "par" {
            heads.entry(t).or_default().push(arrow_head(p[6], p[7], e["s"].as_f64().unwrap_or(1.0), 7.0));
        }
    }
    for (t, d) in &paths {
        let (stroke, opacity) = match *t { "par" => ("muted", 1.0), "cyc" => ("bad", 1.0), _ => ("muted", 0.75) };
        let mut op = json!({ "op": "path", "d": d, "stroke": stroke, "opacity": opacity, "width": 1.3, "px": true });
        if *t == "par" {
            op["dash"] = json!([1.5, 4]);
        }
        ops.push(op);
    }
    for (t, d) in &heads {
        ops.push(json!({ "op": "path", "d": d, "fill": if *t == "cyc" { "bad" } else { "muted" } }));
    }
    // node boxes and their words, grouped by how faint they are
    let mut boxes: BTreeMap<(u8, i64), Vec<Value>> = BTreeMap::new();
    let mut glyphs: BTreeMap<(i64, usize), Vec<Value>> = BTreeMap::new();
    let mut titles: BTreeMap<(i64, i64), Vec<Value>> = BTreeMap::new();
    let (mut ghost_boxes, mut more_boxes, mut more_text) = (vec![], vec![], vec![]);
    for n in l["nodes"].as_array().into_iter().flatten() {
        let (x, y, w, h) = (num(&n["x"]), num(&n["y"]), num(&n["w"]), num(&n["h"]));
        let rect = json!([x, y, w, h]);
        let cy = y + h / 2.0;
        let kind = n["kind"].as_str().unwrap_or("");
        let word_ctx = |n: &Value| (n["id"].as_str().unwrap_or("").to_string(), n["title"].as_str().unwrap_or("").to_string());
        if let Some(g) = n["ghost"].as_str() {
            ghost_boxes.push(rect);
            ops.push(json!({ "op": "text", "t": [[x + 10.0, cy, format!("missing: {g}")]], "size": 12, "px": true, "fill": "faint", "maxw": w - 20.0 }));
            hits.push(json!({ "id": g, "rect": [x, y, w, h], "label": format!("missing: {g}"),
                "tip": [format!("{g} is named as a blocker but is not in this repo")] }));
        } else if let Some(m) = n["more"].as_object() {
            let key = m["key"].as_str().unwrap_or("");
            let count = m["count"].as_u64().unwrap_or(0);
            more_boxes.push(rect);
            more_text.push(json!([x + 10.0, cy, format!("+ {count} more")]));
            hits.push(json!({ "id": key, "rect": [x, y, w, h], "action": "graph_more", "context": { "key": key },
                "label": format!("{count} more beads"), "tip": [format!("{count} more beads")] }));
        } else {
            let (id, title) = word_ctx(n);
            let st = (num(&n["st"]) as usize).min(6);
            let op = (num(&n["opacity"]) * 100.0).round() as i64;
            boxes.entry((u8::from(kind == "center"), op)).or_default().push(rect);
            glyphs.entry((op, st)).or_default().push(json!([x + 9.0, cy + 0.5, GLYPH[st]]));
            titles.entry((op, w.round() as i64)).or_default().push(json!([x + 27.0, cy + 0.5, title.clone()]));
            let word = n["word"].as_str().unwrap_or("");
            let mut hit = json!({ "id": id, "rect": [x, y, w, h], "doubleAction": "open_bead",
                "label": format!("{id}, {word}, {title}"), "tip": tip_bead(&id, st, &title, None) });
            if kind != "center" {
                hit["action"] = json!("focus_on");
            }
            hits.push(hit);
        }
    }
    for ((centre, op), r) in &boxes {
        let (fill, stroke, width) = if *centre == 1 { ("accentFill", "accent", 1.5) } else { ("group", "line", 1.0) };
        ops.push(json!({ "op": "rect", "r": r, "fill": fill, "stroke": stroke, "width": width, "px": true, "radius": 7, "opacity": *op as f64 / 100.0 }));
    }
    ops.push(json!({ "op": "rect", "r": ghost_boxes, "stroke": "line", "width": 1, "px": true, "dash": [3, 3], "radius": 7 }));
    ops.push(json!({ "op": "rect", "r": more_boxes, "fill": "group", "stroke": "line", "width": 1, "px": true, "radius": 7 }));
    ops.push(json!({ "op": "text", "t": more_text, "size": 12, "px": true, "fill": "accent", "weight": "semibold" }));
    for ((op, st), t) in &glyphs {
        ops.push(json!({ "op": "text", "t": t, "size": 12, "px": true, "fill": TONE[*st], "opacity": *op as f64 / 100.0 }));
    }
    for ((op, w), t) in &titles {
        ops.push(json!({ "op": "text", "t": t, "size": 12, "px": true, "fill": "fg", "maxw": *w as f64 - 36.0, "opacity": *op as f64 / 100.0 }));
    }
    let c = &l["center"];
    let nodes = l["nodes"].as_array().map(Vec::len).unwrap_or(0);
    json!({ "ops": ops, "hits": hits, "bounds": [0, 0, width, height], "controls": false, "wheel": "pan",
        "focus": { "rect": [c["x"], c["y"], c["w"], c["h"]], "zoom": 1, "key": format!("f:{}:{}:{}", c["id"].as_str().unwrap_or(""), width, nodes) } })
}

/// The Graph layout as a scene for the Canvas: three layers by zoom, a bar per lane, a dot per bead, a box per bead.
fn whole_scene(l: &Value, centre: bool) -> Value {
    let lanes = l["lanes"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = l["ids"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap_or("")).collect();
    let titles: Vec<&str> = l["titles"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap_or("")).collect();
    let (xs, ys) = (nums(&l["x"]), nums(&l["y"]));
    let st: Vec<usize> = nums(&l["st"]).iter().map(|v| (*v as usize).min(6)).collect();
    let lane: Vec<usize> = nums(&l["lane"]).iter().map(|v| *v as usize).collect();
    let rank = nums(&l["rank"]);
    let dimmed: Vec<bool> = nums(&l["dim"]).iter().map(|v| *v != 0.0).collect();
    let (ea, eb): (Vec<usize>, Vec<usize>) = (nums(&l["ea"]).iter().map(|v| *v as usize).collect(), nums(&l["eb"]).iter().map(|v| *v as usize).collect());
    let sel = l["sel"].as_i64().filter(|s| *s >= 0).map(|s| s as usize);
    let mut marked = vec![false; ids.len()];
    for m in nums(&l["marks"]) {
        if let Some(x) = marked.get_mut(m as usize) {
            *x = true;
        }
    }
    let has_marks = sel.is_some();
    let dim = |i: usize| (has_marks && !marked[i]) || dimmed[i];
    let filtered = l["filtered"].as_bool().unwrap_or(false);
    let (w, h) = (num(&l["W"]), num(&l["H"]));
    let mut ops: Vec<Value> = vec![];
    let mut hits: Vec<Value> = vec![];
    let dots = |op: Value| -> Value {
        let mut op = op;
        op["minZoom"] = json!(LANES_BELOW);
        op["maxZoom"] = json!(DOTS_BELOW);
        op
    };
    let boxes = |op: Value| -> Value {
        let mut op = op;
        op["minZoom"] = json!(DOTS_BELOW);
        op
    };

    // lanes: a bar each when far out, a tinted band with its name nearer
    let named = [(0usize, "ready"), (1, "in progress"), (2, "blocked"), (3, "need you"), (6, "done")];
    let mut bars: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    let mut bands = vec![];
    let mut heads = vec![];
    for lane_v in &lanes {
        let (y0, lh, lw) = (num(&lane_v["y0"]), num(&lane_v["h"]), num(&lane_v["w"]));
        let counts: Vec<usize> = nums(&lane_v["counts"]).iter().map(|v| *v as usize).collect();
        let n = (num(&lane_v["n"]) as usize).max(1);
        let name = lane_v["name"].as_str().unwrap_or("");
        let full = lw + 12.0;
        let mut acc = -6.0;
        for code in [6usize, 3, 1, 2, 0, 5, 4] {
            let c = counts.get(code).copied().unwrap_or(0);
            if c == 0 {
                continue;
            }
            let seg = full.max(CW) * c as f64 / n as f64;
            bars.entry(code).or_default().push(json!([r1(acc), r1(y0 + lh * 0.075), r1(seg), r1(lh * 0.85)]));
            acc += seg;
        }
        bands.push(json!([-6, r1(y0), r1(full), r1(lh)]));
        heads.push(json!([2, r1(y0 + LANE_HDR / 2.0), format!("{name} · {}", lane_v["n"])]));
        let mut summary = vec![format!("{} beads", lane_v["n"])];
        summary.extend(named.iter().filter(|(c, _)| counts.get(*c).copied().unwrap_or(0) > 0).map(|(c, wd)| format!("{} {wd}", counts[*c])));
        hits.push(json!({ "id": format!("lane:{name}"), "rect": [-6, r1(y0), r1(full.max(CW * 3.0)), r1(lh)], "slop": 3, "maxZoom": LANES_BELOW,
            "fit": [-6, r1(y0), r1(lw.max(CW * 3.0) + 12.0), r1(lh)], "label": name,
            "tip": [[{ "t": name, "bold": true }], summary.join(" · "), [{ "t": "click to zoom to this lane", "tone": "muted" }]] }));
    }
    for code in [6usize, 3, 1, 2, 0, 5, 4] {
        if let Some(r) = bars.get(&code) {
            ops.push(json!({ "op": "rect", "r": r, "fill": TONE[code], "opacity": if code == 6 { 0.35 } else { 0.85 },
                "hpx": [1.5, 26], "maxZoom": LANES_BELOW }));
        }
    }
    ops.push(json!({ "op": "rect", "r": bands, "fill": "fill", "opacity": 0.5, "radius": 8, "minZoom": LANES_BELOW }));
    ops.push(json!({ "op": "text", "t": heads, "size": 11, "px": true, "fill": "muted", "minZoom": 9.0 / LANE_HDR, "maxw": 400, "minx": 8 }));

    // edges: the ones a selection lights up, or all of them unless a filter dims an end
    let mut plain = vec![];
    let mut hi = vec![];
    for k in 0..ea.len() {
        let (a, b) = (ea[k], eb[k]);
        if has_marks {
            if marked[a] && marked[b] {
                hi.push(k);
            }
        } else if !(filtered && (dimmed[a] || dimmed[b])) {
            plain.push(k);
        }
    }
    let edge = |k: usize, boxed: bool| -> String {
        let (a, b) = (ea[k], eb[k]);
        let (x1, y1, x2, y2) = if boxed {
            (xs[a] + BW, ys[a] + BH / 2.0, xs[b], ys[b] + BH / 2.0)
        } else {
            (xs[a] + DOT, ys[a] + DOT, xs[b] + DOT, ys[b] + DOT)
        };
        let dx = ((x2 - x1).abs() * 0.5).max(10.0);
        svg_curve([x1, y1, x1 + dx, y1, x2 - dx, y2, x2, y2])
    };
    if plain.len() <= 3500 && !plain.is_empty() {
        ops.push(dots(json!({ "op": "path", "d": plain.iter().map(|k| edge(*k, false)).collect::<Vec<_>>(), "stroke": "muted", "opacity": 0.3, "width": 1, "px": true })));
        ops.push(boxes(json!({ "op": "path", "d": plain.iter().map(|k| edge(*k, true)).collect::<Vec<_>>(), "stroke": "muted", "opacity": 0.55, "width": 1, "px": true })));
    }
    if !hi.is_empty() {
        ops.push(dots(json!({ "op": "path", "d": hi.iter().map(|k| edge(*k, false)).collect::<Vec<_>>(), "stroke": "accent", "width": 1.3, "px": true })));
        ops.push(boxes(json!({ "op": "path", "d": hi.iter().map(|k| edge(*k, true)).collect::<Vec<_>>(), "stroke": "accent", "width": 1.6, "px": true })));
        if hi.len() < 400 {
            let heads: Vec<String> = hi.iter().map(|k| arrow_head(xs[eb[*k]], ys[eb[*k]] + BH / 2.0, 1.0, 6.0)).collect();
            ops.push(boxes(json!({ "op": "path", "d": heads, "fill": "accent" })));
        }
    }

    // a dot per bead far in, faint when dimmed
    for pass in [false, true] {
        for code in 0..7 {
            let c: Vec<Value> = (0..ids.len()).filter(|&i| st[i] == code && dim(i) == pass).map(|i| json!([r1(xs[i] + DOT), r1(ys[i] + DOT), r1(DOT)])).collect();
            if !c.is_empty() {
                let opacity = if pass { 0.16 } else if code == 6 { 0.5 } else { 1.0 };
                ops.push(dots(json!({ "op": "circle", "c": c, "fill": TONE[code], "opacity": opacity, "rpx": [1.4, 5.5] })));
            }
        }
    }
    if let Some(i) = sel {
        ops.push(dots(json!({ "op": "circle", "c": [[r1(xs[i] + DOT), r1(ys[i] + DOT), r1(DOT)]], "stroke": "accent", "width": 2, "px": true, "rpx": [4.4, 8.5] })));
    }
    // a box per bead near in
    let mut rects: BTreeMap<(bool, bool), Vec<Value>> = BTreeMap::new();
    let mut glyphs: BTreeMap<(bool, usize), Vec<Value>> = BTreeMap::new();
    let mut words: BTreeMap<bool, Vec<Value>> = BTreeMap::new();
    for i in 0..ids.len() {
        let (x, y, d) = (xs[i], ys[i], dim(i));
        rects.entry((Some(i) == sel, d)).or_default().push(json!([r1(x), r1(y), BW, BH]));
        glyphs.entry((d, st[i])).or_default().push(json!([r1(x + 6.0), r1(y + BH / 2.0 + 0.5), GLYPH[st[i]]]));
        words.entry(d).or_default().push(json!([r1(x + 20.0), r1(y + BH / 2.0 + 0.5), titles[i]]));
    }
    for ((selected, d), r) in &rects {
        let (fill, stroke, width) = if *selected { ("accentFill", "accent", 2.0) } else { ("group", "line", 1.0) };
        ops.push(boxes(json!({ "op": "rect", "r": r, "fill": fill, "stroke": stroke, "width": width, "px": true, "radius": 6, "opacity": if *d { 0.2 } else { 1.0 } })));
    }
    for ((d, code), t) in &glyphs {
        ops.push(boxes(json!({ "op": "text", "t": t, "size": 11, "px": true, "fill": TONE[*code], "opacity": if *d { 0.2 } else { 1.0 } })));
    }
    for (d, t) in &words {
        ops.push(boxes(json!({ "op": "text", "t": t, "size": 11, "px": true, "fill": "fg", "maxw": BW - 26.0, "opacity": if *d { 0.2 } else { 1.0 } })));
    }
    // a hit per bead: a circle far in, a box near in
    for i in 0..ids.len() {
        let lane_name = lanes.get(lane[i]).and_then(|v| v["name"].as_str()).unwrap_or("");
        let tip = tip_bead(ids[i], st[i], titles[i], Some(format!("{lane_name} · layer {}", rank[i])));
        let one = |shape: Value, min: f64, max: f64| {
            let mut v = json!({ "id": ids[i], "action": "open_bead", "tip": tip,
                "label": format!("{}, {}, {}", ids[i], WORDS[st[i]], titles[i]), "minZoom": min });
            v.as_object_mut().unwrap().extend(shape.as_object().unwrap().clone());
            if max.is_finite() {
                v["maxZoom"] = json!(max);
            }
            v
        };
        hits.push(one(json!({ "circle": [r1(xs[i] + DOT), r1(ys[i] + DOT), r1(DOT)], "rpx": [1.4, 5.5], "slop": 4 }), LANES_BELOW, DOTS_BELOW));
        hits.push(one(json!({ "rect": [r1(xs[i]), r1(ys[i]), BW, BH] }), DOTS_BELOW, f64::INFINITY));
    }
    let mut v = json!({ "ops": ops, "hits": hits, "bounds": [0, 0, w, h], "controls": true, "wheel": "zoom",
        "emptyAction": "graph_select", "emptyContext": { "id": "" } });
    if let Some(i) = sel {
        v["enterAction"] = json!("open_bead");
        v["enterContext"] = json!({ "id": ids[i] });
        if centre {
            v["focus"] = json!({ "rect": [r1(xs[i]), r1(ys[i]), CW, CH], "zoom": 0.9, "key": format!("g:{}", ids[i]) });
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(id: &str, f: impl Fn(&mut Bead)) -> Bead {
        let mut b = Bead {
            id: id.into(), title: format!("title {id}"), status: "open".into(), kind: "task".into(), priority: Some(2),
            parent: None, labels: vec![], owner: None, updated: String::new(), blocked_by: vec![],
            description: String::new(), ready: false, epic: false,
        };
        f(&mut b);
        b
    }

    fn snap(beads: Vec<Bead>) -> Snapshot {
        Snapshot { name: "t".into(), path: "/t".into(), beads, has_repo: true, ..Default::default() }
    }

    fn chain() -> Snapshot {
        snap(vec![
            bead("e", |b| b.kind = "epic".into()),
            bead("a", |b| b.parent = Some("e".into())),
            bead("b", |b| { b.parent = Some("e".into()); b.blocked_by = vec!["a".into()] }),
            bead("c", |b| { b.parent = Some("e".into()); b.blocked_by = vec!["b".into(), "gone".into()] }),
            bead("d", |_| {}),
        ])
    }

    #[test]
    fn layers_are_the_longest_path_from_a_bead_with_no_blockers() {
        let s = chain();
        let m = Model::new(&s, &Ui::default());
        let r: Vec<i32> = ["a", "b", "c", "d"].iter().map(|id| m.rank[m.index(id).unwrap()]).collect();
        assert_eq!(r, [0, 1, 2, 0]);
    }

    #[test]
    fn a_cycle_still_gets_layers() {
        let s = snap(vec![
            bead("a", |b| b.blocked_by = vec!["b".into()]),
            bead("b", |b| b.blocked_by = vec!["a".into()]),
        ]);
        let m = Model::new(&s, &Ui::default());
        assert_eq!(m.rank.len(), 2);
    }

    #[test]
    fn focus_puts_blockers_left_and_unlocks_right() {
        let s = chain();
        let ui = Ui { sel: Some("b".into()), view: "focus".into(), ..Default::default() };
        let m = Model::new(&s, &ui);
        let v = m.layout(&ui);
        let nodes = v["nodes"].as_array().unwrap();
        let x = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap()["x"].as_f64().unwrap();
        assert!(x("a") < x("b") && x("b") < x("c"));
        assert_eq!(v["center"]["id"], "b");
        // the parent epic sits in the band above
        assert!(nodes.iter().any(|n| n["id"] == "e" && n["kind"] == "parent"));
    }

    #[test]
    fn focus_names_a_blocker_missing_from_the_repo() {
        let s = chain();
        let ui = Ui { sel: Some("c".into()), view: "focus".into(), ..Default::default() };
        let v = Model::new(&s, &ui).layout(&ui);
        assert!(v["nodes"].as_array().unwrap().iter().any(|n| n["ghost"] == "gone"));
        assert!(v["caption"].as_str().unwrap().contains("1 blocker missing"));
    }

    #[test]
    fn graph_has_one_lane_per_epic_and_one_for_no_epic() {
        let s = chain();
        let ui = Ui { view: "graph".into(), ..Default::default() };
        let v = Model::new(&s, &ui).layout(&ui);
        let lanes = v["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[1]["name"], "No epic");
        assert_eq!(v["ids"].as_array().unwrap().len(), 4);
        assert_eq!(v["ea"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn selecting_a_bead_marks_what_it_blocks_and_what_blocks_it() {
        let s = chain();
        let ui = Ui { view: "graph".into(), sel: Some("b".into()), ..Default::default() };
        let v = Model::new(&s, &ui).layout(&ui);
        assert_eq!(v["marks"].as_array().unwrap().len(), 3);
        assert!(v["caption"].as_str().unwrap().contains("2 beads upstream and downstream"));
    }

    #[test]
    fn the_focus_scene_draws_every_node_and_hits_the_ones_that_act() {
        let s = chain();
        let ui = Ui { sel: Some("b".into()), view: "focus".into(), ..Default::default() };
        let m = Model::new(&s, &ui);
        let v = focus_scene(&m.layout(&ui));
        let hits = v["hits"].as_array().unwrap();
        let hit = |id: &str| hits.iter().find(|h| h["id"] == id).unwrap();
        assert_eq!(hit("a")["action"], "focus_on");
        assert!(hit("b").get("action").is_none(), "the centre is not a focus target");
        assert_eq!(hit("b")["doubleAction"], "open_bead");
        assert_eq!(hit("b")["label"], "b, blocked, title b");
        assert_eq!(v["focus"]["zoom"], 1);
        assert_eq!(v["controls"], false);
    }

    #[test]
    fn the_graph_scene_has_three_zoom_layers_and_a_hit_per_bead_in_each() {
        let s = chain();
        let ui = Ui { view: "graph".into(), sel: Some("b".into()), centre: true, ..Default::default() };
        let m = Model::new(&s, &ui);
        let v = whole_scene(&m.layout(&ui), true);
        let ops = v["ops"].as_array().unwrap();
        assert!(ops.iter().any(|o| o["maxZoom"] == LANES_BELOW && o["hpx"].is_array()), "lane bars");
        assert!(ops.iter().any(|o| o["op"] == "circle" && o["rpx"] == json!([1.4, 5.5])), "dots");
        assert!(ops.iter().any(|o| o["minZoom"] == DOTS_BELOW && o["op"] == "rect"), "boxes");
        let hits = v["hits"].as_array().unwrap();
        assert_eq!(hits.iter().filter(|h| h["id"] == "b").count(), 2);
        assert_eq!(v["enterContext"]["id"], "b");
        assert_eq!(v["focus"]["key"], "g:b");
    }

    #[test]
    fn a_more_node_opens_a_sheet_listing_what_it_hides() {
        let mut beads = vec![bead("c0", |_| {})];
        for i in 0..15 {
            beads.push(bead(&format!("d{i}"), |b| b.blocked_by = vec!["c0".into()]));
        }
        let s = snap(beads);
        let ui = Ui { sel: Some("c0".into()), view: "focus".into(), more: Some("R1".into()), ..Default::default() };
        let m = Model::new(&s, &ui);
        let mut p = Page { out: vec![] };
        let ids = m.components(&mut p, &ui);
        assert!(ids.contains(&"graph.more".to_string()));
        let sheet = p.out.iter().find(|c| c["id"] == "graph.more").unwrap();
        assert_eq!(sheet["dismissAction"], "escape");
        let table = p.out.iter().find(|c| c["id"] == "graph.more.t").unwrap();
        assert_eq!(table["sections"][0]["rows"].as_array().unwrap().len(), 2);
        assert_eq!(table["rowAction"], "focus_on");
    }
}
