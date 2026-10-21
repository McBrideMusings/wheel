# ADR-0013: Wheel runs only what it ships or names

Everything Wheel runs is inside its app bundle, except the agent programs a person uses it with: `claude`, `codex`, `pi`. wheeld talks to each through that program's own protocol (`claude --print` with stream-json, `codex app-server`), with no adapter, runtime or package between them. Each agent's program is located by a setting, detected when wheeld starts, and listed in Settings as found or missing, with how to install it. The pane picker offers only the agents that were found, and Wheel runs with none of them.
