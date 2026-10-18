# ADR-0009: A person approves a refresh command, remembered per folder and command

No refresh command runs until a person answers a permission request, the same request the lead's tools raise. "Always" is remembered in `artifact-trust.json` in the instance directory, not in the saved layout, keyed by the artifact's folder and the command, so editing the command asks again. A command that runs project code its text does not pin down (a relative path, an interpreter, a path under the artifact's folder or the space's directory) offers only "allow once". Approvals are listed and forgotten by artifact name (`wheel artifact trust [--forget <name>]`).

A timer tick never raises a permission request: it runs only a remembered pair and otherwise skips the tick, logging why, until the pair is remembered again. The on-demand refresh is the path that asks. A pane's timer state (`off`, `ticking`, `untrusted`, `idle`, `stopped`) is computed on each read, not remembered.

A refusal, a non-zero exit, a kill by signal or the 60-second time limit is the run's recorded result, and the pane goes on serving its static files.
