//! A plugin pane's view as the client holds it (ADR-0052): one [`ViewNode`] per component the
//! script's `view` returned, kept by id across renders. [`ViewTree::reconcile`] takes each new
//! `view` and writes only what changed.
//!
//! Identity: the root is `root`; a child is `<parent>/=<key>` when it has a `key` (`%`, `/` and
//! `@` in it percent-escaped), else `<parent>/<Component><n>` with n its place among siblings of
//! the same component; a component inside a property is `<parent>@<property path>` (`root@child`,
//! `root/Table0/b-12@cells.2.view`); a Table's row is `<table>/<row path, else row id, else
//! section.row>`. A node whose component changes is built again. A node keeps its `serial` while
//! its identity survives, so two reads of a node are the same node when id and serial match.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value as Json};

use crate::PaneNode;

/// A node another node holds: its id and the serial it had when it was placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub id: String,
    pub serial: u64,
}

/// One component of the view.
#[derive(Clone, Debug)]
pub struct ViewNode {
    pub id: String,
    pub component: String,
    /// Which build of this id the node is: a node built again gets a new one.
    pub serial: u64,
    /// The component's properties as the script wrote them, with every component inside a
    /// property replaced by its id, a Table's `sections[].rows` by its row ids, and a bound
    /// control's `value` taken out (`draft`).
    pub props: Map<String, Json>,
    pub children: Vec<Ref>,
    /// The components inside properties, by property path (`child`, `actions.0`, `cells.2.view`).
    pub embedded: BTreeMap<String, Ref>,
    /// A Table's rows, in order, each a `TableRow` node whose props are the row the script wrote.
    pub rows: Vec<Ref>,
    /// What a control that holds a value shows: the last value the person or the script gave it.
    pub draft: Option<Json>,
    /// The script's `value` at the last render, for a bound control: a new one replaces the
    /// draft, the same one leaves it.
    given: Option<Json>,
    /// A Table row's own `id` field, read when a selection names rows.
    pub row_id: String,
}

impl ViewNode {
    fn new(id: String, component: String, serial: u64) -> Self {
        Self {
            id,
            component,
            serial,
            props: Map::new(),
            children: Vec::new(),
            embedded: BTreeMap::new(),
            rows: Vec::new(),
            draft: None,
            given: None,
            row_id: String::new(),
        }
    }

    /// The ids drawn as this component's content: its children, then its `child`, `trigger` and
    /// `content`.
    pub fn parts(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.children.iter().map(|r| r.id.as_str()).collect();
        for key in ["child", "trigger", "content"] {
            if let Some(r) = self.embedded.get(key) {
                out.push(&r.id);
            }
        }
        out
    }

    /// What a control shows: its draft, else the `value` the script gave.
    pub fn value(&self) -> Option<&Json> {
        self.draft.as_ref().or_else(|| self.props.get("value"))
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.props.get(key).and_then(Json::as_str)
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.props.get(key).and_then(Json::as_f64)
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.props.get(key).and_then(Json::as_bool)
    }

    /// `key` when the property holds a table the component raises, else "".
    pub fn action<'a>(&self, key: &'a str) -> &'a str {
        if matches!(self.props.get(key), Some(Json::Object(_))) { key } else { "" }
    }

    /// The node's `modifiers` entries that are tables.
    pub fn modifiers(&self) -> impl Iterator<Item = &Map<String, Json>> {
        self.props.get("modifiers").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object)
    }

    fn reference(&self) -> Ref {
        Ref { id: self.id.clone(), serial: self.serial }
    }
}

/// What a control did that the pane must act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Raised {
    /// A table to send to the script's `update`.
    Message(Json),
    /// Text to put on the pasteboard.
    Copy(String),
}

#[derive(Default)]
pub struct ViewTree {
    nodes: HashMap<String, ViewNode>,
    root: Option<Ref>,
    /// The Modals with `presented` set, drawn over the pane.
    modals: Vec<Ref>,
    /// Whether the view takes the pane's height itself (a frame-less Table, vertical List or
    /// height-less VideoStream), so the pane has no scroll view.
    fills_pane: bool,
    /// The node a check holds a pointer on.
    pub hover_held: Option<String>,
    /// Nodes the last reconcile or `set` created or wrote.
    written: usize,
    /// The first duplicate `key` the last reconcile met, as "<parent> <key>".
    duplicate: Option<String>,
    next_serial: u64,
    touched: BTreeSet<String>,
}

/// One reconcile's working state: the nodes it may reuse and the ones it reached.
struct Pass {
    old: HashMap<String, ViewNode>,
    reached: HashMap<String, ViewNode>,
    presenting: Vec<Ref>,
}

impl ViewTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root(&self) -> Option<&ViewNode> {
        self.root.as_ref().and_then(|r| self.nodes.get(&r.id))
    }

    pub fn node(&self, id: &str) -> Option<&ViewNode> {
        self.nodes.get(id)
    }

    /// Every node the last render reached, in no order.
    pub fn all(&self) -> impl Iterator<Item = &ViewNode> {
        self.nodes.values()
    }

    pub fn modals(&self) -> impl Iterator<Item = &ViewNode> {
        self.modals.iter().filter_map(|r| self.nodes.get(&r.id))
    }

    pub fn fills_pane(&self) -> bool {
        self.fills_pane
    }

    pub fn written(&self) -> usize {
        self.written
    }

    pub fn duplicate(&self) -> Option<&str> {
        self.duplicate.as_deref()
    }

    // MARK: reconcile

    /// Makes the tree match `tree`: a node whose identity survives keeps its serial and its
    /// draft; a property, child list or embedded map is assigned only when it differs; a node no
    /// longer reached is dropped.
    pub fn reconcile(&mut self, tree: &PaneNode) {
        self.touched.clear();
        self.duplicate = None;
        let mut pass = Pass { old: std::mem::take(&mut self.nodes), reached: HashMap::new(), presenting: Vec::new() };
        let reuse = self.root.as_ref().is_some_and(|r| r.id == "root");
        let r = self.place(&mut pass, tree, "root".into(), reuse);
        self.root = Some(r);
        self.nodes = pass.reached;
        if pass.presenting != self.modals {
            self.modals = pass.presenting;
        }
        self.fills_pane = self.root().is_some_and(|r| self.fills(r));
        self.written = self.touched.len();
    }

    fn serial(&mut self) -> u64 {
        self.next_serial += 1;
        self.next_serial
    }

    /// The node for `pane` at `id`: the old one when `reuse` allows it and it is the same
    /// component, else a new one; then its contents are brought up to date.
    fn place(&mut self, pass: &mut Pass, pane: &PaneNode, id: String, reuse: bool) -> Ref {
        let old = if reuse { pass.old.remove(&id) } else { None };
        let mut node = match old {
            Some(old) if old.component == pane.component => old,
            _ => {
                self.touched.insert(id.clone());
                let serial = self.serial();
                ViewNode::new(id.clone(), pane.component.clone(), serial)
            }
        };
        // What the node held last time: a child, embedded node or row is reused only from its own host.
        let old_embedded = std::mem::take(&mut node.embedded);
        let old_rows: BTreeSet<String> = node.rows.iter().map(|r| r.id.clone()).collect();
        let old_children: BTreeSet<String> = node.children.iter().map(|r| r.id.clone()).collect();
        let mut embedded = BTreeMap::new();
        let mut props = Map::new();
        for (key, value) in &pane.props {
            let v = if pane.component == "Table" && key == "sections" {
                value.clone()
            } else {
                self.lift(pass, value, &node.id, key, &old_embedded, &mut embedded)
            };
            props.insert(key.clone(), v);
        }
        let mut rows = Vec::new();
        if pane.component == "Table"
            && let Some(sections) = props.remove("sections")
        {
            let s = self.table_rows(pass, sections, &node.id, &old_rows, &mut rows);
            props.insert("sections".into(), s);
        }
        if pane.component == "TextField" || props.contains_key("on_change") {
            let value = props.remove("value");
            match &value {
                Some(v) if !opt_eq(Some(v), node.given.as_ref()) => self.write(&mut node, v.clone()),
                None if node.given.is_none() && node.draft.is_none() && pane.component == "TextField" => self.write(&mut node, Json::String(String::new())),
                _ => {}
            }
            node.given = value;
        }
        if !map_eq(&node.props, &props) {
            node.props = props;
            self.touched.insert(node.id.clone());
        }
        if node.rows != rows {
            node.rows = rows;
            self.touched.insert(node.id.clone());
        }
        if old_embedded != embedded {
            self.touched.insert(node.id.clone());
        }
        node.embedded = embedded;
        if node.component == "Modal" && node.bool("presented") == Some(true) {
            pass.presenting.push(node.reference());
        }
        let children = self.place_children(pass, &pane.children, &node.id, &old_children);
        if node.children != children {
            node.children = children;
            self.touched.insert(node.id.clone());
        }
        let r = node.reference();
        pass.reached.insert(id, node);
        r
    }

    fn place_children(&mut self, pass: &mut Pass, panes: &[PaneNode], parent: &str, old: &BTreeSet<String>) -> Vec<Ref> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut keys = BTreeSet::new();
        let mut out = Vec::new();
        for pane in panes {
            let id = if let Some(key) = pane.props.get("key").map(key_text) {
                if !keys.insert(key.clone()) {
                    if self.duplicate.is_none() {
                        self.duplicate = Some(format!("{parent} {key}"));
                    }
                    continue;
                }
                child_id(parent, &key)
            } else {
                let n = counts.entry(&pane.component).or_insert(0);
                let id = format!("{parent}/{}{n}", pane.component);
                *n += 1;
                id
            };
            let reuse = old.contains(&id);
            out.push(self.place(pass, pane, id, reuse));
        }
        out
    }

    /// A property's value with every component in it, at any depth, placed as an embedded node
    /// and replaced by its id.
    fn lift(&mut self, pass: &mut Pass, value: &Json, host: &str, path: &str, old: &BTreeMap<String, Ref>, into: &mut BTreeMap<String, Ref>) -> Json {
        if let Some(pane) = PaneNode::from_json(value.clone()) {
            let id = format!("{host}@{path}");
            let reuse = old.get(path).is_some_and(|r| r.id == id);
            let r = self.place(pass, &pane, id.clone(), reuse);
            into.insert(path.to_owned(), r);
            return Json::String(id);
        }
        match value {
            Json::Array(items) => Json::Array(items.iter().enumerate().map(|(i, v)| self.lift(pass, v, host, &format!("{path}.{i}"), old, into)).collect()),
            Json::Object(fields) => {
                let mut out = Map::new();
                for (k, v) in fields {
                    out.insert(k.clone(), self.lift(pass, v, host, &format!("{path}.{k}"), old, into));
                }
                Json::Object(out)
            }
            _ => value.clone(),
        }
    }

    /// A Table's sections with each row replaced by its row node's id; the rows are placed in
    /// `rows`, in order.
    fn table_rows(&mut self, pass: &mut Pass, sections: Json, table: &str, old: &BTreeSet<String>, rows: &mut Vec<Ref>) -> Json {
        let Json::Array(list) = sections else { return sections };
        let mut seen = BTreeSet::new();
        let mut out = Vec::with_capacity(list.len());
        for (si, section) in list.into_iter().enumerate() {
            let Json::Object(mut sec) = section else {
                out.push(section);
                continue;
            };
            let Some(Json::Array(items)) = sec.remove("rows") else {
                out.push(Json::Object(sec));
                continue;
            };
            let mut ids = Vec::with_capacity(items.len());
            for (ri, item) in items.into_iter().enumerate() {
                let Json::Object(row) = item else {
                    ids.push(item);
                    continue;
                };
                let mut key = row.get("path").and_then(Json::as_str).or_else(|| row.get("id").and_then(Json::as_str)).map(str::to_owned).unwrap_or_else(|| format!("{si}.{ri}"));
                if !seen.insert(key.clone()) {
                    key = format!("{key}#{si}.{ri}");
                    seen.insert(key.clone());
                }
                let id = format!("{table}/{key}");
                let row_id = row.get("id").and_then(Json::as_str).unwrap_or("").to_owned();
                let pane = PaneNode { component: "TableRow".into(), props: row, children: Vec::new() };
                let reuse = old.contains(&id);
                let r = self.place(pass, &pane, id.clone(), reuse);
                if let Some(n) = pass.reached.get_mut(&id) {
                    n.row_id = row_id;
                }
                rows.push(r);
                ids.push(Json::String(id));
            }
            sec.insert("rows".into(), Json::Array(ids));
            out.push(Json::Object(sec));
        }
        Json::Array(out)
    }

    fn write(&mut self, node: &mut ViewNode, draft: Json) {
        if opt_eq(node.draft.as_ref(), Some(&draft)) {
            return;
        }
        node.draft = Some(draft);
        self.touched.insert(node.id.clone());
    }

    /// The view fills the pane, with no scroll view around it, when it holds a node whose
    /// `frame` asks for `maxHeight = "infinity"` or a Table or vertical List with no `frame`
    /// (that component scrolls itself). A node whose `frame` fixes a height (`height` or a
    /// numeric `maxHeight`) bounds what it holds, so nothing inside it makes the pane fill.
    fn fills(&self, n: &ViewNode) -> bool {
        let frames: Vec<&Map<String, Json>> = n.modifiers().filter_map(|m| m.get("frame").and_then(Json::as_object)).collect();
        if frames.iter().any(|f| f.contains_key("height") || f.get("maxHeight").is_some_and(Json::is_number)) {
            return false;
        }
        if frames.iter().any(|f| f.get("maxHeight").and_then(Json::as_str) == Some("infinity")) {
            return true;
        }
        let scrolls = n.component == "Table"
            || (n.component == "List" && n.string("direction") != Some("horizontal"))
            || (n.component == "VideoStream" && !n.props.contains_key("height"));
        if scrolls && frames.is_empty() {
            return true;
        }
        n.parts().iter().filter_map(|id| self.nodes.get(*id)).any(|c| self.fills(c))
    }

    /// The script restarted: its next render's values all count as new, so each replaces what
    /// its control shows.
    pub fn forget_given(&mut self) {
        for node in self.nodes.values_mut() {
            node.given = None;
        }
    }

    // MARK: writes from outside a render

    /// Writes one property of one node, as a fine-grained script does when a value it bound
    /// changes; nothing when it is equal. A bound control's `value` follows the render rule: a
    /// value the script did not give last replaces the draft. False when there is no node `id`.
    pub fn set(&mut self, id: &str, prop: &str, value: Option<Json>) -> bool {
        self.touched.clear();
        let Some(mut node) = self.nodes.remove(id) else { return false };
        if prop == "value" && (node.component == "TextField" || node.props.contains_key("on_change")) {
            if let Some(v) = &value
                && !opt_eq(Some(v), node.given.as_ref())
            {
                self.write(&mut node, v.clone());
            }
            node.given = value;
        } else if !opt_eq(node.props.get(prop), value.as_ref()) {
            match value {
                Some(v) => node.props.insert(prop.to_owned(), v),
                None => node.props.remove(prop),
            };
            self.touched.insert(node.id.clone());
        }
        self.nodes.insert(id.to_owned(), node);
        self.written = self.touched.len();
        true
    }

    // MARK: what a control does

    /// The table at `key_path` of node `id`'s props (`action`, `rowAction`,
    /// `hits.3.doubleAction`, `menu.1.action`), with each of `fields` the table lacks added, for
    /// the script. None when the node has left the view or holds no table there.
    pub fn raise(&self, id: &str, key_path: &str, fields: &Map<String, Json>) -> Option<Json> {
        if key_path.is_empty() {
            return None;
        }
        let node = self.nodes.get(id)?;
        let Some(Json::Object(mut msg)) = lookup(&node.props, key_path).cloned() else { return None };
        for (k, v) in fields {
            msg.entry(k.clone()).or_insert_with(|| v.clone());
        }
        Some(Json::Object(msg))
    }

    /// What pressing node `id` does: its `action` `{copy = {text}}` goes to the pasteboard; any
    /// other `action` table goes to the script.
    pub fn activate(&self, id: &str) -> Option<Raised> {
        let node = self.nodes.get(id)?;
        if let Some(Json::Object(action)) = node.props.get("action")
            && let Some(text) = action.get("copy").and_then(|c| c.get("text")).and_then(Json::as_str)
        {
            return Some(Raised::Copy(text.to_owned()));
        }
        if node.action("action").is_empty() {
            return None;
        }
        self.raise(id, "action", &Map::new()).map(Raised::Message)
    }

    /// A control writing a value: it shows at once, and its `on_change` table, with `value` set,
    /// is returned for the script.
    pub fn change(&mut self, id: &str, value: Json) -> Option<Json> {
        let node = self.nodes.get_mut(id)?;
        if opt_eq(node.value(), Some(&value)) {
            return None;
        }
        node.draft = Some(value.clone());
        let Some(Json::Object(mut msg)) = node.props.get("on_change").cloned() else { return None };
        msg.insert("value".into(), value);
        Some(Json::Object(msg))
    }
}

/// A keyed child's id: `=` and the key with `%`, `/` and `@` escaped, so it never equals a
/// generated `<Component><n>` segment or reads as a deeper path or a property.
pub fn child_id(parent: &str, key: &str) -> String {
    let escaped = key.replace('%', "%25").replace('/', "%2F").replace('@', "%40");
    format!("{parent}/={escaped}")
}

fn key_text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        _ => text(v),
    }
}

/// The value as text, for a control that shows whatever it was given.
pub fn text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Number(n) => number_text(n.as_f64().unwrap_or(0.0)),
        Json::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// A number as Swift's `JSONValue.text` writes it: whole numbers without a fraction.
fn number_text(n: f64) -> String {
    if n == n.round() && n.abs() < 9.2e18 { format!("{}", n as i64) } else { format!("{n}") }
}

/// The value at a dotted path through objects and array indices.
pub fn lookup<'a>(props: &'a Map<String, Json>, key_path: &str) -> Option<&'a Json> {
    let mut parts = key_path.split('.');
    let mut cur = props.get(parts.next()?)?;
    for part in parts {
        cur = match cur {
            Json::Object(o) => o.get(part)?,
            Json::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// JSON equality with numbers compared as f64, as Swift's `JSONValue` holds them: the script's
/// `1` and wheeld's `1` are equal whichever side wrote a fraction.
pub fn json_eq(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b)),
        (Json::Object(x), Json::Object(y)) => map_eq(x, y),
        _ => a == b,
    }
}

fn map_eq(x: &Map<String, Json>, y: &Map<String, Json>) -> bool {
    x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
}

fn opt_eq(a: Option<&Json>, b: Option<&Json>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => json_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// JSON text with sorted keys and whole numbers written without a fraction, so equal values
/// give equal text.
pub fn canonical(v: &Json) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Json, out: &mut String) {
    match v {
        Json::Number(n) => match n.as_f64() {
            Some(f) if f == f.round() && f.abs() < 9.0e15 => out.push_str(&format!("{}", f as i64)),
            _ => out.push_str(&n.to_string()),
        },
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Json::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Json::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&fields[k], out);
            }
            out.push('}');
        }
        _ => out.push_str(&v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_sorts_keys_and_drops_whole_fractions() {
        assert_eq!(canonical(&json!({"b": 1.0, "a": [2.5, "x", null]})), r#"{"a":[2.5,"x",null],"b":1}"#);
    }

    #[test]
    fn a_keyed_child_escapes_its_key() {
        assert_eq!(child_id("root", "a/b@c%"), "root/=a%2Fb%40c%25");
    }
}
