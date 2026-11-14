-- Text and boxes with the modifiers the agent view uses. Every module that draws text goes through `style.text`.
local M = {}

-- One Text. `o`: size, weight, mono, color (a SwiftUI colour name), lines (a line limit).
function M.text(body, o)
    o = o or {}
    local mods = {}
    if o.size or o.weight or o.mono then
        table.insert(mods, {
            font = { size = o.size or 13, weight = o.weight or "regular", design = if o.mono then "monospaced" else "default" },
        })
    end
    if o.color then table.insert(mods, { foregroundStyle = o.color }) end
    if o.lines then table.insert(mods, { lineLimit = o.lines }) end
    table.insert(mods, { frame = { maxWidth = "infinity", alignment = "leading" } })
    return ui.Text{ text = body, modifiers = mods }
end

-- A VStack of `children` in a tinted rounded box.
function M.box(children, o)
    o = o or {}
    return ui.Column{
        spacing = o.spacing or 4,
        children = children,
        modifiers = {
            { padding = o.padding or { 6, 8 } },
            { background = o.fill or "controlBackground" },
            { clipShape = { cornerRadius = 8 } },
            { frame = { maxWidth = "infinity", alignment = "leading" } },
        },
    }
end

return M
