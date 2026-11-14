-- The Repo pane: a project's branches and worktrees as a sortable table with filters and status boxes, a History tab of
-- stashes and commits, and deleting a dead branch.
--
-- `wheelhouse.repo` (the backend's feed, see wheel-plugin.toml) holds the branches, stashes and commits. The pane's own
-- state (tab, search and filters, sort, selection, the last result) lives in `state`.
local kit = require("lib/ui")
local list = require("lib/list")
local tm = require("lib/time")
local sorting = require("lib/sort")

local NONE = {}

-- ───────────────────────────── the branches ─────────────────────────────

-- "dead" (safe to delete), "blocked" (dead but something holds it) or "live".
local function kinds(b)
    if b.dead then
        if b.blocked then return "blocked" end
        return "dead"
    end
    return "live"
end

local function ci(b)
    local c = b.pr and b.pr.ci
    if c == "pass" then return "passing" end
    if c == "fail" then return "failing" end
    if c ~= nil then return "pending" end
    return "none"
end

local function changed(b)
    local w = b.worktree
    if not w then return false end
    local s = w.status or NONE
    return (s.staged or 0) + (s.unstaged or 0) + (s.untracked or 0) + (s.conflicted or 0) > 0
end

local function leaf(path)
    return string.match(path, "([^/]*)$") or path
end

local SPEC = {
    filters = {
        { key = "state", name = "state" },
        { key = "has", name = "has" },
        { key = "checks", name = "checks" },
    },
    sorts = { "branch", "worktree", "vs", "pr", "ci", "state", "updated" },
    has = function(b, key, value)
        if key == "state" then return b._kind == value end
        if key == "has" then
            if value == "worktree" then return b.worktree ~= nil end
            if value == "pr" then return b.pr ~= nil end
            if value == "changes" then return changed(b) end
            if value == "local-only" then return (b.local_only or 0) > 0 end
            return false
        end
        if key == "checks" then return b._ci == value end
        return false
    end,
    haystack = function(b)
        return b.branch .. " " .. (b.subject or "") .. " " .. (b.pr and b.pr.title or "")
    end,
}

local CI_RANK = { failing = 0, pending = 1, passing = 2, none = 3 }
local GROUP = { dead = 0, blocked = 1, live = 2 }

local function compare(key, a, b)
    if key == "branch" then return sorting.cmp(string.lower(a.branch), string.lower(b.branch)) end
    if key == "worktree" then return sorting.cmp(a._wt, b._wt) end
    if key == "vs" then return sorting.cmp(a.ahead or -1, b.ahead or -1) end
    if key == "pr" then return sorting.cmp(a.pr and a.pr.number or 0, b.pr and b.pr.number or 0) end
    if key == "ci" then return sorting.cmp(CI_RANK[a._ci], CI_RANK[b._ci]) end
    if key == "state" then return sorting.cmp(GROUP[a._kind], GROUP[b._kind]) end
    return sorting.cmp(a.ts or 0, b.ts or 0)
end

-- What the sort leaves equal: newest first, then by name.
local function tie(a, b)
    local o = sorting.cmp(b.ts or 0, a.ts or 0)
    if o ~= 0 then return o end
    return sorting.cmp(a.branch, b.branch)
end

-- Fields the comparisons and filters read, set once per render.
local function prepare(snap)
    for _, b in snap.branches or NONE do
        b._kind = kinds(b)
        b._ci = ci(b)
        b._wt = if b.worktree then string.lower(leaf(b.worktree.path)) else ""
    end
end

-- ───────────────────────────── the table ─────────────────────────────

local function branch_row(st, b, now)
    local wt = ""
    if b.worktree then
        wt = leaf(b.worktree.path)
        if changed(b) then wt = wt .. " (changes)" end
    end
    local vs = ""
    if b.ahead ~= nil and b.behind ~= nil then vs = "+" .. b.ahead .. " -" .. b.behind end
    local ci_cell
    if b._ci == "passing" then
        ci_cell = kit.cell("passing", { chip = true, tone = "ok", sort = 2 })
    elseif b._ci == "failing" then
        ci_cell = kit.cell("failing", { chip = true, tone = "bad", sort = 0 })
    elseif b._ci == "pending" then
        ci_cell = kit.cell("pending", { chip = true, tone = "warn", sort = 1 })
    else
        ci_cell = kit.cell("", { sort = 3 })
    end
    local state_cell
    if b._kind == "dead" then
        state_cell = kit.cell(b.dead, { chip = true, tone = "ok" })
    elseif b._kind == "blocked" then
        state_cell = kit.cell(b.blocked, { tone = "warn" })
    else
        local n = b.local_only or 0
        state_cell = kit.cell(if n > 0 then n .. " local-only" else "", { tone = "muted" })
    end
    local cells = {
        kit.cell(b.branch, { key = "branch", mono = true, sort = string.lower(b.branch) }),
        kit.cell(wt, { key = "worktree", tone = "muted", sort = b._wt }),
        kit.cell(vs, { key = "vs", mono = true, tone = "muted", sort = b.ahead or -1 }),
        kit.cell(if b.pr then "#" .. b.pr.number else "", { key = "pr", mono = true, sort = b.pr and b.pr.number or 0 }),
        ci_cell,
        state_cell,
        kit.cell(tm.ago_ts(b.ts, now), { key = "updated", tone = "muted", sort = b.ts or 0 }),
    }
    ci_cell.key = "ci"
    state_cell.key = "state"
    local row = { id = b.branch, path = b.branch, depth = 0, cells = cells, menu = {} }
    if st.sel == b.branch then row.sel = true end
    if b._kind == "dead" then
        row.menu[1] = { label = "Delete branch", action = { type = "delete_branch", branch = b.branch } }
    end
    return row
end

local function branches_table(st, snap, shown, now)
    local rows = {}
    for i, b in shown do rows[i] = branch_row(st, b, now) end
    local columns = {
        kit.column("branch", "Branch", "flex", false, false, true),
        kit.column("worktree", "Worktree", 180, false, false, true),
        kit.column("vs", "vs " .. (snap.default or "default"), 90, false, false, true),
        kit.column("pr", "PR", 60, false, false, true),
        kit.column("ci", "Checks", 84, false, false, true),
        kit.column("state", "State", 150, false, false, true),
        kit.column("updated", "Updated", 70, false, false, true),
    }
    local selected = {}
    for id in st.list.selected do table.insert(selected, id) end
    table.sort(selected)
    return {
        component = "Table", columns = columns,
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "select_branch" }, foldAction = { type = "select_branch" },
        sortAction = { type = "list.sort" }, selectAction = { type = "list.select" }, selected = selected,
        sort = { key = st.list.sort, asc = st.list.asc },
        empty = if #(snap.branches or NONE) == 0 then "No branches besides the default." else "Nothing matches the search and filters.",
    }
end

local function words_match(q, hay)
    hay = string.lower(hay)
    for w in string.gmatch(string.lower(q), "%S+") do
        if not string.find(hay, w, 1, true) then return false end
    end
    return true
end

local function history_table(st, snap, now)
    local q = st.list.q
    local stash_rows, commit_rows = {}, {}
    for _, s in snap.stashes or NONE do
        if words_match(q, s.ref .. " " .. s.subject) then
            table.insert(stash_rows, {
                id = s.ref, path = s.ref, depth = 0, menu = {}, sel = if st.sel == s.ref then true else nil,
                cells = {
                    kit.cell(s.ref, { key = "ref", mono = true }), kit.cell(s.subject, { key = "subject" }),
                    kit.cell("", { key = "author" }), kit.cell(s.when or "", { key = "when", tone = "muted" }),
                },
            })
        end
    end
    for _, c in snap.commits or NONE do
        if words_match(q, c.sha .. " " .. c.subject .. " " .. (c.author or "")) then
            table.insert(commit_rows, {
                id = c.sha, path = c.sha, depth = 0, menu = {}, sel = if st.sel == c.sha then true else nil,
                cells = {
                    kit.cell(c.sha, { key = "ref", mono = true }), kit.cell(c.subject, { key = "subject" }),
                    kit.cell(c.author or "", { key = "author", tone = "muted" }),
                    kit.cell(tm.ago_ts(c.ts, now), { key = "when", tone = "muted" }),
                },
            })
        end
    end
    local sections = {}
    if #stash_rows > 0 then table.insert(sections, { title = "Stashes", count = #stash_rows, rows = stash_rows }) end
    if #commit_rows > 0 then
        table.insert(sections, {
            title = "Commits on " .. (snap.default or "the default branch"), count = #commit_rows, rows = commit_rows,
        })
    end
    -- A stash's age is only text ("3 days ago"), so the When column keeps the page's order.
    local columns = {
        kit.column("ref", "Commit", 90, false, false, true),
        kit.column("subject", "Subject", "flex", false, false, true),
        kit.column("author", "Author", 140, false, false, true),
        kit.column("when", "When", 70, false, false, false),
    }
    return {
        component = "Table", columns = columns, sections = sections,
        rowAction = { type = "select_branch" }, foldAction = { type = "select_branch" },
        sort = { key = "when", asc = false },
        empty = if #(snap.stashes or NONE) == 0 and #(snap.commits or NONE) == 0 then "No stashes or commits." else "Nothing matches the search.",
    }
end

-- ───────────────────────────── the page ─────────────────────────────

local function problem_box(text, tone)
    local l = kit.label(text, { tone = tone })
    return kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 }, margin = { 10, 0, 0, 0 } })
end

local TABS = { { "branches", "Branches" }, { "history", "History" } }

local function tab_picker(st)
    local function picker(style, mods)
        local options = {}
        for _, v in TABS do table.insert(options, { label = v[2], value = v[1] }) end
        return ui.ChoicePicker{
            variant = "mutuallyExclusive", displayStyle = style, options = options, value = { st.tab },
            on_change = { type = "set_tab" }, modifiers = mods,
        }
    end
    return ui.ViewThatFits{
        ["in"] = "horizontal",
        picker("chips", { { fixedSize = { horizontal = true } } }),
        picker("menu", { { frame = { maxWidth = "infinity", alignment = "leading" } } }),
    }
end

local function counted(branches, key, values)
    local out = {}
    for _, v in values do
        local n = 0
        for _, b in branches do
            if SPEC.has(b, key, v[1]) then n += 1 end
        end
        table.insert(out, { v[1], v[2] .. "  " .. n })
    end
    return out
end

local function page(st, data, snap)
    local now = snap.now or 0
    local branches = snap.branches or NONE
    prepare(snap)
    local function n(key, v)
        local c = 0
        for _, b in branches do
            if SPEC.has(b, key, v) then c += 1 end
        end
        return c
    end

    local probs = {}
    if snap.error then table.insert(probs, problem_box(snap.error, "bad")) end
    if snap.pr_error then table.insert(probs, problem_box(snap.pr_error, "bad")) end
    local said = {}
    if st.said then table.insert(said, problem_box(st.said, if st.said_bad then "bad" else "ok")) end
    local top = kit.box("v", {
        list.boxes(st.list, {
            { "Safe to delete", "state", "dead", n("state", "dead"), true },
            { "Blocked", "state", "blocked", n("state", "blocked"), false },
            { "Worktrees", "has", "worktree", n("has", "worktree"), false },
            { "With a PR", "has", "pr", n("has", "pr"), false },
        }),
        ui.Column{ spacing = 0, table.unpack(probs) },
        ui.Column{ spacing = 0, table.unpack(said) },
    }, { spacing = 0 })

    local children = { top }
    if not snap.error then
        table.insert(children, list.find(st.list, { tab_picker(st) }))
        local shown = list.sorted(st.list, SPEC, branches, compare, tie)
        local ids = {}
        for i, b in shown do ids[i] = b.branch end
        local took = list.taken(ids, st.list)
        st.dir = data.cwd
        st.ids = took
        local options = {
            counted(branches, "state", { { "dead", "safe to delete" }, { "blocked", "blocked" }, { "live", "live" } }),
            counted(branches, "has", {
                { "worktree", "worktree" }, { "pr", "PR" }, { "changes", "uncommitted changes" }, { "local-only", "local-only commits" },
            }),
            counted(branches, "checks", { { "failing", "failing" }, { "pending", "pending" }, { "passing", "passing" }, { "none", "none" } }),
        }
        local actions = list.actions(snap.scripts or NONE, took, #shown, "branch names", "branches")
        table.insert(children, list.bar(SPEC, st.list, options, actions))
        local t = if st.tab == "history" then history_table(st, snap, now) else branches_table(st, snap, shown, now)
        table.insert(children, kit.box("v", { t }, {}))
    end
    return ui.Column{ align = "stretch", kit.box("v", children, { align = "start", padding = { 18, 16, 48, 16 } }) }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    local state = previous or { tab = "branches", list = list.new("state", true), sel = nil, said = nil, said_bad = false }
    return state
end

local function update(state, msg)
    if msg.action then
        local failed = msg.error ~= nil
        local words = if failed then msg.error else (type(msg.result) == "table" and msg.result.said) or "Done."
        if msg.action == "wheelhouse.branch_delete" or msg.action == "wheelhouse.run_list" then
            state.said, state.said_bad = words, failed
        end
        return
    end
    -- The last result shows once.
    state.said, state.said_bad = nil, false
    if list.handle(state.list, SPEC, msg) then return end
    local t = msg.type
    if t == "set_tab" then
        local v = if type(msg.value) == "table" then msg.value[1] else msg.value
        if v == "branches" or v == "history" then state.tab = v end
    elseif t == "select_branch" then
        if state.sel == msg.id then state.sel = nil else state.sel = msg.id end
    elseif t == "delete_branch" then
        if state.dir and msg.branch then
            wheel.action("wheelhouse.branch_delete", { dir = state.dir, branch = msg.branch })
        end
    elseif t == "list.run" then
        if state.dir and msg.script then
            wheel.action("wheelhouse.run_list", { dir = state.dir, script = msg.script, ids = state.ids or {}, view = "repo" })
        end
    end
end

local function view(state, data)
    local snap = data.feed("wheelhouse.repo", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the repository…" } }
    end
    return page(state, data, snap)
end

return { init = init, update = update, view = view }
