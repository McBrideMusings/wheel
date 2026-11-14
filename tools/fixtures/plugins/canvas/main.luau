-- The canvas fixture: draws with every ctx call, sizes a label box with measureText, draws a
-- generated gradient with drawImage, and shows the last pointer event above the canvas.
local function gradient()
    local width, height = 16, 8
    local data = buffer.create(width * height * 4)
    for y = 0, height - 1 do
        for x = 0, width - 1 do
            local at = (y * width + x) * 4
            buffer.writeu8(data, at, x * 255 // (width - 1))
            buffer.writeu8(data, at + 1, y * 255 // (height - 1))
            buffer.writeu8(data, at + 2, 160)
            buffer.writeu8(data, at + 3, 255)
        end
    end
    return { width = width, height = height, data = data }
end

local image = gradient()

local function draw(ctx, w, h)
    -- paths, curves and the arc
    ctx.strokeStyle = "accent"
    ctx.lineWidth = 2
    ctx:beginPath()
    ctx:moveTo(10, 40)
    ctx:lineTo(60, 10)
    ctx:bezierCurveTo(80, 0, 100, 40, 120, 20)
    ctx:quadraticCurveTo(140, 0, 160, 30)
    ctx:stroke()
    ctx.fillStyle = "rgba(255, 80, 80, 0.6)"
    ctx:beginPath()
    ctx:arc(200, 25, 18, 0, math.pi * 1.5, false)
    ctx:arc(200, 25, 8, math.pi * 1.5, 0, true)
    ctx:closePath()
    ctx:fill()
    -- rect and roundRect with styles and alpha
    ctx.fillStyle = "#3b82f6"
    ctx:beginPath()
    ctx:rect(10, 60, 60, 30)
    ctx:fill()
    ctx.strokeStyle = "green"
    ctx:beginPath()
    ctx:roundRect(80, 60, 60, 30, 8)
    ctx:stroke()
    ctx:save()
    ctx.globalAlpha = 0.4
    ctx.fillStyle = "#ff0"
    ctx:beginPath()
    ctx:roundRect(150, 60, 60, 30, 14)
    ctx:fill()
    ctx:restore()
    -- transforms
    ctx:save()
    ctx:translate(260, 40)
    ctx:rotate(math.pi / 6)
    ctx:scale(1.5, 1)
    ctx.fillStyle = "muted"
    ctx:beginPath()
    ctx:rect(-15, -10, 30, 20)
    ctx:fill()
    ctx:restore()
    -- a label box sized by measureText
    ctx.font = "bold 13px sans-serif"
    local label = "measured " .. math.floor(w) .. "x" .. math.floor(h)
    local m = ctx:measureText(label)
    local pad = 6
    ctx.fillStyle = "#000000cc"
    ctx:beginPath()
    ctx:roundRect(10, 105, m.width + pad * 2, m.fontBoundingBoxAscent + m.fontBoundingBoxDescent + pad * 2, 4)
    ctx:fill()
    ctx.fillStyle = "white"
    ctx:fillText(label, 10 + pad, 105 + pad + m.fontBoundingBoxAscent)
    ctx.font = "11px monospace"
    ctx.fillStyle = "fg"
    ctx:fillText("monospace", 10, 160)
    -- an image
    ctx:drawImage(image, 150, 105)
    ctx:drawImage(image, 150, 140, 64, 32)
end

return {
    init = function(p) return p or { last = "none" } end,
    update = function(state, msg)
        if msg.type == "pointer" then
            state.last = msg.kind .. " " .. math.floor(msg.x) .. " " .. math.floor(msg.y)
        end
    end,
    view = function(state)
        return ui.Column{
            ui.Text("pointer " .. state.last),
            ui.Canvas{ key = "main", height = 200, draw = draw, on_pointer = { type = "pointer" } },
        }
    end,
}
