-- Times for the wheelhouse panes. A pane script has no clock (the sandbox removes `os`), so every age is
-- measured from a `now` the caller passes in seconds since the Unix epoch.
local M = {}

-- Seconds since the Unix epoch of an RFC 3339 UTC time such as `2026-10-30T01:07:41Z`; 0 when it does not parse.
function M.epoch(iso)
    if type(iso) ~= "string" or #iso < 19 then return 0 end
    local y = tonumber(string.sub(iso, 1, 4))
    local mo = tonumber(string.sub(iso, 6, 7))
    local d = tonumber(string.sub(iso, 9, 10))
    local h = tonumber(string.sub(iso, 12, 13))
    local mi = tonumber(string.sub(iso, 15, 16))
    local s = tonumber(string.sub(iso, 18, 19))
    if not (y and mo and d and h and mi and s) then return 0 end
    if mo <= 2 then y -= 1 end
    local era = y // 400
    local yoe = y - era * 400
    local doy = (153 * (if mo > 2 then mo - 3 else mo + 9) + 2) // 5 + d - 1
    local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return (era * 146097 + doe - 719468) * 86400 + h * 3600 + mi * 60 + s
end

-- Rounds half away from zero, as Rust's `f64::round` does.
local function round(x)
    return math.floor(x + 0.5)
end

-- How long before `now` the RFC 3339 time `iso` was: `5m`, `3h`, `2d`, each rounded; "" for no time.
function M.ago(iso, now)
    if iso == nil or iso == "" then return "" end
    local m = math.max(now - M.epoch(iso), 0) / 60
    if m < 60 then return round(m) .. "m" end
    if m < 1440 then return round(m / 60) .. "h" end
    return round(m / 1440) .. "d"
end

-- How long before `now` the Unix time `ts` (seconds) was: `now`, `5m`, `3h`, `2d`, `6w`; "" for no time.
function M.ago_ts(ts, now)
    if ts == nil then return "" end
    local s = math.max(now - ts, 0)
    if s < 60 then return "now" end
    if s < 3600 then return s // 60 .. "m" end
    if s < 86400 then return s // 3600 .. "h" end
    if s < 1209600 then return s // 86400 .. "d" end
    return s // 604800 .. "w"
end

return M
