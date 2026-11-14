-- The sample plugin pane: a counter and a text field. The state lives in the client and
-- survives a reload of this file (init gets the previous state).
local function init(previous)
    return previous or { count = 0, name = "" }
end

local function update(state, msg)
    if msg.type == "add" then
        state.count += 1
    elseif msg.type == "name" then
        state.name = msg.value
    end
end

local function view(state, data)
    return ui.Column{
        modifiers = { { padding = 16 } },
        ui.Text{ text = "Count " .. state.count, variant = "h2" },
        ui.Button{ child = ui.Text("Add one"), action = { type = "add" } },
        ui.TextField{ label = "Name", value = state.name, on_change = { type = "name" } },
        ui.Text(if state.name == "" then "Type a name" else "Hello, " .. state.name),
    }
end

return { init = init, update = update, view = view }
