# ADR-0016: A page asks the daemon to run a command that a person has approved

`wheel.run({command, cwd, env})` in a page posts to `/artifacts/<pane>/.wheel/run`. The daemon runs `command` through `/bin/sh -c` in `cwd`, an existing absolute directory, detached in its own process group with no time limit, so a worker the command starts outlives the click. The command may call `wheel`, an agent or any other program; the daemon does not look inside it.

The first run of a command raises the permission request of ADR-0009, through the same function a refresh command uses. "Always" is remembered in `artifact-trust.json`, keyed by the artifact's folder, `cwd` and the command, together with the hash of every file the command names, so a page can never run a string nobody approved. The request shows the exact string and `cwd`.

What varies between two runs of one approved command is `env`. A page may set only upper-case names of letters, digits and `_`, and never `PATH`, `HOME`, `SHELL`, `IFS`, `ENV`, `BASH_ENV`, `PS4`, `TMPDIR`, `USER`, `PROMPT_COMMAND`, or a name starting `WHEEL_`, `DYLD_`, `LD_` or `BASH_FUNC_`, so a page cannot change how an approved command starts. The command's output goes to a log under the pane's data directory. It runs with the environment an agent has (`WHEEL_URL`, `WHEEL_TOKEN_FILE`, Wheel's bin first on `PATH`, so it can call `wheel`) plus `WHEEL_PANE`, `WHEEL_SPACE_DIR` and the pane's `WHEEL_DATA_DIR`; a refresh command gets the same agent environment.

The reply says the command started, with its pid, or, when it ended within 1.5 seconds, its exit status and the end of its output.
