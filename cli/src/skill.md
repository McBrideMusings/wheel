# Driving Wheel with the `wheel` command

Wheel holds a person's projects as **spaces**. Each space has a directory and a grid of **panes**:
terminals, agent chats, web panes (an artifact: a folder with `artifact.toml` and `index.html`) and
browsers. A daemon (`wheeld`) holds all of it; `wheel` is the command that drives it. Every command
prints to stdout and exits 0 on success; a failure prints `wheel: <why>` to stderr and exits
non-zero (3 for a timeout, 4 when a terminal exited first).

## Where you are

Inside a Wheel terminal or agent pane these variables are set, so no flag is needed:

- `WHEEL_URL` and `WHEEL_TOKEN_FILE`: how `wheel` reaches the daemon.
- No variable names your space or its directory: a command that defaults to "this space" asks the daemon which space holds `WHEEL_PANE` at that moment, so a pane moved to another space acts on the new one. `--space` overrides it. `wheel space ls --json` gives each space's `cwd` and its `panes`, so the entry holding `$WHEEL_PANE` is your space's directory.
- `WHEEL_PANE`: the id of your pane. Closing it ends you.

Outside Wheel, pass `--url ws://127.0.0.1:7420/ws --token-file <file>`, or run
`wheel install` once to link this command into `~/.local/bin`.

`wheel status` prints the whole state as JSON: spaces, the layout of each, terminals, agents
and waiting permission requests. Start there.

## Commands

Ids: spaces look like `s1`, panes `p3`, terminals `t2`, agents `w4`. Where a command takes a
terminal or agent, a pane id showing it works too. `wheel <command> --help` lists every flag.

- `wheel space ls [--json] | new <dir> | focus <id> | close <id> | rename <space> <name>`: the projects. `space new` adds a tab to the focused Mac window and shows nothing new; `space new --focus` shows the new space, and `space new --background` makes a space with no window: the Mac app keeps it in the background and closes it once its last pane closes. `pane split`, `pane open`, `pane run`, `run <name>` and `pane move --new-space` take no focus either; `--focus` asks every client to show the new pane and its space, which an agent passes only when the person asked to be shown something. `wheel space focus <id>` and `wheel pane focus <pane>` do the same: wheeld keeps no active space or focused pane, each client keeps its own view. `wheel client ls [--json]` prints every attached client and, for each of its windows, the session, space and pane it shows. A command with no `--space` takes the space of the pane that runs it (`$WHEEL_PANE`), and outside a pane needs `--space`. A space is named by id or by name. `wheel space ls --json` prints `[{id, name, cwd, status, panes}]`, `panes` being pane ids. `wheel space status` prints each space's status (`needs_you` with its waiting count, `stopped`, `working` or `idle`) and one indented line per pane; `wheel space ls` carries the same `status` fields. `wheel space recent [--json]` lists the directories spaces were made for, newest first. `wheel space browse [path] [--json]` lists the subfolders of a folder on the daemon's machine (`~` is the daemon user's home) and marks any that is open as a space with `(space <id>)`; with no path it lists each root from the `spaces.roots` setting. A missing folder exits non-zero with `no folder at <path>`.
- `wheel session ls [--json] | new <name> | rename <session> <name> | close <session> | reorder <session> <index> | move <space> <session> [--index N]`: sessions, the named groups of spaces; every space is in exactly one. `session close` closes its spaces and their panes. A space made with `space new` from a pane joins that pane's session (`$WHEEL_PANE`); `--session <id|name>` names one and wins, else the first session. A session is named by id or name.
- `wheel window ls [--json] | new [--name N] [--space S] | rename <window> <name> | close <window>` and `wheel space move <space> <window|background>`: the Mac app's windows. A space is in one window or in the background, where it keeps running. They need the Mac app; without it they fail with `no Mac app attached`.
- `wheel pane ls`: the panes of a space, each with its `name` (saved) and `title` (set by its program) when it has them; `status --json` gives them as `pane_name` and `pane_title`. `wheel pane rename <pane> [<name>]` sets a pane's name, or clears it with no name; clients show the name, then the title, then the kind. `wheel pane near [pane]`: the panes beside one (default `$WHEEL_PANE`), one line each: side, id, kind, what it shows. `pane split`, `pane swap`, `pane move`, `pane close`,
  `pane focus`, `pane ratio`: arrange the grid. A pane is in the grid, popped out (a floating window over the grid) or backgrounded (running, shown nowhere); `pane ls`, `space ls --json` and `status --json` report it as `site` (`grid`, `popped` or `backgrounded`). `pane pop <pane>` toggles grid ↔ popped out and `pane background <pane>` toggles grid ↔ backgrounded (a pane that is not in the grid goes back to it, where `panes.placement` puts it); the pane keeps its id, process and scrollback. `pane place <pane> --frame x,y,w,h` moves and sizes a popped-out pane's floating window, in points from the top left of the space's body; every client moves it there, a person's drag sets it too, it is kept while the pane visits the grid, and `pane ls --json` and `status --json` give each popped pane's `frame`. A pane in the grid or the tray is refused. `pane split`, `pane open` and `pane run` take `--pop` or `--background` (not both, and not with a side) to make the new pane that way: `wheel pane run "$WHEEL_PANE" "claude …" --background` starts a worker nobody has to look at, and `wheel pane pop <worker>` shows it. Against a server older than this, they fail with `restart the server`. `pane move <pane> <target> <side>` goes beside a pane in any space; `pane move <pane> --end-of <space>` and `pane move <pane> --new-space [--name N] [--focus]` go to the end of a space or into a new one after its own. The pane keeps its id and process; a space left empty gets one empty pane, and a target space holding only an empty pane loses it.
- `wheel pane mirror <source> --space <space>` makes a mirror: a pane that draws another pane's live content (its terminal, its agent's chat, its page) and prints its id. Typing, sending and answering a permission in it act on the source; `wheel term read`, `wheel term send` and `wheel agent send` take a mirror's id for its source's. It holds only the source's id: `pane ls` shows its kind as `mirror:<source>` and `pane ls --json` and `status --json` carry `source`. It takes the space's lone empty pane, else splits the space's last pane. The source may be in any space of any session but not itself a mirror; when the source closes, the mirror closes.
- `wheel pane push <pane> on|off` turns a pane's own pushes to the phone (needs you, stopped, a long turn done) on or off; the setting stays with the pane through moves and kind changes, `status --json` gives a quiet pane `quiet: true`, and `wheel push send --pane <pane>` still sends.
- `wheel pane start <pane> agent [--agent N] [--model M] [--message T] [--dir D]`: make a pane an agent (or another kind) and start it. `--dir D` (an absolute path to an existing directory, such as a git worktree) runs the agent there instead of in the space's directory; the pane keeps it across restarts, and `agent conversations` and `agent watch`'s `cwd` use it. To hand work to another agent, start it as an agent pane, not a terminal running `claude`: `wheel pane split $WHEEL_PANE right --background` prints a new pane id, `wheel pane start <new> agent --dir <worktree> --message "<task>"` starts the worker there, `wheel agent send <new> "<text>"` messages it and `wheel agent wait <new>` / `agent read <new>` follow it. Never type into a terminal pane with `term send` to message an agent. `--model` hands the agent its model (`sonnet`, a full id such as `claude-sonnet-5-5`, a Codex model id); a name Claude or Codex does not know fails the command with the agent's error (a full Claude id or a pi model is refused at the first turn instead). `wheel status` lists the model each agent pane runs as `model` on the pane.
  `wheel pane kind <pane> <kind>` changes what a pane shows; `pane kind <pane> browser --page <url> [--home <url>]` (and `pane open <pane> --page <url> [--home <url>]`) opens a browser pane at the page, which is its home unless `--home` names another (on a pane that is already a browser pane it sets the home and moves neither browser: `pane page` moves one); `pane home <pane> [<url>]` prints a browser pane's home or sets it (neither the Mac's nor the phone's browser moves; both open at the home when they are made); `pane page <pane> <url>` sends a browser to a page in
  a browser pane; `pane shot <pane> <file.png>` saves a picture of a pane (`--full` makes it one tall picture of a web or artifact page at its full height, or of an agent chat's whole conversation, with no scrolling by you; a pane that fits is unchanged; a terminal refuses, read its scrollback with `term read`); `pane console <pane>` prints a browser
  pane's console messages and uncaught errors since its last page load (`--follow`, `--since <unix ms>`, `--json`, `--on mac|phone`: the Mac's copy of the page and the phone's own keep separate lists, and the default reads the Mac's).
  `pane eval <pane> <js>` runs JavaScript in a browser pane's page and prints the result as JSON (a thrown error prints its message and exits non-zero); `pane dom <pane> [selector]` prints the page's HTML, or the outer HTML of every match (no match fails); `pane shot <pane> <file.png> --selector <css>` saves a picture of the first matching element only; `pane record <pane> <file.mov> --seconds N` (1 to 60) films a browser pane from the web view's own snapshots and prints the absolute path, the frames written and the duration (the picture is the page without the pane's header; no Screen Recording permission). They read the page's main frame only (not iframes), and all four fail on a pane that is not a browser pane. `pane eval`, `dom`, `shot` and `record` take `--on mac|phone` (default Mac): the Mac and the phone each run their own browser on the pane, with their own page, history and logins, and `--on phone` runs in the phone's own web view (a Mac loopback page such as `localhost:<port>` loads on the phone through wheeld) or fails with `the phone is not attached` while the app is suspended. `pane page <pane> <url> [--on mac|phone]` sends that one browser to the page. `pane where <pane> [--on mac|phone]` prints that one browser's page and title as `<url><TAB><title>` (the browser answers within 3 s, else `no answer within 3s`; `--on phone` fails `the phone is not attached` while the app is suspended); `pane info <pane>` on a browser pane lists the home, then a `Mac` field and a `Phone <name>` field per attached phone with each one's page and title (`not attached` for a Mac app that is not connected). No browser reports its page as it moves: wheeld only asks.
- `wheel pane reload <pane>` starts a pane again on what it is on: an agent pane's agent restarts on the same conversation (refused while a turn runs), a terminal's shell starts again in the directory it was in and resumes the Claude Code, Codex or pi conversation it ran, or types again the command `pane run` or `wheel run` started it with while that command still runs, a web pane reloads and runs its artifact's refresh, a browser reloads the page each browser is on, a plugin pane runs its script again from the top, and an empty pane is left as it is.
- When `wheeld` restarts, every pane comes back under its id. A terminal pane's new shell starts in the directory it was last in; it resumes the Claude Code, Codex or pi conversation it ran, or, for a pane `pane run` or `wheel run` made, types its command again when that command was still running (a command that ended, and a terminal someone typed into, come back as a plain shell). An agent, in an agent pane or resumed in a terminal, that was in a turn when `wheeld` stopped gets one message first: `wheeld restarted at <time> UTC`, the turn ended and every process it ran stopped, so check what it was waiting on, start again what it still needs, and continue.
  - `wheel pane reload <pane> --env CLAUDE_CONFIG_DIR=~/.claude-work` starts an agent or terminal pane's new process with that value and keeps it for the pane's later starts (repeat `--env` for more); `--unset-env VAR` drops a kept value so the login shell decides it again. A changed agent home first copies the conversation's file into the new home, and refuses, naming both files, when the one there has turns the pane's lacks. Other kinds of pane refuse both flags.
- `wheel term open | send | read | close`: terminals.
  - `wheel term send <pane> "ls" --enter` types text and presses Return. It sends raw keystrokes; to message an agent, Claude Code, Codex or pi running in a terminal pane included, use `wheel agent send`.
  - `wheel term send <pane> --key up --key enter` presses named keys: `enter`, `tab`, `esc`, `space`,
    `backspace`, `delete`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`,
    `ctrl-a` to `ctrl-z`, `f1` to `f12`, or one character. Use it to answer a menu or choice a
    program shows in a terminal (`--key down --key enter`, `--key 1`).
  - `wheel term read <pane> --until "text" --timeout 30` prints the screen, then live output until
    the text appears (exit 3 if it never does).
- `wheel agent send <pane> "message"`: send a chat agent a message and print its reply as it ends. This is how one agent messages another: it needs no keystrokes and works on a backgrounded pane. It also reaches Claude Code, Codex or pi running in a terminal pane: wheeld pastes the message into its composer and submits it, queued behind a running turn, with no key to press; it returns once the message is in, the reply shows in that pane, `wheel agent wait <pane>` waits until the agent's hooks report it idle and `wheel term read <pane>` shows its screen. It refuses while that agent waits on a permission prompt, which typed text would answer.
  `agent read`, `agent wait`, `agent cancel`, `agent conversations`, `agent new`, `agent resume`,
  `agent context`, `agent config`: follow and steer a chat agent. `wheel agent watch` prints every agent pane as one JSON line (`space`, `cwd`, `pane`, `agent`, `harness`, `model`, `status`: idle, working, needs_you or stopped), then again whenever the list changes, until the daemon goes away.
- `wheel agent card --pane <id>` or `wheel agent card --artifact <name>` puts a card in your own transcript (`--agent <pane|name>` names another agent's): the client draws that pane or artifact live at that point in the conversation, so you show the person a running pane or a page instead of describing it. An artifact is looked for in your space's project, then the machine's. `wheel agent read` prints it as `[card] pane <id>` or `[card] artifact <name> (<scope>)`.
- The orchestrator is an ordinary agent pane in a space whose directory is the orchestrator folder: the machine's `orchestrator.dir` setting, or `orchestrator` beside the machine's wheel.toml when it is empty. Orchestrator mode on the Mac and the phone shows that space; open it with `wheel space new <folder>` and `wheel pane start <pane> agent`. Agent events reach it only through `[[hook]]` commands.
- `wheel perm list | wait | answer`: permission requests agents are waiting on.
  `wheel perm answer <request-id-or-agent-pane> allow|deny|<option-id>` answers one. **Only answer a
  request when the person has told you to.** Never answer your own. wheeld starts every Claude Code
  agent with `wheel perm answer` denied, except in a folder whose own `.claude/settings.json` or
  `settings.local.json` allows `Bash(wheel perm answer…)` by name.
- `wheel artifact new | add | remove | ls | refresh | show`: web-pane artifacts. A refresh
  command in `artifact.toml` runs without asking.
  `wheel artifact state <pane> [<key>] [--wait]` prints what the page stored with `wheel.set(key, value)`
  (`--wait` blocks until the key is written: post a question, wait for the answer);
  `wheel artifact data <pane> <file|->` pushes a JSON value into the running page (its
  `wheel.onData(fn)`), no reload. A page loads the helper with `<script src=".wheel/client.js">`.
- `wheel artifact app status | logs | start | stop | restart <pane>`: the process of an app artifact, an
  entry with `app = "<command>"` (and `restart = "on-failure" | "always" | "never"`) in a wheel.toml.
  The daemon starts the command when a web pane shows the artifact, restarts it by the policy, and
  ends it and its children with the pane. The program announces its port by writing it as a line to
  file descriptor 3 (`$WHEEL_APP_PORT_FD`); `status` prints the pid and that port, `logs` the output of
  the current run (`--run N`, `--tail N`).
- `wheel config ls | get | set | unset | watch`: settings, for the machine or one space.
- `wheel guidance show | path [--agent <name>]`: the text an agent is given and the path of the person's own guidance file for every agent or one agent (made empty if missing).
- `wheel run [<name>]`: the `[runner]` table of wheel.toml maps names you choose to shell commands
  (`build = "make -j8"`); a project's table overrides the machine's. A table with no `command` is a
  group: `[runner.prototypes.terrarium.dev]` with `command`, an optional `name` (what the Runner page
  shows) and `desc` (one sentence) is the command `prototypes.terrarium.dev` under the groups
  `prototypes` and `terrarium`. `wheel run` lists them,
  `wheel run <name>` types the command into a new terminal pane in the space's directory and prints the pane.
  The `runner` built-in shows them as buttons (`wheel artifact builtin install runner`, then
  `wheel pane kind <pane> web --artifact runner --scope machine`), and the command palette lists
  each as `Run <name>`.
- `[[hook]]` in wheel.toml runs a shell command on agent events: `on` names any of `turn_end`, `fail`,
  `permission`, `answered`, `over_time` (a turn past 30 minutes) and `build_failed`; `run` is the command,
  run in the folder of the wheel.toml naming it, with the event's JSON (`event`, `agent`, `pane`, `space`,
  `text`, `at_ms`, `perm`, and `request` for a permission) on stdin. The machine's hooks take every
  agent's events, a project's those of its spaces. `inbox = "<dir>"` first writes each event as one
  Maildir file there (`tmp/`, then `new/`) and passes its path as `$1`. With `decide = true` (taken only from the
  machine's wheel.toml), a `permission` hook's first printed word `allow` or `deny` answers the request; anything else leaves it
  to the person. wheeld reads the files at each event and logs each run.
- `wheel work set [--item <bead|issue|pr>:<id>]... [--stage plan|edit|build|verify|review|gate] [--note T] [--pane P]` declares what the agent in a pane is working on (`--pane` defaults to `$WHEEL_PANE`); each call replaces the last. `wheel work clear` drops it. `wheel status --json` reports it as `work` on the pane, and it goes when the pane closes or changes kind.
- `wheel usage`: Claude account usage.
- `wheel notify "text" [--level notice|warning|error] [--id <id>] [--pane <pane>]`: show a toast (green, orange or red) on every Mac client attached now (the phone shows none), and print its id. The text can be selected and copied; `--pane` adds Focus pane and Close pane buttons. The same `--id` replaces the toast; `wheel notify --clear <id>` takes it down. The daemon keeps nothing, so a client that attaches later does not see it. Use it for what the person should read now, not for progress.
- `wheel remote info`: the remote listener's addresses, port and certificate fingerprint (the `remote.listen` setting turns it on).

## Showing a document or a result with pictures: the `markdown` built-in

A post is a folder of data that another artifact's page draws. `markdown` is the page for a
document: write what you would write in chat, as a markdown file named `index.md`, and show it.
It draws CommonMark with GitHub's tables, task lists and strikethrough. It collects no answer:
to ask the person something, ask in chat. When you have a new turn, change that same post and
push it. Never make a second artifact for the same conversation. Install the page once per machine
with `wheel artifact builtin install markdown` (`wheel artifact builtin ls` says whether it is
there).

```sh
wheel artifact new notes --renderer markdown index.md   # the post: index.md plus every local image it points at
P=$(wheel pane split "$WHEEL_PANE" right)               # a pane for it
wheel pane kind "$P" web --artifact notes
# next turn: edit index.md, then push it; the open page changes with no reload
wheel artifact data "$P" index.md
```

`--renderer markdown` names the page that draws the post; `notes` is the post's own name. A post
made with `--ends-with-agent` or `--expires 2h` deletes itself; without them it stays.

An image path is a file beside `index.md` (or below it); `wheel artifact new` copies each one into
the post and stops with the file name if one is missing. A link to a file beside `index.md` is
copied too and opens from the post; an `http(s)` link, or a bare `http(s)` address, opens in the
default browser. Other schemes and paths outside the folder are not links, and raw HTML is shown as
text. A click on any image opens a spotlight (click again to zoom, drag to pan, Esc to close, arrow
keys step between images).

## Showing a mockup, and getting a pick: the `mockup` built-in

`mockup` draws a page built by the `spike` tool (`spike build --kind mockup`): one frame, with a tab
for each variant, each state, light and dark, and a width (phone, tablet, desktop). The person picks
a variant and Send tells the agent which one. Name the built page `mockup.html`:

```bash
wheel artifact builtin install mockup            # once per machine
wheel artifact new look --renderer mockup --reply-to "$WHEEL_PANE" mockup.html
wheel pane split "$WHEEL_PANE" right             # prints the new pane, then:
wheel pane kind <pane> web --artifact look
wheel artifact state <pane> submitted --wait     # {"answers":{"pick":"Strip"},"note":…,"summary":"Picked Strip: …"}
```

The post holds only `mockup.html`; rebuild it with `spike build` and the pane reloads. Close the pane
once the pick arrives.

Choose the widths the design is for and name them on any element, `<div hidden data-mockup-widths="390 1280"></div>`:
the page then offers Fit and those widths. Name none for something that has one size (a popover, a menu),
and there is no Width control. Never offer all widths by default.

Build it as a **prototype** (`spike build --kind prototype`, one `<template data-variant>` per option and
a top-level `<script>`) when the person has to try it: hover, click, right-click and drag all run live
in the frame, and the Option tabs switch between the variants. A static `--kind mockup` page shows
screens only; use it for looks, never for behaviour.

## Rules

- Leave panes you did not make alone unless the person asks.
- Ask the person before closing a space or a pane that holds work.
- Prefer `wheel status` and `--json` flags (where a command has one) over parsing human text.
