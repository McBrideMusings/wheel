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

Outside Wheel, pass `--url ws://127.0.0.1:7420/ws --token-file <file>`, or run
`wheel install` once to link this command into `~/.local/bin`.

`wheel status` prints the whole state as JSON: spaces, the layout of each, terminals, agent sessions
and waiting permission requests. Start there.

## Commands

Ids: spaces look like `s1`, panes `p3`, terminals `t2`, agent sessions `w4`. Where a command takes a
terminal or agent, a pane id showing it works too. `wheel <command> --help` lists every flag.

- `wheel space ls | new <dir> | focus <id> | close <id>`: the projects.
- `wheel pane ls`: the panes of a space. `pane split`, `pane swap`, `pane move`, `pane close`,
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
- `wheel config ls | get | set | unset | watch`: settings, for the machine or one space.
- `wheel usage`: Claude account usage.

## Rules

- Leave panes you did not make alone unless the person asks.
- Ask the person before closing a space or a pane that holds work.
- Prefer `wheel status` and `--json` flags (where a command has one) over parsing human text.
