//! Which pane has the keyboard, and what typing into a one-line field does.

/// The focused pane among the panes of the space shown, in layout order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    pub current: Option<String>,
}

impl Focus {
    /// Move one pane forward or back, wrapping at the ends; returns the pane now focused.
    pub fn step(&mut self, order: &[String], forward: bool) -> Option<String> {
        if order.is_empty() {
            self.current = None;
            return None;
        }
        let at = self.current.as_ref().and_then(|c| order.iter().position(|p| p == c));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        self.current = Some(order[next].clone());
        self.current.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> Vec<String> {
        ["p1", "p2", "p3"].map(String::from).to_vec()
    }

    #[test]
    fn stepping_wraps_both_ways() {
        let mut f = Focus { current: Some("p3".into()) };
        assert_eq!(f.step(&order(), true).as_deref(), Some("p1"));
        assert_eq!(f.step(&order(), false).as_deref(), Some("p3"));
        assert_eq!(f.step(&order(), false).as_deref(), Some("p2"));
    }

    #[test]
    fn a_closed_pane_loses_focus_and_stepping_starts_over() {
        let mut f = Focus { current: Some("gone".into()) };
        assert_eq!(f.step(&order(), true).as_deref(), Some("p1"));
        assert_eq!(f.step(&[], true), None);
        assert_eq!(f.current, None);
    }
}
