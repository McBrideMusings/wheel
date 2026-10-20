# ADR-0006: A conversation listing is unfiltered

The agent keeps its conversations in one store per directory, shared by every program that runs the same harness there: `claude-agent-acp` and Claude Code read and write the same store, so a terminal Claude Code session in an agent pane's directory is in it.

`wheel agent sessions` lists everything the agent's `session/list` returns for the pane's directory, including conversations wheel never started. wheel applies no filter. The CLI prints a notice on stderr saying so, which leaves stdout as the listing.

wheeld stores each agent pane's conversation id in the pane, in its saved state, and loads that id on start. It never infers a pane's conversation from the list.
