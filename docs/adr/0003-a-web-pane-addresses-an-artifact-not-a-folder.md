# ADR-0003: A web pane addresses an artifact, not a folder

A web pane names an artifact by scope and name. The daemon computes its folder from one of three roots it owns — the space's directory, a gitignored subdirectory of the space's directory, and the daemon's own instance directory — so a filesystem path never crosses the API and there is nothing to validate. A pane cannot show an arbitrary folder; showing one means making it an artifact.
