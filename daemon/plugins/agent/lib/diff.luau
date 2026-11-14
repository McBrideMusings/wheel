-- A line diff of two texts as one node: the path, then removed lines in red ("- "), added lines in green ("+ ") and at most
-- three unchanged lines around each change, the rest elided. Pure Luau: no host calls.
local style = require("lib/style")

local M = {}

local MAX_LINES = 400 -- either side beyond this is not diffed
local CONTEXT = 3
local MAX_SHOWN = 200 -- rows drawn; the rest is counted

local function split(text)
    local lines = {}
    if text == nil or text == "" then return lines end
    for line in string.gmatch(text, "([^\n]*)\n?") do table.insert(lines, line) end
    -- the pattern matches once more at the end of the text
    if lines[#lines] == "" then table.remove(lines) end
    return lines
end

-- The edit script from `a` to `b`: a list of { tag = " " | "-" | "+", text }. The common head and tail are matched
-- first, then the middle by a longest-common-subsequence table.
function M.ops(a, b)
    local ops = {}
    local head = 1
    while head <= #a and head <= #b and a[head] == b[head] do head += 1 end
    local ta, tb = #a, #b
    while ta >= head and tb >= head and a[ta] == b[tb] do
        ta -= 1
        tb -= 1
    end
    for i = 1, head - 1 do table.insert(ops, { tag = " ", text = a[i] }) end
    local n, m = ta - head + 1, tb - head + 1
    -- lcs[i][j]: the common length of a[head+i-1 ..] and b[head+j-1 ..]
    local lcs = {}
    for i = 1, n + 1 do
        local row = table.create(m + 1, 0)
        lcs[i] = row
    end
    for i = n, 1, -1 do
        local row, below = lcs[i], lcs[i + 1]
        local ai = a[head + i - 1]
        for j = m, 1, -1 do
            if ai == b[head + j - 1] then
                row[j] = below[j + 1] + 1
            else
                local down, right = below[j], row[j + 1]
                row[j] = if down >= right then down else right
            end
        end
    end
    local i, j = 1, 1
    while i <= n and j <= m do
        if a[head + i - 1] == b[head + j - 1] then
            table.insert(ops, { tag = " ", text = a[head + i - 1] })
            i += 1
            j += 1
        elseif lcs[i + 1][j] >= lcs[i][j + 1] then
            table.insert(ops, { tag = "-", text = a[head + i - 1] })
            i += 1
        else
            table.insert(ops, { tag = "+", text = b[head + j - 1] })
            j += 1
        end
    end
    while i <= n do
        table.insert(ops, { tag = "-", text = a[head + i - 1] })
        i += 1
    end
    while j <= m do
        table.insert(ops, { tag = "+", text = b[head + j - 1] })
        j += 1
    end
    for k = ta + 1, #a do table.insert(ops, { tag = " ", text = a[k] }) end
    return ops
end

-- The rows to draw for `ops`: changes, and unchanged lines within CONTEXT of one; each longer unchanged run becomes one
-- { tag = "…" } row.
function M.rows(ops)
    local near = {}
    for i, op in ops do
        if op.tag ~= " " then
            for k = math.max(1, i - CONTEXT), math.min(#ops, i + CONTEXT) do near[k] = true end
        end
    end
    local rows = {}
    local elided = false
    for i, op in ops do
        if near[i] then
            table.insert(rows, op)
            elided = false
        elseif not elided then
            table.insert(rows, { tag = "…", text = "" })
            elided = true
        end
    end
    return rows
end

-- The rows of the diffs drawn lately, by old and new text: `view` runs on every feed patch, a streamed token included,
-- and the longest-common-subsequence table is the cost. Forgotten whole past CACHE_MAX texts.
local CACHE_MAX = 64
local cache, cached_count = {}, 0
local function cached(old_text, new_text, a, b)
    local by_new = cache[old_text]
    local rows = by_new and by_new[new_text]
    if rows then return rows end
    if cached_count >= CACHE_MAX then
        cache, cached_count = {}, 0
    end
    rows = M.rows(M.ops(a, b))
    cache[old_text] = cache[old_text] or {}
    cache[old_text][new_text] = rows
    cached_count += 1
    return rows
end

function M.view(old_text, new_text, path)
    local a, b = split(old_text), split(new_text)
    local kids = { style.text(path or "", { size = 12, weight = "semibold", mono = true }) }
    if #a > MAX_LINES or #b > MAX_LINES then
        table.insert(kids, style.text("diff too large: " .. math.max(#a, #b) .. " lines", { size = 12, color = "secondary" }))
    else
        local rows = cached(old_text or "", new_text or "", a, b)
        -- Consecutive lines of one colour are one Text, so a diff costs a node per run, not per line.
        local run, color = {}, nil
        local function flush()
            if #run > 0 then table.insert(kids, style.text(table.concat(run, "\n"), { size = 12, mono = true, color = color })) end
            run = {}
        end
        for i, row in rows do
            if i > MAX_SHOWN then
                flush()
                color = "secondary"
                table.insert(kids, style.text("… " .. (#rows - MAX_SHOWN) .. " more lines", { size = 12, mono = true, color = color }))
                break
            end
            local c = if row.tag == "-" then "red" elseif row.tag == "+" then "green" else "secondary"
            if c ~= color then
                flush()
                color = c
            end
            table.insert(run, if row.tag == "…" then "…" else row.tag .. " " .. row.text)
        end
        flush()
    end
    return ui.Column{ spacing = 0, children = kids }
end

return M
