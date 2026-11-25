---
applies-to: ["daemon/src/voice.md", "daemon/src/claude.rs", "daemon/src/codex.rs", "daemon/src/pi.rs", "daemon/src/shell/pi-voice.ts", "daemon/src/leases.rs", "daemon/src/driver.rs", "apple/WheelUI/Sources/WheelUI/VoiceSession.swift", "clients/gpui/core/src/voice.rs"]
---

# ADR-0059: Voice mode speaks through a voice agent of the voiced agent's harness

While voice mode talks to an agent (the voiced agent: the agent `agent_voice` names while its connection holds the `voice` lease, ADR-0055), wheeld runs a voice agent beside it: a second driver process of the voiced agent's harness, on that harness's voice model, with no tools and no permission prompts, in a conversation of its own that is never a pane, never listed as an agent and never resumed. It starts when `agent_voice` names the agent and ends whenever ADR-0055's voice mark clears. The voice agent does the talking; the voiced agent writes its full reply into the chat as it would for typed text.

Each harness names its voice model in its driver: Claude `haiku`, Codex `gpt-6.1-terra` (checked against Codex 0.172.0's `model/list`, which calls it "Efficient model…"), checked against the harness's model list at start. A harness that names none refuses `agent_voice`; pi names none, so pi has no voice mode, and its extension `pi-voice.ts` and its `/wheel-voice` command go. Codex, which ADR-0055 refused because `turn/start` takes no per-turn instruction, gains voice mode, because the voice agent's brief goes in once as `developerInstructions` at `thread/start`.

wheeld feeds the voice agent, as one turn each and in order: the person's message as it is sent, each text block and tool-call title the voiced agent streams, and the voiced agent's final reply. Its brief tells it to say in one or two short spoken sentences only what those feeds show. A new person message interrupts a voice-agent turn still running. wheeld streams the voice agent's text to the lease holder as a voice event naming the voiced agent and the turn, outside the voiced agent's transcript, and both clients speak those events sentence by sentence instead of the last reply at turn end. A client whose wheeld's op list lacks the event speaks the reply at turn end (ADR-0050).

This narrows ADR-0055. The voiced agent's hidden line no longer asks for short spoken replies; it says that a voice agent speaks for it, that it writes its full reply, and that what must be seen goes in a card with `wheel agent card`. Claude keeps the `UserPromptSubmit` route for it; Codex gets no line. The lease, the mark and when it clears are unchanged.
