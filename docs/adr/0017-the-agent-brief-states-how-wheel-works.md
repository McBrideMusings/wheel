---
applies-to: ["daemon/src/brief.md"]
---

# ADR-0017: The agent brief states how Wheel works

`wheeld` gives every agent it starts the brief in `daemon/src/brief.md`: what spaces and panes are, the `WHEEL_*` variables, what each `wheel` command does and prints, and what words for where a pane sits name in a grid of panes. The brief holds no instruction about how an agent should behave — when to make an artifact, what to say, what to ask or leave alone. Those belong to the person and live in their own agent instructions, which Wheel neither ships nor reads.
