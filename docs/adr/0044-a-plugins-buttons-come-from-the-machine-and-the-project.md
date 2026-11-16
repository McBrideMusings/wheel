# ADR-0044: A plugin's buttons come from the machine's wheel.toml and the project's

A plugin's buttons are written in two `wheel.toml` files: this machine's (`~/.wheel/wheel.toml`, or the file `wheeld` was started with `--machine-file`) and the project's. A button the machine's file writes applies in every project. This supersedes ADR-0025 for plugin buttons.

A plugin reads the machine's file first, then the project's. Each button is a table named for it (`[prs.buttons.review]`, `[beads.list.mark]`, `[beads.task]`): a project table with the same name replaces the machine's whole, its keys never merged with the machine's, and a project table holding `enabled = false` drops the machine's button in that project. A machine button's `run` runs in the project's directory, the space's, exactly as a project button does, so one command written once serves every project and a script it calls can live in one place outside any repo. A machine file that does not parse is reported the way a project file that does not parse is: a line naming the file and the error, on the page beside what still reads.

`wheeld` gives each plugin backend it starts the machine file's path in `WHEEL_MACHINE_FILE`; a backend run without it reads `~/.wheel/wheel.toml`.

The plugin itself still ships no commands, defaults, launchers or helper scripts: every command comes from a `wheel.toml` the person wrote, the machine's or the project's.
