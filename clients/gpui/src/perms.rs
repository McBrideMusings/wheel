//! The request an agent pane shows above its composer (the Swift client's `PermissionCard`,
//! `PermissionChoices` and `QuestionChoices` in `apple/WheelUI/Sources/WheelUI/PermissionViews.swift`
//! and `apple/Mac/Sources/ChatView.swift`): which request, which buttons in which order, which key
//! answers it, and how tall the card is so the transcript above it shrinks by exactly that much.

use wheel_proto::{PermInfo, PermOption};

/// A question the agent put to the person: its choices are answers to it, not allowances.
pub fn is_question(perm: &PermInfo) -> bool {
    perm.options.iter().any(|o| o.kind == "answer")
}

fn rejects(kind: &str) -> bool {
    kind.starts_with("reject")
}

/// The oldest waiting request `agent` raised, with its 1-based place among every waiting request.
pub fn waiting<'a>(perms: &'a [PermInfo], agent: &str) -> Option<(usize, &'a PermInfo)> {
    perms.iter().enumerate().find(|(_, p)| p.agent == agent).map(|(i, p)| (i + 1, p))
}

/// The three answers the keyboard gives a permission card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Deny,
    Always,
    Allow,
}

impl Answer {
    /// The request's option for this answer, when it offers one: Deny is any refusal, Always the
    /// agent's own allow-always option, Allow the one-time allow, else the first option that is
    /// neither a refusal, an allow-always nor a question's answer.
    pub fn option(self, perm: &PermInfo) -> Option<&PermOption> {
        let o = &perm.options;
        match self {
            Answer::Deny => o.iter().find(|x| rejects(&x.kind)),
            Answer::Always => o.iter().find(|x| x.kind == "allow_always"),
            Answer::Allow => o.iter().find(|x| x.kind == "allow_once")
                .or_else(|| o.iter().find(|x| !rejects(&x.kind) && x.kind != "allow_always" && x.kind != "answer")),
        }
    }

    /// The answer a key in the composer gives: Return with no modifier and an empty draft allows,
    /// Command-Return always allows, Esc denies. Every other key is the draft's.
    pub fn for_key(key: &str, command: bool, other_modifier: bool, draft_empty: bool) -> Option<Answer> {
        match key {
            "enter" if command && !other_modifier => Some(Answer::Always),
            "enter" if !command && !other_modifier && draft_empty => Some(Answer::Allow),
            "escape" if !command && !other_modifier => Some(Answer::Deny),
            _ => None,
        }
    }
}

/// The tooltip a button shows: its name, the rule it adds when it adds one, and its key.
pub fn help(option: &PermOption, perm: &PermInfo, keys: bool) -> String {
    let key = match option.kind.as_str() {
        "allow_once" if keys => " (↩)",
        "allow_always" if keys => " (⌘↩)",
        "reject_once" if keys => " (Esc)",
        _ => "",
    };
    match (&perm.why.always_rule, option.kind.as_str()) {
        (Some(rule), "allow_always") => format!("{}: adds the rule {rule}{key}", option.name),
        _ => format!("{}{key}", option.name),
    }
}

/// One button on the card.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub option: PermOption,
    /// The one-time allow, drawn filled as the default.
    pub prominent: bool,
}

/// A permission's buttons in the order they sit at the right: the refusals, then the allowances
/// that last, then the one-time allow, filled. A question has its answers in a column instead and
/// only its refusals (Skip) in the row.
pub fn row(perm: &PermInfo) -> Vec<Button> {
    let button = |o: &PermOption, prominent| Button { option: o.clone(), prominent };
    if is_question(perm) {
        return perm.options.iter().filter(|o| rejects(&o.kind)).map(|o| button(o, false)).collect();
    }
    let allow = Answer::Allow.option(perm).map(|o| o.id.clone());
    let mut out: Vec<Button> = perm.options.iter().filter(|o| rejects(&o.kind)).map(|o| button(o, false)).collect();
    out.extend(perm.options.iter().filter(|o| !rejects(&o.kind) && Some(&o.id) != allow.as_ref()).map(|o| button(o, false)));
    out.extend(perm.options.iter().filter(|o| Some(&o.id) == allow.as_ref()).map(|o| button(o, true)));
    out
}

/// A question's answers, top to bottom, each its name over its description.
pub fn answers(perm: &PermInfo) -> Vec<&PermOption> {
    perm.options.iter().filter(|o| o.kind == "answer").collect()
}

/// The lines under the header that say why: the agent's reason, then what made it ask, then the path.
pub fn why_lines(perm: &PermInfo) -> Vec<String> {
    let w = &perm.why;
    [w.reason.clone(), w.decision_reason.clone(), w.blocked_path.as_ref().map(|p| format!("Path: {p}"))].into_iter().flatten().collect()
}

/// How many lines of the request's title the card shows; a longer one ends in `…`.
pub const TITLE_LINES: usize = 6;

/// The request's title as the card draws it: at most `TITLE_LINES` lines.
pub fn title_lines(perm: &PermInfo) -> Vec<String> {
    let mut lines: Vec<String> = perm.title.lines().map(str::to_owned).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > TITLE_LINES {
        lines.truncate(TITLE_LINES);
        lines[TITLE_LINES - 1].push_str(" …");
    }
    lines
}

/// The card's fixed measures, in points: every line has its own height, so the card's height is
/// known before it is drawn and the transcript above it gives up exactly that much.
pub const PADDING: f32 = 10.;
pub const BORDER: f32 = 1.;
pub const GAP: f32 = 6.;
pub const LINE: f32 = 16.;
pub const BUTTON: f32 = 22.;
/// The space between a question's answer buttons.
pub const ANSWER_GAP: f32 = 4.;
/// An answer button's padding above and below its lines.
pub const ANSWER_PAD: f32 = 4.;
/// The space between the transcript and the card under it.
pub const MARGIN: f32 = 4.;

/// An answer button: its name line, its description line when it has one, and its padding.
pub fn answer_height(option: &PermOption) -> f32 {
    2. * ANSWER_PAD + LINE + if option.description.is_some() { LINE } else { 0. }
}

/// The card's whole height, its margin to the composer included.
pub fn height(perm: &PermInfo) -> f32 {
    let mut sections = vec![LINE];
    let why = why_lines(perm).len();
    if why > 0 {
        sections.push(why as f32 * LINE);
    }
    sections.push(title_lines(perm).len() as f32 * LINE);
    if is_question(perm) {
        let answers = answers(perm);
        sections.push(answers.iter().map(|o| answer_height(o)).sum::<f32>() + ANSWER_GAP * answers.len().saturating_sub(1) as f32);
    }
    if !row(perm).is_empty() {
        sections.push(BUTTON);
    }
    2. * PADDING + sections.iter().sum::<f32>() + GAP * (sections.len() - 1) as f32 + 2. * BORDER + MARGIN
}

/// The card's height in a pane that has `room` for it: its whole height when that fits, else
/// `room`, but never less than its header and buttons, which stay while the lines between them
/// scroll.
pub fn fitted(perm: &PermInfo, room: f32) -> f32 {
    let buttons = if row(perm).is_empty() { 0. } else { GAP + BUTTON };
    let least = 2. * PADDING + LINE + buttons + 2. * BORDER + MARGIN;
    height(perm).min(room.max(least))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::PermWhy;

    fn option(id: &str, name: &str, kind: &str) -> PermOption {
        PermOption { id: id.into(), name: name.into(), kind: kind.into(), description: None }
    }

    fn permission() -> PermInfo {
        PermInfo {
            id: "perm-1".into(), agent: "a1".into(), title: "rm -rf build".into(), tool_call: Some("call-1".into()),
            options: vec![option("allow", "Allow", "allow_once"), option("always", "Always allow", "allow_always"), option("deny", "Deny", "reject_once")],
            why: PermWhy { reason: Some("Remove the build folder".into()), always_rule: Some("Bash(rm:*)".into()), ..PermWhy::default() },
        }
    }

    fn question() -> PermInfo {
        let mut red = option("red", "Red", "answer");
        red.description = Some("warm".into());
        PermInfo {
            id: "perm-2".into(), agent: "a1".into(), title: "Which color?".into(), tool_call: None,
            options: vec![red, option("blue", "Blue", "answer"), option("skip", "Skip", "reject_once")],
            why: PermWhy::default(),
        }
    }

    #[test]
    fn keys_map_to_answers() {
        assert_eq!(Answer::for_key("enter", false, false, true), Some(Answer::Allow));
        assert_eq!(Answer::for_key("enter", false, false, false), None, "Return with a draft sends the draft");
        assert_eq!(Answer::for_key("enter", false, true, true), None, "Shift-Return is a new line");
        assert_eq!(Answer::for_key("enter", true, false, false), Some(Answer::Always));
        assert_eq!(Answer::for_key("escape", false, false, false), Some(Answer::Deny));
        assert_eq!(Answer::for_key("a", false, false, true), None);
    }

    #[test]
    fn answers_pick_the_options_the_swift_card_picks() {
        let p = permission();
        assert_eq!(Answer::Allow.option(&p).map(|o| o.id.as_str()), Some("allow"));
        assert_eq!(Answer::Always.option(&p).map(|o| o.id.as_str()), Some("always"));
        assert_eq!(Answer::Deny.option(&p).map(|o| o.id.as_str()), Some("deny"));
        let q = question();
        assert_eq!(Answer::Allow.option(&q), None, "Return picks no answer to a question");
        assert_eq!(Answer::Deny.option(&q).map(|o| o.id.as_str()), Some("skip"), "Esc skips a question");
    }

    #[test]
    fn the_row_puts_refusals_first_and_the_default_last() {
        let ids: Vec<(String, bool)> = row(&permission()).into_iter().map(|b| (b.option.id, b.prominent)).collect();
        assert_eq!(ids, [("deny".into(), false), ("always".into(), false), ("allow".into(), true)]);
        let q = question();
        assert!(is_question(&q) && !is_question(&permission()));
        assert_eq!(row(&q).into_iter().map(|b| b.option.id).collect::<Vec<_>>(), ["skip"]);
        assert_eq!(answers(&q).len(), 2);
    }

    #[test]
    fn help_names_the_rule_and_the_key() {
        let p = permission();
        assert_eq!(help(&p.options[1], &p, true), "Always allow: adds the rule Bash(rm:*) (⌘↩)");
        assert_eq!(help(&p.options[0], &p, true), "Allow (↩)");
        assert_eq!(help(&p.options[2], &p, false), "Deny");
    }

    #[test]
    fn waiting_finds_the_agents_oldest_request_and_its_place() {
        let mut other = permission();
        other.agent = "a2".into();
        other.id = "perm-0".into();
        let perms = vec![other, permission(), question()];
        assert_eq!(waiting(&perms, "a1").map(|(n, p)| (n, p.id.as_str())), Some((2, "perm-1")));
        assert_eq!(waiting(&perms, "a3"), None);
    }

    #[test]
    fn height_counts_every_line() {
        // header, reason, title, buttons: four sections, three gaps.
        assert_eq!(height(&permission()), 2. * PADDING + LINE + LINE + LINE + BUTTON + 3. * GAP + 2. + MARGIN);
        // header, title, two answers (one with a description), Skip.
        let answers = (8. + 2. * LINE) + (8. + LINE) + ANSWER_GAP;
        assert_eq!(height(&question()), 2. * PADDING + LINE + LINE + answers + BUTTON + 3. * GAP + 2. + MARGIN);
        let mut long = permission();
        long.title = (0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(title_lines(&long).len(), TITLE_LINES);
        assert_eq!(fitted(&question(), 1000.), height(&question()), "a card that fits keeps its height");
        assert_eq!(fitted(&question(), 120.), 120.);
        assert_eq!(fitted(&question(), 10.), 2. * PADDING + LINE + GAP + BUTTON + 2. * BORDER + MARGIN, "the header and buttons always show");
        assert!(title_lines(&long)[TITLE_LINES - 1].ends_with('…'));
    }
}
