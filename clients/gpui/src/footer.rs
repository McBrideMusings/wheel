//! The words of the status line under an agent's composer, as the Swift client writes them
//! (`apple/WheelKit/Sources/WheelKit/ContextUse.swift`, `AgentFooter.swift`,
//! `AgentCatalog.swift`, `AgentChat.swift`): the context meter's figures, the footer's
//! directory and background tasks, the agent's home, and the order of its config controls. The
//! `chat.footer.items` setting names the items and their order.
use wheel_proto::{AgentFooter, ConfigOption, ContextUse};

/// What the line shows, left to right, until the daemon says: `chat.footer.items`' default.
pub const DEFAULT_ITEMS: [&str; 6] = ["context", "background", "directory", "home", "branch", "changes"];

/// The items a `chat.footer.items` value lists, each once, in order; an empty value lists none.
pub fn items(value: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for item in value.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        if !out.iter().any(|o| o == item) {
            out.push(item.to_owned());
        }
    }
    out
}

/// The share of the window in use as a fraction for a bar, held between 0 and 1.
pub fn fraction(context: &ContextUse) -> f32 {
    if context.size == 0 {
        return 0.;
    }
    (context.used as f64 / context.size as f64).clamp(0., 1.) as f32
}

/// What the row shows beside the bar: "37%".
pub fn short_text(context: &ContextUse) -> String {
    format!("{}%", context.percent())
}

/// The raw figures in the short units a row has room for: "74K / 200K".
pub fn raw_text(context: &ContextUse) -> String {
    format!("{} / {}", short(context.used), short(context.size))
}

/// The exact numbers, for the hover.
pub fn help_text(context: &ContextUse) -> String {
    format!("Context window: {} of {} tokens used ({}%)", grouped(context.used), grouped(context.size), context.percent())
}

/// The meter's tint: the accent until 75%, orange until 90%, red from there.
pub fn tint(context: &ContextUse) -> u32 {
    match context.percent() {
        0..=74 => 0x7fb89a,
        75..=89 => 0xff9f0a,
        _ => 0xff453a,
    }
}

fn short(n: u64) -> String {
    if n >= 1_000_000 {
        let s = format!("{:.1}", n as f64 / 1_000_000.);
        format!("{}M", s.strip_suffix(".0").unwrap_or(&s))
    } else if n >= 1_000 {
        format!("{}K", (n as f64 / 1_000.).round() as u64)
    } else {
        n.to_string()
    }
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `path` with the person's home folder written `~`.
pub fn with_tilde(path: &str, home: &str) -> String {
    if !home.is_empty() && (path == home || path.strip_prefix(home).is_some_and(|rest| rest.starts_with('/'))) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_owned()
    }
}

/// The directory as the status line shows it.
pub fn shown_directory(footer: &AgentFooter) -> String {
    with_tilde(&footer.directory, &std::env::var("HOME").unwrap_or_default())
}

/// The running background tasks as the status line reads them, `1 shell · 2 agents`; none when
/// none run, so the line draws nothing at zero.
pub fn background_text(footer: &AgentFooter) -> Option<String> {
    let mut parts = vec![];
    if footer.shells > 0 {
        parts.push(if footer.shells == 1 { "1 shell".to_owned() } else { format!("{} shells", footer.shells) });
    }
    if footer.agents > 0 {
        parts.push(if footer.agents == 1 { "1 agent".to_owned() } else { format!("{} agents", footer.agents) });
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The lines added and removed, "+12 −3"; none outside a repository or with no change.
pub fn changes_text(footer: &AgentFooter) -> Option<String> {
    (footer.branch.is_some() && footer.added + footer.removed > 0).then(|| format!("+{} \u{2212}{}", footer.added, footer.removed))
}

/// Where the agent keeps its login and conversations when `agent.<name>.home` is unset.
pub fn default_home(harness: &str) -> &'static str {
    match harness {
        "claude" => "~/.claude",
        "codex" => "~/.codex",
        "pi" => "~/.pi/agent",
        _ => "its own default",
    }
}

/// The home as written beside the composer: the home the pane saved for its own agent, else the
/// one the daemon lists for the harness (`agent.<name>.home`), else the harness's default.
pub fn shown_home(own: Option<&str>, harness: &str, listed: Option<&str>) -> String {
    match [own, listed].into_iter().flatten().find(|h| !h.is_empty()) {
        Some(home) => with_tilde(home, &std::env::var("HOME").unwrap_or_default()),
        None => default_home(harness).to_owned(),
    }
}

/// The agent's own categories say what an option is for, so the row can order them without this
/// wheel knowing any option's name: what answers, then how hard it thinks, then what it may do
/// without asking. Anything uncategorised sorts last, in the order the agent sent it.
pub fn sorted_config(options: &[ConfigOption]) -> Vec<ConfigOption> {
    let rank = |category: Option<&str>| match category {
        Some("model") => 0,
        Some("thought_level") => 1,
        Some("mode") => 2,
        Some("model_config") => 3,
        _ => 4,
    };
    let mut indexed: Vec<(usize, &ConfigOption)> = options.iter().enumerate().collect();
    indexed.sort_by_key(|(i, o)| (rank(o.category.as_deref()), *i));
    indexed.into_iter().map(|(_, o)| o.clone()).collect()
}

/// What a control reads: the current choice's name, `On`/`Off` for a boolean, the option's name
/// for a kind this client cannot draw.
pub fn control_text(option: &ConfigOption) -> String {
    match &option.kind {
        wheel_proto::ConfigKind::Select { current, choices } => choices.iter().find(|c| &c.value == current).map(|c| c.name.clone()).unwrap_or_else(|| current.clone()),
        wheel_proto::ConfigKind::Boolean { current } => if *current { "On".into() } else { "Off".into() },
        wheel_proto::ConfigKind::Unknown => option.name.clone(),
    }
}

/// A control's tooltip (`ChatView.swift`'s `ConfigControl`): the option's name and description,
/// or why it cannot be set.
pub fn control_help(option: &ConfigOption) -> String {
    match &option.kind {
        wheel_proto::ConfigKind::Select { choices, .. } if choices.is_empty() => format!("The agent offers no values for \u{201c}{}\u{201d}.", option.name),
        wheel_proto::ConfigKind::Unknown => format!("This version of Wheel can't show a \u{201c}{}\u{201d} control.", option.name),
        _ => option.description.as_ref().map(|d| format!("{}: {d}", option.name)).unwrap_or_else(|| option.name.clone()),
    }
}

/// The environment variable `harness` reads its home from (`AgentInfo.homeVariable`).
pub fn home_variable(harness: &str) -> &'static str {
    match harness {
        "codex" => "CODEX_HOME",
        "pi" => "PI_CODING_AGENT_DIR",
        _ => "CLAUDE_CONFIG_DIR",
    }
}

/// An item's tooltip, as the Swift client's footer writes it: `text` is what the item shows,
/// `harness` the pane's agent.
pub fn item_help(item: &str, text: &str, context: Option<&ContextUse>, footer: Option<&AgentFooter>, harness: &str) -> Option<String> {
    Some(match item {
        "context" => context.map(help_text).unwrap_or_else(|| "The agent has not reported its context use yet".into()),
        "background" => "Background tasks the agent has running".into(),
        "directory" => format!("The directory the agent works in: {}", footer.map(|f| f.directory.as_str()).unwrap_or(text)),
        "home" => format!("{} is {text}. Set by agent.{harness}.home; chat.footer.items places this text.", home_variable(harness)),
        "branch" => "The branch checked out in the directory".into(),
        "changes" => "Lines added and removed against HEAD, untracked files not counted".into(),
        _ => return None,
    })
}

/// The stand-in's tooltip when items are hidden.
pub const MORE_HELP: &str = "More items don't fit in this width";

/// How many of the items, at `widths` and `spacing` apart, the row keeps in `available` points
/// (`ChatView.swift`'s `PriorityRow`): all of them while they fit, else as many from the front as
/// fit with the `marker`-wide "…" after them. Fewer than all means the row shows the "…".
pub fn fit(widths: &[f32], spacing: f32, marker: f32, available: f32) -> usize {
    let used = |count: usize| widths[..count].iter().sum::<f32>() + spacing * count.saturating_sub(1) as f32;
    let mut keep = widths.len();
    if used(keep) > available {
        while keep > 0 && used(keep) + spacing + marker > available {
            keep -= 1;
        }
    }
    keep
}

/// The value a control's option has in force, as its menu names values.
pub fn control_current(option: &ConfigOption) -> String {
    match &option.kind {
        wheel_proto::ConfigKind::Select { current, .. } => current.clone(),
        wheel_proto::ConfigKind::Boolean { current } => current.to_string(),
        wheel_proto::ConfigKind::Unknown => String::new(),
    }
}

/// The values a control's menu offers, (value, name); empty for one that cannot be set.
pub fn control_choices(option: &ConfigOption) -> Vec<(String, String)> {
    match &option.kind {
        wheel_proto::ConfigKind::Select { choices, .. } => choices.iter().map(|c| (c.value.clone(), c.name.clone())).collect(),
        wheel_proto::ConfigKind::Boolean { .. } => vec![("true".into(), "On".into()), ("false".into(), "Off".into())],
        wheel_proto::ConfigKind::Unknown => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::{ConfigChoice, ConfigKind};

    #[test]
    fn the_context_meter_reads_as_the_swift_client_writes_it() {
        let c = ContextUse { used: 74_000, size: 200_000 };
        assert_eq!(short_text(&c), "37%");
        assert_eq!(raw_text(&c), "74K / 200K");
        assert_eq!(help_text(&c), "Context window: 74,000 of 200,000 tokens used (37%)");
        assert_eq!(raw_text(&ContextUse { used: 1_260_000, size: 1_000_000 }), "1.3M / 1M");
        assert_eq!(fraction(&ContextUse { used: 300, size: 200 }), 1.);
        assert_eq!(fraction(&ContextUse { used: 1, size: 0 }), 0.);
        assert_eq!(tint(&ContextUse { used: 74, size: 100 }), 0x7fb89a);
        assert_eq!(tint(&ContextUse { used: 75, size: 100 }), 0xff9f0a);
        assert_eq!(tint(&ContextUse { used: 90, size: 100 }), 0xff453a);
    }

    #[test]
    fn the_footer_words_follow_the_swift_client() {
        let f = AgentFooter { directory: "/Users/me/Projects/x".into(), branch: Some("main".into()), added: 12, removed: 3, shells: 1, agents: 2 };
        assert_eq!(with_tilde(&f.directory, "/Users/me"), "~/Projects/x");
        assert_eq!(with_tilde("/Users/meow/x", "/Users/me"), "/Users/meow/x");
        assert_eq!(background_text(&f).as_deref(), Some("1 shell · 2 agents"));
        assert_eq!(background_text(&AgentFooter::default()), None);
        assert_eq!(changes_text(&f).as_deref(), Some("+12 \u{2212}3"));
        assert_eq!(changes_text(&AgentFooter { branch: None, ..f.clone() }), None);
        assert_eq!(shown_home(None, "claude", None), "~/.claude");
        assert_eq!(shown_home(Some(""), "codex", Some("/opt/codex")), "/opt/codex");
    }

    #[test]
    fn the_row_keeps_whole_items_from_the_front_and_leaves_room_for_the_marker() {
        let widths = [40., 30., 50.];
        // 40 + 10 + 30 + 10 + 50 = 140 fits exactly.
        assert_eq!(fit(&widths, 10., 8., 140.), 3);
        // One point short: the last goes, and 40 + 10 + 30 + 10 + 8 = 98 fits.
        assert_eq!(fit(&widths, 10., 8., 139.), 2);
        assert_eq!(fit(&widths, 10., 8., 97.), 1);
        assert_eq!(fit(&widths, 10., 8., 57.), 0);
        assert_eq!(fit(&[], 10., 8., 0.), 0);
    }

    #[test]
    fn tooltips_read_as_the_swift_client_writes_them() {
        let f = AgentFooter { directory: "/Users/me/x".into(), ..AgentFooter::default() };
        assert_eq!(item_help("directory", "~/x", None, Some(&f), "claude").as_deref(), Some("The directory the agent works in: /Users/me/x"));
        assert_eq!(item_help("home", "~/.codex", None, None, "codex").as_deref(), Some("CODEX_HOME is ~/.codex. Set by agent.codex.home; chat.footer.items places this text."));
        assert_eq!(item_help("context", "0%", None, None, "claude").as_deref(), Some("The agent has not reported its context use yet"));
        assert_eq!(item_help("bogus", "x", None, None, "claude"), None);
        let option = |kind| ConfigOption { id: "model".into(), name: "Model".into(), description: Some("Which model answers".into()), category: None, kind };
        assert_eq!(control_help(&option(ConfigKind::Boolean { current: true })), "Model: Which model answers");
        assert_eq!(control_help(&option(ConfigKind::Select { current: "a".into(), choices: vec![] })), "The agent offers no values for \u{201c}Model\u{201d}.");
        assert_eq!(control_help(&option(ConfigKind::Unknown)), "This version of Wheel can't show a \u{201c}Model\u{201d} control.");
    }

    #[test]
    fn the_items_setting_lists_each_once_in_order() {
        assert_eq!(items("branch, context,branch,,"), vec!["branch", "context"]);
        assert!(items("").is_empty());
    }

    #[test]
    fn config_controls_sort_by_category_and_read_their_current_choice() {
        let select = |id: &str, category: Option<&str>, current: &str| ConfigOption {
            id: id.into(), name: id.into(), description: None, category: category.map(str::to_owned),
            kind: ConfigKind::Select { current: current.into(), choices: vec![ConfigChoice { value: "a".into(), name: "Alpha".into(), description: None }, ConfigChoice { value: "b".into(), name: "Beta".into(), description: None }] },
        };
        let options = vec![select("extra", None, "a"), select("mode", Some("mode"), "b"), select("model", Some("model"), "a")];
        let sorted: Vec<String> = sorted_config(&options).iter().map(|o| o.id.clone()).collect();
        assert_eq!(sorted, vec!["model", "mode", "extra"]);
        assert_eq!(control_text(&options[1]), "Beta");
        assert_eq!(control_text(&ConfigOption { id: "x".into(), name: "x".into(), description: None, category: None, kind: ConfigKind::Boolean { current: true } }), "On");
        assert_eq!(control_choices(&options[0]).len(), 2);
    }
}
