-- The Queue pane: what waits on the person in a folder's queue (the orchestrator's, ADR-0054), as the
-- `wheelhouse.queue` feed groups it: Needs you, Ready for review, Finished. Each row reads its kind, where it came
-- from, its age and its title. The pane only shows the queue; the person answers in the orchestrator's chat.
local kit = require("lib/ui")
local time = require("lib/time")

local NONE = {}

-- The tone a row's kind word takes.
local TONES = { permission = "bad", bug = "bad", question = "warn", review = "ok", done = "muted", unconfirmed = "muted" }

local function row(it, now)
    local from = {}
    if it.space then table.insert(from, it.space) end
    if it.agent or it.pane then table.insert(from, it.agent or it.pane) end
    local head = kit.box("h", {
        kit.label(string.upper(it.kind or ""), { tone = TONES[it.kind] or "warn", size = 11, weight = "semibold" }),
        kit.label(table.concat(from, " · "), { tone = "muted", size = 11 }),
        kit.label(time.ago(it.created, now), { tone = "muted", size = 11, tabular = true }),
    }, { spacing = 8, widths = { false, "flex", false } })
    local title = kit.label(it.title or "", {})
    kit.modify(title, { lineLimit = 2 })
    return kit.box("v", { head, title }, { spacing = 3, padding = { 8, 0 }, edge = "bottom" })
end

local function group(g, now)
    local kids = { kit.label(string.upper(g.title) .. " · " .. #(g.items or NONE), { tone = "muted", size = 11, weight = "semibold" }) }
    for _, it in g.items or NONE do table.insert(kids, row(it, now)) end
    return kit.box("v", kids, { spacing = 2, padding = { 12, 0, 4, 0 } })
end

local function init(previous)
    return previous or {}
end

local function update(state, _msg)
    return state
end

local function view(_state, data)
    local q = data.feed("wheelhouse.queue", { dir = data.cwd })
    local body
    if q == nil then
        body = { kit.label("Reading the queue…", { tone = "muted" }) }
    elseif q.error then
        body = { kit.label(q.error, { tone = "bad" }) }
    elseif not q.has_queue then
        body = { kit.label("This folder keeps no queue (no .beads directory).", { tone = "muted" }) }
    else
        body = {}
        for _, g in q.groups or NONE do table.insert(body, group(g, q.now)) end
    end
    return ui.Column{ align = "stretch", kit.box("v", body, { align = "stretch", spacing = 0, padding = { 12, 16, 32, 16 } }) }
end

return { init = init, update = update, view = view }
