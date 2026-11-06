---
applies-to: ["daemon/src/brief.md", "daemon/src/brief.rs", "daemon/src/guidance.rs"]
---

# ADR-0017: The agent brief states how Wheel works

`wheeld` gives every agent it starts the brief in `daemon/src/brief.md`: what spaces and panes are, the `WHEEL_*` variables, what each `wheel` command does and prints, and what words for where a pane sits name in a grid of panes, followed by the artifacts that exist in the agent's space when it starts (ADR-0018). The brief holds no instruction about how an agent should behave — when to make an artifact, what to say, what to ask or leave alone. Those belong to the person.

The person's own instructions reach an agent in two ways. Their agent instructions files (`CLAUDE.md` and the like) Wheel neither ships nor reads. Their guidance is the other: `guidance.md` for every agent and `guidance/<agent>.md` for one, at fixed paths beside this machine's `wheel.toml`. When a pane starts, `wheeld` appends them after the brief and its artifacts section, the text for every agent first and then that agent's, each under a line saying whose it is, character for character as the person wrote it. Wheel parses none of it, checks none of it and enforces none of it; it ships none (both files start missing or empty, and a missing or empty file adds nothing); and a running agent keeps the guidance it started with. Settings › Guidance shows the brief read-only beside the files, and `wheel guidance show` prints what an agent would be given.
