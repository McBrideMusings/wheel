# Orchestrator redesign

Design from the 22 Nov 2026 session. It replaces the flow in ADR-0054 and `docs/orchestrator-workflow/`. Nothing here is built yet.

## 1. The problem

The goal is one long-running chat that manages every space and agent on a wheeld: dispatch work, plan, act as the clearing house for child agents that need guidance, and show artifacts, plans and mirrors of any pane. You work on **one thread at a time**.

Earlier attempts broke that thread. Child agents reported back the moment they finished, the parent had to answer, and an unrelated topic jumped into the conversation.

The current implementation still does this, one message later. `daemon/src/inbox.rs` attaches every inbox event since your last message to your next message (`<wheel-inbox>`), and the brief tells the orchestrator to report them. Every turn on topic A still carries B, C and D.

The workflow is also compiled into wheeld: routing (space vs global, first-started wins), the Orchestrator session rules, the brief text in `orchestrator.rs`, `--confirmed`, the edit/shell refusal, and muting pushes per task. Every change needs a rebuild, which goes against ADR-0043.

## 2. Principles

1. **Nothing enters the orchestrator's chat unless you put it there.** Not events, not child reports, not inbox summaries. The orchestrator takes a turn only when you message it, or when it is running a wait you asked for.
2. **The workflow lives in files, not in wheeld.** Guidance (`CLAUDE.md`), Claude Code settings and hooks, and scripts in the orchestrator's folder. Changing the workflow means editing a file, with no build.
3. **wheeld gets only generic capabilities**, ones that pass the ADR-0043 test.
4. **One topic at a time.** You switch topics. The orchestrator never does.
5. **Use existing tools before building new ones:** beads for the queue, Claude Code hooks for context, `wheel agent wait` for waiting.

## 3. The model

Three things are kept apart:

| | What it is | Who sees it |
|---|---|---|
| **Topic** | The one subject of the chat right now | You and the orchestrator |
| **Queue** | Everything that wants your attention | You, in a pane or sheet. The orchestrator only when you say "next" |
| **World** | Spaces, agents and status | Read on demand: `wheel status --json` |

A queue you can glance at doesn't force a turn. Switching is something you do: say "next", say "back to X", or tap a queue item, which sends that message for you.

## 4. Under the hood

### 4.1 The orchestrator's folder (`~/.wheel/orchestrator`)

```
CLAUDE.md                 role, one-topic discipline, dispatch footer, never raise queue items unasked
.claude/settings.json     deny Edit/Write; Bash allows only `wheel *` and `tools/*`; hooks (4.3)
tools/on-event            deterministic reducer: inbox event -> queue item, watches, pushes
tools/ask "<q>"           child agent: create a needs-you item, then end its turn
tools/done "<summary>"    child agent: mark the work ready for review or done, link PR or bead
tools/switch <topic>      save, wait for the turn to end, swap session and mirrors
threads/current           id of the active topic
threads/<id>.md           topic notes
threads/archive/          closed topics
watches/<id>.json         one-shot "tell me when" rules
inbox/{tmp,new,cur}/      wheeld's raw event log (Maildir; already exists)
.beads/                   the queue: its own beads database, separate from project beads
```

`.claude/settings.json` replaces wheeld's edit/shell refusal. It's configuration, not code.

### 4.2 Topics

- **A topic is something you are talking about.** Dispatching work creates no topic: "implement wheel-abc" gets sent, the orchestrator replies "sent to wheel-3" and stays in the current topic. A topic starts only when you pick a queue item or say "let's talk about X". When a topic is about one bead, its id is the bead id.
- **One Claude Code session per topic, shown as one continuous chat.** Each topic keeps a clean context, and returning to one resumes its session in full rather than a summary. The client draws the sessions one after another in the order you visited them, older ones dimmed, with your replies always going to the session at the bottom. Sessions are never shown to you as a concept.
- **`threads/<id>.md` holds** the title, session id, agents, beads, mirrored panes, decisions and open questions. The orchestrator writes it at each switch, following `CLAUDE.md`.
- **Switching** (`tools/switch`) runs detached so the orchestrator's turn can end first:
  1. Wait for the turn to end: `wheel agent wait $WHEEL_PANE`.
  2. Write `threads/current`.
  3. `wheel agent resume <pane> <session>`, or `wheel agent new` for a new topic.
  4. Close the old topic's mirrors and open the new topic's.
  5. Send the opening message.
- **Lifecycle:** open → parked → closed.
  - **Resolved** is decided by `on-event`, not the model: every linked queue item is closed, every named agent is idle or stopped, and its PR is merged if it has one.
  - **Closing:** resolved topics are offered at the next "next", but a topic closes only on your word. Closed notes move to `threads/archive/` and the session stays resumable.
  - **Stale:** topics parked past a threshold are flagged. Nothing closes on its own.

### 4.3 Hooks (Claude Code, in the orchestrator folder)

| Hook | Does |
|---|---|
| `UserPromptSubmit` | Adds one line to each message: current topic, its notes path, "earlier topics are closed; don't raise them unless asked" |
| `PreCompact` | Makes sure the current topic's notes are written |
| `SessionStart` (matcher `compact`, and on resume) | Loads `threads/<current>.md` back into context |

### 4.4 The queue

1. **Events:** wheeld keeps writing its Maildir inbox, which already exists. Only the step that attaches events to your message is deleted.
2. **`tools/on-event`** runs for each new event file (wheeld already does this). It's deterministic, with no model:
   - permission request → needs-decision item; when the request is answered, the item closes
   - failure, a turn past its time limit, or the same failure twice → bug item at the top
   - turn ended after `tools/done` → ready-for-review (with a PR) or done
   - turn ended with neither `done` nor `ask` → "finished, not confirmed"
   - one open item per agent and kind; checks `watches/`
3. **Storage:** beads issues labeled `needs-you` in the orchestrator's own beads database. Each records agent, space, topic and priority, and can link a project bead.
4. **The dispatch footer** (guidance in `CLAUDE.md`, sent with every task): "Need a decision: run `$ORCH/tools/ask "<q>"` and stop. Done: run `$ORCH/tools/done "<summary>" [--pr N]`." Child agents never message the orchestrator.
5. **The model reads the queue** only through `bd list --label needs-you`, when you say "next". After passing your answer to the child with `wheel agent send`, it closes the item with `bd close`.

### 4.5 Notifications

| Event | Queue pane | Push | Chat |
|---|---|---|---|
| Item needs you (permission, decision) | ✓ | ✓, one per item | — |
| Ready for review | ✓ | ✓ | — |
| Plain completion | ✓ | digest only | — |
| Every dispatched agent idle | ✓ | ✓ "nothing running; N waiting" | — |
| N items needing you past a threshold | badge | one push | — |
| A watch you set | — | ✓ | only in wait mode (4.6) |

The digest is a script on a schedule: hourly, at the end of the day, or never. Which kinds push is a setting in `on-event`.

### 4.6 Waiting deterministically

- **You're waiting and doing nothing else:** say "I'm waiting on the wheel-2 build." The orchestrator runs `wheel agent wait wheel-2` as a background shell command. When that process exits, Claude Code tells the orchestrator, which reports the result. This is the one time it waits, and it's a rule in `CLAUDE.md`.
- **You walk away or switch topics:** the orchestrator writes `watches/<id>.json` (`{agent, on: [turn_end, fail], notify: push}`), and `on-event` fires the push and deletes the watch. No model is involved.
- **To watch a command rather than the agent**, the watch uses `wheel term read <pane> --until "<text>"`.

### 4.7 Proactivity

The orchestrator never switches topics on its own. All proactive behavior is in `on-event`, which raises the urgency of the signal (badge color, a push) instead of interrupting the chat. When several items wait, "next" offers a **triage pass**: one exchange per item, quick ones first. Any item that turns into a real discussion becomes its own topic, and the pass carries on afterwards.

An opt-in "triage time" nudge (current topic resolved and you've been idle for N minutes) is possible, but it's off by default because it brings back the interruption.

## 5. wheeld changes

**Keep (already exists):** the Maildir event log; running `tools/on-event`; `wheel agent send | wait | new | resume | conversations | card`; `wheel push send`; artifacts and plugins.

**Add (generic):**
- event hooks in `wheel.toml` (`[[hook]] on=[…] run=…`, JSON on stdin)
- a permission hook (a command returns allow, deny or ask)
- mirroring any pane into any space
- turning notifications off per pane
- drawing several sessions in one agent chat pane (client side)

**Delete:**
- attaching the inbox to your messages
- orchestrator scope and routing
- the Orchestrator session and space special rules
- the compiled orchestrator brief
- `--confirmed`
- the edit/shell refusal
- muting pushes per task
- the `orchestrator_tasks` ops

The orchestrator becomes a normal agent pane whose folder is `~/.wheel/orchestrator`.

## 6. How other tools do it

- **Claude Code agent teams:** a lead chat and teammates linked by file mailboxes. Teammates send an idle notification after every turn, and it arrives in the lead as a new turn. That's the interruption this design removes, and users have filed bugs about the ack loops it causes ([docs](https://code.claude.com/docs/en/agent-teams), [protocol write-up](https://dev.to/nwyin/reverse-engineering-claude-code-agent-teams-architecture-and-protocol-o49)).
- **Conductor, Superset, Warp:** no orchestrator. One workspace per agent, plus attention badges, chimes and notifications ([Conductor](https://www.conductor.build/), [Superset](https://docs.superset.sh/agent-status), [Warp](https://docs.warp.dev/agents/capabilities/agent-notifications/)).
- **Vibe Kanban:** a kanban board of tasks run by agents ([repo](https://github.com/BloopAI/vibe-kanban)).
- **LangChain Agent Inbox:** an inbox of durable interrupts, where an agent pauses until a human answers. It's the closest to the queue plus a parked child ([HITL docs](https://docs.langchain.com/oss/python/langchain/frontend/human-in-the-loop)).

None of these combine a single lead chat with a queue the model never sees unless asked.

## 7. Desktop (Mac)

Screens 1–6 come from the first pass. They show the flow, but they predate the mobile decisions in §8: one header row, the queue as a tray icon with a badge, a gear for settings, model and context in the composer, and session-per-topic. Restyle them to match before building.

### 1 · Working one topic
![](images/Main.png)
The chat is the topic. The right side shows only that topic's panes, mirrored from any space. When a child finishes elsewhere, only the queue count changes.

### 2 · Glance at the queue
![](images/02-glance.png)
You open the queue whenever you want, and the model never sees it. The queue is a plugin pane that reads the queue's beads.

### 3 · Close and switch
![](images/03-switch.png)
"Next" saves the topic and only then reads the queue. A topic is a divider plus a notes file, and the panes swap to the new topic's.

### 4 · Answer the child
![](images/04-answer.png)
The child parked itself on its question. You answer in the main chat, the orchestrator passes it on, and the item clears.

### 5 · Urgent while focused
![](images/05-urgent.png)
Urgency changes the indicator and sends a phone push. The chat still gets nothing.

### 6 · Return to an earlier topic
![](images/06-return.png)
Returning reads that topic's notes and the agents' current status. What happened while you were away arrives now, because you asked.

### 7 · Changed files
![](images/d1-changes.png)
A turn that edited files gets one card in the transcript: Edited N files, Undo for the turn, View changes, and one row per file with lines added and removed. It's the same card in any agent pane.

## 8. Mobile (iPhone)

### 8.1 Shared layout

- **One header row.**
  - Title: the mode menu ("Orchestrator ⌄" / "Spaces ⌄" / "Agents ⌄").
  - Under the title: the context line. A topic in Orchestrator mode opens its notes; a space · pane in Spaces mode switches space; a session in Agents mode switches session.
  - On the right: the panes icon (badge), the queue tray (badge; Orchestrator mode only) and the settings gear.
- **Badges** are the same everywhere: a top-right rounded pill with dark text. The queue badge turns gray, orange or red with urgency; other badges stay gray.
- **The composer** follows Claude's: a text row on top, then +, the model, the context ring, the mic and voice mode. It appears **only on agent panes**.
- **Keep it compact:** rows 44–48 pt, 15 pt body text, tap targets at least 44 pt.
- **Interaction rules:**
  - Tapping or swiping on the chat, a pane or an artifact hides the keyboard.
  - Swiping left or right on a pane moves between panes; it's the main way to navigate a space.
  - Skills and snippets open from the composer's + sheet; typing `/` filters skills inline.

### 8.2 Orchestrator mode

| | |
|---|---|
| ![](images/m1-topic.png) | **M1 · Topic chat.** The header names the current topic; tap it for the notes (M8). `wheel agent card` brings a child's pane or doc into the chat. The tray icon is the queue. |
| ![](images/m2-queue.png) | **M2 · Queue sheet.** Needs you, Ready for review, Finished. Built from the queue beads by `on-event`. |
| ![](images/m3-next.png) | **M3 · Next.** Saves the topic, offers resolved topics to close, lists what needs you, and offers a triage pass. |
| ![](images/m4-triage.png) | **M4 · Triage pass.** One exchange per item, quick ones first. |
| ![](images/m5-wait.png) | **M5 · Waiting on a build.** A background `wheel agent wait`; the process exiting brings the result back. |
| ![](images/m6-push.png) | **M6 · Pushes.** From `on-event`: items that need you, and an all-idle fleet, push; completions go to the digest. |
| ![](images/m7-earlier.png) | **M7 · Earlier topics.** One continuous chat across topics, older ones dimmed. Resume on a divider brings that topic back to the bottom. |
| ![](images/m8-notes.png) | **M8 · Topic notes.** The topic's notes, rendered read-only. |
| ![](images/m9-panes.png) | **M9 · Topic panes.** Swipe left from the chat, or tap the panes icon. The chat collapses to a sheet over the pane. |
| ![](images/m12-panes-chat.png) | **M12 · Panes, chat expanded.** The sheet slides over the pane, which keeps its full size underneath. |
| ![](images/m10-model.png) | **M10 · Model and context.** The model picker, and token use for this topic. |
| ![](images/m11-switch.png) | **M11 · Switch view.** The title menu only switches modes: Orchestrator, Spaces, Agents. |

### 8.3 Spaces mode

A space has no lead chat. You see whichever pane you're on.

| | |
|---|---|
| ![](images/s1-space.png) | **S1 · Space view.** The header line is space · pane. On an agent pane, the composer carries that agent's model and context. Page dots show where you are in the swipe. No queue in Spaces mode. |
| ![](images/s2-switch.png) | **S2 · Switch space.** Sessions as chips (space count, orange badge when an agent in that session needs you; hidden when there is only one session, with New session then in Settings). Search, spaces with agent status dots, New space. The badge comes from agents' `needs_you` status, not the orchestrator queue. |
| ![](images/s3-mode.png) | **S3 · Switch mode.** Modes only. |
| ![](images/s4-panes.png) | **S4 · Panes in a space.** A list to jump to a pane, and + to add one. Agents keep their tag and status; other panes show an icon and their name. |
| ![](images/s5-terminal.png) | **S5 · Terminal pane.** A non-agent pane fills the screen, with no composer. |

### 8.4 Agents mode

| | |
|---|---|
| ![](images/a1-agents.png) | **A1 · Agents.** Every agent in the session, grouped as Needs you / Working / Idle. Each row: a harness mark (Claude Code, Codex, pi) with a status dot, the pane name, the project, the last line; status, model and context in small print. |
| ![](images/a2-agent.png) | **A2 · Agent open.** The agent's chat, with its permission prompt inline and the composer. |

### 8.5 Questions

The same card in every agent pane.

| | |
|---|---|
| ![](images/q1-question.png) | **Q1 · Question.** Replaces the composer. Single select, Other with dictation, Skip/Submit, "1 of N" for batches. |
| ![](images/q2-multi.png) | **Q2 · Multi-select, preview.** Checkboxes, plus a preview for the focused option. |
| ![](images/q3-hidden.png) | **Q3 · Question hidden.** "Asking a question" in the chat; a waiting bar with Show question and Stop. |
| ![](images/q4-answered.png) | **Q4 · Answered.** One card per batch in the transcript. |

### 8.6 Composer: + sheet, skills, changed files

| | |
|---|---|
| ![](images/k1-skills.png) | **K1 · Plus sheet.** Photos, Camera, Add files; Permission; Skills ›, Snippets ›. |
| ![](images/k3-plus.png) | **K3 · Skills view.** The agent's skills from its harness, with info buttons and search. |
| ![](images/k2-skill-chip.png) | **K2 · Skill inline.** A picked skill is an inline block in the text, deleted as one unit. |
| ![](images/f1-changes.png) | **F1 · Changed files.** A pill above the composer: files, lines added and removed for the turn. |
| ![](images/f2-changes-sheet.png) | **F2 · Changed files sheet.** Per-file counts, Undo for the turn, and tap a file for its diff. |

### 8.7 Snippets

| | |
|---|---|
| ![](images/n1-snippets.png) | **N1 · Snippets view.** From the + sheet. Tap to insert; Manage opens the manager. |
| ![](images/n2-snippet-in.png) | **N2 · Snippet inserted.** Plain editable text, unlike a skill. |
| ![](images/n5-manager.png) | **N4 · Snippets manager.** Search, drag to reorder (the picker follows this order), +, and a tag for each snippet's scope (all projects or one). |
| ![](images/n6-edit.png) | **N5 · Edit snippet.** Name, text, where it's available, delete. |

### 8.8 Settings and devices

The gear opens Settings directly. Snippets, notifications and devices all live here. The phone connects to one Wheel at a time, so devices are only in Settings.

| | |
|---|---|
| ![](images/n4-settings.png) | **N3 · Settings.** Connection (Devices), Composer (Snippets, Skills, Dictation), Notifications (Pushes, Digest), Orchestrator (Model, Approvals). |
| ![](images/v2-devices-manage.png) | **V1 · Devices.** Paired Macs with status and how each is reached; the connected one is checked. Tap another to switch to it; › to rename, make default or forget; Pair a device with the Mac's one-time code. |

### 8.9 Voice mode

Voice is a full-screen mode that any agent view can switch to; the orchestrator is the main user. There is no in-chat voice bar. Decisions already made: transcription on the device on both phone and Mac (text goes over the wire, not audio), but not Apple's built-in speech recognition; one voice client active at a time; a system voice for replies, no voice cloning; on iOS it keeps running in the background through an active audio session.

**How a conversation flows**
- **It's the same chat.** Everything you say lands as your message and every reply is written to the chat, so voice and text can be mixed freely and the topic model (§4.2) applies unchanged. "Next" and "back to X" work by voice.
- **Turn-taking:** detection on the device notices when you stop speaking and sends what you said. Hold-to-talk is an option for noisy places.
- **Interrupting:** tap, or just start talking, while it speaks. It stops at once and listens.
- **Approvals are inferred from what you say.** Reversible ones are approved and reported in one line. Anything that can't be undone gets a card, a spoken question, and waits for a clear yes or no.
- **Showing things:** when the orchestrator mentions a diff, pane or doc, it puts it on screen with `wheel agent card`.

| | |
|---|---|
| ![](images/vo2-full.png) | **VO1 · Voice, listening.** The voice button in any agent's composer opens full-screen voice: an orb with its state and the topic. The panes icon, or swiping left, shows panes without stopping voice. Type instead, mute, end. |
| ![](images/vo3-speak.png) | **VO2 · Speaking.** The spoken reply with a live caption; tap or talk to interrupt. |
| ![](images/vo4-approve.png) | **VO3 · Approval by voice.** A one-line report for a reversible approval; a card plus a spoken question for an irreversible one. |
| ![](images/vo5-show.png) | **VO4 · Showing a pane.** What it refers to appears above the orb; tap it to open full screen. |
| ![](images/vo6-lock.png) | **VO5 · Locked.** Keeps listening with the screen locked; a Live Activity shows the state and topic, with mute and end. |
| ![](images/vo7-panes.png) | **VO6 · Panes during voice.** Panes come back full screen and voice shrinks to a floating pill (orb, caption, mute, end). It keeps listening and speaking; tap the orb to return to full screen. |
| ![](images/vo8-agent.png) | **VO7 · Voice on any agent.** Any agent view can switch to this: here a Spaces agent pane (W11482 in llama-broker). Same layout without the queue; the header names the agent and its space. |

## 9. Open items

1. Restyle desktop screens 1–6 to the mobile decisions in §8.
2. An ADR replacing ADR-0054, covering §4 and §5.
3. Set up `~/.wheel/orchestrator`: `CLAUDE.md`, `.claude/settings.json` hooks, `tools/on-event | ask | done | switch`, and the queue's beads database.
4. Client: draw several sessions in one chat pane (M7); swipe between panes; hide the keyboard on tap or swipe.
5. Classification: rules in `on-event` first; Jev only for what the rules can't decide.

Design canvas: https://claude.ai/artifact/XfjmQGaLfjHkYHJ2338FcF
