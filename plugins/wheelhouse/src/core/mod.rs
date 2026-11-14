//! What every view shares and none owns: which project and pane a request is for, running the
//! project's tools (`bd`, `gh`, `git`), what the running agents are doing, noticing that a
//! project's files changed, and the Table component. Nothing here knows what a bead or a diff is.

pub mod agents;
pub mod buttons;
pub mod git;
pub mod layout;
pub mod list;
pub mod page;
pub mod project;
pub mod run;
pub mod table;
pub mod tracker;
pub mod watch;

/// A whole number from JSON: an integer, or a whole number a JSON writer printed as a float (`3.0`).
pub fn number_of(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9e15).map(|f| f as i64))
}

#[cfg(test)]
mod tests {
    use super::number_of;
    use serde_json::json;

    #[test]
    fn a_number_may_be_a_whole_float() {
        assert_eq!(number_of(&json!(7)), Some(7));
        assert_eq!(number_of(&json!(7.0)), Some(7));
        assert_eq!(number_of(&json!(7.5)), None);
        assert_eq!(number_of(&json!("7")), None);
    }
}
