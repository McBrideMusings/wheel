-- The PRs pane: a project's open pull requests in four inbox boxes, a sortable table with filters, and a detail
-- Modal for the selected PR with merge, close, draft, checkout and worktree buttons.
--
-- `wheelhouse.prs` holds the list (and the project's buttons and list scripts); `wheelhouse.pr_detail` holds the open
-- PR in full. The pane's own state (search and filters, sort, selection, the open PR, the merge choices, the last
-- result) lives in `state`. `update` cannot read feeds, so `view` stores the directory and the listed ids in `state`.
local kit = require("lib/ui")
local list = require("lib/list")
local tm = require("lib/time")
local sorting = require("lib/sort")

local NONE = {}

-- The most table rows, blocks, conversation entries and check rows drawn: the runtime refuses a view tree over
-- 100,000 values, and a Table row costs about 41.
local MAX_ROWS = 1000
local MAX_BLOCKS = 300
local MAX_TALK = 40
local MAX_CHECKS = 40

-- ───────────────────────────── what the list says ─────────────────────────────

local BUCKETS = {
    { "needs-me", "Needs me" }, { "mine", "Mine" }, { "waiting", "Waiting on others" }, { "other", "Other" },
}

-- Where a review stands for the viewer: key, words, tone, whether it is the viewer's turn. The order is the Review sort.
local STANDINGS = {
    { "review", "Review requested", "warn", true },
    { "re-review", "New commits to review", "warn", true },
    { "reply", "Replied to you", "warn", true },
    { "changes-requested", "Changes requested", "bad", true },
    { "comments", "Comments to answer", "bad", true },
    { "no-reviewer", "No reviewer", "warn", false },
    { "waiting-reviewer", "Waiting on reviewer", "muted", false },
    { "waiting-author", "Waiting on author", "muted", false },
    { "not-asked", "Not asked", "muted", false },
    { "approved", "Approved", "ok", false },
}

local function standing(p)
    for i, s in STANDINGS do
        if s[1] == p.standing then return i, s end
    end
    return #STANDINGS, STANDINGS[#STANDINGS]
end

local CHECKS_RANK = { failing = 0, pending = 1, passing = 2, none = 3 }
local CHECKS_TONE = { passing = "ok", failing = "bad", pending = "warn" }

local SPEC = {
    filters = {
        { key = "inbox", name = "inbox" },
        { key = "review", name = "review" },
        { key = "draft", name = "draft" },
        { key = "checks", name = "checks" },
        { key = "author", name = "author" },
    },
    sorts = { "number", "title", "draft", "review", "checks", "branch", "author", "updated" },
    has = function(p, key, value)
        if key == "inbox" then return p.bucket == value end
        if key == "review" then return p.standing == value end
        if key == "draft" then return (if p.draft then "draft" else "ready") == value end
        if key == "checks" then return p.checks == value end
        if key == "author" then return p.author == value end
        return false
    end,
    haystack = function(p)
        return "#" .. p.number .. " " .. p.title .. " " .. p.author .. " " .. p.branch .. " " .. table.concat(p.labels or NONE, " ")
    end,
}

local function compare(key, a, b)
    if key == "number" then return sorting.cmp(a.number, b.number) end
    if key == "title" then return sorting.cmp(string.lower(a.title), string.lower(b.title)) end
    if key == "draft" then return sorting.cmp(if a.draft then 0 else 1, if b.draft then 0 else 1) end
    if key == "review" then return sorting.cmp((standing(a)), (standing(b))) end
    if key == "checks" then return sorting.cmp(CHECKS_RANK[a.checks] or 3, CHECKS_RANK[b.checks] or 3) end
    if key == "branch" then return sorting.cmp(string.lower(a.branch), string.lower(b.branch)) end
    if key == "author" then return sorting.cmp(string.lower(a.author), string.lower(b.author)) end
    return sorting.cmp(a.updated or 0, b.updated or 0)
end

local function tie(a, b)
    return sorting.cmp(a.number, b.number)
end

-- Whether a project button wants this PR: any of its `on` entries matches.
local function wants(button, pr)
    for _, on in button.on or NONE do
        if on == "any" then return true end
        if on == "fix" then
            if pr.fix then return true end
        elseif on == "draft" then
            if pr.draft then return true end
        elseif on == "ready" then
            if not pr.draft and pr.mergeable == "MERGEABLE" then return true end
        elseif pr.bucket == on then
            return true
        end
    end
    return false
end

-- ───────────────────────────── the table ─────────────────────────────

local function pr_row(st, p, now)
    local rank, s = standing(p)
    local review = kit.cell(s[2], { key = "review", chip = true, tone = s[3], weight = if s[4] then "semibold" else "regular", sort = rank })
    local cells = {
        kit.cell("#" .. p.number, { key = "number", mono = true, tone = "muted", sort = p.number }),
        kit.cell(p.title, { key = "title", tone = if p.fix then "bad" else "fg", sort = string.lower(p.title) }),
        kit.cell(if p.draft then "draft" else "ready", {
            key = "draft", chip = p.draft, tone = if p.draft then "warn" else "muted", sort = if p.draft then 0 else 1,
        }),
        review,
        kit.cell(if p.checks == "none" then "" else p.checks, {
            key = "checks", chip = p.checks ~= "none", tone = CHECKS_TONE[p.checks] or "muted", sort = CHECKS_RANK[p.checks] or 3,
        }),
        kit.cell(p.branch, { key = "branch", mono = true, size = 12, tone = "muted", sort = string.lower(p.branch) }),
        kit.cell(p.author, { key = "author", tone = "muted", sort = string.lower(p.author) }),
        kit.cell(tm.ago_ts(p.updated, now), { key = "updated", tone = "muted", sort = p.updated or 0 }),
    }
    local id = tostring(p.number)
    local row = { id = id, path = id, depth = 0, kids = 0, collapsed = false, cells = cells, menu = {} }
    if p.draft then row.dim = true end
    if st.sel == p.number then row.sel = true end
    return row
end

local function prs_table(st, snap, shown, now)
    local rows = {}
    for i, p in shown do
        if i > MAX_ROWS then break end
        rows[i] = pr_row(st, p, now)
    end
    local columns = {
        kit.column("number", "#", 52, false, false, true),
        kit.column("title", "Title", "flex", false, false, true),
        kit.column("draft", "Draft", 56, false, true, true),
        kit.column("review", "Review", 160, false, false, true),
        kit.column("checks", "Checks", 72, false, true, true),
        kit.column("branch", "Branch", 140, false, true, true),
        kit.column("author", "Author", 90, false, true, true),
        kit.column("updated", "Updated", 64, false, true, true),
    }
    local selected = {}
    for id in st.list.selected do table.insert(selected, id) end
    table.sort(selected)
    return {
        component = "Table", columns = columns,
        sections = { { title = "", count = #rows, rows = rows } },
        rowAction = { type = "select_pr" }, foldAction = { type = "select_pr" },
        sortAction = { type = "list.sort" }, selectAction = { type = "list.select" }, selected = selected,
        sort = { key = st.list.sort, asc = st.list.asc },
        empty = if #snap.prs == 0 then "No open PRs." else "Nothing matches the search and filters.",
    }
end

-- ───────────────────────────── the detail ─────────────────────────────

local function tone_of_state(s)
    if s == "APPROVED" or s == "SUCCESS" or s == "MERGED" or s == "COMPLETED" then return "ok" end
    if s == "CHANGES_REQUESTED" or s == "FAILURE" or s == "ERROR" or s == "CLOSED" or s == "TIMED_OUT" then return "bad" end
    if s == "PENDING" or s == "IN_PROGRESS" or s == "QUEUED" or s == "EXPECTED" then return "warn" end
    return "muted"
end

local function merge_tone(s)
    if s == "CLEAN" or s == "HAS_HOOKS" then return "ok" end
    if s == "DIRTY" or s == "BLOCKED" then return "bad" end
    if s == "BEHIND" or s == "UNSTABLE" then return "warn" end
    return "muted"
end

local function words(s)
    return (string.gsub(string.lower(s or ""), "_", " "))
end

local function fact(name, value)
    local l = kit.label(name, { tone = "muted", size = 12 })
    return kit.box("h", { l, value }, { spacing = 12, widths = { 84, "flex" }, align = "firstTextBaseline" })
end

local function or_none(items, sep)
    if #items == 0 then return kit.label("none", { tone = "muted" }) end
    return kit.label(table.concat(items, sep), {})
end

local function heading(title, count)
    return kit.label("", { runs = { { text = title, weight = "semibold", size = 13 }, { text = "  " .. count, tone = "muted", size = 13 } } })
end

local function ago_words(ts, now)
    local a = tm.ago_ts(ts, now)
    if a == "now" or a == "" then return "just now" end
    return a .. " ago"
end

local HEADINGS = {
    { size = 20, weight = "bold" }, { size = 17, weight = "bold" }, { size = 15, weight = "semibold" }, { size = 14, weight = "semibold" },
}

-- One description or comment block: a Text with inline Markdown, an Image, a code box or a rule.
local function block(b)
    local indent = b.indent or 0
    local node
    if b.kind == "image" then
        node = ui.Image{ url = b.url, fit = "contain", modifiers = {
            { frame = { maxWidth = "infinity", maxHeight = 360, alignment = "leading" } }, { help = b.alt or "" },
        } }
    elseif b.kind == "code" then
        return kit.box("v", { kit.label(b.text or "", { mono = true, size = 12 }) }, {
            padding = { 8, 10 }, fill = "soft", radius = 6, align = "start", margin = if indent > 0 then { 0, indent, 0, 0 } else nil,
        })
    elseif b.kind == "rule" then
        return ui.Divider{}
    else
        local h = b.heading or 0
        local e = { markdown = true }
        if h >= 1 and h <= 4 then
            e.size, e.weight = HEADINGS[h].size, HEADINGS[h].weight
        end
        if b.quote then e.tone = "muted" end
        node = kit.label(b.text or "", e)
        if h > 0 then kit.modify(node, { padding = { 6, 0, 0, 0 } }) end
    end
    if indent > 0 then kit.modify(node, { padding = { 0, indent, 0, 0 } }) end
    return node
end

-- Blocks as a Column, at most `budget.n` of them across the whole modal, then a note naming what was cut.
local function blocks(list_of, budget)
    local out = {}
    for i, b in list_of do
        if budget.n <= 0 then
            table.insert(out, kit.label("… " .. (#list_of - i + 1) .. " more blocks not shown", { tone = "muted", size = 12 }))
            break
        end
        budget.n -= 1
        table.insert(out, block(b))
    end
    return kit.box("v", out, { spacing = 8, align = "start" })
end

-- Merge, Close PR and Confirm share one label width, so the footer does not shift when a button asks for confirmation.
local CONFIRM_WIDTH = { { frame = { minWidth = 64 } } }

local METHODS = { { "squash", "Squash" }, { "merge", "Merge commit" }, { "rebase", "Rebase" } }
local PLACES = { { "pane", "This space" }, { "space", "New space" } }

local function choice(id, options, value, style, on_change, mods)
    local o = {}
    for _, v in options do table.insert(o, { label = v[2], value = v[1] }) end
    return ui.ChoicePicker{
        variant = "mutuallyExclusive", displayStyle = style, options = o, value = { value },
        on_change = on_change, modifiers = mods,
    }
end

-- The loaded sections of the modal body (everything below the facts every state shows).
local function loaded_sections(st, pr, d, now)
    local c = {}
    table.insert(c, fact("Assignee", or_none(d.assignees or NONE, ", ")))
    table.insert(c, fact("Reviewers", or_none(d.reviewers or NONE, ", ")))
    table.insert(c, fact("Labels", or_none(d.labels or NONE, "  ·  ")))
    table.insert(c, fact("Size", kit.label(
        pr.commits .. " commits · " .. pr.files .. " files · +" .. pr.additions .. " −" .. pr.deletions, { tabular = true })))
    table.insert(c, fact("Dates", kit.label("opened " .. ago_words(d.created, now)
        .. " · updated " .. ago_words(d.updated, now), {})))
    table.insert(c, fact("GitHub", ui.Link{ title = pr.url, destination = pr.url }))

    local agents = { { "default", "Default agent" } }
    for _, a in d.agents or NONE do table.insert(agents, { a, a }) end
    local function key(text)
        return kit.label(text, { tone = "muted", size = 12 })
    end
    table.insert(c, kit.box("h", {
        key("Merge with"), choice("method", METHODS, st.method, "chips", { type = "set_method" }, { { fixedSize = { horizontal = true } } }),
        key("Open with"), choice("agent", agents, st.agent, "menu", { type = "set_agent" }, { { fixedSize = true } }),
        key("Open in"), choice("open", PLACES, st.open, "chips", { type = "set_open" }, { { fixedSize = { horizontal = true } } }),
    }, { spacing = 8, align = "center", padding = { 4, 0, 0, 0 } }))

    local budget = { n = MAX_BLOCKS }
    local desc
    if d.body and #d.body > 0 then
        desc = blocks(d.body, budget)
    else
        desc = kit.label("No description.", { tone = "muted" })
    end
    table.insert(c, kit.box("v", { desc }, { padding = { 10, 12 }, fill = "soft", radius = 8, align = "start" }))

    local checks = d.checks or NONE
    local ck = {}
    if #checks > 0 then
        table.insert(ck, heading("Checks", #checks))
        for i, k in checks do
            if i > MAX_CHECKS then
                table.insert(ck, kit.label("… " .. (#checks - MAX_CHECKS) .. " more checks", { tone = "muted", size = 12 }))
                break
            end
            local chip = kit.label(words(k.state), { chip = true, size = 11, tone = tone_of_state(k.state) })
            local name = kit.label(k.name, { size = 12 })
            kit.modify(name, { lineLimit = 1 })
            table.insert(ck, kit.box("h", { chip, name }, { spacing = 8, align = "center", widths = { 90, "flex" } }))
        end
    end
    table.insert(c, kit.box("v", ck, { spacing = 3, padding = { 8, 0, 0, 0 } }))

    local talk = d.talk or NONE
    local cm = {}
    if #talk > 0 then
        table.insert(cm, heading("Conversation", #talk))
        local first = math.max(1, #talk - MAX_TALK + 1)
        if first > 1 then
            table.insert(cm, kit.label("… " .. (first - 1) .. " earlier entries not shown", { tone = "muted", size = 12 }))
        end
        for i = first, #talk do
            local t = talk[i]
            local head = kit.label("", { runs = {
                { text = t.who, weight = "semibold", size = 12 },
                { text = "  " .. t.what .. "  ·  " .. ago_words(t.at, now), tone = "muted", size = 12 },
            } })
            table.insert(cm, kit.box("v", { head, blocks(t.blocks or NONE, budget) }, {
                spacing = 4, padding = { 8, 12 }, fill = "soft", radius = 8, align = "start",
            }))
        end
    end
    table.insert(c, kit.box("v", cm, { spacing = 6, padding = { 8, 0, 0, 0 }, align = "start" }))

    local commits = d.commits or NONE
    local hi = {}
    if #commits > 0 then
        local total = d.commits_total or #commits
        table.insert(hi, heading("Commits", total))
        for _, k in commits do
            local sha = kit.label(k.sha, { mono = true, size = 12, tone = "muted" })
            local msg = kit.label(k.headline, { size = 12 })
            kit.modify(msg, { lineLimit = 1 })
            local age = kit.label(tm.ago_ts(k.at, now), { size = 12, tone = "muted", tabular = true, align = "end" })
            table.insert(hi, kit.box("h", { sha, msg, age }, { spacing = 8, align = "center", widths = { 64, "flex", 56 } }))
        end
        if total > #commits then
            table.insert(hi, kit.label("… " .. (total - #commits) .. " older commits", { tone = "muted", size = 12 }))
        end
    end
    table.insert(c, kit.box("v", hi, { spacing = 3, padding = { 8, 0, 0, 0 } }))

    local files = d.files or NONE
    local fl = {}
    if #files > 0 then
        local total = d.files_total or #files
        table.insert(fl, heading("Files", total))
        for _, f in files do
            local path = kit.label(f.path, { mono = true, size = 12 })
            kit.modify(path, { lineLimit = 1 })
            local delta = kit.label("+" .. f.additions .. " −" .. f.deletions, { mono = true, size = 12, tone = "muted", tabular = true, align = "end" })
            table.insert(fl, kit.box("h", { path, delta }, { spacing = 8, align = "center", widths = { "flex", 96 } }))
        end
        if total > #files then
            table.insert(fl, kit.label("… " .. (total - #files) .. " more files", { tone = "muted", size = 12 }))
        end
    end
    table.insert(c, kit.box("v", fl, { spacing = 3, padding = { 8, 0, 0, 0 } }))
    return c
end

-- The open PR as a Modal: `pr` is its row of the list, `d` the pr_detail value (nil while loading).
local function modal(st, snap, pr, d, now)
    local loaded = d ~= nil and not d.error
    local state = if loaded then d.state else "OPEN"
    local draft = if loaded then d.draft == true else pr.draft
    local c = {}
    table.insert(c, kit.label("#" .. pr.number .. " " .. pr.title, { size = 17, weight = "semibold" }))

    local chips = {
        kit.label(if draft then "draft" else words(state), {
            chip = true, size = 11, tone = if draft then "muted" else tone_of_state(state),
        }),
    }
    if pr.checks ~= "none" then
        table.insert(chips, kit.label("checks " .. pr.checks, { chip = true, size = 11, tone = CHECKS_TONE[pr.checks] or "warn" }))
    end
    local merge_state = if loaded then d.merge_state else nil
    table.insert(chips, kit.label(words(merge_state or "unknown"), { chip = true, size = 11, tone = merge_tone(merge_state) }))
    local _, s = standing(pr)
    table.insert(chips, kit.label(s[2], { chip = true, size = 11, tone = "muted" }))
    table.insert(c, kit.box("h", chips, { spacing = 6, align = "center" }))

    table.insert(c, fact("Branch", kit.label(pr.branch .. " → " .. (if loaded then d.base or "?" else "?"), { mono = true, size = 12 })))
    table.insert(c, fact("Author", kit.label(pr.author, {})))

    -- The loading slot: a spinner and the summary until the details arrive, or why they did not.
    local wait = {}
    if not loaded then
        if d and d.error then
            table.insert(wait, kit.label(d.error, { tone = "bad", size = 12 }))
        else
            table.insert(wait, kit.label("Loading details…", { symbol = "spinner", tone = "muted", size = 12 }))
        end
        if pr.summary and pr.summary ~= "" then
            table.insert(wait, kit.box("v", { kit.label(pr.summary, { tone = "muted" }) }, {
                padding = { 10, 12 }, fill = "soft", radius = 8, align = "start",
            }))
        end
    end
    table.insert(c, kit.box("v", wait, { spacing = 12, align = "start" }))
    table.insert(c, kit.box("v", if loaded then loaded_sections(st, pr, d, now) else {}, { spacing = 12, align = "start" }))
    local body = kit.box("v", c, { spacing = 12, align = "start" })

    -- The footer keeps every control in its place: one that does not apply is disabled, never missing.
    local footer = {}
    local ran = st.ran
    table.insert(footer, kit.label(if ran then ran.text else "", {
        tone = if ran and ran.bad then "bad" else "ok", weight = "semibold",
    }))
    local open = loaded and state == "OPEN"
    local mergeable = open and not draft and d.mergeable == "MERGEABLE"
    local mine = {}
    for _, b in snap.buttons or NONE do
        local on = wants(b, pr)
        local row = kit.box("h", { ui.Icon{ name = "terminal" }, ui.Text{ text = b.label or b.name } }, { spacing = 5, align = "center" })
        table.insert(mine, {
            component = "Button", child = row, action = { type = "pr_button", name = b.name },
            modifiers = {
                { help = (b.label or b.name) .. ": runs the command in wheel.toml [prs.buttons." .. b.name .. "]"
                    .. (if on then "" else " (does not apply to this PR)") },
                table.unpack(if on then NONE else { { disabled = true } }),
            },
        })
    end
    if #mine > 0 then table.insert(mine, ui.Divider{ axis = "vertical" }) end
    table.insert(footer, kit.box("h", mine, { spacing = 8, align = "center" }))
    table.insert(footer, kit.button("Terminal on branch", { type = "pr_checkout" }, {
        disabled = not loaded, help = "Switch this project's checkout to the PR's branch and open a terminal in a new pane",
    }))
    table.insert(footer, kit.button("Agent in worktree", { type = "pr_worktree" }, {
        disabled = not loaded,
        help = "Open the branch in its own worktree and start the agent chosen under Open with, in the place chosen under Open in",
    }))
    table.insert(footer, kit.button(if draft then "Mark ready" else "Convert to draft", { type = "pr_draft", draft = not draft }, {
        disabled = not open, help = if draft then "Take this PR out of draft" else "Put this PR back into draft",
    }))
    local closing, merging = st.confirm == "close", st.confirm == "merge"
    table.insert(footer, kit.button(if closing then "Confirm" else "Close PR", { type = if closing then "pr_close" else "ask_close" }, {
        disabled = not open, tone = "red", label_modifiers = CONFIRM_WIDTH,
        help = if closing then "Click again to close #" .. pr.number .. " without merging" else "Close this PR without merging",
    }))
    local merge_help = "Merge this PR with the method chosen under Merge with"
    if not mergeable then
        merge_help = "Not mergeable: closed, a draft, conflicting or not yet computed"
    elseif merging then
        merge_help = "Click again to merge #" .. pr.number .. " (" .. st.method .. ")"
    end
    table.insert(footer, kit.button(if merging then "Confirm" else "Merge", { type = if merging then "pr_merge" else "ask_merge" }, {
        style = "primary", tint = if merging then "green" else "tint", disabled = not mergeable, help = merge_help,
        label_modifiers = CONFIRM_WIDTH,
    }))
    return ui.Modal{ presented = true, dismissAction = { type = "close_pr" }, actions = footer, body }
end

-- ───────────────────────────── the page ─────────────────────────────

local function problem_box(text, tone)
    local l = kit.label(text, { tone = tone })
    return kit.box("v", { l }, { border = tone, radius = 8, padding = { 8, 12 }, margin = { 10, 0, 0, 0 } })
end

local function counted(prs, key, values)
    local out = {}
    for _, v in values do
        local n = 0
        for _, p in prs do
            if SPEC.has(p, key, v[1]) then n += 1 end
        end
        table.insert(out, { v[1], v[2] .. "  " .. n, n })
    end
    return out
end

local function page(st, data, snap)
    local now = snap.now or 0
    local prs = snap.prs or NONE
    snap.prs = prs
    local function n(bucket)
        local c = 0
        for _, p in prs do
            if p.bucket == bucket then c += 1 end
        end
        return c
    end

    -- A selected PR that left the list (merged or closed, here or elsewhere) closes the modal; what the last action said
    -- moves to the page's box, since the modal's footer is gone with it.
    local open
    for _, p in prs do
        if p.number == st.sel then open = p end
    end
    if st.sel and not open and snap.repo then
        if st.ran then st.said, st.said_bad = st.ran.text, st.ran.bad end
        st.sel, st.ran, st.confirm = nil, nil, nil
    end

    local cards = {}
    for _, b in BUCKETS do table.insert(cards, { b[2], "inbox", b[1], n(b[1]), b[1] == "needs-me" }) end
    local probs, said = {}, {}
    if snap.error then table.insert(probs, problem_box(snap.error, "bad")) end
    if st.said then table.insert(said, problem_box(st.said, if st.said_bad then "bad" else "ok")) end
    local top = kit.box("v", {
        list.boxes(st.list, cards),
        ui.Column{ spacing = 0, table.unpack(probs) },
        ui.Column{ spacing = 0, table.unpack(said) },
    }, { spacing = 0 })

    local children = { top }
    st.dir = data.cwd
    if snap.repo then
        local clock = ""
        if (snap.fetched or 0) > 0 then
            clock = "Updated " .. (snap.fetched_clock or "") .. " · next by " .. (snap.next_clock or "")
        end
        local status = kit.label(clock, { tone = "muted", size = 11, tabular = true })
        kit.modify(status, { lineLimit = 1 })
        local refresh = kit.button("Refresh", { type = "refresh" }, { icon = "arrow.clockwise", style = "plain", help = "Ask GitHub again now" })
        table.insert(children, list.find(st.list, { status, refresh }))

        local shown = list.sorted(st.list, SPEC, prs, compare, tie)
        local ids = {}
        for i, p in shown do ids[i] = tostring(p.number) end
        local took = list.taken(ids, st.list)
        st.ids = took

        local review = {}
        for _, o in counted(prs, "review", (function()
            local v = {}
            for _, s in STANDINGS do table.insert(v, { s[1], s[2] }) end
            return v
        end)()) do
            if o[3] > 0 then table.insert(review, { o[1], o[2] }) end
        end
        local options = {
            counted(prs, "inbox", BUCKETS),
            review,
            counted(prs, "draft", { { "draft", "draft" }, { "ready", "ready" } }),
            counted(prs, "checks", { { "failing", "failing" }, { "pending", "pending" }, { "passing", "passing" }, { "none", "none" } }),
            list.counted(prs, function(p) return { p.author } end),
        }
        local actions = list.actions(snap.scripts or NONE, took, #shown, "PR numbers", "PRs")
        table.insert(children, list.bar(SPEC, st.list, options, actions))
        table.insert(children, kit.box("v", { prs_table(st, snap, shown, now) }, {}))
        local cut = {}
        if #shown > MAX_ROWS then
            table.insert(cut, kit.label("… " .. (#shown - MAX_ROWS) .. " more not shown. Narrow the list with the search or filters.", {
                tone = "muted", size = 12,
            }))
        end
        table.insert(children, ui.Column{ spacing = 0, table.unpack(cut) })

        -- A closed panel is a Modal that is not presented, so the open one is replaced rather than left behind.
        if open then
            local d = data.feed("wheelhouse.pr_detail", { dir = data.cwd, number = math.floor(open.number) })
            table.insert(children, modal(st, snap, open, d, d and d.now or now))
        else
            table.insert(children, ui.Modal{ presented = false, dismissAction = { type = "close_pr" } })
        end
    end
    return ui.Column{ align = "stretch", kit.box("v", children, { align = "start", padding = { 18, 16, 48, 16 } }) }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or {
        list = list.new("updated", false), sel = nil, said = nil, said_bad = false,
        method = "squash", agent = "default", open = "pane", confirm = nil, ran = nil,
    }
end

local MODAL_ACTIONS = {
    ["wheelhouse.pr_merge"] = true, ["wheelhouse.pr_close"] = true, ["wheelhouse.pr_draft"] = true,
    ["wheelhouse.pr_checkout"] = true, ["wheelhouse.pr_worktree"] = true, ["wheelhouse.pr_button"] = true,
}

local function picked(msg)
    if type(msg.value) == "table" then return msg.value[1] end
    return msg.value
end

local function oneof(v, options)
    for _, o in options do
        if o[1] == v then return true end
    end
    return false
end

local function update(state, msg)
    if msg.action then
        local failed = msg.error ~= nil
        local text = if failed then msg.error else (type(msg.result) == "table" and msg.result.said) or "Done."
        if msg.action == "wheelhouse.run_list" then
            state.said, state.said_bad = text, failed
        elseif msg.action == "wheelhouse.pr_refresh" then
            if failed then state.said, state.said_bad = text, true end
        elseif MODAL_ACTIONS[msg.action] then
            state.ran = { text = text, bad = failed }
        end
        return
    end
    -- The last list result shows once, and a confirmation lasts only until the next message that is not its ask.
    state.said, state.said_bad = nil, false
    local t = msg.type
    if t ~= "ask_merge" and t ~= "ask_close" then state.confirm = nil end
    if list.handle(state.list, SPEC, msg) then return end
    local dir, number = state.dir, state.sel and math.floor(state.sel)
    if t == "select_pr" then
        local n = tonumber(msg.id)
        if state.sel == n then state.sel = nil else state.sel = n end
        state.ran = nil
    elseif t == "close_pr" then
        state.sel, state.ran = nil, nil
    elseif t == "set_method" then
        local v = picked(msg)
        if oneof(v, METHODS) then state.method = v end
    elseif t == "set_agent" then
        local v = picked(msg)
        if type(v) == "string" then state.agent = v end
    elseif t == "set_open" then
        local v = picked(msg)
        if oneof(v, PLACES) then state.open = v end
    elseif t == "ask_merge" then
        state.confirm = "merge"
    elseif t == "ask_close" then
        state.confirm = "close"
    elseif not dir then
        return
    elseif t == "refresh" then
        wheel.action("wheelhouse.pr_refresh", { dir = dir })
    elseif t == "list.run" then
        if msg.script then
            wheel.action("wheelhouse.run_list", { dir = dir, script = msg.script, ids = state.ids or {}, view = "prs" })
        end
    elseif number then
        state.ran = nil
        if t == "pr_merge" then
            wheel.action("wheelhouse.pr_merge", { dir = dir, number = number, method = state.method })
        elseif t == "pr_close" then
            wheel.action("wheelhouse.pr_close", { dir = dir, number = number })
        elseif t == "pr_draft" then
            wheel.action("wheelhouse.pr_draft", { dir = dir, number = number, draft = msg.draft == true })
        elseif t == "pr_checkout" then
            wheel.action("wheelhouse.pr_checkout", { dir = dir, number = number })
        elseif t == "pr_worktree" then
            wheel.action("wheelhouse.pr_worktree", { dir = dir, number = number, agent = state.agent, open = state.open })
        elseif t == "pr_button" and msg.name then
            wheel.action("wheelhouse.pr_button", { dir = dir, number = number, name = msg.name })
        end
    end
end

local function view(state, data)
    local snap = data.feed("wheelhouse.prs", { dir = data.cwd })
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading pull requests…" } }
    end
    return page(state, data, snap)
end

return { init = init, update = update, view = view }
