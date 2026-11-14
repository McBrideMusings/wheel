-- The one fixed-height row at the top: the agent pane, its phase, the queued count and whether the feed is stale.
local style = require("lib/style")

local M = {}

local HEIGHT = 28

-- `status`: the wheel.agent.status value (or nil before it is read).
function M.view(pane, status, stale)
    local st = status and status.state or {}
    local phase = st.phase or "…"
    local queued = st.queued or 0
    local right = (if queued > 0 then queued .. " queued" else "") .. (if stale then (if queued > 0 then "  " else "") .. "stale" else "")
    return ui.Row{
        spacing = 8,
        children = {
            style.text(pane or "", { size = 13, weight = "semibold" }),
            ui.Text{
                text = phase,
                modifiers = {
                    { font = { size = 12, weight = "regular", design = "default" } },
                    { foregroundStyle = if phase == "busy" then "orange" else "secondary" },
                    { frame = { minWidth = 56, alignment = "leading" } },
                },
            },
            ui.Text{
                text = right,
                modifiers = {
                    { font = { size = 12, weight = "regular", design = "default" } },
                    { foregroundStyle = if stale then "red" else "secondary" },
                    { frame = { minWidth = 96, alignment = "trailing" } },
                },
            },
        },
        modifiers = { { padding = { 0, 10 } }, { frame = { height = HEIGHT, maxWidth = "infinity", alignment = "leading" } } },
    }
end

return M
