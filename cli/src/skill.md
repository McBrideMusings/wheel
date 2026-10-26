# Driving Wheel with the `wheel` command

Wheel holds a person's projects as **spaces**. Each space has a directory and a grid of **panes**:
terminals, agent chats, web panes (an artifact: a folder with `artifact.toml` and `index.html`) and
browsers. A daemon (`wheeld`) holds all of it; `wheel` is the command that drives it. Every command
prints to stdout and exits 0 on success; a failure prints `wheel: <why>` to stderr and exits
non-zero (3 for a timeout, 4 when a terminal exited first).

## Where you are

Inside a Wheel terminal or agent pane these variables are set, so no flag is needed:

- `WHEEL_URL` and `WHEEL_TOKEN_FILE`: how `wheel` reaches the daemon.
- `WHEEL_SPACE`: the id of your space.
- `WHEEL_PANE`: the id of your pane. Closing it ends you.
- `WHEEL_SPACE_DIR`: the space's directory. Set in terminal panes, not in chat agents.

Outside Wheel, pass `--url ws://127.0.0.1:7420/ws --token-file <file>`, or run
`wheel install` once to link this command into `~/.local/bin`.

`wheel status` prints the whole state as JSON: spaces, the layout of each, terminals, agent sessions
and waiting permission requests. Start there.

## Commands

Ids: spaces look like `s1`, panes `p3`, terminals `t2`, agent sessions `w4`. Where a command takes a
terminal or agent, a pane id showing it works too. `wheel <command> --help` lists every flag.

- `wheel space ls [--json] | new <dir> | focus <id> | close <id> | rename <space> <name>`: the projects. `space new --background` makes a space without showing it: the Mac app keeps it in the background, with no window, and closes it once its last pane closes; split it with `pane split --background` so it stays there. A space is named by id or by name. `wheel space ls --json` prints `[{id, name, cwd, status, panes}]`, `panes` being pane ids. `wheel space status` prints each space's status (`needs_you` with its waiting count, `stopped`, `working` or `idle`) and one indented line per pane; `wheel space ls` carries the same `status` fields. `wheel space recent [--json]` lists the directories spaces were made for, newest first. `wheel space browse [path] [--json]` lists the subfolders of a folder on the daemon's machine (`~` is the daemon user's home) and marks any that is open as a space with `(space <id>)`; with no path it lists each root from the `spaces.roots` setting. A missing folder exits non-zero with `no folder at <path>`.
- `wheel window ls [--json] | new [--name N] [--space S] | rename <window> <name> | close <window>` and `wheel space move <space> <window|background>`: the Mac app's windows. A space is in one window or in the background, where it keeps running. They need the Mac app; without it they fail with `no Mac app attached`.
- `wheel pane ls`: the panes of a space. `wheel pane near [pane]`: the panes beside one (default `$WHEEL_PANE`), one line each: side, id, kind, what it shows. `pane split`, `pane swap`, `pane move`, `pane close`,
  `pane focus`, `pane ratio`: arrange the grid.
- `wheel pane start`: make a pane an agent (or another kind) and start it.
  `wheel pane kind <pane> <kind>` changes what a pane shows; `pane page <pane> <url>` opens a page in
  a browser pane; `pane shot <pane> <file.png>` saves a picture of a pane.
- `wheel term open | send | read | close`: terminals.
  - `wheel term send <pane> "ls" --enter` types text and presses Return.
  - `wheel term send <pane> --key up --key enter` presses named keys: `enter`, `tab`, `esc`, `space`,
    `backspace`, `delete`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`,
    `ctrl-a` to `ctrl-z`, `f1` to `f12`, or one character. Use it to answer a menu or choice a
    program shows in a terminal (`--key down --key enter`, `--key 1`).
  - `wheel term read <pane> --until "text" --timeout 30` prints the screen, then live output until
    the text appears (exit 3 if it never does).
- `wheel agent send <pane> "message"`: send a chat agent a message and print its reply as it ends.
  `agent read`, `agent wait`, `agent cancel`, `agent sessions`, `agent new`, `agent resume`,
  `agent context`, `agent config`: follow and steer a chat agent.
- `wheel perm list | wait | answer`: permission requests agents are waiting on.
  `wheel perm answer <request-id-or-agent-pane> allow|deny|<option-id>` answers one. **Only answer a
  request when the person has told you to.** Never answer your own.
- `wheel artifact new | add | remove | ls | refresh | trust | show`: web-pane artifacts. A refresh
  command in `artifact.toml` runs only after the person approves it once.
  `wheel artifact state <pane> [<key>] [--wait]` prints what the page stored with `wheel.set(key, value)`
  (`--wait` blocks until the key is written: post a question, wait for the answer);
  `wheel artifact data <pane> <file|->` pushes a JSON value into the running page (its
  `wheel.onData(fn)`), no reload. A page loads the helper with `<script src=".wheel/client.js">`.
- `wheel config ls | get | set | unset | watch`: settings, for the machine or one space.
- `wheel usage`: Claude account usage.
- `wheel remote info`: the remote listener's addresses, port and certificate fingerprint (the `remote.listen` setting turns it on).

## Asking the person a question, or showing a result with pictures: the `markdown` built-in

A post is a folder of data that another artifact's page draws. `markdown` is the page for a
question or a result: write what you would write in chat, as a markdown file named `index.md`,
and show it. When you have a new turn, change that same post and push it. Never make a second
artifact for the same conversation. Install the page once per machine with
`wheel artifact builtin install markdown` (`wheel artifact builtin ls` says whether it is there).

```sh
wheel artifact new ask --renderer markdown --reply-to "$WHEEL_PANE" index.md   # the post: index.md plus every local image it points at
P=$(wheel pane split "$WHEEL_PANE" right)             # a pane for it
wheel pane kind "$P" web --artifact ask
wheel artifact state "$P" submitted --wait            # blocks until the person presses Send; prints {"answers":{"1":"A"},"note":"…","summary":"…"}
# next turn: edit index.md, then push it; the open page changes with no reload and a choice on a kept question stays
wheel artifact data "$P" index.md
```

The page has a footer that lists what the person has chosen, a note box and a Send button. Choosing
only changes that draft; **Send** is the one event you hear. It stores `submitted`, and does what the
post's entry names: `--reply-to <pane>` types a one-line summary of the answers into that pane (your
chat or terminal, so you are woken even if you are not waiting), and `--notify <url>` POSTs
`{post, pane, submitted}` to an http(s) URL. Without either flag, wait on `submitted` as above.

**When the answer has arrived, close the question pane** (`wheel pane close "$P"`) and tell the person what you took from it. A pane left open after Send is a question nobody is asking. Open a new pane for the next question, or push the next turn into the same post and keep the pane only while you are still asking.

`--renderer markdown` names the page that draws the post; `ask` is the post's own name. A post
made with `--ends-with-session` or `--expires 2h` deletes itself; without them it stays.
`index.md` is read like this:

```markdown
# Per-agent status dots

Plain paragraphs. **bold** and `code` only.

![A, six agents, dark](dots-a.png)

## 1. How do several agents show up as dots?

### 1A. what physically happens — my pick
- strongest case, one bullet each
![](dots-a.png)
Cost: what it costs

### 1B. what happens instead
Cost: what it costs

Answer 1A, or type go to take every pick.
```

An image path is a file beside `index.md` (or below it); `wheel artifact new` copies each one into
the post and stops with the file name if one is missing. A markdown file with no `##` question
headings is a plain information page. A click on any image opens a spotlight (click again to zoom,
drag to pan, Esc to close, arrow keys step between images).

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

## Rules

- Leave panes you did not make alone unless the person asks.
- Ask the person before closing a space or a pane that holds work.
- Prefer `wheel status` and `--json` flags (where a command has one) over parsing human text.
