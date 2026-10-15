# ADR-0006: A conversation listing is unfiltered

The agent keeps its conversations in one store per directory, shared by every program that runs the same harness there: `claude-agent-acp` and Claude Code read and write the same store, so a terminal Claude Code session in the lead's directory is in it.

`wheel lead sessions`, and every client's conversation menu, lists everything the agent's `session/list` returns for the lead's directory, including conversations wheel never started. wheel applies no filter. The CLI prints a notice on stderr saying so, which leaves stdout as the listing.

wheeld stores the lead's own conversation id in its saved state and loads that id on start. It never infers the lead's conversation from the list.
