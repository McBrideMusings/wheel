# ADR-0016: A page asks the daemon to run a command

`wheel.run({command, cwd, env})` in a page posts to `/artifacts/<pane>/.wheel/run`. The daemon runs `command` through `/bin/sh -c` in `cwd`, an existing absolute directory, detached in its own process group with no time limit, so a worker the command starts outlives the click. The command may call `wheel`, an agent or any other program; the daemon does not look inside it.

A page chooses `command`, `cwd` and `env`. A page may set only upper-case names of letters, digits and `_`, and never `PATH`, `HOME`, `SHELL`, `IFS`, `ENV`, `BASH_ENV`, `PS4`, `TMPDIR`, `USER`, `PROMPT_COMMAND`, or a name starting `WHEEL_`, `DYLD_`, `LD_` or `BASH_FUNC_`, so a page cannot change how a command starts. The command's output goes to a log under the pane's data directory. It runs with the environment an agent has (`WHEEL_URL`, `WHEEL_TOKEN_FILE`, Wheel's bin first on `PATH`, so it can call `wheel`) plus `WHEEL_PANE`, `WHEEL_SPACE`, `WHEEL_SPACE_DIR` and the pane's `WHEEL_DATA_DIR`; a page run has no pane process of its own, so it is given its space's id, which an agent or terminal pane is not: those learn their space by asking wheeld which space holds `WHEEL_PANE`. A refresh command gets the same agent environment.

The reply says the command started, with its pid, or, when it ended within 1.5 seconds, its exit status and the end of its output.
