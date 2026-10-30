# Swarm — wheel

`H="$HOME/.claude/skills/swarm/swarm"`. Run each command below from the worktree root.

## Heavy commands

Every one of these goes through `"$H" heavy -- <command>`:

- `cargo build`, `cargo test` (any filter), `cargo clippy --all-targets -- -D warnings`
- `swift build` / `swift test --package-path apple/WheelKit`
- `xcodegen generate` followed by `xcodebuild … build` for the Mac app
- `xcodebuild -scheme WheelUI -destination 'generic/platform=iOS Simulator' build` from `apple/WheelUI` (no simulator is booted; give it `-derivedDataPath $DD-ios`)
- a check script that starts its own wheeld and drives it for more than a few seconds

Run the full `cargo test` once per round of edits, not after every file.

## The cheap check

- Rust: `cargo check -p wheeld` (or `-p wheel`, `-p wheel_proto` for the crate you touched),
  then one test binary: `cargo test -p wheeld --test spaces <name>`. Both still go through
  `heavy`: each one uses every core.
- Swift: `swift build --package-path apple/WheelKit` through `heavy`.

## Shared caches

`swarm launch` sets `CARGO_TARGET_DIR` to your worktree's own `target/`, cloned from the main
checkout's, so dependencies and the Ghostty build start warm and your first build recompiles only
wheel's crates. Never override it. The daemon binary you drive is `$CARGO_TARGET_DIR/debug/wheeld`,
the CLI is `$CARGO_TARGET_DIR/debug/wheel`, and the fake agent is
`$CARGO_TARGET_DIR/debug/wheel-fake-agent`; only your builds write them. The Mac app's
`embed-wheeld.sh` builds into the same `<worktree>/target`.

The daemon's terminal crate builds with Zig 0.16 through `tools/zig` (mise). The first build
after a fresh target downloads Ghostty's source and compiles it, which takes a few minutes. Run
cargo from inside the worktree so `.cargo/config.toml` applies.

Offline, the build works only when the Ghostty source is local (`GHOSTTY_SOURCE_DIR=<extracted tarball>`; otherwise the build script's download fails with `Connection refused`) and `~/.cache/zig` already holds the Zig packages `zig build` fetches (`uucode`, `translate-c`). Your cloned `target/` already has the compiled library, so an offline worker only hits this after `cargo clean` or a `ghostty-vt-sys` version bump; park the bead with `blocked: no network for the ghostty-vt-sys build` rather than clearing `~/.cache/zig`.

## Per-worker resources

The integration tests bind `127.0.0.1:0` and use per-process temp directories, so they already
run side by side. Everything verify.md and the check scripts use by a fixed name
does not. Derive your own from the bead id before your first run:

```bash
N=$(( $(printf %s "$SWARM_WORKER" | cksum | cut -d' ' -f1) % 400 ))
WHEEL_PORT=$(( 7500 + N ))             # replaces verify.md's 7499 and any 7498
WHEEL_DEBUG_PORT=$(( 8000 + N ))       # replaces the Mac debug server's 7421 / 7431
SCR=/private/tmp/claude/Projects-wheel/swarm-$SWARM_WORKER
INST=$SCR/inst LOGS=$SCR/logs DD=$SCR/DerivedData
```

Run check scripts with `env -u WHEEL_SPACE -u WHEEL_PANE`: inside a Wheel pane the CLI
otherwise asks the real wheeld for the space holding `WHEEL_PANE` and acts on that space.

- **wheeld:** `--listen 127.0.0.1:$WHEEL_PORT --support-dir $INST --machine-file $INST/wheel.toml --log-dir $LOGS`.
  Never start one without `--support-dir` and `--machine-file`: the defaults are the person's real
  instance and `~/.wheel/wheel.toml`.
- **wheel CLI:** `--url ws://127.0.0.1:$WHEEL_PORT/ws --token-file $INST/token` on every call.
- **Check scripts** in `.claude/skills/project/tools/` start their own wheeld on a free
  port (from `$SWARM_WORKER`) in a `mktemp` directory and remove both on exit, so run them as
  they are, through `heavy`: `"$H" heavy -- bash .claude/skills/project/tools/all.sh`.
- **Mac app:** build with `-derivedDataPath $DD`. Launch with
  `open -g -n --env WHEEL_URL=ws://127.0.0.1:$WHEEL_PORT/ws --env WHEEL_TOKEN_FILE=$INST/token --env WHEEL_DEBUG_PORT=$WHEEL_DEBUG_PORT $DD/Build/Products/Debug/Wheel.app`.
  Never install into `/Applications`. Quit it by PID or by its path under `$DD`, never with
  `pkill Wheel`, which also kills the person's own copy. A worker drives and screenshots the app
  itself through `$WHEEL_DEBUG_PORT` (verify.md §4); a change the app shows is never left
  as "needs a person to look".
- **Processes:** stop every wheeld you started by its PID. `make kill` stops every wheeld built in
  the checkout it runs from, a check script's included, so a worker never runs it.
- **Deploys:** a worker never runs `make deploy-client`, `make restart-server` or their dev pair;
  the client and the server deploy separately, and only when Pierce says (ADR-0020). Change
  `daemon/` or `proto/` only when the bead needs what only the server holds (processes, files,
  other clients); everything else goes in the client, and the server depends on nothing in it.
- **Cleanup**, before your pass ends: stop your processes, then `rm -rf $SCR`.

## Decisions a worker never commits

An ADR under `docs/adr/` or a term in `docs/CONTEXT.md` that your change adds or alters goes in
your report under **Needs you**, as the proposed text beside the current text. It never goes in
a commit: the person reads it first.

## Per-worker resources, added for the Mac app's checks

The windows, menu bar, drag, server and pairing checks (`.claude/skills/project/tools/windows.sh`, `menubar.sh`, `window-commands.sh`, `drag.sh`, `server.sh`, `pairing-window.sh`) launch the built app with `open -g -n`, each on its own debug port and its own preferences domain (`WHEEL_PREFS_SUITE`), and start their own wheeld. Run them as they are through `"$H" heavy -- bash .claude/skills/project/tools/<name>.sh` with `WHEEL_DERIVED_DATA=$DD`. They never call `screencapture` and never touch the person's windows or daemon.
