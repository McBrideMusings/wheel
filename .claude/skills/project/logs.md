# Logs

- **wheeld:** `<support dir>/logs/` for the instance (`~/Library/Application Support/wheel/` for the release app's daemon, `.../wheel-dev/` for the Debug build's); `wheel status` prints `log_path`. A check's daemon logs under its own `--log-dir`.
- **The Mac app:** `~/Library/Logs/wheel/WheelMac.log`, timestamped; every layout op, window change, drag, pairing and server action writes a line.
- **The iPhone app:** `Documents/phone.log` inside its container (`xcrun simctl get_app_container <udid> com.piercemakes.wheel.phone data`, or `devicectl` for a device).
- **Launch agent:** `launchctl print gui/$UID/com.piercemakes.wheel.daemon` (`.dev.daemon` for Debug).

Read the file; never ask the person to paste lines.
