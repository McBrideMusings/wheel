-- The Runner pane: the commands of a project's [runner] tables (the machine's too) as a foldable tree under a search
-- field, a Running section pinned above it, and one run's log filling the pane.
--
-- `runner.commands` {dir} holds the commands, the project's recent runs (newest first) and each status check's reading;
-- `runner.log` {dir, id} holds one run's last 300 lines. `runner.run` starts a command and `runner.stop` ends a run.
-- The pane's own state (query, folded groups, the open run, the command asking, the last problem) lives in `state`.
-- `update` cannot read feeds, so `view` stores the directory and the commands there.
local NONE = {}

-- The last value drawn per project (and per run): a read that answers nil draws this meanwhile, so the pane keeps its list.
local LAST, LAST_LOG = {}, {}

-- The label of each command by name, set by `view` from the commands it draws: a run is filed under its command's name.
local LABELS = {}

local function label_of(name)
    return LABELS[name] or name
end

-- The line that shows the last problem has one height whether it is empty or not, so nothing under it moves.
local PROBLEM_HEIGHT = 20
local ROW_HEIGHT = 28

local COLOUR = { fg = "primary", muted = "secondary", ok = "green", warn = "orange", bad = "red" }

-- ───────────────────────────── pieces ─────────────────────────────

-- One Text. `o`: size, weight, mono, tone, lines (a line limit), flex (fills the row).
local function text(body, o)
    o = o or {}
    local mods = {}
    if o.size or o.weight or o.mono then
        table.insert(mods, { font = { size = o.size or 13, weight = o.weight or "regular", design = if o.mono then "monospaced" else "default" } })
    end
    if o.tone and COLOUR[o.tone] then table.insert(mods, { foregroundStyle = COLOUR[o.tone] }) end
    if o.lines then table.insert(mods, { lineLimit = o.lines }) end
    if o.flex then table.insert(mods, { frame = { maxWidth = "infinity", alignment = "leading" } }) end
    return ui.Text{ text = body, modifiers = mods }
end

-- A symbol, or a spinner for "spinner".
local function symbol(name, tone)
    if name == "spinner" then
        return ui.ProgressView{ modifiers = { { frame = { width = 14, height = 14 } } } }
    end
    local mods = {}
    if tone and COLOUR[tone] then table.insert(mods, { foregroundStyle = COLOUR[tone] }) end
    return ui.Icon{ name = name, modifiers = mods }
end

-- A Button whose label is a Text (`label`), an Icon (`icon`) or both. `help` is its hover text and accessibility label.
local function button(label, icon, action, help, o)
    o = o or {}
    local child
    if icon and label then
        child = ui.Row{ spacing = 4, children = { ui.Icon{ name = icon }, ui.Text{ text = label } } }
    elseif icon then
        child = ui.Icon{ name = icon }
    else
        child = ui.Text{ text = label }
    end
    local mods = {}
    if help then
        table.insert(mods, { help = help })
        table.insert(mods, { accessibilityLabel = help })
    end
    if o.tone and COLOUR[o.tone] then table.insert(mods, { foregroundStyle = COLOUR[o.tone] }) end
    local b = { component = "Button", child = child, action = action }
    if o.plain then b.variant = "borderless" end
    if #mods > 0 then b.modifiers = mods end
    return b
end

local function row(children, o)
    o = o or {}
    local mods = { { frame = { maxWidth = "infinity", alignment = "leading" } } }
    if o.indent and o.indent > 0 then table.insert(mods, { padding = { 0, 0, 0, o.indent } }) end
    if o.height then table.insert(mods, 1, { frame = { minHeight = o.height } }) end
    return ui.Row{ spacing = o.spacing or 8, children = children, modifiers = mods }
end

-- ───────────────────────────── times and states ─────────────────────────────

-- `0:42`, `1:12`, `1:02:03`: a length of time in seconds.
local function duration(secs)
    local s = math.max(0, math.floor(secs))
    if s >= 3600 then return string.format("%d:%02d:%02d", s // 3600, s % 3600 // 60, s % 60) end
    return string.format("%d:%02d", s // 60, s % 60)
end

-- `HH:MM:SS`, UTC, of the Unix time `ts`.
local function clock(ts)
    local s = math.floor(ts) % 86400
    return string.format("%02d:%02d:%02d", s // 3600, s % 3600 // 60, s % 60)
end

-- The state word, its symbol and its tone for a run.
local function state_of(run)
    local s, code = run.state, run.exit_code
    if s == "running" then return "running", "spinner", "warn" end
    if s == "exited" and code == 0 then return "exit 0", "checkmark.circle.fill", "ok" end
    if s == "exited" then return "exit " .. (if code ~= nil then tostring(code) else "?"), "xmark.circle.fill", "bad" end
    if s == "stopped" then return "stopped", "stop.circle.fill", "muted" end
    return "lost", "xmark.circle.fill", "bad"
end

-- A run's status as one line: `running 2:14` while it goes, else `exit 65 · 17:40:12`.
local function status_line(run, now)
    local word = state_of(run)
    if run.state == "running" then return word .. " " .. duration(now - run.started) end
    return word .. " · " .. clock(run.started)
end

-- ───────────────────────────── the tree ─────────────────────────────

-- Whether runner `r` answers the search `q` (lower case): its label, description, group path or name holds it.
local function matches(r, q)
    if q == "" then return true end
    local hay = string.lower(r.label .. " " .. (r.desc or "") .. " " .. table.concat(r.group or NONE, " ") .. " " .. r.name)
    return string.find(hay, q, 1, true) ~= nil
end

-- The commands that match as a tree: each node holds its commands, then its groups, in the order the file gave them.
local function build(runners, q)
    local root = { cmds = {}, kids = {}, kidmap = {}, count = 0 }
    for _, r in runners do
        if matches(r, q) then
            local node, path = root, ""
            node.count += 1
            for _, g in r.group or NONE do
                path = if path == "" then g else path .. "." .. g
                local kid = node.kidmap[g]
                if not kid then
                    kid = { name = g, path = path, cmds = {}, kids = {}, kidmap = {}, count = 0 }
                    node.kidmap[g] = kid
                    table.insert(node.kids, kid)
                end
                node = kid
                node.count += 1
            end
            table.insert(node.cmds, r)
        end
    end
    return root
end

-- ───────────────────────────── rows ─────────────────────────────

-- The newest run of each command, and the runs that go.
local function latest_runs(runs)
    local latest, going = {}, {}
    for _, r in runs do
        if latest[r.name] == nil then latest[r.name] = r end
        if r.state == "running" then table.insert(going, r) end
    end
    return latest, going
end

local function command_row(st, r, latest, check, now, depth)
    local indent = depth * 16
    if st.asking == r.name then
        local q = r.confirm or ""
        return row({
            text(q, { flex = true, tone = "warn" }),
            button("Cancel", nil, { type = "cancel" }, "Do not run " .. r.label),
            -- The Run button names the question it answers; the command it was drawn with is what it confirms.
            button("Run", nil, { type = "confirm", name = r.name, command = r.command }, "Run " .. r.label .. ": " .. q),
        }, { indent = indent, height = ROW_HEIGHT })
    end
    local lines = { text(r.label, { weight = "semibold", flex = true }) }
    if r.desc then table.insert(lines, text(r.desc, { size = 12, tone = "muted", flex = true, lines = 2 })) end
    if latest then
        local _, sym, tone = state_of(latest)
        table.insert(lines, ui.Row{ spacing = 5, children = { symbol(sym, tone), text(status_line(latest, now), { size = 12, tone = tone }) } })
    end
    local kids = { ui.Column{ spacing = 2, children = lines, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } } }
    if r.status and check then
        table.insert(kids, text((if check.up then "up" else "down") .. (if check.text ~= "" then " · " .. check.text else ""),
            { size = 12, tone = if check.up then "ok" else "bad", lines = 1 }))
    end
    if latest then
        table.insert(kids, button("Open", nil, { type = "open", id = latest.id }, "Open the log of " .. r.label))
    end
    table.insert(kids, button(nil, "play.fill", { type = "run", name = r.name }, "Run " .. r.label))
    return row(kids, { indent = indent, height = ROW_HEIGHT })
end

local function group_row(st, node, depth, q)
    local folded = st.folded[node.path] == true and q == ""
    return row({
        button(node.name .. "  " .. node.count, if folded then "chevron.right" else "chevron.down", { type = "fold", path = node.path },
            (if folded then "Open " else "Fold ") .. node.name, { plain = true }),
    }, { indent = depth * 16, height = ROW_HEIGHT })
end

-- The rows of `node` and everything it holds, folded groups cut.
local function tree_rows(st, node, depth, q, cmds, rows, now)
    for _, r in node.cmds do
        table.insert(rows, command_row(st, r, cmds.latest[r.name], cmds.checks[r.name], now, depth))
    end
    for _, kid in node.kids do
        table.insert(rows, group_row(st, kid, depth, q))
        if not (st.folded[kid.path] == true and q == "") then
            tree_rows(st, kid, depth + 1, q, cmds, rows, now)
        end
    end
end

-- The Running section: a row per run that goes, with Open and Stop.
local function running_section(going, now)
    local kids = {}
    for _, run in going do
        table.insert(kids, row({
            symbol("spinner"),
            text(label_of(run.name), { weight = "semibold", flex = true }),
            text(duration(now - run.started), { size = 12, tone = "muted", mono = true }),
            button("Open", nil, { type = "open", id = run.id }, "Open the log of " .. label_of(run.name)),
            button("Stop", nil, { type = "stop", id = run.id }, "Stop " .. label_of(run.name), { tone = "bad" }),
        }, { height = ROW_HEIGHT }))
    end
    if #kids > 0 then
        table.insert(kids, 1, text("Running", { size = 11, weight = "semibold", tone = "muted" }))
        table.insert(kids, ui.Divider{})
    end
    return ui.Column{ spacing = 4, children = kids, modifiers = { { padding = { 0, 10 } } } }
end

-- One fixed-height line for the last problem, empty when there is none.
local function problem_slot(st)
    return ui.Column{
        children = { text(st.problem or "", { size = 12, tone = "bad", lines = 1, flex = true }) },
        modifiers = { { padding = { 0, 10 } }, { frame = { height = PROBLEM_HEIGHT, maxWidth = "infinity", alignment = "leading" } } },
    }
end

-- ───────────────────────────── the two views ─────────────────────────────

local function list_view(st, data, snap)
    local now = snap.now or 0
    local runners = snap.runners or NONE
    local runs = snap.runs or NONE
    st.dir, st.pane = data.cwd, data.pane
    st.runners = {}
    for _, r in runners do st.runners[r.name] = { command = r.command, confirm = r.confirm } end
    local latest, going = latest_runs(runs)
    local q = string.lower(st.query or "")
    local tree = build(runners, q)
    local rows = {}
    tree_rows(st, tree, 0, q, { latest = latest, checks = snap.checks or NONE }, rows, now)
    if #rows == 0 then
        rows[1] = text(if #runners == 0 then "This project has no [runner] commands." else "Nothing matches the search.", { tone = "muted" })
    end
    rows.spacing = 2
    -- Padding, not a frame: a List with no frame fills the pane under the fields.
    rows.modifiers = { { padding = { 4, 10 } } }
    local field = ui.TextField{
        label = "Search commands", value = st.query or "", variant = "shortText", on_change = { type = "query" },
        modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
    }
    return ui.Column{
        spacing = 0,
        children = {
            ui.Row{ children = { field }, modifiers = { { padding = { 10, 10, 4, 10 } } } },
            problem_slot(st),
            running_section(going, now),
            ui.List(rows),
        },
    }
end

-- The positional id of the log List in `log_view`'s root, for `scrollTo`.
local LOG_AT = "root.2"

local function log_view(st, data, snap, log)
    local now = snap.now or 0
    st.dir, st.pane = data.cwd, data.pane
    local _, going = latest_runs(snap.runs or NONE)
    local head = { button("Back", "chevron.left", { type = "back" }, "Back to the commands") }
    if log then
        local word, sym, tone = state_of(log)
        table.insert(head, text(label_of(log.name or log.id), { weight = "semibold" }))
        table.insert(head, ui.Row{ spacing = 5, children = { symbol(sym, tone), text(if log.state == "running" then status_line(log, now) else word, { size = 12, tone = tone }) } })
        table.insert(head, text("", { flex = true }))
        if log.state == "running" then
            table.insert(head, button("Stop", nil, { type = "stop", id = log.id }, "Stop " .. label_of(log.name or log.id), { tone = "bad" }))
        end
    else
        table.insert(head, text("Reading the log…", { tone = "muted", flex = true }))
    end
    -- A button for each other command that goes: it opens that run's log.
    local others = {}
    for _, run in going do
        if run.id ~= st.open then
            table.insert(others, button(label_of(run.name), "play.fill", { type = "open", id = run.id }, "Open the log of " .. label_of(run.name)))
        end
    end
    local lines = {}
    if log and log.error then
        lines[1] = text(log.error, { tone = "bad" })
    else
        for _, l in (log and log.lines) or NONE do
            table.insert(lines, text(l, { size = 12, mono = true, flex = true }))
        end
        if #lines == 0 then lines[1] = text("No output yet.", { tone = "muted" }) end
    end
    lines.spacing = 1
    lines.modifiers = { { padding = { 4, 10 } } }
    lines.scrollTo = LOG_AT .. "." .. (#lines - 1)
    return ui.Column{
        spacing = 0,
        children = {
            row(head, { height = ROW_HEIGHT, spacing = 10 }),
            ui.Column{ spacing = 0, children = { problem_slot(st), row(others, { height = ROW_HEIGHT }) } },
            ui.List(lines),
        },
    }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or { query = "", folded = {}, open = nil, asking = nil, problem = nil }
end

-- What a `runner.run` answer says: nothing to show when it started, else why not.
local function ran(state, msg)
    if msg.error then
        state.problem = tostring(msg.error)
        return
    end
    local r = msg.result or NONE
    if r.started then
        state.asking, state.problem = nil, nil
    elseif r.guard then
        state.asking = nil
        state.problem = (state.starting or "the command") .. " did not start: " .. r.guard
    elseif r.confirm then
        -- The entry changed after the question was shown: ask again with what it says now.
        state.asking = state.starting
        state.problem = (state.starting or "the command") .. " changed since it was asked about; answer again"
    end
end

local function update(state, msg)
    if msg.action then
        if msg.action == "runner.run" then
            ran(state, msg)
        elseif msg.action == "runner.stop" and msg.error then
            state.problem = tostring(msg.error)
        end
        return
    end
    local t = msg.type
    if t == "query" then
        state.query = if type(msg.value) == "string" then msg.value else ""
    elseif t == "fold" then
        if state.folded[msg.path] then state.folded[msg.path] = nil else state.folded[msg.path] = true end
    elseif t == "open" then
        state.open, state.problem = msg.id, nil
    elseif t == "back" then
        state.open, state.problem = nil, nil
    elseif t == "cancel" then
        state.asking = nil
    elseif not state.dir then
        return
    elseif t == "run" then
        local known = state.runners and state.runners[msg.name]
        state.problem = nil
        if known and known.confirm then
            -- A command with a question starts nothing until its Run button answers it.
            state.asking = msg.name
        else
            state.starting = msg.name
            wheel.action("runner.run", { dir = state.dir, name = msg.name })
        end
    elseif t == "confirm" then
        state.problem = nil
        state.starting = msg.name
        wheel.action("runner.run", { dir = state.dir, name = msg.name, confirmed = msg.command })
    elseif t == "stop" then
        state.problem = nil
        wheel.action("runner.stop", { dir = state.dir, id = msg.id })
    end
end

local function view(state, data)
    local fresh = data.feed("runner.commands", { dir = data.cwd })
    local snap = fresh
    if fresh ~= nil then LAST[data.cwd] = fresh else snap = LAST[data.cwd] end
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the commands…" } }
    end
    LABELS = {}
    for _, r in snap.runners or NONE do LABELS[r.name] = r.label end
    if state.open then
        local key = data.cwd .. "\0" .. state.open
        local log = data.feed("runner.log", { dir = data.cwd, id = state.open })
        if log ~= nil then LAST_LOG[key] = log else log = LAST_LOG[key] end
        return log_view(state, data, snap, log)
    end
    return list_view(state, data, snap)
end

return { init = init, update = update, view = view }
