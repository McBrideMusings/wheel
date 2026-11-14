-- The narrowed, sorted list every list pane shows (Beads, Issues, PRs, Repo): the search, the filters (each a popover of
-- ticks, with "Is not"), the status boxes that set one filter, Clear, the header sort and the row selection. A port of
-- core/list.rs and core/buttons.rs `actions`.
--
-- A pane keeps one list in its state (`list.new`), names its filters and sortable columns in a spec, and says how one
-- of its rows answers a filter and a search:
--
--   local SPEC = {
--       filters = { { key = "status", name = "status", hidden = "done" }, { key = "type", name = "type" } },
--       sorts = { "id", "title" },
--       has = function(item, key, value) ... end,   -- whether the row has the filter's value
--       haystack = function(item) ... end,          -- the text a search looks in
--   }
--
-- The pane gives `list.handle` every message first (it returns true when the message was a list one), then lists its rows
-- with `list.matcher` or `list.sorted`, and draws the controls with `list.find`, `list.bar`, `list.boxes` and
-- `list.actions`. The controls send these messages (`type`, then its fields; `value` is what the control wrote):
--
--   list.query {value}          the search text
--   list.filter {key, value}    one filter set to these values, comma separated; "" clears it (a status box)
--   list.toggle {key, item, value}  one value of a filter ticked or unticked
--   list.not {key, value}       a filter made to exclude its values, or to match them again
--   list.sort {key, asc}        a header click
--   list.select {ids}           the table's selection
--   list.clear                  Clear
--   list.run {script}           a list button of `list.actions`; the pane runs it over the ids it lists
local kit = require("lib/ui")

local M = {}

local NONE = {}

-- One pane's list: what it searches for (q), the ticked values of each filter (f), the filters that exclude rather than
-- match (neg), the sort and the selected rows (selected, a set of ids).
function M.new(sort, asc)
    return { q = "", f = {}, neg = {}, sort = sort, asc = asc, selected = {} }
end

function M.values(l, key)
    return l.f[key] or NONE
end

-- Whether a search, any filter or an "Is not" narrows the list, which is when Clear does something.
function M.active(l)
    if string.find(l.q, "%S") then return true end
    for _, v in l.f do
        if #v > 0 then return true end
    end
    return next(l.neg) ~= nil
end

local function known(spec, key)
    for _, f in spec.filters do
        if f.key == key then return true end
    end
    return false
end

local function sortable(spec, key)
    return table.find(spec.sorts, key) ~= nil
end

-- A filter left with no ticks matches every row, so its "Is not" goes with the last tick: the next tick matches again.
local function settle(l, key)
    if #M.values(l, key) == 0 then l.neg[key] = nil end
end

-- Applies a list message to `l`; false when `msg` is not one. A change to what is listed clears the selection.
function M.handle(l, spec, msg)
    local t = msg.type
    if t == "list.query" then
        l.q = if type(msg.value) == "string" then msg.value else ""
        l.selected = {}
    elseif t == "list.filter" then
        if known(spec, msg.key) then
            local v = {}
            for part in string.gmatch(msg.value or "", "[^,]+") do table.insert(v, part) end
            l.f[msg.key] = v
            settle(l, msg.key)
            l.selected = {}
        end
    elseif t == "list.toggle" then
        if known(spec, msg.key) then
            local v = {}
            for _, x in M.values(l, msg.key) do
                if x ~= msg.item then table.insert(v, x) end
            end
            if msg.value then table.insert(v, msg.item) end
            l.f[msg.key] = v
            settle(l, msg.key)
            l.selected = {}
        end
    elseif t == "list.not" then
        if known(spec, msg.key) then
            l.neg[msg.key] = if msg.value then true else nil
            l.selected = {}
        end
    elseif t == "list.sort" then
        if sortable(spec, msg.key) then
            l.sort = msg.key
            l.asc = msg.asc == true
        end
    elseif t == "list.clear" then
        l.q = ""
        l.f = {}
        l.neg = {}
        l.selected = {}
    elseif t == "list.select" then
        local s = {}
        for _, id in msg.ids or NONE do s[id] = true end
        l.selected = s
    else
        return false
    end
    return true
end

-- A function that says whether a row stays in the list: every filter holds it (a filter holds a row that has one of its
-- values, or, excluding, none of them), a row with a filter's hidden value shows only while that filter is set, and every
-- word of the search is in its text. The search text of a row is built only when there is a search.
function M.matcher(l, spec)
    local words = {}
    for w in string.gmatch(string.lower(l.q), "%S+") do table.insert(words, w) end
    local has, haystack = spec.has, spec.haystack
    local checks = {}
    for _, f in spec.filters do
        local v = M.values(l, f.key)
        if #v == 0 then
            if f.hidden then table.insert(checks, { key = f.key, hidden = f.hidden }) end
        else
            table.insert(checks, { key = f.key, values = v, neg = l.neg[f.key] == true })
        end
    end
    return function(item)
        for _, c in checks do
            if c.hidden then
                if has(item, c.key, c.hidden) then return false end
            else
                local any = false
                for _, x in c.values do
                    if has(item, c.key, x) then
                        any = true
                        break
                    end
                end
                if any == c.neg then return false end
            end
        end
        if #words > 0 then
            local hay = string.lower(haystack(item))
            for _, w in words do
                if not string.find(hay, w, 1, true) then return false end
            end
        end
        return true
    end
end

-- `items` (an array) that `matcher` keeps, in the sort's order. `cmp(key, a, b)` compares two rows by a column key and
-- `tie(a, b)` breaks what it leaves equal (both return a negative number, 0 or a positive one); rows still equal keep their
-- order in `items`, descending sorts included, so the order never depends on the sort's stability.
function M.sorted(l, spec, items, cmp, tie)
    local keeps = M.matcher(l, spec)
    local v = {}
    for i, item in items do
        if keeps(item) then table.insert(v, { item, i }) end
    end
    local key, asc = l.sort, l.asc
    table.sort(v, function(a, b)
        local o = cmp(key, a[1], b[1])
        if o == 0 and tie then o = tie(a[1], b[1]) end
        if o == 0 then return a[2] < b[2] end
        if asc then return o < 0 end
        return o > 0
    end)
    for i, e in v do v[i] = e[1] end
    return v
end

-- The ids the list actions (Copy IDs, the project's list buttons) take: the selected ones among `listed`, in the order
-- shown, or every listed id when none of the selection is listed.
function M.taken(listed, l)
    local picked = {}
    for _, id in listed do
        if l.selected[id] then table.insert(picked, id) end
    end
    if #picked == 0 then return listed end
    return picked
end

-- A filter's options counted over `items`: each value `of(item)` gives (an array), with how many rows have it, the
-- commonest first. Each option is { value, text } and the text reads "value  count".
function M.counted(items, of)
    local c, order = {}, {}
    for _, item in items do
        for _, v in of(item) do
            if not c[v] then
                c[v] = 0
                table.insert(order, v)
            end
            c[v] += 1
        end
    end
    table.sort(order, function(a, b)
        if c[a] ~= c[b] then return c[a] > c[b] end
        return a < b
    end)
    local out = {}
    for _, v in order do table.insert(out, { v, v .. "  " .. c[v] }) end
    return out
end

-- A filter's options in a fixed order, each with how many `items` have it: `values` are { value, text }.
function M.fixed(spec, key, items, values)
    local out = {}
    for _, p in values do
        local n = 0
        for _, item in items do
            if spec.has(item, key, p[1]) then n += 1 end
        end
        table.insert(out, { p[1], p[2] .. "  " .. n })
    end
    return out
end

-- The status boxes: each a number over a caption that, tapped, sets one filter to its value alone, and tapped again
-- clears it. `cards` are { caption, filter key, value, count, whether a non-zero count is good news }.
function M.boxes(l, cards)
    local kids, widths = {}, {}
    for _, c in cards do
        local label, key, value, count, hot = c[1], c[2], c[3], c[4], c[5]
        local v = M.values(l, key)
        local on = #v == 1 and v[1] == value
        local tone = if hot and count > 0 then "ok" elseif count == 0 then "muted" else "fg"
        local num = kit.label(tostring(count), { size = 20, weight = "semibold", tabular = true, tone = tone })
        local cap = kit.label(label, { tone = "muted", size = 11 })
        kit.modify(cap, { lineLimit = 1 })
        local inner = kit.box("v", { num, cap }, { spacing = 2, align = "start" })
        local mods = {
            { frame = { maxWidth = "infinity" } }, { padding = { 7, 8 } },
            { overlay = { stroke = if on then "tint" else "separator", cornerRadius = 8 } },
        }
        if on then table.insert(mods, { background = "controlBackground" }) end
        table.insert(kids, {
            component = "Button", child = inner, variant = "borderless", modifiers = mods,
            action = { type = "list.filter", key = key, value = if on then "" else value },
        })
        table.insert(widths, "flex")
    end
    return kit.box("h", kids, { spacing = 8, widths = widths })
end

-- The search row: the search field, then `beside` (a views choice, a tab, a refresh button).
function M.find(l, beside)
    local field = ui.TextField{
        label = "Search", value = l.q, variant = "shortText", on_change = { type = "list.query" },
        modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
    }
    -- What sits beside the field is offered its width first, so the field shrinks to make room for it.
    local kids, widths = { field }, { "flex" }
    for _, b in beside do
        kit.modify(b, { layoutPriority = 1 })
        table.insert(kids, b)
        table.insert(widths, false)
    end
    local row = kit.box("h", kids, { spacing = 8, align = "center", widths = widths, padding = { 8, 0, 0, 0 } })
    kit.modify(row, { frame = { maxWidth = "infinity" } })
    return row
end

local function text_of(opts, v)
    for _, o in opts do
        if o[1] == v then return (string.match(o[2], "^(.-)  ") or o[2]) end
    end
    return v
end

local function capitalise(s)
    return string.upper(string.sub(s, 1, 1)) .. string.sub(s, 2)
end

-- The filter row: one popover of ticks per filter (`options[i]` gives the i-th filter's { value, text } in the spec's
-- order), then Clear and `actions` (the list actions chip). Each popover's title says what it holds ("Any label",
-- "Status not: done"); Clear is hidden, not removed, when nothing narrows the list, so nothing after it moves.
function M.bar(spec, l, options, actions)
    local menus = {}
    for i, f in spec.filters do
        local opts = options[i] or NONE
        local key, name = f.key, f.name
        local held = M.values(l, key)
        local neg = l.neg[key] == true
        local title
        if #held == 0 then
            title = "Any " .. name
        else
            local texts = {}
            for _, v in held do table.insert(texts, text_of(opts, v)) end
            title = capitalise(name) .. (if neg then " not" else "") .. ": " .. table.concat(texts, ", ")
        end
        local kids = {
            ui.CheckBox{ label = "Is not", value = neg, on_change = { type = "list.not", key = key } },
            ui.Divider{},
        }
        local ticked = {}
        for _, v in held do ticked[v] = true end
        for _, o in opts do
            table.insert(kids, ui.CheckBox{
                label = o[2], value = ticked[o[1]] == true, on_change = { type = "list.toggle", key = key, item = o[1] },
            })
        end
        -- A Mac menu closes after each tick, so the ticks sit in a popover, which stays open until it is dismissed.
        table.insert(menus, ui.Popover{
            kit.label(title .. " ⌄", { size = 12 }),
            kit.box("v", kids, { spacing = 6, align = "start" }),
        })
    end
    -- Two groups, by proximity: what narrows the list, and Clear with the actions. The gap between them is whatever width
    -- is left, never under 16. Sorting is the column headers' job.
    local narrow = kit.box("h", menus, { spacing = 6, align = "center" })
    local clear = kit.button("Clear", { type = "list.clear" }, { style = "link", disabled = not M.active(l), hideOff = true })
    local shape = kit.box("h", { clear, actions }, { spacing = 10, align = "center" })
    local bar = kit.box("h", { narrow, shape }, { spacing = 16, align = "center", widths = { "flex", false }, padding = { 4, 0, 0, 0 } })
    kit.modify(bar, { frame = { maxWidth = "infinity" } })
    return bar
end

-- The list actions as one chip drawn like the filters: its title counts the rows the actions take, `took` (their ids) of
-- the `listed` rows ("12 listed" or "3 selected of 12"), and its popover holds Copy IDs (the ids space separated, `copied`
-- naming them in its help) then the project's list buttons `scripts` ({ name, label }), each with that count, over `rows`.
-- Each keeps its slot: with no rows the buttons are disabled, not removed.
function M.actions(scripts, took, listed, copied, rows)
    local n = #took
    local title, what
    if n < listed then
        title, what = n .. " selected of " .. listed .. " · Actions ⌄", "selected"
    else
        title, what = n .. " listed · Actions ⌄", "listed"
    end
    local menu = {
        kit.copy_button("Copy IDs", table.concat(took, " "), {
            style = "link", disabled = n == 0, help = "Copy the " .. n .. " " .. what .. " " .. copied .. ", space separated",
        }),
    }
    for _, s in scripts do
        local label = s.label or s.name
        table.insert(menu, kit.button(label .. "  " .. n, { type = "list.run", script = s.name }, {
            style = "link", disabled = n == 0, help = label .. " over the " .. n .. " " .. what .. " " .. rows,
        }))
    end
    return ui.Popover{
        kit.label(title, { size = 12, tabular = true }),
        kit.box("v", menu, { spacing = 6, align = "start" }),
    }
end

return M
