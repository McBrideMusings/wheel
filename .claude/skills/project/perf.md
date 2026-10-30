# Performance

What Wheel may cost, measured in the real Mac app against its own wheeld and the fake agent.
A pass runs the scenario for every area its diff touches, after `verify.md`; the swarm lead runs
both after each landing. A check over its limit fails the pass. On a loaded machine (`uptime`
load average over the core count) rerun the scenario once before counting a failure, and report
both runs.

The chat and layout scripts need `admin build mac`, start their own daemon and app, and touch
nothing of the person's running Wheel; `--live` only reads the running one. `WHEEL_SHOTS=<dir>`
keeps each table.

| Scenario | Run | Diff that needs it |
| --- | --- | --- |
| chat | `bash .claude/skills/project/tools/perf-chat.sh` | anything under `apple/`; a daemon change to sessions, files, links or artifacts |
| live | `bash .claude/skills/project/tools/perf-chat.sh --live` | every client deploy, against the installed app and running wheeld |
| spaces | `bash .claude/skills/project/tools/perf-spaces.sh` | anything under `apple/Mac` or `apple/WheelUI`; a change to how spaces, the sidebar or a window draw |
| layout | `bash .claude/skills/project/tools/grid-motion.sh` | anything under `apple/Mac` or `apple/WheelUI`; a change to spaces or terminals |
| project | `bash .claude/skills/project/tools/project-artifact.sh` | anything under `artifacts/project/` |

## chat

Six spaces, each an agent pane of 40 replies (headings, a found path, a missing path unique to
the reply, a link, code, a table) and a terminal. After launch it reads the app's CPU once a
second; then, for 36 s, the agent on screen replies every 2 s while `POST /agent/typing` types
ten characters a second into its composer. wheeld's `status` `requests` (`<client> <op>` counts
since start) gives the app's own requests.

| Measure | Limit | Now | Target |
| --- | --- | --- | --- |
| seconds until CPU under 10% after connecting | 30 | 22 | 10 (wheel-652w) |
| idle CPU, mean of 10 s | 5% | 0.1% | |
| CPU while typing and replying, mean of 20 s | 50% | 27% | 10% (wheel-3q2k) |
| app requests per reply, `mac log` left out | 3 | 1.1 | |

A client that asks again on every redraw shows in requests per reply: the reply-link code
before its fix read 78.

`--live` takes two readings from the installed app over 60 s while you use it: under 10% of one
core, and under 120 `file_peek` lines a minute in `~/Library/Logs/wheel/wheeld.log`.

## spaces

Sixteen spaces, each an agent pane of 40 replies and a terminal, the last on screen: four sidebar
toggles, then 2.5 s of window resize, every request with `quiet=1`. A hidden space stays mounted
and must cost nothing when the grid changes size.

| Measure | Limit | Now | Broken |
| --- | --- | --- | --- |
| pane bodies, four sidebar toggles | 4 | 0 | 132 (every pane of every space on each toggle) |
| terminal resizes, four sidebar toggles | 8 | 4 | 32 |
| four sidebar toggles, duration | 5 s | 3.5 s | 6.4-8.8 s |
| pane bodies, window resize | 4 | 0 | |

A hidden space draws no chat transcript (`AgentTranscript` reads `spaceShown`), so AppKit lays out
none of its table rows while the grid moves; a transcript left scrolled back through history comes
back at the same rows. Frames: 91-98 for the toggles and 119-125 for the resize, against 62 and
98-105 with hidden transcripts mounted and 141 for the toggles with only the active space mounted.

## layout

`grid-motion.sh`: a 2x2 grid of terminals through 2.5 s of divider drag, four zoom toggles and
2.5 s of window resize (`POST /windows?action=sweep`), each recorded by `POST /metrics`. Limits
are its checks: a window resize keeps at most 10 frames over 34 ms (now 2), four zoom toggles
send one `termResize` each.

## project

`project-artifact.sh` ends by collecting this repo's own beads the way the project artifact does
every 30 s per open pane, and prints the snapshot's size and the time it took. The snapshot grows
with every bead a person files; a closed bead travels without its long fields, which is what holds
it down.

| Measure | Limit | Now | Broken |
| --- | --- | --- | --- |
| snapshot of this repo (657 beads) | 800 KB | 449 KB | 1,323 KB (closed beads keep description, notes, design, acceptance) |
| `collect.py` on this repo | 10 s | 3.9 s | |

## Adding a measure

A feature whose cost grows with something a person accumulates (rows, spaces, accounts,
windows) or that runs per redraw, per keystroke or per frame gets a measure here in the same
change: the fixture grows that thing, the script prints the number, and the limit sits between
today's value and the value of the broken version, which the pass proves by running the
scenario once against it.
