# Wheel

A native macOS and iOS agent development environment: a grid of panes (terminals, agent chats, web views) per project, one space per project, all state held by a Rust daemon.

## Language

### Domain

**Space**:
One project: a directory and a split tree of panes. The window shows one space at a time.
_Avoid_: tab, workspace

**Agent pane**:
A pane that owns one coding agent's session and shows its conversation as a chat. Every agent in Wheel is one.
_Avoid_: lead, agent card
