# Wheel — design

## Overview

Wheel is a native macOS and iOS app for running coding agents across projects. Each project is a space with its own grid of terminals, agent chats and web views.

## Direction

Until the MVP works end to end, the look is stock macOS: system materials, controls, type and SF Symbols, with no custom palette or avatar art. The one animation is the chat's progress shimmer on work still running. Custom visual work — visual passes over the whole app — happens later as spikes.

## Layout

- The grid fills the window; space tabs sit above it, aligned to its left edge.
- Every agent pane has the same composer: a rounded box holding the text, attachments, `/` skills and send.
- While an agent is working, a Stop button sits in its pane's header.
- The controls the agent offers over its session — model, effort, permission mode — sit as a row of plain values under the composer, each opening a menu.
- A new pane starts empty and asks what it shows; any pane can change kind from its title.

## Chat

Every agent pane draws its conversation the same way, after assistant-ui's thread.

- **Type.** Three sizes, two weights, one family. Messages, replies and the composer are body (13 pt); tool, reasoning and request lines are callout (12 pt); times, counts and settings are caption (10 pt). Semibold marks a name or a heading, never a whole line. Monospace is for code only: inline code, a command, a diff. Markdown headings stay within 1.25× body.
- **Turns.** The person's message is a right-aligned grey bubble. The reply under it has no bubble: its text, reasoning and tool calls in the order they arrived. 24 pt separates turns, 12 pt a message from its reply, 8 pt the parts of one reply.
- **Tool calls.** One quiet line each: a status icon, the title, how long it ran, a chevron that opens the call's text and diffs. Two or more calls in a row collapse into one line, "Used 3 tools", that opens onto each.
- **Work in progress.** A running tool, group or thought shows a spinner, a highlight sweeping across its label, and a timer that counts each second. The daemon stamps every event, so the times survive a relaunch. Reduce Motion stops the sweep.
- **Reasoning.** "Thinking" with the latest lines showing while it streams; "Thought for 4s", collapsed, once it ends.
- **Requests.** One at a time. The agent's oldest waiting request is a card directly above its pane's composer — what kind of action, the command or diff — with the agent's choices, refusals on the left and the one-time allow filled on the right (⌘↩ in the focused pane), "1 of N" when more wait across Wheel. The tool line it gates says it needs your input.
- **Copy.** The last reply of a finished turn has a copy button under it; earlier replies show one on hover.
- **Empty.** An empty conversation says what to ask, centred in the space the transcript has.
- **Scrolling.** The transcript follows new output until the person scrolls away from the end, and again once they scroll back.
