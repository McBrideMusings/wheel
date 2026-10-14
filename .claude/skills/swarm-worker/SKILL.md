---
name: swarm-worker
description: "How a swarm worker builds and tests wheel beside other workers: heavy cargo/swift/xcodebuild commands, the shared target directory, and per-worker wheeld ports, instance directories, DerivedData and Mac debug ports. Read before the first build or test in a swarm worktree."
---

# Swarm worker — wheel

`H="$HOME/.claude/skills/swarm/swarm"`. Run each command below from the worktree root.

## Heavy commands

Every one of these goes through `"$H" heavy -- <command>`:

- `cargo build`, `cargo test` (any filter), `cargo clippy --all-targets -- -D warnings`
- `swift build` / `swift test --package-path apple/WheelKit`
- `xcodegen generate` followed by `xcodebuild … build` for the Mac app
- a check script that starts its own wheeld and drives it for more than a few seconds

Run the full `cargo test` once per round of edits, not after every file.

## The cheap check

- Rust: `cargo check -p wheeld` (or `-p wheel`, `-p wheel_proto` for the crate you touched),
  then one test binary: `cargo test -p wheeld --test spaces <name>`. Both still go through
  `heavy`: the target directory is shared, so even a check takes cargo's lock.
- Swift: `swift build --package-path apple/WheelKit` through `heavy`.

## Shared caches

`swarm launch` sets `CARGO_TARGET_DIR` to the main checkout's `target/`. Never override it and
never run `cargo clean`. The daemon binary you drive is `$CARGO_TARGET_DIR/debug/wheeld`, and the
CLI is `$CARGO_TARGET_DIR/debug/wheel`. Both, and `wheel-fake-agent`, are rebuilt by whichever
worker built last, so another worker's older source can replace yours between two calls. In the
same `heavy` call as your build, copy `wheeld`, `wheel` and `wheel-fake-agent` into `$SCR/bin`,
and drive only those copies.

## Per-worker resources

The integration tests bind `127.0.0.1:0` and use per-process temp directories, so they already
run side by side. Everything the verify-project skill and the check scripts use by a fixed name
does not. Derive your own from the bead id before your first run:

```bash
N=$(( $(printf %s "$SWARM_WORKER" | cksum | cut -d' ' -f1) % 400 ))
WHEEL_PORT=$(( 7500 + N ))             # replaces verify-project's 7499 and any 7498
WHEEL_DEBUG_PORT=$(( 8000 + N ))       # replaces the Mac debug server's 7421 / 7431
SCR=/private/tmp/claude/Projects-wheel/swarm-$SWARM_WORKER
INST=$SCR/inst LOGS=$SCR/logs DD=$SCR/DerivedData
```

- **wheeld:** `--listen 127.0.0.1:$WHEEL_PORT --support-dir $INST --log-dir $LOGS`. Never start
  one without `--support-dir`: the default is the person's real instance.
- **wheel CLI:** `--url ws://127.0.0.1:$WHEEL_PORT/ws --token-file $INST/token` on every call.
- **Check scripts** under `/private/tmp/claude/Projects-wheel/` hard-code 7499 or 7498 and the
  shared `verify-inst`/`verify-logs` directories. Copy one into `$SCR` and change those values
  before running it; never run the shared copy.
- **Mac app:** build with `-derivedDataPath $DD`. Launch with
  `open -g -n --env WHEEL_URL=ws://127.0.0.1:$WHEEL_PORT/ws --env WHEEL_TOKEN_FILE=$INST/token --env WHEEL_DEBUG_PORT=$WHEEL_DEBUG_PORT $DD/Build/Products/Debug/Wheel.app`.
  Never install into `/Applications`. Quit it by PID or by its path under `$DD`, never with
  `pkill Wheel`, which also kills the person's own copy.
- **Processes:** stop every wheeld you started by its PID. `admin kill` stops whatever wheeld is
  on the default port, so a worker never runs it.
- **Cleanup**, before your pass ends: stop your processes, then `rm -rf $SCR`.

## Known flake

`a_web_pane_watch_follows_its_folder_and_stops_with_the_pane` (`daemon/tests/spaces.rs`) can fail
in a full run and passes alone in 12–15 s (wheel-6pg). When it is the only failure, rerun it
alone and report both results. Any other failure is real.
