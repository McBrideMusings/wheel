# ADR-0008: An inbox holds what waits on an agent

The lead and each supervisor have an inbox, kept on disk in the instance directory. Each item carries its source, kind, time and state: new, seen, surfaced or closed. A supervisor's inbox takes its workers' escalations. The lead's takes supervisors' forwards and every agent pane's turn ends, permission requests and failures. Nothing starts a lead turn: the daemon adds a count of new items to each message the person sends, and the lead raises them once the current thread ends. An agent reads another's conversation through `read_agent`, by its last turns, by search or whole, starting with the last turn.

None of this runs unless wheeld starts with `--orchestration`; off, its requests and MCP tools are refused and no agent is told of it.
