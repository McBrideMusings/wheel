# Wheel

A native macOS and iOS agent development environment: one lead agent beside a grid of panes, one space per project, all state held by a Rust daemon.

## Language

### Domain

**Lead**:
The one agent the person talks to across all of Wheel. It directs supervisors, triages its inbox, and brings the person only decisions and finished work.
_Avoid_: chief of staff, orchestrator, mayor

**Supervisor**:
An agent in a pane that pursues one goal or backlog focus in one space. Its workers run in the background, and it handles their escalations and the landings code couldn't finish. A space can hold several.
_Avoid_: space lead, swarm lead, swarm, foreman

**Worker**:
A background agent session that does one bead in its own worktree, under a supervisor. It can be shown in a pane without changing what it is.
_Avoid_: polecat, task agent, subagent

**Inbox**:
The items waiting for an agent's attention: a worker's turn ending, a permission request or a failure for a supervisor, and escalations for the lead. Kept on disk with a state per item.
_Avoid_: queue, notifications
