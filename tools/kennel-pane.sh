#!/bin/bash
# Check the kennel pane (plugins/kennel/panes/kennel.luau) in the hidden GPUI app (never shown, never focused) against a
# throwaway wheeld, a copy of kennel's config (KENNEL_CONFIG_DIR) and an empty vault (XDG_DATA_HOME): a Skills row's
# menu moves the skill into the vault after a confirmation, another opens the file's history, a commit row shows the
# file at that commit, and the Vault view offers install, copy and remove. Drives the pane through the app's debug
# /plugin route (set, raise, state) and draws each state with `wheel pane shot`. Needs `kennel`, `jq` and `git` on PATH.
#
#   tools/kennel-pane.sh [path to a Wheel*.app holding wheel-gpui and libWheelGpuiBridge.dylib]
#   default: clients/gpui/target/app/Wheel.app
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=${1:-$ROOT/clients/gpui/target/app/Wheel.app}
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/kpane
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
PORT=$(free_port)
DEBUG=$(free_port)
PIDS=()
FAILS=0
cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

command -v kennel > /dev/null || { echo "FAIL kennel is not on PATH"; exit 1; }
rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/kennel" "$T/data" "$T/proj/.wheel/plugins" "$T/proj/.claude/skills/kplug-demo"
: > "$T/machine.toml"
cp "${KENNEL_CONFIG_DIR:-$HOME/.config/kennel}/config.toml" "$T/kennel/"
: > "$T/kennel/routes"
export KENNEL_CONFIG_DIR="$T/kennel" XDG_DATA_HOME="$T/data"
MARK="kennel-pane.sh check skill $$"
printf -- '---\nname: kplug-demo\ndescription: %s\n---\nhi\n' "$MARK" > "$T/proj/.claude/skills/kplug-demo/SKILL.md"
git -C "$T/proj" init -q . && git -C "$T/proj" -c user.name=k -c user.email=k@k commit -q --allow-empty -m init
ln -s "$ROOT/plugins/kennel" "$T/proj/.wheel/plugins/kennel"
SKILL="$T/proj/.claude/skills/kplug-demo"

env -u WHEEL_SPACE -u WHEEL_PANE "$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" >> "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { env -u WHEEL_SPACE -u WHEEL_PANE "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }
AUTH="Authorization: Bearer $(cat "$T/support/token")"
dbg() { curl -s -G -H "$AUTH" -X POST "http://127.0.0.1:$DEBUG/plugin" --data-urlencode "pane=$PANE" "$@"; }
shot() { w pane shot "$PANE" "$T/$1.png" > "$T/shot-$1.out" 2>&1 || echo "shot $1: $(cat "$T/shot-$1.out")"; }

SPACE=$(w space new "$T/proj")
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin kennel --plugin-pane kennel > /dev/null

env -u WHEEL_SPACE -u WHEEL_PANE WHEEL_HIDDEN=1 WHEEL_DEBUG_PORT=$DEBUG WHEEL_URL="ws://127.0.0.1:$PORT/ws" \
  WHEEL_TOKEN_FILE="$T/support/token" WHEEL_CLIENT_SPACE="$SPACE" \
  WHEEL_GPUI_BRIDGE="$APP/Contents/MacOS/libWheelGpuiBridge.dylib" \
  "$APP/Contents/MacOS/wheel-gpui" --space "$SPACE" > "$T/app.out" 2>&1 &
PIDS+=($!)

# q <python expression over d (the pane's /plugin answer), texts, comp, state and rows>: prints the expression's value, or "no state".
q() {
  dbg --data-urlencode action=state > "$T/pane.json" 2>/dev/null
  python3 - "$T/pane.json" "$1" <<'PY'
import json, sys
try: d = json.load(open(sys.argv[1]))
except Exception: print("no state"); sys.exit()
texts = [c.get("text") for c in d.get("components", []) if isinstance(c, dict) and c.get("text")]
def comp(kind): return next((c for c in d.get("components", []) if c.get("type") == kind), None)
# The script's state, and the rows the table draws as "cell | cell | ..." text.
state = json.loads(d.get("state") or "{}")
rows = [c for c in d.get("components", []) if c.get("type") == "TableRow"]
print(eval(sys.argv[2]))
PY
}
# wait_for <seconds> <python condition over d, texts, comp>: succeeds once it holds.
wait_for() {
  local end=$((SECONDS + $1))
  while [ $SECONDS -lt $end ]; do
    [ "$(q "bool($2)")" = True ] && return 0
    sleep 0.3
  done
  return 1
}
raise() { dbg --data-urlencode action=raise --data-urlencode "component=$1" --data-urlencode "key=$2" ${3:+--data-urlencode "context=$3"} > "$T/raise.json"; }

wait_for 40 'd.get("error") is None and any(t.startswith("This folder (proj)") for t in texts)' && pass "the pane draws Routes with no script error" ||
  fail "the pane: $(q '(d.get("error"), texts[:6])')"

CHIPS=$(q 'comp("ChoicePicker")["id"]')
dbg --data-urlencode action=set --data-urlencode "component=$CHIPS" --data-urlencode 'json=["skills"]' > /dev/null
wait_for 20 'any(r["cells"].startswith("kplug-demo | 1 files") for r in rows)' && pass "Skills lists the project's skill" || fail "Skills: $(q 'rows')"
shot skills

# The first table row is the project's one skill; its menu's first item moves it into the vault.
# A row's menu is raised on the row's own node (`<table>/<row key>`), keyed `menu.N.action`; read it again after each change.
row0() { q 'rows[0]["node"]'; }
raise "$(row0)" "menu.0.action"
wait_for 10 'state.get("confirm", {}).get("label") == "Move into the vault" and "moves the original folder to the Trash" in state["confirm"]["ask"]' &&
  pass "Move into the vault asks first" || fail "no confirmation: $(q 'state') $(cat "$T/raise.json")"
shot confirm
[ ! -L "$SKILL" ] && pass "nothing moved before the answer" || fail "the skill moved before the answer"
DIALOG=$(q 'comp("ConfirmationDialog")["id"]')
raise "$DIALOG" "buttons.0.action"
wait_for 20 'any(t.startswith("Added skills/kplug-demo") for t in texts)' && pass "the answer runs vault add: its output shows" ||
  fail "after the answer: $(q 'texts[-3:]')"
[ -L "$SKILL" ] && pass "the skill folder is now a link into the vault" || fail "the skill folder is not a link"
wait_for 20 'state.get("confirm") is None and any(r["cells"].startswith("kplug-demo | vault · 1 files") for r in rows)' &&
  pass "the row now reads vault and the dialog is gone" || fail "after add: $(q 'rows')"
# The original kennel moved to the Trash: its path is on vault add's Original line; delete it when it holds this run's mark.
ORIG=$(q 'next((t for t in texts if t.startswith("Added")), "")' | sed -n 's/.*Original: ~\(.*\)$/\1/p')
[ -n "$ORIG" ] && grep -qs "$MARK" "$HOME$ORIG/SKILL.md" && rm -rf "$HOME$ORIG"
shot added

raise "$(row0)" "menu.1.action"
wait_for 20 'state.get("file", {}).get("path", "").endswith("kplug-demo/SKILL.md") and len(rows) == 1' &&
  pass "History of SKILL.md lists its one commit: $(q 'rows[0]["cells"]')" || fail "history: $(q 'state, rows')"
shot history
COMMIT=$(q 'rows[0]["id"]')
raise "$(q 'comp("Table")["id"]')" rowAction "{\"id\":\"$COMMIT\"}"
wait_for 20 'any(t.startswith("at ") for t in texts) and any("hi" in t and "kplug-demo" in t for t in texts)' &&
  pass "a commit row shows the file at that commit" || fail "file at commit: $(q 'texts')"
shot file

dbg --data-urlencode action=set --data-urlencode "component=$CHIPS" --data-urlencode 'json=["vault"]' > /dev/null
wait_for 20 'state.get("file") is None and any(r["cells"].startswith("kplug-demo | 1 files") for r in rows)' && pass "Vault lists the skill" || fail "Vault: $(q 'rows')"
shot vault
# The skill's menu: install globally, install here, copy here, remove. Copy names the folder the link already holds, so
# the gate refuses it; Remove asks first, then kennel refuses while the link points at the item.
raise "$(row0)" "menu.2.action"
wait_for 20 'state.get("out", "").startswith("exit 2: ") and "already exists" in state["out"]' &&
  pass "Copy into this project goes through the gate: $(q 'state["out"]')" || fail "copy: $(q 'state.get("out")')"
raise "$(row0)" "menu.3.action"
wait_for 10 'state.get("confirm", {}).get("label") == "Remove from the vault"' && pass "Remove from the vault asks first" ||
  fail "remove: $(q 'state')"
raise "$(q 'comp("ConfirmationDialog")["id"]')" "buttons.0.action"
wait_for 20 'state.get("out", "").startswith("exit 1: ") and "still linked" in state["out"]' &&
  pass "the answer runs vault rm: kennel's refusal shows" || fail "rm: $(q 'state.get("out")')"
shot vault-rm

echo "snapshots: $T/skills.png $T/confirm.png $T/added.png $T/history.png $T/file.png $T/vault.png $T/vault-rm.png"
[ $FAILS -eq 0 ] && echo "ALL PASS" || echo "$FAILS FAILED"
exit $FAILS
