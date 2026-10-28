//! The `chat.links.patterns` setting: a list of entries that each turn text matching a regular
//! expression into a link. An entry is `match` (the regular expression) and an `action`:
//! `open` with a `url` template, `run` with a `command` template, or `copy` with an optional
//! `text` template (the match itself without one). `$0` in a template is the match and `$1`… its
//! groups.
//!
//! The setting's value is the list as JSON, so a client reads it without a TOML parser and
//! `wheel config set` takes the same text; the file holds it as an array of tables. An entry is
//! checked here, one at a time, whoever wrote it, so a client only ever sees entries that parse.
//! The expression is checked in Rust's regex syntax (no look-around, no back-references) so the
//! same table can serve a matcher on either side of the wire.

use serde_json::{Map, Value};

const FIELDS: [&str; 5] = ["match", "action", "url", "command", "text"];

/// The list as the setting's value, or why it is refused. All or nothing: a value written through
/// `wheel config set` with one bad entry is not written.
pub(crate) fn take(value: &str) -> Result<String, String> {
    let list: Value = serde_json::from_str(value.trim())
        .map_err(|e| format!("not a JSON list of entries: {e}"))?;
    let Value::Array(items) = list else {
        return Err(
            "not a list: write [{\"match\": \"…\", \"action\": \"open\", \"url\": \"…\"}, …]"
                .into(),
        );
    };
    let mut kept = Vec::new();
    for (i, item) in items.iter().enumerate() {
        kept.push(entry(item).map_err(|why| format!("entry {}: {why}", i + 1))?);
    }
    Ok(Value::Array(kept).to_string())
}

/// The entries of a file's array that are good, as the setting's value, and one line for each that
/// is not. The good ones stay in force: a regular expression that does not parse costs only its own
/// link.
pub(crate) fn from_file(items: &[toml::Value]) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut problems = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let checked = serde_json::to_value(item)
            .map_err(|e| e.to_string())
            .and_then(|v| entry(&v));
        match checked {
            Ok(v) => kept.push(v),
            Err(why) => problems.push(format!("entry {} is skipped: {why}", i + 1)),
        }
    }
    (Value::Array(kept).to_string(), problems)
}

/// The value as the array of inline tables a `wheel.toml` holds. `value` has been through [`take`].
pub(crate) fn to_toml(value: &str) -> toml_edit::Value {
    let mut array = toml_edit::Array::new();
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(value) {
        for item in items {
            let mut table = toml_edit::InlineTable::new();
            if let Value::Object(fields) = item {
                for (name, field) in fields {
                    if let Value::String(text) = field {
                        table.insert(&name, text.into());
                    }
                }
            }
            array.push(table);
        }
    }
    array.into()
}

/// One entry as it will be stored: the fields its action uses, and no others.
fn entry(item: &Value) -> Result<Value, String> {
    let Value::Object(fields) = item else {
        return Err("is not a table".into());
    };
    if let Some(name) = fields.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(format!(
            "has no field {name}; its fields are {}",
            FIELDS.join(", ")
        ));
    }
    let text = |name: &str| -> Result<Option<&str>, String> {
        match fields.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err(format!("{name} is not a string")),
        }
    };
    let pattern = text("match")?.ok_or("has no match")?;
    let groups = groups(pattern)?;
    let action = text("action")?.ok_or("has no action: open, run or copy")?;
    let (field, template) = match action {
        "open" => ("url", text("url")?.ok_or("is an open with no url")?),
        "run" => (
            "command",
            text("command")?.ok_or("is a run with no command")?,
        ),
        "copy" => ("text", text("text")?.unwrap_or("$0")),
        other => return Err(format!("action {other:?} is not open, run or copy")),
    };
    if let Some(stray) = ["url", "command", "text"]
        .iter()
        .find(|f| **f != field && fields.contains_key(**f))
    {
        return Err(format!("{action} takes {field}, not {stray}"));
    }
    if template.trim().is_empty() {
        return Err(format!("{field} is empty"));
    }
    if action == "open" && !(template.starts_with("http://") || template.starts_with("https://")) {
        return Err(format!(
            "url {template:?} does not start with http:// or https://"
        ));
    }
    if let Some(n) = references(template).into_iter().find(|n| *n > groups) {
        return Err(format!(
            "{field} uses ${n}, but match has {groups} group(s)"
        ));
    }
    let mut out = Map::new();
    out.insert("match".into(), pattern.into());
    out.insert("action".into(), action.into());
    out.insert(field.into(), template.into());
    Ok(Value::Object(out))
}

/// How many groups `pattern` captures, or why it is not a regular expression wheel takes.
fn groups(pattern: &str) -> Result<usize, String> {
    let hir = regex_syntax::Parser::new()
        .parse(pattern)
        .map_err(|e| format!("match {pattern:?} is not a regular expression: {e}"))?;
    if hir.properties().minimum_len() == Some(0) {
        return Err(format!(
            "match {pattern:?} can match no text at all, so it would link nothing"
        ));
    }
    Ok(hir.properties().explicit_captures_len())
}

/// The `$N` references in a template.
fn references(template: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut digits = String::new();
    let mut after_dollar = false;
    for c in template.chars().chain(std::iter::once(' ')) {
        if after_dollar && c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            found.push(digits.parse().unwrap_or(usize::MAX));
            digits.clear();
        }
        after_dollar = c == '$';
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_list_is_stored_with_only_the_fields_its_actions_use() {
        let v = take(r##"[{"match":"wheel-[a-z0-9.]+","action":"run","command":"bd show $0"},{"match":"#(\\d+)","action":"open","url":"https://example.com/$1"},{"match":"sha-\\w+","action":"copy"}]"##).unwrap();
        let v: Value = serde_json::from_str(&v).unwrap();
        assert_eq!(v[0]["command"], "bd show $0");
        assert_eq!(v[1]["url"], "https://example.com/$1");
        assert_eq!(v[2]["text"], "$0");
    }

    #[test]
    fn a_bad_entry_is_named_and_a_file_keeps_the_others() {
        let err = take(r#"[{"match":"(","action":"copy"}]"#).unwrap_err();
        assert!(
            err.starts_with("entry 1: match \"(\" is not a regular expression"),
            "{err}"
        );
        let table: toml::Table = toml::from_str(
            "p = [{ match = \"(\", action = \"copy\" }, { match = \"x+\", action = \"copy\" }, { match = \"(a)\", action = \"run\", command = \"echo $2\" }]",
        )
        .unwrap();
        let (kept, problems) = from_file(table["p"].as_array().unwrap());
        let kept: Value = serde_json::from_str(&kept).unwrap();
        assert_eq!(kept.as_array().unwrap().len(), 1);
        assert_eq!(kept[0]["match"], "x+");
        assert_eq!(problems.len(), 2);
        assert!(problems[1].contains("$2"), "{problems:?}");
    }

    #[test]
    fn what_cannot_link_anything_is_refused() {
        for bad in [
            r#"[{"match":"a*","action":"copy"}]"#,
            r#"[{"match":"a","action":"open","url":"ftp://x"}]"#,
            r#"[{"match":"a","action":"run"}]"#,
            r#"[{"match":"a","action":"run","command":"x","url":"https://x"}]"#,
            r#"[{"match":"a","action":"jump"}]"#,
            r#"{"match":"a"}"#,
        ] {
            assert!(take(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_file_form_reads_back_as_the_same_list() {
        let v = take(r#"[{"match":"x+","action":"copy","text":"<$0>"}]"#).unwrap();
        let mut table = toml_edit::DocumentMut::new();
        table["p"] = toml_edit::value(to_toml(&v));
        let parsed: toml::Table = toml::from_str(&table.to_string()).unwrap();
        assert_eq!(from_file(parsed["p"].as_array().unwrap()), (v, vec![]));
    }
}
