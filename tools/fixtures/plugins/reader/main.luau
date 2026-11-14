-- The reader fixture's pane: reads the clock plugin's feed through `reads = ["clock.*"]`.
local function view(state, data)
    local now = data.feed("clock.now", {})
    local tick = if now then now.tick else 0
    return ui.Text("Reader tick: " .. tostring(tick))
end

return { init = function(previous) return previous or {} end, update = function(state, msg) end, view = view }
