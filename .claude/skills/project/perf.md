# Performance

What Wheel may cost, measured in the real Mac app against its own wheeld and the fake agent.
A pass runs the scenario for every area its diff touches, after `verify.md`; the swarm lead runs
both after each landing. A check over its limit fails the pass. On a loaded machine (`uptime`
load average over the core count) rerun the scenario once before counting a failure, and report
both runs.

Both scripts need `admin build mac`, start their own daemon and app, and touch nothing of the
person's running Wheel. `WHEEL_SHOTS=<dir>` keeps each table.

| Scenario | Run | Diff that needs it |
| --- | --- | --- |
| chat | `bash .claude/skills/project/tools/perf-chat.sh` | anything under `apple/`; a daemon change to sessions, files, links or artifacts |
| layout | `bash .claude/skills/project/tools/grid-motion.sh` | anything under `apple/Mac` or `apple/WheelUI`; a change to spaces or terminals |

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

## layout

`grid-motion.sh`: a 2x2 grid of terminals through 2.5 s of divider drag, four zoom toggles and
2.5 s of window resize (`POST /windows?action=sweep`), each recorded by `POST /metrics`. Limits
are its checks: a window resize keeps at most 10 frames over 34 ms (now 2), four zoom toggles
send one `termResize` each.

## Adding a measure

A feature whose cost grows with something a person accumulates (rows, spaces, accounts,
windows) or that runs per redraw, per keystroke or per frame gets a measure here in the same
change: the fixture grows that thing, the script prints the number, and the limit sits between
today's value and the value of the broken version, which the pass proves by running the
scenario once against it.
