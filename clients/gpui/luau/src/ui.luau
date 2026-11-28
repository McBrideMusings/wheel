local measure_host, sizes, canvases = ...
setmetatable(canvases, { __mode = 'k' })
local buffer_len, type_of = buffer.len, typeof
local function node(name, arg)
    local n = { component = name }
    if type(arg) == "string" then
        n.text = arg
        return n
    end
    if type(arg) ~= "table" then return n end
    for k, v in arg do
        if type(k) ~= "number" then n[k] = v end
    end
    local children = {}
    for i, v in ipairs(arg) do
        children[i] = if type(v) == "string" then { component = "Text", text = v } else v
    end
    if #children > 0 then n.children = children end
    return n
end

local styleTypes = {
    fillStyle = "string", strokeStyle = "string", font = "string",
    lineWidth = "number", globalAlpha = "number",
}
local function numbers(method, ...)
    for i = 1, select("#", ...) do
        if type(select(i, ...)) ~= "number" then
            error("ctx:" .. method .. " expects numbers", 3)
        end
    end
end
local function recorder(commands)
    local style = { fillStyle = "fg", strokeStyle = "fg", lineWidth = 1, font = "10px sans-serif", globalAlpha = 1 }
    local stack = {}
    local methods = {}
    local count = 0
    local function add(command)
        count += 1
        if count > 250000 then
            error("a Canvas draw recorded " .. count .. " commands; the limit is 250,000", 3)
        end
        commands[count] = command
    end
    local function plain(method, count)
        methods[method] = function(_, ...)
            numbers(method, ...)
            if select("#", ...) ~= count then error("ctx:" .. method .. " expects " .. count .. " numbers", 2) end
            add({ method, ... })
        end
    end
    local function bare(method)
        methods[method] = function() add({ method }) end
    end
    for _, m in { "beginPath", "closePath", "fill", "stroke" } do bare(m) end
    for m, count in { moveTo = 2, lineTo = 2, bezierCurveTo = 6, quadraticCurveTo = 4, rect = 4,
                      roundRect = 5, translate = 2, scale = 2, rotate = 1 } do
        plain(m, count)
    end
    methods.arc = function(_, x, y, r, a0, a1, ccw)
        numbers("arc", x, y, r, a0, a1)
        if ccw ~= nil and type(ccw) ~= "boolean" then error("ctx:arc expects a boolean for counterclockwise", 2) end
        add({ "arc", x, y, r, a0, a1, ccw == true })
    end
    methods.save = function()
        local saved = {}
        for k, v in style do saved[k] = v end
        stack[#stack + 1] = saved
        add({ "save" })
    end
    methods.restore = function()
        local saved = stack[#stack]
        if saved then
            stack[#stack] = nil
            for k, v in saved do style[k] = v end
        end
        add({ "restore" })
    end
    methods.fillText = function(_, text, x, y)
        if type(text) ~= "string" then error("ctx:fillText expects text", 2) end
        numbers("fillText", x, y)
        add({ "fillText", text, x, y })
    end
    methods.drawImage = function(_, img, x, y, w, h)
        if type(img) ~= "table" or type(img.width) ~= "number" or type(img.height) ~= "number"
            or type_of(img.data) ~= "buffer" then
            error("ctx:drawImage expects { width, height, data = buffer }", 2)
        end
        numbers("drawImage", x, y)
        if w ~= nil or h ~= nil then numbers("drawImage", w, h) end
        for _, side in { img.width, img.height } do
            if side % 1 ~= 0 or side < 1 or side > 2048 then
                error("ctx:drawImage: an image is 1 to 2048 whole pixels on each side (2048x2048 at most), this one is "
                    .. img.width .. "x" .. img.height, 2)
            end
        end
        local expected = img.width * img.height * 4
        if buffer_len(img.data) ~= expected then
            error("ctx:drawImage: a " .. img.width .. "x" .. img.height .. " image needs " .. expected
                .. " bytes of RGBA, the buffer has " .. buffer_len(img.data), 2)
        end
        local copy = buffer.create(expected)
        buffer.copy(copy, 0, img.data)
        add({ "drawImage", copy, img.width, img.height, x, y, w or img.width, h or img.height })
    end
    methods.measureText = function(_, text)
        if type(text) ~= "string" then error("ctx:measureText expects text", 2) end
        if #text > 10000 then error("ctx:measureText takes at most 10,000 bytes of text", 2) end
        local width, ascent, descent = measure_host(text, style.font)
        return { width = width, fontBoundingBoxAscent = ascent, fontBoundingBoxDescent = descent }
    end
    return setmetatable({}, {
        __index = function(_, k)
            local v = style[k]
            if v ~= nil then return v end
            local method = methods[k]
            if method == nil then
                error("ctx has no field or method '" .. tostring(k) .. "' (see docs/plugin-runtime.md for the Canvas 2D calls ctx has)", 2)
            end
            return method
        end,
        __newindex = function(_, k, v)
            local want = styleTypes[k]
            if want == nil then error("ctx has no field " .. tostring(k), 2) end
            if type(v) ~= want then error("ctx." .. k .. " expects a " .. want, 2) end
            style[k] = v
            add({ "set", k, v })
        end,
        __metatable = false,
    })
end

local function canvas(arg)
    if type(arg) ~= "table" or type(arg.draw) ~= "function" then return node("Canvas", arg) end
    local key = if arg.key == nil then "canvas" else arg.key
    local height = if arg.height == nil then 240 else arg.height
    local size = sizes[key]
    local commands = {}
    arg.draw(recorder(commands), if size then size.w else 0, if size then size.h else height)
    local n = node("Canvas", arg)
    n.draw, n.key, n.on_pointer = nil, nil, nil
    canvases[commands] = true
    n.commands, n.height, n.canvasKey, n.pointerAction = commands, height, key, arg.on_pointer
    return n
end

return setmetatable({}, {
    __index = function(_, name)
        if name == "Canvas" then return canvas end
        return function(arg) return node(name, arg) end
    end,
    __metatable = false,
})
