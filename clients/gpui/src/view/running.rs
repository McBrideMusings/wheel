//! The running background tasks of an agent (`RunningStrip.swift`, `RunningMark.swift`): a strip
//! above its composer with a row per task, a row's last lines, the full view over the transcript
//! that reads a task's output from wheeld (`task_output`), and what `/state` reports of them.
//! The words and sizes are `crate::running`.
use super::*;
use crate::running::{self, AgentTasks};
use crate::tooltip::tip;
use serde_json::{Value, json};
use wheel_proto::{BackgroundTask, TaskKind};

/// What an agent pane draws for its tasks: the strip, the room it takes, and the full view that
/// replaces the transcript column while open.
#[derive(Default)]
pub(super) struct TaskParts {
    pub strip: Option<gpui::Div>,
    pub height: f32,
    pub full: Option<gpui::Div>,
}

const RULE: u32 = 0x2c3d34;
const MUTED: u32 = 0x8fa89b;
const FAINT: u32 = 0x6f8a7c;

fn dot(tint: u32, size: f32) -> gpui::Div {
    div().flex_none().w(px(size)).h(px(size)).rounded_full().bg(rgb(tint))
}

impl Client {
    /// `agent`'s running tasks, oldest first.
    pub fn tasks_of(&self, agent: &str) -> &[BackgroundTask] {
        self.transcripts.get(agent).and_then(|t| t.footer.as_ref()).map(|f| f.tasks.as_slice()).unwrap_or_default()
    }

    /// The task the full view shows over `agent`'s transcript.
    pub fn full_task(&self, agent: &str) -> Option<&str> {
        self.running.agents.get(agent).and_then(|s| s.full.as_deref())
    }

    /// Whether `agent`'s strip row for `task` is open to its last lines.
    pub fn task_open(&self, agent: &str, task: &str) -> bool {
        self.running.agents.get(agent).is_some_and(|s| s.expanded.contains(task))
    }

    /// A press on a strip row: it opens to its last lines, or shuts.
    pub fn toggle_task(&mut self, agent: &str, task: &str) {
        self.press(format!("task row {task}"));
        let state = self.running.agents.entry(agent.to_owned()).or_default();
        if !state.expanded.remove(task) {
            state.expanded.insert(task.to_owned());
        }
    }

    /// The expand control on a row, and a tab of the full view: the full view shows `task`.
    pub fn open_full_task(&mut self, agent: &str, task: &str) {
        self.press(format!("task full {task}"));
        self.running.agents.entry(agent.to_owned()).or_default().full = Some(task.to_owned());
        self.sync_tasks();
    }

    /// Close: the full view shuts. False when none was open.
    pub fn close_full_task(&mut self, agent: &str) -> bool {
        self.press(format!("task close {agent}"));
        self.running.agents.get_mut(agent).and_then(|s| s.full.take()).is_some()
    }

    /// Show in Transcript: the transcript scrolls to the call that started the task and the full
    /// view shuts.
    pub fn reveal_task_call(&mut self, agent: &str, task: &str) {
        self.press(format!("task reveal {task}"));
        let Some(call) = self.tasks_of(agent).iter().find(|t| t.id == task).and_then(|t| t.call.clone()) else { return };
        if let Some(row) = self.transcripts.get(agent).and_then(|t| t.rows.iter().position(|r| matches!(&r.kind, Kind::Tool { id, .. } if *id == call))) {
            self.log(format!("transcript {agent}: shows call {call}"));
            self.reveal(agent, row);
        }
        if let Some(state) = self.running.agents.get_mut(agent) {
            state.full = None;
        }
    }

    /// Keep each agent's open rows, full view and output to the tasks that still run, and ask for
    /// the output of the task the full view shows when it has grown since the last ask.
    pub(super) fn sync_tasks(&mut self) {
        let agents: Vec<String> = self.running.agents.keys().cloned().collect();
        for agent in agents {
            let tasks = self.tasks_of(&agent).to_vec();
            let link_up = self.link == Link::Up;
            let ask = self.running.agents.get_mut(&agent).and_then(|state| {
                state.keep_only(&tasks);
                let shown = state.full.clone()?;
                let task = tasks.iter().find(|t| t.id == shown)?;
                if state.asked_at.get(&shown) == Some(&task.output_bytes) || !link_up {
                    return None;
                }
                state.asked_at.insert(shown.clone(), task.output_bytes);
                Some(shown)
            });
            if let Some(shown) = ask {
                let req = self.next_req;
                self.next_req += 1;
                self.running.asks.insert(req, (agent.clone(), shown.clone()));
                self.log(format!("task output ask {agent} {shown} req {req}"));
                self.send(&ClientMsg::TaskOutput { req: Some(req), agent, task: shown });
            }
        }
    }

    /// wheeld's `task_output` reply.
    pub(super) fn on_running_message(&mut self, message: &ServerMsg) -> bool {
        let ServerMsg::TaskOutput { req, text, truncated } = message else { return false };
        // A reply for a task that ended meanwhile is not kept.
        if let Some((agent, task)) = self.running.asks.get(req).cloned() {
            self.log(format!("task output {agent} {task}: {} bytes, truncated {truncated}", text.len()));
        }
        self.running.read(*req, text, *truncated)
    }

    /// wheeld refused a `task_output` ask.
    pub(super) fn running_refused(&mut self, req: u64, message: &str) {
        if let Some((agent, task)) = self.running.asks.remove(&req) {
            self.log(format!("task output {agent} {task} refused: {message}"));
        }
    }

    /// Whether any agent has a task running, which a clock on screen needs a redraw each second for.
    pub(super) fn any_task_running(&self) -> bool {
        self.transcripts.values().any(|t| t.footer.as_ref().is_some_and(|f| !f.tasks.is_empty()))
    }

    /// The footer as `/state` reports it (`DebugServer.swift`): the status line's count, each
    /// task with what its row shows, the task the full view is on, and the transcript rows
    /// carrying a running mark.
    pub fn running_json(&self, agent: &str) -> Value {
        let Some(footer) = self.transcripts.get(agent).and_then(|t| t.footer.as_ref()) else { return Value::Null };
        let now = settings::now_ms();
        let none = AgentTasks::default();
        let state = self.running.agents.get(agent).unwrap_or(&none);
        let tasks: Vec<Value> = footer.tasks.iter().map(|task| json!({
            "id": task.id, "kind": match task.kind { TaskKind::Shell => "shell", TaskKind::Agent => "agent" }, "name": task.name, "command": task.command, "call": task.call,
            "started_ms": task.started_ms, "elapsed": running::clock(running::elapsed_secs(task, now)), "latest": running::latest(task), "tail": task.tail,
            "output_bytes": task.output_bytes, "quiet_secs": running::quiet_secs(task, now), "output_error": task.output_error,
            "expanded": state.expanded.contains(&task.id), "output_read_bytes": state.outputs.get(&task.id).map(|(text, _)| text.len()),
            "output_read": state.outputs.get(&task.id).map(|(text, _)| text),
            "detail": state.expanded.contains(&task.id).then(|| running::detail_lines(task)),
            "strip_row_drawn": self.control_bounds(agent, &format!("task-row:{}", task.id)).is_some(),
        })).collect();
        let marks: Vec<Value> = self.transcripts.get(agent).map(|t| t.rows.iter().enumerate().filter_map(|(ix, row)| {
            let Kind::Tool { id, .. } = &row.kind else { return None };
            running::marked(&footer.tasks, id).map(|task| json!({"row": ix, "call": id, "task": task.id, "text": running::mark_text(task, now)}))
        }).collect()).unwrap_or_default();
        json!({"background_text": crate::footer::background_text(footer), "branch": footer.branch, "tasks": tasks, "full_task": state.full, "marks": marks, "ticking": self.redraw_due.is_some(),
            "full_view_drawn": state.full.as_ref().is_some_and(|task| self.control_bounds(agent, &format!("task-close:{task}")).is_some())})
    }

    /// The strip, its room and the full view for `agent`'s pane.
    pub(super) fn task_parts(&self, pane: &str, agent: Option<&str>, cx: &mut Context<Self>) -> TaskParts {
        let Some(agent) = agent else { return TaskParts::default() };
        let tasks = self.tasks_of(agent);
        if tasks.is_empty() {
            return TaskParts::default();
        }
        let none = AgentTasks::default();
        let state = self.running.agents.get(agent).unwrap_or(&none);
        let height = running::strip_height(tasks, &state.expanded);
        let full = state.full.as_ref().and_then(|id| tasks.iter().find(|t| &t.id == id)).map(|task| self.full_view(agent, task, tasks, state, cx));
        TaskParts { strip: Some(self.strip(pane, agent, tasks, state, height, cx)), height, full }
    }

    fn strip(&self, pane: &str, agent: &str, tasks: &[BackgroundTask], state: &AgentTasks, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let now = settings::now_ms();
        let mut rows = div().flex_none().flex().flex_col().mt_1().h(px(height - 4.)).rounded_md().border_1().border_color(rgb(RULE)).bg(rgb(0x121a16)).overflow_hidden();
        for (i, task) in tasks.iter().enumerate() {
            if i > 0 {
                rows = rows.child(div().flex_none().h(px(1.)).bg(rgb(RULE)));
            }
            let open = state.expanded.contains(&task.id);
            let quiet = running::quiet_secs(task, now).is_some();
            let (id, a) = (task.id.clone(), agent.to_owned());
            let (full_id, full_agent) = (task.id.clone(), agent.to_owned());
            let header = div().id(gpui::ElementId::Name(format!("task-row-{pane}-{}", task.id).into())).relative().flex_none().h(px(running::ROW)).px_2()
                .flex().items_center().gap_2().cursor_pointer().text_xs()
                .tooltip(tip(task.command.as_deref().unwrap_or(&task.name)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.toggle_task(&a, &id);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(dot(if quiet { 0xff9f0a } else { 0x30d158 }, 6.))
                .child(div().flex_none().max_w(px(220.)).truncate().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xd9e8e1)).child(task.name.clone()))
                .child(div().flex_1().min_w_0().truncate().font_family("Menlo").text_color(rgb(if quiet { 0xff9f0a } else { MUTED })).child(running::row_text(task, now, open)))
                .child(div().flex_none().w(px(52.)).flex().justify_end().text_color(rgb(0xd9e8e1)).child(running::clock(running::elapsed_secs(task, now))))
                .child(div().id(gpui::ElementId::Name(format!("task-full-{pane}-{}", task.id).into())).relative().flex_none().cursor_pointer().text_color(rgb(MUTED))
                    .tooltip(tip("Show this task's whole output over the transcript"))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.open_full_task(&full_agent, &full_id);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child("\u{2922}")
                    .child(recorded(agent, &format!("task-full:{}", task.id), &self.controls)))
                .child(recorded(agent, &format!("task-row:{}", task.id), &self.controls));
            let mut row = div().flex_none().flex().flex_col().child(header);
            if open {
                let lines = running::detail_lines(task);
                let lines_box = div().flex_none().h(px(8. + running::LINE * lines.len() as f32)).px(px(7.)).py(px(4.)).rounded(px(5.)).bg(rgb(0x1a2621)).overflow_hidden()
                    .flex().flex_col().text_xs().font_family("Menlo").text_color(rgb(0xd9e8e1))
                    .children(lines.into_iter().map(|l| div().flex_none().h(px(running::LINE)).truncate().child(if l.is_empty() { " ".to_owned() } else { l })));
                row = row.child(div().flex_none().h(px(running::detail_height(running::detail_lines(task).len()))).pt(px(4.)).px(px(22.)).flex().flex_col().gap(px(4.))
                    .child(lines_box)
                    .child(div().flex_none().h(px(running::LINE)).truncate().text_xs().text_color(rgb(FAINT)).child(running::facts(task, now))));
            }
            rows = rows.child(row);
        }
        rows
    }

    /// The full view of `task` over the transcript: its header, a tab per task, the command or
    /// brief, the recent output following its end, and the task's facts.
    fn full_view(&self, agent: &str, task: &BackgroundTask, tasks: &[BackgroundTask], state: &AgentTasks, cx: &mut Context<Self>) -> gpui::Div {
        let now = settings::now_ms();
        let quiet = running::quiet_secs(task, now).is_some();
        let read = state.outputs.get(&task.id);
        let lines = running::full_lines(task, read);
        let button = |label: &'static str, id: String, enabled: bool, run: Box<dyn Fn(&mut Client)>, cx: &mut Context<Self>| {
            let el = div().id(gpui::ElementId::Name(format!("{id}-{agent}").into())).relative().flex_none().px_2().py(px(2.)).rounded_sm().text_xs()
                .bg(rgb(0x31483e)).text_color(rgb(if enabled { 0xd9e8e1 } else { FAINT }))
                .child(label).child(recorded(agent, &id, &self.controls));
            if enabled {
                el.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    run(this);
                    cx.stop_propagation();
                    cx.notify();
                }))
            } else {
                el
            }
        };
        let (a, id) = (agent.to_owned(), task.id.clone());
        let reveal = button("Show in Transcript", format!("task-reveal:{}", task.id), task.call.is_some(), Box::new(move |this| this.reveal_task_call(&a, &id)), cx);
        let a = agent.to_owned();
        let close = button("Close", format!("task-close:{}", task.id), true, Box::new(move |this| {
            this.close_full_task(&a);
        }), cx);
        let header = div().flex_none().h(px(36.)).px_3().flex().items_center().gap_2()
            .child(dot(if quiet { 0xff9f0a } else { 0x30d158 }, 7.))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xd9e8e1)).child(task.name.clone()))
            .child(div().flex_none().w(px(60.)).flex().justify_end().text_color(rgb(0xd9e8e1)).child(running::clock(running::elapsed_secs(task, now))))
            .child(reveal).child(close);
        let rule = || div().flex_none().h(px(1.)).bg(rgb(RULE));
        let mut view = div().size_full().flex().flex_col().overflow_hidden().bg(rgb(0x0b100e)).border_1().border_color(rgb(RULE)).rounded_md().child(header).child(rule());
        if tasks.len() > 1 {
            let mut tabs = div().flex_none().h(px(28.)).px_3().flex().items_center().gap_1().text_xs();
            for other in tasks {
                let (a, id) = (agent.to_owned(), other.id.clone());
                let shown = other.id == task.id;
                tabs = tabs.child(div().id(gpui::ElementId::Name(format!("task-tab-{agent}-{}", other.id).into())).relative().flex_none().px_2().py(px(2.)).rounded_sm().cursor_pointer()
                    .bg(rgb(if shown { 0x31483e } else { 0x1a2621 })).text_color(rgb(if shown { 0xd9e8e1 } else { MUTED }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.open_full_task(&a, &id);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(format!("{}  {}", other.name, running::clock(running::elapsed_secs(other, now))))
                    .child(recorded(agent, &format!("task-tab:{}", other.id), &self.controls)));
            }
            view = view.child(tabs).child(rule());
        }
        if let Some(command) = &task.command {
            let first = command.lines().next().unwrap_or_default();
            view = view.child(div().flex_none().h(px(24.)).px_3().flex().items_center().truncate().text_xs().font_family("Menlo").text_color(rgb(MUTED))
                .child(if task.kind == TaskKind::Shell { format!("$ {first}") } else { first.to_owned() })).child(rule());
        }
        let mut output = div().flex_1().min_h_0().px_3().py_2().flex().flex_col().justify_end().overflow_hidden().text_xs().font_family("Menlo").text_color(rgb(0xd9e8e1));
        if read.is_some_and(|(_, truncated)| *truncated) {
            output = output.child(div().flex_none().h(px(running::LINE)).text_color(rgb(FAINT)).child("Earlier output left out: this shows the last 256 KB."));
        }
        output = output.children(lines.into_iter().map(|l| div().flex_none().h(px(running::LINE)).truncate().child(if l.is_empty() { " ".to_owned() } else { l })));
        view.child(output).child(rule())
            .child(div().flex_none().h(px(24.)).px_3().flex().items_center().truncate().text_xs().text_color(rgb(FAINT)).child(format!("{} \u{b7} following the end", running::facts(task, now))))
    }
}
