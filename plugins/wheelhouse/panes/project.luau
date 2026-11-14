-- The Project pane: the project's status against its vision (six counts and the epics) and editing the vision.
-- The data is the `wheelhouse.project` feed; `wheelhouse.vision_save`
-- writes docs/CONTEXT.md. The pane's own state (editing, the draft text, the last result) lives in `state`; `update`
-- cannot read feeds, so `view` stores the directory and the vision there.
local kit = require("lib/ui")

local NONE = {}

local STATS = {
    { "Done", "done", "ok" }, { "In progress", "progress", "fg" }, { "Ready", "ready", "ok" },
    { "Blocked", "blocked", "warn" }, { "Open", "open", "fg" }, { "Deferred", "deferred", "muted" },
}

-- The vision row and the notice slot have one height in every state, so nothing below them moves.
local VISION_HEIGHT = 32
local NOTICE_HEIGHT = 44

local function box_of(text, tone)
    local l = kit.label(text, { tone = tone })
    return kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 } })
end

-- `text` in a box, or nothing, in a slot of the same height either way.
local function notice_slot(text, tone)
    local kids = {}
    if text then table.insert(kids, box_of(text, tone)) end
    local slot = ui.Column{ align = "stretch", spacing = 0, table.unpack(kids) }
    slot.modifiers = { { frame = { height = NOTICE_HEIGHT, alignment = "center" } } }
    return slot
end

local function stat(label, value, tone)
    local num = kit.label(tostring(value), { size = 20, weight = "semibold", tabular = true, tone = if value == 0 then "muted" else tone })
    local cap = kit.label(label, { tone = "muted", size = 11 })
    kit.modify(cap, { lineLimit = 1 })
    local inner = kit.box("v", { num, cap }, { spacing = 2, align = "start", padding = { 7, 8 }, border = "line", radius = 8 })
    kit.modify(inner, { frame = { maxWidth = "infinity", alignment = "leading" } })
    return inner
end

local function epics_table(epics)
    local rows = {}
    for i, e in epics do
        local progress = if e.closed then "done" else e.done .. "/" .. e.all
        local cells = {
            kit.cell(e.id, { key = "id", mono = true }),
            kit.cell(e.title, { key = "title" }),
            kit.cell(progress, { key = "progress", mono = true, tabular = true }),
            kit.cell(if e.ready > 0 then e.ready .. " ready" else "", { key = "ready", tone = "ok" }),
        }
        rows[i] = { id = e.id, path = e.id, depth = 0, kids = 0, collapsed = false, dim = e.closed == true, cells = cells, menu = {} }
    end
    return kit.sized_table({
        component = "Table",
        columns = {
            kit.column("id", "Epic", 110, false, false, false), kit.column("title", "Title", "flex", false, false, false),
            kit.column("progress", "Done", 70, false, false, false), kit.column("ready", "Ready", 90, false, false, false),
        },
        sections = { { title = "Epics", count = #rows, rows = rows } },
        rowAction = { type = "noop" }, foldAction = { type = "noop" },
        sort = { key = "id", asc = true },
        empty = "No epics.",
    })
end

local function vision_row(st, snap)
    if st.editing then
        local draft = st.draft or ""
        local field = ui.TextField{
            label = "What this project is for", value = draft, variant = "shortText", on_change = { type = "set_draft" },
            modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
        }
        local save = kit.button("Save", { type = "save_vision" }, { disabled = string.match(draft, "^%s*$") ~= nil })
        local cancel = kit.button("Cancel", { type = "cancel_vision" }, { style = "link" })
        local row = kit.box("h", { field, save, cancel }, { spacing = 8, widths = { "flex", false, false } })
        return kit.modify(row, { frame = { height = VISION_HEIGHT } })
    end
    local text = snap.vision or "No vision yet: the first paragraph of docs/CONTEXT.md."
    local v = kit.label(text, { size = 14, tone = if snap.vision then "fg" else "muted" })
    kit.modify(v, { lineLimit = 1, help = text })
    local edit = kit.button("Edit vision", { type = "edit_vision" }, { style = "link" })
    local row = kit.box("h", { v, edit }, { spacing = 12, widths = { "flex", false } })
    return kit.modify(row, { frame = { height = VISION_HEIGHT } })
end

local function view(state, data)
    local snap = data.feed("wheelhouse.project", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the project…" } }
    end
    state.dir, state.vision = data.cwd, snap.vision

    local head = kit.box("h", { kit.label(snap.name or "", { size = 20, weight = "bold" }) }, { padding = { 0, 0, 8, 0 } })
    local notice
    if state.said then
        notice = notice_slot(state.said, if state.said_bad then "bad" else "ok")
    else
        notice = notice_slot(snap.error, "bad")
    end

    local body = {}
    if not snap.has_tracker then
        table.insert(body, kit.label("This project keeps no tracker (no .beads directory).", { tone = "muted" }))
    elseif not snap.error then
        local counts = snap.counts or NONE
        local cards, widths = {}, {}
        for i, s in STATS do
            cards[i] = stat(s[1], counts[s[2]] or 0, s[3])
            widths[i] = "flex"
        end
        table.insert(body, kit.box("h", cards, { spacing = 10, widths = widths }))
        table.insert(body, kit.box("v", { epics_table(snap.epics or NONE) }, { padding = { 18, 0, 0, 0 } }))
    end

    return ui.Column{ align = "stretch", kit.box("v", {
        head,
        vision_row(state, snap),
        notice,
        kit.box("v", body, { spacing = 8 }),
    }, { align = "start", spacing = 0, padding = { 18, 16, 48, 16 } }) }
end

local function init(previous)
    return previous or { editing = false, draft = "", said = nil, said_bad = false, dir = nil, vision = nil }
end

local function update(state, msg)
    if msg.action then
        if msg.action == "wheelhouse.vision_save" then
            if msg.error ~= nil then
                state.said, state.said_bad = msg.error, true
            else
                state.said = (type(msg.result) == "table" and msg.result.said) or "Vision saved"
                state.said_bad = false
                state.editing = false
            end
        end
        return
    end
    local t = msg.type
    if t == "set_draft" then
        state.draft = if type(msg.value) == "string" then msg.value else ""
        return
    end
    state.said, state.said_bad = nil, false
    if t == "edit_vision" then
        state.editing, state.draft = true, state.vision or ""
    elseif t == "cancel_vision" then
        state.editing = false
    elseif t == "save_vision" and state.dir then
        wheel.action("wheelhouse.vision_save", { dir = state.dir, text = state.draft })
    end
end

return { init = init, update = update, view = view }
