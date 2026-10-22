# Logs — wheel

Three runtimes write logs today, each to a file on the device that runs it. None ships to Loki
yet, so the phone's lines are out of reach without the phone: `wheel-5c6d` builds the shipping
(skills/ref-cells/observability/production.md) and replaces the "Not reachable" row below.

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
- **Fetch:** `grep '^2026-10-22T14' ~/Library/Logs/wheel/wheeld.log`
- **Retention:** never rotated or capped (`tracing_appender::rolling::never`); the whole history
  is in the file.

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
- **A phone out and about:** not reachable from here. Until `wheel-5c6d` lands, say so, name what the
  phone's log would show, and look at the wheeld side of the same window: every phone request
  reaches wheeld and is logged there.

## Devices

"The laptop", "my Mac" → this Mac's files above. "My phone" → the iOS app; its requests also
appear in wheeld.log on the Mac it is paired with.

## From a line to a fix

`wheel status` and the Mac app's debug server (`verify.md` § 4) reproduce what a line shows;
from there it is `diagnose`, an `implement` pass, and `wrap-up`.
