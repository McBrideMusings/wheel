---
applies-to: ["daemon/src/brief.md", "daemon/src/artifacts.rs"]
---

# ADR-0018: An artifact says what it is for, and the brief lists them

Each artifact entry in a `wheel.toml` carries a one-line `description` of what its page shows. The agent brief (`brief::text` for a chat agent, `$WHEEL_BRIEF_FILE` for a terminal agent) lists the artifacts that exist when the session starts — name, scope and description, up to 20 entries and 2,000 bytes — or, past that, their count and `wheel artifact ls --json`. The list states facts only, as ADR-0017 requires. An artifact moves from ad hoc to project or machine scope only through `wheel artifact promote <name> --to project|machine`, which asks the person through the daemon's permission prompt. A saved URL is an artifact entry with `url` in place of a path, and it opens in a browser pane.
