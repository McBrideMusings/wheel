//! The page a view lays out: A2UI components pushed into one list, the helpers that build the
//! common ones, and the empty page a read answers with before the first call draws it.

use serde_json::{json, Value};

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
    json!([{ "version": "v0.9", "updateComponents": { "surfaceId": surface, "components": components } }])
}

pub struct Page {
    pub out: Vec<Value>,
}

impl Page {
    pub fn put(&mut self, v: Value) {
        self.out.push(v);
    }
    pub fn label(&mut self, id: &str, text: &str, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Label", "text": text });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
    pub fn runs(&mut self, id: &str, runs: Value, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Label", "text": "", "runs": runs });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
    pub fn boxed(&mut self, id: &str, axis: &str, children: Vec<String>, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Box", "axis": axis, "children": children });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
    /// A number over a caption in an outlined box: `value` in `tone` (muted when zero), `label` under it.
    pub fn stat(&mut self, id: &str, value: &str, label: &str, tone: &str) -> String {
        let v = self.label(&format!("{id}.v"), value, json!({ "size": 22, "weight": "semibold", "tabular": true, "tone": tone }));
        let l = self.label(&format!("{id}.l"), label, json!({ "tone": "muted" }));
        self.boxed(id, "v", vec![v, l], json!({ "border": "line", "spacing": 2 }))
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
    fn an_age_uses_the_largest_whole_unit() {
        assert_eq!(ago(0, 30), "now");
        assert_eq!(ago(0, 300), "5m");
        assert_eq!(ago(0, 7200), "2h");
        assert_eq!(ago(0, 3 * 86400), "3d");
        assert_eq!(ago(0, 6 * 604800), "6w");
    }
}
