# Wheel

A native macOS and iOS agent development environment: a grid of panes (terminals, agent chats, web views) per project, one space per project, all state held by a Rust daemon.

## Language

### Domain

**Session**:
A named, ordered group of spaces, held by the daemon. A Mac window or a phone shows one at a time.

**Space**:
One project: a directory and a split tree of panes. Every space is in exactly one session.
_Avoid_: tab, workspace

**Window**:
A Mac window showing one session at a time, picked from its dropdown, with one of that session's spaces shown. The Mac app stores windows; the daemon knows none.

**Background session**:
A session no Mac window shows. Its spaces keep running and it is listed in the menu bar item.

**Client view**:
What one attached client shows: for each window, a session, its shown space and that space's shown pane. Each client owns its own; the daemon reports it but never sends it to another client.

**Pairing code**:
The JSON a phone scans or pastes to connect: the daemon's address, a one-time token and the certificate fingerprint.

**Agent pane**:
A pane that owns one agent and shows its conversation as a chat. Every agent in Wheel is one.
_Avoid_: lead, agent card

**Agent**:
One coding agent's run in an agent pane, with an id like `w3179`. It survives restarts of its OS process.
_Avoid_: session

**Artifact**:
A page a pane can show, defined by one `[artifacts.<name>]` entry in a `wheel.toml`.
_Avoid_: manifest, artifact.toml, plugin
_applies-to_: daemon/src/artifacts.rs

**Static artifact**:
An artifact whose page is HTML or Markdown on disk and which runs no process of its own.
_Avoid_: plain artifact

**App artifact**:
An artifact whose entry names a process (`app`, `mcp` or `mcp_url`) that the daemon runs for the pane showing it.
_applies-to_: daemon/src/app.rs

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

- A **Session** has one or more spaces; a **Space** has one or more panes; an **Agent pane** owns exactly one **Agent**
- An **Artifact** has exactly one **Scope**; a web pane shows one **Artifact** and owns one **Data directory**
- A **Post** names exactly one **Renderer**; a **Renderer** draws many **Posts**
