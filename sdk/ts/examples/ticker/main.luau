-- The ticker example's pane: shows the two feeds of the TypeScript backend and calls its echo action.
local function init(previous)
    local state = previous or {}
    state.out = state.out or ""
    return state
end

local function update(state, msg)
    if msg.action ~= nil then
        state.out = if msg.error then "error: " .. msg.error else "echo: " .. tostring(msg.result.echo.n)
    elseif msg.type == "echo" then
        wheel.action("ticker.echo", { n = 1 })
    end
end

local function view(state, data)
    local tick, stale = data.feed("ticker.tick", {})
    local log = data.feed("ticker.log", {})
    local n = if tick then tick.n else 0
    local time = if tick then tick.time else "--:--:--"
    local list = if log and log.lines then log.lines else {}
    return ui.Column{
        modifiers = { { padding = 16 } },
        ui.Text{ text = "Tick " .. tostring(n) .. " at " .. time .. (if stale then " (stale)" else ""), variant = "h2" },
        ui.Text("Lines: " .. tostring(#list)),
        ui.Button{ child = ui.Text("Echo"), action = { type = "echo" } },
        ui.Text(state.out),
    }
end

return { init = init, update = update, view = view }
