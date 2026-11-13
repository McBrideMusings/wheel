//! The narrowed, sorted list every list page shows (Beads, Issues, PRs, Repo): the search, the filters
//! (each a popover of ticks, with "Is not"), the status boxes that set one filter, Clear, the header
//! sort and the row selection, all held per pane and changed by the same seven tools on every page.
//!
//! A page names its filters and sortable columns in a [`Spec`], says how one of its rows answers a
//! filter and a search ([`Item`]), and how two rows compare by a column. The rest is here: the page
//! calls [`op`] before its own tools, [`List::keeps`] and [`List::sorted`] to list its rows, and
//! [`bar`] and [`boxes`] to draw the controls.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, MetaObject, Tool};
use serde_json::{json, Value};

use super::page::Page;

/// One filter a page offers: its key, the word its title uses, and a value its rows hide until the
/// filter is set (done beads, closed issues).
pub struct Filter {
    pub key: &'static str,
    pub name: &'static str,
    pub hidden: Option<&'static str>,
}

/// What a page's list can be narrowed and sorted by.
pub struct Spec {
    pub filters: &'static [Filter],
    /// The column keys a header click may sort by.
    pub sorts: &'static [&'static str],
}

/// A row the list narrows: the text a search looks in, and whether it has a filter's value.
pub trait Item {
    fn haystack(&self) -> String;
    fn has(&self, key: &str, value: &str) -> bool;
}

/// One pane's list: what it searches for, the ticked values of each filter, the filters that exclude
/// rather than match, the sort and the selected rows.
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub q: String,
    pub f: BTreeMap<String, Vec<String>>,
    pub not: BTreeSet<String>,
    pub sort: (String, bool),
    /// The rows the person selected (a click, a shift-click range, a ⌘-click); the list actions take
    /// them when any of them is listed, else the whole narrowed list.
    pub selected: BTreeSet<String>,
}

impl List {
    pub fn new(sort: &str, asc: bool) -> Self {
        List { q: String::new(), f: BTreeMap::new(), not: BTreeSet::new(), sort: (sort.into(), asc), selected: BTreeSet::new() }
    }

    pub fn values(&self, key: &str) -> &[String] {
        self.f.get(key).map_or(&[], |v| v.as_slice())
    }

    /// Whether a search or any filter narrows the list, which is when Clear does something.
    pub fn active(&self) -> bool {
        !self.q.trim().is_empty() || self.f.values().any(|v| !v.is_empty())
    }

    /// Whether `item` stays in the list: every filter holds it (a filter holds a row that has one of its values,
    /// or, excluding, none of them), a row with a filter's hidden value shows only while that filter is set, and
    /// every word of the search is in its text.
    pub fn keeps(&self, spec: &Spec, item: &impl Item) -> bool {
        for f in spec.filters {
            let v = self.values(f.key);
            if v.is_empty() {
                if f.hidden.is_some_and(|h| item.has(f.key, h)) {
                    return false;
                }
            } else if v.iter().any(|x| item.has(f.key, x)) == self.not.contains(f.key) {
                return false;
            }
        }
        let hay = item.haystack().to_lowercase();
        self.q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
    }

    /// `items` that [`keeps`](Self::keeps) holds, in the sort's order; `cmp` compares two rows by a column key
    /// and `tie` breaks what it leaves equal, so the order never depends on the input's.
    pub fn sorted<'a, T: Item>(&self, spec: &Spec, items: impl IntoIterator<Item = &'a T>, cmp: impl Fn(&str, &T, &T) -> Ordering, tie: impl Fn(&T, &T) -> Ordering) -> Vec<&'a T> {
        let mut v: Vec<&T> = items.into_iter().filter(|i| self.keeps(spec, *i)).collect();
        let (key, asc) = (&self.sort.0, self.sort.1);
        v.sort_by(|a, b| {
            let o = cmp(key, a, b).then_with(|| tie(a, b));
            if asc { o } else { o.reverse() }
        });
        v
    }

    /// Apply one of the list tools. A change to what is listed clears the selection.
    pub fn apply(&mut self, spec: &Spec, op: Op) {
        let known = |k: &str| spec.filters.iter().any(|f| f.key == k);
        match op {
            Op::Query(q) => {
                self.q = q;
                self.selected.clear();
            }
            Op::Filter(key, value) if known(&key) => {
                self.f.insert(key, value.split(',').filter(|s| !s.is_empty()).map(String::from).collect());
                self.selected.clear();
            }
            Op::Toggle(key, value, on) if known(&key) => {
                let v = self.f.entry(key).or_default();
                v.retain(|x| *x != value);
                if on {
                    v.push(value);
                }
                self.selected.clear();
            }
            Op::Not(key, on) if known(&key) => {
                if on { self.not.insert(key); } else { self.not.remove(&key); }
                self.selected.clear();
            }
            Op::Sort(key, asc) if spec.sorts.contains(&key.as_str()) => self.sort = (key, asc),
            Op::Clear => {
                self.q.clear();
                self.f.clear();
                self.not.clear();
                self.selected.clear();
            }
            Op::Select(ids) => self.selected = ids.into_iter().collect(),
            _ => {}
        }
    }
}

/// A call to one of the list tools.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Query(String),
    /// A filter set to these values (comma separated; empty clears it), as a status box does.
    Filter(String, String),
    /// One value of a filter ticked or unticked.
    Toggle(String, String, bool),
    /// A filter made to exclude its values, or match them again.
    Not(String, bool),
    Sort(String, bool),
    Clear,
    Select(Vec<String>),
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => s != "false" && s != "0" && !s.is_empty(),
        _ => false,
    }
}

/// The list tool a call names, read from its arguments; `None` when the call is one of the page's own tools.
pub fn op(params: &CallToolRequestParams) -> Option<Op> {
    let empty = serde_json::Map::new();
    let a = params.arguments.as_ref().unwrap_or(&empty);
    let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(match params.name.as_ref() {
        "set_query" => Op::Query(a.get("q").map_or_else(|| s("value"), |_| s("q"))),
        "set_filter" => Op::Filter(s("key"), s("value")),
        "toggle_filter" => {
            let key = s("key");
            let (field, value) = key.split_once(':')?;
            Op::Toggle(field.into(), value.into(), a.get("value").is_some_and(truthy))
        }
        "set_not" => Op::Not(s("key"), a.get("value").is_some_and(truthy)),
        "set_sort" => Op::Sort(s("key"), a.get("asc").is_some_and(truthy)),
        "clear" => Op::Clear,
        "select_rows" => Op::Select(match a.get("ids") {
            Some(Value::Array(v)) => v.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            Some(Value::String(t)) => t.split([',', ' ']).filter(|x| !x.is_empty()).map(String::from).collect(),
            _ => vec![],
        }),
        _ => return None,
    })
}

/// The seven list tools, as a page lists them beside its own; `meta` is the page's UI resource.
pub fn tools(meta: &MetaObject) -> Vec<Tool> {
    let schema = |props: Value, required: &[&str]| -> Arc<serde_json::Map<String, Value>> {
        Arc::new(json!({ "type": "object", "properties": props, "required": required }).as_object().unwrap().clone())
    };
    let text = |d: &str| json!({ "type": "string", "description": d });
    [
        ("set_query", "Search the list", schema(json!({ "q": text("The search text.") }), &["q"])),
        ("set_filter", "Set one filter to these values (comma separated); empty clears it",
            schema(json!({ "key": text("The filter's key."), "value": text("The values; empty for any.") }), &["key", "value"])),
        ("toggle_filter", "Tick or untick one value of a filter; the key is `<filter>:<value>`",
            schema(json!({ "key": text("`<filter>:<value>`."), "value": text("true to tick, false to untick.") }), &["key", "value"])),
        ("set_not", "Make a filter exclude its values instead of matching them",
            schema(json!({ "key": text("The filter's key."), "value": text("true to exclude, false to match.") }), &["key", "value"])),
        ("set_sort", "Sort the list by a column, ascending or not",
            schema(json!({ "key": text("A column key."), "asc": { "type": ["boolean", "string"], "description": "Ascending when true." } }), &["key", "asc"])),
        ("clear", "Clear the search and the filters", schema(json!({}), &[])),
        ("select_rows", "Select rows of the list: the list actions take them instead of the whole list",
            schema(json!({ "ids": { "type": ["array", "string"], "items": { "type": "string" }, "description": "The ids of the rows selected now; empty clears the selection." } }), &["ids"])),
    ]
    .into_iter()
    .map(|(name, d, s)| Tool::new(name, d, s).with_meta(meta.clone()))
    .collect()
}

/// The ids the list actions (Copy IDs, the project's list buttons) take: the selected ones among `listed`, in the
/// order shown, or every listed id when none of the selection is listed.
pub fn taken(listed: Vec<String>, l: &List) -> Vec<String> {
    let picked: Vec<String> = listed.iter().filter(|id| l.selected.contains(id.as_str())).cloned().collect();
    if picked.is_empty() { listed } else { picked }
}

/// The status boxes: each a number over a caption that, tapped, sets one filter to its value alone, and tapped
/// again clears it. `cards` are (caption, filter key, value, count, whether a non-zero count is good news).
pub fn boxes(p: &mut Page, l: &List, cards: &[(&str, &str, &str, usize, bool)]) -> String {
    let ids: Vec<String> = cards
        .iter()
        .enumerate()
        .map(|(i, (label, key, value, count, hot))| {
            let on = l.values(key) == [value.to_string()];
            let tone = if *hot && *count > 0 { "ok" } else if *count == 0 { "muted" } else { "fg" };
            let v = p.label(&format!("kpi{i}.v"), &count.to_string(), json!({ "size": 20, "weight": "semibold", "tabular": true, "tone": tone }));
            let c = p.label(&format!("kpi{i}.l"), label, json!({ "tone": "muted", "size": 11 }));
            super::layout::modify(&mut p.out, &c, json!({ "lineLimit": 1 }));
            let inner = p.boxed(&format!("kpi{i}.c"), "v", vec![v, c], json!({ "spacing": 2, "align": "start" }));
            let mut mods = vec![json!({ "frame": { "maxWidth": "infinity" } }), json!({ "padding": [7, 8] }),
                json!({ "overlay": { "stroke": if on { "tint" } else { "separator" }, "cornerRadius": 8 } })];
            if on {
                mods.push(json!({ "background": "controlBackground" }));
            }
            p.put(json!({ "id": format!("kpi{i}"), "component": "Button", "child": inner, "variant": "borderless",
                "action": { "event": { "name": "set_filter", "context": { "key": key, "value": if on { "" } else { value } } } }, "modifiers": mods }));
            format!("kpi{i}")
        })
        .collect();
    let widths: Vec<Value> = ids.iter().map(|_| json!("flex")).collect();
    p.boxed("kpis", "h", ids, json!({ "spacing": 8, "widths": widths }))
}

/// The search row: the search field, then `beside` (a views choice, a tab, a refresh button), with id `find`.
pub fn find(p: &mut Page, l: &List, beside: Vec<String>) -> String {
    p.search("search", "set_query", &l.q, "Search");
    super::layout::modify(&mut p.out, "search", json!({ "frame": { "minWidth": 120, "maxWidth": "infinity" } }));
    // What sits beside the field is offered its width first, so the field shrinks to make room for it.
    for b in &beside {
        super::layout::modify(&mut p.out, b, json!({ "layoutPriority": 1 }));
    }
    let mut kids = vec!["search".to_string()];
    let mut widths = vec![json!("flex")];
    for b in beside {
        kids.push(b);
        widths.push(Value::Null);
    }
    p.boxed("find", "h", kids, json!({ "spacing": 8, "align": "center", "widths": widths, "padding": [8, 0, 0, 0] }));
    super::layout::modify(&mut p.out, "find", json!({ "frame": { "maxWidth": "infinity" } }));
    "find".into()
}

/// The filter row, with id `bar`: one popover of ticks per filter (`options` gives each filter's (value, text) in
/// the spec's order), then Clear and `actions` (the list actions chip). Each popover's title says what it holds
/// ("Any label", "Status not: done"); Clear is hidden, not removed, when nothing narrows the list, so nothing after
/// it moves.
pub fn bar(p: &mut Page, spec: &Spec, l: &List, options: Vec<Vec<(String, String)>>, actions: String) -> String {
    let mut menus = vec![];
    for (f, opts) in spec.filters.iter().zip(options) {
        let (key, name) = (f.key, f.name);
        let held = l.values(key);
        let neg = l.not.contains(key);
        let text_of = |v: &str| opts.iter().find(|(x, _)| x == v).map_or(v.to_string(), |(_, t)| t.split("  ").next().unwrap_or(t).to_string());
        let title = if held.is_empty() {
            format!("Any {name}")
        } else {
            format!("{}{}: {}", name[..1].to_uppercase() + &name[1..], if neg { " not" } else { "" }, held.iter().map(|v| text_of(v)).collect::<Vec<_>>().join(", "))
        };
        p.bind(&format!("/set_not/{key}"), json!(neg));
        p.put(json!({ "id": format!("filters.{key}.not"), "component": "CheckBox", "label": "Is not", "value": { "path": format!("/set_not/{key}") } }));
        p.put(json!({ "id": format!("filters.{key}.div"), "component": "Divider" }));
        let mut kids = vec![format!("filters.{key}.not"), format!("filters.{key}.div")];
        for (v, text) in &opts {
            let path = format!("/toggle_filter/{key}:{v}");
            let id = format!("filters.{key}.{v}");
            p.bind(&path, json!(held.contains(v)));
            p.put(json!({ "id": id, "component": "CheckBox", "label": text, "value": { "path": path } }));
            kids.push(id);
        }
        // A Mac menu closes after each tick, so the ticks sit in a popover, which stays open until it is dismissed.
        let trigger = p.label(&format!("filters.{key}.title"), &format!("{title} ⌄"), json!({ "size": 12 }));
        let content = p.boxed(&format!("filters.{key}.ticks"), "v", kids, json!({ "spacing": 6, "align": "start" }));
        p.put(json!({ "id": format!("filters.{key}"), "component": "Popover", "children": [trigger, content] }));
        menus.push(format!("filters.{key}"));
    }
    // Two groups, by proximity: what narrows the list, and Clear with the actions. The gap between them is whatever
    // width is left, never under 16. Sorting is the column headers' job.
    let narrow = p.boxed("bar.narrow", "h", menus, json!({ "spacing": 6, "align": "center" }));
    let clear = p.button("filters.clear", "Clear", "clear", json!({}), json!({ "style": "link", "disabled": !l.active(), "hideOff": true }));
    let shape = p.boxed("bar.shape", "h", vec![clear, actions], json!({ "spacing": 10, "align": "center" }));
    p.boxed("bar", "h", vec![narrow, shape], json!({ "spacing": 16, "align": "center", "widths": ["flex", Value::Null], "padding": [4, 0, 0, 0] }));
    super::layout::modify(&mut p.out, "bar", json!({ "frame": { "maxWidth": "infinity" } }));
    "bar".into()
}

/// A filter's options counted over `items`: each value `of` gives, with how many rows have it, the commonest first.
pub fn counted<'a, T: 'a>(items: impl IntoIterator<Item = &'a T>, of: impl Fn(&T) -> Vec<String>) -> Vec<(String, String)> {
    let mut c: BTreeMap<String, usize> = BTreeMap::new();
    for v in items.into_iter().flat_map(|i| of(i)) {
        *c.entry(v).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = c.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, n)| (k.clone(), format!("{k}  {n}"))).collect()
}

/// A filter's options in a fixed order, each with how many of `items` have it: `values` are (value, text).
pub fn fixed<'a, T: Item + 'a>(key: &str, items: impl IntoIterator<Item = &'a T> + Clone, values: &[(&str, &str)]) -> Vec<(String, String)> {
    values.iter().map(|(v, t)| (v.to_string(), format!("{t}  {}", items.clone().into_iter().filter(|i| i.has(key, v)).count()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row(&'static str, &'static str, i32);
    impl Item for Row {
        fn haystack(&self) -> String {
            self.0.into()
        }
        fn has(&self, key: &str, value: &str) -> bool {
            key == "state" && self.1 == value
        }
    }
    const SPEC: Spec = Spec { filters: &[Filter { key: "state", name: "state", hidden: Some("closed") }], sorts: &["n"] };

    fn rows() -> Vec<Row> {
        vec![Row("alpha", "open", 2), Row("beta", "closed", 1), Row("gamma", "draft", 3)]
    }

    fn names(l: &List) -> Vec<&'static str> {
        let r = rows();
        l.sorted(&SPEC, &r, |_, a, b| a.2.cmp(&b.2), |a, b| a.0.cmp(b.0)).iter().map(|x| x.0).collect()
    }

    #[test]
    fn a_hidden_value_shows_only_once_its_filter_is_set_and_is_not_inverts_a_filter() {
        let mut l = List::new("n", true);
        assert_eq!(names(&l), ["alpha", "gamma"], "closed is hidden until asked for");
        l.apply(&SPEC, Op::Filter("state".into(), "closed".into()));
        assert_eq!(names(&l), ["beta"]);
        l.apply(&SPEC, Op::Toggle("state".into(), "open".into(), true));
        assert_eq!(names(&l), ["beta", "alpha"]);
        l.apply(&SPEC, Op::Not("state".into(), true));
        assert_eq!(names(&l), ["gamma"]);
        l.apply(&SPEC, Op::Clear);
        assert!(!l.active());
        l.apply(&SPEC, Op::Query("ALP".into()));
        assert_eq!(names(&l), ["alpha"]);
    }

    #[test]
    fn a_sort_reaches_only_a_sortable_column_and_unknown_filters_are_ignored() {
        let mut l = List::new("n", true);
        l.apply(&SPEC, Op::Sort("n".into(), false));
        assert_eq!(names(&l), ["gamma", "alpha"]);
        l.apply(&SPEC, Op::Sort("title".into(), true));
        assert_eq!(l.sort, ("n".into(), false));
        l.apply(&SPEC, Op::Filter("nope".into(), "x".into()));
        assert!(l.f.is_empty());
        l.selected.insert("alpha".into());
        assert_eq!(taken(vec!["gamma".into(), "alpha".into()], &l), ["alpha"]);
        l.apply(&SPEC, Op::Query("a".into()));
        assert!(l.selected.is_empty(), "a new search clears the selection");
    }

    #[test]
    fn a_call_names_its_op_and_a_pages_own_tool_is_none() {
        let call = |name: &str, args: Value| {
            let mut p = CallToolRequestParams::new(name.to_string());
            p.arguments = args.as_object().cloned();
            op(&p)
        };
        assert_eq!(call("set_query", json!({ "value": "x" })), Some(Op::Query("x".into())));
        assert_eq!(call("toggle_filter", json!({ "key": "state:open", "value": "true" })), Some(Op::Toggle("state".into(), "open".into(), true)));
        assert_eq!(call("set_sort", json!({ "key": "n", "asc": false })), Some(Op::Sort("n".into(), false)));
        assert_eq!(call("select_rows", json!({ "ids": "a,b c" })), Some(Op::Select(vec!["a".into(), "b".into(), "c".into()])));
        assert_eq!(call("select_pr", json!({ "id": "3" })), None);
        let names: Vec<String> = tools(&MetaObject::default()).iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, ["set_query", "set_filter", "toggle_filter", "set_not", "set_sort", "clear", "select_rows"]);
    }
}
