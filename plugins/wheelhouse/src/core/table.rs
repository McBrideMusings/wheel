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

/// A section of rows under a title (an empty title draws no header).
pub fn section(title: &str, rows: Vec<Value>) -> Value {
    json!({ "title": title, "count": rows.len(), "rows": rows })
}

/// A Table: `columns`, `sections`, the tool a row's click and a fold's click call, the sort and
/// the words shown with no rows.
pub fn component(
    columns: Vec<Value>,
    sections: Vec<Value>,
    row_action: &str,
    fold_action: &str,
    sort: (&str, bool),
    empty: &str,
) -> Value {
    json!({ "component": "Table", "rowAction": row_action, "foldAction": fold_action, "columns": columns,
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
            ("id", true),
            "none",
        );
        assert_eq!(t["component"], "Table");
        assert_eq!(t["rowAction"], "select");
        assert_eq!(t["sections"][0]["count"], 1);
        assert_eq!(t["columns"][0]["key"], "id");
        assert_eq!(t["sort"]["asc"], true);
    }
}
