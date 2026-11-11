//! The usage strip along the window's bottom edge, as the Swift client builds it
//! (`UsageViews.swift`, WheelKit `Usage.swift`, ADR-0034): per shown agent its note or one entry
//! per account (the agent's mark, the labels `usage.labels` names, the metered windows' bars),
//! the entries that fit the width, a `+N` for the rest and the gauge that opens the Usage
//! window. The words and times are written the way the Swift views write them.
use wheel_proto::{UsageAccount, UsageAgent, UsageBoard, UsageKind, UsageMeter, UsageMeterLevel, UsageWindow};

/// The space between entries, and the bar's size.
pub const SPACING: f32 = 18.0;
pub const BAR_W: f32 = 104.0;
pub const BAR_H: f32 = 7.0;
/// The width the percentage takes beside a bar, right-aligned.
pub const PCT_W: f32 = 36.0;

/// The labels `usage.labels` may name, in the order Settings lists them.
pub const LABELS: [&str; 3] = ["billing", "folder", "email"];

/// One entry of the strip: an agent's note, or one of its accounts.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Note { agent: UsageAgent, note: String },
    Account { agent: UsageAgent, account: UsageAccount },
}

impl Item {
    pub fn id(&self) -> String {
        match self {
            Item::Note { agent, .. } => format!("note-{}", agent.id),
            Item::Account { agent, account } => format!("{}-{}", agent.id, account.id),
        }
    }
}

/// The agents the strip shows: those `usage.agents` lists.
pub fn shown(board: &UsageBoard) -> Vec<&UsageAgent> {
    board.agents.iter().filter(|a| a.shown).collect()
}

/// Why an agent has no account, in place of accounts; None when it has one.
pub fn agent_note(agent: &UsageAgent) -> Option<String> {
    agent.accounts.is_empty().then(|| agent.why.clone().unwrap_or_else(|| "no reading".into()))
}

/// Why an account has no reading, in place of its bars; None when it has one.
pub fn account_note(account: &UsageAccount) -> Option<String> {
    account.usage.reading.is_none().then(|| account.usage.why.clone().unwrap_or_else(|| "no reading".into()))
}

pub fn is_stale(account: &UsageAccount) -> bool {
    account.usage.kind == UsageKind::Stale
}

/// Every entry in strip order: per shown agent its note or its accounts.
pub fn items(board: &UsageBoard) -> Vec<Item> {
    shown(board).into_iter().flat_map(|agent| match agent_note(agent) {
        Some(note) => vec![Item::Note { agent: agent.clone(), note }],
        None => agent.accounts.iter().map(|account| Item::Account { agent: agent.clone(), account: account.clone() }).collect(),
    }).collect()
}

/// The level `usage.meters` gives `window` of `agent`; None when the window is off.
pub fn level(board: &UsageBoard, agent: &str, window: &str) -> Option<UsageMeterLevel> {
    board.meters.iter().find(|m| m.agent == agent && m.window == window).map(|m| m.level)
}

/// Whether the strip prints `agent`'s login folders: the folder label is on and the agent has
/// two or more logins on the board.
pub fn prints_folder(board: &UsageBoard, agent: &UsageAgent) -> bool {
    board.labels.iter().any(|l| l == "folder") && agent.accounts.len() >= 2
}

/// The labels printed beside `account`'s mark, in `usage.labels` order, each with its text: the
/// billing chip, the login folder (only when the agent has two or more logins) and the email.
pub fn labels(board: &UsageBoard, agent: &UsageAgent, account: &UsageAccount) -> Vec<(&'static str, String)> {
    board.labels.iter().filter_map(|name| {
        let label = LABELS.iter().copied().find(|l| l == name)?;
        let reading = account.usage.reading.as_ref();
        let text = match label {
            "billing" => reading.and_then(|r| r.billing.clone()),
            "folder" => prints_folder(board, agent).then(|| account.name.clone()),
            _ => reading.and_then(|r| r.email.clone()),
        };
        text.map(|t| (label, t))
    }).collect()
}

/// The strip's rows for `account` of `agent`: its metered windows in meter order. A metered
/// window the reading lacks draws nothing; nothing stands in for it.
pub fn rows(board: &UsageBoard, agent: &UsageAgent, account: &UsageAccount) -> Vec<(UsageWindow, UsageMeterLevel)> {
    let Some(reading) = &account.usage.reading else { return vec![] };
    board.meters.iter().filter(|m| m.agent == agent.id).filter_map(|m| reading.windows.iter().find(|w| w.id == m.window).map(|w| (w.clone(), m.level))).collect()
}

/// The `usage.meters` value after giving `window` of `agent` the level `level`: a changed entry
/// keeps its place in the strip, a new one goes last, None removes it (off).
pub fn meters_setting(board: &UsageBoard, agent: &str, window: &str, level: Option<UsageMeterLevel>) -> String {
    let mut out = board.meters.clone();
    match (out.iter().position(|m| m.agent == agent && m.window == window), level) {
        (Some(at), Some(level)) => out[at] = UsageMeter { agent: agent.into(), window: window.into(), level },
        (Some(at), None) => {
            out.remove(at);
        }
        (None, Some(level)) => out.push(UsageMeter { agent: agent.into(), window: window.into(), level }),
        (None, None) => {}
    }
    out.iter().map(|m| format!("{}:{}:{}", m.agent, m.window, level_name(m.level))).collect::<Vec<_>>().join(",")
}

pub fn level_name(level: UsageMeterLevel) -> &'static str {
    match level {
        UsageMeterLevel::Meter => "meter",
        UsageMeterLevel::Full => "full",
    }
}

/// The most entries, in order, that fit `room` beside the gauge, and the `+N` when some are left
/// out. `widths` are the entries' drawn widths.
pub fn fitting(widths: &[f32], more: f32, gauge: f32, room: f32) -> usize {
    if room <= 0. {
        return widths.len();
    }
    let mut used = gauge;
    for (index, width) in widths.iter().enumerate() {
        used += width + SPACING;
        let rest = if index + 1 < widths.len() { more + SPACING } else { 0. };
        if used + rest > room {
            return index;
        }
    }
    widths.len()
}

/// The bar's fill: neutral, orange from 80%, red from 95%, the track's colour when stale.
pub fn bar_fill(pct: f64, stale: bool) -> u32 {
    if stale {
        TRACK
    } else if pct >= 95. {
        0xff453a
    } else if pct >= 80. {
        0xff9f0a
    } else {
        0x9fb3a9
    }
}
pub const TRACK: u32 = 0x2c3d34;

/// A percentage as the bars write it: whole numbers.
pub fn percent(pct: f64) -> String {
    format!("{}%", pct.round() as i64)
}

/// "4 min", "2 h": how old a stale reading is.
pub fn age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / 86_400)
    }
}

/// "resets 4h", "resets 3d", "resets 25m"; None when the time is absent or unreadable.
pub fn resets(iso: Option<&str>, now: i64) -> Option<String> {
    let at = parse_rfc3339(iso?)?;
    let secs = at - now;
    Some(if secs <= 0 {
        "resets now".into()
    } else if secs < 3600 {
        format!("resets {}m", (secs / 60).max(1))
    } else if secs < 86_400 {
        format!("resets {}h", secs / 3600)
    } else {
        format!("resets {}d", secs / 86_400)
    })
}

/// The share of a window that has passed by `now`, from its reset time and length: 0 at the
/// window's start, 1 at its reset; None without a length or a reset time.
pub fn pace(resets_at: Option<&str>, length_secs: Option<u64>, now: i64) -> Option<f64> {
    let at = parse_rfc3339(resets_at?)?;
    let length = length_secs.filter(|l| *l > 0)? as f64;
    let elapsed = length - (at - now) as f64;
    Some((elapsed / length).clamp(0., 1.))
}

/// The reset time as a clock in this Mac's time zone: "7:00 PM" within the next day, else
/// "Tue 7:00 PM"; None when absent or unreadable.
pub fn reset_clock(iso: Option<&str>, now: i64) -> Option<String> {
    let at = parse_rfc3339(iso?)?;
    let local = local_time(at);
    let clock = format!("{}:{:02} {}", if local.hour % 12 == 0 { 12 } else { local.hour % 12 }, local.minute, if local.hour < 12 { "AM" } else { "PM" });
    Some(if at - now < 86_400 { clock } else { format!("{} {clock}", local.weekday) })
}

pub struct LocalTime {
    pub hour: u32,
    pub minute: u32,
    pub weekday: &'static str,
}

/// `unix` broken into the local hour, minute and weekday through the C library.
pub fn local_time(unix: i64) -> LocalTime {
    let t: libc::time_t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    LocalTime { hour: tm.tm_hour as u32, minute: tm.tm_min as u32, weekday: DAYS[(tm.tm_wday.clamp(0, 6)) as usize] }
}

/// An RFC 3339 time (`2026-10-17T20:00:00Z`, `…T20:00:00.5+02:00`) as seconds since the Unix epoch.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let num = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut at = 19;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let (oh, om) = (num(at + 1, at + 3)?, num(at + 4, at + 6)?);
            let secs = oh * 3600 + om * 60;
            if *sign == b'+' { secs } else { -secs }
        }
        _ => return None,
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::{UsageInfo, UsageReading};

    fn window(id: &str, pct: f64) -> UsageWindow {
        UsageWindow { id: id.into(), label: id.into(), pct, resets_at: Some("2026-10-17T20:00:00Z".into()), length_secs: Some(18_000) }
    }
    fn account(name: &str, windows: Vec<UsageWindow>, kind: UsageKind) -> UsageAccount {
        let reading = (kind != UsageKind::Missing).then(|| UsageReading { windows, billing: Some("Max 20x".into()), extra_usage: None, email: Some("a@b.c".into()), read_at: 0 });
        UsageAccount { id: name.into(), name: name.into(), dirs: vec![], usage: UsageInfo { kind, reading, age_secs: Some(400), why: (kind != UsageKind::Fresh).then(|| "HTTP 429".to_owned()), source: String::new() } }
    }
    fn agent(id: &str, accounts: Vec<UsageAccount>) -> UsageAgent {
        UsageAgent { id: id.into(), name: id.to_uppercase(), shown: true, readable: true, why: None, accounts }
    }
    fn meter(agent: &str, window: &str, level: UsageMeterLevel) -> UsageMeter {
        UsageMeter { agent: agent.into(), window: window.into(), level }
    }

    #[test]
    fn entries_labels_and_rows_follow_the_swift_board() {
        let board = UsageBoard {
            agents: vec![
                agent("claude", vec![account(".claude", vec![window("5h", 3.), window("weekly", 86.)], UsageKind::Fresh), account(".claude-work", vec![], UsageKind::Missing)]),
                agent("codex", vec![account(".codex-work", vec![window("weekly", 92.)], UsageKind::Stale)]),
                UsageAgent { id: "pi".into(), name: "pi".into(), shown: true, readable: false, why: Some("not detected".into()), accounts: vec![] },
                UsageAgent { id: "off".into(), name: "off".into(), shown: false, readable: true, why: None, accounts: vec![] },
            ],
            meters: vec![meter("claude", "5h", UsageMeterLevel::Meter), meter("claude", "weekly", UsageMeterLevel::Full), meter("codex", "5h", UsageMeterLevel::Meter), meter("codex", "weekly", UsageMeterLevel::Meter)],
            labels: vec!["folder".into(), "billing".into()],
        };
        let list = items(&board);
        assert_eq!(list.iter().map(Item::id).collect::<Vec<_>>(), ["claude-.claude", "claude-.claude-work", "codex-.codex-work", "note-pi"]);
        assert_eq!(list[3], Item::Note { agent: board.agents[2].clone(), note: "not detected".into() });
        let (claude, codex) = (&board.agents[0], &board.agents[1]);
        assert_eq!(labels(&board, claude, &claude.accounts[0]), vec![("folder", ".claude".to_owned()), ("billing", "Max 20x".to_owned())]);
        assert_eq!(labels(&board, codex, &codex.accounts[0]), vec![("billing", "Max 20x".to_owned())]);
        let rows_claude = rows(&board, claude, &claude.accounts[0]);
        assert_eq!(rows_claude.iter().map(|(w, l)| (w.id.as_str(), *l)).collect::<Vec<_>>(), [("5h", UsageMeterLevel::Meter), ("weekly", UsageMeterLevel::Full)]);
        assert_eq!(rows(&board, codex, &codex.accounts[0]).len(), 1);
        assert!(rows(&board, claude, &claude.accounts[1]).is_empty());
        assert_eq!(account_note(&claude.accounts[1]).as_deref(), Some("HTTP 429"));
        assert!(is_stale(&codex.accounts[0]));
        assert_eq!(age(account(".x", vec![], UsageKind::Stale).usage.age_secs.unwrap()), "6 min");
        assert_eq!(meters_setting(&board, "claude", "weekly", None), "claude:5h:meter,codex:5h:meter,codex:weekly:meter");
        assert_eq!(meters_setting(&board, "codex", "3d", Some(UsageMeterLevel::Full)), "claude:5h:meter,claude:weekly:full,codex:5h:meter,codex:weekly:meter,codex:3d:full");
        assert_eq!(meters_setting(&board, "claude", "5h", Some(UsageMeterLevel::Full)), "claude:5h:full,claude:weekly:full,codex:5h:meter,codex:weekly:meter");
    }

    #[test]
    fn the_times_and_figures_read_as_the_swift_views_write_them() {
        assert_eq!(parse_rfc3339("2026-10-17T20:00:00Z"), Some(1_792_267_200));
        assert_eq!(parse_rfc3339("2026-10-17T22:00:00.250+02:00"), Some(1_792_267_200));
        assert_eq!(parse_rfc3339("yesterday"), None);
        let now = 1_792_267_200 - 4 * 3600 - 30 * 60;
        assert_eq!(resets(Some("2026-10-17T20:00:00Z"), now).as_deref(), Some("resets 4h"));
        assert_eq!(resets(Some("2026-10-17T20:00:00Z"), now + 4 * 3600).as_deref(), Some("resets 30m"));
        assert_eq!(resets(Some("2026-10-17T20:00:00Z"), now - 3 * 86_400).as_deref(), Some("resets 3d"));
        assert_eq!(resets(Some("2026-10-17T20:00:00Z"), now + 86_400).as_deref(), Some("resets now"));
        assert_eq!(resets(None, now), None);
        assert!((pace(Some("2026-10-17T20:00:00Z"), Some(18_000), now).unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(pace(Some("2026-10-17T20:00:00Z"), None, now), None);
        assert_eq!(pace(Some("2026-10-17T20:00:00Z"), Some(100), now), Some(0.));
        assert_eq!(percent(73.5), "74%");
        assert_eq!(age(59), "59 s");
        assert_eq!(age(3_599), "59 min");
        assert_eq!(age(90_000), "1 d");
        let clock = reset_clock(Some("2026-10-17T20:00:00Z"), now).unwrap();
        assert!(clock.ends_with(" AM") || clock.ends_with(" PM"), "{clock}");
        let later = reset_clock(Some("2026-10-17T20:00:00Z"), now - 2 * 86_400).unwrap();
        assert_eq!(later.split(' ').count(), 3, "{later}");
        assert_eq!(bar_fill(79.9, false), 0x9fb3a9);
        assert_eq!(bar_fill(80., false), 0xff9f0a);
        assert_eq!(bar_fill(95., false), 0xff453a);
        assert_eq!(bar_fill(99., true), TRACK);
    }

    #[test]
    fn the_entries_that_fit_stay_and_the_rest_fold_into_the_count() {
        let widths = [200., 200., 200.];
        assert_eq!(fitting(&widths, 30., 20., 0.), 3);
        assert_eq!(fitting(&widths, 30., 20., 1000.), 3);
        // The third entry needs 20 + 3 × 218 = 674; with 600 the second must also leave room for "+N".
        assert_eq!(fitting(&widths, 30., 20., 600.), 2);
        assert_eq!(fitting(&widths, 30., 20., 480.), 1);
        assert_eq!(fitting(&widths, 30., 20., 200.), 0);
    }
}
