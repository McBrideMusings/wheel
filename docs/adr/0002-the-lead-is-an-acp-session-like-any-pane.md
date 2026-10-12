# ADR-0002: The lead is an ACP session like any pane

The lead is an ordinary ACP session that the daemon starts with an orchestrator system context and MCP tools for spaces, panes and sessions; it changes the app only through those tools. The lead and every agent pane use the same transcript and composer. A permission request from any session goes to every attached client, and the first answer wins.
