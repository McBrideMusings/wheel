-- The message box: a text field, a Send button and one fixed-height line under them for the send's error.
local style = require("lib/style")

local M = {}

local ACTION = "wheel.agent.send"

local function trimmed(text)
    return (string.gsub(string.gsub(text or "", "^%s+", ""), "%s+$", ""))
end

function M.init(state)
    state.draft = state.draft or ""
    state.sending = state.sending or false
    state.error = state.error or ""
    state.unsent = state.unsent or ""
end

-- Handles the composer's messages; returns true when `msg` was one of them.
function M.update(state, msg)
    if msg.type == "draft" then
        state.draft = msg.value or ""
        return true
    elseif msg.type == "send" then
        local text = trimmed(state.draft)
        if text ~= "" and state.agent then
            wheel.action(ACTION, { agent = state.agent, text = text })
            state.sending = true
            state.error = ""
            state.unsent = text
            state.draft = ""
        end
        return true
    elseif msg.action == ACTION then
        state.sending = false
        if msg.error then
            state.error = tostring(msg.error)
            -- give the person their text back to send again
            if trimmed(state.draft) == "" then state.draft = state.unsent end
        else
            state.unsent = ""
        end
        return true
    end
    return false
end

-- The composer row.
function M.view(state)
    local field = ui.TextField{
        label = "Message",
        value = state.draft or "",
        variant = "shortText",
        on_change = { type = "draft" },
        modifiers = { { frame = { minWidth = 120, maxWidth = "infinity" } } },
    }
    local send = ui.Button{
        child = ui.Text{ text = "Send", modifiers = { { frame = { minWidth = 48 } } } },
        action = { type = "send" },
    }
    return ui.Row{ spacing = 8, children = { field, send }, modifiers = { { padding = { 8, 10, 0, 10 } } } }
end

-- The line under the composer: always present, empty when there is nothing to say.
function M.note(state)
    local body, color = "", "secondary"
    if state.error and state.error ~= "" then
        body, color = state.error, "red"
    elseif state.sending then
        body = "Sending…"
    end
    local line = style.text(body, { size = 11, color = color, lines = 1 })
    table.insert(line.modifiers, { padding = { 0, 10 } })
    table.insert(line.modifiers, { frame = { height = 18 } })
    return line
end

return M
