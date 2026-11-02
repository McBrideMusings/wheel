# ADR-0023: An agent pane owns an agent

The coding agent an agent pane runs is an *agent*: it has an id like `w3179`, a conversation, a state (starting, ready, busy, failed, switching, gave up) and a login folder, and it outlives restarts of its OS process, which wheeld starts again under the same id. *Session* names only a group of spaces (ADR-0021). The protocol, the `wheel` command, WheelKit and both apps use *agent* for every operation, event, type, flag and label about it, with no second name.
