# Logs — wheel

wheeld keeps three files and ships them to Loki whenever the machine setting `logs.push_url`
names a push endpoint (`~/.claude/skills/ref-cells/observability/production.md`): its own log,
the usage journal, and the lines the apps send it. `wheel status` shows the shipping as `logs`:
`off`, `ok` with `last_ok_ms`, or `failing` with the error, and `waiting_bytes` either way. The
apps do not send their lines yet (`wheel-5c6d.2`), so the phone's own lines are still out of
reach without the phone.

**Fetch from Loki** (`LOKI_ADDR` in `~/.claude/.env`). Labels: `app="wheel"`, `source`
(`wheeld`, `journal`, `app`), `host` (the Mac wheeld runs on), `client` (`mac`, `local`,
`device`, or the app's name for `source="app"`) and, for journal lines, `kind` (the request's op):

```
logcli query --since=12h '{app="wheel", source="journal"}'
logcli query --since=12h '{app="wheel", source="journal"} | json | ok="false"'
logcli query --from=2026-10-22T02:00:00Z --to=2026-10-22T09:00:00Z '{app="wheel", source="wheeld"} |= "error"'
logcli query --since=1h '{app="wheel", client="device"}'
```

Every line starts with an ISO 8601 timestamp. Read the user's "last night" or "just now" as a
window, say the window used in UTC beside their local time, and filter on the prefix, e.g.
`grep '^2026-10-22T0[2-9]'`. wheeld's and the Mac app's lines are UTC (`Z`).

## wheeld — this Mac

- **File:** `~/Library/Logs/<instance>/wheeld.log`; the instance is `wheel` for the installed
  server, `wheel-dev` for a dev build, and `--log-dir` overrides the directory. `wheel status`
  prints the running daemon's `log_path`.
- **Launch agent:** the installed server runs as `com.piercemakes.wheel.daemon`
  (`com.piercemakes.wheel.dev.daemon` for Debug) with no `--log-dir`;
  `launchctl print gui/$(id -u)/com.piercemakes.wheel.daemon` shows its arguments and state.
- **What:** `tracing` lines, level from `WHEEL_LOG` (default `info`): every client connection,
  request and refusal, pane and agent lifecycle, refresh commands.
- **Fetch:** Loki (above), or `grep '^2026-10-22T14' ~/Library/Logs/wheel/wheeld.log`.
- **Retention:** past 20 MB the file rolls to `wheeld.log.1` once Loki has every line of it, or
  at 40 MB regardless; Loki keeps 90 days. The shipper's own lines (`wheeld::logship`) stay in
  the file and never ship.

## Usage journal — this Mac

- **File:** `<support dir>/usage.jsonl` (`~/Library/Application Support/wheel/usage.jsonl`), one
  JSON line per request a client made: `ts_ms`, `op`, `client`, `device` (a paired device's
  name), `space`, `pane`, `ok`, `ms`, `err`. Keystrokes, resizes, status and usage polls and
  render handshakes are not journaled; nothing a person typed or an agent said is.
- **Fetch:** Loki `source="journal"` (above), or the file.
- **Retention:** past 20 MB the file rolls to `usage.jsonl.1` once its lines are in Loki, or at
  40 MB regardless. The apps' lines (`applog.jsonl`) follow the same rule.
- **Shipper's position:** `<support dir>/logship/<file>.offset` holds the file's inode and the
  byte offset Loki has; a rolled file is a new inode and is read from its start.

## Mac app — this Mac

- **File:** `~/Library/Logs/wheel/WheelMac.log`, also mirrored to the unified log
  (subsystem `com.piercemakes.wheel`, category `app`).
- **Fetch:** `grep '^2026-10-22T14' ~/Library/Logs/wheel/WheelMac.log`, or
  `log show --info --last 2h --predicate 'subsystem == "com.piercemakes.wheel"'` (without `--info` it shows none of them).
- **Retention:** the file is never capped; the unified log ages out on the OS's schedule.

## iOS app — the phone

- **File:** `Documents/phone.log` inside the app's container.
- **Simulator:** `xcrun simctl get_app_container booted <bundle id> data` gives the container;
  the log is under `Documents/`.
- **A real phone on this Mac's network:** `xcrun devicectl device copy from` can pull
  `Documents/phone.log` out of the app's container while the phone is paired and reachable.
- **A phone out and about:** its own lines are not reachable until `wheel-5c6d.2` lands. Say so,
  and read the wheeld side of the same window: every phone request reaches wheeld and is in the
  journal as `client="device"` with the device's name.

## Devices

"The laptop", "my Mac" → this Mac's files above. "My phone" → the iOS app; its requests also
appear in wheeld.log on the Mac it is paired with.

## From a line to a fix

`wheel status` and the Mac app's debug server (`verify.md` § 4) reproduce what a line shows;
from there it is `diagnose`, an `implement` pass, and `wrap-up`.
