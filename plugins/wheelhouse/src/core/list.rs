//! The narrowed list the `wheelhouse beads` command prints and the Beads Graph draws: the search,
//! the filters (each a set of values, or with "Is not" every row without them) and the sort.
//!
//! A list names its filters in a [`Spec`] and says how one of its rows answers a filter and a search
//! ([`Item`]); [`List::keeps`] decides whether a row stays.

use std::collections::{BTreeMap, BTreeSet};

/// One filter a list offers: its key, and a value its rows hide until the filter is set (done beads).
pub struct Filter {
    pub key: &'static str,
    pub hidden: Option<&'static str>,
}

/// What a list can be narrowed by.
pub struct Spec {
    pub filters: &'static [Filter],
}

/// A row the list narrows: the text a search looks in, and whether it has a filter's value.
pub trait Item {
    fn haystack(&self) -> String;
    fn has(&self, key: &str, value: &str) -> bool;
}

/// A list's search, the values of each filter, the filters that exclude rather than match, and the
/// sort (a column key and whether it ascends).
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub q: String,
    pub f: BTreeMap<String, Vec<String>>,
    pub not: BTreeSet<String>,
    pub sort: (String, bool),
}

impl List {
    pub fn new(sort: &str, asc: bool) -> Self {
        List { q: String::new(), f: BTreeMap::new(), not: BTreeSet::new(), sort: (sort.into(), asc) }
    }

    pub fn values(&self, key: &str) -> &[String] {
        self.f.get(key).map_or(&[], |v| v.as_slice())
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

    /// Set filter `key` to `values` (comma separated; empty clears it, and its "Is not" with it). An
    /// unknown key changes nothing.
    pub fn filter(&mut self, spec: &Spec, key: &str, values: &str) {
        if spec.filters.iter().any(|f| f.key == key) {
            let v: Vec<String> = values.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
            if v.is_empty() {
                self.not.remove(key);
            }
            self.f.insert(key.into(), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row(&'static str, &'static str);
    impl Item for Row {
        fn haystack(&self) -> String {
            self.0.into()
        }
        fn has(&self, key: &str, value: &str) -> bool {
            key == "state" && self.1 == value
        }
    }
    const SPEC: Spec = Spec { filters: &[Filter { key: "state", hidden: Some("closed") }] };

    fn names(l: &List) -> Vec<&'static str> {
        [Row("alpha", "open"), Row("beta", "closed"), Row("gamma", "draft")].into_iter().filter(|r| l.keeps(&SPEC, r)).map(|r| r.0).collect()
    }

    #[test]
    fn a_hidden_value_shows_only_once_its_filter_is_set_and_is_not_inverts_a_filter() {
        let mut l = List::new("n", true);
        assert_eq!(names(&l), ["alpha", "gamma"], "closed is hidden until asked for");
        l.filter(&SPEC, "state", "closed");
        assert_eq!(names(&l), ["beta"]);
        l.filter(&SPEC, "state", "closed,open");
        assert_eq!(names(&l), ["alpha", "beta"]);
        l.not.insert("state".into());
        assert_eq!(names(&l), ["gamma"]);
        l.filter(&SPEC, "state", "");
        assert!(l.not.is_empty(), "clearing a filter clears its Is not");
        l.filter(&SPEC, "nope", "x");
        assert_eq!(l.f.len(), 1, "an unknown filter is ignored");
        l.q = "ALP".into();
        assert_eq!(names(&l), ["alpha"]);
    }
}
