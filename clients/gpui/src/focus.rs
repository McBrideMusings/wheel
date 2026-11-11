//! Which pane has the keyboard, and what typing into an agent pane does.

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

/// What a key does to an agent pane's draft.
#[derive(Debug, PartialEq)]
pub enum Typed {
    Nothing,
    Edited,
    Send(String),
}

/// Apply a key, named as GPUI names it, to `draft`. `text` is the character it types, if any.
pub fn type_into(draft: &mut String, key: &str, text: Option<&str>, command: bool) -> Typed {
    if command {
        return Typed::Nothing;
    }
    match key {
        "enter" => {
            let sent = std::mem::take(draft);
            if sent.trim().is_empty() { Typed::Nothing } else { Typed::Send(sent) }
        }
        "backspace" => if draft.pop().is_some() { Typed::Edited } else { Typed::Nothing },
        _ => match text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
            Some(t) => {
                draft.push_str(t);
                Typed::Edited
            }
            None => Typed::Nothing,
        },
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

    #[test]
    fn typing_builds_a_draft_and_enter_sends_it() {
        let mut d = String::new();
        assert_eq!(type_into(&mut d, "h", Some("h"), false), Typed::Edited);
        assert_eq!(type_into(&mut d, "i", Some("i"), false), Typed::Edited);
        assert_eq!(type_into(&mut d, "x", Some("x"), false), Typed::Edited);
        assert_eq!(type_into(&mut d, "backspace", None, false), Typed::Edited);
        assert_eq!(type_into(&mut d, "enter", None, false), Typed::Send("hi".into()));
        assert_eq!(d, "");
        assert_eq!(type_into(&mut d, "enter", None, false), Typed::Nothing);
    }

    #[test]
    fn command_keys_and_control_characters_type_nothing() {
        let mut d = String::new();
        assert_eq!(type_into(&mut d, "]", Some("]"), true), Typed::Nothing);
        assert_eq!(type_into(&mut d, "escape", Some("\u{1b}"), false), Typed::Nothing);
        assert_eq!(d, "");
    }
}
