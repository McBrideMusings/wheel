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

/// The table `t` with a selection the page owns: the rows whose ids are in `selected` draw selected,
/// a click or a shift-click raises `select_action` with `{ids}` (the rows selected now, every one of
/// them) and a double-click or Return raises the table's `rowAction`. Without it a plain click raises
/// `rowAction`.
pub fn selectable(mut t: Value, select_action: &str, selected: &[String]) -> Value {
    t["selectAction"] = json!(select_action);
    t["selected"] = json!(selected);
    t
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
}
