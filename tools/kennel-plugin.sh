#!/bin/bash
# Check the kennel plugin package (plugins/kennel) against a throwaway wheeld, a copy of kennel's config
# (KENNEL_CONFIG_DIR) and an empty vault (XDG_DATA_HOME), so the machine's real routes and vault are never touched: its
# feeds, its route actions, the vault's write actions, file history and the gate in front of them, through
# `wheel plugin feed` and `wheel plugin action` as its pane. Prints PASS or FAIL per check. Uses no GUI, focus, mouse
# or keyboard. Everything it starts is killed by pid at the end. Needs `kennel`, `jq` and `git` on PATH and a config to
# copy. `vault add` moves the skill it adds to the Trash as kennel does; the check deletes that copy from ~/.Trash.
#
#   tools/kennel-plugin.sh            # uses target/debug/wheeld, wheel and wheel-runner
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WHEELD=${WHEELD:-$ROOT/target/debug/wheeld}
WHEEL=${WHEEL:-$ROOT/target/debug/wheel}
T=/private/tmp/claude/wheel/kplug
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
FAILS=0
PIDS=()

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
check() { if [ "$2" = ok ]; then pass "$1"; else fail "$1 ($2)"; fi; }

cleanup() {
  for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null; done
  sleep 0.5
  for pid in "${PIDS[@]}"; do kill -9 "$pid" 2>/dev/null; done
}
trap cleanup EXIT

command -v kennel > /dev/null || { echo "FAIL kennel is not on PATH"; exit 1; }
rm -rf "$T"
mkdir -p "$T/support" "$T/logs" "$T/proj/.wheel/plugins" "$T/kennel" "$T/routed" "$T/data" \
  "$T/vproj/.claude/skills/kplug-demo" "$T/vproj2/.claude/skills"
: > "$T/machine.toml"
cp "${KENNEL_CONFIG_DIR:-$HOME/.config/kennel}/config.toml" "$T/kennel/"
: > "$T/kennel/routes"
export KENNEL_CONFIG_DIR="$T/kennel" XDG_DATA_HOME="$T/data"
# Two projects: vproj holds a skill and a committed CLAUDE.md, vproj2 an empty skills folder.
MARK="kennel-plugin.sh check skill $$"
printf -- '---\nname: kplug-demo\ndescription: %s\n---\nhi\n' "$MARK" > "$T/vproj/.claude/skills/kplug-demo/SKILL.md"
printf '# guidance one\n' > "$T/vproj/CLAUDE.md"
for p in vproj vproj2; do git -C "$T/$p" init -q . && git -C "$T/$p" -c user.name=k -c user.email=k@k commit -q --allow-empty -m init; done
git -C "$T/vproj" add CLAUDE.md && git -C "$T/vproj" -c user.name=k -c user.email=k@k commit -q -m "add guidance"
printf '# guidance two\n' > "$T/vproj/CLAUDE.md"
ln -s "$ROOT/plugins/kennel" "$T/proj/.wheel/plugins/kennel"
PROFILE=$(kennel route list | awk '/^Profiles/ {p = 1; next} p {print $1; exit}')

"$WHEELD" --listen "127.0.0.1:$PORT" --support-dir "$T/support" --machine-file "$T/machine.toml" \
  --log-dir "$T/logs" --cli "$WHEEL" > "$T/wheeld.out" 2>&1 &
PIDS+=($!)
for _ in $(seq 1 100); do
  [ -f "$T/support/token" ] && nc -z 127.0.0.1 "$PORT" > /dev/null 2>&1 && break
  sleep 0.2
done
w() { "$WHEEL" --url "ws://127.0.0.1:$PORT/ws" --token-file "$T/support/token" "$@"; }

SPACE=$(w space new "$T/proj")
# The vault actions take a scope only when it is global or a space's folder, so both projects are spaces.
w space new "$T/vproj" > /dev/null
w space new "$T/vproj2" > /dev/null
PANE=$(w pane ls | awk -v s="$SPACE" '$1 == s {print $2; exit}')
w pane kind "$PANE" plugin --plugin kennel --plugin-pane kennel > /dev/null
echo "space $SPACE pane $PANE profile $PROFILE"

# The last value a feed sends within its first updates (the first may be null, before the first run ends).
feed() { w plugin feed "$PANE" "$1" --params "$2" --count "${3:-2}" 2> /dev/null | tail -1; }
D="{\"dir\":\"$T/routed\"}"

check "the [commands] table gives the package wheel-runner commands" "$(w plugin ls --json --space "$SPACE" | python3 -c '
import json, sys
p = next(p for p in json.load(sys.stdin) if p["id"] == "kennel")
print("ok" if (p.get("backend") or {}).get("command") == "wheel-runner commands" else p)')"

feed kennel.routes "$D" > "$T/routes0.json"
check "routes: kennel route list's lines, no route yet" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0].startswith("Routes (") and "  (none)" in v else v)' "$T/routes0.json")"

feed kennel.which "$D" > "$T/which.json"
check "which: kennel which in the pane's dir" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0] == "profile\t(no route)" else v)' "$T/which.json")"

feed kennel.harness '{"section":"hooks","scope":"global","harness":"claude"}' > "$T/harness.json"
check "harness: kennel harness show's JSON" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v["section"] == "hooks" and v["scope"]["kind"] == "global" and "blocks" in v else v)' "$T/harness.json")"

feed kennel.doctor '{}' > "$T/doctor.json"
check "doctor: its lines, whatever its exit code" "$(python3 -c '
import json, sys
f = json.load(open(sys.argv[1]))["value"]
print("ok" if f["error"] is None and any(l.startswith("Config") for l in f["value"]) else f)' "$T/doctor.json")"

w plugin action "$PANE" kennel.route_add --args "{\"path\":\"$T/routed\",\"profile\":\"$PROFILE\",\"permanent\":true,\"dir\":\"$T/routed\"}" > "$T/add.json"
check "route_add: exit 0 and the route in kennel route list" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and sys.argv[2] in open(sys.argv[3]).read() else r)' "$T/add.json" "$T/routed" "$T/kennel/routes")"

feed kennel.which "$D" > "$T/which2.json"
check "which: the routed dir now gets the profile" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if v[0] == "profile\t" + sys.argv[2] else v)' "$T/which2.json" "$PROFILE")"

w plugin action "$PANE" kennel.route_add --args "{\"path\":\"$T/routed\",\"profile\":\"no such profile\"}" > "$T/bad.json"
check "route_add with an unknown profile: exit 1 and kennel's message" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 1 and "no [profiles.no such profile]" in r["stderr"] else r)' "$T/bad.json")"

w plugin action "$PANE" kennel.route_rm --args "{\"path\":\"$T/routed\"}" > "$T/rm.json"
check "route_rm: exit 0 and the route gone" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and sys.argv[2] not in open(sys.argv[3]).read() else r)' "$T/rm.json" "$T/routed" "$T/kennel/routes")"

w plugin action "$PANE" kennel.route_prune --args '{}' > "$T/prune.json"
check "route_prune: exit 0, nothing expired" "$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r["code"] == 0 and r["stdout"].strip() == "nothing expired" else r)' "$T/prune.json")"

# ───── the vault's write actions and file history, each behind gate.sh ─────

# act <action> <args JSON>: the action's {code, stdout, stderr} in a new file, its path printed, and in $T/<action>.json
# (the action's last answer). It runs inside $(...), so a file name is the only thing it can hand back.
act() { local f; f=$(mktemp "$T/$1-XXXX"); w plugin action "$PANE" "kennel.$1" --args "$2" > "$f"; cp "$f" "$T/$1.json"; echo "$f"; }
# answered <file> <code> <text>: the action exited <code> and its output holds <text>.
answered() { python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
print("ok" if r.get("code") == int(sys.argv[2]) and sys.argv[3] in r.get("stdout", "") + r.get("stderr", "") else r)' "$@"; }
SKILL="$T/vproj/.claude/skills/kplug-demo"
S1="\"scope\":\"$T/vproj\",\"harness\":\"claude\""
S2="\"scope\":\"$T/vproj2\",\"harness\":\"claude\""

mkdir -p "$T/stray/kplug-demo" "$T/stray/.claude/skills/kplug-stray"
printf 'exit 0\n' > "$T/stray/gate.sh"
check "a vault action refuses a dir arg, so a gate.sh in the folder it names never runs" "$(answered "$(act vault_rm "{\"item\":\"skills/nope\",\"dir\":\"$T/stray\"}")" 2 "this action takes no dir")"
check "vault_add refuses a scope that is no space's folder" "$(answered "$(act vault_add "{\"path\":\"$T/stray/.claude/skills/kplug-stray\",\"scope\":\"$T/stray\",\"harness\":\"claude\"}")" 2 "is not the folder of a space")"
check "vault_add refuses a skill kennel does not list" "$(answered "$(act vault_add "{\"path\":\"$T/stray/kplug-demo\",$S1}")" 2 "is not a skill kennel lists")"
check "vault_add refuses a listed skill named under another project" "$(answered "$(act vault_add "{\"path\":\"$SKILL\",$S2}")" 2 "is not a skill kennel lists")"
check "vault_add moves a listed skill into the vault and links it back" "$(answered "$(act vault_add "{\"path\":\"$SKILL\",$S1}")" 0 "Added skills/kplug-demo")"
[ -L "$SKILL" ] && pass "the skill folder is now a link" || fail "the skill folder is not a link: $(ls -la "$SKILL")"

feed kennel.vault '{}' > "$T/vault.json"
check "vault: the added skill is listed" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
print("ok" if [s["item"] for s in v["skills"]] == ["skills/kplug-demo"] else v)' "$T/vault.json")"

check "vault_install refuses an item the vault does not hold" "$(answered "$(act vault_install '{"item":"skills/nope","harness":"claude","project":""}')" 2 "is not a skill in the vault")"
check "vault_install refuses a folder that is no space's" "$(answered "$(act vault_install "{\"item\":\"skills/kplug-demo\",\"harness\":\"claude\",\"project\":\"$T/stray\"}")" 2 "is not the folder of a space")"
check "vault_install refuses a harness kennel has none of" "$(answered "$(act vault_install "{\"item\":\"skills/kplug-demo\",\"harness\":\"--project\",\"project\":\"\"}")" 2 "is not a harness")"
check "vault_install links the skill into another project" "$(answered "$(act vault_install "{\"item\":\"skills/kplug-demo\",\"harness\":\"claude\",\"project\":\"$T/vproj2\"}")" 0 "Installed skills/kplug-demo for claude")"
[ -L "$T/vproj2/.claude/skills/kplug-demo" ] && pass "vproj2 has the link" || fail "vproj2 has no link"

check "vault_copy refuses a path outside a listed skills folder" "$(answered "$(act vault_copy "{\"item\":\"skills/kplug-demo\",\"path\":\"$T/vproj2/kplug-copy\",$S2}")" 2 "is not in a skills folder kennel lists")"
check "vault_copy refuses a path that climbs out" "$(answered "$(act vault_copy "{\"item\":\"skills/kplug-demo\",\"path\":\"$T/vproj2/.claude/skills/../kplug-copy\",$S2}")" 2 "is not a plain path")"
check "vault_copy refuses a path that exists" "$(answered "$(act vault_copy "{\"item\":\"skills/kplug-demo\",\"path\":\"$T/vproj2/.claude/skills/kplug-demo\",$S2}")" 2 "already exists")"
check "vault_copy copies the skill into the project and records it" "$(answered "$(act vault_copy "{\"item\":\"skills/kplug-demo\",\"path\":\"$T/vproj2/.claude/skills/kplug-copy\",$S2}")" 0 "Copied skills/kplug-demo")"

check "vault_unlink refuses a skill that is not a link" "$(answered "$(act vault_unlink "{\"path\":\"$T/vproj2/.claude/skills/kplug-copy\",$S2}")" 2 "is not a link into the vault")"
check "vault_unlink turns the link into a recorded copy" "$(answered "$(act vault_unlink "{\"path\":\"$T/vproj2/.claude/skills/kplug-demo\",$S2}")" 0 "now a copy of skills/kplug-demo")"

check "vault_update refuses a path kennel recorded no copy at" "$(answered "$(act vault_update "{\"path\":\"$SKILL\"}")" 2 "is not a copy kennel recorded")"
check "vault_update of a current copy: up to date" "$(answered "$(act vault_update "{\"path\":\"$T/vproj2/.claude/skills/kplug-copy\"}")" 0 "Up to date")"
# A new commit of the item in the vault puts the copy behind; update brings it to that commit.
VAULT="$T/data/kennel/vault"
printf 'two\n' >> "$VAULT/skills/kplug-demo/SKILL.md"
git -C "$VAULT" -c user.name=k -c user.email=k@k commit -q -am "kplug-demo two"
feed kennel.vault '{}' > "$T/vault-behind.json"
check "vault: the copy is behind after the vault's new commit" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"]
c = [c for c in v["copies"] if c["path"].endswith("/kplug-copy")]
print("ok" if c and c[0]["behind"] else v["copies"])' "$T/vault-behind.json")"
check "vault_update brings a behind copy to the item's commit" "$(answered "$(act vault_update "{\"path\":\"$T/vproj2/.claude/skills/kplug-copy\"}")" 0 "Updated ")"
grep -q '^two$' "$T/vproj2/.claude/skills/kplug-copy/SKILL.md" && pass "the copy holds the new commit's text" || fail "the copy did not change"

check "vault_rm refuses an item the vault does not hold" "$(answered "$(act vault_rm '{"item":"skills/nope"}')" 2 "is not an item in the vault")"
check "vault_rm of a linked item: kennel's refusal" "$(answered "$(act vault_rm '{"item":"skills/kplug-demo"}')" 1 "still linked")"

G="\"section\":\"guidance\",$S1"
feed kennel.file_log "{\"path\":\"$T/vproj/CLAUDE.md\",$G}" > "$T/log.json"
REV=$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]["value"] or []
print(v[0]["commit"] if [c["subject"] for c in v] == ["add guidance"] else "")' "$T/log.json")
[ -n "$REV" ] && pass "file_log: the commits that touched a listed file" || fail "file_log: $(cat "$T/log.json")"
feed kennel.file_show "{\"path\":\"$T/vproj/CLAUDE.md\",\"rev\":\"$REV\",$G}" > "$T/show-rev.json"
check "file_show at a commit: the file's text then" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if v["value"] == "# guidance one\n" else v)' "$T/show-rev.json")"
feed kennel.file_show "{\"path\":\"$T/vproj/CLAUDE.md\",\"rev\":\"\",$G}" > "$T/show-now.json"
check "file_show with no rev: the file's text now" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if v["value"] == "# guidance two\n" else v)' "$T/show-now.json")"
feed kennel.file_show "{\"path\":\"$T/vproj/CLAUDE.md\",\"rev\":\"x@y\",$G}" > "$T/show-at.json"
check "file_show refuses a rev holding @" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if "is not a revision" in (v["error"] or "") else v)' "$T/show-at.json")"
feed kennel.file_log "{\"path\":\"$T/vproj/CLAUDE.md\",$G,\"dir\":\"$T/stray\"}" > "$T/log-dir.json"
check "a file feed refuses a dir param" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if "this feed takes no dir" in (v["error"] or "") else v)' "$T/log-dir.json")"
feed kennel.file_log "{\"path\":\"$T/vproj/.git/config\",$G}" > "$T/log-bad.json"
check "file_log refuses a file the section does not list" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if v["value"] is None and "exit 2: " in (v["error"] or "") and "is not a file kennel lists" in v["error"] else v)' "$T/log-bad.json")"
feed kennel.file_log "{\"path\":\"$T/data/kennel/vault/skills/kplug-demo/SKILL.md\",\"section\":\"vault\",\"scope\":\"\",\"harness\":\"\"}" > "$T/log-vault.json"
check "file_log of a vault file: the vault's commits of it, vault add's the oldest" "$(python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))["value"]
print("ok" if v["error"] is None and len(v["value"]) == 2 and v["value"][-1]["subject"].startswith("add skills/kplug-demo") else v)' "$T/log-vault.json")"

# The original `vault add` moved to the Trash: its path is on the action's Original line (the Trash cannot be listed
# without Full Disk Access); deleted when it holds this run's mark.
ORIG=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["stdout"])' "$T/vault_add.json" | sed -n 's/.*Original: ~\(.*\)$/\1/p')
[ -n "$ORIG" ] && grep -qs "$MARK" "$HOME$ORIG/SKILL.md" && rm -rf "$HOME$ORIG"

echo "fails: $FAILS"
[ "$FAILS" -eq 0 ]
