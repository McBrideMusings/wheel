# ADR-0030: wheeld owns merged streams

A `[streams.<group>]` table in a `wheel.toml` names a group's sources, project entries over machine ones; `wheeld` runs a group when asked and owns it until it is stopped, its space closes or `wheeld` exits. Each source's lines are stamped with the time they arrived and, where the line carries one, the time it states; `wheeld` holds every line 2 seconds and emits them ordered by stated time, else arrival time. A running group appends its lines to `$TMPDIR/wheel-streams/<space>/<group>/<start time>.log` and keeps its recent lines in memory, where `wheel stream tail`, `wheel status` and the built-in `streams` page read them.

### Config

```toml
[streams.api]
web  = "weblogs tail --tab 12 --json"                                  # a command
srv1 = { command = "ssh server1 'tail -F /var/log/app.log'", tz = "UTC" } # zone for zone-less times
dev  = { run = "npm run dev" }                                         # opens a terminal pane running it
```

- Project `wheel.toml` groups win over the machine's, group by group (as `[runner]` does). `[logs]` is already taken (`push_url`); this table is `[streams]`.
- A source is a string (a command), or a table with exactly one of `command` / `run` plus optional `tz` (an IANA zone name). Any other key, both or neither of `command`/`run`, or an unknown zone is a problem the listing reports for that source.
- `command` runs through `/bin/sh -c` in the space's directory with the environment an agent pane gets (Wheel's bin first on `PATH`), in its own process group, stdout and stderr both read.
- `run` opens a terminal pane in the space running that command, and the stream reads that pane's output as lines with no escape sequences. Stop closes the pane.

### Ordering

- Every line carries `arrived` (when wheeld read it) and, when the line states one, `occurred`.
- Recognised stated times, looked for at the start of the line only (so a time inside a message body is never taken): RFC 3339 / ISO-8601 with or without zone and fraction (`T` or space separator); syslog `Oct 29 15:04:05`; time of day only `HH:MM:SS[.fff]` (today's date, or yesterday when that puts it more than 12 hours after arrival); a JSON object line with a string or epoch-number field `ts`, `time`, `timestamp`, `@timestamp` or `t` (epoch seconds or milliseconds by magnitude).
- A stated time with no zone is read in the source's `tz`, else this Mac's zone.
- wheeld holds each line 2 seconds after arrival, then emits lines in order of `occurred`, else `arrived`. Ties keep arrival order.

### Output

- Plain line: `<occurred-or-arrived, RFC 3339 UTC with ms> [<source>] <text>`; JSON line: `{occurred, arrived, source, text, time: "stated"|"arrived"}`.
- A source that exits emits a line `[<source>] exited <code>` and the rest keep running.
- Each run appends to `$TMPDIR/wheel-streams/<space name>/<group>/<start, UTC, YYYYMMDDTHHMMSSZ>.log` (the daemon's own `TMPDIR`, via the platform temp dir), and keeps the last 5,000 lines in memory.
- One running instance per group per space; starting a running group returns the running one.

### Surfaces

- `wheel stream ls [--json]` — groups for the space (project over machine), each source, problems, running or not.
- `wheel stream start <group>` / `wheel stream stop <group>` — prints the output file on start.
- `wheel stream tail <group> [--since 2m] [--follow] [--json]`.
- `wheel status --json` → `streams[]`: `{space, group, running, started_at, sources, lines, file}`.
- Built-in `streams` artifact page (`artifacts/streams/`, `builtin.toml`, installed from Settings › Artifacts like `runner`).
- wheeld logs each start, stop, source exit and problem with raw values to `wheeld.log`.
