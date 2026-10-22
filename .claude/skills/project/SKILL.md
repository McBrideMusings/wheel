---
name: project
description: "wheel: how to verify it works (wheeld over its socket, the wheel CLI, the Mac app through its debug server), where its logs live and how to fetch them, and how swarm workers build it beside each other. Read before declaring a change verified, before diagnosing anything that happened while it ran, and before the first build in a swarm worktree."
---

# wheel

- **Verify** — read [verify.md](verify.md) before reporting any change works.
- **Logs** — read [logs.md](logs.md) when something happened while it ran. It fetches the lines itself; never ask the user to paste them.
- **Swarm** — read [swarm.md](swarm.md) before the first build or test in a swarm worktree.

Scripts the sections call live in [tools/](tools/).
