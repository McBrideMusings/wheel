# Sourced by tools/plugin-backend.sh and tools/plugin-call.sh: `clock_copy <dest>` copies the clock
# fixture package to <dest>. With CLOCK_BACKEND=rust the package's backend is the SDK's clock example
# (sdk/rust/examples/clock.rs, built by `cargo build -p wheel-plugin-sdk --examples`) copied in as
# `backend`; `backend.py` stays as a file of comments that the checks touch to make a watched change,
# since editing the binary would break its signature.
clock_copy() {
  cp -R "$ROOT/tools/fixtures/plugins/clock" "$1"
  [ "${CLOCK_BACKEND:-python}" = rust ] || return 0
  cp "${CLOCK_EXE:-$ROOT/target/debug/examples/clock}" "$1/backend"
  printf '# the watched marker of the Rust clock backend\n' > "$1/backend.py"
  sed -i '' 's|^command = .*|command = "exec ./backend"|' "$1/wheel-plugin.toml"
}
