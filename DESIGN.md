# Wheel — design

## Overview

Wheel is a native macOS and iOS app for running coding agents across projects. One lead agent sits in a floating card beside a grid of panes; each project is a space with its own grid of terminals, agent chats and web views.

## Direction

Until the MVP works end to end, the look is stock macOS: system materials, controls, type and SF Symbols, with no custom palette, avatar art or animation. Custom visual work — a 3D lead avatar that shows app state, and visual passes over the whole app — happens later as spikes.

## Layout

- The lead is a floating card on the left with a gap on every side; the grid is a separate surface beside it.
- Space tabs sit above the grid only, aligned to its left edge.
- Lead and agent panes share one composer: attachments, `/` skills, send.
- Permission requests collect in a "Waiting on you" list pinned above the lead's conversation, each with Deny and Approve.
- While the lead is working, a Stop button sits at the right of its header.
- In the transcript, the person's messages are right-aligned tinted bubbles, replies are plain left-aligned text, and tool calls are small monospace rows with a status.
- A new pane starts empty and asks what it shows; any pane can change kind from its title.
