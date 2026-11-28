# Wheel redesign (iOS + macOS): implementation handoff

Targets: the Wheel iOS app (SwiftUI) and the macOS app (sections 7–8). The phone is a remote control and monitor for agents and panes that run on the Mac's `wheeld` daemon. This document covers the final "Racks style" direction: dark mode only, a navy-black palette, pill-shaped rows and controls.


`ios-mockups/` holds each screen as an HTML file (`*.dc.html`). Treat them as markup references for layout, sizes, colours and copy. They use a small design runtime (`support.js`, `<dc-import>`, `{{…}}` holes), so they will not render on their own; read the inline styles. Sizes in the mockups are in points on a 393×852 frame (iPhone 16/17 Pro).

`DesignTokens.swift` is a starting point for the colour, type, spacing and radius tokens below.

---

## 1. Navigation model

```
Home (hosts → spaces → sessions)
 ├─ tap a session row ──────────► Pane (inside its space)
 │                                 └─ pane pill switches panes in that space
 ├─ eye button ─────────────────► Agents view (needs you / working / done)
 ├─ orchestrator button ────────► Orchestrator (its own chat + panes it shows you)
 ├─ search / ≡ pill ────────────► Search / command palette
 └─ settings button ────────────► Settings
```

- No tab bar and no mode menu. Home is the hub; everything else is pushed from it or opened as a sheet.
- **Inside a space**, panes are pages in a horizontal carousel. The **pane pill** at the bottom shows them and switches between them. Swiping also switches.
- **Sheets** (R10–R13) open over the pane they belong to and use detents.
- **Voice** is not a separate screen. It's an overlay mode of the current agent chat (R6).

## 2. Screen inventory

| ID | File | What it is |
|---|---|---|
| R1 | `R-Home.dc.html` | Home. Each host (Studio, MacBook Pro) is a header with icon, name and `•••`. Under it, each space is a grey section header with ⊕ (new pane) and ⌄ (collapse), followed by a pill card of session rows: agent tile, title, chevron. Row status: orange dot on the tile = needs you; blue dot = new output; `•••` before the chevron = working. Bottom bar, left to right: settings, orchestrator, [search · palette] pill, eye (orange dot when anything needs you). |
| R2 | `R-Agents.dc.html` | Agents view, opened from the eye. Sections Needs you / Working / Done. The first needs-you permission is expanded with the command, Deny and Allow, so it can be answered in one tap. Other needs-you rows have inline Reply or Review. |
| R3 | `R-Chat.dc.html` | Agent chat. Header: back · centred pill (session title / space · harness) · pill with context ring + `•••`. Transcript: plain 17pt text with no bubbles for the agent, and faded **tool rows** (icon + grey text + ›). Status line ("Waiting on you · 2 running tasks"). The permission card sits **above the composer**, not in the transcript. Two-row composer. Pane pill at the bottom. |
| R4 | `R-Terminal.dc.html` | Terminal pane: Ghostty view, layout mini-map at top right, pane pill. |
| R5 | `R-Overview.dc.html` | Space overview (the carousel zoomed out). Pane cards with labels; the current pane has a white ring. Host switcher pill, space name and page dots. |
| R6 | `R-Voice.dc.html` | Voice mode as an **overlay on the chat**. The transcript stays visible, and your speech appears as a dashed live bubble. Voice approvals show as tool rows ("✓ Approved by voice · git push"). The composer is replaced by a large mic (listening ring), then + · model pill · read-aloud toggle · white ✕ (end). The pane pill stays, so you can look at other panes while talking. |
| R7 | `R-Palette.dc.html` | Command palette: a rounded sheet with a search field. The selected row is highlighted in navy, with ⌘ shortcuts. |
| R8 | `R-Browser.dc.html` | Browser pane. The page is full screen. A floating top bar holds home · capsule [‹ › url] · `•••` (reload, open on Mac, share, console). The floating pane pill is at the bottom. |
| R9 | `R-Artifact.dc.html` | Artifact pane. Same chrome as R8; the capsule shows the document title instead of a URL. The document is full screen. |
| R10 | `R-Tasks.dc.html` | Background tasks sheet. Running (stop button, live timer, progress line) and Finished (Completed or Failed with exit code, chevron to detail). |
| R11 | `R-Tool.dc.html` | Tool-call detail sheet (e.g. Bash). Status badge, time, exit code and folder. Description, Command block, Output block (copy buttons; "Running…" while live), and "Rerun in a terminal pane". |
| R12 | `R-Model.dc.html` | Model sheet: the model list for the **session's own harness**, then Effort and Permission mode rows. The harness is chosen when the agent is created, never in this sheet. |
| R13 | `R-Usage.dc.html` | Usage sheet, opened from the context ring. Context % with a split bar (system + tools, memory + CLAUDE.md, messages), Compact now / New session. Plan limits (5-hour window, weekly, per-model) with reset times. |
| R14 | `R-Orchestrator.dc.html` | Orchestrator chat. Agent state appears as faded tool rows (icon + text, no dots, no colour coding). An inline preview card shows a pane from another space. Suggested replies are small chips above the composer. The pane pill lists the orchestrator chat plus the panes it's showing you. |
| R15 | `R-Orch-Mirror.dc.html` | A pane shown by the orchestrator (e.g. the render-fix agent from game-port). Header says "shown by Orchestrator · space" and has an **Open ↗** button to go to the real space. The composer replies to that agent directly. |
| R16 | `R-Slash.dc.html` | Typing `/` in the composer. A menu above the composer lists Skills (from the session's harness on the Mac) and Commands; the first match is highlighted. The pane pill hides while the keyboard is up. |
| R17 | `R-Files.dc.html` | Files pane, tree. "All files / Changed · N" filter, git status letters (M/A) in grey, open file highlighted. |
| R18 | `R-Editor.dc.html` | Files pane, editor. File name and folder in the header; the path chip goes back to the tree. `+42 −18` chip switches to the diff. Line numbers, syntax colours, thin gutter bars for agent-added (green) and changed (blue) lines. |
| — | `PanePill.dc.html` | Shared pane pill component (see 4.1). |

## 3. Design tokens (dark)

Typography: use **SF Pro** (the mockups use Inter as a stand-in) and **SF Mono** for terminal, tool and code text (the mockups use JetBrains Mono). Support Dynamic Type; the sizes below are the defaults at Large.

### Colour

| Token | Hex | Use |
|---|---|---|
| `bg.list` | `#0A0D12` | Home, Agents view, voice backdrop |
| `bg.pane` | `#0E111A` | Chat, terminal, panes, editor |
| `bg.overview` | `#000000` | Space overview |
| `surface.card` | `#1E2025` | Row cards, permission card, composer |
| `surface.control` | `#1A1D23` | Header pills, round buttons, pane pill |
| `surface.bar` | `#16191E` | Home bottom-bar buttons |
| `surface.raised` | `#2A2D33` | Buttons inside cards (Deny, +, mic) |
| `surface.selected` | `#2E3138` | Selected item in a pill or segmented control |
| `surface.sheet` | `#14171D` | Sheets |
| `surface.code` | `#151820` | Tool-output blocks, preview cards |
| `line.hairline` | `#23262C` | Control borders |
| `line.divider` | `#2E3137` | Row dividers inside cards |
| `line.separator` | `#181B21` | Under headers, above bottom bar |
| `text.primary` | `#F2F3F5` | |
| `text.secondary` | `#8E9199` | Section headers, subtitles, tool rows |
| `text.tertiary` | `#6E727A` | Placeholders, chevrons, timestamps |
| `text.code` | `#C9CCD1` | Mono text |
| `accent` | `#3B82F6` | Allow button, selected eye, context ring, links ("2 running tasks") |
| `select.navy` | `#1B2B47` | Highlighted row in palette, slash menu, tree |
| `codeChip.text` / `.bg` / `.border` | `#7FB2FF` / `#16203A` / `#233258` | Inline code in messages |

Status (used sparingly: dots on tiles and the eye, never as row backgrounds):

| Token | Hex |
|---|---|
| `status.needsYou` | `#FF9F0A` |
| `status.working` | `#3B82F6` (tile dot = new output) |
| `status.done` | `#4ADE80` |
| `status.stopped` / destructive | `#FF6961` (end-voice button `#EF5350`) |
| permission card inner border | `#4A3412` |

Syntax (editor): keyword `#FC5FA3`, type `#5DD8FF`, string `#FC6A5D`, comment `#6C7986`, number `#D0BF69`, property `#A167E6`.

Light mode is not designed yet. Keep every colour behind a token so a light set can be added.

### Type scale (pt)

| Role | Size / weight |
|---|---|
| Host name | 19 semibold |
| Header pill title / subtitle | 14 semibold / 12 regular |
| Chat body (agent text) | 17, line height 26 |
| Row title | 17 |
| Section header | 15 semibold, secondary colour |
| Tool row | 16, secondary colour |
| Caption / meta | 13 |
| Mono (inline code, commands) | 13–14; terminal 12.5; tool output 12 |
| Sheet title | 18 semibold |

### Spacing and size

- Screen side padding: 16 (lists), 20 (chat text), 12 (cards and composer at the bottom).
- Card padding 14; gaps 8 / 10 / 12.
- Row heights: home 52, agents view 66–70, tool row 36, tree row 40.
- Agent tile: 33–34 square with 9 corner radius. Lettermark for now (CC / CX / π); use real harness icons.
- Hit targets: at least 44. Where a visual is smaller (pane pill items are 40×28), extend the hit area with `contentShape`.

### Radii

Row cards 26 · composer 26 · permission card 24 · sheets 34 (top) · preview and code cards 18–20 · chips 16 · tiles 9 · capsules and round buttons = height / 2.

## 4. Components

### 4.1 Pane pill (shared, fixed position)

- 36 tall capsule: `surface.control` with a hairline border. Items are 40×28, and the selected item has a `surface.selected` fill. A separate 36 round **+** sits to its right (opens the overview).
- **The same size and position on every pane screen**: centred horizontally, bottom-aligned at the same offset (30 from the bottom of the frame in the mockups). Nothing else may push it sideways; back/forward and other controls live in the top bar.
- Variant `floating`: translucent fill `rgba(26,29,35,0.88)` with a shadow, used over full-screen pages (R8, R9).
- Item icons by pane kind: agent (harness lettermark/icon), terminal `>_`, browser globe, artifact doc, plugin node graph, files folder-with-code, orchestrator hub. An orange 6pt dot on an item = that pane needs you.
- Inputs: the space's pane list, the active pane, floating on or off. In the orchestrator, the first item is the orchestrator chat and the rest are panes it is showing you.
- Hidden while the keyboard is up (R16).

### 4.2 Header

44 round back button · flexible centred pill (title / subtitle; tap = switcher) · right pill (context ring 30pt with % inside, opens Usage; `•••`). All controls use `surface.control` with a hairline border.

### 4.3 Session row card (home)

A pill card grouping a space's sessions. Rows are tile + title + optional `•••` (working) + chevron, with inset dividers. Swipe actions: delete (red circle), rename (blue circle).

### 4.4 Tool row

Grey 20pt icon + grey 16pt text + `›`, with no background, no dots and no colour coding. One row per call or collapsed group ("Read 3 files, edited 2"). Tap opens R11. The orchestrator uses the same row for agent status ("`>_` ios-redesign wants to run git push").

### 4.5 Permission card

Sits above the composer. `surface.card`, 24 radius, inner border `#4A3412`.
- Top line: orange dot + "Wants to run" + tool · scope (e.g. "Bash · network").
- Command block in mono.
- Buttons: Deny (`surface.raised`), Allow (`accent`), and `•••` (always allow this command pattern).

### 4.6 Composer (two rows)

- **Row 1:** text field with placeholder "Reply to <session> or hold to talk". While the agent is working: "Queue for after this turn…".
- **Row 2:** + (attach, skills) · model pill "Opus 5.5 High" (opens R12) · spacer · mic (dictation) · action button.
- **Action button:** Stop while working; Send when there's a draft; Voice when idle with an empty draft. It is hidden while a permission is pending.

### 4.7 Status line

A small line under the last message: "Waiting on you" in orange, then "· N running tasks" in `accent`, which opens R10.

### 4.8 Suggested replies

32pt outlined chips (hairline border, grey 14pt text) in one row above the composer. Tap sends the chip as the reply. The row scrolls sideways when full.

### 4.9 Sheets

`surface.sheet`, top corners 34, grabber, round ✕ at top left, centred 18pt title, divider. Use `.presentationDetents`.

### 4.10 Floating page chrome (browser and artifact)

- **Top:** round home button · capsule [‹ › url-or-title] · round `•••`. All translucent with a shadow.
- **Bottom:** the floating pane pill only.
- The page scrolls under both.

## 5. Behaviour notes

- **Needs you** means a permission request or a question. It shows as:
  - an orange dot on the session tile (home)
  - an orange dot on the eye button
  - the needs-you section in the Agents view
  - an orange dot on the pane pill item
  - a notification / Live Activity (designed earlier, not in this set)

  There is no in-pane toast.
- **Voice approvals:** inferred from speech. Each one is echoed as a tool row in the transcript.
- **Orchestrator:** one long-running chat across all spaces and hosts. It can add panes from any space to its own pane pill (R15), and remove them. "Open ↗" jumps to the real space.
- **Model sheet:** shows only the current harness's models. Changes apply on the next turn.
- **Slash menu:** lists come from the session's harness on the Mac.

## 6. Open decisions

1. Whether the editor (R18) is fully editable on the phone, or read and review first with small edits. It's drawn read-first.
2. Whether + on the pane pill opens the overview (drawn) or a "new pane" menu.
3. Light mode token set.
4. Real harness icons for the tiles in place of lettermarks.


---

## 7. macOS app

Same language, tokens and parts as iOS: the colours, tool rows, permission card, two-row composer, status dots and code chips are all shared. Desktop mockups are in `desktop-mockups/` (1440×900 frame, window 1400×860). The canvas page is "Desktop (macOS)".

### 7.1 Window model

- **No sidebar by default.** The title bar holds:
  - traffic lights
  - **host/space pill** (globe icon, space name, host underneath), which opens the switcher (D3)
  - **session tab pill** (agent tile, title, `•••`)
  - on the right, round 32pt buttons: **eye** (agents popover; orange dot when anything needs you), **⌘** (command palette), **+** (new tab/session)
- **Panes are tiled** in splits, never a carousel, so there's no pane pill on desktop.
- **Each pane:**
  - background `#0D1015`, 1px border `#1C1F25` (focused `#2A3245`), radius 10, 8pt gaps
  - 30pt title line: 16pt tile + title, then faint (`#4A4E57`) split right / split down / duplicate / maximize / close icons
- **Optional sidebar (D2):** 280pt wide.
  - Top: traffic lights, sidebar toggle, and eye / ⌘ / +, which move out of the title bar.
  - Middle: host groups → space labels → session rows. Rows are 34pt with a 20pt tile and status dot; the selected row is a `#23262C` pill with `•••`.
  - Bottom: settings, orchestrator, add host.

### 7.2 Desktop screen inventory

| ID | File | What it is |
|---|---|---|
| D1 | `D-Main.dc.html` | Main window: title bar as above, two agent-chat panes split side by side (left one has the pending permission). |
| D2 | `D-Sidebar.dc.html` | Same with the optional sidebar. |
| D3 | `D-Switcher.dc.html` | Host-pill dropdown. "Filter or create…", then **Orchestrator ⌘0** at the top (switching to the orchestrator is a session switch). Then host sections, each with a new-session button and its spaces with ⌘1… shortcuts (current one checked, highlighted row `#2F62D0`). Then New Session ⌘N, New Session on Host…, Add Remote Host…. Menu background `rgba(32,34,40,0.97)`, border `#33363D`, radius 16. |
| D4 | `D-Agents.dc.html` | Eye popover (380pt). **Needs you** first: the permission with command and Deny / Allow inline, then the other needs-you rows. **Working** rows have an activity line; the selected row is blue. **Filter** field at the bottom. |
| D11 | `D-Orch-Min.dc.html` | **Orchestrator (chosen design).** No title bar. Traffic lights float top-left with "Orchestrator ⌄" as plain grey text (the session dropdown from D3). Faint ⌘ and + icons top-right, with no eye and no session tab. The orchestrator chat has no pane frame: it's a centred column (max 720pt) on the window background, with suggested-reply chips and the composer. The panes it's showing you are bordered panes on the right, each with "shown by Orchestrator · Open ↗". |
| D7 | `D-Browser.dc.html` | Browser pane. Bar: back/forward · centred URL capsule · reload · `{ } N` console button (error count). Page below. |
| D8 | `D-Artifact.dc.html` | Artifact pane. Same bar with the document title and "Open in browser ↗". Document in a centred column up to 700pt; decision options can be clicked; selecting text comments back to the agent. |
| D9 | `D-Files.dc.html` | Files pane: **tree + editor in one pane**. Tree column 250pt with an All / Changed filter and search. Editor has file tabs (blue dot = unsaved), a `+42 −18` diff chip, line numbers, syntax colours, and gutter bars for agent-added (green) and changed (blue) lines. |
| D10 | `D-Plugin.dc.html` | Plugin pane (deps-graph, marked "plugin") above a terminal pane. Graph cards are outlined by status, with a legend. |

### 7.3 Desktop shortcuts shown

⌘⏎ Allow · ⌘⌫ Deny · ⏎ send · ⌘. stop · hold ⌥ to talk · ⌘0 orchestrator · ⌘1–9 spaces · ⌘N new session · ⌘T new tab.

### 7.4 Extra desktop tokens

| Token | Value |
|---|---|
| `window.bg` | `#0B0E13` |
| `pane.bg` / `pane.border` / `pane.borderFocused` | `#0D1015` / `#1C1F25` / `#2A3245` |
| `pane.chromeIcon` | `#4A4E57` |
| `titlebar.hostPill` / `titlebar.sessionPill` | `#15181E` / `#23262C` |
| `menu.bg` / `menu.border` / `menu.selected` | `rgba(32,34,40,0.97)` / `#33363D` / `#2F62D0` |
| Title-bar round buttons | 32pt, `#1A1D23` (eye `#3B82F6` when its popover is open) |
| Desktop chat body | 14.5–15.5pt, line height 22–24 |

## 8. Superseded designs (not included)

The canvas still holds the earlier iOS directions (A/B), the browser variations R8a–R8d, desktop D5 and D6 (earlier orchestrator layouts, replaced by D11), and the first desktop pass. They're kept for reference only. Build from the files in this package.
