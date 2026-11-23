---
applies-to: ["daemon/src/voice.md", "daemon/src/claude.rs", "daemon/src/leases.rs"]
---

# ADR-0055: Voice mode adds the voice line to each turn of the agent it talks to

The design is `docs/voice-mode/index.md` (§3, §6).

While a connection holds the `voice` lease and has named an agent with `agent_voice`, wheeld adds the voice line (`daemon/src/voice.md`: the reply is read aloud, so answer in a few short spoken sentences, summarize code and output, and put what must be seen in an artifact shown with `wheel agent card`) to each of that agent's turns as hidden context. The mark follows the lease: it clears when the holder names none, when another connection takes the lease, when the holder's connection closes, and when the agent ends. It sits on the agent, not its process, so a restart or another conversation keeps it. `status` names the voiced agent as `voice`, and `wheel agent voice <agent>` holds the lease and the mark until it is stopped.

A Claude agent takes the line through the `UserPromptSubmit` hook wheeld registers in `initialize` for every Claude process: each prompt fires it, a message sent while a turn runs included once the agent takes it in (checked on Claude Code 2.1.330), and wheeld answers with the line as `additionalContext` while the agent is voiced and with nothing otherwise. The line never shows in the transcript, and the person's message stays as they sent it. Codex and pi refuse `agent_voice`: Codex's `turn/start` has no per-turn instruction field, and pi has no hook.

This narrows two earlier rules. ADR-0054's "wheeld attaches nothing to the person's message" allows this one addition, made only while the person is in voice mode with that agent. ADR-0017's "the brief holds no instruction about how an agent should behave" allows the voice line, because how long a spoken reply may be is what voice mode needs to work, not the person's taste; `wheel guidance show` prints the line under its own heading so the person can read what the agent is told.

A line in the brief was ruled out because the brief is fixed when the process starts and could not be cleared when voice ends. Prefixing the person's message was ruled out because it would show in the transcript and change what they sent.
