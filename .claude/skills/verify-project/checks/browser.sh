#!/usr/bin/env bash
# § 2c-browser Browser panes: opening, typed text, the one URL rule, kinds that own fields,
# nothing served, the saved file, one write per burst of navigation.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STATE="$SCR/spaces.json"
PROJ="$SCR/proj"
mkdir -p "$PROJ"
start_daemon --no-lead --state-file "$STATE"
w space new "$PROJ" >/dev/null
P="$(w pane ls | awk '{print $2}')"
fld() { w space ls | jget "$1"; }
url_of() { w space ls | grep -A4 "\"id\": \"$1\"" | grep -m1 '"url"' | sed 's/.*"url": "\(.*\)".*/\1/'; }

expect_exit 0 "pane kind browser exits 0" w pane kind "$P" browser
eq "no --page opens the homepage setting (about:blank)" "$(fld spaces.0.root.url)" "about:blank"
eq "space ls reports kind browser" "$(fld spaces.0.root.kind)" "browser"

# the homepage setting, as it resolves for the pane's space
w config set browser.homepage https://home.example --space s1 >/dev/null
Q="$(w pane split "$P" right)"
w pane kind "$Q" browser >/dev/null
eq "the project's homepage is what a new browser pane opens" "$(fld spaces.0.root.b.url)" "https://home.example"
w config unset browser.homepage --space s1 >/dev/null
w pane kind "$Q" empty >/dev/null

expect_exit 0 "--page opens exactly that" w pane kind "$P" browser --page https://example.com/one
eq "...space ls reports it" "$(fld spaces.0.root.a.url)" "https://example.com/one"
eq "pane page moves it" "$(w pane page "$P" 'https://example.com/two?q=1#x')" "https://example.com/two?q=1#x"
eq "...space ls reports the new page" "$(fld spaces.0.root.a.url)" "https://example.com/two?q=1#x"
eq "typed example.com reads as https://example.com" "$(w pane page "$P" example.com)" "https://example.com"
eq "typed localhost:3000/x keeps its port" "$(w pane page "$P" localhost:3000/x)" "https://localhost:3000/x"
eq "typed /tmp/a b.html is a file URL" "$(w pane page "$P" '/tmp/a b.html')" "file:///tmp/a%20b.html"
eq "a space after the host is %20" "$(w pane page "$P" 'example.com/a b')" "https://example.com/a%20b"
eq "typing the page it is already on still replies" "$(w pane page "$P" 'https://example.com/a%20b')" "https://example.com/a%20b"
for t in 'mailto:a@b.com' 'ftp://example.com' 'file:/tmp/a.html'; do
  expect_exit_out 6 "isn't a web address or a file path" "typed '$t' is refused" w pane page "$P" "$t"
done

# one rule on every path: opening and typed text
for u in 'file://otherhost/etc/passwd' 'file:relative.html' 'data:text/html,<p>x' 'javascript:alert(1)' 'https://' 'about:config' 'http://:80' 'https://@/' 'http://user@' 'https://a b/c'; do
  expect_exit 6 "typed '$u' is refused" w pane page "$P" "$u"
  expect_exit 6 "--page '$u' is refused" w pane kind "$P" browser --page "$u"
done
eq "after every refusal the pane is where it was" "$(url_of "$P")" "https://example.com/a%20b"
expect_exit_out 6 'http:// or https:// URL with a host' "--page example.com names the accepted forms" w pane kind "$P" browser --page example.com
expect_exit 0 "a scheme is matched without case" w pane kind "$P" browser --page 'HTTPS://Example.com'
expect_exit 0 "a file:/// URL opens" w pane kind "$P" browser --page file:///tmp/notes/index.html
eq "pane page moves to a file:/// URL" "$(w pane page "$P" file:///tmp/notes/two.html)" "file:///tmp/notes/two.html"

# fields belong to their own kinds
T="$(w pane split "$P" down)"
w pane kind "$T" terminal >/dev/null
expect_exit_out 6 'url is only for a browser pane' "--page on a terminal exits 6" w pane kind "$T" terminal --page https://example.com
expect_exit_out 6 'only for a web pane' "--artifact on a browser exits 6" w pane kind "$P" browser --artifact site
expect_exit_out 6 'is not a browser pane' "pane page on a terminal exits 6" w pane page "$T" example.com
expect_exit 6 "pane page on a missing pane exits 6" w pane page p999 example.com

# nothing of the artifact machinery
w pane kind "$P" browser --page https://example.com >/dev/null
eq "no watches row" "$(w status | jget watches)" "0"
eq "no refreshes row" "$(w status | jget refreshes)" "0"
check "no pane data directory" test ! -e "$INST/panes/$P"
eq "/artifacts/<p>/index.html?token= is 404" "$(curl -s -o /dev/null -w '%{http_code}' "$(BASE)/artifacts/$P/index.html?token=$(TOKEN)")" "404"

# a pane that was a web pane gives everything up when it becomes a browser
mkdir -p "$PROJ/site"; printf '<html>s</html>\n' >"$PROJ/site/index.html"; printf '[artifacts.site]\npath = "site"\n' >"$PROJ/wheel.toml"
w pane kind "$T" web --artifact site >/dev/null
eq "the web pane has two watches rows" "$(w status | jget watches)" "2"
check "...and a data directory" test -d "$INST/panes/$T"
w pane kind "$T" browser --page https://example.com >/dev/null
eq "as a browser it has no watches rows" "$(w status | jget watches)" "0"
check "...and no data directory" test ! -e "$INST/panes/$T"

# twenty navigations: the last is reported at once and the file is written once
SAVES0="$(w status | jget layout_saves)"
PIDS=(); for i in $(seq 1 19); do w pane page "$P" "https://example.com/$i" >/dev/null & PIDS+=($!); done; wait "${PIDS[@]}"; w pane page "$P" https://example.com/20 >/dev/null
eq "space ls reports the twentieth page at once" "$(url_of "$P")" "https://example.com/20"
saved() { grep -q 'example.com/20' "$STATE"; }
poll 10 saved
check "the file holds example.com/20" grep -q 'example.com/20' "$STATE"
check "...and no earlier page" bash -c "! grep -q 'example.com/19' '$STATE'"
SAVES1="$(w status | jget layout_saves)"
check "layout_saves rose by at most 2 for a burst of 20 (one write per burst, not per page): rose by $((SAVES1 - SAVES0))" test "$((SAVES1 - SAVES0))" -le 2
has_log "browser pane navigated.*from=.*to=" "log: browser pane navigated with from and to"

# a split still writes at once
w pane split "$P" right >/dev/null
SAVES2="$(w status | jget layout_saves)"
check "another change writes the file at once" test "$SAVES2" -gt "$SAVES1"

# SIGTERM flushes a page navigated to in the last moment; the page survives a restart
w pane page "$P" https://example.com/last >/dev/null
restart_daemon
eq "after a restart the pane is on the same page" "$(url_of "$P")" "https://example.com/last"
expect_exit 0 "pane kind empty leaves an empty pane" w pane kind "$P" empty
eq "...it is empty" "$(w space ls | grep -A2 "\"id\": \"$P\"" | grep -c '"kind": "empty"')" "1"

# the file is held to the same rule as a caller
stop_daemon
rm -f "$STATE"
start_daemon --no-lead --state-file "$STATE"
w space new "$PROJ" >/dev/null
BP="$(w pane ls | awk '{print $2}')"
w pane kind "$BP" browser --page https://example.com >/dev/null
stop_daemon
sed -i '' 's|"url": "https://example.com"|"url": "javascript:alert(1)", "zoom": 1.5|' "$STATE"
start_daemon --no-lead --state-file "$STATE"
eq "a saved javascript: browser pane is kept whole, as kind browser" "$(w space ls | jget spaces.0.root.kind)" "browser"
eq "...with the url unchanged (never read as a page)" "$(w space ls | jget spaces.0.root.url)" "javascript:alert(1)"
has_log "unknown pane kind; kept as is.*kind=browser" "log: unknown pane kind kept as is (browser)"
stop_daemon
sed -i '' 's|"url": "javascript:alert(1)"|"url": "https://example.com"|' "$STATE"
start_daemon --no-lead --state-file "$STATE"
w pane page "$BP" https://example.com/next >/dev/null
check "a field this build does not read survives navigation" bash -c "sleep 1; grep -q '\"zoom\": 1.5' '$STATE'"
w pane kind "$BP" terminal >/dev/null
sleep 1
check "changing the kind drops it" bash -c "! grep -q zoom '$STATE'"

finish
