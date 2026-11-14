-- The controls fixture: shows the pane's context and logs every message a Table, Canvas, CheckBox,
-- ChoicePicker or Modal sends to update.
return {
    init = function(p) return p or { log = {} } end,
    update = function(state, msg)
        local keys = {}
        for k, v in msg do table.insert(keys, k .. "=" .. (if type(v) == "table" then "[" .. #v .. "]" else tostring(v))) end
        table.sort(keys)
        table.insert(state.log, table.concat(keys, ","))
    end,
    view = function(state, data)
        return ui.Column{
            ui.Text("last " .. (state.log[#state.log] or "none")),
            ui.Text("pane " .. tostring(data.pane) .. " cwd " .. tostring(data.cwd) .. " " .. tostring(data.plugin) .. "/" .. tostring(data.plugin_pane)),
            ui.Table{
                columns = { { key = "t", label = "T" } },
                sections = { { rows = { { id = "a", cells = { { key = "t", text = "A" } }, menu = { { label = "Drop", action = { type = "drop", id = "a" } } } } } } },
                rowAction = { type = "open" }, sortAction = { type = "sort" }, foldAction = { type = "fold" },
            },
            ui.Canvas{ ops = {}, hits = { { id = "n1", rect = { 0, 0, 10, 10 }, action = { type = "hit" }, doubleAction = { type = "open", id = "n1" } } } },
            ui.CheckBox{ label = "Done", value = false, on_change = { type = "done" } },
            ui.ChoicePicker{ options = { { label = "A", value = "a" } }, value = {}, on_change = { type = "pick" } },
            ui.Modal{ dismissAction = { type = "dismiss" }, ui.Text("modal") },
        }
    end,
}
