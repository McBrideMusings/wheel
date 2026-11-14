-- One tool call as a card: title, kind and status, then its content (text capped to 20 lines, diffs through lib/diff).
local style = require("lib/style")
local diff = require("lib/diff")

local M = {}

local MAX_TEXT_LINES = 20
local MAX_ITEMS = 8 -- content items drawn per card

local STATUS_COLOR = { completed = "green", failed = "red", in_progress = "orange", pending = "secondary" }

local function capped(text)
    local lines = {}
    for line in string.gmatch(text, "([^\n]*)\n?") do table.insert(lines, line) end
    if lines[#lines] == "" then table.remove(lines) end
    if #lines <= MAX_TEXT_LINES then return text end
    local kept = {}
    for i = 1, MAX_TEXT_LINES do kept[i] = lines[i] end
    table.insert(kept, "… " .. (#lines - MAX_TEXT_LINES) .. " more lines")
    return table.concat(kept, "\n")
end

-- `call`: the tool_call event with its tool_update events folded in. `waiting`: the call gates a permission request.
function M.view(call, waiting)
    local status = if waiting then "Waiting for permission" else (call.status or "")
    local color = if waiting then "orange" else (STATUS_COLOR[call.status or ""] or "secondary")
    local head = ui.Row{
        spacing = 8,
        children = {
            style.text(call.title or "", { size = 13, weight = "semibold", lines = 2 }),
            ui.Text{
                text = status,
                modifiers = {
                    { font = { size = 11, weight = "regular", design = "default" } },
                    { foregroundStyle = color },
                    { fixedSize = true },
                },
            },
        },
    }
    local kids = { head, style.text(call.tool_kind or "", { size = 11, color = "secondary" }) }
    local content = call.content or {}
    for i, item in content do
        if i > MAX_ITEMS then
            table.insert(kids, style.text("… " .. (#content - MAX_ITEMS) .. " more items", { size = 12, color = "secondary" }))
            break
        end
        if item.type == "text" and item.text and item.text ~= "" then
            table.insert(kids, style.text(capped(item.text), { size = 12, mono = true }))
        elseif item.type == "diff" then
            table.insert(kids, diff.view(item.old_text, item.new_text, item.path))
        end
    end
    return style.box(kids)
end

return M
