-- The Graph pane: a project's open beads as a dependency graph on a Canvas. One node per open bead, one curve from each
-- blocker to the bead it blocks. Hovering a node highlights it and its edges, a click opens the bead in a card under the
-- drawing, and a drag pans.
--
-- The data is `wheelhouse.beads`. The layout (a column per dependency depth, rows ordered to cut crossings) is laid out
-- here, once per change of the beads; `update` cannot read feeds, so `view` keeps the layout in LAYOUT for the pointer
-- events' hit tests. `state` holds only what the person did (pan, hover, the open bead) and what a check reads back.
local kit = require("lib/ui")

local NONE = {}

local NODE_W, NODE_H = 210, 30
local GAP_X, GAP_Y = 72, 12
local MARGIN = 20
local CANVAS_HEIGHT = 520
local DRAG_SLOP = 4
local TITLE_CHARS = 26

-- The status tones a node's border takes.
local TONES = { ready = "ok", ["in progress"] = "accent", blocked = "warn", deferred = "faint", open = "muted" }

-- The layout of the beads last drawn: `sig` says which beads it was made from, `nodes` is a list of
-- `{ id, x, y, bead }`, `by_id` finds one, `edges` is a list of `{ from, to }` ids (blocker, blocked) and `pos` is
-- `id -> { x, y }`, the copy `state` shows a check. Kept here and not in `state`, which a check reads as JSON.
local LAYOUT = { sig = "", nodes = {}, by_id = {}, edges = {}, pos = {} }

local function kind_of(b)
    if b.status == "in_progress" then return "in progress" end
    if b.status == "deferred" then return "deferred" end
    if b.ready then return "ready" end
    if #(b.blocked_by or NONE) > 0 then return "blocked" end
    return "open"
end

-- ───────────────────────────── layout ─────────────────────────────

local function signature(beads)
    local parts = {}
    for _, b in beads do
        if b.status ~= "closed" then
            table.insert(parts, b.id .. "|" .. b.title .. "|" .. kind_of(b) .. "|" .. table.concat(b.blocked_by or NONE, ","))
        end
    end
    return table.concat(parts, "\n")
end

-- The open beads laid out: the column of a bead is one past its deepest open blocker (a cycle stops at the bead count),
-- and each column's rows are ordered by the mean row of the neighbours in the columns beside it.
local function layout(beads)
    local open, by_id = {}, {}
    for _, b in beads do
        if b.status ~= "closed" then
            table.insert(open, b)
            by_id[b.id] = b
        end
    end
    table.sort(open, function(a, b) return a.id < b.id end)
    local n = #open

    local edges, blockers, blocking = {}, {}, {}
    for _, b in open do
        blockers[b.id] = {}
        blocking[b.id] = blocking[b.id] or {}
    end
    for _, b in open do
        for _, from in b.blocked_by or NONE do
            if by_id[from] and from ~= b.id then
                table.insert(edges, { from = from, to = b.id })
                table.insert(blockers[b.id], from)
                table.insert(blocking[from], b.id)
            end
        end
    end

    local col = {}
    for _, b in open do col[b.id] = 0 end
    for _ = 1, n do
        local moved = false
        for _, e in edges do
            local want = math.min(col[e.from] + 1, n)
            if col[e.to] < want then
                col[e.to] = want
                moved = true
            end
        end
        if not moved then break end
    end

    local columns, last = {}, 0
    for _, b in open do
        local c = col[b.id] + 1
        columns[c] = columns[c] or {}
        table.insert(columns[c], b.id)
        last = math.max(last, c)
    end
    for c = 1, last do columns[c] = columns[c] or {} end

    local row = {}
    local function number(c)
        for i, id in columns[c] do row[id] = i end
    end
    local function order(c, neighbours)
        local key = {}
        for _, id in columns[c] do
            local sum, count = 0, 0
            for _, other in neighbours[id] do
                sum += row[other]
                count += 1
            end
            key[id] = if count > 0 then sum / count else row[id]
        end
        table.sort(columns[c], function(a, b)
            if key[a] ~= key[b] then return key[a] < key[b] end
            return a < b
        end)
        number(c)
    end
    for c = 1, last do number(c) end
    for _ = 1, 3 do
        for c = 2, last do order(c, blockers) end
        for c = last - 1, 1, -1 do order(c, blocking) end
    end

    local nodes, lookup, pos = {}, {}, {}
    for c = 1, last do
        for i, id in columns[c] do
            local node = {
                id = id, bead = by_id[id],
                x = MARGIN + (c - 1) * (NODE_W + GAP_X), y = MARGIN + (i - 1) * (NODE_H + GAP_Y),
            }
            table.insert(nodes, node)
            lookup[id] = node
            pos[id] = { x = node.x, y = node.y }
        end
    end
    return { nodes = nodes, by_id = lookup, edges = edges, pos = pos }
end

local function sync(beads)
    local sig = signature(beads)
    if sig == LAYOUT.sig then return end
    local l = layout(beads)
    l.sig = sig
    LAYOUT = l
end

-- ───────────────────────────── pointer ─────────────────────────────

-- The node under the stage point (x, y) when the drawing is panned by (ox, oy), or nil.
local function hit(x, y, ox, oy)
    local wx, wy = x - ox, y - oy
    for _, n in LAYOUT.nodes do
        if wx >= n.x and wx <= n.x + NODE_W and wy >= n.y and wy <= n.y + NODE_H then return n.id end
    end
    return nil
end

local function pointer(state, msg)
    if msg.kind == "move" then
        state.hover = hit(msg.x, msg.y, state.ox, state.oy)
    elseif msg.kind == "down" then
        state.press = { x = msg.x, y = msg.y, moved = false }
        state.hover = hit(msg.x, msg.y, state.ox, state.oy)
    elseif msg.kind == "drag" then
        local p = state.press
        if p then
            if p.moved then
                state.ox += msg.dx
                state.oy += msg.dy
                state.hover = nil
            elseif math.abs(msg.x - p.x) > DRAG_SLOP or math.abs(msg.y - p.y) > DRAG_SLOP then
                -- The first event past the slop carries the whole movement since the press, so the drawing keeps up with the pointer.
                p.moved = true
                state.ox += msg.x - p.x
                state.oy += msg.y - p.y
                state.hover = nil
            end
        end
    elseif msg.kind == "up" then
        local p = state.press
        state.press = nil
        if p and not p.moved then
            local id = hit(msg.x, msg.y, state.ox, state.oy)
            if id then state.open = id end
        end
        state.hover = hit(msg.x, msg.y, state.ox, state.oy)
    end
end

-- ───────────────────────────── drawing ─────────────────────────────

local function clip(title)
    if utf8.len(title) == nil or utf8.len(title) <= TITLE_CHARS then return title end
    return string.sub(title, 1, utf8.offset(title, TITLE_CHARS) - 1) .. "…"
end

local function draw_graph(state)
    return function(ctx, w, h)
        local hover = state.hover
        ctx:translate(state.ox, state.oy)
        -- Edges first, so a node covers the ends of its curves. The hovered node's edges are drawn last and on top.
        local function edge(e, lit)
            local a, b = LAYOUT.by_id[e.from], LAYOUT.by_id[e.to]
            local x0, y0 = a.x + NODE_W, a.y + NODE_H / 2
            local x1, y1 = b.x, b.y + NODE_H / 2
            local mid = (x1 - x0) / 2
            ctx.strokeStyle = if lit then "accent" else "muted"
            ctx.lineWidth = if lit then 2 else 1.5
            ctx:beginPath()
            ctx:moveTo(x0, y0)
            ctx:bezierCurveTo(x0 + mid, y0, x1 - mid, y1, x1, y1)
            ctx:stroke()
            ctx.fillStyle = ctx.strokeStyle
            ctx:beginPath()
            ctx:moveTo(x1, y1)
            ctx:lineTo(x1 - 7, y1 - 4)
            ctx:lineTo(x1 - 7, y1 + 4)
            ctx:closePath()
            ctx:fill()
        end
        for _, e in LAYOUT.edges do
            if e.from ~= hover and e.to ~= hover then edge(e, false) end
        end
        if hover then
            for _, e in LAYOUT.edges do
                if e.from == hover or e.to == hover then edge(e, true) end
            end
        end
        for _, n in LAYOUT.nodes do
            local lit = n.id == hover
            local tone = TONES[kind_of(n.bead)] or "muted"
            ctx.fillStyle = if lit then "accentFill" else "fill"
            ctx:beginPath()
            ctx:roundRect(n.x, n.y, NODE_W, NODE_H, 6)
            ctx:fill()
            ctx.strokeStyle = if n.id == state.open then "fg" else tone
            ctx.lineWidth = if lit or n.id == state.open then 2 else 1
            ctx:beginPath()
            ctx:roundRect(n.x, n.y, NODE_W, NODE_H, 6)
            ctx:stroke()
            ctx.font = "bold 10px monospace"
            ctx.fillStyle = tone
            ctx:fillText(n.id, n.x + 8, n.y + 12)
            ctx.font = "11px sans-serif"
            ctx.fillStyle = "fg"
            ctx:fillText(clip(n.bead.title), n.x + 8, n.y + 24)
        end
    end
end

-- ───────────────────────────── the page ─────────────────────────────

local function names(ids)
    if #ids == 0 then return "none" end
    return table.concat(ids, ", ")
end

local function card(id)
    local n = LAYOUT.by_id[id]
    if not n then return nil end
    local b = n.bead
    local blocked_by, blocks = {}, {}
    for _, e in LAYOUT.edges do
        if e.to == id then table.insert(blocked_by, e.from) end
        if e.from == id then table.insert(blocks, e.to) end
    end
    local description = b.description or ""
    if utf8.len(description) ~= nil and utf8.len(description) > 600 then
        description = string.sub(description, 1, utf8.offset(description, 600) - 1) .. "…"
    end
    local head = kit.box("h", {
        kit.label(b.id, { mono = true, weight = "semibold", tone = TONES[kind_of(b)] or "muted" }),
        kit.label(b.title, { weight = "semibold" }),
        kit.button("Close", { type = "close" }, { style = "link" }),
    }, { spacing = 8, align = "center" })
    return kit.box("v", {
        head,
        kit.label(kind_of(b) .. " · " .. (b.kind or "task") .. " · P" .. tostring(b.priority or "-"), { tone = "muted", size = 12 }),
        kit.label("Blocked by " .. names(blocked_by) .. " · Blocks " .. names(blocks), { tone = "muted", size = 12 }),
        kit.label(description, { size = 12 }),
    }, { spacing = 4, padding = 12, border = true })
end

local function init(previous)
    return previous or { ox = 0, oy = 0, hover = nil, open = nil, press = nil }
end

local function update(state, msg)
    if msg.type == "pointer" then
        pointer(state, msg)
    elseif msg.type == "close" then
        state.open = nil
    end
    return state
end

local function view(state, data)
    local snap, stale = data.feed("wheelhouse.beads", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the beads…" } }
    end
    sync(snap.beads)
    -- The open bead and the hover drop a bead the tracker no longer lists as open.
    if state.open and not LAYOUT.by_id[state.open] then state.open = nil end
    if state.hover and not LAYOUT.by_id[state.hover] then state.hover = nil end
    state.nodes = #LAYOUT.nodes
    state.edges = #LAYOUT.edges
    state.layout = LAYOUT.pos

    local caption = tostring(#LAYOUT.nodes) .. " open issues · " .. tostring(#LAYOUT.edges) .. " dependencies"
    local parts = {}
    if stale then
        table.insert(parts, kit.label("Showing the last data: the beads backend is not answering.", { tone = "warn", size = 12 }))
    end
    table.insert(parts, kit.modify(kit.label(caption, { tone = "muted", size = 12 }), { padding = { 10, 2, 8, 2 } }))
    if #LAYOUT.nodes == 0 then
        table.insert(parts, kit.modify(kit.label("No open issues", { tone = "muted" }), { padding = 12 }))
    else
        table.insert(parts, ui.Canvas{
            key = "graph", height = CANVAS_HEIGHT, draw = draw_graph(state), on_pointer = { type = "pointer" },
            modifiers = { { clipShape = { cornerRadius = 0 } } },
        })
        table.insert(parts, kit.modify(kit.label("drag to pan · hover to highlight · click to open", { tone = "muted", size = 12 }), { padding = { 10, 2, 0, 2 } }))
    end
    local open = state.open and card(state.open)
    if open then table.insert(parts, open) end
    return ui.Column{ spacing = 0, align = "stretch", table.unpack(parts) }
end

return { init = init, update = update, view = view }
