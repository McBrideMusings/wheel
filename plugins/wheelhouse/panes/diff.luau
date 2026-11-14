-- The Diff pane: the project's diff by scope, one file's hunks at a time.
--
-- `wheelhouse.diff` (the backend's feed, see wheel-plugin.toml) holds the scopes and each scope's files with their hunks.
-- The pane is read-only; its own state (`scope`, `sel`) lives in `state`.
local kit = require("lib/ui")

local NONE = {}

-- The most hunk lines drawn for one file.
local DRAWN = 600
-- The page's padding at its left and right.
local SIDE = 16
-- The page's width at which every file is shown stacked beside the file list: a 640 pt pane less the page's sides.
-- Narrower, one file is open under a fixed file table.
local WIDE = 640 - 2 * SIDE
-- The most diff lines the stacked layout draws across the files that are not selected; later ones show their header and a
-- note. The selected file is always drawn, up to DRAWN lines.
local STACKED = 2000
-- The most files drawn in the stacked sections and in each file table; the rest are counted in a note. Lines are budgeted
-- above, so this bounds the view tree: 400 files with 60-character lines at both line budgets make 74,351 values, under the
-- 100,000 the runtime accepts.
local FILES = 200
-- The wide layout's height. The client wraps a plugin pane's surface in a scroll view, which gives the split's lists no height
-- of their own, so the page names one; wheel-pmf2.10 removes the wrapper and with it this height.
local WIDE_HEIGHT = 600
-- The fixed-width slot the totals sit in at the right of the top row, so a longer or shorter total moves nothing.
local TOTALS = 210
-- Rows the narrow layout's file table shows before it scrolls (its header row included in the height).
local TABLE_ROWS = 7

-- Node ids are positional (`root`, then the parent's id and the child's place), and a List's `scrollTo` names one, so the page
-- keeps its slots fixed: root holds [problem, top, layout, note]; layout holds [wide, narrow]; wide holds [split]; the split holds
-- [side, stream]; narrow holds [files, nav, lines].
local WIDE_STREAM = "root.2.0.0.1"
local NARROW_LINES = "root.2.1.2"

-- ───────────────────────────── the data ─────────────────────────────

local function scope_of(snap, st)
    local scopes = snap.scopes or NONE
    for _, c in scopes do
        if c.key == st.scope then return c end
    end
    return scopes[1]
end

local function file_of(scope, st)
    local files = scope.files or NONE
    for _, f in files do
        if f.path == st.sel then return f end
    end
    return files[1]
end

local function index_of(scope, file)
    if not file then return nil end
    for i, f in scope.files or NONE do
        if f.path == file.path then return i end
    end
    return nil
end

local function totals_of(scope)
    local add, del = 0, 0
    for _, f in scope.files or NONE do
        add += f.add or 0
        del += f.del or 0
    end
    return add, del
end

local function status_tone(c)
    if c == "A" or c == "?" then return "ok" end
    if c == "D" then return "bad" end
    return "warn"
end

-- GitHub's five squares for a file: green for its share of added lines, red for removed, grey when it has none.
local function squares(f)
    local add, del = f.add or 0, f.del or 0
    local total = add + del
    local green = if total == 0 then 0 else (add * 5 + total // 2) // total
    local red = if total == 0 then 0 else 5 - green
    local runs = {}
    for _, r in { { green, "ok" }, { red, "bad" }, { 5 - green - red, "todo" } } do
        if r[1] > 0 then table.insert(runs, { text = string.rep("■", r[1]), tone = r[2] }) end
    end
    return runs
end

-- Gerrit's size bar: up to ten blocks, the scope's largest file filling it.
local function size_bar(f, largest)
    local total = (f.add or 0) + (f.del or 0)
    if total == 0 or largest == 0 then return "" end
    return string.rep("▇", math.clamp(math.ceil(total * 10 / largest), 1, 10))
end

-- A file's name first, then the folder it is in, so the name survives a narrow column.
local function short_name(f)
    local dir, name = string.match(f.path, "^(.*)/([^/]*)$")
    if dir then return name .. "  " .. dir .. "/" end
    return f.path
end

-- The files a layout draws: the first FILES, and the selected one after them when it lies beyond.
local function drawn_files(scope, sel)
    local files = {}
    for i, f in scope.files do
        if i <= FILES or (sel ~= nil and sel.path == f.path) then
            table.insert(files, f)
        end
    end
    return files
end

-- The note for the files left out of the layouts; a space when none is, so its slot stays.
local function more_note(scope)
    local more = #scope.files - FILES
    local text = if more > 0 then more .. " more files; `wheelhouse diff` lists them all." else " "
    local note = kit.label(text, { tone = "muted", size = 11 })
    return kit.modify(note, { lineLimit = 1 })
end

local function clean(scope)
    if scope.key == "uncommitted" then return "No uncommitted changes." end
    return "No changes in this scope."
end

-- ───────────────────────────── the lines ─────────────────────────────

-- A label with a padding modifier (a label's own style has none).
local function padded(text, style, pad)
    return kit.modify(kit.label(text, style), { padding = pad })
end

local function number(v)
    if v == nil then return "    " end
    return string.format("%4d", v)
end

-- One drawn line of a hunk: both line numbers, the kind and the text, coloured by kind.
local function diff_line(l)
    local tone = if l.kind == "+" then "ok" elseif l.kind == "-" then "bad" else "fg"
    return kit.label(number(l.old) .. " " .. number(l.new) .. " " .. l.kind .. " " .. (l.text or ""), { mono = true, size = 11, tone = tone })
end

-- A file's hunks as lines, at most `budget` diff lines; a note replaces what is left out. Returns the lines and the count drawn.
local function hunk_lines(f, budget)
    local out = {}
    if f.binary then
        table.insert(out, padded("Binary file.", { tone = "muted" }, { 6, 0, 6, 0 }))
        return out, 0
    end
    if f.truncated then
        table.insert(out, padded("Too large to show here; the counts still hold.", { tone = "muted" }, { 6, 0, 6, 0 }))
        return out, 0
    end
    local cap = math.min(budget, DRAWN)
    local drawn = 0
    for _, h in f.hunks or NONE do
        table.insert(out, padded("@@ " .. h.header, { mono = true, size = 11, tone = "muted" }, { 6, 0, 2, 0 }))
        for _, l in h.lines or NONE do
            if drawn == cap then
                table.insert(out, padded(drawn .. " lines shown; the rest is in `wheelhouse diff --json`.", { tone = "muted" }, { 6, 0, 6, 0 }))
                return out, drawn
            end
            drawn += 1
            table.insert(out, diff_line(l))
        end
    end
    return out, drawn
end

-- ───────────────────────────── the top row ─────────────────────────────

local function totals_box(scope)
    local n = #(scope.files or NONE)
    local add, del = totals_of(scope)
    local count = kit.label(n .. " " .. (if n == 1 then "file" else "files") .. "  +" .. add .. " −" .. del,
        { size = 12, weight = "medium", tabular = true, align = "end" })
    local since
    if scope.commits ~= nil and scope.fork ~= nil then
        since = scope.commits .. " " .. (if scope.commits == 1 then "commit" else "commits") .. " since " .. scope.fork
    else
        since = "working tree vs " .. (scope.base or "")
    end
    local base = kit.label(since, { tone = "muted", size = 11, tabular = true, align = "end" })
    kit.modify(count, { lineLimit = 1 })
    kit.modify(base, { lineLimit = 1 })
    local box = kit.box("v", { count, base }, { spacing = 1, align = "end" })
    return kit.modify(box, { frame = { width = TOTALS, alignment = "trailing" } })
end

-- The scope switcher at the left and the totals in a fixed slot at the right. A ViewThatFits of the same row with chips and
-- with a menu: the totals' slot is fixed, so which one fits depends on the pane's width alone.
local function top_row(snap, st, scope)
    local function picker(style, mods)
        local options = {}
        for _, c in snap.scopes do table.insert(options, { label = c.label, value = c.key }) end
        return ui.ChoicePicker{
            variant = "mutuallyExclusive", displayStyle = style, options = options, value = { scope.key },
            on_change = { type = "set_scope" }, modifiers = mods,
        }
    end
    local function row(style, mods)
        local r = kit.box("h", { picker(style, mods), totals_box(scope) }, { spacing = 12, align = "center" })
        r.justify = "spaceBetween"
        return r
    end
    return ui.ViewThatFits{
        ["in"] = "horizontal",
        row("chips", { { fixedSize = { horizontal = true } } }),
        row("menu", { { fixedSize = true } }),
    }
end

-- ───────────────────────────── the file tables ─────────────────────────────

-- The file table's rows, each file's name first: `wide` is the stacked layout's list of names; otherwise the narrow layout's
-- table also has a size bar and the counts.
local function file_rows(scope, sel, wide)
    local largest = 0
    for _, f in scope.files do largest = math.max(largest, (f.add or 0) + (f.del or 0)) end
    local rows = {}
    for _, f in drawn_files(scope, sel) do
        local on = sel ~= nil and sel.path == f.path
        local name = if f.old then f.old .. " → " .. f.path else short_name(f)
        local cells = {
            kit.cell(f.status or "?", { key = "status", chip = true, tone = status_tone(f.status) }),
            kit.cell(name, { key = "path", mono = true, tone = if on then "accent" else "fg" }),
        }
        if not wide then
            table.insert(cells, kit.cell(size_bar(f, largest), { key = "size", mono = true, tone = "muted" }))
            table.insert(cells, kit.cell("+" .. (f.add or 0), { key = "add", mono = true, tone = "ok" }))
            table.insert(cells, kit.cell("−" .. (f.del or 0), { key = "del", mono = true, tone = "bad" }))
        end
        table.insert(rows, { id = f.path, path = f.path, depth = 0, kids = false, collapsed = false, dim = false, sel = on, cells = cells, menu = {} })
    end
    return rows
end

local function file_table(columns, rows, empty)
    return {
        component = "Table", columns = columns,
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "select_file" }, foldAction = { type = "select_file" },
        sort = { key = "path", asc = true },
        empty = empty,
    }
end

-- ───────────────────────────── the layouts ─────────────────────────────

-- At least 640 pt across (GitHub's layout): the file list at the left with its own scroll, and every file stacked at the right
-- under a header that stays at the top while its file is read.
local function wide_layout(scope, sel)
    local columns = { kit.column("status", "", 28, false, false), kit.column("path", "File", nil, false, false) }
    local files = file_table(columns, file_rows(scope, sel, true), "No files.")
    local side = kit.box("v", { files }, { spacing = 0 })
    kit.modify(side, { frame = { minWidth = 140, idealWidth = 170, maxWidth = 200, maxHeight = "infinity" } })

    local sections = {}
    local budget = STACKED
    local drawn_set = drawn_files(scope, sel)
    local at = nil
    for i, f in drawn_set do
        local path = kit.label(if f.old then f.old .. " → " .. f.path else f.path, { mono = true, weight = "semibold", size = 12 })
        kit.modify(path, { lineLimit = 1 })
        kit.modify(path, { truncationMode = "head" })
        local count = kit.label("+" .. (f.add or 0) .. " −" .. (f.del or 0), { mono = true, size = 11, tone = "muted", tabular = true })
        kit.modify(count, { fixedSize = true })
        local bar = kit.label("", { size = 9, runs = squares(f) })
        kit.modify(bar, { fixedSize = true })
        local header = kit.box("h", { path, count, bar }, {
            spacing = 8, align = "center", widths = { "flex", false, false }, padding = { 6, 8 }, fill = "card", radius = 0,
        })
        local selected = sel ~= nil and sel.path == f.path
        local lines, drawn
        if budget == 0 and not selected then
            lines = { padded("Not drawn: this view draws " .. STACKED .. " lines across the files; select this one to draw it.", { tone = "muted" }, { 6, 0, 6, 0 }) }
            drawn = 0
        else
            lines, drawn = hunk_lines(f, if selected then DRAWN else budget)
        end
        if not selected then budget -= drawn end
        for _, l in lines do kit.modify(l, { padding = { horizontal = 8 } }) end
        -- The space under a file's last line, before the next file's header.
        if #lines > 0 then kit.modify(lines[#lines], { padding = { bottom = 14 } }) end
        lines.header = header
        sections[i] = ui.Section(lines)
        if selected then at = i end
    end
    if #sections == 0 then
        table.insert(sections, padded(clean(scope), { tone = "muted" }, { 24, 16, 0, 16 }))
    end
    local stream = sections
    stream.spacing = 0
    stream.pinnedViews = { "sectionHeaders" }
    stream.modifiers = { { frame = { minWidth = 280, maxWidth = "infinity", maxHeight = "infinity" } } }
    if at then stream.scrollTo = WIDE_STREAM .. "." .. (at - 1) end
    local split = ui.HSplitView{ side, ui.List(stream), modifiers = { { frame = { maxHeight = "infinity" } } } }
    local wide = kit.box("v", { split }, { spacing = 0 })
    return kit.modify(wide, { frame = { minWidth = WIDE, idealWidth = WIDE, maxWidth = "infinity", height = WIDE_HEIGHT } })
end

-- A previous or next button that keeps its place: disabled at the end of the list.
local function step(icon, help, target)
    return kit.button("", { type = "select_file", id = if target then target.path else "" },
        { icon = icon, style = "plain", disabled = target == nil, help = help })
end

-- Under 640 pt across (Gerrit's layout): a file table with a size bar fixed at the top, scrolling inside itself, and the one
-- open file under it with previous, next and its place in the list.
local function narrow_layout(scope, sel)
    local columns = {
        kit.column("status", "", 28, false, false), kit.column("path", "File", nil, false, false),
        kit.column("size", "Size", 78, false, false), kit.column("add", "+", 44, false, false), kit.column("del", "−", 44, false, false),
    }
    local files = file_table(columns, file_rows(scope, sel, false), clean(scope))
    -- Seven rows of 24 pt under a 28 pt header.
    files.modifiers = { { frame = { height = 28 + 24 * TABLE_ROWS } } }
    local at = index_of(scope, sel)
    local total = #(scope.files or NONE)
    local prev = step("chevron.left", "Previous file", at and scope.files[at - 1])
    local nxt = step("chevron.right", "Next file", at and scope.files[at + 1])
    local place = kit.label((at or 0) .. " / " .. total, { size = 11, tone = "muted", tabular = true })
    kit.modify(place, { frame = { minWidth = 48 } })
    local name = kit.label(sel and sel.path or "", { mono = true, weight = "semibold", size = 12 })
    kit.modify(name, { lineLimit = 1 })
    kit.modify(name, { truncationMode = "head" })
    local nav = kit.box("h", { prev, place, nxt, name }, { spacing = 6, align = "center", widths = { false, false, false, "flex" }, padding = { 4, 0, 4, 0 } })
    local lines = {}
    if sel then lines = hunk_lines(sel, DRAWN) end
    lines.spacing = 0
    lines.modifiers = { { frame = { maxHeight = "infinity" } } }
    -- A file's first line, so a newly opened file starts at its top (a binary file's first line is its note).
    if #lines > 0 then lines.scrollTo = NARROW_LINES .. ".0" end
    local list = ui.List(lines)
    local narrow = kit.box("v", { files, nav, list }, { spacing = 4 })
    return kit.modify(narrow, { frame = { maxHeight = "infinity" } })
end

-- ───────────────────────────── the page ─────────────────────────────

local function problem_box(text)
    local l = kit.label(text, { tone = "bad" })
    return kit.box("v", { l }, { border = "bad", radius = 8, padding = { 8, 12 }, margin = { 10, 0, 0, 0 } })
end

local function page(st, snap)
    local problem = if snap.error then problem_box(snap.error) else ui.Column{}
    local top, layout = ui.Column{}, ui.Column{}
    local scope = scope_of(snap, st)
    if scope then
        scope.files = scope.files or {}
        top = top_row(snap, st, scope)
        local sel = file_of(scope, st)
        layout = ui.ViewThatFits{ ["in"] = "horizontal", wide_layout(scope, sel), narrow_layout(scope, sel) }
    end
    -- Only the file list and the diff text scroll: the file table without a frame makes the page fill the pane.
    local note = if scope then more_note(scope) else ui.Column{}
    return kit.box("v", { problem, top, layout, note }, { align = "start", spacing = 10, padding = { 12, SIDE, 10, SIDE } })
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or { scope = "", sel = nil }
end

local function update(state, msg)
    if msg.action then return end
    local t = msg.type
    if t == "set_scope" then
        local v = msg.value
        if type(v) == "table" then v = v.key or v[1] end
        if type(v) == "string" and v ~= "" then
            state.scope = v
            state.sel = nil
        end
    elseif t == "select_file" then
        if type(msg.id) == "string" and msg.id ~= "" then state.sel = msg.id end
    end
end

local function view(state, data)
    local snap = data.feed("wheelhouse.diff", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the diff…" } }
    end
    return page(state, snap)
end

return { init = init, update = update, view = view }
