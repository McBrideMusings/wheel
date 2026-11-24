# ADR-0051: Every artifact is offered to a pane

This supersedes ADR-0018's sentences on the `pane` key: promote no longer sets it, and the pane picker no longer reads it.

A pane picker offers every artifact the space has: the project's own entries and the machine's. `artifact_ls` lists them all and takes no filter, and an artifact record (`ArtifactInfo`) carries no `pane` field. The registries are the scopes: a `machine` artifact is offered in every space, a `project` one only in its own project.

`pane` is not an entry key. `wheel artifact new`, `wheel artifact add` and `wheel artifact promote` write none, and the CLI has no `--pane` flag on `artifact new`, `artifact add` or `artifact ls`. An entry that still carries `pane`, true or false, reads as any unknown key does: it lists with the problem `unknown key pane, which nothing reads`, and like any entry with a problem a page still opens in a pane, while promote refuses it, until the line is removed.

Removing the field adds no level to `BREAKS` (ADR-0050). `pane` was a defaulted field. `wheeld` ignores a request field it does not know, so an older client that sends `pane: true` gets the full list, and a newer client decodes an older `wheeld`'s records, which still carry `pane`, by ignoring it.
