//! The page a view lays out: A2UI components pushed into one list, the helpers that build the
//! common ones, and the empty page a read answers with before the first call draws it.

use serde_json::{json, Value};

use super::layout;

pub const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";

/// The messages a read of a page answers with: the surface and an empty `page` column.
pub fn skeleton(surface: &str) -> Value {
    json!([
        { "version": "v0.9", "createSurface": { "surfaceId": surface, "catalogId": CATALOG } },
        { "version": "v0.9", "updateComponents": { "surfaceId": surface, "components": [
            { "id": "root", "component": "Column", "align": "stretch", "children": ["page"] },
            { "id": "page", "component": "Column", "children": [] }
        ] } }
    ])
}

/// The `updateComponents` message for `components` on `surface`.
pub fn update(surface: &str, components: Vec<Value>) -> Value {
    // `Page::bind` entries carry a data-model value rather than a component; each becomes an `updateDataModel` message.
    let (data, components): (Vec<Value>, Vec<Value>) = components.into_iter().partition(|c| c.get("updateDataModel").is_some());
    let mut messages = vec![json!({ "version": "v0.9", "updateComponents": { "surfaceId": surface, "components": components } })];
    for d in data {
        let mut m = d;
        m["version"] = json!("v0.9");
        m["updateDataModel"]["surfaceId"] = json!(surface);
        messages.push(m);
    }
    Value::Array(messages)
}

pub struct Page {
    pub out: Vec<Value>,
}

impl Page {
    pub fn put(&mut self, v: Value) {
        self.out.push(v);
    }
    pub fn label(&mut self, id: &str, text: &str, extra: Value) -> String {
        layout::label(&mut self.out, id, text, &extra)
    }
    pub fn runs(&mut self, id: &str, runs: Value, extra: Value) -> String {
        let mut e = extra;
        e["runs"] = runs;
        layout::label(&mut self.out, id, "", &e)
    }
    pub fn boxed(&mut self, id: &str, axis: &str, children: Vec<String>, extra: Value) -> String {
        layout::boxed(&mut self.out, id, axis, children, &extra)
    }
    /// A number over a caption in an outlined box: `value` in `tone` (muted when zero), `label` under it.
    pub fn stat(&mut self, id: &str, value: &str, label: &str, tone: &str) -> String {
        let v = self.label(&format!("{id}.v"), value, json!({ "size": 22, "weight": "semibold", "tabular": true, "tone": tone }));
        let l = self.label(&format!("{id}.l"), label, json!({ "tone": "muted" }));
        self.boxed(id, "v", vec![v, l], json!({ "border": "line", "spacing": 2 }))
    }

    /// Puts `value` in the data model at `path`, the path a control's `value` is bound to. A change the person makes there
    /// calls the tool the path's first segment names (`view::changed`).
    pub fn bind(&mut self, path: &str, value: Value) {
        self.put(json!({ "updateDataModel": { "path": path, "value": value } }));
    }

    /// A text field that calls `tool` with what the person types.
    pub fn search(&mut self, id: &str, tool: &str, value: &str, placeholder: &str) -> String {
        self.bind(&format!("/{tool}"), json!(value));
        self.put(json!({ "id": id, "component": "TextField", "label": placeholder, "value": { "path": format!("/{tool}") }, "variant": "shortText" }));
        id.to_string()
    }

    /// A choice of one among `options` ({value, text}); `path` is `/<tool>` or `/<tool>/<key>`. A few options draw as a segmented
    /// control (`chips`), more as a menu.
    pub fn choose(&mut self, id: &str, path: &str, selected: &str, options: impl Into<Value>, chips: bool, disabled: bool) -> String {
        self.bind(path, json!([selected]));
        let options: Vec<Value> = options.into().as_array().cloned().unwrap_or_default().iter().map(|o| json!({ "label": o["text"], "value": o["value"] })).collect();
        let mut v = json!({ "id": id, "component": "ChoicePicker", "variant": "mutuallyExclusive", "options": options, "value": { "path": path } });
        if chips {
            v["displayStyle"] = json!("chips");
        }
        if disabled {
            v["modifiers"] = json!([{ "disabled": true }]);
        }
        self.put(v);
        id.to_string()
    }

    /// A checkbox that calls `tool` with "true" or "false".
    pub fn check(&mut self, id: &str, tool: &str, text: &str, value: bool) -> String {
        self.bind(&format!("/{tool}"), json!(value));
        self.put(json!({ "id": id, "component": "CheckBox", "label": text, "value": { "path": format!("/{tool}") } }));
        id.to_string()
    }

    /// A library `Button` whose label is a `Text` (or an `Icon` when `extra` has `icon`). `extra`: `style` link or
    /// plain draws it borderless, `disabled` disables it (`hideOff` also hides it), `help` is its hover text.
    pub fn button(&mut self, id: &str, text: &str, name: &str, context: Value, extra: Value) -> String {
        let child = match extra["icon"].as_str() {
            Some(i) => {
                self.put(json!({ "id": format!("{id}.i"), "component": "Icon", "name": i }));
                format!("{id}.i")
            }
            None => {
                self.put(json!({ "id": format!("{id}.t"), "component": "Text", "text": text }));
                format!("{id}.t")
            }
        };
        let mut mods = vec![];
        if extra["disabled"] == true {
            mods.push(json!({ "disabled": true }));
            if extra["hideOff"] == true {
                mods.push(json!({ "opacity": 0 }));
            }
        }
        if let Some(h) = extra["help"].as_str() {
            mods.push(json!({ "help": h }));
            mods.push(json!({ "accessibilityLabel": h }));
        }
        let mut v = json!({ "id": id, "component": "Button", "child": child, "action": { "event": { "name": name, "context": context } } });
        if matches!(extra["style"].as_str(), Some("link" | "plain")) {
            v["variant"] = json!("borderless");
        }
        if !mods.is_empty() {
            v["modifiers"] = json!(mods);
        }
        self.put(v);
        id.to_string()
    }
}


pub fn cell(text: impl Into<String>, extra: Value) -> Value {
    let mut v = json!({ "text": text.into() });
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}


/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// How long before `now` the Unix time `ts` was: `5m`, `3h`, `2d`, `6w`.
pub fn ago(ts: i64, now: i64) -> String {
    let s = (now - ts).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        86400..=1209599 => format!("{}d", s / 86400),
        _ => format!("{}w", s / 604800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_value_becomes_a_data_message_and_never_a_component() {
        let mut p = Page { out: vec![] };
        p.search("search", "set_query", "bug", "Search");
        let m = update("s", p.out);
        let components = m[0]["updateComponents"]["components"].as_array().unwrap();
        assert!(components.iter().all(|c| c["id"].is_string() && c["component"].is_string()));
        assert_eq!(m[1]["updateDataModel"]["path"], "/set_query");
        assert_eq!(m[1]["updateDataModel"]["value"], "bug");
    }

    #[test]
    fn an_age_uses_the_largest_whole_unit() {
        assert_eq!(ago(0, 30), "now");
        assert_eq!(ago(0, 300), "5m");
        assert_eq!(ago(0, 7200), "2h");
        assert_eq!(ago(0, 3 * 86400), "3d");
        assert_eq!(ago(0, 6 * 604800), "6w");
    }
}
