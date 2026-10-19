#!/usr/bin/env bash
# § 2c Web panes: artifact resolution, serving, cookie trade, refusals, reload, the pane data
# directory, the artifact verbs and the approved refresh command.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ/site" "$PROJ/other"
printf '<html><link rel=stylesheet href=style.css><body>site-page</body></html>\n' >"$PROJ/site/index.html"
printf 'body{}\n' >"$PROJ/site/style.css"
printf 'SECRET=1\n' >"$PROJ/site/.env"
mkdir -p "$PROJ/site/lessons"
printf 'lesson-one\n' >"$PROJ/site/lessons/one.html"
printf '[artifacts.site]\npath = "site"\n' >"$PROJ/wheel.toml"

start_daemon --no-lead --state-file "$STATE"

http() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

w space new "$PROJ" >/dev/null
P="$(w space ls | jget spaces.0.root.id)"
B="$(BASE)/artifacts/$P"
TOK="$(TOKEN)"

# --- the pane names an artifact -----------------------------------------------------------
expect_exit_out 6 'needs artifact' "pane kind web with no artifact exits 6" w pane kind "$P" web
expect_exit_out 6 'only for a web pane' "--artifact on a terminal exits 6" w pane kind "$P" terminal --artifact site
expect_exit 0 "pane kind web --artifact site exits 0" w pane kind "$P" web --artifact site
eq "space ls: kind web" "$(w space ls | jget spaces.0.root.kind)" "web"
eq "space ls: scope project" "$(w space ls | jget spaces.0.root.scope)" "project"
eq "space ls: name site" "$(w space ls | jget spaces.0.root.name)" "site"
eq "web_panes row: dir is the artifact folder" "$(w space ls | jget web_panes.0.dir)" "$PROJ/site"

# both registries: a bare name tries project, then machine
mkdir -p "$SCR/machine-site"
printf 'machine-page\n' >"$SCR/machine-site/index.html"
printf '[artifacts.site]\npath = "%s"\n' "$SCR/machine-site" >"$INST/wheel.toml"
expect_exit 0 "bare name still resolves" w pane kind "$P" web --artifact site
eq "the project entry wins a bare name" "$(w space ls | jget spaces.0.root.scope)" "project"
expect_exit 0 "--scope machine" w pane kind "$P" web --artifact site --scope machine
eq "the machine entry's folder is watched" "$(w space ls | jget web_panes.0.dir)" "$SCR/machine-site"
expect_exit 0 "back to project" w pane kind "$P" web --artifact site --scope project
rm -f "$INST/wheel.toml"

# --- serving ---------------------------------------------------------------------------
JAR="$SCR/jar"
eq "no token: 401" "$(http "$B/")" "401"
eq "bearer header: 200" "$(http -H "Authorization: Bearer $TOK" "$B/")" "200"
HDRS="$(curl -s -D - -o /dev/null "$B/index.html?token=$TOK")"
printf %s "$HDRS" | head -1 | grep -q 303 && ok "?token= is traded: 303" || fail "?token= is traded: 303" "$HDRS"
printf %s "$HDRS" | grep -qi "^location: /artifacts/$P/index.html\r\?$" && ok "location has no token" || fail "location has no token" "$HDRS"
printf %s "$HDRS" | grep -qi "^set-cookie: wheel_token=.*Path=/artifacts/$P/; HttpOnly; SameSite=Strict" && ok "cookie is HttpOnly, SameSite=Strict, scoped to the pane" || fail "cookie attributes" "$HDRS"
eq "curl -L -c jar ends at 200" "$(http -L -c "$JAR" "$B/index.html?token=$TOK")" "200"
eq "folder address with the cookie: 200" "$(http -b "$JAR" "$B/")" "200"
eq "style.css with the cookie: 200" "$(http -b "$JAR" "$B/style.css")" "200"
eq "style.css is text/css" "$(curl -s -b "$JAR" -o /dev/null -w '%{content_type}' "$B/style.css" | cut -d';' -f1)" "text/css"
eq "a file in a subfolder serves" "$(http -b "$JAR" "$B/lessons/one.html")" "200"
eq "../ (path-as-is) is 404" "$(http --path-as-is -b "$JAR" "$B/../x")" "404"
eq "%2e%2e is 404" "$(http --path-as-is -b "$JAR" "$B/%2e%2e/x")" "404"
eq ".env is 404" "$(http -b "$JAR" "$B/.env")" "404"
eq "a hostile Origin is 403" "$(http -b "$JAR" -H 'Origin: http://evil.example' "$B/")" "403"
eq "the daemon's own Origin is 200" "$(http -b "$JAR" -H "Origin: http://127.0.0.1:$PORT" "$B/")" "200"
eq "?token= on a pane that is not a web pane: 404" "$(http "$(BASE)/artifacts/p999/index.html?token=$TOK")" "404"
ln -s /etc "$PROJ/site/outlink"
eq "a symlink inside the folder pointing out is 404" "$(http -b "$JAR" "$B/outlink/hosts")" "404"
has_log "artifact token traded for a cookie" "log: token traded for a cookie"
has_log "artifact served.*bytes=" "log: artifact served"
has_log "refused: browser origin" "log: refused: browser origin"
has_log 'refused: missing or wrong token via="artifacts' "log: refused: missing or wrong token via artifacts"
has_log "artifact not found, or outside the pane's folder" "log: outside the pane's folder"
has_log "watching web pane" "log: watching web pane"

# --- reload ----------------------------------------------------------------------------
WAITOUT="$SCR/reload.out"
bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane wait-reload $P >'$WAITOUT' 2>&1"
RW=$BGPID
sleep 1
printf '<html>site-page-2</html>\n' >"$PROJ/site/index.html"
got_reload() { grep -q "^pane_reload $P at " "$WAITOUT"; }
if poll 3 got_reload; then ok "an edit in the artifact folder prints pane_reload"; else fail "pane_reload after an edit" "$(cat "$WAITOUT")"; fi
wait "$RW" 2>/dev/null
sleep 1.5
bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane wait-reload $P --timeout 2 >'$WAITOUT' 2>&1"
RW=$BGPID
sleep 0.7
printf 'X=2\n' >"$PROJ/site/.env"
wait "$RW" 2>/dev/null
if grep -q pane_reload "$WAITOUT"; then fail "writing only .env sends nothing" "$(cat "$WAITOUT")"; else ok "writing only .env sends nothing"; fi
has_log "pane reload pane=$P.*changes=" "log: pane reload with changes"

# --- status watches ----------------------------------------------------------------------
eq "status: two watch rows for the web pane" "$(w status | jget watches)" "2"
eq "status: first role is artifact" "$(w status | jget watches.0.role)" "artifact"
eq "status: second role is data" "$(w status | jget watches.1.role)" "data"

# --- survives a restart --------------------------------------------------------------------
restart_daemon
has_log "watching web pane pane=$P" "log: watching web pane after restart"
eq "the pane still shows the artifact" "$(w space ls | jget spaces.0.root.name)" "site"

# --- refusals on the pane's artifact name ---------------------------------------------------
for bad in / /etc .. . .env a/b ../out -x ""; do
  out="$(w pane kind "$P" web --artifact "$bad" 2>&1)"; code=$?
  if [ "$code" != 0 ]; then ok "--artifact '$bad' is refused"; else fail "--artifact '$bad' is refused" "$out"; fi
done
expect_exit_out 6 'no artifact nosuch' "a plain name no wheel.toml has is refused" w pane kind "$P" web --artifact nosuch
expect_exit 2 "--dir no longer exists" w pane kind "$P" web --dir /etc
w pane kind "$P" web --artifact site >/dev/null

# --- the pane's data directory ---------------------------------------------------------------
DD="$INST/panes/$P"
check "the pane's data directory exists" test -d "$DD"
check "the artifact's folder gains no data/" test ! -e "$PROJ/site/data"
check "the project gains no panes/ directory" test ! -e "$PROJ/panes"
printf '{"n":1}\n' >"$DD/rows.json"
mkdir -p "$DD/sub"; printf 'subfile\n' >"$DD/sub/x.txt"
w space ls >/dev/null
eq "data/rows.json: 200" "$(http -b "$JAR" "$B/data/rows.json")" "200"
eq "data/rows.json is application/json" "$(curl -s -b "$JAR" -o /dev/null -w '%{content_type}' "$B/data/rows.json" | cut -d';' -f1)" "application/json"
eq "data/rows.json holds the written bytes" "$(curl -s -b "$JAR" "$B/data/rows.json")" '{"n":1}'
eq "data/sub/x.txt: 200" "$(http -b "$JAR" "$B/data/sub/x.txt")" "200"
eq "data/../../etc/passwd is 404" "$(http --path-as-is -b "$JAR" "$B/data/../../etc/passwd")" "404"
eq "data//etc/passwd is 404" "$(http --path-as-is -b "$JAR" "$B/data//etc/passwd")" "404"
printf 'S=1\n' >"$DD/.env"
eq "data/.env is 404" "$(http -b "$JAR" "$B/data/.env")" "404"
eq "a missing data file is 404" "$(http -b "$JAR" "$B/data/missing.json")" "404"
eq "data without the cookie is 401" "$(http "$B/data/rows.json")" "401"
mkdir -p "$PROJ/site/data"; printf '{"n":"artifact-own"}\n' >"$PROJ/site/data/rows.json"
eq "an artifact's own data/ is unreachable (the pane's bytes win)" "$(curl -s -b "$JAR" "$B/data/rows.json")" '{"n":1}'
expect_out 'data/ is reserved' "artifact show reports data/ is reserved" w artifact show site
rm -rf "$PROJ/site/data"

# a write in the data directory reaches wait-reload
bg bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' pane wait-reload $P >'$WAITOUT' 2>&1"
RW=$BGPID
sleep 1
printf '{"n":2}\n' >"$DD/rows.json"
if poll 3 got_reload; then ok "a write in the data directory prints pane_reload"; else fail "pane_reload after a data write" "$(cat "$WAITOUT")"; fi
wait "$RW" 2>/dev/null
has_log "pane reload.*role=Data" "log: reload with role=Data"
has_log "pane reload.*role=Artifact" "log: reload with role=Artifact"

# the directory goes with the web kind
expect_exit 0 "pane kind empty exits 0" w pane kind "$P" empty
check "the data directory is removed" bash -c "! test -e '$DD'"
eq "no watches rows remain" "$(w status | jget watches)" "0"
has_log "removed the pane's data directory" "log: removed the pane's data directory"
w pane kind "$P" web --artifact site >/dev/null
check "showing the artifact again makes a fresh empty directory" bash -c "test -d '$DD' && [ -z \"\$(ls -A '$DD')\" ]"

# a restore sweeps what no layout names
stop_daemon
mkdir -p "$INST/panes/p77/sub"; printf '{}\n' >"$INST/panes/p77/sub/rows.json"
start_daemon --no-lead --state-file "$STATE"
swept() { [ ! -e "$INST/panes/p77" ]; }
poll 10 swept
check "a pane directory no layout names is swept" test ! -e "$INST/panes/p77"
check "the live pane's directory stays" test -d "$INST/panes/$P"
has_log "swept a pane data directory no layout names" "log: swept a pane data directory"

# --- the artifact verbs -----------------------------------------------------------------------
NEWOUT="$(w artifact new dash 2>/dev/null)"
eq "artifact new prints the folder" "$NEWOUT" "$PROJ/.wheel/artifacts/dash"
eq "the folder holds only index.html" "$(ls -A "$PROJ/.wheel/artifacts/dash")" "index.html"
check "wheel.toml gains the entry" grep -q '^\[artifacts.dash\]' "$PROJ/wheel.toml"
check "...with the path" grep -q 'path = ".wheel/artifacts/dash"' "$PROJ/wheel.toml"
check "...with pane = false" bash -c "sed -n '/artifacts.dash/,/^\[/p' '$PROJ/wheel.toml' | grep -q 'pane = false'"
expect_exit_out 6 'there is already a project artifact dash' "the same name again exits 6" w artifact new dash
for bad in a/b .. .env /etc -x ""; do
  out="$(w artifact new "$bad" 2>&1)"; code=$?
  if [ "$code" != 0 ]; then ok "artifact new '$bad' is refused"; else fail "artifact new '$bad' is refused" "$out"; fi
done
expect_exit_out 2 'is not one of the accepted values' "--scope local exits 2" w artifact new x --scope local
expect_exit_out 2 'is not one of the accepted values' "--scope repo exits 2" w artifact new x --scope repo
check "nothing writes a .gitignore" test ! -e "$PROJ/.gitignore"
eq "--scope machine prints <inst>/artifacts/<name>" "$(w artifact new viewer --scope machine 2>/dev/null)" "$INST/artifacts/viewer"
check "...and writes <inst>/wheel.toml" grep -q '^\[artifacts.viewer\]' "$INST/wheel.toml"
eq "--path makes the folder where asked" "$(w artifact new pathed --path "$SCR/elsewhere" 2>/dev/null)" "$SCR/elsewhere"
check "...and the entry names it absolutely" grep -q "path = \"$SCR/elsewhere\"" "$PROJ/wheel.toml"

w artifact new flagged --pane >/dev/null 2>&1
eq "artifact show --json reports pane: true for --pane" "$(w artifact show flagged --json | jget pane)" "true"
expect_out 'in the pane picker' "plain artifact show marks the flagged one" w artifact show flagged
eq "artifact show --json reports pane: false by default" "$(w artifact show dash --json | jget pane)" "false"
eq "artifact ls --pane lists the flagged project one and the machine ones asking" \
  "$(w artifact ls --pane --json | jget artifacts.0.name)" "flagged"

LSJSON="$(w artifact ls --json)"
check "artifact ls --json carries resolves" bash -c "printf %s '$LSJSON' | grep -q resolves"
expect_exit_out 6 'no artifact' "artifact show of a name nowhere exits 6" w artifact show nosuch
expect_exit_out 6 'no' "artifact show viewer --scope project exits 6 (viewer is machine only)" w artifact show viewer --scope project

# broken entries report problems wherever they are shown
mkdir -p "$SCR/emptyfolder/data"
printf '[artifacts.broken]\npath = "%s"\ncolour = 1\nevery = 2\n' "$SCR/emptyfolder" >>"$PROJ/wheel.toml"
BROKEN="$(w artifact show broken)"
for s in 'unknown key colour, which nothing reads' 'every asks for a refresh on a timer, but no command says what to run' 'no index.html in' 'data/ is reserved'; do
  if printf %s "$BROKEN" | grep -q "$s"; then ok "artifact show broken: $s"; else fail "artifact show broken: $s" "$BROKEN"; fi
done
printf '<html>b</html>\n' >"$SCR/emptyfolder/index.html"
rmdir "$SCR/emptyfolder/data"

# add / remove
mkdir -p "$SCR/course"; printf '<html>c</html>\n' >"$SCR/course/index.html"
w artifact add "$SCR/course" --pane >/dev/null 2>&1
check "artifact add writes the entry named for the folder" grep -q '^\[artifacts.course\]' "$PROJ/wheel.toml"
expect_exit_out 6 'already' "artifact add of the same name again exits 6" w artifact add "$SCR/course"
printf 'not html\n' >"$SCR/plain.txt"
expect_exit_out 6 'is not an HTML file' "artifact add of a non-HTML file exits 6" w artifact add "$SCR/plain.txt"
expect_exit_out 6 'nothing at' "artifact add of a missing path exits 6" w artifact add "$SCR/nonesuch"
printf '<html>s</html>\n' >"$SCR/interview-sheet.html"
w artifact add "$SCR/interview-sheet.html" >/dev/null 2>&1
expect_out 'interview-sheet.html' "a file registers under its name, with the page" w artifact show interview-sheet
expect_exit_out 6 'every' "--every without --command exits 6" w artifact add "$SCR/course" --name c2 --every 5
BEFORE="$(cat "$PROJ/wheel.toml")"
expect_exit 6 "a refused add" w artifact add "$SCR/course"
eq "a refused add leaves wheel.toml byte-identical" "$(cat "$PROJ/wheel.toml")" "$BEFORE"
expect_exit_out 0 "removed project artifact course" "artifact remove prints the removal" w artifact remove course
check "the folder is left as it was" test -f "$SCR/course/index.html"
expect_exit 6 "a second remove exits 6" w artifact remove course
has_log "artifact created" "log: artifact created"
has_log "artifact added" "log: artifact added"
has_log "artifact removed" "log: artifact removed"
has_log "artifact has problems" "log: artifact has problems"

# --- refresh: approval, environment, failure, refusal, trust ---------------------------------------
mkdir -p "$PROJ/rf"
printf '<html>rf</html>\n' >"$PROJ/rf/index.html"
cat >>"$PROJ/wheel.toml" <<TOML
[artifacts.rf]
path = "rf"
command = 'pwd > "\$WHEEL_DATA_DIR/pwd"; env | grep ^WHEEL_ > "\$WHEEL_DATA_DIR/env"; echo "{\\"n\\":9}" > "\$WHEEL_DATA_DIR/rows.json"'
TOML
RP="$(w pane split "$P" right)"
w pane kind "$RP" web --artifact rf >/dev/null 2>&1 &
KIND_PID=$!; BG_PIDS+=("$KIND_PID")
perm_id() { w perm list 2>/dev/null | plutil -extract 0.id raw -o - - 2>/dev/null; }
if poll 10 perm_id; then
  PID="$(perm_id)"
  eq "the refresh raises a request naming the command's artifact" "$(w perm list | jget 0.session)" "$RP"
  check "title names the artifact and the space" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' perm list | grep -q 'project artifact rf'"
  eq "answer allow exits 0" "$(w perm answer "$PID" allow >/dev/null 2>&1; echo $?)" "0"
else
  fail "the refresh raises a request" "perm list stayed empty"
fi
wait "$KIND_PID" 2>/dev/null
ran() { [ "$(w status | jget refreshes.0.last.exit)" = 0 ]; }
poll 15 ran
RB="$(BASE)/artifacts/$RP"
eq "the command's output serves from the pane's data directory" "$(curl -s -H "Authorization: Bearer $TOK" "$RB/data/rows.json")" '{"n":9}'
eq "the command ran in the space's directory" "$(cat "$INST/panes/$RP/pwd")" "$PROJ"
check "WHEEL_PANE is set" grep -qx "WHEEL_PANE=$RP" "$INST/panes/$RP/env"
check "WHEEL_SPACE_DIR is set" grep -qx "WHEEL_SPACE_DIR=$PROJ" "$INST/panes/$RP/env"
check "WHEEL_DATA_DIR is set" grep -qx "WHEEL_DATA_DIR=$INST/panes/$RP" "$INST/panes/$RP/env"
eq "status refreshes: one row" "$(w status | jget refreshes)" "1"
eq "last.exit is 0" "$(w status | jget refreshes.0.last.exit)" "0"
eq "allow is not remembered" "$(w artifact trust --json)" "[]"
check "the project gains nothing from the run" test ! -e "$PROJ/rows.json"

# always remembers it
(w artifact refresh "$RP" >/dev/null 2>&1) & RFP=$!; BG_PIDS+=("$RFP")
if poll 10 perm_id; then w perm answer "$(perm_id)" always >/dev/null 2>&1; fi
wait "$RFP" 2>/dev/null
eq "artifact trust lists one remembered pair" "$(w artifact trust --json | jget 0.name)" "rf"
check "<inst>/artifact-trust.json exists" test -f "$INST/artifact-trust.json"
check "the saved layout does not carry the approval" file_lacks WHEEL_DATA_DIR "$STATE"
expect_exit 0 "a trusted refresh runs with nothing waiting on a person" w artifact refresh "$RP"
eq "perm list stays empty" "$(w perm list)" "[]"
has_log "refresh command trusted from now on" "log: trusted from now on"
has_log "refresh command already trusted" "log: already trusted"
has_log "refresh end.*exit" "log: refresh end"
expect_exit 0 "artifact trust --forget rf" w artifact trust --forget rf
eq "...leaves []" "$(w artifact trust --json)" "[]"
expect_exit 6 "--forget nosuch exits 6" w artifact trust --forget nosuch

# a failing refresh is not a blank pane
sed -i '' "s|^command = .*|command = 'printf \"boom\\\\n\" >\&2; exit 3'|" "$PROJ/wheel.toml"
(w artifact refresh "$RP" >"$SCR/fail.out" 2>&1; echo $? >"$SCR/fail.code") & RFP=$!; BG_PIDS+=("$RFP")
if poll 10 perm_id; then w perm answer "$(perm_id)" always >/dev/null 2>&1; fi
wait "$RFP" 2>/dev/null
eq "a failing command exits the CLI with 1" "$(cat "$SCR/fail.code")" "1"
check "...printing exit 3" grep -q 'exit 3' "$SCR/fail.out"
eq "status reports exit 3" "$(w status | jget refreshes.0.last.exit)" "3"
eq "...with the stderr tail" "$(w status | jget refreshes.0.last.stderr_tail)" "boom"
eq "the pane still serves its page" "$(http -H "Authorization: Bearer $TOK" "$RB/index.html")" "200"

# deny is the run's result
w artifact trust --forget rf >/dev/null 2>&1
(w artifact refresh "$RP" >"$SCR/deny.out" 2>&1; echo $? >"$SCR/deny.code") & RFP=$!; BG_PIDS+=("$RFP")
if poll 10 perm_id; then w perm answer "$(perm_id)" deny >/dev/null 2>&1; fi
wait "$RFP" 2>/dev/null
eq "deny exits the CLI with 1" "$(cat "$SCR/deny.code")" "1"
check "...saying the person refused" grep -q 'the person refused this command' "$SCR/deny.out"
check "status last.error says so" bash -c "'$WHEEL' --url ws://127.0.0.1:$PORT/ws --token-file '$INST/token' status | grep -q 'the person refused'"

# an entry with no command refreshes nothing
sed -i '' '/^command = /d' "$PROJ/wheel.toml"
expect_exit_out 6 'declares no refresh command' "artifact refresh on an artifact with no command exits 6" w artifact refresh "$RP"
expect_exit_out 6 '' "artifact refresh on a pane showing no artifact exits 6" w artifact refresh "$(w pane split "$P" down)"

finish
