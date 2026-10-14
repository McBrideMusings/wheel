# ADR-0005: A browser pane is not an artifact pane

A pane that shows a web page is one of two kinds. A `web` pane names an artifact by scope and name, has no browser chrome, and is served from the daemon's own roots behind its token, as ADR-0003 sets out. A `browser` pane holds a page and draws an address field around it. Nothing about an artifact applies to it — no folder, no data directory, no token, no cookie — so a page from the open internet never reaches the daemon's server.

A browser pane may point at an `http` or `https` URL with a host, a `file` URL naming an absolute path, or `about:blank`. A web pane points at one plain artifact name, never a path. A file page loads with read access to the whole disk, so its assets open and its scripts can read any file the user can. A file page names a path on the daemon's machine, and a client on another machine draws that pane as a file it cannot reach rather than loading a path of its own.

The rule for a page is `page_allowed` in `wheel_proto`, and wheeld applies it wherever a page arrives: a caller's request, a `wheel.toml` value, a pane in the saved layout, and every page a client reports landing on. Only wheeld reads what a person types into an address field, with `page_typed`, whose result passes the same rule. A client keeps only its web view's scheme policy, and wheeld refuses any page the client reports that a pane may not be on.

*Page* is the term for what a browser pane points at. `url` names only the protocol fields that carry one.

When a browser pane moves to a new page, wheeld sends every client that one field as `pane_url` instead of the whole layout, and each client applies it to the tree it already holds. This is the one layout change a client does not take whole from the daemon.

Settings live in two `wheel.toml` files: a project's at the root of its space's directory, over this machine's in the instance directory, over a built-in default. They resolve key by key. The saved layout holds no settings. Every value is checked when it is set and again when it is read from a file. A value that a layer sets and wheel refuses is reported as a problem on that key, and the layer below stays in force.
