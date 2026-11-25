---
name: project-deploy
description: "wheel: which layer a landed change needs and the order to deploy it \u2014 the client (Wheel.app), the server (wheeld, with the person's yes), built-in artifacts, then the phone. Read before deploying or restarting anything."
---

# Deploy — wheel

Two layers run separately (ADR-0020): the **client** (`/Applications/Wheel.app`, with the `wheel` command in `Contents/Helpers`) and the **server** (the running `wheeld`, which ships as `Contents/Helpers/Wheel Server.wheelserver`, is copied by the app at launch to `~/Library/Application Support/Wheel/Server/Versions/<source hash>/Wheel Server.app` and runs from there as its own launch agent; macOS lists it in Privacy & Security as Wheel Server with its Desktop, Documents, Downloads and network-volume switches). A client deploy copies the new `wheeld` there but never runs it; a server restart ends every terminal and agent session, so it waits for the person's yes. Never `pkill` Wheel, and never start a deploy before the change has landed on origin/main.

## What a landed change needs

| Changed | Steps |
| --- | --- |
| `apple/` only (Mac app, WheelKit, WheelUI) | 1 |
| `cli/` | 1 (the running server finds `Helpers/wheel` by path, so it uses the new one at once) |
| `daemon/`, `proto/`, `Cargo.lock` | 1, then 2 (each also stages and restarts the Linux host) |
| `daemon/src/wheel.css`, `daemon/src/client.js` | 1, then 2 (compiled into `wheeld`) |
| `artifacts/<built-in>/` | 1, 2, then 3 |
| `apple/iOS/`, or WheelKit when the phone needs it | 4 |
| a `[runner]` entry in a `wheel.toml` | none |

## The order

1. **Client:** `make deploy-client` (runner: `wheel: deploy`). Builds the Swift Release objects and the GPUI client on them (ADR-0040) and swaps `/Applications/Wheel.app`; the running app, its terminals and the agent that ran it stay up until the swap. The new app compares the running `wheeld`'s source hash with the bundled one and offers Restart Server when they differ. Confirm: `pgrep -fx /Applications/Wheel.app/Contents/MacOS/wheel-gpui` shows a process started after the install, the `wheeld` pid is unchanged, and `wheel status --json`'s `host.source` against `plutil -extract SourceHash raw /Applications/Wheel.app/Contents/Resources/WheelDaemon.plist` says whether the app offers a restart (different) or not (same). The step ends with `make deploy-linux` (below), whose last line names the Linux host's state.
2. **Server, only when step 1 left it `different` and the person said yes:** `make restart-server` (it runs the installed app's `Resources/wheel-server.sh restart`: Current moves to the new version, the agent restarts). It ends every terminal and agent; the person decides when. Confirm `~/Library/Logs/wheel/restart-server.log` shows an old and a new pid, and `wheel status --json`'s `host.source` equals the installed `WheelDaemon.plist`'s `SourceHash`. The detached restart then runs `make restart-server-linux` (below) and writes its lines to the same log.
3. **Built-in artifacts, after step 2:** built-ins are compiled into `wheeld` and installed as copies in `~/.wheel/artifacts/`. A starting `wheeld` brings every installed built-in that is behind up to date when none of its files was edited; one with an edited file stays as it is, and `wheel artifact builtin ls` lists it as `edited`. For that one run `wheel artifact builtin update <name>` (it shows the diff), then `--replace-edited` to take the shipped page or `--keep-edited` to keep the edit. `wheel artifact new --renderer <built-in>` reports a stale built-in renderer as a problem.
4. **Phone, last:** `make deploy-phone` (needs `WHEEL_IOS_DEVICE`).

Between steps 1 and 2 the app works with the running `wheeld` (ADR-0050): a feature whose op the running `wheeld` does not list stays in place disabled or hidden, a closable notice reads "The server was updated." with Restart Server, and `wheel` refuses only a command whose op is missing, naming it. That is the expected state until step 2, not a failure. Only a raised level in `BREAKS` (`proto/src/ops.rs`) replaces the window with "This Wheel needs a newer wheeld" until step 2.

Several landed beads deploy together in one pass through this order, once, at the end of the batch: not once per bead.

## The Linux host

wheeld also runs on a Linux host, the always-on agent machine (ADR-0058), named by `WHEEL_LINUX_HOST` (an ssh destination) in `~/.claude/.env`. Both steps go through `tools/linux-server.sh`, log to `~/Library/Logs/wheel/deploy-linux.log`, and with no host set say so and do nothing.

- `make deploy-linux` (run last by `make deploy-client`) cross-compiles `wheeld`, `wheel` and `wheel-runner` with `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.41` and copies them to `~/.local/share/wheel/server/versions/<source hash>/` on the host through a staging folder, never touching a version that exists, and points `staged` at it. It never runs it; the running pid stays.
- `make restart-server-linux` (run by `make restart-server` after the Mac's restart) moves `previous` to the old `current` and `current` to `staged`, installs `~/.config/systemd/user/wheel.service` from `daemon/linux/wheeld.service.in` when it is missing or differs, restarts it, and waits for `wheel status` on the host. It ends every terminal and agent on the host, so it waits for the person's yes like step 2. Its last line names the old and new pid and the source hash `wheel status` reports.
- An unreachable host never fails either step: the step prints `behind: unreachable (<ssh's error>)` and exits 0. A failed build, or a host that answers ssh and then fails a step, fails it.
- The remote listener on the host's Tailscale address is the host's own setting (`remote.listen` in its `~/.wheel/wheel.toml`), not the unit's.
- `bash tools/linux-server-check.sh` drives both steps on the host with a throwaway instance (`wheel-check` on 127.0.0.1:7491) and removes it.

## Dev builds

`make deploy-dev` and `make restart-server-dev` follow the same order for `/Applications/Wheel Dev.app`. Its daemon (`com.piercemakes.wheel.dev.daemon`) keeps running after the app quits; unload it when testing is done with `launchctl bootout gui/$(id -u)/com.piercemakes.wheel.dev.daemon`.
