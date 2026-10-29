//! `wheel`: drive a running `wheeld` from the command line. Every command speaks the same
//! WebSocket protocol the apps use, so anything a client can do, a script can check.
//!
//! Exit codes: 0 success, 2 bad arguments, 3 timed out before `--until` matched,
//! 4 the terminal exited before `--until` matched, 5 could not connect or authenticate,
//! 6 the daemon refused the request, 7 lost the connection after connecting (the request may
//! already have taken effect), 1 anything else.

mod statusline;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wheel_proto::{
    decode_bytes, encode_bytes, ArtifactScope, ClientMsg, ConfigKind, Dir, DirEntry, NewPaneKind,
    PaneView, ServerMsg, SessionEvent, SessionState, SettingInfo, SettingOrigin, StartKind,
    TimerState, ToolContent,
};

#[derive(Parser)]
#[command(
    name = "wheel",
    about = "Drive a running wheeld",
    after_help = "AI agents: run `wheel --skill` for the guide to every command. Inside a Wheel pane, \
WHEEL_URL, WHEEL_TOKEN_FILE and WHEEL_PANE are already set."
)]
struct Cli {
    /// Daemon WebSocket URL.
    #[arg(long, global = true, env = "WHEEL_URL", default_value_t = format!("ws://{}/ws", wheel_proto::DEFAULT_LISTEN))]
    url: String,
    /// File holding the daemon's client token. Defaults to ~/Library/Application Support/wheel/token.
    #[arg(long, global = true, env = "WHEEL_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// Print the guide to driving Wheel with this command, for an AI agent to read, and exit.
    #[arg(long)]
    skill: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

/// The guide `wheel --skill` prints.
const SKILL: &str = include_str!("skill.md");

#[derive(Subcommand)]
enum Command {
    /// Link this `wheel` into ~/.local/bin (or --dir) so agents outside Wheel can run it, or with
    /// --status say whether that link is there and on PATH. Needs no daemon.
    Install {
        /// Only report: `installed`, `elsewhere` (another wheel is there), `missing` or `not on PATH`.
        #[arg(long)]
        status: bool,
        /// Where to put the link instead of ~/.local/bin.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Print the daemon's state as JSON: its terminals, spaces, permissions, usage, and `agents`,
    /// each agent's command with whether its program was found, where, and why not.
    Status {
        /// Accepted for symmetry with the other commands; the output is always JSON.
        #[arg(long)]
        json: bool,
    },
    /// Every Claude account's usage: 5-hour and weekly percentages, reset times, source and age,
    /// then the rows the usage strip shows. Exits 3 when any account's reading is stale or missing.
    Usage,
    /// Remote access to the daemon.
    #[command(subcommand)]
    Remote(RemoteCommand),
    /// Terminals.
    #[command(subcommand)]
    Term(TermCommand),
    /// Agent sessions, by the name their agent pane carries (e.g. `w5`).
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Permission requests waiting on a person.
    #[command(subcommand)]
    Perm(PermCommand),
    /// Spaces: one per project, each a split tree of panes.
    #[command(subcommand)]
    Space(SpaceCommand),
    /// The Mac app's windows. They need the Mac app to be running.
    #[command(subcommand)]
    Window(WindowCommands),
    /// Panes inside a space.
    #[command(subcommand)]
    Pane(PaneCommand),
    /// Files on the daemon's machine, as a chat reply's file links read them.
    #[command(subcommand)]
    File(FileCommand),
    /// Artifacts: a page a web pane can show, named by an entry in a wheel.toml.
    #[command(subcommand)]
    Artifact(ArtifactCommand),
    /// Wheel's own settings, from wheel.toml: a project's over this machine's.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Run a command the `[runner]` table of wheel.toml names, in a new terminal pane in the
    /// space's directory, and print the pane. Without a name, list the commands. The table maps
    /// any name you choose to a shell command, e.g. `build = "make -j8"`; a project's table
    /// overrides this machine's, name by name.
    Run {
        /// The runner to start.
        name: Option<String>,
        /// The space whose directory runs it; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Print the list as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RemoteCommand {
    /// Print how a remote client reaches the daemon: the `remote.listen` setting, the addresses
    /// to dial, the port and the certificate fingerprint to pin; "remote access is off" when the
    /// setting is empty.
    Info,
    /// Make a one-time pairing token (good for five minutes) and print the pairing code a phone
    /// scans or pastes: `{url, token, cert_sha256}` as JSON.
    Pair {
        /// The address the code names, one of those `info` lists; a Tailscale address when there
        /// is one, else the first IP.
        #[arg(long)]
        address: Option<String>,
    },
    /// List the paired devices: id, name, when each last connected and whether it is connected now.
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Forget a paired device: its credential stops working and its connections close.
    Revoke { id: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print every setting, its value, and which wheel.toml that value came from.
    Ls {
        /// The space whose wheel.toml is the project layer; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the absolute path of this machine's wheel.toml and of the active (or named) space's.
    Path {
        /// The space whose wheel.toml is the project layer; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Print the paths as JSON: `{"machine": ..., "project": ...}`.
        #[arg(long)]
        json: bool,
    },
    /// Print one setting's value in force.
    Get {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        #[arg(long)]
        space: Option<String>,
        /// Print the record as JSON rather than the bare value.
        #[arg(long)]
        json: bool,
    },
    /// Write one setting, into a project's wheel.toml or this machine's.
    Set {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        /// The value to write.
        value: String,
        /// Write the wheel.toml of this space's directory; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Write this machine's wheel.toml (~/.wheel/wheel.toml) instead.
        #[arg(long)]
        machine: bool,
        /// Print the settings as JSON once written.
        #[arg(long)]
        json: bool,
    },
    /// Print the folders an agent (claude, codex or pi) could run with as its home, each marked
    /// `valid` (holds the agent's data), `empty` (a new login), `missing` or `not_folder`; with
    /// `--path`, also what that one path is. An empty `--path` asks about the agent's default.
    Homes {
        /// claude, codex or pi.
        agent: String,
        #[arg(long)]
        path: Option<String>,
        /// Print the reply as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Wait for the next settings broadcast, the one every connection hears after a write or a
    /// removal; print the space it was resolved for and each setting's value and origin.
    Watch {
        /// Seconds to wait (0 to 3600); exit 3 if no broadcast arrives.
        #[arg(long, default_value_t = 10.0, value_parser = parse_timeout)]
        timeout: f64,
        /// Print the broadcast as JSON: `{"space": ..., "settings": [...]}`.
        #[arg(long)]
        json: bool,
    },
    /// Remove one setting from a project's wheel.toml or this machine's, so the layer below is
    /// in force: this machine's value under a project's, wheel's default under the machine's.
    Unset {
        /// The setting's key, e.g. `browser.homepage`.
        key: String,
        /// Change the wheel.toml of this space's directory; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Change this machine's wheel.toml (~/.wheel/wheel.toml) instead.
        #[arg(long)]
        machine: bool,
        /// Print the settings as JSON once removed.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ArtifactCommand {
    /// Make an artifact — a folder holding an index.html, and its entry in the scope's wheel.toml —
    /// and print the folder's absolute path.
    New {
        /// One plain name, never a path.
        name: String,
        /// Write `pane = true` into the entry: offer this artifact in a pane picker. Leave it off
        /// for a one-off nobody needs to find in a menu.
        #[arg(long)]
        pane: bool,
        /// Which root to put it under: project (in the project's directory, the default) or machine
        /// (every project on this machine).
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        /// The space whose wheel.toml holds a project artifact's entry; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// The folder to make, anywhere; relative to this directory. With none, the space's
        /// .wheel/artifacts/<name>, or the instance directory's artifacts/<name> for machine.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Delete the artifact when this agent pane's session ends. Needs WHEEL_PANE, so run it
        /// from an agent pane; cannot be combined with --path. Without it nothing is deleted.
        #[arg(long)]
        ends_with_session: bool,
        /// Delete the artifact this long after it is made: a whole number and a unit, s, m, h or
        /// d (30m, 2h, 7d). Cannot be combined with --path. Without it nothing is deleted.
        #[arg(long, value_parser = parse_duration)]
        expires: Option<u64>,
        /// Make a post instead: a folder holding only the files given, drawn by this artifact's
        /// page (for example `markdown`), with no page of its own. Images a markdown file points
        /// at are copied with it.
        #[arg(long, requires = "files")]
        renderer: Option<String>,
        /// The agent pane a page's Send types its answer into (a chat agent as a message, a
        /// terminal as keystrokes and Enter). Pass your own pane: `--reply-to "$WHEEL_PANE"`.
        #[arg(long)]
        reply_to: Option<String>,
        /// An http(s) URL the daemon POSTs a page's Send to, as JSON `{post, pane, submitted}`.
        #[arg(long)]
        notify: Option<String>,
        /// The files a post holds; each must be named here or, for a markdown file's images,
        /// pointed at from it.
        files: Vec<PathBuf>,
    },
    /// Register a page that already exists — a folder holding index.html, or one HTML file —
    /// as an artifact, from wherever it is: write its entry into the scope's wheel.toml, touch
    /// nothing in its folder, and print the record.
    Add {
        /// The folder or HTML file; relative to this directory.
        path: PathBuf,
        /// One plain name. With none, the folder's name, or the file's without its extension.
        #[arg(long)]
        name: Option<String>,
        /// Whose wheel.toml holds the entry: project (the space's, the default) or machine (every
        /// project on this machine).
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        /// The space whose wheel.toml holds a project entry; the active space with none.
        #[arg(long)]
        space: Option<String>,
        /// Shown wherever the artifact is listed; the name with none.
        #[arg(long)]
        title: Option<String>,
        /// A refresh command, run in the space's directory.
        #[arg(long)]
        command: Option<String>,
        /// Seconds between timed refreshes; needs --command.
        #[arg(long)]
        every: Option<u64>,
        /// Offer this artifact in a pane picker.
        #[arg(long)]
        pane: bool,
        /// Print the record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete an artifact's entry from its wheel.toml, leaving every file it names where it is,
    /// and print the folder it named.
    Remove {
        name: String,
        /// Whose wheel.toml to delete it from. With none, the scope the bare name resolves to.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        #[arg(long)]
        space: Option<String>,
    },
    /// Print every artifact of every scope, with its folder, title and any problems.
    Ls {
        #[arg(long)]
        space: Option<String>,
        /// Only the artifacts whose entry says `pane = true`: what a pane picker offers.
        #[arg(long)]
        pane: bool,
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run the refresh command of the artifact a web pane shows, now, and print the run. Exits 1 when
    /// the run failed — a non-zero exit status, a refusal, or a command that would not start.
    Refresh {
        /// The web pane to refresh for; its data directory is what the command writes.
        pane: String,
        /// Print the runner as JSON.
        #[arg(long)]
        json: bool,
        /// Seconds to wait for the run, a person's answer included (0 to 3600).
        #[arg(long, default_value_t = 120.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the values the page of a web pane has stored (`wheel.set(key, value)` in the page) as
    /// JSON: one key's value, or an object of all of them. With `--wait`, blocks until the key is
    /// written, so an agent can post a question into the page and wait for the answer. Exits 3
    /// when the wait ran out, 1 when a key is not stored and `--wait` is not given.
    State {
        pane: String,
        key: Option<String>,
        /// Wait for the key to be written (needs a key).
        #[arg(long, requires = "key")]
        wait: bool,
        /// Seconds to wait (0 to 3600).
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Push one JSON value (from a file, or `-` for stdin) into the running page of a web pane,
    /// without reloading it. The page receives it through `wheel.onData(fn)`; a page that loads
    /// later receives the latest value.
    Data {
        pane: String,
        /// A file holding one JSON value, or `-` for stdin.
        file: String,
    },
    /// Built-in artifacts: pages wheeld carries and installs for every project on this machine.
    #[command(subcommand)]
    Builtin(BuiltinCommand),
    /// Print one artifact's record.
    Show {
        name: String,
        /// Which root it is under. With none, project if it has it, else machine.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        #[arg(long)]
        space: Option<String>,
        /// Print the record as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BuiltinCommand {
    /// Print each built-in: the version wheeld carries, the version installed, and any installed
    /// file a person edited.
    Ls {
        /// Print the records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install a built-in into this machine's artifacts folder, with its machine entry.
    Install { name: String },
    /// Write the version wheeld carries over the installed one. With an edited file and neither
    /// flag, print each edited file's diff, change nothing and exit 1.
    Update {
        name: String,
        /// Leave edited files as they are.
        #[arg(long, conflicts_with = "replace_edited")]
        keep_edited: bool,
        /// Overwrite edited files with the new version.
        #[arg(long)]
        replace_edited: bool,
    },
    /// Remove an installed built-in: its machine entry, its folder and Wheel's record of it.
    Remove { name: String },
}

#[derive(Subcommand)]
enum SpaceCommand {
    /// Print every space, its pane tree, and the active space as JSON.
    Ls {
        /// Print a flat JSON array of {id, name, cwd, status, panes} instead, `panes` being the
        /// ids of the space's panes.
        #[arg(long)]
        json: bool,
    },
    /// Print each space's status (needs_you, stopped, working or idle) and its waiting count,
    /// then one indented line per pane.
    Status,
    /// Make a space for a directory and print its id.
    New {
        cwd: PathBuf,
        /// Defaults to the directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Leave the space being shown as it is. The Mac app keeps the new space in the
        /// background, with no window, and closes it once its last pane closes.
        #[arg(long)]
        background: bool,
    },
    /// Print the directories spaces were made for, newest first.
    Recent {
        /// Print them as a JSON array of {dir, opened_ms}.
        #[arg(long)]
        json: bool,
    },
    /// List the folders a space can be made for: the subfolders of a path, marking any that is
    /// open as a space with that space's id. With no path, each browsing root (the `spaces.roots`
    /// setting) and its subfolders.
    Browse {
        /// A folder on the daemon's machine; `~` is the daemon user's home.
        path: Option<String>,
        /// Print JSON: {path, dirs} for a path, an array of them for the roots.
        #[arg(long)]
        json: bool,
    },
    /// Close a space and every pane in it.
    Close { id: String },
    /// Make a space the active one.
    Focus { id: String },
    /// Name a space; the name is saved and every client shows it. A space is named by id or name.
    Rename { space: String, name: String },
    /// Put a space in a Mac window, or `background` to run it with no window. Needs the Mac app.
    Move { space: String, to: String },
}

#[derive(Subcommand)]
enum WindowCommands {
    /// List the Mac app's windows and the spaces in each, the background spaces, then each little
    /// window with its space and the main window it belongs to.
    Ls {
        /// Print the windows as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Open a window and print its id.
    New {
        #[arg(long)]
        name: Option<String>,
        /// A space to open in it, by id or name.
        #[arg(long)]
        space: Option<String>,
    },
    /// Name a window, by id or name; an empty name clears it.
    Rename { window: String, name: String },
    /// Ask in a sheet on a window that holds spaces whether to background them or stop them; an empty window closes.
    Close { window: String },
}

#[derive(Subcommand)]
enum PaneCommand {
    /// Print every pane, one per line: its space, id, kind and whether it has its space's focus.
    Ls {
        /// Print the panes as a JSON array of {space, pane, kind, focused}.
        #[arg(long)]
        json: bool,
    },
    /// Print the panes beside a pane, one per line: which side they are on (left, right, up or
    /// down), id, kind and what they show (a browser's URL, an artifact's name, a terminal's
    /// directory and agent).
    ///
    /// The pane is `$WHEEL_PANE` unless one is named. A pane that reaches along a side of it counts
    /// for that side, so a tall pane on the right is the neighbour on the right of every pane that
    /// touches it.
    Near {
        /// The pane to look around. Defaults to the pane this runs in.
        #[arg(env = "WHEEL_PANE")]
        pane: String,
        /// Print the neighbours as a JSON array of {side, pane, kind, shows}.
        #[arg(long)]
        json: bool,
    },
    /// Split a pane and print the new (empty) pane's id. With no side, the `panes.placement`
    /// setting chooses it and the output reads `<new pane> placed by <strategy>`.
    Split {
        pane: String,
        /// Side the new pane goes on: left, right, up or down.
        #[arg(value_parser = wheel_proto::from_wire::<Dir>)]
        dir: Option<Dir>,
        /// Leave the focus and the space being shown as they are, so a split in a background
        /// space keeps it in the background.
        #[arg(long)]
        background: bool,
    },
    /// Split a pane and show a page (`--page`) or an artifact (`--artifact`) in the new pane;
    /// prints the new pane's id.
    Open {
        pane: String,
        /// Side the new pane goes on: left, right, up or down.
        #[arg(value_parser = wheel_proto::from_wire::<Dir>)]
        dir: Dir,
        /// A browser pane on this page. Not `--url`, which is the daemon's address.
        #[arg(long)]
        page: Option<String>,
        /// A web pane on this artifact.
        #[arg(long)]
        artifact: Option<String>,
        /// Which registry holds the artifact: project or machine. With none, the first with an entry.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
    },
    /// Run a shell command in a new terminal pane split right of a pane, in the space's
    /// directory; prints the new pane's id.
    Run {
        pane: String,
        /// The command, typed into the new pane's shell.
        command: String,
    },
    /// Swap the places of two panes in one space.
    Swap { pane: String, with: String },
    /// Move a pane, keeping its id and its running process: beside a pane in any space, to the end
    /// of a space (`--end-of`), or into a new space after its own (`--new-space`). The split it
    /// left collapses; a space left with no pane gets one empty pane, and a target space whose
    /// only pane is empty loses that empty pane.
    Move {
        pane: String,
        /// The pane to go beside, in any space.
        #[arg(requires = "dir", conflicts_with_all = ["end_of", "new_space"])]
        target: Option<String>,
        /// Side of the target the pane goes on: left, right, up or down.
        #[arg(value_parser = wheel_proto::from_wire::<Dir>)]
        dir: Option<Dir>,
        /// Put the pane at the end of this space (an id or a name).
        #[arg(long, value_name = "SPACE", conflicts_with = "new_space")]
        end_of: Option<String>,
        /// Put the pane in a new space made after its own.
        #[arg(long)]
        new_space: bool,
        /// With --new-space: the new space's name; default the folder's name.
        #[arg(long, requires = "new_space")]
        name: Option<String>,
        /// With --new-space: leave the active space as it is.
        #[arg(long, requires = "new_space")]
        background: bool,
    },
    /// Close a pane; closing a space's last pane leaves the space with one new empty pane.
    Close { pane: String },
    /// Start a terminal or an agent in a pane, whatever it showed going. Prints the pane and, for
    /// an agent, its session name and state.
    ///
    /// Waits up to a minute for an agent to come up.
    Start {
        pane: String,
        #[arg(value_parser = wheel_proto::from_wire::<StartKind>)]
        kind: StartKind,
        /// For an agent: its first message. The command waits until the agent is ready or has
        /// given up, and fails with the daemon's error when it could not start.
        #[arg(long)]
        message: Option<String>,
        /// For an agent: which one to run, by name (see `agents` in `wheel status`). Without it,
        /// the `agent.default` setting names it. Fails, saying why, when its program is not found.
        #[arg(long)]
        agent: Option<String>,
        /// For an agent: the model to run, in the agent's own words (`sonnet`, `claude-sonnet-5-5`
        /// for Claude; a model id for Codex). Without it the agent's own default. A name Claude or
        /// Codex can check against its model list and does not know fails the command with the
        /// agent's error; a full Claude id or a pi model is the agent's to refuse at the first
        /// turn. `wheel status` shows the model each pane runs.
        #[arg(long)]
        model: Option<String>,
        /// For an agent: the permission mode to start in, in the agent's own terms. Claude Code:
        /// default, acceptEdits, auto, bypassPermissions, dontAsk, plan (its `--permission-mode`).
        /// Codex: untrusted, on-request, never (its approval policy). Fails, naming the valid
        /// ones, when the agent has no such mode. `wheel status --json` reports the mode each
        /// agent pane is in as `permission_mode`.
        #[arg(long)]
        permission_mode: Option<String>,
        /// For a Codex agent: its sandbox, read-only, workspace-write or danger-full-access.
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Show a pane's Claude Code, Codex or pi conversation the other way: `agent` turns a terminal
    /// running the agent into an agent pane on that conversation, `terminal` turns an agent pane
    /// into a terminal running `claude --resume`, `codex resume` or `pi --session` on it. The other
    /// view's process stops first; a switch during a running turn fails, saying why. With no view,
    /// changes nothing.
    ///
    /// Prints the view the pane is in and the conversation it shows, `-` for none.
    View {
        pane: String,
        /// The view to show the conversation in: agent or terminal.
        #[arg(value_parser = wheel_proto::from_wire::<PaneView>)]
        to: Option<PaneView>,
    },
    /// What a pane is: its kind and the fields a client's ⓘ popover lists (an agent pane's
    /// conversation id, session and home; a terminal's directory and program; a web pane's
    /// artifact and folder; a browser's page). Takes a pane id or an agent pane's session name.
    Info {
        pane: String,
        /// Print `{pane, kind, fields: [{label, value, hint}]}` instead of one line per field.
        #[arg(long)]
        json: bool,
    },
    /// Change what a pane shows: empty, terminal, web (an artifact, given by --artifact) or
    /// browser (a page, given by --page).
    Kind {
        pane: String,
        #[arg(value_parser = wheel_proto::from_wire::<NewPaneKind>)]
        kind: NewPaneKind,
        /// The artifact a web pane shows, by name: one plain name, never a path.
        #[arg(long)]
        artifact: Option<String>,
        /// Which root the artifact is under: project or machine. With none, project if its wheel.toml
        /// has an entry by that name, else machine.
        #[arg(long, value_parser = wheel_proto::from_wire::<ArtifactScope>)]
        scope: Option<ArtifactScope>,
        /// The page a browser pane opens: an http or https URL, a file:/// URL, or about:blank. With
        /// none, the browser.homepage setting as it resolves for the pane's space. Named `page`
        /// rather than `url` because `--url` is this CLI's own flag for the daemon's address.
        #[arg(long)]
        page: Option<String>,
    },
    /// Move a browser pane to a page, as typing it into the pane's address field does; print the
    /// page the text named.
    Page {
        pane: String,
        /// Whatever the address field takes: a URL, `example.com`, a path, or about:blank.
        page: String,
    },
    /// Say what the coding agent in a terminal pane is doing: needs_you, working or idle. The
    /// hooks Wheel's `claude` and `pi` wrappers add call this. Exits 0 and does nothing
    /// outside a Wheel pane, where `WHEEL_PANE` is unset.
    Report {
        #[arg(value_parser = wheel_proto::from_wire::<wheel_proto::ReportedStatus>)]
        status: wheel_proto::ReportedStatus,
        /// The terminal pane; with none, the one this shell runs in.
        #[arg(long, env = "WHEEL_PANE")]
        pane: Option<String>,
    },
    /// Say how full the context window of the coding agent in a terminal pane is, in the agent's
    /// own figures. The extension Wheel's `pi` wrapper loads calls this. Exits 0 and does nothing
    /// outside a Wheel pane, where `WHEEL_PANE` is unset.
    ReportContext {
        /// Tokens in the conversation.
        #[arg(long)]
        used: u64,
        /// Tokens the window holds.
        #[arg(long)]
        size: u64,
        /// The terminal pane; with none, the one this shell runs in.
        #[arg(long, env = "WHEEL_PANE")]
        pane: Option<String>,
    },
    /// Say which conversation Claude Code in a terminal pane is in now: reads the `SessionStart`
    /// hook input on stdin (or takes `--session`) and tells the daemon its `session_id`. Added by
    /// Wheel's `claude` wrapper. Exits 0 and does nothing outside a Wheel pane.
    ReportSession {
        /// The session id; with none, the `session_id` of the hook input on stdin.
        #[arg(long)]
        session: Option<String>,
        /// The terminal pane; with none, the one this shell runs in.
        #[arg(long, env = "WHEEL_PANE")]
        pane: Option<String>,
    },
    /// Claude Code's status-line command inside a Wheel pane: reads the status-line input on
    /// stdin, tells the daemon the context window it reports, then prints what the person's own
    /// status-line command prints for the same input. Added by Wheel's `claude` wrapper.
    Statusline {
        /// The terminal pane; with none, the one this shell runs in.
        #[arg(long, env = "WHEEL_PANE")]
        pane: Option<String>,
    },
    /// Wait for a web pane's artifact folder or data directory to change; print
    /// `pane_reload <pane> at <unix ms>`.
    WaitReload {
        pane: String,
        /// Seconds to wait (0 to 3600); exit 3 if no change arrives.
        #[arg(long, default_value_t = 10.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Write a PNG of what a pane shows to a file and print the absolute path written.
    ///
    /// The daemon asks a connected client that draws its panes (the Mac app) for the pane's own
    /// view, so no screen-recording permission and no window focus is needed. Exits after the
    /// file is complete; fails when no client is connected or none shows the pane.
    Shot {
        pane: String,
        /// The PNG file to write; a relative path is taken from this shell's directory.
        out: PathBuf,
    },
    /// Focus a pane, making its space active.
    Focus { pane: String },
    /// Set the share of a split its first pane takes (0.1 to 0.9).
    Ratio { split: String, ratio: f64 },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Send an agent a message and print its reply as it streams.
    ///
    /// When the turn ends for any reason but a normal finish, stderr gets one line with the raw
    /// protocol token, `[turn ended: max_tokens]`. The token is deliberate: scripts match on it,
    /// where the apps word the same reason for a person.
    Send {
        /// An agent pane's id or its session name.
        session: String,
        text: String,
        /// Seconds to wait for the reply to finish (0 to 3600).
        #[arg(long, default_value_t = 300.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print an agent's conversation so far.
    Read {
        /// An agent pane's id or its session name.
        session: String,
        /// Print the session's state and its raw events as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop a session's current turn, or with none running its next queued message; later ones still
    /// run. Fails when there is neither.
    Cancel {
        /// An agent pane's id or its session name.
        session: String,
    },
    /// Wait until an agent is ready (no turn running or queued) and print its pane, session and
    /// state. Exits 3 when the time runs out first, 6 when the session gave up.
    Wait {
        /// An agent pane's id or its session name.
        session: String,
        /// Seconds to wait (0 to 3600).
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// Print the conversations an agent holds for its directory, newest first, marking the one
    /// its pane is in.
    Sessions {
        /// An agent pane's id or its session name.
        session: String,
        /// Print them as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Put an agent's pane in a fresh conversation and print the agent's id for it. Every
    /// client's transcript empties with it.
    New {
        /// An agent pane's id or its session name.
        session: String,
    },
    /// Put an agent's pane back in a past conversation, named by an id `agent sessions` printed.
    /// Every client's transcript is replaced by that conversation's.
    Resume {
        /// An agent pane's id or its session name.
        session: String,
        id: String,
    },
    /// Print how full an agent's context window is: tokens used out of the window and the percent,
    /// or `unknown` while the agent has reported none. With no session, one line per agent pane.
    Context {
        /// An agent pane's id or its session name.
        session: Option<String>,
        /// Print the figures as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the controls an agent offers, or set one by giving its id and a value.
    Config {
        /// An agent pane's id or its session name.
        session: String,
        config: Option<String>,
        value: Option<String>,
        /// Print the options as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PermCommand {
    /// Print waiting permission requests as JSON.
    List,
    /// Answer one: `allow`, `deny`, or one of its option ids. The first argument is the request's
    /// id, or the id of the agent pane (or its session) whose waiting request this answers.
    Answer { id: String, choice: String },
    /// Wait for a permission request and print it as JSON (an already-waiting one counts).
    Wait {
        #[arg(long, default_value_t = 60.0, value_parser = parse_timeout)]
        timeout: f64,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Start a shell and print its id.
    Open {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 32)]
        rows: u16,
    },
    /// Type text and press named keys in a terminal.
    Send {
        /// A terminal id, or the id of a pane showing a terminal.
        id: String,
        /// Text to type; may be empty when --key is given.
        #[arg(default_value = "")]
        text: String,
        /// Send the text as one bracketed paste, so a program that turned bracketed paste on
        /// reads its newlines as text rather than as Return.
        #[arg(long)]
        paste: bool,
        /// Press Return after the text.
        #[arg(long)]
        enter: bool,
        /// Press a named key after the text, in order (repeatable): enter, tab, esc, space,
        /// backspace, delete, up, down, left, right, home, end, pageup, pagedown, ctrl-a to ctrl-z,
        /// f1 to f12, or one character.
        #[arg(long = "key", value_parser = parse_key)]
        keys: Vec<Vec<u8>>,
    },
    /// Print a terminal's screen as the daemon holds it, then live output until --until appears or --timeout passes.
    Read {
        /// A terminal id, or the id of a pane showing a terminal.
        id: String,
        /// Stop when this text appears; exit 3 if it never does, 4 if the terminal exits first.
        #[arg(long)]
        until: Option<String>,
        /// Skip the screen: only output produced after this command attached counts.
        #[arg(long)]
        new: bool,
        /// Seconds to keep reading live output (0 to 3600).
        #[arg(long, default_value_t = 1.0, value_parser = parse_timeout)]
        timeout: f64,
    },
    /// End a terminal started with `term open`. A pane's terminal ends with its pane instead.
    Close { id: String },
}

/// Where `wheel install` puts the link: `dir`, else ~/.local/bin.
fn install_dir(dir: Option<PathBuf>) -> Result<PathBuf, Fail> {
    match dir {
        Some(dir) => Ok(std::path::absolute(dir).map_err(other)?),
        None => std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".local/bin"))
            .map_err(|_| Fail::Other("HOME is not set; pass --dir".into())),
    }
}

/// How the link in `dir` stands against this program: `installed`, `elsewhere`, `missing` or, when
/// it is there but `dir` is not on PATH, `not on PATH`.
fn install_state(dir: &std::path::Path, exe: &std::path::Path) -> &'static str {
    let link = dir.join("wheel");
    match std::fs::canonicalize(&link) {
        Err(_) => "missing",
        Ok(target) if target != exe => "elsewhere",
        Ok(_) => {
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            let on_path = std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|p| std::fs::canonicalize(&p).unwrap_or(p) == dir)
            });
            if on_path {
                "installed"
            } else {
                "not on PATH"
            }
        }
    }
}

fn install(status: bool, dir: Option<PathBuf>) -> Result<(), Fail> {
    let dir = install_dir(dir)?;
    let exe = std::fs::canonicalize(std::env::current_exe().map_err(other)?).map_err(other)?;
    if !status {
        std::fs::create_dir_all(&dir).map_err(other)?;
        let link = dir.join("wheel");
        // A real file there is somebody's own `wheel`; only a link, which an earlier install made
        // or a rebuild left pointing elsewhere, is replaced.
        if std::fs::symlink_metadata(&link).is_ok_and(|m| !m.file_type().is_symlink()) {
            return Err(Fail::Other(format!(
                "{} exists and is not a link; remove it first",
                link.display()
            )));
        }
        // Replaced through a rename, so a running `wheel` never finds the path empty.
        let tmp = dir.join(".wheel.new");
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&exe, &tmp).map_err(other)?;
        std::fs::rename(&tmp, &link).map_err(other)?;
    }
    println!(
        "{}: {} -> {}",
        install_state(&dir, &exe),
        dir.join("wheel").display(),
        exe.display()
    );
    Ok(())
}

/// The bytes a terminal receives when the named key is pressed.
fn parse_key(name: &str) -> Result<Vec<u8>, String> {
    let lower = name.to_lowercase();
    let fixed: Option<&[u8]> = match lower.as_str() {
        "enter" | "return" => Some(b"\r"),
        "tab" => Some(b"\t"),
        "esc" | "escape" => Some(b"\x1b"),
        "space" => Some(b" "),
        "backspace" => Some(b"\x7f"),
        "delete" | "del" => Some(b"\x1b[3~"),
        "up" => Some(b"\x1b[A"),
        "down" => Some(b"\x1b[B"),
        "right" => Some(b"\x1b[C"),
        "left" => Some(b"\x1b[D"),
        "home" => Some(b"\x1b[H"),
        "end" => Some(b"\x1b[F"),
        "pageup" | "pgup" => Some(b"\x1b[5~"),
        "pagedown" | "pgdn" => Some(b"\x1b[6~"),
        "f1" => Some(b"\x1bOP"),
        "f2" => Some(b"\x1bOQ"),
        "f3" => Some(b"\x1bOR"),
        "f4" => Some(b"\x1bOS"),
        "f5" => Some(b"\x1b[15~"),
        "f6" => Some(b"\x1b[17~"),
        "f7" => Some(b"\x1b[18~"),
        "f8" => Some(b"\x1b[19~"),
        "f9" => Some(b"\x1b[20~"),
        "f10" => Some(b"\x1b[21~"),
        "f11" => Some(b"\x1b[23~"),
        "f12" => Some(b"\x1b[24~"),
        _ => None,
    };
    if let Some(bytes) = fixed {
        return Ok(bytes.to_vec());
    }
    if let Some(letter) = lower.strip_prefix("ctrl-").and_then(|l| {
        let mut chars = l.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_lowercase() => Some(c),
            _ => None,
        }
    }) {
        return Ok(vec![letter as u8 - b'a' + 1]);
    }
    if name.chars().count() == 1 {
        return Ok(name.as_bytes().to_vec());
    }
    Err(format!(
        "{name:?} is not a key name; see `wheel term send --help`"
    ))
}

fn parse_timeout(s: &str) -> Result<f64, String> {
    let secs: f64 = s
        .parse()
        .map_err(|_| format!("{s:?} is not a number of seconds"))?;
    if (0.0..=3600.0).contains(&secs) {
        Ok(secs)
    } else {
        Err("timeout must be between 0 and 3600 seconds".into())
    }
}

/// A failure, carrying the exit code a script can branch on.
#[derive(Debug)]
enum Fail {
    Timeout(String),
    Exited(Option<u32>),
    Connect(String),
    Daemon(String),
    /// The socket closed or failed after connecting.
    Lost(String),
    /// A usage reading is stale or missing; the message says which and why.
    Usage(String),
    Other(String),
}

impl Fail {
    fn code(&self) -> u8 {
        match self {
            Fail::Other(_) => 1,
            Fail::Timeout(_) | Fail::Usage(_) => 3,
            Fail::Exited(_) => 4,
            Fail::Connect(_) => 5,
            Fail::Daemon(_) => 6,
            Fail::Lost(_) => 7,
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Timeout(m) | Fail::Connect(m) | Fail::Other(m) | Fail::Usage(m) => f.write_str(m),
            Fail::Exited(code) => write!(
                f,
                "terminal exited (code {code:?}) before the text appeared"
            ),
            Fail::Daemon(m) => write!(f, "daemon: {m}"),
            Fail::Lost(m) => write!(f, "lost the connection to the daemon: {m}"),
        }
    }
}

fn other(e: impl std::fmt::Display) -> Fail {
    Fail::Other(e.to_string())
}

fn lost(e: impl std::fmt::Display) -> Fail {
    Fail::Lost(e.to_string())
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Conn {
    ws: Ws,
    next_req: u64,
    /// Pushed events that arrived while waiting for a reply, delivered by the next `recv`.
    events: std::collections::VecDeque<ServerMsg>,
}

impl Conn {
    async fn open(url: &str, token_file: Option<PathBuf>) -> Result<Conn, Fail> {
        let path = match token_file {
            Some(path) => path,
            None => wheel_proto::support_dir(wheel_proto::DEFAULT_INSTANCE)
                .map(|dir| dir.join("token"))
                .map_err(|_| Fail::Connect("HOME is not set; pass --token-file".into()))?,
        };
        let token = std::fs::read_to_string(&path)
            .map_err(|e| Fail::Connect(format!("read token from {}: {e}", path.display())))?;
        // An image the daemon sends is up to 20 MB, a third more as base64: past the 16 MiB default.
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(64 << 20))
            .max_frame_size(Some(64 << 20));
        let (ws, _) = tokio_tungstenite::connect_async_with_config(
            format!("{url}?token={}", token.trim()),
            Some(config),
            false,
        )
        .await
        .map_err(|e| Fail::Connect(format!("connect to {url}: {e}")))?;
        Ok(Conn {
            ws,
            next_req: 0,
            events: Default::default(),
        })
    }

    fn req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    async fn send(&mut self, msg: &ClientMsg) -> Result<(), Fail> {
        let text = serde_json::to_string(msg).map_err(other)?;
        self.ws.send(Message::Text(text.into())).await.map_err(lost)
    }

    /// The next daemon message, or None if `wait` passes first.
    async fn recv(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        match self.events.pop_front() {
            Some(event) => Ok(Some(event)),
            None => self.recv_frame(wait).await,
        }
    }

    /// Send a request and wait up to ten seconds for the reply carrying its `req` — enough for
    /// anything the daemon answers by itself.
    async fn call(&mut self, msg: ClientMsg, req: u64) -> Result<ServerMsg, Fail> {
        self.call_within(msg, req, Duration::from_secs(10)).await
    }

    /// The same, waiting `wait` for each frame: for a request the daemon cannot answer until a
    /// person has, such as a permission request.
    async fn call_within(
        &mut self,
        msg: ClientMsg,
        req: u64,
        wait: Duration,
    ) -> Result<ServerMsg, Fail> {
        self.send(&msg).await?;
        let mut held = std::collections::VecDeque::new();
        let result = loop {
            let Some(reply) = self.recv_frame(wait).await? else {
                break Err(Fail::Timeout(format!(
                    "no reply from daemon within {:.0}s",
                    wait.as_secs_f64()
                )));
            };
            match &reply {
                ServerMsg::Error {
                    req: Some(r),
                    message,
                    ..
                } if *r == req => break Err(Fail::Daemon(message.clone())),
                _ if reply.req() == Some(req) => break Ok(reply),
                _ => held.push_back(reply),
            }
        };
        self.events.extend(held);
        result
    }

    /// The next frame off the socket, skipping the queue `recv` drains first.
    async fn recv_frame(&mut self, wait: Duration) -> Result<Option<ServerMsg>, Fail> {
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(lost("it closed the socket")),
                Ok(Some(frame)) => match frame.map_err(lost)? {
                    Message::Text(text) => {
                        return serde_json::from_str(&text).map(Some).map_err(other)
                    }
                    Message::Close(Some(close)) => {
                        return Err(lost(format!("it closed the socket ({})", close.reason)))
                    }
                    _ => {}
                },
            }
        }
    }
}

async fn status(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let reply = conn.call(ClientMsg::Status { req: Some(req) }, req).await?;
    println!("{}", serde_json::to_string_pretty(&reply).map_err(other)?);
    Ok(())
}

/// Print the daemon's remote listener as `key: value` lines, or say it is off.
async fn remote_info(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Remote {
        listen,
        addresses,
        port,
        fingerprint,
        problem,
        ..
    } = conn
        .call(ClientMsg::RemoteInfo { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to remote_info".into()));
    };
    let Some(listen) = listen else {
        println!("remote access is off");
        return Ok(());
    };
    println!("listen: {listen}");
    if let Some(problem) = problem {
        return Err(Fail::Other(format!(
            "remote access did not start: {problem}"
        )));
    }
    println!("addresses: {}", addresses.join(", "));
    println!("port: {port}");
    println!("fingerprint: {fingerprint}");
    Ok(())
}

/// The pairing code for a new token: the JSON a phone reads.
async fn remote_pair(conn: &mut Conn, address: Option<String>) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Remote {
        listen,
        addresses,
        port,
        fingerprint,
        problem,
        ..
    } = conn
        .call(ClientMsg::RemoteInfo { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to remote_info".into()));
    };
    if listen.is_none() {
        return Err(Fail::Other("remote access is off".into()));
    }
    if let Some(problem) = problem {
        return Err(Fail::Other(format!(
            "remote access did not start: {problem}"
        )));
    }
    let is_ip = |a: &str| {
        a.contains(':')
            || a.split('.').count() == 4 && a.split('.').all(|p| p.parse::<u8>().is_ok())
    };
    let tailscale = |a: &String| {
        let parts: Vec<u8> = a.split('.').filter_map(|p| p.parse().ok()).collect();
        parts.len() == 4
            && a.split('.').count() == 4
            && parts[0] == 100
            && (64..=127).contains(&parts[1])
    };
    let address = match address {
        Some(a) if addresses.contains(&a) => a,
        Some(a) => {
            return Err(Fail::Other(format!(
                "{a} is not an address wheeld listens on; they are: {}",
                addresses.join(", ")
            )))
        }
        None => addresses
            .iter()
            .find(|a| tailscale(a))
            .or_else(|| addresses.iter().find(|a| is_ip(a)))
            .or(addresses.first())
            .cloned()
            .ok_or_else(|| Fail::Other("the daemon reports no address".into()))?,
    };
    let req = conn.req();
    let ServerMsg::Pairing { token, .. } = conn
        .call(ClientMsg::PairingIssue { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to pairing_issue".into()));
    };
    let host = if address.contains(':') {
        format!("[{address}]")
    } else {
        address
    };
    let code = serde_json::json!({
        "cert_sha256": fingerprint,
        "token": token,
        "url": format!("wss://{host}:{port}/ws"),
    });
    println!("{code}");
    Ok(())
}

async fn remote_devices(conn: &mut Conn, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Devices { devices, .. } = conn
        .call(ClientMsg::DeviceList { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to device_list".into()));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&devices).map_err(other)?);
        return Ok(());
    }
    if devices.is_empty() {
        println!("no paired devices");
    }
    for d in devices {
        println!(
            "{}  {}  last seen {} ms  {}",
            d.id,
            d.name,
            d.last_seen_ms,
            if d.connected > 0 {
                "connected"
            } else {
                "not connected"
            }
        );
    }
    Ok(())
}

async fn remote_revoke(conn: &mut Conn, id: String) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(
        ClientMsg::DeviceRevoke {
            req: Some(req),
            id: id.clone(),
        },
        req,
    )
    .await?;
    println!("revoked {id}");
    Ok(())
}

/// The rows the strip shows, one line each as `strip: <agent> <limit> <pct>%`, then the agents the
/// settings leave out or wheel cannot read.
fn print_strip(board: &wheel_proto::UsageBoard) {
    for agent in board.agents.iter().filter(|a| a.shown) {
        if agent.accounts.is_empty() {
            println!(
                "strip: {} {}",
                agent.id,
                agent.why.clone().unwrap_or_else(|| "no reading".into())
            );
        }
        for account in &agent.accounts {
            let info = &account.usage;
            let Some(r) = &info.reading else {
                println!(
                    "strip: {} {} {}",
                    agent.id,
                    account.name,
                    info.why.clone().unwrap_or_else(|| "no reading".into())
                );
                continue;
            };
            let stale = if info.kind == wheel_proto::UsageKind::Stale {
                format!(" (stale, {} s old)", info.age_secs.unwrap_or(0))
            } else {
                String::new()
            };
            for pin in &board.pins {
                let Some(limit) = pin.strip_prefix(&format!("{}:", agent.id)) else {
                    continue;
                };
                let pct = match limit {
                    "session" => Some(r.session_pct),
                    "weekly" => Some(r.weekly_pct),
                    other => r.model_limits.iter().find(|l| l.id == other).map(|l| l.pct),
                };
                if let Some(pct) = pct {
                    println!("strip: {} {} {limit} {pct}%{stale}", agent.id, account.name);
                }
            }
        }
    }
    for agent in board.agents.iter().filter(|a| !a.shown) {
        println!("agent: {} not shown", agent.id);
    }
}

/// Print each Claude account's usage as lines, then the strip, and fail with exit 3 when any
/// account's reading is stale or missing.
async fn usage(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Usage { board, .. } = conn
        .call_within(
            ClientMsg::Usage {
                req: Some(req),
                due_only: false,
            },
            req,
            Duration::from_secs(30),
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to usage".into()));
    };
    let mut wrong = Vec::new();
    let claude = board.agents.iter().find(|a| a.id == "claude");
    if claude.is_none_or(|a| a.accounts.is_empty()) {
        let why = claude
            .and_then(|a| a.why.clone())
            .unwrap_or_else(|| "no Claude account".into());
        println!("missing: {why}");
        wrong.push(format!("missing: {why}"));
    }
    for account in claude.map(|a| a.accounts.as_slice()).unwrap_or_default() {
        let info = &account.usage;
        println!("account: {}", account.name);
        let why = info.why.clone().unwrap_or_default();
        let Some(r) = &info.reading else {
            println!("missing: {why}");
            wrong.push(format!("{} missing: {why}", account.name));
            continue;
        };
        let reset = |at: &Option<String>| at.clone().unwrap_or_else(|| "-".into());
        println!(
            "5-hour: {}% (resets {})",
            r.session_pct,
            reset(&r.session_resets_at)
        );
        println!(
            "weekly: {}% (resets {})",
            r.weekly_pct,
            reset(&r.weekly_resets_at)
        );
        for limit in &r.model_limits {
            println!(
                "{}: {}% (resets {})",
                limit.label.to_lowercase(),
                limit.pct,
                reset(&limit.resets_at)
            );
        }
        if let Some(plan) = &r.plan {
            println!("plan: {plan}");
        }
        if let Some(on) = r.extra_usage {
            println!("extra usage: {}", if on { "on" } else { "off" });
        }
        if let Some(email) = &r.email {
            println!("email: {email}");
        }
        println!("source: {}", info.source);
        println!("age: {} s", info.age_secs.unwrap_or(0));
        if info.kind == wheel_proto::UsageKind::Stale {
            println!("stale: {why}");
            wrong.push(format!("{} stale: {why}", account.name));
        }
    }
    print_strip(&board);
    match wrong.is_empty() {
        true => Ok(()),
        false => Err(Fail::Usage(wrong.join("; "))),
    }
}

async fn term_open(conn: &mut Conn, cwd: Option<String>, cols: u16, rows: u16) -> Result<(), Fail> {
    let req = conn.req();
    match conn
        .call(
            ClientMsg::TermOpen {
                req: Some(req),
                cwd,
                cols,
                rows,
            },
            req,
        )
        .await?
    {
        ServerMsg::TermOpened { id, .. } => {
            println!("{id}");
            Ok(())
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// The space a command acts on: `--space` as given, else the `WHEEL_SPACE` a page run carries,
/// else the space that holds `WHEEL_PANE` right now (it moves with the pane), else None for the
/// active space.
async fn default_space(conn: &mut Conn, space: Option<String>) -> Result<Option<String>, Fail> {
    if space.is_some() {
        return Ok(space);
    }
    if let Ok(page_space) = std::env::var("WHEEL_SPACE") {
        return Ok(Some(page_space));
    }
    let Ok(pane) = std::env::var("WHEEL_PANE") else {
        return Ok(None);
    };
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    fn holds(node: &wheel_proto::Node, pane: &str) -> bool {
        match node {
            wheel_proto::Node::Pane { id, .. } => id == pane,
            wheel_proto::Node::Split { a, b, .. } => holds(a, pane) || holds(b, pane),
        }
    }
    match spaces.iter().find(|s| holds(&s.root, &pane)) {
        Some(s) => Ok(Some(s.id.clone())),
        None => Err(Fail::Daemon(format!("no space holds {pane} (WHEEL_PANE)"))),
    }
}

/// The terminal an id names: a terminal id as is, or the terminal a pane id (`p3`) shows.
async fn terminal_id(conn: &mut Conn, id: String) -> Result<String, Fail> {
    if !id.starts_with('p') {
        return Ok(id);
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    fn find<'a>(node: &'a wheel_proto::Node, pane: &str) -> Option<&'a wheel_proto::PaneKind> {
        match node {
            wheel_proto::Node::Pane { id, kind, .. } => (id == pane).then_some(kind),
            wheel_proto::Node::Split { a, b, .. } => find(a, pane).or_else(|| find(b, pane)),
        }
    }
    match spaces.iter().find_map(|s| find(&s.root, &id)) {
        Some(wheel_proto::PaneKind::Terminal { term, .. }) => Ok(term.clone()),
        Some(_) => Err(Fail::Daemon(format!("{id} is not a terminal pane"))),
        None => Err(Fail::Daemon(format!("no pane {id}"))),
    }
}

/// Pause between `term send`'s text and its keystrokes: long enough for the pty to hand the text
/// to the program as its own read, short enough not to be noticed.
const TERM_SEND_KEY_DELAY: Duration = Duration::from_millis(100);

async fn term_send(
    conn: &mut Conn,
    id: String,
    text: String,
    paste: bool,
    enter: bool,
    keys: Vec<Vec<u8>>,
) -> Result<(), Fail> {
    if text.is_empty() && !enter && keys.is_empty() {
        return Err(Fail::Usage(
            "term send needs text, --enter or at least one --key".into(),
        ));
    }
    if paste && text.is_empty() {
        return Err(Fail::Usage("term send --paste needs text".into()));
    }
    let id = terminal_id(conn, id).await?;
    let mut after = Vec::new();
    if enter {
        after.push(b'\r');
    }
    for key in keys {
        after.extend(key);
    }
    // A TUI with bracketed paste reads text and Enter arriving in one read as a pasted
    // newline, so the keystrokes go in a second write after the text has been consumed.
    if !text.is_empty() {
        let bytes = if paste {
            // A marker inside the text would end the paste early and type the rest as keys.
            let text = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
            [b"\x1b[200~".as_slice(), text.as_bytes(), b"\x1b[201~"].concat()
        } else {
            text.into_bytes()
        };
        term_input(conn, &id, bytes).await?;
        if !after.is_empty() {
            tokio::time::sleep(TERM_SEND_KEY_DELAY).await;
        }
    }
    if !after.is_empty() {
        term_input(conn, &id, after).await?;
    }
    Ok(())
}

async fn term_input(conn: &mut Conn, id: &str, bytes: Vec<u8>) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(
        ClientMsg::TermInput {
            req: Some(req),
            id: id.to_string(),
            data: encode_bytes(&bytes),
        },
        req,
    )
    .await?;
    Ok(())
}

async fn term_read(
    conn: &mut Conn,
    id: String,
    until: Option<String>,
    new: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let id = terminal_id(conn, id).await?;
    let req = conn.req();
    let first = conn
        .call(ClientMsg::TermAttach { req: Some(req), id }, req)
        .await?;
    // Bytes of replay still to arrive; with --new they are dropped rather than printed.
    let mut replay_left = match first {
        ServerMsg::TermAttached { replay_bytes, .. } if new => replay_bytes,
        _ => 0,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    let mut finder = until.as_deref().map(wheel_proto::TextFinder::new);
    let mut out = std::io::stdout();
    loop {
        // Always drain what is already queued, even with --timeout 0.
        let left = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(Duration::from_millis(20));
        match conn.recv(left).await? {
            None => {
                return match &until {
                    Some(n) => Err(Fail::Timeout(format!("timed out before {n:?} appeared"))),
                    None => Ok(()),
                }
            }
            Some(ServerMsg::TermOutput { data, .. }) => {
                let mut bytes = decode_bytes(&data).map_err(other)?;
                let skip = replay_left.min(bytes.len());
                replay_left -= skip;
                bytes.drain(..skip);
                out.write_all(&bytes).map_err(other)?;
                out.flush().map_err(other)?;
                if finder.as_mut().is_some_and(|f| f.feed(&bytes)) {
                    return Ok(());
                }
            }
            Some(ServerMsg::TermAttached { replay_bytes, .. }) => {
                // The daemon dropped output to this connection and is replaying; start over.
                eprintln!("\n[output resynced; replaying {replay_bytes} bytes]");
                if let Some(f) = finder.as_mut() {
                    f.clear();
                }
                replay_left = if new { replay_bytes } else { 0 };
            }
            Some(ServerMsg::TermExited { code, .. }) => {
                eprintln!("\n[terminal exited: {code:?}]");
                return match until {
                    Some(_) => Err(Fail::Exited(code)),
                    None => Ok(()),
                };
            }
            Some(_) => {}
        }
        if tokio::time::Instant::now() >= deadline && until.is_none() {
            return Ok(());
        }
    }
}

fn print_perm_hint(perm: &wheel_proto::PermInfo) {
    eprintln!(
        "\n[waiting on you] {}: {} wants {:?} -- wheel perm answer {} allow|deny",
        perm.id, perm.session, perm.title, perm.id
    );
}

async fn session_send(
    conn: &mut Conn,
    session: String,
    text: String,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    // The daemon resolves a pane id to its session, and events carry the session's name.
    let ServerMsg::SessionAttached { session, .. } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    let req = conn.req();
    let ServerMsg::SessionQueued { turn, .. } = conn
        .call(
            ClientMsg::SessionSend {
                req: Some(req),
                session: session.clone(),
                text,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_send".into()));
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    let mut out = std::io::stdout();
    // Our turn may wait behind others; it starts with the `turn_start` event carrying its id.
    let mut started = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = conn.recv(left).await? else {
            return Err(Fail::Timeout(format!(
                "{session} did not finish within {timeout}s"
            )));
        };
        match msg {
            ServerMsg::Session { session: s, event } if s == session => match event.event {
                SessionEvent::TurnStart { turn: t } if t == turn => started = true,
                SessionEvent::Text { text } if started => {
                    out.write_all(text.as_bytes()).map_err(other)?;
                    out.flush().map_err(other)?;
                }
                SessionEvent::ToolCall { title, status, .. } if started => {
                    eprintln!("\n[tool] {title} ({status})")
                }
                SessionEvent::TurnEnd {
                    turn: t,
                    stop_reason,
                } if t == turn => {
                    println!();
                    if stop_reason != "end_turn" {
                        eprintln!("[turn ended: {stop_reason}]");
                    }
                    return Ok(());
                }
                SessionEvent::Error {
                    turn: Some(t),
                    message,
                } if t == turn => return Err(Fail::Daemon(message)),
                // The agent process stopped. A turn in flight is lost; a queued one runs after the
                // restart, unless the session itself ended (its pane closed), which runs nothing more.
                SessionEvent::Error {
                    turn: None,
                    message,
                } if started || message == wheel_proto::SESSION_ENDED => {
                    return Err(Fail::Daemon(message))
                }
                _ => {}
            },
            ServerMsg::PermRequested { perm } => print_perm_hint(&perm),
            _ => {}
        }
    }
}

/// Wait for a session to be ready; print its pane, session and state.
async fn session_wait(conn: &mut Conn, session: String, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::SessionWaited {
        session,
        pane,
        state,
        ready,
        ..
    } = conn
        .call_within(
            ClientMsg::SessionWait {
                req: Some(req),
                session,
                timeout_secs: timeout,
            },
            req,
            Duration::from_secs_f64(timeout) + Duration::from_secs(10),
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_wait".into()));
    };
    let state = serde_json::to_string(&state).map_err(other)?;
    if !ready {
        return Err(Fail::Timeout(format!(
            "{session} was not ready within {timeout}s: {state}"
        )));
    }
    println!("{} {session} {state}", pane.as_deref().unwrap_or("-"));
    Ok(())
}

/// Every agent pane's session name in a layout, in pane order.
fn agent_sessions(node: &wheel_proto::Node, out: &mut Vec<String>) {
    match node {
        wheel_proto::Node::Pane {
            kind: wheel_proto::PaneKind::Agent { session, .. },
            ..
        } => out.push(session.clone()),
        wheel_proto::Node::Pane { .. } => {}
        wheel_proto::Node::Split { a, b, .. } => {
            agent_sessions(a, out);
            agent_sessions(b, out);
        }
    }
}

/// One session's context use, read from its attach reply.
async fn context_of(
    conn: &mut Conn,
    session: String,
) -> Result<(String, Option<wheel_proto::ContextUse>), Fail> {
    let req = conn.req();
    let ServerMsg::SessionAttached {
        session, context, ..
    } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    Ok((session, context))
}

async fn session_context(conn: &mut Conn, session: Option<String>, json: bool) -> Result<(), Fail> {
    let names = match session {
        Some(session) => vec![session],
        None => {
            let req = conn.req();
            let ServerMsg::Status { spaces, .. } =
                conn.call(ClientMsg::Status { req: Some(req) }, req).await?
            else {
                return Err(Fail::Other("unexpected reply to status".into()));
            };
            let mut names = Vec::new();
            for space in &spaces {
                agent_sessions(&space.root, &mut names);
            }
            names
        }
    };
    let mut rows = Vec::new();
    for name in names {
        rows.push(context_of(conn, name).await?);
    }
    if json {
        let all: Vec<_> = rows
            .iter()
            .map(|(session, context)| serde_json::json!({ "session": session, "context": context }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    for (session, context) in rows {
        match context {
            Some(c) => println!("{session} {}/{} tokens ({}%)", c.used, c.size, c.percent()),
            None => println!("{session} unknown"),
        }
    }
    Ok(())
}

/// Print a session's config options, or set one. The agent names them, so there is no fixed list:
/// whatever it offers is what prints and what can be set.
async fn session_config(
    conn: &mut Conn,
    session: String,
    config: Option<String>,
    value: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    if let Some(config) = config {
        // Listing on a half-typed set would hide the typo behind output that looks like a success.
        let Some(value) = value else {
            return Err(Fail::Other(format!(
                "give a value to set {config} to, or no arguments to list the options"
            )));
        };
        return simple(conn, |req| ClientMsg::SessionConfigSet {
            req: Some(req),
            session,
            config,
            value,
        })
        .await;
    }
    let req = conn.req();
    let ServerMsg::SessionAttached { config, state, .. } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session: session.clone(),
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    // While the session is starting or failed, the list is empty or the dead agent's: either way
    // a restart in progress, not what the agent offers.
    match state {
        SessionState::Starting | SessionState::Failed { .. } => {
            return Err(Fail::Other(format!(
                "{session}'s agent is restarting; its config options come back once it is ready"
            )));
        }
        SessionState::GaveUp { tries, message } => {
            return Err(Fail::Other(format!(
                "{session}'s agent gave up after {tries} tries: {message}"
            )));
        }
        SessionState::Switching => {
            return Err(Fail::Other(format!(
                "{session} is switching to the terminal view"
            )));
        }
        SessionState::Ready | SessionState::Busy { .. } => {}
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&config).map_err(other)?);
        return Ok(());
    }
    if config.is_empty() {
        println!("{session} offers no config options");
        return Ok(());
    }
    for option in config {
        match option.kind {
            ConfigKind::Select { current, choices } => {
                println!("{} = {current}  ({})", option.id, option.name);
                for choice in choices {
                    let mark = if choice.value == current { "*" } else { " " };
                    println!("  {mark} {} — {}", choice.value, choice.name);
                }
            }
            ConfigKind::Boolean { current } => {
                println!("{} = {current}  ({})", option.id, option.name);
                println!("    true\n    false");
            }
            ConfigKind::Unknown => {
                println!(
                    "{}  ({}) — this wheel cannot set it",
                    option.id, option.name
                )
            }
        }
    }
    Ok(())
}

/// Print the conversations a session's agent holds for its directory, newest first, marking the
/// one the session is in with `*`.
async fn session_conversations(conn: &mut Conn, session: String, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Conversations { conversations, .. } = conn
        .call_within(
            ClientMsg::SessionConversationLs {
                req: Some(req),
                session: session.clone(),
            },
            req,
            Duration::from_secs(60),
        )
        .await?
    else {
        return Err(Fail::Other(
            "unexpected reply to session_conversation_ls".into(),
        ));
    };
    // The agent's store belongs to the directory, not to wheel: anything else running the same
    // harness there keeps its conversations in the same place. Saying so beats a filter, since
    // nothing in an entry says which program made it. On stderr, so stdout stays the listing —
    // and so a caller reading the JSON is told too.
    if !conversations.is_empty() {
        eprintln!(
            "[these are every conversation the agent holds for that directory, including ones \
             started outside wheel — a terminal Claude Code session in it, say]"
        );
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&conversations).map_err(other)?
        );
        return Ok(());
    }
    if conversations.is_empty() {
        eprintln!("[{session}'s agent holds no conversations for its directory]");
        return Ok(());
    }
    for conversation in &conversations {
        let mark = if conversation.current { "*" } else { " " };
        println!(
            "{mark} {}  {}  {}",
            conversation.id,
            conversation.updated_at.as_deref().unwrap_or("-"),
            conversation.title.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// Put a session in a fresh conversation, or back in the past one `load` names, and print the
/// agent's id for it.
async fn session_switch(
    conn: &mut Conn,
    session: String,
    load: Option<String>,
) -> Result<(), Fail> {
    let req = conn.req();
    let msg = match load {
        Some(id) => ClientMsg::SessionConversationResume {
            req: Some(req),
            session,
            id,
        },
        None => ClientMsg::SessionConversationNew {
            req: Some(req),
            session,
        },
    };
    // A real agent takes up to a minute to load a conversation and replay it.
    let reply = conn.call_within(msg, req, Duration::from_secs(120)).await?;
    let ServerMsg::Conversation { id, .. } = reply else {
        return Err(Fail::Other(format!("unexpected reply {reply:?}")));
    };
    println!("{id}");
    Ok(())
}

async fn session_read(conn: &mut Conn, session: String, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::SessionAttached {
        session,
        state,
        events,
        ..
    } = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to session_attach".into()));
    };
    if json {
        let all = serde_json::json!({ "state": state, "events": events });
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    // A `user` event arrives when the message is queued; `turn_start` says it began running.
    let waiting = wheel_proto::queued_turns(&events);
    // A call is announced `pending` and finished by later `tool_update` events; print where it ended.
    let mut latest: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();
    for event in &events {
        if let SessionEvent::ToolUpdate {
            id, title, status, ..
        } = &event.event
        {
            let entry = latest.entry(id.clone()).or_default();
            if title.is_some() {
                entry.0 = title.clone();
            }
            if status.is_some() {
                entry.1 = status.clone();
            }
        }
    }
    for event in events {
        match event.event {
            SessionEvent::User { turn, text } => {
                let queued = if waiting.contains(&turn) {
                    " (queued)"
                } else {
                    ""
                };
                println!("\n> {text}{queued}")
            }
            SessionEvent::Text { text } => println!("{text}"),
            SessionEvent::Thought { text } => println!("[thought] {text}"),
            SessionEvent::ToolCall {
                id,
                title,
                status,
                content,
                ..
            } => {
                let (new_title, new_status) = latest.remove(&id).unwrap_or_default();
                let title = new_title.unwrap_or(title);
                let status = new_status.unwrap_or(status);
                println!("[tool] {title} ({status})");
                print_tool_content(&content);
            }
            SessionEvent::ToolUpdate {
                id,
                content: Some(content),
                ..
            } => {
                println!("[tool {id}] content now:");
                print_tool_content(&content);
            }
            SessionEvent::Mode { mode } => println!("[mode] {mode}"),
            SessionEvent::Error { message, .. } => println!("[error] {message}"),
            _ => {}
        }
    }
    eprintln!(
        "[{session}: {}]",
        serde_json::to_string(&state).map_err(other)?
    );
    Ok(())
}

async fn session_cancel(conn: &mut Conn, session: String) -> Result<(), Fail> {
    simple(conn, |req| ClientMsg::SessionCancel {
        req: Some(req),
        session,
    })
    .await
}

/// Text indented under its tool row; a diff by its path and line counts, never its text.
fn print_tool_content(content: &[ToolContent]) {
    for item in content {
        match item {
            ToolContent::Text { text } => {
                for line in text.lines() {
                    println!("  {line}");
                }
            }
            ToolContent::Diff {
                path,
                old_text,
                new_text,
            } => println!(
                "  diff {path} (-{} +{} lines)",
                old_text.as_deref().map_or(0, |t| t.lines().count()),
                new_text.lines().count()
            ),
            ToolContent::Unknown => println!("  (content this wheel cannot show)"),
        }
    }
}

async fn perm_list(conn: &mut Conn) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    println!("{}", serde_json::to_string_pretty(&perms).map_err(other)?);
    Ok(())
}

/// The id of the waiting request `target` names: a request id as it is, else the one request the
/// agent pane or session `target` has waiting.
async fn perm_id(conn: &mut Conn, target: String) -> Result<String, Fail> {
    let req = conn.req();
    let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    if perms.iter().any(|p| p.id == target) {
        return Ok(target);
    }
    let req = conn.req();
    let attached = conn
        .call(
            ClientMsg::SessionAttach {
                req: Some(req),
                session: target.clone(),
            },
            req,
        )
        .await;
    let Ok(ServerMsg::SessionAttached { session, .. }) = attached else {
        // Not a pane or session either: the daemon says no such request when it is asked.
        return Ok(target);
    };
    let mut waiting = perms.iter().filter(|p| p.session == session);
    match (waiting.next(), waiting.next()) {
        (Some(p), None) => Ok(p.id.clone()),
        (None, _) => Err(Fail::Daemon(format!(
            "{target} has no permission request waiting"
        ))),
        (Some(_), Some(_)) => Err(Fail::Daemon(format!(
            "{target} has several permission requests waiting; name one by its id (wheel perm list)"
        ))),
    }
}

async fn perm_wait(conn: &mut Conn, timeout: f64) -> Result<(), Fail> {
    let req = conn.req();
    if let ServerMsg::Status { perms, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    {
        if let Some(perm) = perms.into_iter().next() {
            println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
            return Ok(());
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no permission request within {timeout}s"
                )))
            }
            Some(ServerMsg::PermRequested { perm }) => {
                println!("{}", serde_json::to_string_pretty(&perm).map_err(other)?);
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

async fn wait_reload(conn: &mut Conn, pane: String, timeout: f64) -> Result<(), Fail> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no change in {pane}'s folder within {timeout}s"
                )))
            }
            Some(ServerMsg::PaneReload { pane: changed }) if changed == pane => {
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(other)?
                    .as_millis();
                println!("pane_reload {pane} at {at}");
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

/// Wait for the next settings broadcast (a frame with no `req`, which a reply never is) and print
/// it. The space is the one the daemon resolved the list for, `machine-wide` when it resolved
/// against none, so a script can tell a machine write from a project write.
async fn config_watch(conn: &mut Conn, timeout: f64, json: bool) -> Result<(), Fail> {
    // The connection is up and the broadcast will be heard: a script that writes after this line
    // cannot miss it.
    eprintln!("watching settings");
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.recv(left).await? {
            None => {
                return Err(Fail::Timeout(format!(
                    "no settings broadcast within {timeout}s"
                )))
            }
            Some(ServerMsg::Settings {
                req: None,
                space,
                settings,
            }) => {
                if json {
                    let record = serde_json::json!({ "space": space, "settings": settings });
                    println!("{}", serde_json::to_string_pretty(&record).map_err(other)?);
                    return Ok(());
                }
                println!(
                    "settings space={}",
                    space.as_deref().unwrap_or("machine-wide")
                );
                for s in &settings {
                    let origin = match s.origin {
                        SettingOrigin::Project => "project",
                        SettingOrigin::Machine => "machine",
                        SettingOrigin::Default => "default",
                    };
                    println!("{} = {}   ({origin})", s.key, s.value);
                    if let Some(problem) = &s.problem {
                        println!("  problem: {problem}");
                    }
                }
                return Ok(());
            }
            Some(_) => {}
        }
    }
}

async fn simple(conn: &mut Conn, make: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    conn.call(make(req), req).await?;
    Ok(())
}

/// Send a request that makes something and print the new id.
async fn create(conn: &mut Conn, make: impl FnOnce(u64) -> ClientMsg) -> Result<(), Fail> {
    let req = conn.req();
    match conn.call(make(req), req).await? {
        ServerMsg::Created { id, .. } => {
            println!("{id}");
            Ok(())
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

async fn space_ls(conn: &mut Conn, flat: bool) -> Result<(), Fail> {
    fn pane_ids(node: &wheel_proto::Node, out: &mut Vec<String>) {
        match node {
            wheel_proto::Node::Pane { id, .. } => out.push(id.clone()),
            wheel_proto::Node::Split { a, b, .. } => {
                pane_ids(a, out);
                pane_ids(b, out);
            }
        }
    }
    let req = conn.req();
    let ServerMsg::Status {
        spaces,
        active,
        watches,
        ..
    } = conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    if flat {
        let rows: Vec<_> = spaces
            .iter()
            .map(|space| {
                let mut panes = Vec::new();
                pane_ids(&space.root, &mut panes);
                serde_json::json!({
                    "id": space.id, "name": space.name, "cwd": space.cwd,
                    "status": space.status, "panes": panes,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).map_err(other)?);
        return Ok(());
    }
    // Each web pane's node names only its artifact's scope and name; the daemon owns the roots, so
    // the folder it resolved comes back with the watch rather than being recomputed here.
    let layout = serde_json::json!({ "spaces": spaces, "active": active, "web_panes": watches });
    println!("{}", serde_json::to_string_pretty(&layout).map_err(other)?);
    Ok(())
}

/// A status as `space status` prints it: its name, and for `needs_you` how many requests wait.
fn status_words(status: Option<wheel_proto::PaneStatus>) -> String {
    use wheel_proto::PaneStatus;
    match status {
        None => "-".into(),
        Some(PaneStatus::NeedsYou { waiting }) => format!("needs_you  waiting {waiting}"),
        Some(PaneStatus::Stopped) => "stopped".into(),
        Some(PaneStatus::Working) => "working".into(),
        Some(PaneStatus::Idle) => "idle".into(),
    }
}

/// Each space's status and waiting count, with one indented line per pane.
async fn space_status(conn: &mut Conn) -> Result<(), Fail> {
    fn walk(
        node: &wheel_proto::Node,
        out: &mut Vec<(String, String, Option<wheel_proto::PaneStatus>)>,
    ) {
        match node {
            wheel_proto::Node::Pane { id, kind, status } => {
                let kind = serde_json::to_value(kind)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                out.push((id.clone(), kind, *status));
            }
            wheel_proto::Node::Split { a, b, .. } => {
                walk(a, out);
                walk(b, out);
            }
        }
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    for space in &spaces {
        println!("{}  {}", space.name, status_words(space.status));
        let mut panes = Vec::new();
        walk(&space.root, &mut panes);
        for (pane, kind, status) in panes {
            println!("  {pane}  {kind}  {}", status_words(status));
        }
    }
    Ok(())
}

/// One artifact for a person to read: what it is, where the daemon resolved its page to, which
/// wheel.toml holds its entry, what the entry asks for, and every problem it has — a problem is part of the record, so it
/// prints here as well as in `--json`, never only in the daemon's log.
fn print_artifact(a: &wheel_proto::ArtifactInfo) {
    // The flag is on the name's own line, because what it answers — would a pane picker offer
    // this? — is a property of the artifact rather than a line of its configuration.
    let offered = if a.pane { "  (in the pane picker)" } else { "" };
    println!("{} {} — {}{offered}", a.scope.name(), a.name, a.title);
    if let Some(renderer) = &a.renderer {
        println!("  a post, drawn by {renderer}");
    }
    if let Some(pane) = &a.reply_to {
        println!("  Send types its answer into pane {pane}");
    }
    if let Some(url) = &a.webhook {
        println!(
            "  Send posts to {}",
            url.split(['?', '#']).next().unwrap_or_default()
        );
    }
    match &a.page {
        Some(page) => println!("  {}/{page}", a.dir.trim_end_matches('/')),
        None => println!("  {}", a.dir),
    }
    println!("  entry in {}", a.entry);
    if let Some(pane) = &a.ends_with_pane {
        println!("  deleted when the session in pane {pane} ends");
    }
    if let Some(at) = a.expires_at {
        println!("  deleted at {at} (Unix seconds)");
    }
    match (&a.command, a.every) {
        (Some(command), Some(secs)) => println!("  refresh: {command}  (every {secs}s)"),
        (Some(command), None) => println!("  refresh: {command}  (on open and on demand)"),
        (None, Some(secs)) => println!("  every {secs}s"),
        (None, None) => {}
    }
    for problem in &a.problems {
        println!("  problem: {problem}");
    }
}

/// Send a request about one artifact and take the record it replies with.
async fn artifact_of(
    conn: &mut Conn,
    msg: impl FnOnce(u64) -> ClientMsg,
) -> Result<wheel_proto::ArtifactInfo, Fail> {
    let req = conn.req();
    match conn.call(msg(req), req).await? {
        ServerMsg::Artifact { artifact, .. } => Ok(artifact),
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// A length of time as whole seconds: a whole number and one of s, m, h, d.
fn parse_duration(text: &str) -> Result<u64, String> {
    let bad =
        || format!("{text:?} is not a duration: a whole number and s, m, h or d, like 30m or 2h");
    let split = text.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(bad()),
    };
    match n.checked_mul(unit) {
        Some(0) => Err("a duration must be longer than zero".into()),
        Some(secs) => Ok(secs),
        None => Err(bad()),
    }
}

/// Make an artifact and print its folder's absolute path, the one thing a script needs next.
#[allow(clippy::too_many_arguments)]
async fn artifact_new(
    conn: &mut Conn,
    name: String,
    pane: bool,
    scope: Option<ArtifactScope>,
    space: Option<String>,
    path: Option<PathBuf>,
    ends_with_pane: Option<String>,
    expires_in: Option<u64>,
    renderer: Option<String>,
    files: Vec<PathBuf>,
    reply_to: Option<String>,
    webhook: Option<String>,
) -> Result<(), Fail> {
    let path = path.map(from_here).transpose()?;
    let files = post_files(files)?;
    let artifact = artifact_of(conn, |req| ClientMsg::ArtifactNew {
        req: Some(req),
        pane,
        name,
        scope,
        space,
        path: path.map(|p| p.display().to_string()),
        ends_with_pane,
        expires_in,
        renderer,
        files,
        reply_to,
        webhook,
    })
    .await?;
    println!("{}", artifact.dir);
    eprintln!(
        "[{} artifact {} — {}]",
        artifact.scope.name(),
        artifact.name,
        artifact.title
    );
    Ok(())
}

/// What a post holds: each file given, under its own name, and for a markdown file every local
/// image it points at (`![alt](path)`), under the path the markdown writes it with, so the
/// copied post reads as the original did.
fn post_files(files: Vec<PathBuf>) -> Result<Vec<wheel_proto::PostFile>, Fail> {
    let mut out: Vec<wheel_proto::PostFile> = Vec::new();
    let mut add = |name: String, from: PathBuf| -> Result<(), Fail> {
        if let Some(have) = out.iter().find(|f| f.name == name) {
            if have.from == from.display().to_string() {
                return Ok(());
            }
            return Err(Fail::Other(format!(
                "{name} would be in the post twice, from {} and {}",
                have.from,
                from.display()
            )));
        }
        out.push(wheel_proto::PostFile {
            name,
            from: from.display().to_string(),
        });
        Ok(())
    };
    for file in files {
        let file = from_here(file)?;
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Fail::Other(format!("{} names no file", file.display())))?;
        if !file.is_file() {
            return Err(Fail::Other(format!("{} is not a file", file.display())));
        }
        if file
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| Fail::Other(format!("{}: {e}", file.display())))?;
            let dir = file.parent().map(PathBuf::from).unwrap_or_default();
            for reference in markdown_images(&text) {
                let from = dir.join(&reference);
                if !from.is_file() {
                    return Err(Fail::Other(format!(
                        "{} points at {reference}, which is not a file ({})",
                        file.display(),
                        from.display()
                    )));
                }
                add(reference, from)?;
            }
        }
        add(name, file)?;
    }
    Ok(out)
}

/// The local files a markdown text shows as images: the target of each line that is only
/// `![alt](target)` (the page draws no other image), outside a fenced code block, less a quoted
/// title, a `?query` and a `#fragment` and a leading `./`; a URL, an absolute path and anything
/// with `..` is skipped.
fn markdown_images(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let Some(inner) = line
            .strip_prefix("![")
            .and_then(|rest| rest.split_once("]("))
            .and_then(|(alt, rest)| (!alt.contains(']')).then_some(rest))
            .and_then(|rest| rest.strip_suffix(')'))
        else {
            continue;
        };
        if fenced {
            continue;
        }
        let mut target = inner.trim();
        for quote in ['"', '\''] {
            if let Some(at) = target.find(&format!(" {quote}")) {
                if target.ends_with(quote) {
                    target = target[..at].trim_end();
                }
            }
        }
        let mut target = target.split(['?', '#']).next().unwrap_or("");
        while let Some(rest) = target.strip_prefix("./") {
            target = rest;
        }
        let local = !target.is_empty()
            && !target.contains("://")
            && !target.starts_with('/')
            && !target.starts_with("data:")
            && !target.split('/').any(|part| part == "..");
        if local && !found.iter().any(|f| f == target) {
            found.push(target.to_string());
        }
    }
    found
}

/// A path to send the daemon as it was written, made absolute — a relative one means this
/// directory rather than the daemon's — with its links left for the daemon to follow, so a folder
/// that is not there yet can be named and a link keeps its own name. [`absolute`] resolves one.
fn from_here(path: PathBuf) -> Result<PathBuf, Fail> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()
        .map_err(|e| Fail::Other(format!("read the current directory: {e}")))?
        .join(path))
}

/// Register an existing page and print its record: the page's path on stdout for a script, like
/// `artifact new`, or the whole record as JSON.
#[allow(clippy::too_many_arguments)]
async fn artifact_add(
    conn: &mut Conn,
    path: PathBuf,
    name: Option<String>,
    scope: Option<ArtifactScope>,
    space: Option<String>,
    title: Option<String>,
    command: Option<String>,
    every: Option<u64>,
    pane: bool,
    json: bool,
) -> Result<(), Fail> {
    let path = from_here(path)?.display().to_string();
    let artifact = artifact_of(conn, |req| ClientMsg::ArtifactAdd {
        req: Some(req),
        path,
        name,
        scope,
        space,
        title,
        command,
        every,
        pane,
    })
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&artifact).map_err(other)?
        );
    } else {
        print_artifact(&artifact);
    }
    Ok(())
}

/// Delete an artifact's entry: the folder it named, which is left as it was, on stdout for a
/// script, and which entry went from which file on stderr.
async fn artifact_remove(
    conn: &mut Conn,
    name: String,
    scope: Option<ArtifactScope>,
    space: Option<String>,
) -> Result<(), Fail> {
    let was = artifact_of(conn, |req| ClientMsg::ArtifactRemove {
        req: Some(req),
        name,
        scope,
        space,
    })
    .await?;
    println!("{}", was.dir);
    eprintln!(
        "[removed {} artifact {} from {}; its folder is left as it was]",
        was.scope.name(),
        was.name,
        was.entry
    );
    Ok(())
}

/// One line per built-in: `<name>  shipped N  installed M|not installed`, then one indented line
/// per edited file.
fn print_builtins(builtins: &[wheel_proto::BuiltinInfo]) {
    for b in builtins {
        let installed = b
            .installed_version
            .map_or("not installed".to_string(), |v| format!("installed {v}"));
        println!("{}  shipped {}  {installed}", b.name, b.shipped_version);
        for file in &b.edited_files {
            println!("  edited: {file}");
        }
    }
}

/// Run one built-in request and print every built-in as it now stands. An update that found
/// edited files and was given no choice prints their diffs and exits 1.
async fn builtin(conn: &mut Conn, command: BuiltinCommand) -> Result<(), Fail> {
    let json = matches!(command, BuiltinCommand::Ls { json: true });
    let req = conn.req();
    let msg = match command {
        BuiltinCommand::Ls { .. } => ClientMsg::BuiltinLs { req: Some(req) },
        BuiltinCommand::Install { name } => ClientMsg::BuiltinInstall {
            req: Some(req),
            name,
        },
        BuiltinCommand::Update {
            name,
            keep_edited,
            replace_edited,
        } => ClientMsg::BuiltinUpdate {
            req: Some(req),
            name,
            on_edited: match (keep_edited, replace_edited) {
                (true, _) => wheel_proto::OnEdited::Keep,
                (_, true) => wheel_proto::OnEdited::Replace,
                _ => wheel_proto::OnEdited::Ask,
            },
        },
        BuiltinCommand::Remove { name } => ClientMsg::BuiltinRemove {
            req: Some(req),
            name,
        },
    };
    match conn.call(msg, req).await? {
        ServerMsg::Builtins { builtins, .. } if json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&builtins).map_err(other)?
            );
            Ok(())
        }
        ServerMsg::Builtins { builtins, .. } => {
            print_builtins(&builtins);
            Ok(())
        }
        ServerMsg::BuiltinEdited { name, edited, .. } => {
            for file in &edited {
                print!("{}", file.diff);
            }
            Err(Fail::Other(format!(
                "{name} has {} edited file{}; run again with --keep-edited or --replace-edited",
                edited.len(),
                if edited.len() == 1 { "" } else { "s" }
            )))
        }
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// Ask the daemon for every setting as it resolves for `space`, or write one first.
async fn settings(
    conn: &mut Conn,
    msg: impl FnOnce(u64) -> ClientMsg,
) -> Result<Vec<SettingInfo>, Fail> {
    let req = conn.req();
    match conn.call(msg(req), req).await? {
        ServerMsg::Settings { settings, .. } => Ok(settings),
        other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
    }
}

/// One setting for a person to read: the value in force, and which wheel.toml it came from, so a
/// value that is not the one they expected says where to go and change it.
fn print_setting(s: &SettingInfo) {
    let origin = match s.origin {
        SettingOrigin::Project => "this project's wheel.toml",
        SettingOrigin::Machine => "this machine's wheel.toml",
        SettingOrigin::Default => "wheel's default",
    };
    println!("{} = {}   ({origin})", s.key, s.value);
    println!("  {}", s.help);
    // What removing the project's value would bring back.
    if let (SettingOrigin::Project, Some(machine)) = (s.origin, &s.machine) {
        println!("  this machine's: {machine}");
    }
    if s.origin != SettingOrigin::Default {
        println!("  default: {}", s.default);
    }
    // A file wheel could not use is why the value above is not the one the person wrote, so it is
    // printed beside the value rather than left in the daemon's log.
    if let Some(problem) = &s.problem {
        println!("  problem: {problem}");
    }
}

async fn config_ls(conn: &mut Conn, space: Option<String>, json: bool) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingsLs {
        req: Some(req),
        space,
    })
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    for s in &all {
        print_setting(s);
    }
    Ok(())
}

/// `config path`: where the machine's `wheel.toml` and the space's are, as the daemon sees them.
async fn config_path(conn: &mut Conn, space: Option<String>, json: bool) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingsLs {
        req: Some(req),
        space,
    })
    .await?;
    let first = all.first();
    let machine = first.map(|s| s.machine_file.clone()).unwrap_or_default();
    let project = first.and_then(|s| s.project_file.clone());
    if json {
        let record = serde_json::json!({ "machine": machine, "project": project });
        println!("{}", serde_json::to_string_pretty(&record).map_err(other)?);
        return Ok(());
    }
    println!("machine: {machine}");
    match project {
        Some(project) => println!("project: {project}"),
        None => println!("project: none (no space)"),
    }
    Ok(())
}

/// `config homes`: the daemon's candidate home folders for an agent and each one's state.
async fn config_homes(
    conn: &mut Conn,
    agent: String,
    path: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::AgentHomes {
        homes,
        checked,
        error,
        ..
    } = conn
        .call(
            ClientMsg::AgentHomes {
                req: Some(req),
                agent,
                path,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to agent_homes".into()));
    };
    if let Some(error) = error {
        return Err(Fail::Daemon(error));
    }
    if json {
        let record = serde_json::json!({ "homes": homes, "checked": checked });
        println!("{}", serde_json::to_string_pretty(&record).map_err(other)?);
        return Ok(());
    }
    let state = |h: &wheel_proto::AgentHome| {
        serde_json::to_value(h.state)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    };
    for home in &homes {
        println!("{}  {}", state(home), home.path);
    }
    if let Some(checked) = &checked {
        println!("checked: {}  {}", state(checked), checked.path);
    }
    Ok(())
}

/// One setting's value on stdout and nothing else, so `$(wheel config get <key>)` is the value.
async fn config_get(
    conn: &mut Conn,
    key: String,
    space: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingsLs {
        req: Some(req),
        space,
    })
    .await?;
    let Some(s) = all.iter().find(|s| s.key == key) else {
        let known: Vec<&str> = all.iter().map(|s| s.key.as_str()).collect();
        return Err(Fail::Daemon(format!(
            "no setting {key}. Wheel's settings are: {}",
            known.join(", ")
        )));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(s).map_err(other)?);
    } else {
        println!("{}", s.value);
    }
    Ok(())
}

async fn config_set(
    conn: &mut Conn,
    key: String,
    value: String,
    space: Option<String>,
    machine: bool,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingSet {
        req: Some(req),
        key: key.clone(),
        value,
        space,
        machine,
    })
    .await?;
    print_changed(&key, &all, json)
}

async fn config_unset(
    conn: &mut Conn,
    key: String,
    space: Option<String>,
    machine: bool,
    json: bool,
) -> Result<(), Fail> {
    let all = settings(conn, |req| ClientMsg::SettingUnset {
        req: Some(req),
        key: key.clone(),
        space,
        machine,
    })
    .await?;
    print_changed(&key, &all, json)
}

/// After a write or a removal: the settings as JSON, or the one changed.
fn print_changed(key: &str, all: &[SettingInfo], json: bool) -> Result<(), Fail> {
    if json {
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    // The one that was written, as it now resolves: a project write under a machine value still
    // wins, and a machine write under a project value does not, which this is how you see. The
    // daemon refuses a key it does not have and answers with every key it does, so a reply missing
    // the key it just wrote is a daemon that disagrees with this build about what settings are —
    // which is worth saying, not worth printing the whole list over.
    let Some(s) = all.iter().find(|s| s.key == key) else {
        return Err(Fail::Other(format!(
            "the daemon changed {key} and then reported {} settings without it",
            all.len()
        )));
    };
    print_setting(s);
    Ok(())
}

async fn artifact_ls(
    conn: &mut Conn,
    space: Option<String>,
    pane: bool,
    json: bool,
) -> Result<(), Fail> {
    let req = conn.req();
    let ServerMsg::Artifacts {
        artifacts,
        resolves,
        ..
    } = conn
        .call(
            ClientMsg::ArtifactLs {
                req: Some(req),
                space,
                pane,
            },
            req,
        )
        .await?
    else {
        return Err(Fail::Other("unexpected reply to artifact_ls".into()));
    };
    if json {
        let all = serde_json::json!({ "artifacts": artifacts, "resolves": resolves });
        println!("{}", serde_json::to_string_pretty(&all).map_err(other)?);
        return Ok(());
    }
    if artifacts.is_empty() {
        if pane {
            eprintln!("[no artifact in any scope asks for the pane picker]");
        } else {
            eprintln!("[no artifacts in any scope]");
        }
        return Ok(());
    }
    for artifact in &artifacts {
        print_artifact(artifact);
    }
    println!("a bare name resolves to:");
    for (name, scope) in &resolves {
        println!("  {name} -> {}", scope.name());
    }
    Ok(())
}

async fn artifact_show(
    conn: &mut Conn,
    name: String,
    scope: Option<ArtifactScope>,
    space: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    let artifact = artifact_of(conn, |req| ClientMsg::ArtifactShow {
        req: Some(req),
        name,
        scope,
        space,
    })
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&artifact).map_err(other)?
        );
    } else {
        print_artifact(&artifact);
    }
    Ok(())
}

/// One pane's refresh runner for a person to read: what it runs where, how many runs it has made,
/// and what the last one did — the exit status and the tail of its error output, so a failing
/// refresh is diagnosable without reproducing it.
fn print_refresh(r: &wheel_proto::RefreshInfo) {
    println!("{} {} {} — {}", r.pane, r.scope.name(), r.name, r.command);
    match r.timer {
        TimerState::Ticking => println!(
            "  on open, on demand and every {}s",
            r.every.unwrap_or_default()
        ),
        TimerState::BackingOff => println!(
            "  on open and on demand; every {}s asked, but the last runs failed the same way, so the timer waits {}s between runs until one works",
            r.every.unwrap_or_default(),
            r.backoff_secs.unwrap_or_default()
        ),
        TimerState::Idle => println!(
            "  on open and on demand; the timer is skipping every tick — the entry asks for no command or no interval"
        ),
        TimerState::Stopped => println!(
            "  on open and on demand; this pane's timer is gone until the pane shows the artifact again"
        ),
        // An entry that gained `every` after the pane opened gets its timer at the next open, as
        // one that gained a `command` does.
        TimerState::Off => match r.every {
            Some(secs) => {
                println!("  on open and on demand; every {secs}s starts when the pane next shows it")
            }
            None => println!("  on open and on demand"),
        },
    }
    println!("  in {}", r.cwd);
    println!("  writes {}", r.data_dir);
    println!("  runs: {}", r.runs);
    let Some(last) = &r.last else {
        println!("  no run has finished yet");
        return;
    };
    match (last.exit, &last.error) {
        (Some(code), _) => println!(
            "  last run at {}: exit {code} in {}ms",
            last.started_ms, last.duration_ms
        ),
        (None, Some(why)) => println!(
            "  last run at {}: did not run — {why} ({}ms)",
            last.started_ms, last.duration_ms
        ),
        (None, None) => println!("  last run at {}: no result", last.started_ms),
    }
    for line in last.stderr_tail.lines() {
        println!("  stderr: {line}");
    }
}

/// Run a pane's refresh now. Exits non-zero when the run failed, so a check reads fresh data or
/// knows it did not.
async fn artifact_refresh(
    conn: &mut Conn,
    pane: String,
    json: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    eprintln!("[refreshing {pane}]");
    let reply = conn
        .call_within(
            ClientMsg::ArtifactRefresh {
                req: Some(req),
                pane: pane.clone(),
            },
            req,
            Duration::from_secs_f64(timeout),
        )
        .await?;
    let ServerMsg::Refresh { refresh, .. } = reply else {
        return Err(Fail::Other("unexpected reply to artifact_refresh".into()));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&refresh).map_err(other)?);
    } else {
        print_refresh(&refresh);
    }
    match &refresh.last {
        Some(run) if run.ok() => Ok(()),
        Some(run) => Err(Fail::Other(format!(
            "{pane}'s refresh failed: {}",
            match (run.exit, &run.error) {
                (Some(code), _) => format!("exit {code}"),
                (None, Some(why)) => why.clone(),
                (None, None) => "no result".into(),
            }
        ))),
        None => Err(Fail::Other(format!("{pane}'s refresh recorded no run"))),
    }
}

/// Print a web pane's stored page values; see `ArtifactCommand::State`.
async fn artifact_state(
    conn: &mut Conn,
    pane: String,
    key: Option<String>,
    wait: bool,
    timeout: f64,
) -> Result<(), Fail> {
    let req = conn.req();
    let wait_ms = wait.then_some((timeout * 1000.0) as u64);
    let reply = conn
        .call_within(
            ClientMsg::ArtifactState {
                req: Some(req),
                pane,
                key: key.clone(),
                wait_ms,
            },
            req,
            Duration::from_secs_f64(timeout) + Duration::from_secs(10),
        )
        .await?;
    let ServerMsg::PageState {
        values, timed_out, ..
    } = reply
    else {
        return Err(Fail::Other("unexpected reply to artifact_state".into()));
    };
    match key {
        Some(key) => match values.get(&key) {
            Some(value) => {
                println!("{value}");
                Ok(())
            }
            None if timed_out => Err(Fail::Timeout(format!("{key} was not written in time"))),
            None => Err(Fail::Other(format!("{key} is not stored"))),
        },
        None => {
            println!("{}", serde_json::to_string_pretty(&values).map_err(other)?);
            Ok(())
        }
    }
}

/// Push one JSON value into a web pane's page; see `ArtifactCommand::Data`.
async fn artifact_data(conn: &mut Conn, pane: String, file: String) -> Result<(), Fail> {
    let text = if file == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(other)?;
        text
    } else {
        std::fs::read_to_string(&file).map_err(|e| Fail::Other(format!("{file}: {e}")))?
    };
    // A markdown file is sent as the one string it is; anything else must be one JSON value.
    let value: serde_json::Value = if file.to_ascii_lowercase().ends_with(".md") {
        serde_json::Value::String(text)
    } else {
        serde_json::from_str(&text)
            .map_err(|e| Fail::Other(format!("the input is not one JSON value: {e}")))?
    };
    let req = conn.req();
    conn.call(
        ClientMsg::ArtifactData {
            req: Some(req),
            pane,
            value,
        },
        req,
    )
    .await?;
    Ok(())
}

/// The daemon resolves nothing against this command's directory, so send it absolute paths.
fn absolute(path: &std::path::Path) -> Result<String, Fail> {
    Ok(std::fs::canonicalize(path)
        .map_err(|e| Fail::Other(format!("{}: {e}", path.display())))?
        .to_string_lossy()
        .into_owned())
}

async fn space_new(
    conn: &mut Conn,
    cwd: PathBuf,
    name: Option<String>,
    background: bool,
) -> Result<(), Fail> {
    let cwd = absolute(&cwd)?;
    create(conn, |req| ClientMsg::SpaceCreate {
        req: Some(req),
        cwd,
        name,
        background,
    })
    .await
}

async fn space_recent(conn: &mut Conn, json: bool) -> Result<(), Fail> {
    let req = conn.req();
    let msg = ClientMsg::SpaceRecent { req: Some(req) };
    let ServerMsg::SpaceRecent { dirs, .. } = conn.call(msg, req).await? else {
        return Err(Fail::Other("unexpected reply to space_recent".into()));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&dirs).map_err(other)?);
        return Ok(());
    }
    if dirs.is_empty() {
        eprintln!("[no space has been made yet]");
    }
    for d in &dirs {
        println!("{}  (opened at {})", d.dir, d.opened_ms);
    }
    Ok(())
}

/// One `dir_list` call: the expanded path and its subfolders, or the daemon's reason it has none.
async fn dir_list(
    conn: &mut Conn,
    path: String,
) -> Result<(String, Vec<DirEntry>, Option<String>), Fail> {
    let req = conn.req();
    let msg = ClientMsg::DirList {
        req: Some(req),
        path,
    };
    let ServerMsg::DirList {
        path, dirs, error, ..
    } = conn.call(msg, req).await?
    else {
        return Err(Fail::Other("unexpected reply to dir_list".into()));
    };
    Ok((path, dirs, error))
}

fn print_dirs(dirs: &[DirEntry], indent: &str) {
    for d in dirs {
        match &d.space {
            Some(space) => println!("{indent}{}  (space {space})", d.path),
            None => println!("{indent}{}", d.path),
        }
    }
}

async fn space_browse(conn: &mut Conn, path: Option<String>, json: bool) -> Result<(), Fail> {
    if let Some(path) = path {
        let (path, dirs, error) = dir_list(conn, path).await?;
        if let Some(error) = error {
            return Err(Fail::Other(error));
        }
        if json {
            let listing = serde_json::json!({ "path": path, "dirs": dirs });
            println!("{}", serde_json::to_string_pretty(&listing).map_err(other)?);
        } else {
            print_dirs(&dirs, "");
        }
        return Ok(());
    }
    let req = conn.req();
    let ServerMsg::DirRoots { roots, .. } = conn
        .call(ClientMsg::DirRoots { req: Some(req) }, req)
        .await?
    else {
        return Err(Fail::Other("unexpected reply to dir_roots".into()));
    };
    let mut listings: Vec<(String, Vec<DirEntry>)> = Vec::new();
    for root in roots {
        // A root the daemon says is gone or unreadable is reported; the others still list. A
        // failed call is not that, and ends the command.
        match dir_list(conn, root.clone()).await? {
            (path, dirs, None) => listings.push((path, dirs)),
            (_, _, Some(error)) => eprintln!("{root}: {error}"),
        }
    }
    if json {
        let listings: Vec<_> = listings
            .iter()
            .map(|(path, dirs)| serde_json::json!({ "path": path, "dirs": dirs }))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&listings).map_err(other)?
        );
        return Ok(());
    }
    for (path, dirs) in listings {
        println!("{path}");
        print_dirs(&dirs, "  ");
    }
    Ok(())
}

async fn space(conn: &mut Conn, command: SpaceCommand) -> Result<(), Fail> {
    match command {
        SpaceCommand::Ls { json } => space_ls(conn, json).await,
        SpaceCommand::Status => space_status(conn).await,
        SpaceCommand::New {
            cwd,
            name,
            background,
        } => space_new(conn, cwd, name, background).await,
        SpaceCommand::Recent { json } => space_recent(conn, json).await,
        SpaceCommand::Browse { path, json } => space_browse(conn, path, json).await,
        SpaceCommand::Close { id } => {
            simple(conn, |req| ClientMsg::SpaceClose {
                req: Some(req),
                id,
                if_empty: false,
            })
            .await
        }
        SpaceCommand::Focus { id } => {
            simple(conn, |req| ClientMsg::Focus {
                req: Some(req),
                space: Some(id),
                pane: None,
            })
            .await
        }
        SpaceCommand::Rename { space, name } => {
            let id = resolve_space(conn, &space).await?;
            simple(conn, |req| ClientMsg::SpaceRename {
                req: Some(req),
                space: id,
                name,
            })
            .await
        }
        SpaceCommand::Move { space, to } => {
            window_call(conn, wheel_proto::WindowCommand::SpaceMove { space, to }).await?;
            Ok(())
        }
    }
}

/// The id of the space named by `text`, an id or a name; says which names it could have meant.
async fn resolve_space(conn: &mut Conn, text: &str) -> Result<String, Fail> {
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    if spaces.iter().any(|s| s.id == text) {
        return Ok(text.to_string());
    }
    let named: Vec<_> = spaces.iter().filter(|s| s.name == text).collect();
    match named.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(Fail::Other(format!(
            "no space {text}; the spaces are {}",
            spaces
                .iter()
                .map(|s| format!("{} ({})", s.name, s.id))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
        many => Err(Fail::Other(format!(
            "{} spaces are named {text}; use one of their ids: {}",
            many.len(),
            many.iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Run a window command on the Mac app, through the daemon, and return what it answered.
async fn window_call(
    conn: &mut Conn,
    command: wheel_proto::WindowCommand,
) -> Result<serde_json::Value, Fail> {
    let req = conn.req();
    match conn
        .call(
            ClientMsg::Window {
                req: Some(req),
                command,
            },
            req,
        )
        .await?
    {
        ServerMsg::WindowReply { data, .. } => Ok(data),
        _ => Err(Fail::Other("unexpected reply to window".into())),
    }
}

async fn window(conn: &mut Conn, command: WindowCommands) -> Result<(), Fail> {
    use wheel_proto::WindowCommand as W;
    match command {
        WindowCommands::Ls { json } => {
            let data = window_call(conn, W::Ls).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&data).map_err(other)?);
                return Ok(());
            }
            let names = |spaces: &serde_json::Value| -> String {
                spaces
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|s| {
                                format!(
                                    "{} ({})",
                                    s["name"].as_str().unwrap_or("?"),
                                    s["id"].as_str().unwrap_or("?")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            for w in data["windows"].as_array().cloned().unwrap_or_default() {
                let count = w["spaces"].as_array().map_or(0, Vec::len);
                println!(
                    "{}  {}  {} {}: {}",
                    w["id"].as_str().unwrap_or("?"),
                    w["title"].as_str().unwrap_or("?"),
                    count,
                    if count == 1 { "space" } else { "spaces" },
                    names(&w["spaces"])
                );
            }
            println!("background: {}", names(&data["background"]));
            for l in data["little"].as_array().cloned().unwrap_or_default() {
                println!(
                    "little {}  {} ({})  owner: {}",
                    l["id"].as_str().unwrap_or("?"),
                    l["space"]["name"].as_str().unwrap_or("?"),
                    l["space"]["id"].as_str().unwrap_or("?"),
                    l["owner"].as_str().unwrap_or("none")
                );
            }
            Ok(())
        }
        WindowCommands::New { name, space } => {
            let data = window_call(conn, W::New { name, space }).await?;
            println!("{}", data["window"].as_str().unwrap_or_default());
            Ok(())
        }
        WindowCommands::Rename { window, name } => {
            window_call(conn, W::Rename { window, name }).await?;
            Ok(())
        }
        WindowCommands::Close { window } => {
            window_call(conn, W::Close { window }).await?;
            Ok(())
        }
    }
}

/// Every pane in tab order.
async fn pane_ls(conn: &mut Conn, as_json: bool) -> Result<(), Fail> {
    fn walk(node: &wheel_proto::Node, out: &mut Vec<(String, String)>) {
        match node {
            wheel_proto::Node::Pane { id, kind, .. } => {
                let kind = serde_json::to_value(kind)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                out.push((id.clone(), kind));
            }
            wheel_proto::Node::Split { a, b, .. } => {
                walk(a, out);
                walk(b, out);
            }
        }
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    let mut rows = Vec::new();
    for space in &spaces {
        let mut panes = Vec::new();
        walk(&space.root, &mut panes);
        for (pane, kind) in panes {
            let focused = pane == space.focus;
            rows.push(serde_json::json!({
                "space": space.id, "pane": pane, "kind": kind, "focused": focused,
            }));
        }
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows).map_err(other)?);
        return Ok(());
    }
    for row in &rows {
        let focused = if row["focused"] == true {
            "  (focused)"
        } else {
            ""
        };
        println!(
            "{} {} {}{focused}",
            row["space"].as_str().unwrap_or_default(),
            row["pane"].as_str().unwrap_or_default(),
            row["kind"].as_str().unwrap_or_default(),
        );
    }
    if rows.is_empty() {
        eprintln!("[no panes]");
    }
    Ok(())
}

/// What a pane shows, in a few words an agent can act on.
fn pane_shows(kind: &wheel_proto::PaneKind) -> String {
    let v = serde_json::to_value(kind).unwrap_or_default();
    let text = |key: &str| v[key].as_str().map(str::to_owned);
    match v["kind"].as_str().unwrap_or_default() {
        "browser" => text("url").unwrap_or_default(),
        "web" => match (text("scope"), text("name")) {
            (Some(scope), Some(name)) => format!("artifact {name} ({scope})"),
            (None, Some(name)) => format!("artifact {name}"),
            _ => String::new(),
        },
        "terminal" => {
            let mut out = text("cwd").map(|c| format!("cwd {c}")).unwrap_or_default();
            if let Some(agent) = v["agent"]["name"].as_str() {
                out = format!("{out} running {agent}").trim().to_owned();
            }
            out
        }
        "agent" => text("agent").unwrap_or_default(),
        _ => String::new(),
    }
}

/// The panes that touch `pane` along each of its four sides, in the space that holds it.
async fn pane_near(conn: &mut Conn, pane: &str, as_json: bool) -> Result<(), Fail> {
    const EPS: f64 = 1e-9;
    type Rect = (f64, f64, f64, f64);
    fn layout<'a>(
        node: &'a wheel_proto::Node,
        r: Rect,
        out: &mut Vec<(&'a str, &'a wheel_proto::PaneKind, Rect)>,
    ) {
        match node {
            wheel_proto::Node::Pane { id, kind, .. } => out.push((id, kind, r)),
            wheel_proto::Node::Split {
                axis, ratio, a, b, ..
            } => {
                let (x, y, w, h) = r;
                if *axis == wheel_proto::Axis::Horizontal {
                    layout(a, (x, y, w * ratio, h), out);
                    layout(b, (x + w * ratio, y, w * (1.0 - ratio), h), out);
                } else {
                    layout(a, (x, y, w, h * ratio), out);
                    layout(b, (x, y + h * ratio, w, h * (1.0 - ratio)), out);
                }
            }
        }
    }
    let req = conn.req();
    let ServerMsg::Status { spaces, .. } =
        conn.call(ClientMsg::Status { req: Some(req) }, req).await?
    else {
        return Err(Fail::Other("unexpected reply to status".into()));
    };
    let mut rows = Vec::new();
    for space in &spaces {
        let mut panes = Vec::new();
        layout(&space.root, (0.0, 0.0, 1.0, 1.0), &mut panes);
        let Some(&(_, _, (x, y, w, h))) = panes.iter().find(|p| p.0 == pane) else {
            continue;
        };
        let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| a1.min(b1) - a0.max(b0) > EPS;
        for side in ["left", "right", "up", "down"] {
            for &(id, kind, (ox, oy, ow, oh)) in &panes {
                let touches = match side {
                    "left" => (ox + ow - x).abs() < EPS && overlap(y, y + h, oy, oy + oh),
                    "right" => (ox - (x + w)).abs() < EPS && overlap(y, y + h, oy, oy + oh),
                    "up" => (oy + oh - y).abs() < EPS && overlap(x, x + w, ox, ox + ow),
                    _ => (oy - (y + h)).abs() < EPS && overlap(x, x + w, ox, ox + ow),
                };
                if id != pane && touches {
                    let name = serde_json::to_value(kind)
                        .ok()
                        .and_then(|v| v["kind"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    rows.push(serde_json::json!({
                        "side": side, "pane": id, "kind": name, "shows": pane_shows(kind),
                    }));
                }
            }
        }
        if as_json {
            println!("{}", serde_json::to_string_pretty(&rows).map_err(other)?);
        } else {
            for row in &rows {
                let line = format!(
                    "{} {} {} {}",
                    row["side"].as_str().unwrap_or_default(),
                    row["pane"].as_str().unwrap_or_default(),
                    row["kind"].as_str().unwrap_or_default(),
                    row["shows"].as_str().unwrap_or_default(),
                );
                println!("{}", line.trim_end());
            }
            if rows.is_empty() {
                eprintln!("[no panes beside {pane}]");
            }
        }
        return Ok(());
    }
    Err(Fail::Daemon(format!("no pane {pane}")))
}

/// What `wheel file` does.
#[derive(Subcommand)]
enum FileCommand {
    /// Print the lines of a file around a line (the first lines without one), numbered. Exits 6
    /// when the file does not exist.
    Peek {
        path: String,
        /// Directory a relative path is read against.
        #[arg(long)]
        cwd: Option<String>,
        /// The line to centre on.
        #[arg(long)]
        line: Option<u32>,
        /// Lines each side of it (5 by default, 200 at most).
        #[arg(long)]
        context: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Fetch an image file (png, jpg, jpeg, gif or webp, up to 20 MB) from the daemon's machine, the
    /// way a phone does to draw it in a reply. Prints its path, type and size; `--out` writes the
    /// bytes to a file. Exits 6 when the file is not there or not a small image.
    Image {
        path: String,
        /// Directory a relative path is read against.
        #[arg(long)]
        cwd: Option<String>,
        /// Write the bytes here.
        #[arg(long)]
        out: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

async fn file(conn: &mut Conn, command: FileCommand) -> Result<(), Fail> {
    let (path, cwd, line, context, json) = match command {
        FileCommand::Peek {
            path,
            cwd,
            line,
            context,
            json,
        } => (path, cwd, line, context, json),
        FileCommand::Image {
            path,
            cwd,
            out,
            json,
        } => return file_image(conn, path, cwd, out, json).await,
    };
    let req = conn.req();
    let msg = ClientMsg::FilePeek {
        req: Some(req),
        path: path.clone(),
        cwd,
        line,
        context,
    };
    let ServerMsg::FilePeek {
        path,
        found,
        start,
        lines,
        error,
        ..
    } = conn.call(msg, req).await?
    else {
        return Err(Fail::Other("unexpected reply to file_peek".into()));
    };
    if json {
        let peek = serde_json::json!({
            "path": path, "found": found, "start": start, "lines": lines, "error": error,
        });
        println!("{}", serde_json::to_string_pretty(&peek).map_err(other)?);
    } else if found {
        println!("{path}");
        for (i, text) in lines.iter().enumerate() {
            println!("{:>5}  {text}", start as usize + i);
        }
    }
    if !found {
        return Err(Fail::Daemon(format!("no file at {path}")));
    }
    if let Some(error) = error {
        return Err(Fail::Other(error));
    }
    Ok(())
}

async fn file_image(
    conn: &mut Conn,
    path: String,
    cwd: Option<String>,
    out: Option<String>,
    json: bool,
) -> Result<(), Fail> {
    use base64::Engine;
    let req = conn.req();
    let msg = ClientMsg::FileImage {
        req: Some(req),
        path: path.clone(),
        cwd,
    };
    let ServerMsg::FileImage {
        path: resolved,
        found,
        mime,
        data,
        error,
        ..
    } = conn.call(msg, req).await?
    else {
        return Err(Fail::Other("unexpected reply to file_image".into()));
    };
    let bytes = match data {
        Some(data) => base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(other)?,
        None => Vec::new(),
    };
    if let (true, Some(out)) = (found, &out) {
        std::fs::write(out, &bytes).map_err(other)?;
    }
    if json {
        let image = serde_json::json!({
            "path": resolved, "found": found, "mime": mime, "bytes": bytes.len(), "error": error,
        });
        println!("{}", serde_json::to_string_pretty(&image).map_err(other)?);
    } else if found {
        println!(
            "{resolved} {} {} bytes",
            mime.unwrap_or_default(),
            bytes.len()
        );
    }
    if !found {
        return Err(Fail::Daemon(
            error.unwrap_or_else(|| format!("no file at {path}")),
        ));
    }
    Ok(())
}

async fn pane(conn: &mut Conn, command: PaneCommand) -> Result<(), Fail> {
    match command {
        PaneCommand::Open {
            pane,
            dir,
            page,
            artifact,
            scope,
        } => {
            create(conn, |req| ClientMsg::PaneOpen {
                req: Some(req),
                pane,
                dir,
                url: page,
                artifact,
                scope,
            })
            .await
        }
        PaneCommand::Run { pane, command } => {
            create(conn, |req| ClientMsg::PaneRun {
                req: Some(req),
                pane,
                command,
            })
            .await
        }
        PaneCommand::Near { pane, json } => pane_near(conn, &pane, json).await,
        PaneCommand::Ls { json } => pane_ls(conn, json).await,
        PaneCommand::Split {
            pane,
            dir,
            background,
        } => {
            let req = conn.req();
            let msg = ClientMsg::PaneSplit {
                req: Some(req),
                pane,
                dir,
                background,
            };
            match conn.call(msg, req).await? {
                ServerMsg::Created {
                    id,
                    placed_by: Some(strategy),
                    ..
                } => println!("{id} placed by {strategy}"),
                ServerMsg::Created { id, .. } => println!("{id}"),
                other => return Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
            Ok(())
        }
        PaneCommand::Swap { pane, with } => {
            simple(conn, |req| ClientMsg::PaneSwap {
                req: Some(req),
                pane,
                with,
            })
            .await
        }
        PaneCommand::Move {
            pane,
            target,
            dir,
            end_of,
            new_space,
            name,
            background,
        } => {
            let to = match (target, dir, end_of, new_space) {
                (Some(pane), Some(dir), None, false) => {
                    wheel_proto::MoveTarget::Beside { pane, dir }
                }
                (None, None, Some(space), false) => wheel_proto::MoveTarget::End {
                    space: resolve_space(conn, &space).await?,
                },
                (None, None, None, true) => wheel_proto::MoveTarget::NewSpace { name, background },
                _ => {
                    return Err(Fail::Other(
                        "pane move needs <target> <side>, --end-of <space> or --new-space".into(),
                    ))
                }
            };
            simple(conn, |req| ClientMsg::PaneMove {
                req: Some(req),
                pane,
                to,
            })
            .await
        }
        PaneCommand::Close { pane } => {
            simple(conn, |req| ClientMsg::PaneClose {
                req: Some(req),
                pane,
            })
            .await
        }
        PaneCommand::Info { pane, json } => {
            let req = conn.req();
            let msg = ClientMsg::PaneInfo {
                req: Some(req),
                pane,
            };
            match conn.call(msg, req).await? {
                ServerMsg::PaneInfo {
                    pane, kind, fields, ..
                } => {
                    if json {
                        let value =
                            serde_json::json!({"pane": pane, "kind": kind, "fields": fields});
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value).unwrap_or_default()
                        );
                    } else {
                        println!("{kind} {pane}");
                        for f in &fields {
                            println!("{}: {}", f.label, f.value);
                        }
                    }
                    Ok(())
                }
                other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
        }
        PaneCommand::View { pane, to } => {
            let req = conn.req();
            let msg = ClientMsg::PaneView {
                req: Some(req),
                pane,
                to,
            };
            match conn.call_within(msg, req, Duration::from_secs(70)).await? {
                ServerMsg::PaneView {
                    view,
                    conversation,
                    switch_ms,
                    ..
                } => {
                    let view = view
                        .map(|v| serde_json::to_value(v).unwrap_or_default())
                        .and_then(|v| v.as_str().map(str::to_owned));
                    print!(
                        "{} {}",
                        view.as_deref().unwrap_or("-"),
                        conversation.as_deref().unwrap_or("-")
                    );
                    if let Some(ms) = switch_ms {
                        print!(" switched in {ms}ms");
                    }
                    println!();
                    Ok(())
                }
                other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
        }
        PaneCommand::Start {
            pane,
            kind,
            message,
            agent,
            model,
            permission_mode,
            sandbox,
        } => {
            let req = conn.req();
            let msg = ClientMsg::PaneStart {
                req: Some(req),
                pane,
                kind,
                message,
                agent,
                model,
                permission_mode,
                sandbox,
            };
            let ServerMsg::Started {
                pane,
                session,
                state,
                ..
            } = conn.call_within(msg, req, Duration::from_secs(70)).await?
            else {
                return Err(Fail::Other("unexpected reply to pane start".into()));
            };
            let started = serde_json::json!({ "pane": pane, "session": session, "state": state });
            println!("{}", serde_json::to_string_pretty(&started).map_err(other)?);
            Ok(())
        }
        PaneCommand::Kind {
            pane,
            kind,
            artifact,
            scope,
            page,
        } => {
            simple(conn, |req| ClientMsg::PaneSetKind {
                req: Some(req),
                pane,
                kind,
                artifact,
                scope,
                url: page,
            })
            .await
        }
        PaneCommand::Page { pane, page } => {
            let req = conn.req();
            let msg = ClientMsg::PaneBrowserOpen {
                req: Some(req),
                pane,
                text: page,
            };
            match conn.call(msg, req).await? {
                ServerMsg::Opened { url, .. } => {
                    println!("{url}");
                    Ok(())
                }
                other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
        }
        PaneCommand::Report { status, pane } => {
            let Some(pane) = pane else {
                unreachable!("main exits before connecting when there is no pane")
            };
            simple(conn, |req| ClientMsg::PaneReport {
                req: Some(req),
                pane,
                status,
            })
            .await
        }
        PaneCommand::ReportContext { used, size, pane } => {
            let Some(pane) = pane else {
                unreachable!("main exits before connecting when there is no pane")
            };
            simple(conn, |req| ClientMsg::PaneReportContext {
                req: Some(req),
                pane,
                context: wheel_proto::ContextUse { used, size },
            })
            .await
        }
        PaneCommand::ReportSession { session, pane } => {
            let Some(pane) = pane else {
                unreachable!("main exits before connecting when there is no pane")
            };
            let session = match session {
                Some(session) => session,
                None => {
                    use std::io::Read as _;
                    let mut raw = Vec::new();
                    let _ = std::io::stdin().read_to_end(&mut raw);
                    let input: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
                    match input["session_id"].as_str() {
                        Some(id) if !id.is_empty() => id.to_string(),
                        _ => return Err(Fail::Other("no session_id in the hook input".into())),
                    }
                }
            };
            simple(conn, |req| ClientMsg::PaneReportSession {
                req: Some(req),
                pane,
                session,
                pid: std::process::id(),
            })
            .await
        }
        PaneCommand::Statusline { .. } => unreachable!("handled before connecting"),
        PaneCommand::WaitReload { pane, timeout } => wait_reload(conn, pane, timeout).await,
        PaneCommand::Shot { pane, out } => {
            let path = std::path::absolute(&out).map_err(other)?;
            let req = conn.req();
            let msg = ClientMsg::PaneShot {
                req: Some(req),
                pane,
                path: path.to_string_lossy().into_owned(),
            };
            match conn.call_within(msg, req, Duration::from_secs(60)).await? {
                ServerMsg::PaneShot { path, .. } => {
                    println!("{path}");
                    Ok(())
                }
                other => Err(Fail::Other(format!("unexpected reply {other:?}"))),
            }
        }
        PaneCommand::Focus { pane } => {
            simple(conn, |req| ClientMsg::Focus {
                req: Some(req),
                space: None,
                pane: Some(pane),
            })
            .await
        }
        PaneCommand::Ratio { split, ratio } => {
            simple(conn, |req| ClientMsg::SplitRatio {
                req: Some(req),
                split,
                ratio,
            })
            .await
        }
    }
}

/// Claude Code's status line inside a Wheel pane. Whatever goes wrong reporting, the person's own
/// status line still prints: it is their screen, and the report is Wheel's business.
async fn statusline(url: &str, token_file: Option<PathBuf>, pane: Option<String>) -> ExitCode {
    use std::io::Read as _;
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    let input: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
    if let (Some(pane), Some(context)) = (
        pane.filter(|p| !p.is_empty()),
        statusline::context_of(&input),
    ) {
        let report = async {
            let mut conn = Conn::open(url, token_file).await?;
            simple(&mut conn, |req| ClientMsg::PaneReportContext {
                req: Some(req),
                pane,
                context,
            })
            .await
        };
        match tokio::time::timeout(Duration::from_secs(2), report).await {
            Ok(Ok(())) => {}
            Ok(Err(fail)) => eprintln!("wheel: context report failed: {fail}"),
            Err(_) => eprintln!("wheel: context report timed out"),
        }
    }
    let cwd = statusline::cwd_of(&input).or_else(|| std::env::current_dir().ok());
    let command = statusline::config_dir()
        .and_then(|config| statusline::person_command(cwd.as_deref(), &config));
    let Some(command) = command else {
        return ExitCode::SUCCESS;
    };
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return ExitCode::SUCCESS;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&raw);
    }
    let _ = child.wait();
    ExitCode::SUCCESS
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let Some(command) = cli.command else {
        let _ = <Cli as clap::CommandFactory>::command().print_help();
        return ExitCode::from(2);
    };
    if let Command::Install { status, dir } = command {
        return match install(status, dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(fail) => {
                eprintln!("wheel: {fail}");
                ExitCode::from(fail.code())
            }
        };
    }
    if let Command::Pane(
        PaneCommand::Report { pane, .. }
        | PaneCommand::ReportContext { pane, .. }
        | PaneCommand::ReportSession { pane, .. },
    ) = &command
    {
        if pane.as_deref().is_none_or(str::is_empty) {
            return ExitCode::SUCCESS;
        }
    }
    if let Command::Pane(PaneCommand::Statusline { pane }) = command {
        return statusline(&cli.url, cli.token_file, pane).await;
    }
    let result = async {
        let mut conn = Conn::open(&cli.url, cli.token_file).await?;
        match command {
            Command::Install { .. } => unreachable!("handled before connecting"),
            Command::Status { .. } => status(&mut conn).await,
            Command::Usage => usage(&mut conn).await,
            Command::Window(command) => window(&mut conn, command).await,
            Command::Remote(RemoteCommand::Info) => remote_info(&mut conn).await,
            Command::Remote(RemoteCommand::Pair { address }) => remote_pair(&mut conn, address).await,
            Command::Remote(RemoteCommand::Devices { json }) => remote_devices(&mut conn, json).await,
            Command::Remote(RemoteCommand::Revoke { id }) => remote_revoke(&mut conn, id).await,
            Command::Term(TermCommand::Open { cwd, cols, rows }) => {
                term_open(&mut conn, cwd, cols, rows).await
            }
            Command::Term(TermCommand::Send {
                id,
                text,
                paste,
                enter,
                keys,
            }) => term_send(&mut conn, id, text, paste, enter, keys).await,
            Command::Term(TermCommand::Read {
                id,
                until,
                new,
                timeout,
            }) => term_read(&mut conn, id, until, new, timeout).await,
            Command::Term(TermCommand::Close { id }) => {
                simple(&mut conn, |req| ClientMsg::TermClose { req: Some(req), id }).await
            }
            Command::Agent(AgentCommand::Config {
                session,
                config,
                value,
                json,
            }) => session_config(&mut conn, session, config, value, json).await,
            Command::Agent(AgentCommand::Context { session, json }) => {
                session_context(&mut conn, session, json).await
            }
            Command::Agent(AgentCommand::Sessions { session, json }) => {
                session_conversations(&mut conn, session, json).await
            }
            Command::Agent(AgentCommand::New { session }) => {
                session_switch(&mut conn, session, None).await
            }
            Command::Agent(AgentCommand::Resume { session, id }) => {
                session_switch(&mut conn, session, Some(id)).await
            }
            Command::Agent(AgentCommand::Send {
                session,
                text,
                timeout,
            }) => session_send(&mut conn, session, text, timeout).await,
            Command::Agent(AgentCommand::Read { session, json }) => {
                session_read(&mut conn, session, json).await
            }
            Command::Agent(AgentCommand::Cancel { session }) => {
                session_cancel(&mut conn, session).await
            }
            Command::Agent(AgentCommand::Wait { session, timeout }) => {
                session_wait(&mut conn, session, timeout).await
            }
            Command::Perm(PermCommand::List) => perm_list(&mut conn).await,
            Command::Perm(PermCommand::Answer { id, choice }) => {
                let id = perm_id(&mut conn, id).await?;
                simple(&mut conn, |req| ClientMsg::PermAnswer {
                    req: Some(req),
                    id,
                    choice,
                })
                .await
            }
            Command::Perm(PermCommand::Wait { timeout }) => perm_wait(&mut conn, timeout).await,
            Command::Space(command) => space(&mut conn, command).await,
            Command::Pane(command) => pane(&mut conn, command).await,
            Command::File(command) => file(&mut conn, command).await,
            Command::Artifact(ArtifactCommand::New {
                name,
                pane,
                scope,
                space,
                path,
                ends_with_session,
                expires,
                renderer,
                files,
                reply_to,
                notify,
            }) => {
                let space = default_space(&mut conn, space).await?;
                let ends_with_pane = if ends_with_session {
                    Some(std::env::var("WHEEL_PANE").map_err(|_| {
                        Fail::Usage(
                            "--ends-with-session needs the agent pane this runs in, and WHEEL_PANE is not set; run it from an agent pane".into(),
                        )
                    })?)
                } else {
                    None
                };
                artifact_new(&mut conn, name, pane, scope, space, path, ends_with_pane, expires, renderer, files, reply_to, notify).await
            }
            Command::Artifact(ArtifactCommand::Add {
                path,
                name,
                scope,
                space,
                title,
                command,
                every,
                pane,
                json,
            }) => {
                let space = default_space(&mut conn, space).await?;
                artifact_add(
                    &mut conn, path, name, scope, space, title, command, every, pane, json,
                )
                .await
            }
            Command::Artifact(ArtifactCommand::Remove { name, scope, space }) => {
                let space = default_space(&mut conn, space).await?;
                artifact_remove(&mut conn, name, scope, space).await
            }
            Command::Artifact(ArtifactCommand::Ls { space, pane, json }) => {
                let space = default_space(&mut conn, space).await?;
                artifact_ls(&mut conn, space, pane, json).await
            }
            Command::Run { name: None, space, json } => {
                let space = default_space(&mut conn, space).await?;
                let req = conn.req();
                let ServerMsg::Runners { runners, .. } = conn
                    .call(ClientMsg::RunnerLs { req: Some(req), space }, req)
                    .await?
                else {
                    return Err(Fail::Other("unexpected reply to run".into()));
                };
                if json {
                    println!("{}", serde_json::to_string_pretty(&runners).map_err(other)?);
                } else {
                    for r in &runners {
                        println!("{}\t{}", r.name, r.command);
                    }
                }
                Ok(())
            }
            Command::Run { name: Some(name), space, .. } => {
                let space = default_space(&mut conn, space).await?;
                let req = conn.req();
                let ServerMsg::Started { pane, .. } = conn
                    .call_within(
                        ClientMsg::Run { req: Some(req), name, space },
                        req,
                        Duration::from_secs(70),
                    )
                    .await?
                else {
                    return Err(Fail::Other("unexpected reply to run".into()));
                };
                println!("{pane}");
                Ok(())
            }
            Command::Config(ConfigCommand::Ls { space, json }) => {
                let space = default_space(&mut conn, space).await?;
                config_ls(&mut conn, space, json).await
            }
            Command::Config(ConfigCommand::Path { space, json }) => {
                let space = default_space(&mut conn, space).await?;
                config_path(&mut conn, space, json).await
            }
            Command::Config(ConfigCommand::Get { key, space, json }) => {
                let space = default_space(&mut conn, space).await?;
                config_get(&mut conn, key, space, json).await
            }
            Command::Config(ConfigCommand::Set {
                key,
                value,
                space,
                machine,
                json,
            }) => {
                let space = default_space(&mut conn, space).await?;
                config_set(&mut conn, key, value, space, machine, json).await
            }
            Command::Config(ConfigCommand::Homes { agent, path, json }) => {
                config_homes(&mut conn, agent, path, json).await
            }
            Command::Config(ConfigCommand::Watch { timeout, json }) => {
                config_watch(&mut conn, timeout, json).await
            }
            Command::Config(ConfigCommand::Unset {
                key,
                space,
                machine,
                json,
            }) => {
                let space = default_space(&mut conn, space).await?;
                config_unset(&mut conn, key, space, machine, json).await
            }
            Command::Artifact(ArtifactCommand::Refresh {
                pane,
                json,
                timeout,
            }) => artifact_refresh(&mut conn, pane, json, timeout).await,
            Command::Artifact(ArtifactCommand::State {
                pane,
                key,
                wait,
                timeout,
            }) => artifact_state(&mut conn, pane, key, wait, timeout).await,
            Command::Artifact(ArtifactCommand::Data { pane, file }) => {
                artifact_data(&mut conn, pane, file).await
            }
            Command::Artifact(ArtifactCommand::Builtin(command)) => builtin(&mut conn, command).await,
            Command::Artifact(ArtifactCommand::Show {
                name,
                scope,
                space,
                json,
            }) => {
                let space = default_space(&mut conn, space).await?;
                artifact_show(&mut conn, name, scope, space, json).await
            }
        }
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => {
            eprintln!("wheel: {fail}");
            ExitCode::from(fail.code())
        }
    }
}
