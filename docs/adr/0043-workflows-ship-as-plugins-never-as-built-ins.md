# ADR-0043: Workflows ship as plugins, never as built-ins

Wheel's own codebase (`daemon/`, `cli/`, `clients/`, `apple/`, `artifacts/`) holds only the base systems every project needs whatever its workflow: terminals, web views and browsers, agent chats, spaces and panes, the artifact store with its generic renderers (markdown), and the plugin runtime of ADR-0041. Anything shaped for one way of working, one kind of project or one development environment — a decision page, a mockup viewer, a review flow, a feed of an agent's turns, a backlog board — is a plugin: a `wheel-plugin.toml` package of Luau panes and an optional backend, installed per machine or per project.

The test for a new built-in is whether a person who never uses agents for software, never uses beads and never runs this repo's workflow would still need it. A feature that fails the test is designed as a plugin from its first slice, and any piece the plugin needs from the base is added as a generic capability (a feed, an action, a component, a CLI op) that other plugins can use too, never as a branch for that one plugin.

A plugin in this repo's `plugins/` directory is still a plugin: it ships beside Wheel as an example or default and may be removed or replaced without touching the base. The built-in artifacts in `artifacts/` that fail the test move to plugins.
