-- The Issues pane: a project's GitHub issues in a sortable table with Open and Closed boxes, filters and the project's
-- list buttons, a field to open an issue, and a detail Modal with a Close issue button. A port of the page in
-- src/issues/serve.rs.
--
-- `wheelhouse.issues` holds the list and the project's list scripts; `wheelhouse.issue_create` and
-- `wheelhouse.issue_close` write to GitHub and `wheelhouse.run_list` runs a list script. The pane's own state (search and
-- filters, sort, selection, the open issue, the new-issue draft, the last result) lives in `state`. `update` cannot
-- read feeds, so `view` stores the directory, the pane id and the listed numbers there.
local kit = require("lib/ui")
local list = require("lib/list")
local tm = require("lib/time")
local sorting = require("lib/sort")

local NONE = {}

-- The most table rows drawn: the runtime refuses a view tree over 100,000 values, and a Table row costs about 41.
local MAX_ROWS = 1000

-- The status slot holds the last result, else why the list could not be read, else nothing. It has one height (two lines of
-- text and the box around them) in every case, so nothing below it moves.
local STATUS_HEIGHT = 56

-- The most lines of an issue's body the detail draws; the rest is on GitHub.
local BODY_LINES = 12

-- The assignee filter's value for an issue nobody is assigned.
local NOBODY = "nobody"

-- The last feed value drawn per project: a read that answers nil (the clock refresh, a hop) draws this meanwhile, so the
-- pane keeps its table. Kept here and not in `state`, which a check reads as JSON.
local LAST = {}

-- ───────────────────────────── what the list says ─────────────────────────────

local SPEC = {
    filters = {
        { key = "state", name = "state", hidden = "closed" },
        { key = "label", name = "label" },
        { key = "who", name = "assignee" },
        { key = "author", name = "author" },
    },
    sorts = { "number", "title", "state", "labels", "who", "author", "updated" },
    has = function(i, key, value)
        if key == "state" then return i.state == value end
        if key == "label" then return table.find(i.labels or NONE, value) ~= nil end
        if key == "who" then
            if value == NOBODY then return #(i.assignees or NONE) == 0 end
            return table.find(i.assignees or NONE, value) ~= nil
        end
        if key == "author" then return i.author == value end
        return false
    end,
    haystack = function(i)
        return "#" .. i.number .. " " .. i.title .. " " .. table.concat(i.labels or NONE, " ") .. " "
            .. table.concat(i.assignees or NONE, " ") .. " " .. (i.author or "")
    end,
}

local function joined(items)
    return string.lower(table.concat(items or NONE, ","))
end

local function compare(key, a, b)
    if key == "number" then return sorting.cmp(a.number, b.number) end
    if key == "title" then return sorting.cmp(string.lower(a.title), string.lower(b.title)) end
    if key == "state" then return sorting.cmp(a.state, b.state) end
    if key == "labels" then return sorting.cmp(joined(a.labels), joined(b.labels)) end
    if key == "who" then return sorting.cmp(joined(a.assignees), joined(b.assignees)) end
    if key == "author" then return sorting.cmp(string.lower(a.author or ""), string.lower(b.author or "")) end
    return sorting.cmp(a.updated or 0, b.updated or 0)
end

local function tie(a, b)
    return sorting.cmp(a.number, b.number)
end

-- ───────────────────────────── the table ─────────────────────────────

local function issue_row(st, i, now)
    local closed = i.state == "closed"
    local labels, who = table.concat(i.labels or NONE, ", "), table.concat(i.assignees or NONE, ", ")
    local cells = {
        kit.cell("#" .. i.number, { key = "number", mono = true, tone = "muted", sort = i.number }),
        kit.cell(i.title, { key = "title", sort = string.lower(i.title) }),
        kit.cell(i.state, { key = "state", chip = true, tone = if closed then "muted" else "ok", sort = i.state }),
        kit.cell(labels, { key = "labels", tone = "muted", size = 12, sort = joined(i.labels) }),
        kit.cell(who, { key = "who", tone = "muted", sort = joined(i.assignees) }),
        kit.cell(i.author or "", { key = "author", tone = "muted", sort = string.lower(i.author or "") }),
        kit.cell(tm.ago_ts(i.updated, now), { key = "updated", tone = "muted", sort = i.updated or 0 }),
    }
    local id = tostring(i.number)
    local row = { id = id, path = id, depth = 0, kids = 0, collapsed = false, cells = cells, menu = {} }
    if closed then row.dim = true end
    if st.sel == i.number then row.sel = true end
    return row
end

local function issues_table(st, snap, shown, now)
    local rows = {}
    for n, i in shown do
        if n > MAX_ROWS then break end
        rows[n] = issue_row(st, i, now)
    end
    local columns = {
        kit.column("number", "#", 48, false, false, true),
        kit.column("title", "Title", "flex", false, false, true),
        kit.column("state", "State", 56, false, false, true),
        kit.column("labels", "Labels", 100, false, true, true),
        kit.column("who", "Assignee", 80, false, true, true),
        kit.column("author", "Author", 80, false, false, true),
        kit.column("updated", "Updated", 84, false, false, true),
    }
    local selected = {}
    for id in st.list.selected do table.insert(selected, id) end
    table.sort(selected)
    return kit.sized_table({
        component = "Table", columns = columns,
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "select_issue" }, foldAction = { type = "select_issue" },
        sortAction = { type = "list.sort" }, selectAction = { type = "list.select" }, selected = selected,
        sort = { key = st.list.sort, asc = st.list.asc },
        empty = if snap.error then "GitHub issues could not be read."
            elseif #snap.issues == 0 then "No issues on GitHub." else "Nothing matches the search and filters.",
    })
end

-- ───────────────────────────── the detail ─────────────────────────────

local function fact(name, value)
    local l = kit.label(name, { tone = "muted", size = 12 })
    return kit.box("h", { l, value }, { spacing = 12, widths = { 84, "flex" }, align = "firstTextBaseline" })
end

-- The open issue as a Modal. The footer's label is the last result, so a failed close is seen where it was asked.
local function modal(st, issue)
    local c = {}
    table.insert(c, kit.label(issue.title, { size = 17, weight = "semibold" }))
    table.insert(c, fact("Number", kit.label("#" .. issue.number, { mono = true, size = 12 })))
    table.insert(c, fact("State", kit.label(issue.state, { chip = true, tone = if issue.state == "open" then "ok" else "muted" })))
    table.insert(c, fact("Author", kit.label(issue.author or "", {})))
    local labels, who = issue.labels or NONE, issue.assignees or NONE
    if #labels > 0 then table.insert(c, fact("Labels", kit.label(table.concat(labels, "  ·  "), {}))) end
    if #who > 0 then table.insert(c, fact("Assignees", kit.label(table.concat(who, ", "), {}))) end
    table.insert(c, fact("URL", kit.label(issue.url or "", { mono = true, size = 12 })))
    if issue.body and issue.body ~= "" then
        -- A Modal does not scroll, so a long body is cut at BODY_LINES lines and the footer stays in view.
        local text = kit.label(string.sub(issue.body, 1, 3000), { size = 13 })
        kit.modify(text, { lineLimit = BODY_LINES })
        local shown = { text }
        if #issue.body > 400 then
            table.insert(shown, kit.label("The whole text is on GitHub.", { tone = "muted", size = 12 }))
        end
        table.insert(c, kit.box("v", shown, { padding = { 10, 12 }, fill = "soft", radius = 8, align = "start", spacing = 6 }))
    end
    local body = kit.box("v", c, { spacing = 12, align = "start" })
    local footer = {
        kit.label(if st.said then st.said else "", { tone = if st.said_bad then "bad" else "ok", weight = "semibold" }),
    }
    if issue.state == "open" then
        table.insert(footer, kit.button("Close issue", { type = "close_issue" }, { help = "Close this issue on GitHub" }))
    end
    return ui.Modal{ presented = true, dismissAction = { type = "close_detail" }, actions = footer, body }
end

-- ───────────────────────────── the page ─────────────────────────────

-- `text` in a box (at most two lines, cut at the end), or nothing, in a slot of the same height either way.
local function status_slot(text, tone)
    local kids = {}
    if text then
        local l = kit.label(text, { tone = tone })
        kit.modify(l, { lineLimit = 2 })
        table.insert(kids, kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 } }))
    end
    local slot = ui.Column{ align = "stretch", spacing = 0, table.unpack(kids) }
    slot.modifiers = { { frame = { height = STATUS_HEIGHT, alignment = "center" } } }
    return slot
end

local function page(st, data, snap)
    local now = snap.now or 0
    local issues = snap.issues or NONE
    snap.issues = issues
    st.dir, st.pane = data.cwd, data.pane

    local function n(state)
        local c = 0
        for _, i in issues do
            if i.state == state then c += 1 end
        end
        return c
    end

    -- The counts read "—" while the list could not be read, so a failed read is not taken for no issues.
    local function count(state)
        if snap.error then return "—" end
        return n(state)
    end

    local open
    for _, i in issues do
        if i.number == st.sel then open = i end
    end
    if st.sel and not open then st.sel = nil end

    -- One slot: the last result, else the read error or the first wheel.toml problem.
    local failed = snap.error ~= nil
    local status_text, status_tone
    if st.said and not open then
        status_text, status_tone = st.said, if st.said_bad then "bad" else "ok"
    elseif snap.error then
        status_text, status_tone = snap.error, "bad"
    elseif #(snap.problems or NONE) > 0 then
        local more = #snap.problems - 1
        status_text, status_tone = snap.problems[1] .. (if more > 0 then "  (+" .. more .. " more)" else ""), "bad"
    end
    local status = status_slot(status_text, status_tone)

    local draft = st.draft or ""
    local field = ui.TextField{
        label = "New issue title", value = draft, variant = "shortText", on_change = { type = "set_draft" },
        modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
    }
    local create = kit.button("Create", { type = "create_issue" }, {
        disabled = failed or string.match(draft, "^%s*$") ~= nil, help = "Open this issue on GitHub",
        label_modifiers = { { frame = { minWidth = 56 } } },
    })
    local new = kit.box("h", { field, create }, { spacing = 8, widths = { "flex", false }, padding = { 0, 0, 12, 0 } })

    local shown = list.sorted(st.list, SPEC, issues, compare, tie)
    local ids = {}
    for i, it in shown do ids[i] = tostring(it.number) end
    local took = list.taken(ids, st.list)
    st.ids = took

    local labelled = list.counted(issues, function(i) return i.labels or NONE end)
    local assignees = list.counted(issues, function(i)
        if #(i.assignees or NONE) == 0 then return { NOBODY } end
        return i.assignees
    end)
    local options = {
        { { "open", "open  " .. count("open") }, { "closed", "closed  " .. count("closed") } },
        labelled,
        assignees,
        list.counted(issues, function(i) return { i.author or "" } end),
    }
    local actions = list.actions(snap.scripts or NONE, took, #shown, "issue numbers", "issues", failed)

    local cut = {}
    if #shown > MAX_ROWS then
        table.insert(cut, kit.label("… " .. (#shown - MAX_ROWS) .. " more not shown. Narrow the list with the search or filters.", {
            tone = "muted", size = 12,
        }))
    end

    local children = {
        list.boxes(st.list, {
            { "Open", "state", "open", count("open"), true }, { "Closed", "state", "closed", count("closed"), false },
        }),
        status,
        new,
        list.find(st.list, {}),
        list.bar(SPEC, st.list, options, actions),
        kit.box("v", { issues_table(st, snap, shown, now) }, {}),
        ui.Column{ spacing = 0, table.unpack(cut) },
        -- A closed panel is a Modal that is not presented, so the open one is replaced rather than left behind.
        if open then modal(st, open) else ui.Modal{ presented = false, dismissAction = { type = "close_detail" } },
    }
    return ui.Column{ align = "stretch", kit.box("v", children, { align = "start", padding = { 18, 16, 48, 16 }, spacing = 0 }) }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or { list = list.new("updated", false), sel = nil, draft = "", said = nil, said_bad = false }
end

local function update(state, msg)
    if msg.action then
        local failed = msg.error ~= nil
        local text = if failed then msg.error else (type(msg.result) == "table" and msg.result.said) or (type(msg.result) == "string" and msg.result) or "Done."
        if msg.action == "wheelhouse.issue_create" then
            state.said, state.said_bad = text, failed
            -- Clear the draft only when it is still the title that was sent; one typed since stays.
            if not failed and string.match(state.draft or "", "^%s*(.-)%s*$") == state.creating then state.draft = "" end
            state.creating = nil
        elseif msg.action == "wheelhouse.issue_close" then
            state.said, state.said_bad = text, failed
            -- Dismiss the modal only when it still shows the issue that was closed.
            if not failed and state.sel == state.closing then state.sel = nil end
            state.closing = nil
        elseif msg.action == "wheelhouse.run_list" then
            state.said, state.said_bad = text, failed
        end
        return
    end
    local t = msg.type
    if t == "set_draft" then
        state.draft = if type(msg.value) == "string" then msg.value else ""
        -- Typing a new title takes down a failure notice.
        if state.said_bad then state.said, state.said_bad = nil, false end
        return
    end
    -- The last result shows until the next message.
    state.said, state.said_bad = nil, false
    if list.handle(state.list, SPEC, msg) then return end
    local dir = state.dir
    if t == "select_issue" then
        local n = tonumber(msg.id)
        if state.sel == n then state.sel = nil else state.sel = n end
    elseif t == "close_detail" then
        state.sel = nil
    elseif not dir then
        return
    elseif t == "create_issue" then
        local title = string.match(state.draft or "", "^%s*(.-)%s*$")
        state.creating = title
        wheel.action("wheelhouse.issue_create", { dir = dir, title = title })
    elseif t == "close_issue" and state.sel then
        state.closing = state.sel
        wheel.action("wheelhouse.issue_close", { dir = dir, number = math.floor(state.sel) })
    elseif t == "list.run" then
        if msg.script then
            wheel.action("wheelhouse.run_list", {
                dir = dir, script = msg.script, ids = state.ids or {}, view = "issues", pane = state.pane or "",
            })
        end
    end
end

local function view(state, data)
    local fresh = data.feed("wheelhouse.issues", { dir = data.cwd })
    local snap = fresh
    if fresh ~= nil then LAST[data.cwd] = fresh else snap = LAST[data.cwd] end
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading issues…" } }
    end
    return page(state, data, snap)
end

return { init = init, update = update, view = view }
