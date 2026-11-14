//! Styled text and stacks, built from the A2UI library's `Text`, `Row`, `Column`, `Icon` and `Divider` and a `modifiers`
//! list (ADR-0031): the vocabulary a page's helpers speak (`tone`, `size`, `spacing`, `widths`, `border`) turned into what
//! the client draws.

use serde_json::{json, Value};

/// A tone's SwiftUI colour name.
fn colour(tone: &str) -> Option<&'static str> {
    match tone {
        "fg" => Some("primary"),
        "muted" => Some("secondary"),
        "ok" => Some("green"),
        "warn" => Some("orange"),
        "bad" => Some("red"),
        "accent" | "focus" => Some("tint"),
        "purple" => Some("purple"),
        "todo" | "faint" => Some("tertiary"),
        "line" => Some("separator"),
        "card" | "soft" | "bg" => Some("controlBackground"),
        _ => None,
    }
}

/// The modifiers a text's style asks for: `size`, `weight`, `mono`, `tone`, `tabular`, `upper`, `tracking`, `align`, `chip`.
fn text_modifiers(e: &Value) -> Vec<Value> {
    let mut m = vec![];
    let size = e["size"].as_f64().unwrap_or(13.0);
    let weight = e["weight"].as_str().unwrap_or("regular");
    let mono = e["mono"] == true;
    if size != 13.0 || weight != "regular" || mono {
        m.push(json!({ "font": { "size": size, "weight": weight, "design": if mono { "monospaced" } else { "default" } } }));
    }
    if let Some(c) = e["tone"].as_str().and_then(colour) {
        m.push(json!({ "foregroundStyle": c }));
    }
    if e["tabular"] == true {
        m.push(json!({ "monospacedDigit": true }));
    }
    if e["upper"] == true {
        m.push(json!({ "textCase": "uppercase" }));
    }
    if let Some(t) = e["tracking"].as_f64() {
        m.push(json!({ "tracking": t }));
    }
    if e["align"] == "end" {
        m.push(json!({ "multilineTextAlignment": "trailing" }));
        m.push(json!({ "frame": { "maxWidth": "infinity", "alignment": "trailing" } }));
    }
    if e["chip"] == true {
        m.push(json!({ "padding": { "horizontal": 4 } }));
        m.push(json!({ "overlay": { "stroke": "separator", "cornerRadius": 4 } }));
    }
    m
}

fn text(id: &str, text: &str, e: &Value) -> Value {
    let mut v = json!({ "id": id, "component": "Text", "text": text });
    let m = text_modifiers(e);
    if !m.is_empty() {
        v["modifiers"] = json!(m);
    }
    if e["markdown"] == true {
        v["markdown"] = json!(true);
    }
    v
}

/// Styled text. With a `dot`, a `symbol` or `runs` it is a `Row` of an `Icon` or `ProgressView` and the text(s); otherwise one `Text`.
pub fn label(out: &mut Vec<Value>, id: &str, body: &str, e: &Value) -> String {
    let tone = e["tone"].as_str().and_then(colour);
    let lead = if let Some(dot) = e["dot"].as_str() {
        let name = match dot { "ring" | "hollow" => "circle", _ => "circle.fill" };
        let mut v = json!({ "id": format!("{id}.d"), "component": "Icon", "name": name, "modifiers": [{ "font": { "size": 8 } }] });
        if let Some(c) = tone {
            v["modifiers"].as_array_mut().unwrap().push(json!({ "foregroundStyle": c }));
        }
        Some(v)
    } else if let Some(sym) = e["symbol"].as_str() {
        Some(if sym == "spinner" {
            json!({ "id": format!("{id}.d"), "component": "ProgressView", "modifiers": [{ "frame": { "width": 14, "height": 14 } }] })
        } else {
            let mut v = json!({ "id": format!("{id}.d"), "component": "Icon", "name": sym });
            if let Some(c) = tone {
                v["modifiers"] = json!([{ "foregroundStyle": c }]);
            }
            v
        })
    } else {
        None
    };
    if lead.is_none() && e.get("runs").is_none() {
        out.push(text(id, body, e));
        return id.to_string();
    }
    let mut kids = vec![];
    if let Some(l) = lead {
        kids.push(l["id"].as_str().unwrap().to_string());
        out.push(l);
    }
    if let Some(runs) = e["runs"].as_array() {
        for (i, r) in runs.iter().enumerate() {
            let rid = format!("{id}.r{i}");
            let mut style = e.clone();
            style.as_object_mut().unwrap().extend(r.as_object().cloned().unwrap_or_default());
            out.push(text(&rid, r["text"].as_str().unwrap_or(""), &style));
            kids.push(rid);
        }
    } else {
        let tid = format!("{id}.t");
        out.push(text(&tid, body, e));
        kids.push(tid);
    }
    out.push(json!({ "id": id, "component": "Row", "children": kids, "align": "center", "spacing": 5 }));
    id.to_string()
}

/// Adds a modifier to the component `id` already in `out`.
pub fn modify(out: &mut [Value], id: &str, modifier: Value) {
    if let Some(c) = out.iter_mut().find(|c| c["id"] == id) {
        match c["modifiers"].as_array_mut() {
            Some(m) => m.push(modifier),
            None => c["modifiers"] = json!([modifier]),
        }
    }
}

/// A `Row` (`axis` "h") or `Column` of `children`. `extra`: `spacing`, `padding`, `margin`, `widths` (a number, "flex" or null per
/// child), `border`, `fill`, `radius`, `edge` (top or bottom rule), `dividers`, `align`, `justify`, `wrap` (a row that scrolls
/// sideways), `mono`.
pub fn boxed(out: &mut Vec<Value>, id: &str, axis: &str, children: Vec<String>, e: &Value) -> String {
    let horizontal = axis == "h";
    let mut kids = children;
    if let Some(widths) = e["widths"].as_array() {
        for (i, w) in widths.iter().enumerate() {
            let Some(kid) = kids.get(i).cloned() else { continue };
            if let Some(n) = w.as_f64() {
                modify(out, &kid, json!({ "frame": { "width": n, "alignment": "leading" } }));
            } else if w == "flex" {
                modify(out, &kid, json!({ "frame": { "maxWidth": "infinity", "alignment": "leading" } }));
            }
        }
    }
    if horizontal && e["dividers"] == true {
        let mut with = vec![];
        for (i, k) in kids.iter().enumerate() {
            if i > 0 {
                let d = format!("{id}.div{i}");
                out.push(json!({ "id": d, "component": "Divider", "axis": "vertical" }));
                with.push(d);
            }
            with.push(k.clone());
        }
        kids = with;
    }
    if let Some(edge) = e["edge"].as_str() {
        let d = format!("{id}.edge");
        out.push(json!({ "id": d, "component": "Divider" }));
        if edge == "top" { kids.insert(0, d) } else { kids.push(d) }
    }
    let outlined = e.get("border").is_some() || e.get("fill").is_some();
    let radius = e["radius"].as_f64().unwrap_or(if outlined { 8.0 } else { 0.0 });
    let mut m = vec![];
    if e["mono"] == true {
        m.push(json!({ "font": { "size": 13, "design": "monospaced" } }));
    }
    if let Some(p) = e.get("padding") {
        m.push(json!({ "padding": p }));
    } else if outlined || e.get("edge").is_some() {
        m.push(json!({ "padding": [10, 12] }));
    }
    if let Some(f) = e["fill"].as_str().and_then(|f| if f == "ready" { Some("controlBackground") } else { colour(f) }) {
        m.push(json!({ "background": f }));
    }
    if let Some(b) = e["border"].as_str().and_then(colour) {
        m.push(json!({ "overlay": { "stroke": b, "cornerRadius": radius } }));
    }
    if radius > 0.0 {
        m.push(json!({ "clipShape": { "cornerRadius": radius } }));
    }
    if let Some(margin) = e.get("margin") {
        m.push(json!({ "padding": margin }));
    }
    let align = match e["align"].as_str() {
        Some("end") => "end",
        Some("center") | Some("firstTextBaseline") => "center",
        _ => "start",
    };
    let wrap = horizontal && e["wrap"] == true;
    let mut v = json!({ "id": id, "component": if wrap { "List" } else if horizontal { "Row" } else { "Column" }, "children": kids,
        "spacing": e["spacing"].as_f64().unwrap_or(if horizontal { 12.0 } else { 8.0 }) });
    if wrap {
        v["direction"] = json!("horizontal");
    } else {
        v["align"] = json!(if horizontal && align == "start" { "center" } else { align });
        if horizontal && e["trailing"].as_u64().unwrap_or(0) > 0 && !e["widths"].as_array().is_some_and(|w| w.iter().any(|x| x == "flex")) {
            v["justify"] = json!("spaceBetween");
        }
    }
    if !m.is_empty() {
        v["modifiers"] = json!(m);
    }
    out.push(v);
    id.to_string()
}
