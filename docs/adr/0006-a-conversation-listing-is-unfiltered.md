# ADR-0006: A conversation listing is unfiltered

Each agent program keeps its conversations in its own store (Claude Code: `<config dir>/projects/<directory>/`, Codex: `<CODEX_HOME>/sessions/`), and an agent pane and the same program in a terminal read and write that one store, so a terminal session in an agent pane's directory is in it.

`wheel agent sessions` lists every conversation wheeld finds in the pane's agent's store for the pane's directory, including conversations wheel never started. wheel applies no filter. The CLI prints a notice on stderr saying so, which leaves stdout as the listing.

wheeld stores each agent pane's conversation id in the pane, in its saved state, and loads that id on start. It never infers a pane's conversation from the list.
