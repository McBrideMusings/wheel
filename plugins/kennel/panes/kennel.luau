-- The kennel pane: the routes kennel keeps (add, remove, prune), the profile this space's folder gets, and what a
-- harness (Claude, Codex or pi) gets in this project or globally: skills, guidance, permissions, hooks, MCP servers,
-- the vault and kennel doctor's checks.
--
-- Every value is a kennel command's output through Wheel's command backend (wheel-plugin.toml): `routes`, `which`
-- and `doctor` are kennel's text lines, `harness` and `vault` its JSON. Each feed's value is {value, error}.

local NONE = {}

local VIEWS = {
    { "routes", "Routes" }, { "skills", "Skills" }, { "guidance", "Guidance" }, { "permissions", "Permissions" },
    { "hooks", "Hooks" }, { "mcp", "MCP" }, { "vault", "Vault" }, { "doctor", "Doctor" },
}
local HARNESSES = { { "claude", "Claude" }, { "codex", "Codex" }, { "pi", "pi" } }
local SCOPES = { { "project", "This project" }, { "global", "Global" } }

local function init(previous)
    local s = previous or {}
    s.view = s.view or "routes"
    s.harness = s.harness or "claude"
    s.scope = s.scope or "project"
    s.path = s.path or ""
    s.profile = s.profile or ""
    s.permanent = s.permanent or false
    s.out = s.out or ""
    s.out_tone = s.out_tone or "muted"
    return s
end

local function first(v)
    if type(v) == "table" then return v[1] end
    return v
end

-- What an action answered, as one line for the status text.
local function said(msg)
    if msg.error then return msg.error, "bad" end
    local r = msg.result or NONE
    local text = string.gsub(string.gsub((r.stdout or "") .. (r.stderr or ""), "^%s+", ""), "%s+$", "")
    text = string.gsub(text, "^kennel: ", "")
    if r.code == 0 then return (if text == "" then "Done." else text), "ok" end
    return "exit " .. tostring(r.code) .. ": " .. text, "bad"
end

local function update(s, msg)
    if msg.action ~= nil then
        s.out, s.out_tone = said(msg)
        if msg.action == "kennel.route_add" and msg.result and msg.result.code == 0 then s.path = "" end
    elseif msg.type == "view" then s.view = first(msg.value) or s.view
    elseif msg.type == "harness" then s.harness = first(msg.value) or s.harness
    elseif msg.type == "scope" then s.scope = first(msg.value) or s.scope
    elseif msg.type == "path" then s.path = msg.value
    elseif msg.type == "profile" then s.profile = first(msg.value) or ""
    elseif msg.type == "permanent" then s.permanent = msg.value == true
    elseif msg.type == "add" then
        -- The button carries the path and profile the view shows, defaults included (the space's folder, the first profile).
        wheel.action("kennel.route_add", { path = msg.path, profile = msg.profile, permanent = s.permanent, dir = msg.dir })
    elseif msg.type == "rm" then
        wheel.action("kennel.route_rm", { path = msg.path, dir = msg.dir })
    elseif msg.type == "prune" then
        wheel.action("kennel.route_prune", { dir = msg.dir })
    end
end

-- ───────────────────────────── reading kennel's text ─────────────────────────────

-- `kennel route list`: the routes (path, profile, status) and the profiles (name, homes).
local function parse_routes(lines)
    local routes, profiles, part = {}, {}, nil
    for _, l in lines or NONE do
        if string.match(l, "^Routes") then part = "routes"
        elseif string.match(l, "^Profiles") then part = "profiles"
        elseif part == "routes" then
            local path, profile, status = string.match(l, "^%s+(.-)%s+→%s+(%S+)(.*)$")
            if path then
                status = string.gsub(string.gsub(status, "^%s*%[", ""), "%]%s*$", "")
                table.insert(routes, { path = path, profile = profile, status = status })
            end
        elseif part == "profiles" then
            local name, homes = string.match(l, "^%s+(%S+)%s+(.*)$")
            if name then table.insert(profiles, { name = name, homes = homes }) end
        end
    end
    return routes, profiles
end

-- `kennel which`: tab-separated key and value lines.
local function parse_which(lines)
    local out = {}
    for _, l in lines or NONE do
        local k, v = string.match(l, "^([^\t]+)\t(.*)$")
        if k then table.insert(out, { k, v }) end
    end
    return out
end

local function leaf(path)
    return string.match(path or "", "([^/]*)$") or path
end

-- ───────────────────────────── pieces ─────────────────────────────

local function cell(key, text, extra)
    local c = { key = key, text = text or "" }
    if extra then for k, v in extra do c[k] = v end end
    return c
end

local function col(key, label, width)
    return { key = key, label = label, width = width, sortable = true }
end

local function picker(options, value, on_change)
    local opts = {}
    for _, o in options do table.insert(opts, { label = o[2], value = o[1] }) end
    return ui.ChoicePicker{
        variant = "mutuallyExclusive", displayStyle = "menu", options = opts, value = { value },
        on_change = on_change, modifiers = { { fixedSize = { horizontal = true } } },
    }
end

local function feed_error(f)
    if f and f.error then
        return ui.Text{ text = f.error, modifiers = { { foregroundStyle = "red" }, { lineLimit = 3 } } }
    end
    return nil
end

local function table_node(columns, sections, empty)
    return {
        component = "Table", columns = columns, sections = sections, empty = empty,
        rowAction = { type = "noop" },
    }
end

-- ───────────────────────────── the views ─────────────────────────────

local function routes_view(s, data, dir)
    local f = data.feed("kennel.routes", { dir = dir }) or NONE
    local w = data.feed("kennel.which", { dir = dir }) or NONE
    local routes, profiles = parse_routes(f.value)
    local rows = {}
    for _, r in routes do
        table.insert(rows, {
            id = r.path, path = r.path, depth = 0,
            menu = { { label = "Remove route", action = { type = "rm", path = r.path, dir = dir } } },
            cells = {
                cell("path", r.path, { mono = true }), cell("profile", r.profile),
                cell("status", r.status, { tone = if string.find(r.status, "IGNORED") or string.find(r.status, "EXPIRED") then "bad" else "muted" }),
            },
        })
    end
    local profile_rows = {}
    for _, p in profiles do
        table.insert(profile_rows, {
            id = "profile:" .. p.name, path = "profile:" .. p.name, depth = 0, menu = {},
            cells = { cell("path", p.name), cell("profile", ""), cell("status", p.homes, { tone = "muted" }) },
        })
    end
    local here = {}
    for _, kv in parse_which(w.value) do table.insert(here, kv[1] .. ": " .. kv[2]) end
    local options = {}
    for _, p in profiles do table.insert(options, { p.name, p.name }) end
    local profile = s.profile
    if profile == "" and #profiles > 0 then profile = profiles[1].name end
    local path = if s.path == "" then dir or "" else s.path
    return {
        ui.Text{ text = "This folder (" .. leaf(dir) .. ")  " .. table.concat(here, " · "), modifiers = { { lineLimit = 2 } } },
        feed_error(w),
        ui.Row{
            ui.TextField{ label = "Path", value = path, on_change = { type = "path" }, modifiers = { { frame = { maxWidth = "infinity" } } } },
            picker(options, profile, { type = "profile" }),
            ui.CheckBox{ label = "Permanent", value = s.permanent, on_change = { type = "permanent" } },
            ui.Button{ child = ui.Text("Add route"), action = { type = "add", dir = dir, path = path, profile = profile } },
            ui.Button{ child = ui.Text("Prune expired"), action = { type = "prune", dir = dir } },
        },
        feed_error(f),
        table_node(
            { col("path", "Path", nil), col("profile", "Profile", 120), col("status", "Status", 260) },
            { { title = "Routes", count = #rows, rows = rows }, { title = "Profiles", count = #profile_rows, rows = profile_rows } },
            "No routes."
        ),
    }
end

-- One harness section's rows for one block of `kennel harness show <section> --json`.
local function entry_cells(section, e)
    if section == "skills" then
        return { cell("name", e.name), cell("detail", (if e.vault then "vault · " else "") .. #(e.files or NONE) .. " files"), cell("where", e.path, { mono = true, tone = "muted" }) }
    elseif section == "guidance" then
        return { cell("name", e.rel or leaf(e.path)), cell("detail", ""), cell("where", e.path, { mono = true, tone = "muted" }) }
    elseif section == "permissions" then
        return { cell("name", e.rule, { mono = true }), cell("detail", e.decision, { tone = if e.decision == "deny" then "bad" elseif e.decision == "allow" then "ok" else "muted" }), cell("where", e.file, { mono = true, tone = "muted" }) }
    elseif section == "hooks" then
        local what = (e.event or "") .. (if e.matcher and e.matcher ~= "" then " · " .. e.matcher else "")
        return { cell("name", what), cell("detail", e.command or e.script, { mono = true }), cell("where", e.file, { mono = true, tone = "muted" }) }
    end
    local cmd = e.command or e.url or ""
    for _, a in e.args or NONE do cmd ..= " " .. a end
    return { cell("name", e.name), cell("detail", (e.type or "") .. "  " .. cmd, { mono = true }), cell("where", e.file, { mono = true, tone = "muted" }) }
end

local DETAIL = { skills = "Files", guidance = "", permissions = "Decision", hooks = "Command", mcp = "Server" }
local NAME = { skills = "Skill", guidance = "File", permissions = "Rule", hooks = "Event", mcp = "Name" }

local function harness_view(s, data, dir)
    local scope = if s.scope == "global" or not dir then "global" else dir
    local f = data.feed("kennel.harness", { section = s.view, scope = scope, harness = s.harness, dir = dir }) or NONE
    local v = f.value or NONE
    local sections = {}
    for bi, b in v.blocks or NONE do
        local rows = {}
        for i, e in b.entries or NONE do
            local id = bi .. ":" .. i
            table.insert(rows, { id = id, path = id, depth = 0, menu = {}, cells = entry_cells(s.view, e) })
        end
        -- The block's folder is in each row's Where cell, so the title names the scope and the folder's last part.
        table.insert(sections, { title = (b.scope or "") .. "  " .. leaf(b.path), count = #rows, rows = rows })
    end
    local home = (v.scope or NONE).home
    return {
        ui.Row{
            picker(HARNESSES, s.harness, { type = "harness" }),
            picker(SCOPES, s.scope, { type = "scope" }),
            ui.Text{ text = if home then "home " .. home else "", modifiers = { { lineLimit = 1 } } },
        },
        feed_error(f),
        table_node(
            { col("name", NAME[s.view], 260), col("detail", DETAIL[s.view], nil), col("where", "Where", 320) },
            sections, "Nothing here."
        ),
    }
end

local function vault_view(data, dir)
    local f = data.feed("kennel.vault", { dir = dir }) or NONE
    local v = f.value or NONE
    -- `kennel vault ls --json`: skills and snippets are {name, item, path, files}; copies are {path, item, rev, head,
    -- behind}, a copy behind when the vault has a newer commit of its item.
    local function items(list, kind)
        local rows = {}
        for i, e in list or NONE do
            local id = kind .. ":" .. i
            table.insert(rows, { id = id, path = id, depth = 0, menu = {}, cells = {
                cell("name", e.name or e.item), cell("detail", if e.files then #e.files .. " files" else ""),
                cell("where", e.path, { mono = true, tone = "muted" }),
            } })
        end
        return rows
    end
    local function copies_of(list)
        local rows = {}
        for i, e in list or NONE do
            local id = "copy:" .. i
            local state, tone = "current", "ok"
            if e.head == nil or e.head == "" then state, tone = "item gone from the vault", "bad"
            elseif e.behind then state, tone = "behind " .. string.sub(e.head, 1, 7), "warn" end
            table.insert(rows, { id = id, path = id, depth = 0, menu = {}, cells = {
                cell("name", e.item), cell("detail", state, { tone = tone }), cell("where", e.path, { mono = true, tone = "muted" }),
            } })
        end
        return rows
    end
    local skills, snippets, copies = items(v.skills, "skill"), items(v.snippets, "snippet"), copies_of(v.copies)
    return {
        ui.Text{ text = "Vault " .. (v.dir or ""), modifiers = { { lineLimit = 1 } } },
        feed_error(f),
        table_node(
            { col("name", "Item", 260), col("detail", "State", 160), col("where", "Where", nil) },
            {
                { title = "Skills", count = #skills, rows = skills },
                { title = "Snippets", count = #snippets, rows = snippets },
                { title = "Copies", count = #copies, rows = copies },
            },
            "The vault is empty."
        ),
    }
end

local function doctor_view(data, dir)
    local f = data.feed("kennel.doctor", { dir = dir }) or NONE
    local sections, rows, title = {}, {}, nil
    local function close()
        if title then table.insert(sections, { title = title, count = #rows, rows = rows }) end
        rows = {}
    end
    for i, l in f.value or NONE do
        local status, rest = string.match(l, "^(%S+)%s%s+(.*)$")
        if status then
            local id = "d" .. i
            table.insert(rows, { id = id, path = id, depth = 0, menu = {}, cells = {
                cell("check", rest, { mono = true }), cell("status", status, { tone = if status == "ok" then "ok" else "bad" }),
            } })
        elseif string.match(l, "^%s") then
            -- An indented line with no status is kennel's note for a part with nothing to check, `(no [hooks] opt-in)`.
            local id = "d" .. i
            table.insert(rows, { id = id, path = id, depth = 0, menu = {}, cells = {
                cell("check", (string.gsub(l, "^%s+", "")), { tone = "muted" }), cell("status", ""),
            } })
        elseif l ~= "" then
            close()
            title = l
        end
    end
    close()
    return {
        feed_error(f),
        table_node({ col("check", "Check", nil), col("status", "", 60) }, sections, "kennel doctor printed nothing."),
    }
end

local function view(s, data)
    local dir = data.cwd
    local body
    if s.view == "routes" then body = routes_view(s, data, dir)
    elseif s.view == "vault" then body = vault_view(data, dir)
    elseif s.view == "doctor" then body = doctor_view(data, dir)
    else body = harness_view(s, data, dir) end
    local opts = {}
    for _, v in VIEWS do table.insert(opts, { label = v[2], value = v[1] }) end
    local root = {
        modifiers = { { padding = 12 } }, spacing = 8,
        ui.ChoicePicker{
            variant = "mutuallyExclusive", displayStyle = "chips", options = opts, value = { s.view },
            on_change = { type = "view" },
        },
    }
    -- `body` holds nil where a feed has no error, so it is walked by index.
    for i = 1, 8 do
        if body[i] then table.insert(root, body[i]) end
    end
    table.insert(root, ui.Text{ text = s.out, modifiers = { { lineLimit = 2 } } })
    return ui.Column(root)
end

return { init = init, update = update, view = view }
