#!/usr/bin/env bash
# § 3d Wheel control from inside a pane: the WHEEL_* variables, the shell integration (zsh and bash
# start the person's own files, then put the wheel bin directory first on PATH), the claude/codex
# wrappers, `wheel --skill`, named keys, answering a permission from a pane, and `wheel install`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The daemon's HOME holds startup files that leave a mark and put /user/bin on PATH, so a pane
# shows whether they ran and whether the wheel bin directory still came first.
mkdir -p "$SCR/home"
printf 'export USERRC=zsh-rc-ran\nPATH=/user/bin:$PATH\n' >"$SCR/home/.zshrc"
printf 'export USERRC=bash-rc-ran\nPATH=/user/bin:$PATH\n' >"$SCR/home/.bash_profile"
export HOME="$SCR/home"
use_fake_agent
start_daemon
w space new "$SCR" >/dev/null
SPACE="$(w status | jget spaces.0.id)"
P="$(w status | jget spaces.0.root.id)"

# A chat agent: the variables, the bin directory first on PATH, and Bash(wheel:*) allowed.
w pane start "$P" agent --message "show setup" >/dev/null
w agent wait "$P" --timeout 10 >/dev/null
SEEN="$(w agent read "$P")"
expect_out "wheel: env=1 space=$SPACE pane=$P url=ws://127.0.0.1:$PORT/ws token_file=$INST/token path=$INST/shell/bin allowed=Bash\(wheel:\*\) denied=Bash\(wheel perm answer:\*\)" "a chat agent has WHEEL_* set, the bin directory first and wheel allowed except perm answer" echo "$SEEN"
expect_out "wheel control for terminals and agents" "log: the control address is logged" cat "$(LOG)"

# A zsh pane and a bash pane: the person's rc ran, then the bin directory is first.
for sh in zsh bash; do
  w config set terminal.shell "/bin/$sh" --machine >/dev/null
  T="$(w pane split "$P" right | tail -1)"
  w pane kind "$T" terminal >/dev/null
  w term send "$T" 'd=no; [ "$WHEEL_SPACE_DIR" = "'"$SCR"'" ] && d=yes; f=other; case ${PATH%%:*} in */shell/bin) f=bin;; esac; echo "rc=$USERRC seat=${WHEEL_SPACE-none}/$WHEEL_PANE/$WHEEL_ENV dir=$d first=$f"' --enter
  expect_exit 0 "$sh pane: the person's rc ran, WHEEL_* and WHEEL_SPACE_DIR are set and the bin directory is first on PATH" w term read "$T" --until "rc=$sh-rc-ran seat=none/$T/1 dir=yes first=bin" --timeout 10
done

# wheel --skill and --help need no daemon.
expect_out "WHEEL_PANE" "wheel --skill prints the guide" "$WHEEL" --skill
expect_out "wheel --skill" "wheel --help points at the guide" "$WHEEL" --help

# Named keys: Ctrl-C ends a foreground sleep.
K="$(w term open --cwd "$SCR")"
w term send "$K" "sleep 30" --key enter
w term send "$K" "" --key ctrl-c
w term send "$K" 'echo key-$((6*7))' --key enter
expect_exit 0 "a named key reaches the terminal (ctrl-c ends sleep, next command runs)" w term read "$K" --until key-42 --timeout 10

# Answering a permission from an agent pane.
Q="$(w pane split "$P" down | tail -1)"
w pane start "$Q" agent --message "permission please" >/dev/null
poll 10 test -n "$(perm_id)"
expect_exit 0 "perm answer takes the agent pane" w perm answer "$Q" allow

# wheel install links the CLI and reports its state.
expect_out "^missing:" "install --status before" "$WHEEL" install --status --dir "$SCR/bin"
expect_out "^installed:" "install links the CLI" env PATH="$SCR/bin:$PATH" "$WHEEL" install --dir "$SCR/bin"

finish
