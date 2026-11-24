//! The words and sizes of the strip of running background tasks above an agent's composer, its
//! full view over the transcript, and the `● running m:ss` mark on a call's transcript row, as
//! the Swift client writes them (`apple/WheelUI/Sources/WheelUI/RunningStrip.swift`,
//! `RunningMark.swift`, `apple/WheelKit/Sources/WheelKit/AgentFooter.swift`'s `RunningTask`).
//! The tasks come from the footer (`AgentFooter::tasks`); `task_output` reads a shell's output.
use std::collections::{HashMap, HashSet};
use wheel_proto::{BackgroundTask, TaskKind};

/// Output older than this with the task still running reads as quiet.
pub const QUIET_AFTER_MS: u64 = 5 * 60 * 1000;

/// What the client holds about each agent's tasks beyond the footer: which strip rows are open,
/// which task the full view shows, the output read for each, and the reads in flight.
#[derive(Default)]
pub struct Running {
    pub agents: HashMap<String, AgentTasks>,
    /// The `task_output` requests in flight: which agent's task each reply belongs to.
    pub asks: HashMap<u64, (String, String)>,
}

#[derive(Default, Clone)]
pub struct AgentTasks {
    pub expanded: HashSet<String>,
    pub full: Option<String>,
    /// The output last read for each task, and whether earlier output was left out.
    pub outputs: HashMap<String, (String, bool)>,
    /// The `output_bytes` each task had when its output was last asked for.
    pub asked_at: HashMap<String, u64>,
}

impl AgentTasks {
    /// A task that ended takes its open row, full view and output with it.
    pub fn keep_only(&mut self, tasks: &[BackgroundTask]) {
        let running: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        self.expanded.retain(|id| running.contains(id.as_str()));
        if self.full.as_deref().is_some_and(|id| !running.contains(id)) {
            self.full = None;
        }
        self.outputs.retain(|id, _| running.contains(id.as_str()));
        self.asked_at.retain(|id, _| running.contains(id.as_str()));
    }
}

impl Running {
    /// The reply to a `task_output` ask, kept when its task still runs.
    pub fn read(&mut self, req: u64, text: &str, truncated: bool) -> bool {
        let Some((agent, task)) = self.asks.remove(&req) else { return false };
        let state = self.agents.entry(agent).or_default();
        if state.asked_at.contains_key(&task) {
            state.outputs.insert(task, (text.to_owned(), truncated));
        }
        true
    }
}

/// Seconds the task has run at `now` (unix ms).
pub fn elapsed_secs(task: &BackgroundTask, now: u64) -> u64 {
    now.saturating_sub(task.started_ms) / 1000
}

/// `m:ss`, or `h:mm:ss` from an hour, as the strip and the transcript show a running time.
pub fn clock(seconds: u64) -> String {
    if seconds >= 3600 { format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60) } else { format!("{}:{:02}", seconds / 60, seconds % 60) }
}

/// Seconds since a shell's output last grew (since its start when there is none yet), when that
/// is over five minutes; none while output is recent. Subagents write no output file, so they
/// are never quiet.
pub fn quiet_secs(task: &BackgroundTask, now: u64) -> Option<u64> {
    if task.kind != TaskKind::Shell {
        return None;
    }
    let since = now.saturating_sub(task.last_output_ms.unwrap_or(task.started_ms));
    (since >= QUIET_AFTER_MS).then_some(since / 1000)
}

/// "1 tool call", "26.7k tokens": tokens in thousands with one decimal.
fn thousands(n: u64) -> String {
    format!("{:.1}k tokens", n as f64 / 1000.)
}

/// The line the strip shows beside the name: the latest output line, a subagent's progress, or
/// why there is none.
pub fn latest(task: &BackgroundTask) -> String {
    match task.kind {
        TaskKind::Agent => {
            let mut parts = vec![];
            if let Some(n) = task.tool_uses {
                parts.push(if n == 1 { "1 tool call".to_owned() } else { format!("{n} tool calls") });
            }
            if let Some(tool) = &task.last_tool {
                parts.push(format!("last: {tool}"));
            }
            if let Some(tokens) = task.tokens {
                parts.push(thousands(tokens));
            }
            if parts.is_empty() { "started".into() } else { parts.join(" \u{b7} ") }
        }
        TaskKind::Shell => task.output_error.clone().or_else(|| task.tail.last().cloned()).unwrap_or_else(|| "no output yet".into()),
    }
}

/// What the strip row shows beside the name: nothing while the row is open, a quiet shell's
/// silence ahead of its latest line, else the latest line.
pub fn row_text(task: &BackgroundTask, now: u64, open: bool) -> String {
    if open {
        return String::new();
    }
    match quiet_secs(task, now) {
        Some(secs) => format!("no output for {}m \u{b7} {}", secs / 60, latest(task)),
        None => latest(task),
    }
}

/// A duration as the Swift client's `formatElapsed` writes it.
pub fn elapsed_words(seconds: f64) -> String {
    if seconds < 1. {
        "<1s".into()
    } else if seconds < 10. {
        format!("{seconds:.1}s")
    } else if seconds < 60. {
        format!("{}s", seconds as u64)
    } else {
        format!("{}m {}s", seconds as u64 / 60, seconds as u64 % 60)
    }
}

/// A moment as the facts line writes it, `1:19pm`, in local time.
pub fn time_of_day(ms: u64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let hour = match tm.tm_hour % 12 { 0 => 12, h => h };
    format!("{hour}:{:02}{}", tm.tm_min, if tm.tm_hour >= 12 { "pm" } else { "am" })
}

/// `started 1:19pm · last output 2s ago · 21 bytes · /private/tmp/…/b8jhu969e.output`, with only
/// the facts the task has.
pub fn facts(task: &BackgroundTask, now: u64) -> String {
    let mut parts = vec![format!("started {}", time_of_day(task.started_ms))];
    match (task.kind, task.last_output_ms, task.tokens) {
        (TaskKind::Agent, _, Some(tokens)) => parts.push(thousands(tokens)),
        (TaskKind::Shell, Some(last), _) => parts.push(format!("last output {} ago", elapsed_words(now.saturating_sub(last) as f64 / 1000.))),
        _ => {}
    }
    if task.output_bytes > 0 {
        parts.push(crate::composer::byte_count(task.output_bytes));
    }
    if let Some(path) = &task.output_path {
        parts.push(path.clone());
    }
    parts.join(" \u{b7} ")
}

/// A row opened: a shell's last lines (a subagent's brief instead), at most eight.
pub fn detail_lines(task: &BackgroundTask) -> Vec<String> {
    let lines: Vec<String> = match task.kind {
        TaskKind::Agent => task.command.clone().unwrap_or_else(|| latest(task)).split('\n').map(str::to_owned).collect(),
        TaskKind::Shell if task.tail.is_empty() => vec![latest(task)],
        TaskKind::Shell => task.tail.clone(),
    };
    lines.into_iter().take(8).collect()
}

/// What the full view's output area shows: the output read for the task; until that arrives,
/// its last lines from the footer; for a subagent, which writes no output file, its progress.
pub fn full_lines(task: &BackgroundTask, read: Option<&(String, bool)>) -> Vec<String> {
    if let Some((text, _)) = read.filter(|(text, _)| !text.is_empty()) {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        return lines;
    }
    if let Some(error) = &task.output_error {
        return vec![error.clone()];
    }
    match task.kind {
        TaskKind::Agent => vec![latest(task)],
        TaskKind::Shell if task.tail.is_empty() => vec!["no output yet".into()],
        TaskKind::Shell => task.tail.clone(),
    }
}

/// The task whose call a transcript row holds, for the running mark.
pub fn marked<'a>(tasks: &'a [BackgroundTask], call: &str) -> Option<&'a BackgroundTask> {
    tasks.iter().find(|t| t.call.as_deref() == Some(call))
}

/// The mark itself, `● running 40:12`.
pub fn mark_text(task: &BackgroundTask, now: u64) -> String {
    format!("\u{25cf} running {}", clock(elapsed_secs(task, now)))
}

/// The mark's colour: green, amber once the task's output has been quiet five minutes.
pub fn tint(task: &BackgroundTask, now: u64) -> u32 {
    if quiet_secs(task, now).is_some() { 0xff9f0a } else { 0x30d158 }
}

/// A strip row's height: its 4 pt of padding above and below one `text_xs` line.
pub const ROW: f32 = 24.;
/// A `text_xs` line, and the lines' box inside an open row: 4 pt of padding above and below.
pub const LINE: f32 = 16.;

/// An open row's detail: 4 pt under the row, the lines in their box, 4 pt, the facts line.
pub fn detail_height(lines: usize) -> f32 {
    4. + 8. + LINE * lines as f32 + 4. + LINE
}

/// The strip's height above the composer: its 1 pt border, each row, a 1 pt rule between rows,
/// each open row's detail, and the 4 pt gap above it; none while no task runs.
pub fn strip_height(tasks: &[BackgroundTask], open: &HashSet<String>) -> f32 {
    if tasks.is_empty() {
        return 0.;
    }
    let details: f32 = tasks.iter().filter(|t| open.contains(&t.id)).map(|t| detail_height(detail_lines(t).len())).sum();
    4. + 2. + ROW * tasks.len() as f32 + (tasks.len() - 1) as f32 + details
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(tail: &[&str]) -> BackgroundTask {
        BackgroundTask { id: "bash-1".into(), kind: TaskKind::Shell, name: "Build".into(), started_ms: 1_000_000, tail: tail.iter().map(|s| (*s).to_owned()).collect(), ..Default::default() }
    }

    fn agent() -> BackgroundTask {
        BackgroundTask { id: "agent-1".into(), kind: TaskKind::Agent, name: "find callers".into(), command: Some("Find every caller\nof AgentFooter".into()), tool_uses: Some(2), last_tool: Some("Bash".into()), tokens: Some(26_666), ..Default::default() }
    }

    #[test]
    fn a_running_time_reads_m_ss_and_h_mm_ss() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(2412), "40:12");
        assert_eq!(clock(3600 + 61), "1:01:01");
        assert_eq!(elapsed_secs(&shell(&[]), 1_000_000 + 72_900), 72);
    }

    #[test]
    fn the_latest_line_is_the_output_a_subagents_progress_or_why_there_is_none() {
        assert_eq!(latest(&shell(&["a", "line 3"])), "line 3");
        assert_eq!(latest(&shell(&[])), "no output yet");
        assert_eq!(latest(&BackgroundTask { output_error: Some("no output file".into()), ..shell(&["x"]) }), "no output file");
        assert_eq!(latest(&agent()), "2 tool calls \u{b7} last: Bash \u{b7} 26.7k tokens");
        assert_eq!(latest(&BackgroundTask { kind: TaskKind::Agent, ..Default::default() }), "started");
    }

    #[test]
    fn only_a_shell_with_no_output_for_five_minutes_is_quiet() {
        let t = shell(&["x"]);
        assert_eq!(quiet_secs(&t, 1_000_000 + 299_999), None);
        assert_eq!(quiet_secs(&t, 1_000_000 + 300_000), Some(300));
        assert_eq!(quiet_secs(&BackgroundTask { last_output_ms: Some(1_290_000), ..t.clone() }, 1_300_000), None);
        assert_eq!(quiet_secs(&agent(), u64::MAX / 2), None);
        assert_eq!(row_text(&t, 1_000_000 + 420_000, false), "no output for 7m \u{b7} x");
        assert_eq!(row_text(&t, 1_000_000 + 420_000, true), "");
        assert_eq!(tint(&t, 1_000_000), 0x30d158);
        assert_eq!(tint(&t, 1_000_000 + 300_000), 0xff9f0a);
    }

    #[test]
    fn durations_read_as_the_swift_client_writes_them() {
        assert_eq!(elapsed_words(0.4), "<1s");
        assert_eq!(elapsed_words(2.25), "2.2s");
        assert_eq!(elapsed_words(42.9), "42s");
        assert_eq!(elapsed_words(125.), "2m 5s");
    }

    #[test]
    fn an_open_row_shows_the_last_eight_lines_or_the_brief() {
        let lines: Vec<String> = (1..=12).map(|i| format!("l{i}")).collect();
        let t = BackgroundTask { tail: lines.clone(), ..shell(&[]) };
        assert_eq!(detail_lines(&t), lines[..8]);
        assert_eq!(detail_lines(&agent()), vec!["Find every caller", "of AgentFooter"]);
        assert_eq!(detail_lines(&shell(&[])), vec!["no output yet"]);
    }

    #[test]
    fn the_full_view_shows_the_read_output_then_the_footers_lines() {
        let t = shell(&["tail"]);
        assert_eq!(full_lines(&t, Some(&("one\ntwo\n".into(), false))), vec!["one", "two"]);
        assert_eq!(full_lines(&t, Some(&(String::new(), false))), vec!["tail"]);
        assert_eq!(full_lines(&t, None), vec!["tail"]);
        assert_eq!(full_lines(&shell(&[]), None), vec!["no output yet"]);
        assert_eq!(full_lines(&agent(), None), vec!["2 tool calls \u{b7} last: Bash \u{b7} 26.7k tokens"]);
    }

    #[test]
    fn the_facts_line_has_only_the_facts_the_task_has() {
        let t = BackgroundTask { output_bytes: 21, last_output_ms: Some(1_040_000), output_path: Some("/tmp/x.output".into()), ..shell(&[]) };
        let line = facts(&t, 1_042_000);
        assert!(line.starts_with("started "), "{line}");
        assert!(line.ends_with(" \u{b7} last output 2.0s ago \u{b7} 21 bytes \u{b7} /tmp/x.output"), "{line}");
        assert!(facts(&agent(), 0).ends_with(" \u{b7} 26.7k tokens"));
    }

    #[test]
    fn the_mark_goes_on_the_row_that_holds_the_call() {
        let tasks = vec![BackgroundTask { call: Some("bg-call-1".into()), ..shell(&[]) }, agent()];
        assert_eq!(marked(&tasks, "bg-call-1").map(|t| t.id.as_str()), Some("bash-1"));
        assert!(marked(&tasks, "call-9").is_none());
        assert_eq!(mark_text(&tasks[0], 1_000_000 + 2_412_000), "\u{25cf} running 40:12");
    }

    #[test]
    fn the_strip_takes_room_for_its_rows_and_each_open_detail() {
        let tasks = vec![shell(&["a"]), agent()];
        assert_eq!(strip_height(&[], &HashSet::new()), 0.);
        assert_eq!(strip_height(&tasks, &HashSet::new()), 4. + 2. + 48. + 1.);
        let open: HashSet<String> = ["agent-1".to_owned()].into();
        assert_eq!(strip_height(&tasks, &open), 55. + detail_height(2));
    }

    #[test]
    fn a_task_that_ended_takes_its_open_row_and_output_with_it() {
        let mut state = AgentTasks { full: Some("bash-1".into()), ..Default::default() };
        state.expanded.insert("bash-1".into());
        state.expanded.insert("agent-1".into());
        state.outputs.insert("bash-1".into(), ("x".into(), false));
        state.keep_only(&[agent()]);
        assert_eq!(state.full, None);
        assert!(state.outputs.is_empty());
        assert_eq!(state.expanded.len(), 1);
    }

    #[test]
    fn a_reply_is_kept_only_for_a_task_that_was_asked_about() {
        let mut running = Running::default();
        running.asks.insert(7, ("w1".into(), "bash-1".into()));
        running.agents.entry("w1".into()).or_default().asked_at.insert("bash-1".into(), 21);
        assert!(running.read(7, "out", true));
        assert_eq!(running.agents["w1"].outputs["bash-1"], ("out".to_owned(), true));
        assert!(!running.read(7, "again", false));
    }
}
