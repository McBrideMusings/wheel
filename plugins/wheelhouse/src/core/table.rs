//! The Table component a page lays out (ADR-0029: it takes presentation data, never domain data).

use serde_json::{json, Value};

/// A column: its key, header, width and whether it holds the tree's fold control.
pub fn column(key: &str, label: &str, width: Value, tree: bool, wide: bool) -> Value {
    json!({ "key": key, "label": label, "width": width, "tree": tree, "wide": wide })
}

/// The column `c` made sortable: a click on its header sorts the rows by the cells' `sort` values.
pub fn sortable(mut c: Value) -> Value {
    c["sortable"] = json!(true);
    c
}

/// `asc` of a sort as a tool argument: a bool from a header click, or the words `true`/`false` from a
/// caller that only has strings (the Debug app's `/a2ui/action`, the command line).
pub fn asc<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Asc { B(bool), S(String) }
    Ok(match <Asc as serde::Deserialize>::deserialize(d)? {
        Asc::B(b) => b,
        Asc::S(s) => s != "false" && s != "0" && !s.is_empty(),
    })
}

/// The table `t` with a selection the page owns: the rows whose ids are in `selected` draw selected,
/// a click or a shift-click raises `select_action` with `{ids}` (the rows selected now, every one of
/// them) and a double-click or Return raises the table's `rowAction`. Without it a plain click raises
/// `rowAction`.
pub fn selectable(mut t: Value, select_action: &str, selected: &[String]) -> Value {
    t["selectAction"] = json!(select_action);
    t["selected"] = json!(selected);
    t
}

/// The ids the list actions take: the `selected` ones among `listed`, in the order shown, or every
/// listed id when none of the selection is listed.
pub fn taken(listed: Vec<String>, selected: &std::collections::BTreeSet<String>) -> Vec<String> {
    let picked: Vec<String> = listed.iter().filter(|id| selected.contains(id.as_str())).cloned().collect();
    if picked.is_empty() { listed } else { picked }
}

/// A list of ids as a tool argument: an array of strings from the client's Table, or one string of
/// ids separated by commas or spaces from a caller that only has strings (the Debug app's
/// `/a2ui/action`, the command line).
pub fn ids<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Ids { V(Vec<String>), S(String) }
    Ok(match <Ids as serde::Deserialize>::deserialize(d)? {
        Ids::V(v) => v,
        Ids::S(s) => s.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string).collect(),
    })
}

/// A section of rows under a title (an empty title draws no header).
pub fn section(title: &str, rows: Vec<Value>) -> Value {
    json!({ "title": title, "count": rows.len(), "rows": rows })
}

/// A Table: `columns`, `sections`, the tools a row's click, a fold's click and a header's click
/// call, the sort the rows are in and the words shown with no rows. With a `sortAction` the client
/// never re-orders the rows itself: it tells the page `{key, asc}` and the page sends them sorted.
pub fn component(
    columns: Vec<Value>,
    sections: Vec<Value>,
    row_action: &str,
    fold_action: &str,
    sort_action: &str,
    sort: (&str, bool),
    empty: &str,
) -> Value {
    json!({ "component": "Table", "rowAction": row_action, "foldAction": fold_action, "sortAction": sort_action, "columns": columns,
        "sort": { "key": sort.0, "asc": sort.1 }, "sections": sections, "empty": empty })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_carries_its_columns_sections_and_actions() {
        let t = component(
            vec![column("id", "ID", json!(80), false, false)],
            vec![section("Ready", vec![json!({ "id": "a" })])],
            "select",
            "fold",
            "sort",
            ("id", true),
            "none",
        );
        assert_eq!(t["component"], "Table");
        assert_eq!(t["rowAction"], "select");
        assert_eq!(t["sortAction"], "sort");
        assert_eq!(t["sections"][0]["count"], 1);
        assert_eq!(t["columns"][0]["key"], "id");
        assert_eq!(t["sort"]["asc"], true);
        let t = selectable(t, "pick", &["a".into()]);
        assert_eq!(t["selectAction"], "pick");
        assert_eq!(t["selected"], json!(["a"]));
    }

    #[test]
    fn ids_come_as_an_array_or_one_string() {
        #[derive(serde::Deserialize)]
        struct P { #[serde(deserialize_with = "ids")] ids: Vec<String> }
        let p: P = serde_json::from_value(json!({ "ids": ["a", "b"] })).unwrap();
        assert_eq!(p.ids, vec!["a", "b"]);
        let p: P = serde_json::from_value(json!({ "ids": "a,b c" })).unwrap();
        assert_eq!(p.ids, vec!["a", "b", "c"]);
        let p: P = serde_json::from_value(json!({ "ids": "" })).unwrap();
        assert!(p.ids.is_empty());
    }
}
