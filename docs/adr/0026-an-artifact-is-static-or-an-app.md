# ADR-0026: An artifact is static or an app

ADR-0041 supersedes the app half of this decision: the `app`, `mcp` and `mcp_url` keys and everything said about them. The static artifact stands.

An artifact is either static or an app. This adds to ADR-0003 and ADR-0004; their rules for a static artifact stand.

A **static artifact** is HTML or Markdown on disk with no process of its own. Its entry has a `path` (or, for a browser pane, a `url`), and may carry the refresh command, interval, `renderer` and `wheel.run` behaviour that ADR-0004 and ADR-0016 describe. None of that changes.

An **app artifact** is an entry that names a process with one of three keys: `app` (a command that serves HTTP; the daemon starts it when a web pane shows the entry, and the program announces its port on descriptor 3), `mcp` (a command that speaks MCP over its standard input and output), or `mcp_url` (a streamable-HTTP MCP server). An entry names one process: `app` and `mcp`/`mcp_url` exclude each other, and an app entry has no `path` and no `renderer`.

A web pane may show a live process by address. For an `app` entry the daemon proxies the app's announced port into the pane, so the pane is the host and the page is the program's own. A pane showing an app does not serve a folder, so ADR-0003's root and dot-file rules apply to static artifacts only.

Plugin-style hosts are not a kind of artifact.

A machine-scope entry may name its own `app`, `mcp` or `mcp_url`. ADR-0025 forbids a machine artifact that ships with Wheel (a built-in) from carrying commands, because Wheel must not guess what a project needs to run. An entry the user wrote into their own `~/.wheel/wheel.toml` is the user's command, run in the space's directory (ADR-0019), and it must not depend on the contents of any one project. A built-in names no `app`, `mcp` or `mcp_url`.
