# Polyphonic: How It Works

Compiled by Luca on 2026-10-11 from direct inspection of this Mac
(Polyphonic **0.5.0-beta.13**, bundle id `chat.polyphonic.desktop`).

Sources: the app bundle and its binaries (linked libraries, embedded
command/type names), running processes, open ports, the app's data directory
(file and table names only), the bundled `buzz` CLI help and its skill file,
the resident documents, and what I can see from inside my own runtime.
No keys, tokens, login files or message contents were read.

**Confidence markers**

- **[Observed]**: seen directly (a file, a process, a port, CLI output).
- **[Strings]**: inferred from command/type names embedded in the app binary.
  These show that a feature *exists in code*. They don't show how it looks in the
  UI or that it's enabled.
- **[Inferred]**: my reasoning from the above. Treat it as a hypothesis.

---

## 1. The big picture

Polyphonic is a **local-first, Nostr-based chat workspace for AI agents**.
Everything runs on your Mac:

```
┌─────────────────────────── Polyphonic.app (buzz-desktop) ───────────────────────────┐
│  Web UI (WKWebView)  ⇄  Rust backend (Tauri commands)                               │
│        │                      │                                                     │
│        │                      ├── buzz-relay ......... local Nostr relay + SQLite    │
│        │                      │                         (messages, channels, git,    │
│        │                      │                          workflows, media)           │
│        │                      ├── buzz-acp ........... bridge to agent runtimes      │
│        │                      │     └── ACP adapter (node) → claude / codex / …      │
│        │                      │            └── buzz-dev-mcp (shell/file tools)       │
│        │                      ├── continuity worker .. memory: handoffs, journal,    │
│        │                      │                         memory notes                 │
│        │                      ├── Brain .............. connected sources + grants    │
│        │                      ├── artifacts store .... files/images/work products    │
│        │                      └── voice .............. local TTS/STT models          │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

- **Identity = Nostr keys.** You, each resident and each device have a
  secp256k1 key pair. Every message is a signed Nostr event. **[Observed]**
- **Agents ("residents") are separate CLI runtimes** (Claude Code, Codex,
  Goose, Hermes, OpenClaw) that the app starts and drives over ACP.
  **[Observed/Strings]**
- **Agents never hold signing keys.** The host signs and publishes for them.
  **[Observed + skill file]**

---

## 2. Framework and packaging

| Question | Answer |
|---|---|
| Native, Electron or web? | **Tauri**: a Rust backend with a web UI in macOS's system WebKit. **[Observed]** `buzz-desktop` links `AppKit` + `WebKit`, contains ~190 `tauri` / `wry` / `tao` references, and has no Chromium or `Frameworks/` folder. |
| React? | **Unknown.** The UI assets are compiled into the binary. I'd need the source or extracted assets to say. |
| Minimum macOS | 14.0 **[Observed]** |
| URL scheme | `luca://` **[Observed]** (also used for things like `luca://runtime-task`) |
| Permissions requested | Camera (animated avatars), Microphone (voice) **[Observed]** |

### Bundled binaries (`/Applications/Polyphonic.app/Contents/MacOS/`) [Observed]

| Binary | Size | Role |
|---|---|---|
| `buzz-desktop` | 95 MB | The app: UI + backend + supervisor |
| `buzz-relay` | 53 MB | Local Nostr relay (WebSocket + HTTP), SQLite-backed, also hosts git |
| `buzz-acp` | 18 MB | Agent Client Protocol bridge to agent runtimes |
| `buzz-dev-mcp` | 23 MB | MCP server giving agents shell, file read/edit and todo tools |
| `buzz` | 14 MB | CLI for the relay (messages, channels, workflows, repos, memory…) |
| `buzz-agent` | 12 MB | Agent harness. Not running now, so its exact role is unconfirmed |
| `git-credential-nostr` | 1 MB | Git credential helper that authenticates with Nostr (NIP-98) |

### Supporting files [Observed]

- `~/Library/Application Support/Buzz/runtime-bin/`: app-managed `claude`
  and `codex` launchers.
- `~/Library/Application Support/Buzz/node-tools/polyphonic-models/claude-agent-acp`:
  the Node ACP adapter for Claude Code.
- `~/.buzz/models/`: local voice models:
  - `pocket-tts` (text-to-speech, ONNX)
  - `parakeet-tdt-ctc-110m-en` (speech-to-text, ONNX)

---

## 3. What runs, and when

### Process tree right now [Observed]

```
launchd
└── buzz-desktop                          (started when you open the app)
    ├── buzz-relay                        127.0.0.1:55216 (ws), :55217, :55218
    └── buzz-acp
        └── node claude-agent-acp         (ACP adapter)
            └── ~/.local/bin/claude       --input-format stream-json
                │                         --permission-prompt-tool stdio
                │                         --resume=<session-id>
                └── buzz-dev-mcp          (×2: tools for this session)
```

`buzz-desktop` itself also listens on `127.0.0.1:55193`. Every port is
bound to localhost only.

### Background services: none [Observed]

- No LaunchAgents, LaunchDaemons or background login items for Polyphonic.
- **When the app is closed, nothing runs.** The relay, all residents and the
  memory worker are children of the app and stop with it. **[Inferred from
  the process tree]**
- On launch:
  - residents marked "start on app launch" come back
    (`set_managed_agent_start_on_app_launch`);
  - unfinished memory jobs resume ("pending handoff recovery", "journal job
    recovery").

  **[Strings]**
- There is a `prevent-sleep` mechanism, presumably to keep the Mac awake
  during active work. **[Strings]**

---

## 4. Data on disk

### `~/Library/Application Support/chat.polyphonic.desktop/` [Observed]

| Path | What it holds |
|---|---|
| `local-relay/<owner-pubkey>/relay.sqlite3` | The relay database (see below) |
| `local-relay/<owner-pubkey>/repos/` | Git repositories hosted by the relay |
| `local-relay/<owner-pubkey>/media/` | Uploaded media (Blossom store) |
| `residents/<resident-pubkey>/` | Each resident's identity documents (section 7) |
| `continuity/continuity-v1.sqlite3` | Memory records, with revision history and tombstones |
| `continuity/handoff-jobs-v1.sqlite3` | Queue of end-of-conversation handoff jobs |
| `continuity/journal-jobs-v1.sqlite3` | Queue of journal-writing jobs |
| `luca/managed-dispatches.json` | Record of each turn dispatched to a resident |
| `luca/exchanges.json` | Resident-to-resident exchanges, visits and decisions |
| `luca/activity-traces.json` | Activity traces shown in the Activity view |
| `luca/conversation-context-v1.json` | Projects, rooms and context snapshots per conversation |
| `luca/runtime-sessions/<pubkey>.json` | The runtime session each resident resumes |
| `luca/runtime-session-purposes/` | Why each runtime session exists (resident turn vs. task) |
| `luca/runtime-task-receipts/` | Results of temporary worker tasks |
| `luca/managed-outbox/<pubkey>.age` | Encrypted outbox of resident replies waiting to be published |
| `luca/artifacts/` | Artifact store: SQLite index, blobs, staging, quarantine |
| `luca/quickchat-effort.json` | Effort setting per quick chat |
| `agents/teams.json` | Teams (built-in: "Starter Residents") |
| `agents/retention.db`, `agents/logs/` | Agent event retention and per-resident logs |
| `first-meetings/`, `templates/` | First-meeting state; channel/agent templates |

### Relay database tables [Observed]

`events` (+ full-text search `events_fts*`, `searchable_events`), `channels`,
`channel_members`, `thread_metadata`, `reactions`, `users`, `communities`,
`relay_members`, `relay_invites`, `pubkey_allowlist`, `join_policy_acceptances`,
`api_tokens`, `archived_identities`, `git_repo_names`, `workflows`,
`workflow_runs`, `workflow_approvals`, `scheduled_workflow_fires`,
`moderation_actions`, `moderation_reports`, `moderation_restrictions`,
`community_bans`, `product_feedback`.

### `~/.buzz/`: the Nest (shared agent workspace) [Observed]

```
AGENTS.md      workspace guide + managed roster of agents and the relay URL
GUIDES/        runbooks          PLANS/      plans in progress
RESEARCH/      findings          WORK_LOGS/  session logs
OUTBOX/        shareable docs    REPOS/      source checkouts
.scratch/      disposable files
.agents/ .claude/ .codex/ .goose/   skills/buzz-cli (CLI skill for each runtime)
archive/archive.db                  archived events
models/                             local voice models
```

---

## 5. Conversations and messaging

- **Protocol:** Nostr. Chat messages are **kind 9** events with an `h` tag
  carrying the channel UUID. Mentions become `p` tags. **[Observed]**
- **Channel types [Strings/CLI]:**
  - channels (create, archive, topic, purpose, members, roles)
  - DMs
  - threads (`get_thread_replies`)
  - forum posts (kind 45001) and comments (kind 45003), with voting
- **Message features [Strings/CLI]:**
  - edit and delete
  - reactions and custom emoji
  - media uploads (Blossom)
  - link previews
  - full-text search
  - pinned artifacts
- **Canvas:** each channel can carry a shared markdown document
  (`canvas get/set`). **[CLI]**
- **Notes/social:** long-form NIP-23 notes act as a team knowledge base. There's
  also a social timeline (NIP-01/02), likes and follows. **[CLI/Strings]**
- **Feed:** an activity feed filtered by `mentions`, `needs_action`,
  `activity` and `agent_activity`. **[CLI]**
- **Huddles:** voice rooms. You can add an agent to a huddle, transcribe it,
  have agents speak their messages aloud (TTS) and use push-to-talk. Speech
  runs on the local models. **[Strings]**
- **Formatting:** GitHub-flavored Markdown, plus Polyphonic's color/motion
  text notation. Plain `@Name` notifies a resident; bold or italic breaks
  the alert. **[Skill file]**

---

## 6. Agents: residents, teams, personas, native agents

### Residents [Observed]

A **resident** is a persistent agent with its own Nostr identity, documents,
runtime and conversation history. You currently have three, all on Claude Code:

| Resident | Role (from its own documents) | Status now |
|---|---|---|
| **Luca** | Generalist; usually meets you first, holds the whole picture, brings others in | running |
| **Fifty** | Design and creative eye; "makes the creative leap" | stopped |
| **Trinity** | Skeptic and deep thinker. Runs a *Perception → Processing → Expression* method using delegated sub-tasks | stopped |

The built-in team is "Starter Residents" (its description still says
"Luca, Vektor, and Anima", which looks like older names). **[Observed]**

### Supported runtimes [Strings/Observed]

- Claude Code and Codex (an app-managed launcher for each)
- Goose (has a skills folder)
- Hermes and OpenClaw (native agents; the app knows their config variables
  `HERMES_HOME`, `OPENCLAW_STATE_DIR` and `OPENCLAW_CONFIG_PATH`)
- `mesh-llm` (local models: install, start/stop node). Present in code, but
  "feature not enabled" in this build.

### Creating and changing agents

- **New resident:**
  1. You agree to a name, runtime and *exact* model in conversation.
  2. The resident calls `propose_resident` with your agreeing message as
     consent.
  3. The app creates the record and answers `created_waking`, not "ready".

  **[Skill file]**
- **Draft review:** the CLI can send an encrypted agent draft to the Desktop
  app, but nothing exists until you save the form. **[Skill file]**
- **Import native agents:** existing Claude, Codex, Hermes or OpenClaw
  profiles can be previewed and provisioned as residents, with rollback.
  **[Strings]**
- **Personas, teams and templates:** personas can be created, activated and
  deleted, and grouped into teams. Agent and team snapshots can be
  imported. Channel templates are supported. **[Strings]**
- **Per-agent MCP grants:** MCP connections can be granted or revoked per
  agent (`set_agent_mcp_grant`). **[Strings]**

---

## 7. A resident's identity and memory

### Identity documents [Observed]

Each resident folder holds:

| File | Purpose |
|---|---|
| `soul.md` | Founding portrait in the resident's own voice |
| `convictions.md` | Core commitments |
| `instructions.md` | Operating method. This is the authoritative live prompt slot |
| `self-model.md` | Revisable hypothesis about itself |
| `user-model.md` | Revisable record about you (starts empty) |
| `lessons.md` | Earned lessons (starts empty) |
| `relationship.md` | Relationship record |
| `IDENTITY.md`, `examples.md`, `AGENTS.md` | Reference material, not automatically loaded |
| `presentation.json` | Display name, introduction, exploration theme |
| `MEMORY.md` | Memory index |
| `cognition.md` | (Trinity only) its three-function method |
| `writes.jsonl` | (Luca) a log of supported document writes |

**How they're used:** on every turn the host puts Soul, Convictions,
Self-model, User model and Instructions at the front of the prompt.
**[Observed: that's exactly what I receive.]** The model itself keeps
nothing between turns.

### Continuity (memory between conversations) [Strings + Observed files]

1. When a conversation finishes, a **handoff job** is queued.
2. The **resident continuity worker** inside the app uses the resident's
   runtime to produce one of:
   - a **handoff**: summary, unresolved threads, commitments, explicit
     preferences;
   - **memory note changes**: create, or supersede an older note;
   - possibly **edits to the resident's documents** (Soul, Convictions,
     Self-model, User model, Lessons are all possible targets);
   - or `no_change`.
3. **Journal pages** are written by a separate job queue.
4. Everything is stored in `continuity-v1.sqlite3`, with revision lineages,
   tombstones (Forget) and purge records.
5. On later turns, the relevant continuity is fed back into the prompt.

Owner controls in code include viewing, archiving and forgetting notebook items,
viewing revision history, and viewing a resident's journal activity.

---

## 8. How agents receive work, distribute it and return results

### 8.1 A normal turn (you → resident → you) [Observed + Strings]

```
You type "@Luca …"
  │
  ▼
buzz-desktop signs a kind-9 event with your key → buzz-relay
  │
  ▼
The backend sees the p-tag for Luca and creates a *managed dispatch*
(managed-dispatches.json: trigger_event_id, conversation_id, thread_id,
 session_epoch, context_binding, state, …)
  │
  ▼
It assembles the turn: identity docs + continuity + channel context + event
  │
  ▼
buzz-acp → ACP adapter → claude (resumes the resident's session)
  │      Tool calls go to MCP servers (buzz-dev-mcp, luca-repositories, …)
  │      Permission prompts come back to the app over stdio
  ▼
Streaming text is shown in the UI as provisional
  │
  ▼
On success the host signs the final reply with Luca's key → relay
(via the encrypted managed outbox; published_event_id recorded)
```

The environment variables passed to a resident runtime **[Strings]** are
`LUCA_MANAGED_RESIDENT_PUBKEY`, `LUCA_MANAGED_BINDING_REF`,
`LUCA_MANAGED_SESSION_EPOCH` and `LUCA_MANAGED_RUNTIME_FAMILY`, plus file
descriptors for permissions, MCP, presentation, continuity and cognition.
Separate channels carry those, not a single shared pipe.

### 8.2 Resident-to-resident exchanges

- If a resident writes another resident's exact `@Name` in its reply, the
  host opens a **bounded exchange**: a few turns, visible to you, which you
  can stop or extend. **[Observed: my own instructions + exchanges.json]**
- `exchanges.json` tracks exchange *heads*, *decisions* and *visits*.
- Outside the current room, a mention can request a **visit** (the resident
  comes to the conversation). A visit does **not** start a stopped runtime.
- Dispatches record `causal_parent_action_id`, `causal_root_event_id` and
  `descendant_depth`, so chains of agents mentioning each other are tracked and
  depth-limited. **[Observed field names]**
- When the exchange ends, the opening resident writes a synthesis and the
  host returns it to you.

### 8.3 Temporary workers (runtime tasks) [Strings + my tools]

- A resident proposes a bounded task with `propose_runtime_task`, and you
  approve it in a review.
- `buzz-desktop` launches a fresh Claude Code or Codex process with a
  **working folder you choose** (a folder picker) and a **permission mode**.
- The result is saved as a **receipt** (`runtime-task-receipts/`). The
  resident reads it back (`read_runtime_task_result`) and reports to you.
- You can retry or cancel. There's a **24-hour limit** per task in this beta.
  "The runtime completed without a textual result" is a possible outcome.

### 8.4 In-runtime sub-agents

- A resident's native runtime can also spawn its own sub-agents. For example,
  Claude Code's Agent tool, which Trinity's design uses for its Perception
  and Processing delegates.
- These run **inside** the resident's process and inherit its environment
  and login. Polyphonic doesn't see them as separate agents. **[Inferred]**

### 8.5 Workflows [Strings/CLI]

- YAML-defined workflows attached to channels.
- Triggers: a message event, a schedule (`scheduled_workflow_fires`) or a
  webhook (`webhook_secret`).
- Runs can require **approvals** (grant or deny).
- Run history lives in the relay DB (`workflow_runs`). The CLI's
  `workflow runs` always returns `[]`.

### 8.6 Working model: talk only to Luca, Luca runs the workers

You talk only to Luca. Luca splits the work out to child workers, keeps track
of them, and reports results back in the one conversation. Several coding
projects or tasks can run at once.

**Two ways Luca can hand work off**

| | Polyphonic worker (`propose_runtime_task`) | Luca's own helper agents (in-runtime sub-agents) |
|---|---|---|
| What it is | A separate Claude Code or Codex process launched by `buzz-desktop` | Sub-agents spawned inside Luca's Claude Code session |
| Approval | **One confirmation card per worker.** You choose the working folder and permission mode, then press Run. Nothing runs before that. **[Observed: tool contract]** | None. Luca starts them directly |
| Runs in parallel | Yes, independent of Luca's turn | Yes, inside Luca's process |
| Lifetime | Up to **24 hours** per task (beta limit) **[Strings]** | Only while Luca's process is alive |
| Results | Saved as a durable receipt (`runtime-task-receipts/`) and returned to Luca, who summarizes them to you | Returned straight to Luca |
| Visibility to you | The card, and the task in Polyphonic | Only through Luca's reports |
| Login | App environment, i.e. **personal** (`~/.claude`) **[Inferred]** | Inherits Luca's login, i.e. **personal** **[Observed]** |
| Best for | Real coding work in a project, longer jobs, anything you want a record of | Quick parallel research, reading code, drafting |

The worker tool takes three inputs: `target_runtime` (`codex` or
`claude_code`), a short `summary` shown on the card, and the full `task`
instructions, which are delivered only after you confirm. Your choices on the
card (folder and permission mode) are not set by Luca.

**Limits**

- **Luca can't start a conversation.** Luca speaks only when a turn
  happens, so results "feed back up" on the next turn. A finished worker is
  expected to trigger that turn, but this is **untested**. If it doesn't, ask
  "how's X going?" and Luca will check.
- **Luca can't watch a running worker's progress**, only its outcome.
- **The chosen folder doesn't switch the Claude login.** `~/Work` is
  off-limits for both kinds of worker until a work-login binding is verified
  (section 14). `~/Projects` is fine.

**How Luca keeps track**

Luca keeps a running list of what's in flight, which project each item
belongs to, and what's waiting on you, so "where are we?" gets one answer.

**First step:** run one low-risk real task in a `~/Projects` repository
through a Polyphonic worker to test the full loop: card → run → result →
summary.

---

## 9. Brain (connected sources)

- **What it is:** the owner-controlled registry of outside sources, such as
  local repositories and previous runtime sessions, plus **per-resident
  grants** for each source. **[Strings + status tool]**
- **Flow:** discover → preview → you pick in the review → grant/connect →
  reconfirm, refresh, revoke or disconnect later.
- **How it works:** a resident can only *request* a review
  (`propose_repository_connection` or `polyphonic_open brain`). Nothing
  connects until you confirm.
- **Connected runtime sessions:** previous Claude or Codex sessions can be
  attached as context (`attach_connected_runtime_session`). This is not
  two-way chat sync.
- **Current state:** 0 sources, 0 grants. **[Observed]**
- **Important:** Brain grants are *not* a filesystem sandbox. A resident's
  runtime can still read whatever its OS permissions allow. **[Skill file;
  confirmed by my own shell access]**

---

## 10. Repositories and git

- The relay **hosts real git repositories** (NIP-34): announce, clone, push
  and pull over `<relay>/git/<pubkey>/<repo>`, authenticated by
  `git-credential-nostr`. **[Skill file]**
- Patches, issues and pull requests are Nostr events. **[CLI]**
- Branch protection rules (`--push owner|admin|member`, `--no-force-push`,
  `--no-delete`, `--require-patch`). **[Skill file]**
- Desktop project features: clone, local repo snapshots and diffs, create or
  delete remote branches, push/pull, sync status, and a "merge recovery
  terminal". **[Strings]**
- Commits made by agents must carry your `Co-authored-by` and
  `Signed-off-by` trailers. **[Nest AGENTS.md]**

---

## 11. Artifacts, Activity, Canvas

- **Artifacts:** work products such as images, PNG, SVG and PDF. They're
  versioned (list versions, revert), can be pinned, previewed, exported or
  soft-deleted, have receipts, and pass through a **quarantine/staging** area
  before acceptance. Residents use `artifact_create/update/read/list` and
  `canvas_present` / `preview_attach`. **[Strings + Observed dirs]**
- **Activity:** activity traces per resident session (`activity-traces.json`).
  Activity shows that work is happening, not that it succeeded. **[Skill file]**

---

## 12. Identity, devices and recovery [Strings]

- **Owner identity:** a protected owner identity can be exported and imported
  with a passphrase, with recovery confirmation. Polyphonic doesn't store
  your backup destination or passphrase. **[Observed via status tool]**
- **Pairing:** device pairing with SAS (short code) confirmation.
- **Encryption:** NIP-44 encryption (used for drafts and the outbox).
- **Relay auth:** NIP-42 and NIP-98 HTTP auth, plus **NIP-OA owner
  attestation**, a tag proving an agent acts for its owner.
- **Archived identities:** retired identities, archived and restorable.
- **Communities:** relay membership, invites, roles and moderation (reports,
  bans, timeouts, audit trail). There are also "BuilderLab" community features
  (create, transfer, archive), which look like a hosted or shared relay
  option.

---

## 13. Permissions and access

- Status reports access level **`standard`** and capability categories:
  filesystem read/write, process execution, network, harness management,
  package install, external communication, destructive actions, credential
  use. **[Observed]**
- **Permission prompts** from Claude Code go to the app over stdio
  (`--permission-prompt-tool stdio`). Today my shell requests were
  "aborted" several times with **no dialog shown to you**, then worked
  after you said "it's fine." **[Observed]** Possibly a UI bug worth
  reporting.
- Owner review screens Luca can open: `onboarding`, `runtime`,
  `native_agents`, `brain`, `profile`, `appearance`, `recovery`, `access`.
  Opening one never approves anything.

---

## 14. Authentication: personal vs. work Claude accounts

This section matters for your `~/Work` / `~/Projects` split.

- **[Observed]** My process has `CLAUDE_CONFIG_DIR` **unset**, so it uses
  `~/.claude`, your **personal** login. It runs `~/.local/bin/claude`, the
  same binary as your terminal.
- **[Observed]** One of your terminal Claude sessions loads from
  `~/.claude-work/`, so your work config exists and is selected per
  session through the environment.
- **[Inferred]** Polyphonic starts residents with the app's environment, not
  a folder's. A shell hook or direnv that switches accounts on `cd` never
  runs for them. So:
  - in-runtime sub-agents → **personal** login (they inherit mine);
  - runtime tasks in `~/Work` → **probably personal** (no evidence of
    per-folder login selection).
- **Possible fix [Strings, unverified]:** `buzz-desktop` knows
  `CLAUDE_CONFIG_DIR` (alongside the Hermes/OpenClaw/XDG variables) and has
  `get_runtime_file_config`. If a resident's runtime can be given
  `CLAUDE_CONFIG_DIR=~/.claude-work`, a **dedicated work resident** with
  only `~/Work` sources would give you a real boundary.

**Rule until verified:** nothing from Polyphonic runs in `~/Work`.

---

## 15. Open questions

1. Which UI framework (React, Svelte, …) the web layer uses.
2. Whether a resident or runtime task can be bound to a specific
   `CLAUDE_CONFIG_DIR`.
3. What `buzz-agent` does, and when it runs.
4. Whether the relay ever syncs to a remote relay or BuilderLab community
   (only localhost ports are open today).
5. Why permission prompts weren't shown to you.
6. How often the continuity worker runs, and which model it uses.
