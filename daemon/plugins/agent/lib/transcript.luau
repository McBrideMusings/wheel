-- The transcript as one List, a row per shown event. Tool calls and their later updates are folded into one card.
local style = require("lib/style")
local tool_card = require("lib/tool_card")

local M = {}

local MAX_ROWS = 300 -- the last rows drawn
-- The JSON values the rows may hold: the runtime refuses a tree over 100,000, and the header, composer and note need
-- few. Rows are taken newest first until the next one would pass it.
local MAX_VALUES = 80000

-- The JSON values a node converts to, as the runtime counts them: every table and every scalar in it.
local function values(v)
    if type(v) ~= "table" then return 1 end
    local n = 1
    for _, x in pairs(v) do n += values(x) end
    return n
end

-- The events as shown items: each `tool_call` absorbs the `tool_update` events of its id. Kinds that are not drawn
-- (turn_start, turn_end, mode, *_revised, retract, unknown) are dropped.
function M.fold(events)
    local items = {}
    local calls = {}
    for _, e in events or {} do
        local kind = e.kind
        if kind == "tool_call" then
            local call = {
                kind = "tool_call", id = e.id, title = e.title, tool_kind = e.tool_kind, status = e.status,
                content = e.content or {}, detail = e.detail,
            }
            calls[e.id] = call
            table.insert(items, call)
        elseif kind == "tool_update" then
            local call = calls[e.id]
            if call then
                if e.title then call.title = e.title end
                if e.status then call.status = e.status end
                if e.content then call.content = e.content end
            end
        elseif kind == "user" or kind == "text" or kind == "thought" or kind == "plan" or kind == "error" then
            table.insert(items, e)
        end
    end
    return items
end

local PLAN_MARK = { completed = "☑", in_progress = "◐", pending = "☐" }

local function row(item, waiting)
    local kind = item.kind
    if kind == "user" then
        return style.box({ style.text("You", { size = 11, weight = "semibold", color = "secondary" }), style.text(item.text or "") })
    elseif kind == "text" then
        return style.text(item.text or "")
    elseif kind == "thought" then
        return style.box({
            style.text("Thinking", { size = 11, weight = "semibold", color = "secondary" }),
            style.text(item.text or "", { size = 12, color = "secondary" }),
        }, { fill = "background" })
    elseif kind == "tool_call" then
        return tool_card.view(item, item.id ~= nil and waiting[item.id] == true)
    elseif kind == "plan" then
        local kids = { style.text("Plan", { size = 11, weight = "semibold", color = "secondary" }) }
        for _, entry in item.entries or {} do
            table.insert(kids, style.text((PLAN_MARK[entry.status or ""] or "☐") .. " " .. (entry.content or ""), { size = 12 }))
        end
        return style.box(kids)
    else -- error
        return style.text(item.message or "", { color = "red" })
    end
end

-- `transcript`: the wheel.agent.transcript value; `requests`: the wheel.agent.requests value. `at`: the transcript's
-- positional id (the List's place in its parent), so scrollTo can name its last row.
function M.view(transcript, requests, at)
    local waiting = {}
    for _, r in (requests and requests.requests) or {} do
        if r.tool_call then waiting[r.tool_call] = true end
    end
    local items = M.fold(transcript and transcript.events)
    local newest, spent = {}, 0
    for i = #items, math.max(1, #items - MAX_ROWS + 1), -1 do
        local r = row(items[i], waiting)
        spent += values(r)
        if spent > MAX_VALUES then break end
        table.insert(newest, r)
    end
    local rows = {}
    for i = #newest, 1, -1 do table.insert(rows, newest[i]) end
    if #rows == 0 then rows[1] = style.text("Give this agent a task.", { color = "secondary" }) end
    rows.spacing = 8
    -- Padding, not a frame: a List with no frame fills the pane under the header and over the composer.
    rows.modifiers = { { padding = { 4, 10 } } }
    rows.scrollTo = at .. "." .. (#rows - 1)
    return ui.List(rows)
end

return M
