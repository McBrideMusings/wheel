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
- **Requests.** One at a time. The agent's oldest waiting request is a card directly above its pane's composer — what kind of action, the command or diff — with the agent's choices, refusals on the left and the one-time allow filled on the right (⌘↩ in the focused pane), "1 of N" when more wait across Wheel. The tool line it gates says it needs your input.
- **Copy.** The last reply of a finished turn has a copy button under it; earlier replies show one on hover.
- **Empty.** An empty conversation says what to ask, centred in the space the transcript has.
- **Scrolling.** The transcript follows new output until the person scrolls away from the end, and again once they scroll back.
