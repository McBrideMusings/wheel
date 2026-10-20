# ADR-0002: Every agent is a pane

An agent is an ACP session that an agent pane owns; Wheel runs no agent outside a pane. Every agent pane's session starts the same way: the person's Claude configuration (or the daemon's `--agent-config-dir`), every built-in tool, and Wheel's brief appended to its system prompt. The brief, built into wheeld, differs between panes only in the line naming the pane and its session. A permission request from any session goes to every attached client, the first answer wins, and no agent answers one.
