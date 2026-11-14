-- The embed fixture: a terminal pane of this space beside a list that picks it, a web view, and
-- the first agent pane's chat. Every block has a fixed height and a fixed slot, so no control moves.
local lastPanes = {}
local lastAgents = {}

local function terminals(space, own)
    local found = {}
    for _, p in lastPanes do
        if p.kind == "terminal" and p.space == space and p.id ~= own then
            table.insert(found, p.id)
        end
    end
    return found
end

local function firstAgent(space)
    for _, a in lastAgents do
        if a.space == space then return a.pane end
    end
    return "none"
end

return {
    init = function(p) return p or { sel = nil } end,
    update = function(state, msg)
        if msg.type == "select" then state.sel = msg.pane end
    end,
    view = function(state, data)
        local panes = data.feed("wheel.panes")
        if panes ~= nil then lastPanes = panes end
        local agents = data.feed("wheel.agents")
        if agents ~= nil then lastAgents = agents end

        local found = terminals(data.space, data.pane)
        local selected = found[1] or "none"
        for _, id in found do
            if id == state.sel then selected = id end
        end

        local buttons = {}
        for _, id in found do
            table.insert(buttons, ui.Button{
                child = ui.Text((if id == selected then "> " else "  ") .. id),
                action = { type = "select", pane = id },
            })
        end
        if #found == 0 then table.insert(buttons, ui.Text("no terminal panes")) end

        buttons.modifiers = { { frame = { width = 120, height = 260, alignment = "top" } } }
        return ui.Column{
            ui.Row{
                ui.Column(buttons),
                ui.Terminal{ pane = selected, height = 260 },
            },
            ui.WebView{ url = "https://example.com", height = 220 },
            ui.AgentChat{ pane = firstAgent(data.space), height = 200 },
            ui.Text{ text = "found " .. #found .. " terminal panes", modifiers = { { frame = { height = 20 } } } },
        }
    end,
}
