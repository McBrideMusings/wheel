---
applies-to: ["daemon/src/brief.md", "daemon/src/brief.rs"]
---

# ADR-0017: The agent brief states how Wheel works

`wheeld` gives every agent it starts the brief in `daemon/src/brief.md`: what spaces and panes are, the `WHEEL_*` variables, what each `wheel` command does and prints, and what words for where a pane sits name in a grid of panes, followed by the artifacts that exist in the agent's space when it starts (ADR-0018). The brief holds no instruction about how an agent should behave — when to make an artifact, what to say, what to ask or leave alone. Those belong to the person and live in their own agent instructions, which Wheel neither ships nor reads.
