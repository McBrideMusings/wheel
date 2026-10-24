#!/usr/bin/env bash
# § 4 Keys (wheel-e19f): the Ghostty-style keymap, the empty pane's keys, the confirmation sheet
# and the `keybind` setting. Every key goes in as a key event through the app's event path, so the
# menus' key equivalents and the first responder (a terminal included) see what a keyboard sends.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname "${BASH_SOURCE[0]}")/mac-lib.sh"
trap 'app_cleanup; cleanup' EXIT

use_fake_agent
start_daemon --state-file "$SCR/spaces.json"
prefs_reset
mkdir -p "$SCR/alpha" "$SCR/beta"
A="$(w space new "$SCR/alpha")" B="$(w space new "$SCR/beta")"
launch_app
wait_st '.windows | length == 1 and (.windows[0].spaces | length == 2)' 20

# press <keys>: one key as wheeld spells it (shift+cmd+d, t).
press() { app_post "/key?keys=$(printf %s "$1" | sed 's/+/%2B/g')" >/dev/null; }
focus_of() { st --arg s "$1" '.spaces[] | select(.id==$s) | .focus'; }
panes_of() { st --arg s "$1" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane") | .id] | join(",")'; }
kind_of() { st --arg p "$1" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind'; }
term_of() { st --arg p "$1" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].term'; }

# The menus carry the Ghostty keys, each once.
MENUS="$(st '.menus | join("\n")')"
for item in "File/New Space… ⌘T" "File/Close Pane ⌘W" "File/Close Space ⌥⌘W" "File/Close Window ⇧⌘W" \
  "Pane/Split Right ⌘D" "Pane/Split Down ⇧⌘D" "Pane/Previous Pane ⌘[" "Pane/Pane Left ⌥⌘←" \
  "Pane/Move Divider Right ⌃⌘→" "Pane/Equalize Panes ⌃⌘=" "Pane/Zoom Pane ⇧⌘↩" "Space/Next Space ⇧⌘]"; do
  check "menu: $item" grep -qF -- "$item" <<<"$MENUS"
done
eq "no two menu items share a key" "$(sed -E 's/ \(disabled\)$//; s/.* ([^ ]+)$/\1/' <<<"$MENUS" | sort | uniq -d | tr '\n' ' ')" ""

app_post "/focus?space=$A" >/dev/null
wait_st --arg s "$A" '.active == $s'
P1="$(focus_of "$A")"

# ⌘D from an empty pane: a new pane to the right holds the focus and the keyboard; T makes it a terminal.
press cmd+d
wait_st --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane")] | length == 2'
P2="$(focus_of "$A")"
check "⌘D splits and focuses the new pane" test "$P2" != "$P1"
wait_st '.first_responder | test("terminal") | not'
press t
wait_st --arg p "$P2" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind == "terminal"' 10
eq "T in the focused empty pane starts a terminal" "$(kind_of "$P2")" "terminal"
T2="$(term_of "$P2")"
wait_st --arg t "terminal $T2" '.first_responder == $t' 10
eq "the terminal holds the keyboard" "$(st .first_responder)" "terminal $T2"

# ⇧⌘D with the terminal holding the keyboard: the menu gets it, not the shell.
press shift+cmd+d
wait_st --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane")] | length == 3'
P3="$(focus_of "$A")"
eq "⇧⌘D from a terminal splits it downward" "$(st --arg s "$A" '.spaces[] | select(.id==$s) | .root.b.axis')" "vertical"
wait_st '.first_responder | test("terminal") | not' 5
eq "the terminal gives the keyboard to the new empty pane" "$(st '.first_responder | test("terminal")')" "false"
press 1
wait_st --arg p "$P3" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind == "agent"' 15
eq "1 starts the first agent" "$(kind_of "$P3")" "agent"

# Moving between panes: ⌘[ ⌘] in tree order, ⌥⌘arrows as drawn.
press cmd+bracket_left
wait_st --arg s "$A" --arg p "$P2" '.spaces[] | select(.id==$s) | .focus == $p'
eq "⌘[ focuses the pane before" "$(focus_of "$A")" "$P2"
press cmd+bracket_right
wait_st --arg s "$A" --arg p "$P3" '.spaces[] | select(.id==$s) | .focus == $p'
eq "⌘] focuses the pane after" "$(focus_of "$A")" "$P3"
press opt+cmd+arrow_left
wait_st --arg s "$A" --arg p "$P1" '.spaces[] | select(.id==$s) | .focus == $p'
eq "⌥⌘← focuses the pane to the left" "$(focus_of "$A")" "$P1"
press opt+cmd+arrow_up
eq "⌥⌘↑ with nothing above leaves the focus" "$(focus_of "$A")" "$P1"

# Dividers and zoom.
press ctrl+cmd+arrow_right
wait_st --arg s "$A" '.spaces[] | select(.id==$s) | .root.ratio > 0.54'
eq "⌃⌘→ moves the divider right by 5%" "$(st --arg s "$A" '.spaces[] | select(.id==$s) | .root.ratio * 100 | round')" "55"
press ctrl+cmd+equal
wait_st --arg s "$A" '.spaces[] | select(.id==$s) | .root.ratio == 0.5'
eq "⌃⌘= equalizes" "$(st --arg s "$A" '.spaces[] | select(.id==$s) | .root.ratio')" "0.5"
press shift+cmd+enter
eq "⇧⌘↩ zooms the focused pane" "$(st .zoomed)" "$P1"
press shift+cmd+enter
eq "and again unzooms it" "$(st .zoomed)" "null"

# Spaces.
press cmd+2
wait_st --arg s "$B" '.active == $s'
eq "⌘2 shows the second space" "$(st .active)" "$B"
press shift+cmd+bracket_left
wait_st --arg s "$A" '.active == $s'
eq "⇧⌘[ shows the space before" "$(st .active)" "$A"
press cmd+9
wait_st --arg s "$B" '.active == $s'
eq "⌘9 shows the last space" "$(st .active)" "$B"
press cmd+1
wait_st --arg s "$A" '.active == $s'

# ⌘W on a terminal asks; Esc keeps it, Return closes it.
app_post "/focus?pane=$P2" >/dev/null
wait_st --arg s "$A" --arg p "$P2" '.spaces[] | select(.id==$s) | .focus == $p'
press cmd+w
wait_st '.confirmation != null'
eq "⌘W on a terminal asks first" "$(st .confirmation.title)" "Kill this pane's shell?"
sleep 0.5
app_post "/key?which=sheet&keys=escape" >/dev/null
wait_st '.confirmation == null'
eq "Esc in the sheet keeps the pane" "$(kind_of "$P2")" "terminal"
press cmd+w
wait_st '.confirmation != null'
sleep 0.5
app_post "/key?which=sheet&keys=enter" >/dev/null
wait_st --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane")] | length == 2'
eq "Return in the sheet closes it" "$(panes_of "$A")" "$P1,$P3"

# Setting a pane's kind from the palette's actions: an empty pane changes at once.
app_post "/focus?pane=$P1" >/dev/null
wait_st --arg s "$A" --arg p "$P1" '.spaces[] | select(.id==$s) | .focus == $p'
app_post "/action?id=pane-browser" >/dev/null
wait_st --arg p "$P1" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind == "browser"'
eq "Show a Browser on an empty pane needs no yes" "$(kind_of "$P1")" "browser"
app_post "/focus?pane=$P3" >/dev/null
wait_st --arg s "$A" --arg p "$P3" '.spaces[] | select(.id==$s) | .focus == $p'
app_post "/action?id=pane-terminal" >/dev/null
wait_st '.confirmation != null'
eq "Show a Terminal on an agent pane asks first" "$(st .confirmation.title)" "End this pane's agent session?"
app_post "/confirm?answer=yes" >/dev/null
wait_st --arg p "$P3" '[.spaces[].root | .. | objects | select(.node=="pane" and .id==$p)][0].kind == "terminal"'
eq "and yes replaces it" "$(kind_of "$P3")" "terminal"

# ⌘T opens New Space.
press cmd+t
wait_st '.palette.open == true'
eq "⌘T opens the palette in New Space mode" "$(st .palette.mode)" "new_space"
app_post "/palette?action=close" >/dev/null

# The keybind setting: wheeld stores one spelling, the menus follow, a bad action is reported.
expect_exit_out 6 "not a modifier" "a malformed binding is refused" w config set keybind "hyper+k=split-right" --machine
expect_exit 0 "keybind takes Ghostty's spelling" w config set keybind "Super+K=split_right, cmd+d=unbind" --machine
eq "wheeld stores it in its own spelling" "$(w config get keybind)" "cmd+k=split-right,cmd+d=unbind"
wait_st '.keymap["split-right"] == "cmd+k"'
eq "the keymap follows" "$(st '.keymap["split-right"]')" "cmd+k"
check "the menu shows ⌘K" grep -qF "Pane/Split Right ⌘K" <<<"$(st '.menus | join("\n")')"
BEFORE="$(panes_of "$A")"
# With ⌘D unbound, a terminal holding the keyboard is not given ⌘D by libghostty's own default
# bindings (its split key): no view takes it. ⌘C and ⌘V stay bound on the surface for copy and paste.
APPLOG="$(st .log_path)"
app_post "/focus?pane=$P1" >/dev/null
wait_st --arg s "$A" --arg p "$P1" '.spaces[] | select(.id==$s) | .focus == $p'
app_post "/focus?pane=$P3" >/dev/null
wait_st '.first_responder | test("terminal")' 10
eq "a focused terminal holds the keyboard" "$(st '.first_responder | test("terminal")')" "true"
last_taker() { grep "debug: key $1 " "$APPLOG" | tail -1 | sed 's/.*taken by //'; }
press cmd+d
eq "unbound ⌘D in a terminal is not taken by a view's key equivalent" "$(last_taker cmd+d)" "nothing (the menus are left out)"
press cmd+c
eq "⌘C in a terminal is still the surface's copy" "$(last_taker cmd+c)" "a view's key equivalent"
press cmd+v
eq "⌘V in a terminal is still the surface's paste" "$(last_taker cmd+v)" "a view's key equivalent"
press cmd+k
wait_st --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane")] | length == 3'
eq "⌘K splits and the unbound ⌘D did nothing" "$(st --arg s "$A" '[.spaces[] | select(.id==$s) | .root | .. | objects | select(.node=="pane")] | length')" "3"
expect_exit 0 "an action the app lacks is accepted by wheeld" w config set keybind "cmd+j=split-sideways" --machine
wait_st '.keymap_problems | length == 1'
eq "and reported by the app" "$(st '.keymap_problems[0]')" "cmd+j=split-sideways: Wheel has no action split-sideways"
expect_exit 0 "unset puts the built-in keys back" w config unset keybind --machine
wait_st '.keymap["split-right"] == "cmd+d"'
eq "⌘D is Split Right again" "$(st '.keymap["split-right"]')" "cmd+d"

APPLOG="$(st .log_path)"
check "the app log has each key's action" grep -q "action split-right: run" "$APPLOG"
check "the app log has the keymap the setting made" grep -q "keymap problem: cmd+j=split-sideways" "$APPLOG"

# ⇧⌘W closes the window; its spaces run on in the background.
press shift+cmd+w
wait_st '.windows | length == 0'
eq "⇧⌘W closes the window and its spaces go to the background" "$(st '.background | length')" "2"
finish
