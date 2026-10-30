# Wheel — design

## Overview

Wheel is a native macOS and iOS app for running coding agents across projects. Each project is a space with its own grid of terminals, agent chats and web views.

## Direction

Until the MVP works end to end, the look is stock macOS: system materials, controls, type and SF Symbols, with no custom palette or avatar art. The two animations are the chat's progress shimmer on work still running and the space tabs leaving or rejoining the title row as the sidebar opens or closes. Custom visual work — visual passes over the whole app — happens later as spikes.

## Layout

- The grid fills the window; space tabs sit in the window's title row, beside the traffic lights and the sidebar toggle, while the sidebar is closed; the + that makes a space sits at the title row's right end in both sidebar states.
- Every agent pane has the same composer: a rounded box holding the text, attachments, `/` skills and send.
- While an agent is working and the draft is empty, the composer's send button is a Stop square beside an "esc" hint, and Esc in the composer stops the turn; typing turns the button back into send, which queues the message. A pane's header has no Stop.
- The controls the agent offers over its session — model, effort, permission mode — sit as a row of plain values under the composer, each opening a menu.
- A new pane starts empty and asks what it shows; any pane can change kind from its title.

## Chat

Every agent pane draws its conversation the same way, after assistant-ui's thread.

- **Type.** Three sizes, two weights, one family. Messages, replies and the composer are body (13 pt); tool, reasoning and request lines are callout (12 pt); times, counts and settings are caption (10 pt). Semibold marks a name or a heading, never a whole line. Monospace is for code only: inline code, a command, a diff. Markdown headings stay within 1.25× body.
- **Turns.** The person's message sits on the right in an accent bubble (`chat.message.style`: `bubble` is grey, `accent` is the accent colour, `plain` is bare text under a "You" label; `chat.message.align` puts it left; `chat.timestamps` adds the send time under it). The reply under it has no bubble. A finished turn reads: the message, then the agent's messages in order with a "Worked for 38s ›" line (collapsed) for each stretch of work between them, and an "Edited 2 files" card when the turn changed files. 24 pt separates turns, 12 pt a message from its reply, 8 pt the parts of one reply.
- **Tool calls.** `chat.tools.summary` sets the grain. `turn` (the default) draws one "Worked for 38s" line per stretch of tool calls and thoughts between two of the agent's messages (before the first and after the last count too); it opens onto that stretch's calls and thoughts. Every message the agent writes is drawn in the main chat, never inside a stretch. `runs` draws one line per run of consecutive calls, worded from their kinds ("Ran 1 command, read 2 files"). `each` draws one quiet line per call: a status icon, the title, how long it ran, a chevron that opens the call's text and diffs. Every line opens onto the lines inside it.
- **Work in progress.** A running tool, run, turn or thought shows a spinner, a highlight sweeping across its label, and a timer that counts each second; the turn's line reads "Working · <current call> 6s". The daemon stamps every event, so the times survive a relaunch. Reduce Motion stops the sweep.
- **Reasoning.** `chat.thinking`: `collapsed` (the default) shows "Thought for 4s" and opens onto the text; `live` also shows the latest lines under "Thinking" while it streams; `hidden` draws no reasoning row.
- **Edits card.** `chat.edits.card` (default on): a finished turn whose tool calls changed files ends with a card, "Edited N files", the total lines added (green) and removed (red), a row per file with its path and counts, and "View changes", which opens the combined diff (changed lines only) in the card; it then reads "Hide changes". A turn with no edits has no card.
- **Requests.** One at a time. The agent's oldest waiting request is a card directly above its pane's composer — the agent's own reason in plain text (with why it had to ask and the path involved, when it says), then what kind of action and the command or diff — with its choices in one group at the right: Deny, Always allow, Allow (filled). Return allows, ⌘↩ always allows, Esc denies, each tooltip naming its key, and Always allow's tooltip naming the rule it adds. "1 of N" when more wait across Wheel. The tool line it gates says it needs your input.
- **Copy.** The last reply of a finished turn has a copy button under it; earlier replies show one on hover.
- **Empty.** An empty conversation says what to ask, centred in the space the transcript has.
- **Scrolling.** The transcript follows new output until the person scrolls away from the end, and again once they scroll back.

## Artifacts

Every artifact page draws with Wheel's design system, `wheel.css`, which wheeld serves to each web pane at `.wheel/wheel.css` (source: `daemon/src/wheel.css`). A page opts in with `<link rel="stylesheet" href=".wheel/wheel.css">` and defines no colours of its own; every built-in artifact does, and a person's own artifact takes the same line.

- **Colour.** The macOS system colours (red, orange, yellow, green, teal, blue, indigo, purple, pink, gray), tuned for text in light and the system's own values in dark, plus label, secondary and tertiary text, separators, and surfaces: the window (`--bg`), a grouped box on it (`--group`), a fill (`--fill`) and a log's well (`--log-bg`). Light and dark follow the system; `<html data-theme="light|dark">` forces one.
- **Type.** The system font at body (13 px), callout (12 px), caption (11 px) and title (20 px); monospace only for code, commands and logs.
- **Groups.** Content sits in rounded boxes (`.group`) under a heading line (`.heading` with its count and a hint at the right), never one long table. A box's rows (`.group .row`) are one line at one height, cut with an ellipsis (`.one`); hover carries the full text.
- **Counts strip.** `.counts`: one cell per count, the number in its colour, a zero greyed; four to a line under 700 px.
- **Highlighted log.** `.log`: a tinted monospace well, one line per entry at one height, newest first. Commands are orange, reading teal, editing blue, agents and skills purple, speech green and italic, thinking faint; inside a command, paths are blue, quoted strings green and flags purple; a running call is orange, an error red. `.log.fade` fades the lines past the sixth, to 30%.
- **No chips.** A state is a coloured word or glyph, never a pill or a badge.
- **Controls.** Push buttons, the accent default button (`button.primary`), segmented controls (`.seg`) and text fields in the macOS shape.
