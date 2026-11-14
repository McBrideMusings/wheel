-- The agent view: one agent pane's conversation as a Luau pane. `wheel.agent.transcript`, `.status` and `.requests` are
-- the agent's feeds; `wheel.agent.send` is its composer. The permission prompt is the host's card under this pane.
--
-- Node ids are positional, so root holds [header, transcript, composer, note] and never changes.
local status_header = require("lib/status_header")
local transcript = require("lib/transcript")
local composer = require("lib/composer")

-- The last value of each feed. A params set reads nil until the feed answers; the view draws these meanwhile.
local LAST = {}

local TRANSCRIPT_AT = "root.1"

local function init(previous)
    local state = previous or {}
    composer.init(state)
    return state
end

local function update(state, msg)
    composer.update(state, msg)
    return state
end

local function read(data, name, params)
    local value, stale = data.feed(name, params)
    if value ~= nil then LAST[name] = value end
    return LAST[name], stale
end

local function view(state, data)
    local pane = data.pane
    state.agent = pane
    local params = { agent = pane }
    local status, stale = read(data, "wheel.agent.status", params)
    local events = read(data, "wheel.agent.transcript", params)
    local requests = read(data, "wheel.agent.requests", params)
    return ui.Column{
        spacing = 0,
        children = {
            status_header.view(pane, status, stale),
            transcript.view(events, requests, TRANSCRIPT_AT),
            composer.view(state),
            composer.note(state),
        },
    }
end

return { init = init, update = update, view = view }
