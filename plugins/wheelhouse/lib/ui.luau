-- Styled text, stacks, buttons and table pieces for the wheelhouse panes. A page speaks tones, sizes, spacings and widths; this turns them into the components and
-- SwiftUI modifiers the client draws (ADR-0031, ADR-0038).
local M = {}

local COLOURS = {
    fg = "primary", muted = "secondary", ok = "green", warn = "orange", bad = "red", accent = "tint", focus = "tint",
    purple = "purple", todo = "tertiary", faint = "tertiary", line = "separator", card = "controlBackground",
    soft = "controlBackground", bg = "controlBackground",
}

-- A tone's SwiftUI colour name, or nil.
function M.colour(tone)
    return COLOURS[tone]
end

-- Adds a modifier to a node.
function M.modify(node, modifier)
    local m = node.modifiers
    if m then table.insert(m, modifier) else node.modifiers = { modifier } end
    return node
end

-- The modifiers a text's style `e` asks for: size, weight, mono, tone, tabular, upper, tracking, align, chip.
local function text_modifiers(e)
    local m = {}
    local size = e.size or 13
    local weight = e.weight or "regular"
    local mono = e.mono == true
    if size ~= 13 or weight ~= "regular" or mono then
        table.insert(m, { font = { size = size, weight = weight, design = if mono then "monospaced" else "default" } })
    end
    local c = e.tone and COLOURS[e.tone]
    if c then table.insert(m, { foregroundStyle = c }) end
    if e.tabular then table.insert(m, { monospacedDigit = true }) end
    if e.upper then table.insert(m, { textCase = "uppercase" }) end
    if e.tracking then table.insert(m, { tracking = e.tracking }) end
    if e.align == "end" then
        table.insert(m, { multilineTextAlignment = "trailing" })
        table.insert(m, { frame = { maxWidth = "infinity", alignment = "trailing" } })
    end
    if e.chip then
        table.insert(m, { padding = { horizontal = 4 } })
        table.insert(m, { overlay = { stroke = "separator", cornerRadius = 4 } })
    end
    return m
end

local function text(body, e)
    local n = { component = "Text", text = body }
    local m = text_modifiers(e)
    if #m > 0 then n.modifiers = m end
    if e.markdown then n.markdown = true end
    return n
end

-- Styled text. With a `dot`, a `symbol` or `runs` it is a Row of an Icon or ProgressView and the text(s); otherwise one Text.
function M.label(body, e)
    e = e or {}
    local tone = e.tone and COLOURS[e.tone]
    local lead
    if e.dot then
        local name = if e.dot == "ring" or e.dot == "hollow" then "circle" else "circle.fill"
        lead = ui.Icon{ name = name, modifiers = { { font = { size = 8 } } } }
        if tone then table.insert(lead.modifiers, { foregroundStyle = tone }) end
    elseif e.symbol then
        if e.symbol == "spinner" then
            lead = ui.ProgressView{ modifiers = { { frame = { width = 14, height = 14 } } } }
        else
            lead = ui.Icon{ name = e.symbol }
            if tone then lead.modifiers = { { foregroundStyle = tone } } end
        end
    end
    if not lead and not e.runs then return text(body, e) end
    local kids = {}
    if lead then table.insert(kids, lead) end
    if e.runs then
        for _, r in e.runs do
            local style = {}
            for k, v in e do if k ~= "runs" then style[k] = v end end
            for k, v in r do style[k] = v end
            table.insert(kids, text(r.text or "", style))
        end
    else
        table.insert(kids, text(body, e))
    end
    return { component = "Row", children = kids, align = "center", spacing = 5 }
end

-- A Row (axis "h") or Column of `children`. `e`: spacing, padding, margin, widths (a number, "flex" or false per child),
-- border, fill, radius, edge (top or bottom rule), dividers, align, wrap (a row that scrolls sideways), mono.
function M.box(axis, children, e)
    e = e or {}
    local horizontal = axis == "h"
    local kids = children
    if e.widths then
        for i, w in e.widths do
            local kid = kids[i]
            if kid then
                if type(w) == "number" then
                    M.modify(kid, { frame = { width = w, alignment = "leading" } })
                elseif w == "flex" then
                    M.modify(kid, { frame = { maxWidth = "infinity", alignment = "leading" } })
                end
            end
        end
    end
    if horizontal and e.dividers then
        local with = {}
        for i, k in kids do
            if i > 1 then table.insert(with, ui.Divider{ axis = "vertical" }) end
            table.insert(with, k)
        end
        kids = with
    end
    if e.edge then
        local d = ui.Divider{}
        if e.edge == "top" then table.insert(kids, 1, d) else table.insert(kids, d) end
    end
    local outlined = e.border ~= nil or e.fill ~= nil
    local radius = e.radius or (if outlined then 8 else 0)
    local m = {}
    if e.mono then table.insert(m, { font = { size = 13, design = "monospaced" } }) end
    if e.padding then
        table.insert(m, { padding = e.padding })
    elseif outlined or e.edge then
        table.insert(m, { padding = { 10, 12 } })
    end
    local fill = e.fill and (if e.fill == "ready" then "controlBackground" else COLOURS[e.fill])
    if fill then table.insert(m, { background = fill }) end
    local border = e.border and COLOURS[e.border]
    if border then table.insert(m, { overlay = { stroke = border, cornerRadius = radius } }) end
    if radius > 0 then table.insert(m, { clipShape = { cornerRadius = radius } }) end
    if e.margin then table.insert(m, { padding = e.margin }) end
    local align = if e.align == "end" then "end" elseif e.align == "center" or e.align == "firstTextBaseline" then "center" else "start"
    local wrap = horizontal and e.wrap
    local v = {
        component = if wrap then "List" elseif horizontal then "Row" else "Column",
        children = kids,
        spacing = e.spacing or (if horizontal then 12 else 8),
    }
    if wrap then
        v.direction = "horizontal"
    else
        v.align = if horizontal and align == "start" then "center" else align
    end
    if #m > 0 then v.modifiers = m end
    return v
end

-- A library Button whose label is a Text (or an Icon when `e.icon`). `e`: style link or plain draws it borderless, primary
-- draws it prominent; fill stretches it across; disabled disables it (hideOff also hides it); help is its hover text;
-- tint, tone; label_modifiers go on the label.
function M.button(body, action, e)
    e = e or {}
    local child
    if e.icon then
        child = ui.Icon{ name = e.icon }
    else
        child = ui.Text{ text = body }
        if e.label_modifiers then child.modifiers = e.label_modifiers end
    end
    local mods = {}
    if e.disabled then
        table.insert(mods, { disabled = true })
        if e.hideOff then table.insert(mods, { opacity = 0 }) end
    end
    if e.tint then table.insert(mods, { tint = e.tint }) end
    if e.tone then table.insert(mods, { foregroundStyle = e.tone }) end
    if e.fill then table.insert(mods, { frame = { maxWidth = "infinity" } }) end
    if e.help then
        table.insert(mods, { help = e.help })
        table.insert(mods, { accessibilityLabel = e.help })
    end
    local v = { component = "Button", child = child, action = action }
    if e.style == "link" or e.style == "plain" then v.variant = "borderless" elseif e.style == "primary" then v.variant = "primary" end
    if #mods > 0 then v.modifiers = mods end
    return v
end

-- A Button that puts `copied` on the clipboard when pressed; the client does it, nothing reaches update.
function M.copy_button(body, copied, e)
    return M.button(body, { copy = { text = copied } }, e)
end

-- A table cell: its text and the style fields (tone, mono, chip, sort).
function M.cell(body, extra)
    local c = { text = body }
    if extra then for k, v in extra do c[k] = v end end
    return c
end

-- The Table node `t` given the height its rows need. The client wraps a plugin pane in a scroll view, which gives a Table no
-- height of its own, so the page names it: the header row, then every row and the title row of each section that has a
-- title (`lines`); a section with no title draws none. A table with no rows gets EMPTY_HEIGHT under its header, room for
-- its `empty` text. The frame clips what the client's own minimum height would draw below the last row.
local HEADER_HEIGHT, LINE_HEIGHT, EMPTY_HEIGHT = 34, 24, 96
function M.sized_table(t)
    local lines, rows = 0, 0
    for _, s in t.sections do
        lines += #s.rows + (if s.title and s.title ~= "" then 1 else 0)
        rows += #s.rows
    end
    local body = if rows == 0 then EMPTY_HEIGHT else LINE_HEIGHT * lines
    t.modifiers = { { frame = { height = HEADER_HEIGHT + body, alignment = "top" } }, { clipShape = { cornerRadius = 0 } } }
    return t
end

-- A table column; `tree` holds the fold control, `wide` shows only when the pane is wide, `sortable` sorts on a header click.
function M.column(key, label, width, tree, wide, sortable)
    return { key = key, label = label, width = width, tree = tree or nil, wide = wide or nil, sortable = sortable or nil }
end

return M
