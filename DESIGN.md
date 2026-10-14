# Wheel — design

## Overview

Wheel is a native macOS and iOS app for running coding agents across projects. One lead agent sits in a floating card beside a grid of panes; each project is a space with its own grid of terminals, agent chats and web views.

## Direction

Until the MVP works end to end, the look is stock macOS: system materials, controls, type and SF Symbols, with no custom palette, avatar art or animation. Custom visual work — a 3D lead avatar that shows app state, and visual passes over the whole app — happens later as spikes.

## Layout

- The lead is a floating card on the left with a gap on every side; the grid is a separate surface beside it.
- Space tabs sit above the grid only, aligned to its left edge.
- Lead and agent panes share one composer: attachments, `/` skills, send.
- A permission request on a visible lead tool-call row is drawn on that row. Every other request (an agent pane's, a `type_into_pane`, a lead request whose row is scrolled out) collects in a "Waiting on you" list directly above the lead composer, each with Deny and Approve.
- While the lead is working, a Stop button sits at the right of its header.
- In the transcript, the person's messages are right-aligned tinted bubbles, replies are the agent's Markdown rendered — headings, lists, tables, block quotes and highlighted code fences — and tool calls are small monospace rows with a status.
- The controls the agent offers over its session — model, effort, permission mode — sit as a row of plain values under the composer, each opening a menu.
- A new pane starts empty and asks what it shows; any pane can change kind from its title.
