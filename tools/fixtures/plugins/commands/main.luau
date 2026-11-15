-- The command backend fixture's pane: shows the `now` and `note` feeds (each `{value, error}`) and
-- runs the package's actions, showing each result.
local function init(previous)
    local state = previous or {}
    state.draft = state.draft or ""
    state.out = state.out or "no action yet"
    return state
end

local function update(state, msg)
    if msg.action ~= nil then
        if msg.error then
            state.out = "error: " .. msg.error
        elseif msg.result.code == 0 then
            state.out = "result: " .. msg.result.stdout
        else
            state.out = "exit " .. tostring(msg.result.code) .. ": " .. msg.result.stderr
        end
    elseif msg.type == "draft" then
        state.draft = msg.value
    elseif msg.type == "write" then
        wheel.action("cmd.write", { text = state.draft })
    elseif msg.type == "break" then
        wheel.action("cmd.break", { why = "on purpose" })
    elseif msg.type == "mend" then
        wheel.action("cmd.mend", {})
    elseif msg.type == "refuse" then
        wheel.action("cmd.refuse", { why = "no" })
    end
end

local function view(state, data)
    local now = data.feed("cmd.now", {}) or {}
    local note = data.feed("cmd.note", {}) or {}
    local lines = note.value or {}
    return ui.Column{
        modifiers = { { padding = 16 } },
        ui.Text{ text = "Now " .. tostring(if now.value then now.value.now else "--"), variant = "h2" },
        ui.Text("Note lines: " .. tostring(#lines) .. ", last: " .. (lines[#lines] or "none")),
        ui.Text("Note error: " .. (note.error or "none")),
        ui.TextField{ label = "Text", value = state.draft, on_change = { type = "draft" } },
        ui.Row{
            ui.Button{ child = ui.Text("Write"), action = { type = "write" } },
            ui.Button{ child = ui.Text("Break"), action = { type = "break" } },
            ui.Button{ child = ui.Text("Mend"), action = { type = "mend" } },
            ui.Button{ child = ui.Text("Refuse"), action = { type = "refuse" } },
        },
        ui.Text(state.out),
    }
end

return { init = init, update = update, view = view }
