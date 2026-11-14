-- Ordering helpers for the wheelhouse panes.
local M = {}

local WIDTH = 12

local function pad(digits)
    local d = string.gsub(digits, "^0+", "")
    if #d >= WIDTH then return d end
    return string.rep("0", WIDTH - #d) .. d
end

-- A string that sorts like the natural order of `s`: case-insensitive, and a run of digits by its number (`wheel-9` before
-- `wheel-10`). Compare two keys with `<`.
function M.natural_key(s)
    return (string.gsub(string.lower(s), "%d+", pad))
end

-- -1, 0 or 1 for two numbers or two strings.
function M.cmp(a, b)
    if a < b then return -1 end
    if a > b then return 1 end
    return 0
end

return M
