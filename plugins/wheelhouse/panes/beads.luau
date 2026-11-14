-- The Beads pane: a project's beads as a list, an epics tree, a depends-on tree and an unlocks tree, with search,
-- filters, status boxes, a sortable table, a detail card and the project's buttons, and the Focus and Graph views drawn
-- on a Canvas.
--
-- `wheelhouse.beads` (the backend's feed, see wheel-plugin.toml) holds the beads; `wheel.panes` says which panes are
-- open; `wheelhouse.beads_graph` holds the scene Focus and Graph draw. The pane's own state (view, search and filters,
-- sort, selection, folds, the open bead, the focused bead and its history, the hops) lives in `state`.
local kit = require("lib/ui")
local list = require("lib/list")
local tm = require("lib/time")
local sorting = require("lib/sort")

local NONE = {}

-- ───────────────────────────── the page's vocabulary ─────────────────────────────

local KINDS = { "in progress", "ready", "blocked", "open", "deferred", "done" }
local RANK = {}
for i, k in KINDS do RANK[k] = i end

local VIEWS = {
    { "list", "List" }, { "epics", "Epics" }, { "deps", "Depends on" }, { "unlocks", "Unlocks" },
    { "focus", "Focus" }, { "graph", "Graph" },
}

-- The height of the Canvas: the client wraps a plugin pane in a scroll view, which gives a view that fills its space no
-- height, so the page names it. Graph and every state with no scene to measure (loading, empty) use CANVAS_HEIGHT; Focus
-- uses its scene's height, never under CANVAS_MIN, so a tall scene is drawn whole instead of spilling out of its frame.
local CANVAS_HEIGHT = 520
local CANVAS_MIN = 380
local CANVAS_MAX = 1200

-- The last feed value drawn per view ("focus", "graph"). The feed's params are the layout's inputs, so every click, hop or
-- width change reads nil until the backend answers; the view draws this meanwhile, so the Canvas stays mounted (it keeps
-- its camera) and nothing moves. Kept here and not in `state`, which a check reads as JSON.
local LAST = {}

-- The status legend of the drawn views: glyph, word and tone, in the order the backend numbers its statuses.
local LEGEND = {
    { "●", "ready", "ok" }, { "◐", "in progress", "accent" }, { "⊘", "blocked", "warn" }, { "◆", "needs you", "purple" },
    { "◌", "deferred", "faint" }, { "○", "open", "muted" }, { "✓", "done", "muted" },
}

local COLS = {
    { key = "id", label = "ID", width = 56, tree = false, wide = true },
    { key = "title", label = "Title", width = nil, tree = true, wide = false },
    { key = "type", label = "Type", width = 56, tree = false, wide = true },
    { key = "unlocks", label = "Unlocks", width = 28, tree = false, wide = true },
    { key = "status", label = "Status", width = 84, tree = false, wide = false },
    { key = "pri", label = "Priority", width = 24, tree = false, wide = false },
    { key = "updated", label = "Updated", width = 30, tree = false, wide = true },
}

local function kind_of(b)
    local s = b.status
    if s == "closed" then return "done" end
    if s == "in_progress" then return "in progress" end
    if s == "deferred" then return "deferred" end
    if b.ready then return "ready" end
    if #(b.blocked_by or NONE) > 0 then return "blocked" end
    return "open"
end

local function tone_of(k)
    if k == "done" or k == "ready" then return "ok" end
    if k == "in progress" then return "warn" end
    if k == "blocked" then return "bad" end
    return "muted"
end

local function is_open(b)
    return b.status ~= "closed"
end

-- A bead that groups others: its button is the epic's.
local function groups(b)
    return b.epic == true or b.kind == "epic" or b.kind == "milestone"
end

-- Epics and milestones are not listed as work.
local function is_leaf(b)
    return b.kind ~= "epic" and b.kind ~= "milestone"
end

-- ───────────────────────────── the list spec ─────────────────────────────

local SPEC = {
    filters = {
        { key = "status", name = "status", hidden = "done" },
        { key = "type", name = "type" },
        { key = "label", name = "label" },
        { key = "pri", name = "priority" },
    },
    sorts = { "id", "title", "type", "unlocks", "status", "pri", "updated" },
    has = function(b, key, value)
        if key == "status" then return b._k == value end
        if key == "type" then return b.kind == value end
        if key == "label" then return table.find(b.labels or NONE, value) ~= nil end
        if key == "pri" then return b.priority ~= nil and tostring(b.priority) == value end
        return false
    end,
    haystack = function(b)
        local parts = { b.id, b.title, b.kind }
        for _, l in b.labels or NONE do table.insert(parts, l) end
        table.insert(parts, b.description or "")
        table.insert(parts, b.owner or "")
        return table.concat(parts, " ")
    end,
}

-- ───────────────────────────── the index ─────────────────────────────

-- The beads of a snapshot by id, what each unlocks and the children of each, built once per render in one pass over the
-- beads; each bead gets the fields the comparisons read (_k its status word, _r its rank, _idk its natural id key, _lt its
-- lowercase title, _ep its update time).
local function index(snap, panes, now)
    local ix = { snap = snap, by_id = {}, unlocks = {}, kids = {}, now = now, acts = {}, cache = {} }
    local prefix = (snap.name or "") .. "-"
    for i, b in snap.beads do
        b._i = i
        b._k = kind_of(b)
        b._r = RANK[b._k] or 4
        b._idk = sorting.natural_key(b.id)
        b._lt = string.lower(b.title or "")
        b._ep = tm.epoch(b.updated)
        ix.by_id[b.id] = b
    end
    for _, b in snap.beads do
        for _, id in b.blocked_by or NONE do
            local u = ix.unlocks[id]
            if not u then
                u = {}
                ix.unlocks[id] = u
            end
            table.insert(u, b)
        end
        if b.parent then
            local k = ix.kids[b.parent]
            if not k then
                k = {}
                ix.kids[b.parent] = k
            end
            table.insert(k, b)
        end
    end
    function ix.blockers(b)
        local out = {}
        for _, id in b.blocked_by or NONE do
            local x = ix.by_id[id]
            if x then table.insert(out, x) end
        end
        return out
    end
    function ix.unlocked(b)
        return ix.unlocks[b.id] or NONE
    end
    function ix.children(b)
        return ix.kids[b.id] or NONE
    end
    function ix.short(id)
        if string.sub(id, 1, #prefix) == prefix then return string.sub(id, #prefix + 1) end
        return id
    end
    -- How many of the beads `b` unlocks are open.
    function ix.unlock_count(b)
        local n = b._unl
        if n == nil then
            n = 0
            for _, u in ix.unlocked(b) do
                if is_open(u) then n += 1 end
            end
            b._unl = n
        end
        return n
    end
    ix.panes = panes
    return ix
end

-- ───────────────────────────── a bead's button ─────────────────────────────

local function verb_run(v)
    if v and type(v.run) == "string" and string.find(v.run, "%S") then return v.run end
    return nil
end

local function verb_label(v)
    if v and type(v.label) == "string" and string.find(v.label, "%S") then return v.label end
    return nil
end

-- The button of a bead whose kind is an epic (`epic`) or not, in the state its record and the panes open now put it in;
-- nil when the project wrote no group for its kind.
local function decide(buttons, status, epic, record, live)
    if status == "closed" then
        return { state = "closed", label = buttons.closed or "Closed", disabled = true, help = "This bead is closed" }
    end
    local g = if epic then buttons.epic else buttons.task
    if not g then return nil end
    local kind = if epic then "epic" else "task"
    local function act(state, v, fallback, name, needs_run)
        local missing = needs_run and verb_run(v) == nil
        return {
            state = state, label = verb_label(v) or fallback, disabled = missing,
            help = if missing then "[beads." .. kind .. "] has no " .. name .. ".run" else "",
        }
    end
    if record then
        if live[record.pane] then return act("live", g.focus, "Focus", "focus", false) end
        return act("saved", g.resume, "Resume", "resume", true)
    end
    return act("fresh", g.start, "Start", "start", true)
end

-- The act of `b`, or nil: from the records, the panes open now and the presses in flight. When the panes cannot be
-- listed yet, a bead with a record shows its button disabled and says so.
local function act_of(ix, b)
    local hit = ix.acts[b.id]
    if hit ~= nil then return hit or nil end
    local snap = ix.snap
    local record = (snap.records or NONE)[b.id]
    local act = decide(snap.buttons or NONE, b.status, groups(b), record, ix.live or NONE)
    if act then
        if ix.live == nil and record and act.state ~= "closed" then
            act.disabled = true
            act.help = "The open panes are not known yet"
        end
        if ix.pressing[b.id] then
            act.disabled = true
            act.help = "Starting; wait for the pane"
        end
    end
    ix.acts[b.id] = act or false
    return act
end

-- ───────────────────────────── the rows a view shows ─────────────────────────────

local function compare(ix, key, a, b)
    if key == "id" then return sorting.cmp(a._idk, b._idk) end
    if key == "title" then return sorting.cmp(a._lt, b._lt) end
    if key == "type" then return sorting.cmp(a.kind or "", b.kind or "") end
    if key == "unlocks" then return sorting.cmp(ix.unlock_count(a), ix.unlock_count(b)) end
    if key == "status" then return sorting.cmp(a._r, b._r) end
    if key == "pri" then return sorting.cmp(a.priority or 9, b.priority or 9) end
    if key == "updated" then return sorting.cmp(a._ep, b._ep) end
    return 0
end

-- The most rows a view works out (a tree stops there, and says so), and the most the table is sent: a view tree over
-- 100,000 JSON values is refused by the pane runtime and each row is about 45, so the table sends 1,500 rows and ends
-- with one row saying how many more are not shown.
local MAX_ROWS = 20000
local SHOWN = 1500
local MORE_ID = "\u{2026}more"

-- A bead and what a tree view hangs under it, kept ones only: a bead with no kept row below it and not matching drops out.
local function walk(ctx, b, depth)
    if depth >= 60 then
        if ctx.ok(b) then return { b = b, depth = depth, nodes = {} } end
        return nil
    end
    ctx.budget -= 1
    if ctx.budget < 0 then
        ctx.cut = true
        return nil
    end
    local onpath = ctx.onpath
    onpath[b.id] = true
    local nodes = {}
    for _, k in ctx.children(b) do
        if ctx.keep[k.id] and not onpath[k.id] then
            local n = walk(ctx, k, depth + 1)
            if n then
                n.i = #nodes + 1
                table.insert(nodes, n)
            end
        end
    end
    onpath[b.id] = nil
    if #nodes == 0 and not ctx.ok(b) then return nil end
    table.sort(nodes, function(x, y)
        if x.b._r ~= y.b._r then return x.b._r < y.b._r end
        return x.i < y.i
    end)
    return { b = b, depth = depth, nodes = nodes }
end

local function flat(n, parent, collapsed, out)
    if #out >= MAX_ROWS then
        out.cut = true
        return
    end
    local path = parent .. ">" .. n.b.id
    table.insert(out, { b = n.b, depth = n.depth, path = path, kids = #n.nodes > 0 })
    if not collapsed[path] then
        for _, c in n.nodes do flat(c, path, collapsed, out) end
    end
end

-- The rows a view shows, as the page's table builds them: a flat sorted list, or a tree flattened with a depth and cut at
-- the collapsed paths. The second value is true when a tree stopped at MAX_ROWS and has more below.
local function build(ix, st)
    local l = st.list
    local beads = ix.snap.beads
    if st.view == "list" then
        local leaves = {}
        for _, b in beads do
            if is_leaf(b) then table.insert(leaves, b) end
        end
        local sorted = list.sorted(l, SPEC, leaves, function(key, a, b) return compare(ix, key, a, b) end,
            function(a, b) return compare(ix, "status", a, b) end)
        local rows = {}
        for i, b in sorted do rows[i] = { b = b, depth = 0, path = b.id, kids = false } end
        return rows, false
    end
    local pool = {}
    local want_done = table.find(list.values(l, "status"), "done") ~= nil
    for _, b in beads do
        if st.view == "epics" or want_done or is_open(b) then table.insert(pool, b) end
    end
    local keep = {}
    for _, b in pool do keep[b.id] = true end
    local children
    if st.view == "epics" then children = ix.children
    elseif st.view == "deps" then children = ix.blockers
    else children = ix.unlocked end
    local roots = {}
    for _, b in pool do
        local root
        if st.view == "epics" then
            root = (b.parent == nil or ix.by_id[b.parent] == nil)
                and (ix.kids[b.id] ~= nil or b.kind == "epic" or b.kind == "milestone")
        elseif st.view == "deps" then
            root = #ix.blockers(b) > 0
            if root then
                for _, u in ix.unlocked(b) do
                    if keep[u.id] then
                        root = false
                        break
                    end
                end
            end
        else
            root = #ix.unlocked(b) > 0
            if root then
                for _, x in ix.blockers(b) do
                    if keep[x.id] then
                        root = false
                        break
                    end
                end
            end
        end
        if root then table.insert(roots, b) end
    end
    local matcher = list.matcher(l, SPEC)
    local okc = {}
    local function ok(b)
        local v = okc[b.id]
        if v == nil then
            v = matcher(b)
            okc[b.id] = v
        end
        return v
    end
    local ctx = { children = children, keep = keep, ok = ok, onpath = {}, budget = MAX_ROWS, cut = false }
    local tops = {}
    for _, r in roots do
        local n = walk(ctx, r, 0)
        if n then
            n.i = #tops + 1
            table.insert(tops, n)
        end
    end
    table.sort(tops, function(x, y)
        if x.b._r ~= y.b._r then return x.b._r < y.b._r end
        return x.i < y.i
    end)
    local out = {}
    for _, n in tops do flat(n, "", st.collapsed, out) end
    return out, ctx.cut or out.cut == true
end

-- ───────────────────────────── the table ─────────────────────────────

local function row_json(ix, st, r)
    local b = r.b
    local k = b._k
    local cells = {}
    for _, c in COLS do
        local key = c.key
        local cell
        if key == "id" then
            cell = kit.cell(ix.short(b.id), { mono = true, tone = "muted" })
        elseif key == "title" then
            cell = kit.cell(b.title)
        elseif key == "type" then
            cell = kit.cell(b.kind, { tone = "muted" })
        elseif key == "unlocks" then
            local n = ix.unlock_count(b)
            cell = kit.cell(if n > 0 then tostring(n) else "", { tone = "muted", sort = n })
        elseif key == "status" then
            cell = kit.cell(k, { chip = true, tone = tone_of(k), sort = b._r })
        elseif key == "pri" then
            local p = b.priority
            if p == nil then
                cell = kit.cell("", { mono = true, sort = 9 })
            elseif p <= 1 then
                cell = kit.cell("P" .. p, { mono = true, tone = "bad", sort = p })
            elseif p >= 3 then
                cell = kit.cell("P" .. p, { mono = true, tone = "muted", sort = p })
            else
                cell = kit.cell("P" .. p, { mono = true, sort = p })
            end
        else
            cell = kit.cell(tm.ago(b.updated, ix.now), { tone = "muted", sort = b._ep })
        end
        cell.key = key
        table.insert(cells, cell)
    end
    local row = { id = b.id, path = r.path, depth = r.depth, cells = cells, menu = {} }
    if r.kids then row.kids = true end
    if st.collapsed[r.path] then row.collapsed = true end
    if k == "done" then row.dim = true end
    if st.detail == b.id then row.sel = true end
    local act = act_of(ix, b)
    if act and not act.disabled then
        row.menu[1] = { label = act.label, action = { type = "press", bead = b.id } }
    end
    return row
end

-- 1234 as "1,234".
local function commas(n)
    local s = tostring(n)
    while true do
        local t, k = string.gsub(s, "^(-?%d+)(%d%d%d)", "%1,%2")
        s = t
        if k == 0 then return s end
    end
end

-- The row that ends a table cut at SHOWN rows: how many more there are, `more` when the count is a lower bound.
local function more_row(hidden, more)
    local text = if hidden > 0
        then commas(hidden) .. (if more then "+" else "") .. " more not shown; search or filter to narrow"
        else "More not shown; search or filter to narrow"
    return { id = MORE_ID, path = MORE_ID, depth = 0, cells = { { key = "title", text = text, tone = "muted" } } }
end

local function table_node(ix, st, rows, total, cut)
    local sortable = st.view == "list"
    local columns = {}
    for _, c in COLS do
        table.insert(columns, kit.column(c.key, c.label, c.width, c.tree, c.wide, sortable))
    end
    local out = {}
    for i = 1, math.min(#rows, SHOWN) do out[i] = row_json(ix, st, rows[i]) end
    if #rows > SHOWN or cut then table.insert(out, more_row(#rows - SHOWN, cut)) end
    local selected = {}
    for id in st.list.selected do table.insert(selected, id) end
    table.sort(selected)
    local t = {
        component = "Table",
        columns = columns,
        sections = { { title = "", count = #rows, rows = out } },
        rowAction = { type = "select_bead" },
        foldAction = { type = "toggle_path" },
        selectAction = { type = "list.select" },
        selected = selected,
        sort = { key = st.list.sort, asc = st.list.asc },
        empty = if total == 0 then "No beads yet" else "Nothing matches the search and filters.",
    }
    -- A tree has no sortable column, so it carries no sort action and no header arrow.
    if sortable then t.sortAction = { type = "list.sort" } end
    return t
end

-- ───────────────────────────── the detail card ─────────────────────────────

local function branch(ix, b, nextf, depth, seen, budget)
    budget.n -= 1
    local k = b._k
    local link = kit.button(ix.short(b.id) .. "  " .. b.title, { type = "open_bead", id = b.id }, {
        style = "link", label_modifiers = { { lineLimit = 1 } },
    })
    local chip = kit.label(k, { chip = true, size = 11, tone = tone_of(k) })
    -- The title takes the line and the status chip keeps one column, so the chips of a list line up whatever the titles.
    local parts = { kit.box("h", { link, chip }, { spacing = 6, align = "center", widths = { "flex", 76 } }) }
    local kids = {}
    for _, x in nextf(b) do
        if not seen[x.id] then table.insert(kids, x) end
    end
    if #kids > 0 then
        if depth >= 4 then
            table.insert(parts, kit.label("… " .. #kids .. " more", { tone = "muted", size = 12 }))
        else
            local seen2 = {}
            for id in seen do seen2[id] = true end
            seen2[b.id] = true
            local inner = {}
            for i, x in kids do
                if budget.n <= 0 then
                    table.insert(inner, kit.label("… " .. (#kids - i + 1) .. " more not shown", { tone = "muted", size = 12 }))
                    break
                end
                table.insert(inner, branch(ix, x, nextf, depth + 1, seen2, budget))
            end
            table.insert(parts, kit.box("v", inner, { padding = { 0, 0, 0, 16 }, spacing = 2 }))
        end
    end
    return kit.box("v", parts, { spacing = 2 })
end

local function tree(ix, b, nextf, budget)
    local ks = nextf(b)
    if #ks == 0 then return kit.label("None.", { tone = "muted", size = 12 }) end
    local seen = { [b.id] = true }
    local out = {}
    for i, k in ks do
        if budget.n <= 0 then
            table.insert(out, kit.label("… " .. (#ks - i + 1) .. " more not shown", { tone = "muted", size = 12 }))
            break
        end
        table.insert(out, branch(ix, k, nextf, 1, seen, budget))
    end
    return kit.box("v", out, { spacing = 2 })
end

local function fact(name, value)
    local l = kit.label(name, { tone = "muted", size = 12 })
    return kit.box("h", { l, value }, { spacing = 12, widths = { 84, "flex" }, align = "firstTextBaseline" })
end

-- The open bead as a Modal; nil when `id` is no bead of the snapshot.
local function detail(ix, st, id)
    local b = ix.by_id[id]
    if not b then return nil end
    local k = b._k
    local c = {}
    -- Back keeps its slot: hidden and disabled when there is nowhere to go back to.
    table.insert(c, kit.button("← Back", { type = "back" }, { style = "link", disabled = #st.hist == 0, hideOff = true }))
    table.insert(c, kit.label(b.title, { size = 17, weight = "semibold" }))
    table.insert(c, fact("ID", kit.label(b.id, { mono = true, size = 12 })))
    table.insert(c, fact("Status", kit.label(k, { chip = true, size = 11, tone = tone_of(k) })))
    table.insert(c, fact("Priority", kit.label(if b.priority then "P" .. b.priority else "—", {})))
    table.insert(c, fact("Type", kit.label(b.kind, {})))
    -- Every fact keeps its row; a missing value reads "—", as Priority does.
    table.insert(c, fact("Labels", kit.label(if #(b.labels or NONE) > 0 then table.concat(b.labels, "  ·  ") else "—", {})))
    local par = b.parent and ix.by_id[b.parent]
    if par then
        table.insert(c, fact("Parent", kit.button(ix.short(par.id) .. "  " .. par.title, { type = "open_bead", id = par.id }, { style = "link" })))
    else
        table.insert(c, fact("Parent", kit.label("—", {})))
    end
    table.insert(c, fact("Owner", kit.label(b.owner or "—", {})))
    table.insert(c, fact("Updated", kit.label(tm.ago(b.updated, ix.now) .. " ago", {})))
    if b.description and b.description ~= "" then
        -- Flat: a rule above and below the text, no raised box.
        table.insert(c, ui.Divider{})
        table.insert(c, kit.label(b.description, { size = 13 }))
        table.insert(c, ui.Divider{})
    end
    local function section(title, nextf)
        local n = #nextf(b)
        local h = kit.label("", { runs = { { text = title, weight = "semibold", size = 13 }, { text = "  " .. n, tone = "muted", size = 13 } } })
        local t = tree(ix, b, nextf, { n = 40 })
        return kit.box("v", { h, t }, { spacing = 4, padding = { 8, 0, 0, 0 } })
    end
    table.insert(c, section("Depends on", ix.blockers))
    table.insert(c, section("Unlocks", ix.unlocked))
    if ix.kids[b.id] then table.insert(c, section("Children", ix.children)) end
    local body = kit.box("v", c, { spacing = 12, align = "start" })
    -- The button sits in the card's footer beside Close, with the last run's result before it.
    local footer = {}
    local act = act_of(ix, b)
    if act then
        -- The result slot is always there, empty until this bead's press answers, so the button never changes place.
        local ran = st.ran
        if ran and ran.bead ~= b.id then ran = nil end
        table.insert(footer, kit.label(if ran then ran.text else "", { tone = if ran and ran.bad then "bad" else "ok", weight = "semibold" }))
        local e = { style = "primary", disabled = act.disabled }
        if act.help ~= "" then e.help = act.help end
        table.insert(footer, kit.button(act.label, { type = "press", bead = b.id }, e))
    end
    return ui.Modal{ presented = true, dismissAction = { type = "close_detail" }, actions = footer, body }
end

-- ───────────────────────────── Focus and Graph ─────────────────────────────

local function is_drawn(view)
    return view == "focus" or view == "graph"
end

-- The feed's params for the drawn views: the search and filters narrow what the scene draws as they narrow the list.
local function graph_params(st, data)
    local l = st.list
    local f = {}
    for k, v in l.f do
        if #v > 0 then f[k] = v end
    end
    local neg = {}
    for k in l.neg do table.insert(neg, k) end
    table.sort(neg)
    local lst = { q = l.q, ["not"] = neg }
    -- An empty table would travel as an array, so an unset filter set is left out.
    if next(f) ~= nil then lst.f = f end
    return {
        dir = data.cwd, view = st.view, sel = st.sel or "", depth = math.floor(st.depth),
        width = math.floor(st.width), more = st.more or "", list = lst,
    }
end

-- Focus's toolbar: Back (disabled when there is nowhere to go back to) and the hops. Graph keeps the row, invisible, so the
-- caption and the drawing sit where Focus has them.
local function toolbar(st, focus)
    local back = kit.button("‹ Back", { type = "focus_back" }, { style = "link", disabled = #st.fhist == 0 })
    kit.modify(back, { frame = { minWidth = 56 } })
    local depth = ui.ChoicePicker{
        variant = "mutuallyExclusive", displayStyle = "menu", on_change = { type = "set_depth" }, value = { tostring(st.depth) },
        options = { { label = "1 hop", value = "1" }, { label = "2 hops", value = "2" }, { label = "3 hops", value = "3" } },
    }
    local row = kit.box("h", { back, depth }, { spacing = 8, align = "center", padding = { 10, 0, 4, 0 } })
    if not focus then
        kit.modify(row, { opacity = 0 })
        kit.modify(row, { disabled = true })
    end
    return row
end

local function legend(focus, cyc)
    local runs = {}
    for _, g in LEGEND do
        table.insert(runs, { text = g[1] .. " ", tone = g[3] })
        table.insert(runs, { text = g[2] .. "    ", tone = g[3], weight = "semibold" })
    end
    local hint
    if focus then
        hint = "solid arrow: blocks · dotted line: parent and child" .. (if cyc then " · red: dependency cycle" else "")
    else
        hint = "one dot per bead · drag to pan, wheel or pinch to zoom"
    end
    local words = kit.label("", { runs = runs, size = 12 })
    return kit.box("h", { words, kit.label(hint, { tone = "muted", size = 12 }) }, { spacing = 8, padding = { 10, 2, 0, 2 }, align = "center" })
end

-- The sheet behind a "+ N more" node: one row per hidden bead, a pick centres Focus on it. Not presented when nothing is open.
local function more_sheet(more)
    if not more then
        return ui.Modal{ presented = false, dismissAction = { type = "graph_escape" } }
    end
    local rows = {}
    for _, r in more.rows or NONE do
        table.insert(rows, {
            id = r.id, path = r.id, depth = 0, menu = {},
            cells = {
                { key = "id", text = r.id, mono = true, tone = "muted" },
                { key = "title", text = r.title },
                { key = "status", text = r.word, chip = true, tone = r.tone },
            },
        })
    end
    local t = kit.sized_table({
        component = "Table",
        columns = {
            kit.column("id", "ID", 150, false, false, false), kit.column("title", "Title", nil, false, false, false),
            kit.column("status", "Status", 110, false, false, false),
        },
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "focus_on" },
        empty = "",
    })
    return ui.Modal{
        presented = true, dismissAction = { type = "graph_escape" },
        ui.Column{ spacing = 8, kit.label(more.title or "", { size = 13, weight = "semibold" }), t },
    }
end

-- The body of the drawn views, in the slot the list views give their table: the toolbar, the caption, the drawing (or the
-- label that stands in for it, in a frame as tall as the drawing's) and the legend; and the sheet after the detail card.
-- Each part keeps its place whether the feed has answered or not.
local function drawn(st, data)
    local focus = st.view == "focus"
    local fresh, stale = data.feed("wheelhouse.beads_graph", graph_params(st, data))
    local value = fresh
    if fresh ~= nil then LAST[st.view] = fresh else value = LAST[st.view] end
    local caption, caption_tone, middle, cyc, more = "", "muted", nil, false, nil
    local height = CANVAS_HEIGHT
    local function standing_in(text)
        local l = kit.label(text, { tone = "muted" })
        kit.modify(l, { padding = 12 })
        local box = kit.box("v", { l }, {})
        kit.modify(box, { frame = { height = height, alignment = "leading" } })
        return box
    end
    if value == nil then
        st.center = nil
        middle = standing_in("Drawing the graph…")
    else
        st.center = value.center
        caption = value.caption or ""
        if value.captionBad then caption_tone = "bad" end
        cyc = value.cyc == true
        more = value.more
        local scene = value.scene
        if scene == nil or value.empty then
            middle = standing_in(value.empty or "")
        else
            local props = {}
            for k, v in scene do props[k] = v end
            props.escapeAction = { type = "graph_escape" }
            props.fitKey = st.view
            if focus then
                local b = scene.bounds
                local h = if b and type(b[4]) == "number" then b[4] else CANVAS_HEIGHT
                height = math.clamp(math.ceil(h), CANVAS_MIN, CANVAS_MAX)
                props.sizeAction = { type = "set_width" }
                props.sizeKnown = st.width
            end
            -- `height` is the stage's own height; the client sizes it to the pane's height otherwise and it spills out of a frame.
            props.height = height
            props.modifiers = { { clipShape = { cornerRadius = 0 } } }
            middle = ui.Canvas(props)
        end
    end
    local hint = {}
    if stale then
        table.insert(hint, kit.label("Showing the last data: the beads backend is not answering.", { tone = "warn", size = 12 }))
    end
    local body = ui.Column{
        spacing = 0, align = "stretch",
        toolbar(st, focus),
        kit.modify(kit.label(caption, { tone = caption_tone, size = 12 }), { padding = { 0, 2, 8, 2 } }),
        middle, legend(focus, cyc), ui.Column{ spacing = 0, table.unpack(hint) },
    }
    return body, more_sheet(more)
end

-- ───────────────────────────── the page ─────────────────────────────

local function problem_box(text, tone)
    local l = kit.label(text, { tone = tone })
    return kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 }, margin = { 10, 0, 0, 0 } })
end

local function page(state, data, snap, stale, panes)
    local st = state
    local now = snap.now
    if not now then
        -- No clock in a pane script: ages count from the newest update when the feed carries no `now`.
        now = 0
        for _, b in snap.beads do
            local e = tm.epoch(b.updated)
            if e > now then now = e end
        end
    end
    local ix = index(snap, panes, now)
    if panes then
        ix.live = {}
        for _, p in panes do ix.live[p.id] = true end
    end
    ix.pressing = {}
    for _, id in snap.pressing or NONE do ix.pressing[id] = true end

    local all = {}
    for _, b in snap.beads do
        if is_leaf(b) then table.insert(all, b) end
    end
    local function n(f)
        local c = 0
        for _, b in all do
            if f(b) then c += 1 end
        end
        return c
    end

    -- The top slot keeps its place whatever it holds, so nothing after it moves (a search field keeps its focus).
    local probs = {}
    for _, p in snap.problems or NONE do table.insert(probs, problem_box(p, "bad")) end
    if snap.error then table.insert(probs, problem_box(snap.error, "bad")) end
    local said = {}
    if st.said then table.insert(said, problem_box(st.said, if st.said_bad then "bad" else "ok")) end
    local hint = {}
    if stale then
        table.insert(hint, kit.label("Showing the last data: the beads backend is not answering.", { tone = "warn", size = 12 }))
    end
    local top = kit.box("v", {
        list.boxes(st.list, {
            { "Ready", "status", "ready", n(function(b) return b.ready == true and b.status == "open" end), true },
            { "Active", "status", "in progress", n(function(b) return b.status == "in_progress" end), false },
            { "Blocked", "status", "blocked", n(function(b) return b.status == "open" and not b.ready and #(b.blocked_by or NONE) > 0 end), false },
            { "Closed", "status", "done", n(function(b) return b.status == "closed" end), false },
        }),
        ui.Column{ spacing = 0, table.unpack(probs) },
        ui.Column{ spacing = 0, table.unpack(said) },
        ui.Column{ spacing = 0, table.unpack(hint) },
    }, { spacing = 0 })

    local children = { top }
    if not snap.has_repo then
        table.insert(children, kit.label("This space has no .beads directory, so there is nothing to list.", { tone = "muted" }))
    else
        local function picker(style, mods)
            local options = {}
            for _, v in VIEWS do table.insert(options, { label = v[2], value = v[1] }) end
            return ui.ChoicePicker{
                variant = "mutuallyExclusive", displayStyle = style, options = options, value = { st.view },
                on_change = { type = "set_view" }, modifiers = mods,
            }
        end
        local views = ui.ViewThatFits{
            ["in"] = "horizontal",
            picker("chips", { { fixedSize = { horizontal = true } } }),
            picker("menu", { { frame = { maxWidth = "infinity", alignment = "leading" } } }),
        }
        table.insert(children, list.find(st.list, { views }))

        local pri = {}
        for p = 0, 4 do
            table.insert(pri, { tostring(p), "P" .. p .. "  " .. n(function(b) return b.priority == p end) })
        end
        local kinds = {}
        for _, k in KINDS do table.insert(kinds, { k, k }) end
        local options = {
            kinds,
            list.counted(all, function(b) return { b.kind } end),
            list.counted(all, function(b) return b.labels or NONE end),
            pri,
        }

        -- The list bar and what it acts on are the list views' own: in Focus and Graph the listed beads are the ones the search
        -- and filters let through, as the List view shows them.
        local drew = is_drawn(st.view)
        local rows, cut = build(ix, if drew then { view = "list", list = st.list, collapsed = st.collapsed } else st)
        -- The ids the list actions take: the listed leaves in the order shown, narrowed to the selected rows among them.
        local listed = {}
        for _, r in rows do
            if is_leaf(r.b) then table.insert(listed, r.b.id) end
        end
        local took = list.taken(listed, st.list)
        st.dir = data.cwd
        st.ids = took
        local actions = list.actions(snap.scripts or NONE, took, #listed, "ids", "beads")
        table.insert(children, list.bar(SPEC, st.list, options, actions))
        local sheet
        if drew then
            local body
            body, sheet = drawn(st, data)
            table.insert(children, body)
        else
            table.insert(children, kit.box("v", { table_node(ix, st, rows, #all, cut) }, {}))
        end

        -- A closed panel is a Modal that is not presented, so the open one is replaced rather than left behind.
        local modal = st.detail and detail(ix, st, st.detail)
        table.insert(children, modal or ui.Modal{ presented = false, dismissAction = { type = "close_detail" } })
        if sheet then table.insert(children, sheet) end
    end
    return ui.Column{ align = "stretch", kit.box("v", children, { align = "start", padding = { 18, 16, 48, 16 } }) }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    local state = previous or {
        view = "list", list = list.new("status", true), collapsed = {}, detail = nil, hist = {},
        ran = nil, said = nil, said_bad = false,
    }
    -- Focus and Graph: the selected bead, the beads Focus was centred on before, the hops, the open "+ N more" list and the
    -- canvas width the client last reported.
    state.fhist = state.fhist or {}
    state.depth = state.depth or 2
    state.width = state.width or 900
    -- The beads whose presses have not answered yet, oldest first.
    state.pressed = state.pressed or {}
    return state
end

local function valid_view(v)
    for _, p in VIEWS do
        if p[1] == v then return true end
    end
    return false
end

-- Drops the last press result unless it is for `id`, the bead about to be shown.
local function forget(state, id)
    if state.ran and state.ran.bead ~= id then state.ran = nil end
end

local function update(state, msg)
    if msg.action then
        -- What wheeld answered for an action this pane called.
        local failed = msg.error ~= nil
        local words = if failed then msg.error else (type(msg.result) == "table" and msg.result.said) or "Done."
        if msg.action == "wheelhouse.press" then
            -- The answer names no bead: it is for the oldest press still waiting.
            local bead = table.remove(state.pressed, 1)
            state.ran = { bead = bead, text = words, bad = failed }
        elseif msg.action == "wheelhouse.run_list" then
            state.said, state.said_bad = words, failed
        end
        return
    end
    -- What the last list button said shows once.
    state.said, state.said_bad = nil, false
    if list.handle(state.list, SPEC, msg) then return end
    local t = msg.type
    if t == "set_view" then
        local v = if type(msg.value) == "table" then msg.value[1] else msg.value
        if valid_view(v) then
            state.view = v
            state.collapsed = {}
            state.more = nil
            state.list.selected = {}
        end
    elseif t == "toggle_path" then
        if state.collapsed[msg.path] then state.collapsed[msg.path] = nil else state.collapsed[msg.path] = true end
    elseif t == "select_bead" then
        if msg.id == MORE_ID then return end
        state.sel = msg.id
        state.hist = {}
        forget(state, msg.id)
        state.detail = msg.id
    elseif t == "open_bead" then
        state.sel = msg.id
        if state.detail then table.insert(state.hist, state.detail) end
        state.detail = msg.id
        forget(state, msg.id)
    elseif t == "back" then
        local prev = table.remove(state.hist)
        if prev then state.detail = prev end
    elseif t == "close_detail" then
        state.detail = nil
        state.hist = {}
    elseif t == "set_depth" then
        local v = if type(msg.value) == "table" then msg.value[1] else msg.value
        local d = tonumber(v)
        if d then
            state.depth = math.clamp(math.floor(d), 1, 3)
            state.more = nil
        end
    elseif t == "focus_on" then
        if msg.id and msg.id ~= "" and (state.known == nil or state.known[msg.id]) then
            if state.center and state.center ~= msg.id then table.insert(state.fhist, state.center) end
            state.sel = msg.id
            state.more = nil
        end
    elseif t == "focus_back" then
        local prev = table.remove(state.fhist)
        if prev then
            state.sel = prev
            state.more = nil
        end
    elseif t == "graph_select" then
        if msg.id == nil or msg.id == "" or msg.id == state.sel then state.sel = nil else state.sel = msg.id end
    elseif t == "graph_more" then
        if msg.key then state.more = if state.more == msg.key then nil else msg.key end
    elseif t == "set_width" then
        local w = tonumber(msg.value)
        if w then state.width = math.clamp(w, 300, 4000) end
    elseif t == "graph_escape" then
        if state.more then
            state.more = nil
        elseif state.view == "focus" and #state.fhist > 0 then
            state.sel = table.remove(state.fhist)
        elseif state.view == "graph" then
            state.sel = nil
        end
    elseif t == "press" then
        if state.dir and msg.bead then
            table.insert(state.pressed, msg.bead)
            wheel.action("wheelhouse.press", { dir = state.dir, bead = msg.bead })
        end
    elseif t == "list.run" then
        if state.dir and msg.script then
            wheel.action("wheelhouse.run_list", { dir = state.dir, script = msg.script, ids = state.ids or {} })
        end
    end
end

local function view(state, data)
    local snap, stale = data.feed("wheelhouse.beads", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the beads…" } }
    end
    local panes = data.feed("wheel.panes", {})
    -- The focus history and the selection drop beads the tracker no longer has.
    local ids = {}
    for _, b in snap.beads do ids[b.id] = true end
    local kept = {}
    for _, id in state.fhist do
        if ids[id] then table.insert(kept, id) end
    end
    state.fhist = kept
    state.known = ids
    if state.sel and not ids[state.sel] then state.sel = nil end
    return page(state, data, snap, stale, panes)
end

return { init = init, update = update, view = view }
