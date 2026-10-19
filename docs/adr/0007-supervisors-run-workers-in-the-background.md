# ADR-0007: Supervisors run workers in the background

A supervisor is a pane that pursues one goal or backlog focus in one space. The daemon runs its loop in code. It claims ready beads in the order the supervisor's agent set, starts each worker as a background ACP session in its own worktree, and reads the worker's bead when a turn ends:

- closed: the daemon merges the branch onto main and runs the project's check, and a conflict or a red check goes back to the worker
- blocked: the item goes to the supervisor's inbox
- still open: the worker is told to continue, up to three times

A second landing failure, or a worker still open after three nudges, goes to the supervisor's inbox. A worker silent past its stall timeout is restarted. The loop holds at the supervisor's usage cap: running workers finish, and nothing new starts.

The supervisor's agent takes a turn only when its loop starts, when its inbox gets an item, and when its scope runs dry. It answers what a bead, an ADR or the person's earlier decision settles, and forwards the rest to the lead. Supervisors and workers start from Wheel's own config, never the person's.

A worker's permission request is answered by its supervisor's policy, which covers edits in its worktree, the project's declared commands, `bd` on its own bead and `git` in its worktree. A project declares its commands under `[policy] commands` in the `wheel.toml` at the root of its space's directory, which wheeld reads there and never from a worker's worktree; a command is approved when it starts with a declared one. Each approval is an `approved_by_policy` event. Anything else waits for the person: the request stays pending, the loop records `pending_for_person`, and the lead's inbox gets an item. Only the policy and a person's client answer a worker's request; no agent has a call that does. A worker shows only inside its supervisor's pane until the person opens it there or pushes it to a pane of its own, which the supervisor manages (ADR-0010). Closing that pane leaves the worker running, and the daemon closes it when the worker's bead lands.
