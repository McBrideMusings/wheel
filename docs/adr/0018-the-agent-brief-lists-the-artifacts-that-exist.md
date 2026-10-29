---
applies-to: ["daemon/src/brief.rs", "daemon/src/brief.md", "daemon/src/artifacts.rs"]
---

# ADR-0018: The agent brief lists the artifacts that exist

The brief reaches an agent with the artifacts of its space as one section: `brief::text` for an agent pane, the file at `WHEEL_BRIEF_FILE` for a terminal. Each machine and project entry is one line of name, scope and description, where the description is the entry's one-line `description` key. The section lists at most 20 entries in at most 2000 bytes. Past either limit it gives the count and points to `wheel artifact ls --json`. It states that the list is as of session start, and it holds facts only, as ADR-0017 requires.

`wheel artifact promote <name> --to project|machine` moves an artifact into the target scope's `wheel.toml`, clears `ends_with_pane` and `expires_at`, and sets `pane = true`. The daemon asks the person through its permission prompt, and a denied prompt changes nothing.

An `[artifacts.<name>]` table may carry `url` in place of `path`. `wheel artifact new <name> --page <url>` writes it, and showing it in a pane makes that pane a browser pane at the url, under the rule of ADR-0005. The flag is `--page` because `--url` is the CLI's global flag for the daemon's address. The entry does not start the server behind the url. It lists with kind `url`, and the pane picker offers it when `pane = true`.
