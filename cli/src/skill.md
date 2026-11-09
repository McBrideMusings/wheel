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
- `wheel pane ls`: the panes of a space. `wheel pane near [pane]`: the panes beside one (default `$WHEEL_PANE`), one line each: side, id, kind, what it shows. `pane split`, `pane swap`, `pane move`, `pane close`,
  `pane focus`, `pane ratio`: arrange the grid. A pane is in the grid, popped out (a floating window over the grid) or backgrounded (running, shown nowhere); `pane ls`, `space ls --json` and `status --json` report it as `site` (`grid`, `popped` or `backgrounded`). `pane pop <pane>` toggles grid ↔ popped out and `pane background <pane>` toggles grid ↔ backgrounded (a pane that is not in the grid goes back to it, where `panes.placement` puts it); the pane keeps its id, process and scrollback. `pane split`, `pane open` and `pane run` take `--pop` or `--background` (not both, and not with a side) to make the new pane that way: `wheel pane run "$WHEEL_PANE" "claude …" --background` starts a worker nobody has to look at, and `wheel pane pop <worker>` shows it. Against a server older than this, they fail with `restart the server`. `pane move <pane> <target> <side>` goes beside a pane in any space; `pane move <pane> --end-of <space>` and `pane move <pane> --new-space [--name N] [--focus]` go to the end of a space or into a new one after its own. The pane keeps its id and process; a space left empty gets one empty pane, and a target space holding only an empty pane loses it.
- `wheel pane start <pane> agent [--agent N] [--model M] [--message T]`: make a pane an agent (or another kind) and start it. `--model` hands the agent its model (`sonnet`, a full id such as `claude-sonnet-5-5`, a Codex model id); a name Claude or Codex does not know fails the command with the agent's error (a full Claude id or a pi model is refused at the first turn instead). `wheel status` lists the model each agent pane runs as `model` on the pane.
  `wheel pane kind <pane> <kind>` changes what a pane shows; `pane page <pane> <url>` opens a page in
  a browser pane; `pane shot <pane> <file.png>` saves a picture of a pane (`--full` makes it one tall picture of a web or artifact page at its full height, or of an agent chat's whole conversation, with no scrolling by you; a pane that fits is unchanged; a terminal refuses, read its scrollback with `term read`); `pane console <pane>` prints a browser
  pane's console messages and uncaught errors since its last page load (`--follow`, `--since <unix ms>`, `--json`).
  `pane eval <pane> <js>` runs JavaScript in a browser pane's page and prints the result as JSON (a thrown error prints its message and exits non-zero); `pane dom <pane> [selector]` prints the page's HTML, or the outer HTML of every match (no match fails); `pane shot <pane> <file.png> --selector <css>` saves a picture of the first matching element only; `pane record <pane> <file.mov> --seconds N` (1 to 60) films a browser pane from the web view's own snapshots and prints the absolute path, the frames written and the duration (the picture is the page without the pane's header; no Screen Recording permission). They read the page's main frame only (not iframes), and all four fail on a pane that is not a browser pane.
- `wheel term open | send | read | close`: terminals.
  - `wheel term send <pane> "ls" --enter` types text and presses Return.
  - `wheel term send <pane> --key up --key enter` presses named keys: `enter`, `tab`, `esc`, `space`,
    `backspace`, `delete`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`,
    `ctrl-a` to `ctrl-z`, `f1` to `f12`, or one character. Use it to answer a menu or choice a
    program shows in a terminal (`--key down --key enter`, `--key 1`).
  - `wheel term read <pane> --until "text" --timeout 30` prints the screen, then live output until
    the text appears (exit 3 if it never does).
- `wheel agent send <pane> "message"`: send a chat agent a message and print its reply as it ends.
  `agent read`, `agent wait`, `agent cancel`, `agent conversations`, `agent new`, `agent resume`,
  `agent context`, `agent config`: follow and steer a chat agent. `wheel agent watch` prints every agent pane as one JSON line (`space`, `cwd`, `pane`, `agent`, `harness`, `model`, `status`: idle, working, needs_you or stopped), then again whenever the list changes, until the daemon goes away.
- `wheel voice start <space-id|daemon>` starts the voice orchestrator, an agent that belongs to no pane, and prints its agent name (`voice-<scope>`); read it and send to it with `wheel agent read` and `wheel agent send`. It runs only whitelisted `wheel` commands. `wheel voice stop <scope>` ends it.
- `wheel perm list | wait | answer`: permission requests agents are waiting on.
  `wheel perm answer <request-id-or-agent-pane> allow|deny|<option-id>` answers one. **Only answer a
  request when the person has told you to.** Never answer your own.
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
- `wheel artifact tools <name>` and `wheel artifact call <name> <tool> [<json>]`: an entry with
  `mcp = "<command>"` (run through `sh -c` in the space's directory, speaking MCP on stdin and stdout) or
  `mcp_url = "<http(s) URL>"` (streamable HTTP) names an MCP server. The daemon connects on the first use
  and keeps the session until the entry's definition changes. `tools` lists each tool's name, description
  and input schema (`--json` for the schemas whole); `call` passes the JSON object (none for a tool with no
  arguments) and prints the result; a tool that fails prints its message and exits 1. Both take `--scope`
  and `--space` like `artifact show`. An mcp entry takes no `path`, and neither `app` nor the other mcp key.
  `wheel artifact read <name> <uri>` prints a resource the server lists (`resources/read`). A server that
  links a tool to an `a2ui://` resource (`_meta.ui.resourceUri`, MIME `application/a2ui+json`) gets a native
  pane on the Mac and iPhone: the resource is the layout, a tool's result carries `updateDataModel`
  messages, and a button's action calls the tool named by the action event with its context as arguments.
- `wheel config ls | get | set | unset | watch`: settings, for the machine or one space.
- `wheel guidance show | path [--agent <name>]`: the text an agent is given and the path of the person's own guidance file for every agent or one agent (made empty if missing).
- `wheel run [<name>]`: the `[runner]` table of wheel.toml maps names you choose to shell commands
  (`build = "make -j8"`); a project's table overrides the machine's. `wheel run` lists them,
  `wheel run <name>` types the command into a new terminal pane in the space's directory and prints the pane.
  The `runner` built-in shows them as buttons (`wheel artifact builtin install runner`, then
  `wheel pane kind <pane> web --artifact runner --scope machine`), and the command palette lists
  each as `Run <name>`.
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
