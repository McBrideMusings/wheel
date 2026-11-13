---
name: project-design
description: "wheel: how to design or change any element, screen, layout or flow in the Mac and iOS apps (SwiftUI/AppKit), the chat transcript, or an artifact page — read DESIGN.md, follow its Layout, Chat and Artifacts rules, render every state in the real app with tools/shots.sh, lint. Read before building or changing anything a user sees in wheel."
---

# wheel — design

The design is [`DESIGN.md`](../../../DESIGN.md) at the repo root. This skill is how to work
against it; it holds no design itself.

1. **Read `DESIGN.md` whole** before the first edit.
2. **Place the work.** A new element follows the `## Layout` rule for its region (title row, grid,
   pane header, composer) and, in a transcript, the `## Chat` rules for its kind of row. A page an
   artifact draws follows `## Artifacts`. Nothing fits → the new rule is added to `DESIGN.md` in
   the same commit.
3. **A new screen, a new flow or a rearranged layout** runs `redesign` on it, with
   `tools/shots.sh` screenshots of the current app as the style anchor. A comp takes its look
   from `DESIGN.md` (`~/.claude/skills/spike/COMP.md`).
4. **Build with what the platform and the design system give.** The apps use system materials,
   controls, type and SF Symbols (`## Direction`): no custom palette, no raw colour. A chat size,
   spacing or style is a `chat.*` setting named in `## Chat`, never a literal in a view. An
   artifact page links `.wheel/wheel.css` (source `daemon/src/wheel.css`) and defines no colours.
5. **Render every state**: `tools/shots.sh <surface>` (surfaces `window` and `chat`; the script
   lists their states), then look at each PNG, light and dark. A state the script does not
   reach yet gets added to it in the same change. A state never rendered is not designed.
6. **Lint**: `tools/lint.sh` exits 0.

`tools/shots.sh` needs `make build-client` (the Debug app in `WHEEL_DERIVED_DATA`) and
`cargo build`, the same as `project-verify`'s Mac checks, whose `lib.sh` and `mac-lib.sh` it
sources. It runs its own daemon and app and never touches the person's running Wheel.

Done means every state of every touched surface was rendered and looked at, `DESIGN.md`
describes what was built, and `tools/lint.sh` passes.
