-- The Turn feed pane: one card per agent turn, newest first.
--
-- `turn-feed.posts` {space} holds the posts this space's agent panes made, so a second space on the same
-- directory has its own feed; `wheel.agents` says which pane still runs an agent. Each post is its own Card
-- (GroupBox), headed by the agent pane that wrote it.
-- A pick or a reply calls `turn-feed.answer`, which records it and returns the text to send; the pane then
-- calls `wheel.agent.send` with it, then `turn-feed.delivered`. An answer stored with `delivered == false` can be
-- sent again from its Send button. All three answers arrive in `update` as `{action, result | error}` in the order
-- the calls were made, so each action keeps a queue of the post ids it was called for.
local NONE = {}

local COLOUR = { fg = "primary", muted = "secondary", ok = "green", warn = "orange", bad = "red" }

local ANSWER, SEND, DELIVERED = "turn-feed.answer", "wheel.agent.send", "turn-feed.delivered"

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

local function row(children, spacing)
    return ui.Row{ spacing = spacing or 8, children = children, modifiers = { { frame = { maxWidth = "infinity", alignment = "leading" } } } }
end

local function trimmed(s)
    return (string.gsub(string.gsub(s or "", "^%s+", ""), "%s+$", ""))
end

-- `HH:MM UTC` of an RFC 3339 time (`2026-11-16T16:40:09.123Z`).
local function clock(at)
    if type(at) ~= "string" or #at < 16 then return "?" end
    return string.sub(at, 12, 16) .. " UTC"
end

-- ───────────────────────────── what a post shows ─────────────────────────────

-- The body of a post: its Markdown drawn as blocks (ADR-0046).
local function body_view(body)
    return ui.StructuredText{ markdown = body }
end

-- The page a post names: the artifact's page inside the post (ADR-0047).
local function page_view(name)
    return ui.WebView{ artifact = name, height = 360 }
end

local function option_view(post, o, locked)
    local head = { text(o.id, { weight = "semibold" }) }
    if o.pick then table.insert(head, text("my pick", { size = 11, tone = "ok", weight = "semibold" })) end
    table.insert(head, text(o.action, { flex = true }))
    table.insert(head, button("Pick " .. o.id, { type = "pick", post = post.post, option = o.id }, "Pick " .. o.id, locked))
    return ui.Column{
        spacing = 2,
        children = {
            row(head),
            text("Case: " .. o.case, { size = 12, tone = "muted", flex = true }),
            text("Cost: " .. o.cost, { size = 12, tone = "muted", flex = true }),
            text("Checked: " .. o.checked, { size = 12, tone = "muted", flex = true }),
        },
        modifiers = { { padding = { 4, 0, 4, 12 } } },
    }
end

-- What the person answered, or why the post cannot be answered.
local function status_line(post, gone)
    local a = post.answer
    if type(a) == "table" then
        if a.delivered == false then return "Saved, but the agent did not get it", "warn" end
        if a.pick then return "Picked " .. a.pick .. " · " .. clock(a.at), "ok" end
        return "Replied · " .. clock(a.at) .. ": " .. tostring(a.reply), "ok"
    end
    if gone then return "Agent pane " .. tostring(post.agent_pane) .. " is gone", "warn" end
    return "", "muted"
end

local function post_view(state, post, running)
    local answered = type(post.answer) == "table"
    local gone = running ~= nil and not running[post.agent_pane]
    local locked = answered or gone
    local undelivered = answered and post.answer.delivered == false and type(post.answer.text) == "string"
    local posted = clock(post.at)
    if post.updated_at then posted = posted .. " · edited " .. clock(post.updated_at) end
    local c = {
        row({
            text("Agent " .. tostring(post.agent_pane), { size = 11, weight = "semibold", tone = "muted", flex = true }),
            text(posted, { size = 11, tone = "muted" }),
        }),
        text(post.title, { weight = "bold", size = 16, flex = true }),
        ui.Divider{},
        body_view(post.body),
    }
    if post.page then table.insert(c, page_view(post.page)) end
    local options = post.options or NONE
    if #options > 0 then
        table.insert(c, text(post.question or "", { weight = "semibold", flex = true }))
        for _, o in options do table.insert(c, option_view(post, o, locked)) end
    end
    local draft = state.drafts[post.post] or ""
    table.insert(c, ui.TextField{
        label = "Reply", value = draft, variant = "longText", on_change = { type = "draft", post = post.post },
        modifiers = {
            { padding = 6 },
            { frame = { maxWidth = "infinity", minHeight = 56, alignment = "topLeading" } },
            { overlay = { stroke = "separator", cornerRadius = 6 } },
            if locked then { disabled = true } else nil,
        },
    })
    local line, tone = status_line(post, gone)
    local problem = state.problems[post.post]
    if problem then line, tone = problem, "bad" end
    local send
    if undelivered then
        send = button("Send again", { type = "resend", post = post.post, agent = post.agent_pane, text = post.answer.text }, "Send the saved answer to the agent again", gone)
    else
        send = button("Send", { type = "send", post = post.post }, "Send the reply", locked)
    end
    table.insert(c, row({ send, text(line, { size = 12, tone = tone, flex = true }) }))
    return ui.Card{
        child = ui.Column{
            spacing = 8,
            children = c,
            modifiers = { { padding = 10 }, { frame = { maxWidth = "infinity", alignment = "leading" } } },
        },
        modifiers = { { padding = { 6, 12, 6, 12 } } },
    }
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or { drafts = {}, problems = {}, inflight = { [ANSWER] = {}, [SEND] = {}, [DELIVERED] = {} } }
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
    elseif msg.type == "resend" then
        state.problems[msg.post] = nil
        push(state, SEND, msg.post)
        wheel.action(SEND, { agent = msg.agent, text = msg.text })
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
    local cards = {}
    for _, post in posts do table.insert(cards, post_view(state, post, running)) end
    cards.spacing = 4
    return ui.List(cards)
end

return { init = init, update = update, view = view }
