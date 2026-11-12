//! An agent's conversations as the Conversations popover lists them, the port of WheelUI's
//! `ConversationPicker.swift` and of the answer bookkeeping in `AppModel.swift`
//! (`conversationActions`, `switchConversation`, the `conversations`, `conversation` and error
//! cases of `handle`): the list a page at a time, newest first, grouped under Today, Yesterday,
//! Last week and Earlier, a search over names and last replies, and the switch to a new or a past
//! conversation that the rows wait on.
use crate::markdown::{self, Block, Text};
use crate::strip::{clock, local_time};
use std::collections::HashMap;
use wheel_proto::ConversationInfo;

/// Whether a harness can fork a conversation at a turn (`AgentPanes.canFork`): Claude Code and
/// Codex can, Pi cannot.
pub fn can_fork(harness: Option<&str>) -> bool {
    matches!(harness, Some("claude" | "codex"))
}

/// One page of the popover (`ConversationPaging.popover`).
pub const PAGE: u32 = 20;
/// The note under the list while nothing was refused.
pub const NOTE: &str = "Includes conversations started outside Wheel.";

/// What the popover's list holds (`AgentChat.conversationListing`).
#[derive(Clone, Debug, PartialEq)]
pub enum Listing {
    Loading,
    Failed(String),
    Loaded(Vec<ConversationInfo>),
}

/// The popover open over one agent pane.
#[derive(Clone, Debug)]
pub struct Popover {
    pub pane: String,
    pub agent: String,
    pub query: String,
    pub listing: Listing,
    /// Older conversations remain after the pages held.
    pub more: bool,
    pub loading_more: bool,
    /// How many rows were held when the last page was asked for: one ask per count, as the Swift
    /// row's `onAppear` fires once per `.id(all.count)`, so a refused page is not asked again.
    pub asked_at: Option<usize>,
}

/// What a request about conversations waits for.
#[derive(Clone, Debug, PartialEq)]
pub enum Req {
    /// A page of the listing numbered `listing`; `more` adds to the list, else it replaces it.
    List { agent: String, listing: u64, more: bool },
    /// A switch to a new or a past conversation.
    Switch { agent: String },
}

/// Every agent's switch in progress, the requests waiting, and the listing count that tells a
/// stale page from the current one.
#[derive(Default)]
pub struct Conversations {
    pub open: Option<Popover>,
    /// The conversation each agent is switching to: "" for a new one.
    pub switching: HashMap<String, String>,
    /// The last refusal of a request about an agent's conversations.
    pub errors: HashMap<String, String>,
    pub reqs: HashMap<u64, Req>,
    listings: u64,
}

impl Conversations {
    /// A new listing begins: its first page replaces whatever an earlier one held.
    pub fn begin_listing(&mut self, req: u64, agent: &str) {
        self.listings += 1;
        self.reqs.insert(req, Req::List { agent: agent.to_owned(), listing: self.listings, more: false });
        if let Some(p) = self.open.as_mut().filter(|p| p.agent == agent) {
            p.listing = Listing::Loading;
            p.more = false;
            p.loading_more = false;
            p.asked_at = None;
        }
    }
    /// The page after the last row held, when there is one and none is on its way: (before, before_id).
    pub fn next_page(&self) -> Option<(String, String, String)> {
        let p = self.open.as_ref()?;
        let Listing::Loaded(held) = &p.listing else { return None };
        if !p.more || p.loading_more || p.asked_at == Some(held.len()) {
            return None;
        }
        let last = held.last()?;
        Some((p.agent.clone(), last.updated_at.clone()?, last.id.clone()))
    }
    pub fn begin_more(&mut self, req: u64, agent: &str) {
        self.reqs.insert(req, Req::List { agent: agent.to_owned(), listing: self.listings, more: true });
        if let Some(p) = self.open.as_mut() {
            p.loading_more = true;
            p.asked_at = match &p.listing {
                Listing::Loaded(held) => Some(held.len()),
                _ => None,
            };
        }
    }
    /// A switch to `id` (`None` for a new conversation) begins; false while one is already waiting.
    pub fn begin_switch(&mut self, req: u64, agent: &str, id: Option<&str>) -> bool {
        if self.switching.contains_key(agent) {
            return false;
        }
        self.errors.remove(agent);
        self.switching.insert(agent.to_owned(), id.unwrap_or_default().to_owned());
        self.reqs.insert(req, Req::Switch { agent: agent.to_owned() });
        true
    }
    /// A `conversations` page. Returns whether it answered a request of ours.
    pub fn listed(&mut self, req: u64, list: &[ConversationInfo], more: bool) -> bool {
        let Some(Req::List { agent, listing, more: adding }) = self.reqs.remove(&req) else { return false };
        let current = self.listings;
        // A page of an earlier listing, or of an agent whose popover closed, changes nothing.
        let Some(p) = self.open.as_mut().filter(|p| p.agent == agent && listing == current) else { return true };
        self.errors.remove(&agent);
        if adding {
            p.loading_more = false;
            if let Listing::Loaded(held) = &mut p.listing {
                let known: Vec<String> = held.iter().map(|c| c.id.clone()).collect();
                held.extend(list.iter().filter(|c| !known.contains(&c.id)).cloned());
                p.more = more;
            }
        } else {
            p.listing = Listing::Loaded(list.to_vec());
            p.more = more;
        }
        true
    }
    /// A `conversation` answer: the switch is done and the popover closes. Returns whether it was ours.
    pub fn switched(&mut self, req: u64) -> bool {
        let Some(Req::Switch { agent }) = self.reqs.remove(&req) else { return false };
        self.switching.remove(&agent);
        if self.open.as_ref().is_some_and(|p| p.agent == agent) {
            self.open = None;
        }
        true
    }
    /// wheeld refused a request of ours: the note says why, and what it waited on stops waiting.
    pub fn refused(&mut self, req: u64, message: &str) -> bool {
        let Some(r) = self.reqs.remove(&req) else { return false };
        match r {
            // A refused page of an earlier listing, or of a closed popover, says nothing.
            Req::List { agent, listing, more } => {
                let current = self.listings;
                if let Some(p) = self.open.as_mut().filter(|p| p.agent == agent && listing == current) {
                    if more {
                        p.loading_more = false;
                    } else {
                        p.listing = Listing::Failed(message.to_owned());
                    }
                    self.errors.insert(agent, message.to_owned());
                }
            }
            Req::Switch { agent } => {
                self.switching.remove(&agent);
                self.errors.insert(agent, message.to_owned());
            }
        }
        true
    }
    /// The connection dropped: no answer to a request sent on it comes, and the popover closes,
    /// since its list was the old wheeld's.
    pub fn link_lost(&mut self) {
        self.reqs.clear();
        self.switching.clear();
        self.open = None;
    }
    /// The note under the list: the agent's last refusal, else what the list includes.
    pub fn note(&self, agent: &str) -> String {
        self.errors.get(agent).cloned().unwrap_or_else(|| NOTE.to_owned())
    }
}

/// Why a row cannot be picked now, as its tooltip says (`conversationBlock`); None when it can.
pub fn block(switching: bool, busy: bool, queued: bool) -> Option<&'static str> {
    if switching {
        Some("Changing conversation…")
    } else if busy || queued {
        Some("Stop the turn first")
    } else {
        None
    }
}

/// What a conversation is called (`conversationName`): the agent's title with a kept
/// `<command-message>x</command-message>` read as `/x`, other tags dropped, whitespace collapsed.
pub fn name(info: &ConversationInfo) -> String {
    let Some(title) = &info.title else { return "Untitled conversation".into() };
    let mut title = title.clone();
    if let Some(open) = title.find("<command-message>") {
        let start = open + "<command-message>".len();
        if let Some(close) = title[start..].find("</command-message>") {
            title = format!("/{}", &title[start..start + close]);
        }
    }
    let mut plain = String::new();
    let mut tag_at = None;
    for (i, ch) in title.char_indices() {
        match ch {
            '<' if tag_at.is_none() => tag_at = Some(i),
            '>' if tag_at.is_some() => {
                tag_at = None;
                plain.push(' ');
            }
            _ if tag_at.is_none() => plain.push(ch),
            _ => {}
        }
    }
    // An unclosed `<` is no tag: what follows it stays as written.
    if let Some(i) = tag_at {
        plain.push_str(&title[i..]);
    }
    let words: Vec<&str> = plain.split_whitespace().collect();
    if words.is_empty() { "Untitled conversation".into() } else { words.join(" ") }
}

/// A reply as a card shows it (`conversationReply`): the block mark a list item or heading starts
/// with taken out and the inline Markdown parsed into marked runs.
pub fn reply(text: &str) -> Text {
    let trimmed = text.trim_start();
    let rest = if let Some(after) = trimmed.strip_prefix(|c| c == '#') {
        let after = after.trim_start_matches('#');
        after.strip_prefix(char::is_whitespace).map(str::trim_start).unwrap_or(trimmed)
    } else if let Some(after) = trimmed.strip_prefix(['-', '*', '+']).filter(|a| a.starts_with(char::is_whitespace)) {
        after.trim_start()
    } else {
        let digits = trimmed.len() - trimmed.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        match trimmed[digits..].strip_prefix(['.', ')']).filter(|a| digits > 0 && a.starts_with(char::is_whitespace)) {
            Some(after) => after.trim_start(),
            None => trimmed,
        }
    };
    match markdown::parse(rest).into_iter().next() {
        Some(Block::Paragraph(t) | Block::Heading { text: t, .. } | Block::Quote(t)) => t,
        _ => Text { text: rest.to_owned(), runs: vec![] },
    }
}

/// The day a conversation falls under in the popover's list (`ConversationDay`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Day {
    Today,
    Yesterday,
    LastWeek,
    Earlier,
}

impl Day {
    pub const ALL: [Day; 4] = [Day::Today, Day::Yesterday, Day::LastWeek, Day::Earlier];
    pub fn label(self) -> &'static str {
        match self {
            Day::Today => "TODAY",
            Day::Yesterday => "YESTERDAY",
            Day::LastWeek => "LAST WEEK",
            Day::Earlier => "EARLIER",
        }
    }
    /// The day of a conversation last worked in at `at` (unix seconds), seen at `now`.
    pub fn of(at: Option<i64>, now: i64) -> Day {
        let Some(at) = at else { return Day::Earlier };
        let (day, today) = (local_time(at).day_number, local_time(now).day_number);
        // A time ahead of this Mac's clock is today too.
        if day >= today {
            Day::Today
        } else if day == today - 1 {
            Day::Yesterday
        } else if at > now - 7 * 86_400 {
            Day::LastWeek
        } else {
            Day::Earlier
        }
    }
}

/// The time a row shows under its day heading (`conversationTime`): the clock for today and
/// yesterday, the date after that, so it never repeats the heading.
pub fn time(at: Option<i64>, day: Day) -> Option<String> {
    let l = local_time(at?);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    Some(match day {
        Day::Today | Day::Yesterday => clock(&l),
        Day::LastWeek | Day::Earlier => format!("{} {}", MONTHS[l.month], l.mday),
    })
}

/// When a conversation was last worked in, in unix seconds.
pub fn updated(info: &ConversationInfo) -> Option<i64> {
    crate::strip::parse_rfc3339(info.updated_at.as_deref()?)
}

/// The conversations whose name or last reply hold every word of `query`, ignoring case.
pub fn matches<'a>(list: &'a [ConversationInfo], query: &str) -> Vec<&'a ConversationInfo> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    list.iter().filter(|c| {
        let text = format!("{} {}", name(c), c.last_reply.as_deref().unwrap_or_default()).to_lowercase();
        words.iter().all(|w| text.contains(w.as_str()))
    }).collect()
}

/// The list as the popover draws it: each day that has rows, its rows newest first as given.
pub fn grouped<'a>(shown: &[&'a ConversationInfo], now: i64) -> Vec<(Day, Vec<&'a ConversationInfo>)> {
    Day::ALL.iter().filter_map(|day| {
        let rows: Vec<&ConversationInfo> = shown.iter().copied().filter(|c| Day::of(updated(c), now) == *day).collect();
        (!rows.is_empty()).then_some((*day, rows))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, title: Option<&str>, at: Option<&str>) -> ConversationInfo {
        ConversationInfo { id: id.into(), cwd: "/w".into(), title: title.map(Into::into), updated_at: at.map(Into::into), current: false, last_reply: None }
    }

    #[test]
    fn names_read_as_the_swift_picker_writes_them() {
        assert_eq!(name(&info("a", None, None)), "Untitled conversation");
        assert_eq!(name(&info("a", Some("<command-message>redesign</command-message> x"), None)), "/redesign");
        assert_eq!(name(&info("a", Some("fix <b>the</b>\n  bug"), None)), "fix the bug");
        assert_eq!(name(&info("a", Some("  "), None)), "Untitled conversation");
        assert_eq!(name(&info("a", Some("a < b"), None)), "a < b");
        assert_eq!(name(&info("a", Some("fix <b>x</b> a < b"), None)), "fix x a < b");
    }

    #[test]
    fn a_reply_loses_its_leading_block_mark() {
        assert_eq!(reply("## Done **now**").text, "Done now");
        assert_eq!(reply("- item one").text, "item one");
        assert_eq!(reply("3. third").text, "third");
        assert_eq!(reply("plain `code`").text, "plain code");
        assert!(reply("**bold** rest").runs.iter().any(|r| r.mark.bold));
    }

    #[test]
    fn days_and_times_follow_the_local_calendar() {
        let now = 1_800_000_000;
        assert_eq!(Day::of(Some(now), now), Day::Today);
        assert_eq!(Day::of(Some(now - 3 * 86_400), now), Day::LastWeek);
        assert_eq!(Day::of(Some(now - 30 * 86_400), now), Day::Earlier);
        assert_eq!(Day::of(None, now), Day::Earlier);
        assert_eq!(Day::of(Some(now + 3 * 86_400), now), Day::Today);
        assert!(time(Some(now), Day::Today).unwrap().ends_with('M'));
        assert!(time(Some(now - 30 * 86_400), Day::Earlier).unwrap().contains(' '));
    }

    #[test]
    fn a_page_from_an_earlier_listing_is_dropped_and_more_adds_without_repeats() {
        let mut c = Conversations { open: Some(Popover { pane: "p".into(), agent: "w".into(), query: String::new(), listing: Listing::Loading, more: false, loading_more: false, asked_at: None }), ..Default::default() };
        c.begin_listing(1, "w");
        c.begin_listing(2, "w");
        assert!(c.listed(1, &[info("old", None, None)], false));
        assert_eq!(c.open.as_ref().unwrap().listing, Listing::Loading);
        assert!(c.listed(2, &[info("a", None, Some("2026-01-02T00:00:00Z"))], true));
        assert_eq!(c.next_page(), Some(("w".into(), "2026-01-02T00:00:00Z".into(), "a".into())));
        c.begin_more(3, "w");
        assert_eq!(c.next_page(), None);
        // A refused page is not asked for again until the rows held change.
        assert!(c.refused(3, "busy"));
        assert_eq!(c.next_page(), None);
        assert_eq!(c.note("w"), "busy");
        c.open.as_mut().unwrap().asked_at = None;
        c.begin_more(4, "w");
        c.listed(4, &[info("a", None, None), info("b", None, None)], false);
        assert_eq!(c.note("w"), NOTE);
        let Listing::Loaded(held) = &c.open.as_ref().unwrap().listing else { panic!() };
        assert_eq!(held.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(!c.open.as_ref().unwrap().more);
    }

    #[test]
    fn a_switch_waits_once_and_its_answer_closes_the_popover() {
        let mut c = Conversations { open: Some(Popover { pane: "p".into(), agent: "w".into(), query: String::new(), listing: Listing::Loading, more: false, loading_more: false, asked_at: None }), ..Default::default() };
        assert!(c.begin_switch(5, "w", None));
        assert!(!c.begin_switch(6, "w", Some("x")));
        assert_eq!(c.switching.get("w").map(String::as_str), Some(""));
        assert!(c.switched(5));
        assert!(c.open.is_none() && c.switching.is_empty());
        c.begin_switch(7, "w", Some("x"));
        assert!(c.refused(7, "busy"));
        assert!(c.switching.is_empty());
        assert_eq!(c.errors.get("w").map(String::as_str), Some("busy"));
    }

    #[test]
    fn search_needs_every_word_in_the_name_or_reply() {
        let mut a = info("a", Some("Fix the parser"), None);
        a.last_reply = Some("Done with tokens".into());
        let list = vec![a, info("b", Some("Other"), None)];
        assert_eq!(matches(&list, "parser TOKENS").len(), 1);
        assert_eq!(matches(&list, "").len(), 2);
        assert!(matches(&list, "parser zebra").is_empty());
    }
}
