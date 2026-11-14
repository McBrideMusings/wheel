-- The clock fixture's pane: reads the backend's two feeds and wheeld's spaces feed, and calls the
-- backend's echo and fail actions.
local function init(previous)
    local state = previous or {}
    state.out = state.out or ""
    return state
end

local function update(state, msg)
    if msg.action ~= nil then
        if msg.error then
            state.out = "error: " .. msg.error
        else
            state.out = "result: " .. tostring(msg.result.echo.n)
        end
    elseif msg.type == "echo" then
        wheel.action("clock.echo", { n = 1 })
    elseif msg.type == "fail" then
        wheel.action("clock.fail", {})
    end
end

local function view(state, data)
    local now = data.feed("clock.now", {})
    local lines = data.feed("clock.lines", {})
    local spaces = data.feed("wheel.spaces", {})
    local tick = if now then now.tick else 0
    local time = if now then now.time else "--:--:--"
    local list = if lines and lines.lines then lines.lines else {}
    local last = list[#list]
    return ui.Column{
        modifiers = { { padding = 16 } },
        ui.Text{ text = "Tick " .. tostring(tick) .. " at " .. time, variant = "h2" },
        ui.Text("Lines: " .. tostring(#list)),
        ui.Text("Last line: " .. (last or "none")),
        ui.Text("Spaces: " .. tostring(if spaces then #spaces else 0)),
        ui.Button{ child = ui.Text("Echo"), action = { type = "echo" } },
        ui.Button{ child = ui.Text("Fail"), action = { type = "fail" } },
        ui.Text(state.out),
    }
end

return { init = init, update = update, view = view }
