local load_module, modules = ...
local function resolve(path)
    if type(path) ~= "string" then
        error('require takes a path in the package, like "lib/ui"', 3)
    end
    if string.sub(path, 1, 1) == "/" then
        error('require("' .. path .. '"): the path leaves the package', 3)
    end
    local parts = {}
    for part in string.gmatch(path, "[^/]+") do
        if part == ".." then
            if #parts == 0 then
                error('require("' .. path .. '"): the path leaves the package', 3)
            end
            table.remove(parts)
        elseif part ~= "." then
            table.insert(parts, part)
        end
    end
    local key = table.concat(parts, "/")
    if string.sub(key, -5) ~= ".luau" then key ..= ".luau" end
    return key
end
return function(path)
    local key = resolve(path)
    local hit = modules.loaded[key]
    if hit ~= nil then return hit.value end
    if modules.loading[key] then
        error('require("' .. path .. '"): ' .. key .. ' is already being required (a require cycle)', 2)
    end
    local fn, message = load_module(key)
    if fn == nil then
        error(message or ('require("' .. path .. '"): the package has no ' .. key), 2)
    end
    modules.loading[key] = true
    local ok, value = pcall(fn)
    modules.loading[key] = nil
    if not ok then error(value, 0) end
    modules.loaded[key] = { value = value }
    return value
end
