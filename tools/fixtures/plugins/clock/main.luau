-- The clock fixture's pane: reads the backend's two feeds and wheeld's spaces feed.
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
    }
end

return { view = view }
