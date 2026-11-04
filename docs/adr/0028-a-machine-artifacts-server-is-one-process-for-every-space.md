# ADR-0028: A server that declares it is shared is one process for every space

An entry that names an `mcp` or `mcp_url` server (ADR-0026) and says `shared = true` gets one session for the machine, not one per space. Only a machine artifact can be shared. Any other server, shared or not declared, stays one process per space, run in the space's directory: Wheel never assumes a server can keep many projects apart.

wheeld names the asking pane and its project in `_meta` on every `tools/call` and `resources/read`, under `wheel/pane` and `wheel/dir`, and reads a resource as `<uri>?dir=<project>`. It subscribes to that URI (`resources/subscribe`) while a pane shows the project and unsubscribes (`resources/unsubscribe`) when none does. The server keeps its state per pane and per project, watches only the projects subscribed to, and when one changes sends `notifications/resources/updated` for the URI; wheeld sends `mcp_updated` to each pane that shows that project. wheeld uses no MCP roots, which the protocol deprecates.

A shared server starts in the home directory with no `WHEEL_SPACE_DIR`, since it belongs to no project (ADR-0019). If the process dies, wheeld drops the session and the next request starts a new one.

The server is a program that works with no host. Run with no `_meta`, it takes its project from its working directory, and it offers the same pages as commands. Wheel renders its pages and holds its process; the program does not depend on Wheel. Where the program is installed is the entry's to name (ADR-0025); a machine artifact that Wheel ships names none (ADR-0013).
