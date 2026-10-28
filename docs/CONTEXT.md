# Wheel

A native macOS and iOS agent development environment: a grid of panes (terminals, agent chats, web views) per project, one space per project, all state held by a Rust daemon.

## Language

### Domain

**Space**:
One project: a directory and a split tree of panes. A window shows one space at a time, and a space is in at most one window.
_Avoid_: tab, workspace

**Window**:
A Mac window holding an ordered list of spaces, one shown at a time, with an optional name. The Mac app stores windows; the daemon knows none.

**Background space**:
A space in no window. It keeps running and is listed in the menu bar item.

**Pairing code**:
The JSON a phone scans or pastes to connect: the daemon's address, a one-time token and the certificate fingerprint.

**Agent pane**:
A pane that owns one coding agent's session and shows its conversation as a chat. Every agent in Wheel is one.
_Avoid_: lead, agent card

**Artifact**:
A page a web pane can show, defined by one `[artifacts.<name>]` entry in a `wheel.toml`.
_Avoid_: manifest, artifact.toml, plugin
_applies-to_: daemon/src/artifacts.rs

**Scope**:
Which `wheel.toml` holds an artifact's entry: `project` (the space's) or `machine` (this machine's, `~/.wheel/wheel.toml`).
_Avoid_: repo, local, shared

**Refresh**:
An artifact's optional command, which the daemon runs in the space's directory to write the pane's data. It runs without a permission request.
_applies-to_: daemon/src/refresh.rs

**Data directory**:
The directory a web pane serves under `data/`: the pane's own `<instance>/panes/<pane>/`, or a post's folder.

**Post**:
An artifact whose folder holds only data, drawn by the page of the artifact its entry names as `renderer`.

**Renderer**:
An artifact whose page draws a post's data.

**Built-in**:
An artifact embedded in wheeld and installed into the machine scope from Settings › Artifacts.
_Avoid_: bundled artifact, core view

## Relationships

- A **Space** has one or more panes; an **Agent pane** owns exactly one session
- An **Artifact** has exactly one **Scope**; a web pane shows one **Artifact** and owns one **Data directory**
- A **Post** names exactly one **Renderer**; a **Renderer** draws many **Posts**
