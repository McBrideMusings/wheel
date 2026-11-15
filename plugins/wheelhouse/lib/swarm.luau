-- The Swarm pane's run section: the latest swarm run of the project from the `wheelhouse.swarm` feed (the run log, each
-- worker's pane and its transcript), as counts, one table section per worker holding its latest actions with their times,
-- and the run log itself. A worker in flight whose transcript has not been written for `stuck_after` seconds is stuck.
local kit = require("lib/ui")

local M = {}

local NONE = {}
local OUTCOME_TONES = { landed = "ok", parked = "warn", failed = "bad", reopened = "warn", finished = "muted", ready = "accent", rejected = "warn", signed = "ok", declined = "bad" }

local function count(list, keep)
    local n = 0
    for _, x in list do
        if keep(x) then n += 1 end
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

local function row(id, cells)
    return { id = id, path = id, depth = 0, kids = 0, collapsed = false, cells = cells, menu = {} }
end

-- What a worker is: its model, its pane and agent state, its declared stage and note.
local function about(w)
    local parts = { w.model }
    if w.pane then table.insert(parts, "pane " .. w.pane .. (if w.status then " (" .. w.status .. ")" else "")) end
    if w.stage then table.insert(parts, w.stage .. (if w.note and w.note ~= "" then ": " .. w.note else "")) end
    return table.concat(parts, " · ")
end

-- The first row of a worker's section: the bead, when it was launched, what it is and where it stands now.
local function head_row(w)
    local state, tone, text
    if w.stuck then
        state, tone = "stuck", "bad"
        text = "no transcript write since " .. (w.stuck_since_clock or "launch")
    elseif w.in_flight then
        state, tone = "in flight", "ok"
        text = if w.last_write_clock then "transcript written " .. w.last_write_clock else "no transcript yet"
    else
        state, tone = w.outcome or "done", OUTCOME_TONES[w.outcome or ""] or "muted"
        text = (w.outcome_clock or "") .. (if w.outcome_detail and w.outcome_detail ~= "" then "  " .. w.outcome_detail else "")
    end
    return row(w.bead .. ":head", {
        kit.cell(w.launched_clock or "", { key = "time", mono = true, tone = "muted" }),
        kit.cell(w.bead, { key = "what", mono = true, tone = "fg" }),
        kit.cell(about(w) .. " · " .. text, { key = "text", tone = if w.stuck then "bad" else "fg" }),
        kit.cell(state, { key = "state", chip = true, tone = tone }),
    })
end

local function action_row(w, a, i)
    local state, tone
    if a.tool == "" then
        state, tone = "said", "muted"
    elseif not a.done then
        state, tone = "running", "accent"
    elseif a.error then
        state, tone = "error", "bad"
    else
        state, tone = "ok", "muted"
    end
    return row(w.bead .. ":" .. i, {
        kit.cell(a.clock or "", { key = "time", mono = true, tone = "muted" }),
        kit.cell(if a.tool == "" then "said" else a.tool, { key = "what", mono = a.tool ~= "" }),
        kit.cell(a.text or "", { key = "text", tone = if a.tool == "" then "muted" else "fg" }),
        kit.cell(state, { key = "state", chip = true, tone = tone }),
    })
end

local function workers_table(workers)
    local sections = {}
    -- Workers in flight first, stuck ones at the top of them, then the finished ones; each keeps its launch order.
    for _, pass in { "stuck", "flight", "done" } do
        for _, w in workers do
            local here = if pass == "stuck" then w.stuck elseif pass == "flight" then w.in_flight and not w.stuck else not w.in_flight
            if here then
                local rows = { head_row(w) }
                for i, a in w.actions or NONE do
                    table.insert(rows, action_row(w, a, i))
                end
                -- No title: a section title draws in the first column, too narrow for it; the head row says it.
                table.insert(sections, { title = "", count = #rows, rows = rows })
            end
        end
    end
    return kit.sized_table({
        component = "Table",
        columns = {
            kit.column("time", "Time", 72, false, false, false), kit.column("what", "Action", 96, false, false, false),
            kit.column("text", "Detail", "flex", false, false, false), kit.column("state", "State", 88, false, false, false),
        },
        sections = sections,
        rowAction = { type = "noop" }, foldAction = { type = "noop" },
        empty = "No worker launched in this run.",
    })
end

local function log_table(events)
    local rows = {}
    for i, e in events do
        table.insert(rows, row("ev:" .. i, {
            kit.cell(e.clock or "", { key = "time", mono = true, tone = "muted" }),
            kit.cell(e.event, { key = "event", chip = true, tone = OUTCOME_TONES[e.event] or "fg" }),
            kit.cell(e.bead or "", { key = "bead", mono = true }),
            kit.cell(e.detail or "", { key = "detail", tone = "muted" }),
        }))
    end
    return kit.sized_table({
        component = "Table",
        columns = {
            kit.column("time", "Time", 72, false, false, false), kit.column("event", "Event", 96, false, false, false),
            kit.column("bead", "Bead", 170, false, false, false), kit.column("detail", "Detail", "flex", false, false, false),
        },
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "noop" }, foldAction = { type = "noop" },
        empty = "The run log is empty.",
    })
end

local function heading(text)
    return kit.box("h", { kit.label(text, { size = 15, weight = "semibold" }) }, { padding = { 14, 0, 6, 0 } })
end

-- The run section for the feed value `snap` (nil before the first value arrives).
function M.section(snap)
    if snap == nil then
        return kit.label("Reading the swarm run…", { symbol = "spinner", tone = "muted" })
    end
    if snap.state == "no-run" then
        local why = if snap.error then snap.error else "No swarm run log at " .. (snap.log or "?")
        return kit.box("v", { heading("Run"), kit.label(why, { tone = "muted" }) }, { align = "start", spacing = 0 })
    end
    local workers = snap.workers or NONE
    local line = "started " .. (snap.started_clock or "?")
    if snap.space then line = "space " .. snap.space .. " · " .. line end
    if snap.lead_pid then line ..= " · lead pid " .. tostring(snap.lead_pid) end
    if snap.state == "ended" then
        line ..= " · ended " .. (snap.ended_clock or "") .. (if snap.end_note and snap.end_note ~= "" then " (" .. snap.end_note .. ")" else "")
    else
        line ..= " · live"
    end
    local kpi = {
        { "Launched", #workers, "fg" },
        { "In flight", count(workers, function(w) return w.in_flight end), "ok" },
        { "Stuck", count(workers, function(w) return w.stuck end), "bad" },
        { "Landed", count(workers, function(w) return w.outcome == "landed" end), "fg" },
        { "Parked", count(workers, function(w) return w.outcome == "parked" end), "warn" },
        { "Failed", count(workers, function(w) return w.outcome == "failed" end), "bad" },
    }
    local cards, widths = {}, {}
    for i, k in kpi do
        cards[i] = stat(k[1], k[2], k[3])
        widths[i] = "flex"
    end
    local kids = {
        heading("Run"),
        kit.label(line, { tone = "muted" }),
        kit.box("h", cards, { spacing = 10, widths = widths, padding = { 10, 0, 0, 0 } }),
        heading("Workers"),
        workers_table(workers),
        heading("Run log"),
        log_table(snap.events or NONE),
    }
    if snap.error then table.insert(kids, 2, kit.label(snap.error, { tone = "bad" })) end
    return kit.box("v", kids, { align = "stretch", spacing = 0 })
end

return M
