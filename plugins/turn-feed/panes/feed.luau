-- The Turn feed pane: one card per agent turn, newest first.
--
-- `turn-feed.posts` {space} holds the posts this space's agent panes made, so a second space on the same
-- directory has its own feed; `wheel.agents` says which pane still runs an agent. Each post is its own Card
-- drawn on the control background with a separator outline over the window background, headed by the agent
-- pane that wrote it. Chips at the top narrow the list to one agent pane; All is the default. Only a post that
-- asks a question has a reply field and Send. Each card's header has two icon buttons: show the agent's pane
-- (`turn-feed.focus`) and delete the post (`turn-feed.delete`). The search field keeps the posts whose text holds
-- every word typed. Times are the backend's `*_local` fields, the machine's own clock.
-- A question in a post that is not its agent's newest is drawn dimmed, with the line saying a newer post replaced it.
-- A pick or a reply calls `turn-feed.answer`, which records it and returns the text to send; the pane then
-- calls `wheel.agent.send` with it, then `turn-feed.delivered`. An answer stored with `delivered == false` can be
-- sent again from its Send button. All three answers arrive in `update` as `{action, result | error}` in the order
-- the calls were made, so each action keeps a queue of the post ids it was called for.
local NONE = {}

local COLOUR = { fg = "primary", muted = "secondary", ok = "green", warn = "orange", bad = "red" }

local ANSWER, SEND, DELIVERED, DELETE, FOCUS = "turn-feed.answer", "wheel.agent.send", "turn-feed.delivered", "turn-feed.delete", "turn-feed.focus"

-- The last value drawn per space: a read that answers nil draws this meanwhile, so the pane keeps its list.
local LAST_POSTS, LAST_AGENTS = {}, nil

-- ───────────────────────────── pieces ─────────────────────────────

-- One Text. `o`: size, weight, mono, tone, flex (fills the row), markdown.
local function text(body, o)
    o = o or {}
    local mods = {}
    if o.size or o.weight or o.mono then
        table.insert(mods, { font = { size = o.size or 13, weight = o.weight or "regular", design = if o.mono then "monospaced" else "default" } })
    end
    if o.tone and COLOUR[o.tone] then table.insert(mods, { foregroundStyle = COLOUR[o.tone] }) end
    if o.flex then table.insert(mods, { frame = { maxWidth = "infinity", alignment = "leading" } }) end
    return ui.Text{ text = body, markdown = o.markdown, modifiers = mods }
end

-- A Button labelled `label`; `help` is its hover text. `disabled` keeps it in place and deaf.
local function button(label, action, help, disabled)
    local mods = { { help = help }, { accessibilityLabel = help } }
    if disabled then table.insert(mods, { disabled = true }) end
    return ui.Button{ child = ui.Text{ text = label }, action = action, modifiers = mods }
end

-- A borderless Button showing SF Symbol `icon`; `help` is its hover text and accessibility label.
local function icon_button(icon, action, help, disabled)
    local mods = { { help = help }, { accessibilityLabel = help } }
    if disabled then table.insert(mods, { disabled = true }) end
    return ui.Button{ child = ui.Icon{ name = icon }, action = action, variant = "borderless", modifiers = mods }
end

local function row(children, spacing)
    return ui.Row{ spacing = spacing or 8, children = children, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
end

local function trimmed(s)
    return (string.gsub(string.gsub(s or "", "^%s+", ""), "%s+$", ""))
end

-- A local time the backend formatted (`3:13pm`, `Nov 16, 3:13pm`), or `?` when it sent none.
local function clock(local_at)
    return if type(local_at) == "string" then local_at else "?"
end

-- Every searchable text of a post, lower-cased in one string.
local function haystack(post)
    local parts = { post.title or "", post.body or "", post.question or "", post.page_markdown or "", "agent " .. tostring(post.agent_pane) }
    for _, o in post.options or {} do
        table.insert(parts, table.concat({ o.id or "", o.action or "", o.case or "", o.cost or "", o.checked or "" }, " "))
    end
    return string.lower(table.concat(parts, "\n"))
end

-- Whether the post holds every space-separated word of `query` (case-insensitive, plain text).
local function matches(post, query)
    local words = {}
    for w in string.gmatch(string.lower(query), "%S+") do table.insert(words, w) end
    if #words == 0 then return true end
    local hay = haystack(post)
    for _, w in words do
        if not string.find(hay, w, 1, true) then return false end
    end
    return true
end

-- ───────────────────────────── what a post shows ─────────────────────────────

-- Markdown drawn from the blocks the backend parsed (`wheel_plugin_sdk::markdown`), every component and size
-- chosen here (ADR-0049). An image fits the card and never grows past its own pixel width; a table or a code
-- block scrolls sideways in its own row; inline runs are Text with `markdown = true`.
local HEADING = { "h2", "h3", "h4", "h5", "h5", "h5" }

local blocks_view

local function block_view(b)
    local k = b.kind
    if k == "heading" then
        return ui.Text{ text = b.text, markdown = true, variant = HEADING[b.level] or "h5",
            modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
    elseif k == "paragraph" then
        return text(b.text, { markdown = true, flex = true })
    elseif k == "image" then
        return ui.Image{ url = b.url, modifiers = {
            { frame = { maxWidth = b.width or "infinity", alignment = "leading" } },
            { accessibilityLabel = b.alt or "" },
        } }
    elseif k == "code" or k == "html" then
        return ui.List{ direction = "horizontal", children = { text(b.text, { size = 12, mono = true }) }, modifiers = {
            { padding = 10 },
            { frame = { maxWidth = "infinity", alignment = "leading" } },
            { background = "separator" },
            { clipShape = { cornerRadius = 6 } },
        } }
    elseif k == "table" then
        local rows = {}
        local head = {}
        for _, cell in b.header or NONE do table.insert(head, text(cell, { markdown = true, weight = "semibold" })) end
        table.insert(rows, ui.GridRow{ children = head })
        table.insert(rows, ui.Divider{})
        for _, r in b.rows or NONE do
            local cells = {}
            for _, cell in r do table.insert(cells, text(cell, { markdown = true })) end
            table.insert(rows, ui.GridRow{ children = cells })
        end
        return ui.List{ direction = "horizontal", children = {
            ui.Grid{ alignment = "leading", horizontalSpacing = 16, verticalSpacing = 6, children = rows },
        }, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
    elseif k == "list" then
        local items = {}
        for n, item in b.items or NONE do
            local marker = if b.ordered then tostring((b.start or 1) + n - 1) .. "." else "•"
            table.insert(items, ui.Row{ spacing = 6, children = {
                text(marker, { tone = "muted" }),
                blocks_view(item, 4),
            }, modifiers = { { frame = { maxWidth = "infinity", alignment = "topLeading" } } } })
        end
        return ui.Column{ spacing = 4, children = items, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
    elseif k == "quote" then
        return ui.Column{ spacing = 0, children = { blocks_view(b.blocks or NONE, 6) }, modifiers = {
            { padding = { 0, 0, 0, 12 } },
            { foregroundStyle = "secondary" },
        } }
    elseif k == "rule" then
        return ui.Divider{}
    end
    return text(tostring(b.text or ""), { tone = "muted", flex = true })
end

blocks_view = function(blocks, spacing)
    local c = {}
    for _, b in blocks or NONE do table.insert(c, block_view(b)) end
    return ui.Column{ spacing = spacing or 10, children = c, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
end

-- The page a post names: the artifact's page inside the post (ADR-0047).
local function page_view(name)
    return ui.WebView{ artifact = name, height = 360 }
end

local function option_view(post, o, locked, dim)
    local label = o.id
    if o.pick then label = label .. " · my pick" end
    local head = row({
        text(label, { weight = "bold", tone = if o.pick then "ok" else nil, flex = true }),
        button("Pick " .. o.id, { type = "pick", post = post.post, option = o.id }, "Pick " .. o.id, locked),
    })
    return ui.Column{
        spacing = 4,
        children = {
            head,
            text(o.action, { weight = "medium", flex = true }),
            text("Case: " .. o.case, { size = 12, tone = "muted", flex = true }),
            text("Cost: " .. o.cost, { size = 12, tone = "muted", flex = true }),
            text("Checked: " .. o.checked, { size = 12, tone = "muted", flex = true }),
        },
        modifiers = {
            { padding = 10 },
            { frame = { maxWidth = "infinity", alignment = "leading" } },
            { overlay = { stroke = "separator", cornerRadius = 8 } },
            if dim then { opacity = 0.45 } else nil,
        },
    }
end

-- What the person answered, or why the post cannot be answered.
local function status_line(post, gone, superseded)
    local a = post.answer
    if type(a) == "table" then
        if a.delivered == false then return "Saved, but the agent did not get it", "warn" end
        if a.pick then return "Picked " .. a.pick .. " · " .. clock(a.at_local), "ok" end
        return "Replied · " .. clock(a.at_local) .. ": " .. tostring(a.reply), "ok"
    end
    if superseded then return "Replaced by a newer post from Agent " .. tostring(post.agent_pane) .. ", so this question is closed", "muted" end
    if gone then return "Agent pane " .. tostring(post.agent_pane) .. " is gone", "warn" end
    return "", "muted"
end

-- `latest` is true for the newest post of its agent pane: only that one can still be answered, since a later post
-- means the conversation moved on.
local function post_view(state, post, running, latest)
    local answered = type(post.answer) == "table"
    local gone = running ~= nil and not running[post.agent_pane]
    local locked = answered or gone or not latest
    local options = post.options or NONE
    local asks = post.question ~= nil or #options > 0
    -- A question a newer post of the same agent replaced: closed, and drawn dimmed so only open questions look open.
    local superseded = asks and not answered and not latest
    local undelivered = answered and post.answer.delivered == false and type(post.answer.text) == "string"
    local posted = clock(post.at_local)
    if post.updated_at then posted = posted .. " · edited " .. clock(post.updated_local) end
    local who = text("Agent " .. tostring(post.agent_pane), { size = 11, weight = "semibold", tone = "muted", flex = true })
    local c = {
        row({
            who,
            text(posted, { size = 11, tone = "muted" }),
            icon_button("arrow.up.forward.app", { type = "focus", post = post.post }, "Show Agent " .. tostring(post.agent_pane) .. "'s pane", gone),
            icon_button("trash", { type = "delete", post = post.post }, "Delete this post"),
        }),
        text(post.title, { weight = "bold", size = 16, flex = true }),
        ui.Divider{},
        blocks_view(post.body_blocks),
    }
    -- A markdown page is drawn as part of the post; any other page is the artifact's own page in a web view.
    if post.page_blocks then
        table.insert(c, ui.Divider{})
        table.insert(c, blocks_view(post.page_blocks))
    elseif post.page then
        table.insert(c, page_view(post.page))
    end
    if #options > 0 then
        local q = text(post.question or "", { weight = "semibold", flex = true })
        if superseded then q = ui.Column{ spacing = 0, children = { q }, modifiers = { { opacity = 0.45 } } } end
        table.insert(c, q)
        for _, o in options do table.insert(c, option_view(post, o, locked, superseded)) end
    end
    local line, tone = status_line(post, gone, superseded)
    local problem = state.problems[post.post]
    if problem then line, tone = problem, "bad" end
    if answered or (latest and not gone and asks) then
        if not answered then
            table.insert(c, ui.TextField{
                label = "Reply", value = state.drafts[post.post] or "", variant = "longText",
                on_change = { type = "draft", post = post.post },
                modifiers = {
                    { frame = { maxWidth = "infinity", minHeight = 56, alignment = "topLeading" } },
                    if locked then { disabled = true } else nil,
                },
            })
        end
        local send
        if undelivered then
            send = button("Send again", { type = "resend", post = post.post, agent = post.agent_pane, text = post.answer.text }, "Send the saved answer to the agent again", gone)
        elseif not answered then
            send = button("Send", { type = "send", post = post.post }, "Send the reply", locked)
        end
        table.insert(c, row(if send then { send, text(line, { size = 12, tone = tone, flex = true }) } else { text(line, { size = 12, tone = tone, flex = true }) }))
    elseif line ~= "" then
        table.insert(c, text(line, { size = 12, tone = tone, flex = true }))
    end
    -- minWidth 0 makes the card take the list's width, so a wide table inside one post cannot widen every card.
    -- `separator` is a see-through grey, so the card reads lighter than the page in dark mode and darker in light.
    local mods = {
        { padding = 14 },
        { frame = { minWidth = 0, maxWidth = "infinity", alignment = "leading" } },
        { background = "separator" },
        { clipShape = { cornerRadius = 10 } },
    }
    return ui.Column{ spacing = 10, children = c, modifiers = mods }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    local s = previous or { drafts = {}, problems = {}, agent = "all", inflight = {} }
    for _, a in { ANSWER, SEND, DELIVERED, DELETE, FOCUS } do s.inflight[a] = s.inflight[a] or {} end
    return s
end

local function push(state, action, post)
    table.insert(state.inflight[action], post)
end

local function pop(state, action)
    return table.remove(state.inflight[action], 1)
end

local function update(state, msg)
    if msg.action == ANSWER then
        local post = pop(state, ANSWER)
        if msg.error then
            if post then state.problems[post] = tostring(msg.error) end
            return
        end
        local r = msg.result or NONE
        if not r.agent_pane or not r.text then
            if post then state.problems[post] = "the answer came back without a pane to send it to" end
            return
        end
        post = r.post or post
        state.drafts[post] = nil
        push(state, SEND, post)
        wheel.action(SEND, { agent = r.agent_pane, text = r.text })
    elseif msg.action == SEND then
        local post = pop(state, SEND)
        if msg.error then
            if post then state.problems[post] = "Answer saved, but the agent did not get it: " .. tostring(msg.error) end
        elseif post then
            push(state, DELIVERED, post)
            wheel.action(DELIVERED, { post = post })
        end
    elseif msg.action == DELIVERED then
        local post = pop(state, DELIVERED)
        if msg.error and post then
            state.problems[post] = "Sent, but the answer could not be marked delivered: " .. tostring(msg.error)
        end
    elseif msg.action == DELETE then
        local post = pop(state, DELETE)
        if msg.error and post then state.problems[post] = "Could not delete: " .. tostring(msg.error) end
    elseif msg.action == FOCUS then
        local post = pop(state, FOCUS)
        if msg.error and post then state.problems[post] = "Could not show the agent's pane: " .. tostring(msg.error) end
    elseif msg.type == "focus" then
        state.problems[msg.post] = nil
        push(state, FOCUS, msg.post)
        wheel.action(FOCUS, { post = msg.post })
    elseif msg.type == "query" then
        state.query = if type(msg.value) == "string" then msg.value else ""
    elseif msg.type == "delete" then
        state.problems[msg.post] = nil
        push(state, DELETE, msg.post)
        wheel.action(DELETE, { post = msg.post })
    elseif msg.type == "resend" then
        state.problems[msg.post] = nil
        push(state, SEND, msg.post)
        wheel.action(SEND, { agent = msg.agent, text = msg.text })
    elseif msg.type == "agent" then
        local v = if type(msg.value) == "table" then msg.value[1] else msg.value
        state.agent = if type(v) == "string" then v else "all"
    elseif msg.type == "draft" then
        state.drafts[msg.post] = if type(msg.value) == "string" then msg.value else ""
    elseif msg.type == "pick" then
        state.problems[msg.post] = nil
        push(state, ANSWER, msg.post)
        wheel.action(ANSWER, { post = msg.post, pick = msg.option })
    elseif msg.type == "send" then
        local reply = trimmed(state.drafts[msg.post])
        if reply == "" then
            state.problems[msg.post] = "Write a reply first"
            return
        end
        state.problems[msg.post] = nil
        push(state, ANSWER, msg.post)
        wheel.action(ANSWER, { post = msg.post, reply = reply })
    end
end

local function view(state, data)
    local fresh = data.feed("turn-feed.posts", { space = data.space })
    local snap = fresh
    if fresh ~= nil then LAST_POSTS[data.space] = fresh else snap = LAST_POSTS[data.space] end
    local agents = data.feed("wheel.agents")
    if agents ~= nil then LAST_AGENTS = agents else agents = LAST_AGENTS end
    if snap == nil then
        return ui.Column{ modifiers = { { padding = 16 } }, ui.Text{ text = "Reading the posts…" } }
    end
    local running
    if agents ~= nil then
        running = {}
        for _, a in agents do running[a.pane] = true end
    end
    local posts = snap.posts or NONE
    if #posts == 0 then
        return ui.Column{
            modifiers = { { padding = 16 } },
            children = { text("No posts yet from this space's agents. Agents post with: wheel plugin action \"$WHEEL_PANE\" turn-feed.post --args-file post.json", { tone = "muted", flex = true }) },
        }
    end
    -- One chip per agent pane that posted, in the order of its newest post, then the posts it picks.
    local counts, order = {}, {}
    for _, post in posts do
        local p = tostring(post.agent_pane)
        if not counts[p] then counts[p] = 0; table.insert(order, p) end
        counts[p] += 1
    end
    local pick = state.agent or "all"
    if pick ~= "all" and not counts[pick] then pick = "all" end
    local options = { { label = "All  " .. #posts, value = "all" } }
    for _, p in order do
        local label = "Agent " .. p .. "  " .. counts[p]
        if running ~= nil and not running[p] then label = label .. " · gone" end
        table.insert(options, { label = label, value = p })
    end
    local function picker(style, mods)
        return ui.ChoicePicker{
            variant = "mutuallyExclusive", displayStyle = style, options = options, value = { pick },
            on_change = { type = "agent" }, modifiers = mods,
        }
    end
    local chips = ui.ViewThatFits{
        ["in"] = "horizontal",
        picker("chips", { { fixedSize = { horizontal = true } } }),
        picker("menu", { { frame = { maxWidth = "infinity", alignment = "leading" } } }),
    }
    local query = state.query or ""
    local search = ui.TextField{
        label = "Search posts", value = query, variant = "shortText",
        on_change = { type = "query" },
        modifiers = { { frame = { maxWidth = "infinity" } } },
    }
    local cards, seen = {}, {}
    for _, post in posts do
        local p = tostring(post.agent_pane)
        local latest = not seen[p]
        seen[p] = true
        if (pick == "all" or p == pick) and matches(post, query) then table.insert(cards, post_view(state, post, running, latest)) end
    end
    if #cards == 0 then
        table.insert(cards, text("No post holds \"" .. trimmed(query) .. "\".", { tone = "muted", flex = true }))
    end
    cards.spacing = 12
    cards.modifiers = { { padding = { 4, 12, 12, 12 } } }
    local list = ui.List(cards)
    return ui.Column{
        spacing = 0,
        children = {
            ui.Column{ spacing = 8, children = { search, chips }, modifiers = { { padding = { 10, 12, 10, 12 } }, { frame = { maxWidth = "infinity", alignment = "leading" } } } },
            ui.Divider{},
            list,
        },
        modifiers = {
            { frame = { maxWidth = "infinity", maxHeight = "infinity", alignment = "top" } },
        },
    }
end

return { init = init, update = update, view = view }
