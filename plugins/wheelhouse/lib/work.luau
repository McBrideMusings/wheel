-- The Implement and Swarm panes: the agent panes of the project the space opened, as passes by item and stage
-- (Implement) or as workers by state (Swarm), and starting an agent. The data is
-- the `wheelhouse.work` feed; `wheelhouse.agent_start` starts an agent from the brief typed so far. `update` cannot
-- read feeds, so `view` stores the directory in `state`.
local kit = require("lib/ui")

local M = {}

local NONE = {}
local STAGES = { "plan", "edit", "build", "verify", "review", "gate" }
-- Every status `wheel agent watch` reports (the daemon's PaneStatus), then Unknown for a pane that reported none.
local STATES = {
    { "needs_you", "Needs you" }, { "working", "Working" }, { "done", "Done" }, { "idle", "Idle" }, { "stopped", "Stopped" },
}
local TONES = { working = "ok", needs_you = "warn", stopped = "bad", done = "accent", idle = "muted" }
local NOTICE_HEIGHT = 44

local function known(s)
    for _, x in STATES do
        if x[1] == s then return true end
    end
    return false
end

local function capitalise(s)
    return string.upper(string.sub(s, 1, 1)) .. string.sub(s, 2)
end

local function status_of(a)
    return a.status or "unknown"
end

local function who(a)
    if a.model then return a.agent .. " · " .. a.model end
    return a.agent
end

local function state_cell(a)
    local s = status_of(a)
    return kit.cell(if s == "needs_you" then "needs you" else s, { key = "status", chip = true, tone = TONES[s] or "muted" })
end

local function row_of(a, i, cells)
    local id = a.pane .. ":" .. (a.item or "") .. ":" .. i
    return { id = id, path = id, depth = 0, kids = 0, collapsed = false, cells = cells, menu = {} }
end

local function implement_row(a, i)
    return row_of(a, i, {
        kit.cell(a.item or "no item declared", { key = "item", mono = a.item ~= nil, tone = if a.item then "fg" else "muted" }),
        if a.stage then kit.cell(a.stage, { key = "stage", chip = true, tone = "fg" }) else kit.cell("", { key = "stage" }),
        kit.cell(a.note or "", { key = "note", tone = "muted" }),
        kit.cell(who(a), { key = "agent", tone = "muted" }),
        state_cell(a),
    })
end

local function swarm_row(a, i)
    return row_of(a, i, {
        kit.cell(a.pane, { key = "pane", mono = true }),
        kit.cell(who(a), { key = "agent" }),
        kit.cell(a.item or "", { key = "item", mono = true }),
        kit.cell(a.stage or "", { key = "stage", tone = "muted" }),
        kit.cell(a.note or "", { key = "note", tone = "muted" }),
        state_cell(a),
    })
end

local function is_stage(s)
    for _, x in STAGES do
        if x == s then return true end
    end
    return false
end

-- The agents `keep` accepts, as table rows.
local function section(title, agents, keep, make)
    local rows = {}
    for i, a in agents do
        if keep(a) then table.insert(rows, make(a, i)) end
    end
    if #rows == 0 then return nil end
    return { title = title, count = #rows, rows = rows }
end

local function count(agents, keep)
    local n = 0
    for _, a in agents do
        if keep(a) then n += 1 end
    end
    return n
end

local function stat(label, value, tone)
    local num = kit.label(tostring(value), { size = 20, weight = "semibold", tabular = true, tone = if value == 0 then "muted" else tone })
    local cap = kit.label(label, { tone = "muted", size = 11 })
    kit.modify(cap, { lineLimit = 1 })
    local inner = kit.box("v", { num, cap }, { spacing = 2, align = "start", padding = { 7, 8 }, border = "line", radius = 8 })
    kit.modify(inner, { frame = { maxWidth = "infinity", alignment = "leading" } })
    return inner
end

local function kpis(mode, agents)
    local list = {}
    if mode == "implement" then
        table.insert(list, { "Items", count(agents, function(a) return a.item ~= nil end), "fg" })
        for _, s in STAGES do
            table.insert(list, { capitalise(s), count(agents, function(a) return a.item ~= nil and a.stage == s end), "fg" })
        end
    else
        table.insert(list, { "Workers", #agents, "fg" })
        table.insert(list, { "Working", count(agents, function(a) return status_of(a) == "working" end), "ok" })
        table.insert(list, { "Needs you", count(agents, function(a) return status_of(a) == "needs_you" end), "warn" })
        table.insert(list, { "Done", count(agents, function(a) return status_of(a) == "done" end), "fg" })
        table.insert(list, { "Idle", count(agents, function(a) return status_of(a) == "idle" end), "fg" })
    end
    local cards, widths = {}, {}
    for i, k in list do
        cards[i] = stat(k[1], k[2], k[3])
        widths[i] = "flex"
    end
    return kit.box("h", cards, { spacing = 10, widths = widths })
end

local function agents_table(mode, agents)
    local sections = {}
    local columns, empty
    if mode == "implement" then
        for _, s in STAGES do
            table.insert(sections, section(capitalise(s), agents, function(a) return a.item ~= nil and a.stage == s end, implement_row) or false)
        end
        table.insert(sections, section("No stage declared", agents, function(a) return a.item ~= nil and not is_stage(a.stage) end, implement_row) or false)
        table.insert(sections, section("Agents with no item", agents, function(a) return a.item == nil end, implement_row) or false)
        columns = {
            kit.column("item", "Item", "flex", false, false, false), kit.column("stage", "Stage", 90, false, false, false),
            kit.column("note", "Doing", "flex", false, true, false), kit.column("agent", "Agent", 180, false, true, false),
            kit.column("status", "State", 96, false, false, false),
        }
        empty = "No agent is running in this project."
    else
        for _, s in STATES do
            table.insert(sections, section(s[2], agents, function(a) return status_of(a) == s[1] end, swarm_row) or false)
        end
        table.insert(sections, section("Unknown", agents, function(a) return not known(status_of(a)) end, swarm_row) or false)
        columns = {
            kit.column("pane", "Pane", 64, false, false, false), kit.column("agent", "Agent", 180, false, true, false),
            kit.column("item", "Item", 170, false, false, false), kit.column("stage", "Stage", 80, false, false, false),
            kit.column("note", "Doing", "flex", false, true, false), kit.column("status", "State", 96, false, false, false),
        }
        empty = "No workers: start an agent above."
    end
    local shown = {}
    for _, s in sections do
        if s then table.insert(shown, s) end
    end
    return kit.sized_table({
        component = "Table", columns = columns, sections = shown,
        rowAction = { type = "noop" }, foldAction = { type = "noop" },
        sort = { key = "status", asc = true },
        empty = empty,
    })
end

local function notice_box(text, bad)
    local tone = if bad then "bad" else "ok"
    local l = kit.label(text, { tone = tone })
    return kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 } })
end

-- The notice slot: `text` in a box, or nothing, in a slot of the same height either way.
local function notice_slot(text, bad)
    local kids = {}
    if text then table.insert(kids, notice_box(text, bad)) end
    local slot = ui.Column{ align = "stretch", spacing = 0, table.unpack(kids) }
    slot.modifiers = { { frame = { height = NOTICE_HEIGHT, alignment = "center" } } }
    return slot
end

-- The pane script for `mode`, "implement" or "swarm".
function M.pane(mode)
    local title = if mode == "implement" then "Implement" else "Swarm"

    local function view(state, data)
        local snap = data.feed("wheelhouse.work", { dir = data.cwd })
        state.dir = data.cwd
        if snap == nil then
            return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading agents…" } }
        end
        local loaded = snap.loaded == true
        local agents = snap.agents or NONE
        local head = kit.box("h", { kit.label(title .. " · " .. (snap.name or ""), { size = 20, weight = "bold" }) }, { padding = { 0, 0, 12, 0 } })

        local notice
        if state.said then notice = notice_slot(state.said, state.said_bad) else notice = notice_slot(snap.error, true) end

        local brief = state.brief or ""
        local field = ui.TextField{
            label = "Brief for a new agent", value = brief, variant = "shortText", on_change = { type = "set_brief" },
            modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
        }
        local start = kit.button("Start agent", { type = "start_agent" }, { disabled = string.match(brief, "^%s*$") ~= nil })
        local starter = kit.box("h", { field, start }, { spacing = 8, widths = { "flex", false }, padding = { 12, 0, 0, 0 } })

        local body
        if loaded then
            body = agents_table(mode, agents)
        else
            body = kit.label("Loading agents…", { symbol = "spinner", tone = "muted" })
        end
        return ui.Column{ align = "stretch", kit.box("v", {
            head,
            kpis(mode, if loaded then agents else NONE),
            notice,
            starter,
            kit.box("v", { body }, { padding = { 12, 0, 0, 0 } }),
        }, { align = "start", spacing = 0, padding = { 18, 16, 48, 16 } }) }
    end

    local function init(previous)
        return previous or { brief = "", said = nil, said_bad = false, dir = nil }
    end

    local function update(state, msg)
        if msg.action then
            if msg.action == "wheelhouse.agent_start" then
                if msg.error ~= nil then
                    state.said, state.said_bad = msg.error, true
                else
                    state.said = (type(msg.result) == "table" and msg.result.said) or "Started the agent."
                    state.said_bad = false
                    state.brief = ""
                end
            end
            return
        end
        if msg.type == "set_brief" then
            state.brief = if type(msg.value) == "string" then msg.value else ""
        elseif msg.type == "start_agent" and state.dir and not string.match(state.brief or "", "^%s*$") then
            state.said = nil
            wheel.action("wheelhouse.agent_start", { dir = state.dir, brief = state.brief })
        end
    end

    return { init = init, update = update, view = view }
end

return M
