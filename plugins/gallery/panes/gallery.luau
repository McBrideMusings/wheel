-- The component gallery pane: one page with a table per class of control. It draws the basic components with every
-- option, the SwiftUI controls, the Table, the modifiers, then Wheel's own components. Nothing on it is wired to a
-- backend; every control raises `{ type = "act" }`, which `update` ignores.
--
-- Every change to how the client draws a component, a property or a modifier updates this file in the same change.

local function copy(props)
    local p = {}
    if props then
        for k, v in props do p[k] = v end
    end
    return p
end

-- A node of `component` with `children` as its array part and `props` as its properties.
local function put(component, children, props)
    local n = copy(props)
    if children then
        for i, c in ipairs(children) do n[i] = c end
    end
    return ui[component](n)
end

local function col(children, props) return put("Column", children, props) end
local function row(children, props) return put("Row", children, props) end

local function txt(t, variant, props)
    local p = copy(props)
    p.text = t
    if variant then p.variant = variant end
    return put("Text", nil, p)
end

local function secondary(t, variant, props)
    local p = copy(props)
    local mods = { { foregroundStyle = "secondary" } }
    for _, m in p.modifiers or {} do table.insert(mods, m) end
    p.modifiers = mods
    return txt(t, variant, p)
end

local function act() return { type = "act" } end

local function button(child, variant)
    local p = { child = child, action = act() }
    if variant then p.variant = variant end
    return put("Button", nil, p)
end

local function concat(into, more)
    for _, v in ipairs(more) do table.insert(into, v) end
    return into
end

-- ---- table layout: fixed-width cells so the columns line up, where the pane is wide enough for them; a narrower pane
-- (a phone) gets each row stacked: its names, its option, then its example, so nothing runs off the pane's edge.
local W = { 130, 130, 230 }

local function fits(wide, narrow)
    return put("ViewThatFits", { wide, narrow }, { ["in"] = "horizontal" })
end

local function stacked(children)
    return col({ col(children, { align = "start", modifiers = { { padding = { vertical = 8 } }, { frame = { maxWidth = "infinity", alignment = "leading" } } } }), put("Divider") })
end

local function cell(content, i)
    local mods
    if i < #W then
        mods = { { frame = { width = W[i + 1], alignment = "leading" } } }
    else
        mods = { { frame = { maxWidth = "infinity", alignment = "leading" } } }
    end
    table.insert(mods, { padding = { vertical = 8, trailing = 12 } })
    return col({ content }, { align = "start", modifiers = mods })
end

local function header()
    local cells = {}
    for i, t in ipairs({ "Component", "SwiftUI", "Option", "Example" }) do
        table.insert(cells, cell(secondary(t, "caption", { modifiers = { { fontWeight = "semibold" } } }), i - 1))
    end
    return fits(col({ row(cells, { justify = "start", align = "start" }), put("Divider") }), col({}))
end

-- One table row per option; the component's name and SwiftUI class on its first row only.
local function comp_rows(name, swiftui, options)
    local rows = {}
    for k, o in ipairs(options) do
        local opt, ex = o[1], o[2]
        local cells = {
            cell(txt(if k == 1 then name else "", nil, { modifiers = { { fontWeight = "semibold" } } }), 0),
            cell(secondary(if k == 1 then swiftui else ""), 1),
            cell(secondary(opt, "caption"), 2),
            cell(ex, 3),
        }
        local narrow = {}
        if k == 1 then
            table.insert(narrow, row({ txt(name, nil, { modifiers = { { fontWeight = "semibold" } } }), secondary(swiftui) }, { justify = "start", align = "start" }))
        end
        table.insert(narrow, secondary(opt, "caption"))
        table.insert(narrow, ex)
        table.insert(rows, fits(col({ row(cells, { justify = "start", align = "start" }), put("Divider") }), stacked(narrow)))
    end
    return rows
end

-- One Card around a class: its heading, its blurb and its table.
local function boxed(title, blurb, body)
    local children = { txt(title, "h2"), secondary(blurb, nil, { modifiers = { { padding = { bottom = 8 } } } }) }
    concat(children, body)
    local inner = col(children, {
        align = "start",
        modifiers = { { padding = 20 }, { frame = { maxWidth = "infinity", alignment = "leading" } } },
    })
    return col({ put("Card", nil, { child = inner }) }, { modifiers = { { padding = { top = 24 } } } })
end

local function klass(title, blurb, comps)
    local rows = { header() }
    for _, c in ipairs(comps) do
        concat(rows, comp_rows(c[1], c[2], c[3]))
    end
    return boxed(title, blurb, rows)
end

local function opts()
    return { { label = "All", value = "a" }, { label = "Open", value = "b" }, { label = "Closed", value = "c" } }
end

local function icons(names)
    local list = {}
    for _, s in ipairs(names) do table.insert(list, put("Icon", nil, { name = s })) end
    return row(list, { justify = "start" })
end

-- ============================================================ classes

local function content_class()
    local text_opts = {}
    for _, v in ipairs({ "h1", "h2", "h3", "h4", "h5", "body", "caption" }) do
        local label
        if string.sub(v, 1, 1) == "h" then
            label = "Heading " .. string.sub(v, 2, 2)
        elseif v == "body" then
            label = "Body"
        else
            label = "Caption"
        end
        table.insert(text_opts, { v, txt(label, v) })
    end
    table.insert(text_opts, {
        "markdown: true reads inline Markdown (bold, italic, code, links)",
        txt("A **bold** word, an *italic* one, `code` and a [link](https://example.com).", nil, { markdown = true }),
    })
    return klass("Content", "Static text and images. Apple's guidelines call read-only text a label: SwiftUI Text, AppKit NSTextField.", {
        { "Text", "Text", text_opts },
        { "Icon", "Image(systemName:)", {
            { "SF Symbol names", icons({ "play.fill", "stop.fill", "xmark", "checkmark.circle.fill", "xmark.circle.fill", "gearshape" }) },
            { "material names", icons({ "check", "close", "add", "delete", "edit", "home" }) },
        } },
        { "Image, Video, AudioPlayer", "Image, VideoPlayer", {
            { "each needs a URL; not shown", secondary("Image variants: icon, avatar, smallFeature, mediumFeature, largeFeature, header") },
        } },
    })
end

local function controls_class()
    return klass("Buttons", "Run an action. The label is a child component.", {
        { "Button", "Button", {
            { "variant default", button(txt("Default")) },
            { "variant primary", button(txt("Primary"), "primary") },
            { "variant borderless", button(txt("Borderless"), "borderless") },
            { "action copy {text}: puts the text on the pasteboard in the client, calls no tool, shows a tick for 1.5 s",
                put("Button", nil, { child = txt("Copy ids"), action = { copy = { text = "a-1 b-2" } }, variant = "borderless" }) },
            { "label is an Icon", button(put("Icon", nil, { name = "play.fill" })) },
            { "label is Icon and Text", button(row({ put("Icon", nil, { name = "play.fill" }), txt("Run build") }, { justify = "start" })) },
        } },
    })
end

local function inputs_class()
    return klass("Selection and input", "Values the person sets.", {
        { "TextField", "TextField, SecureField, TextEditor", {
            { "shortText: a filled box with a border", put("TextField", nil, { label = "Name", value = "", variant = "shortText" }) },
            { "longText", put("TextField", nil, { label = "Notes", value = "", variant = "longText" }) },
            { "number", put("TextField", nil, { label = "Count", value = "12", variant = "number" }) },
            { "obscured", put("TextField", nil, { label = "Token", value = "secret", variant = "obscured" }) },
        } },
        { "CheckBox", "Toggle", {
            { "off", put("CheckBox", nil, { label = "Show closed", value = false }) },
            { "on", put("CheckBox", nil, { label = "Show closed", value = true }) },
        } },
        { "ChoicePicker", "Picker", {
            { "mutuallyExclusive", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "b" }, variant = "mutuallyExclusive" }) },
            { "multipleSelection", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a", "c" }, variant = "multipleSelection" }) },
            { "displayStyle checkbox", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a" }, variant = "multipleSelection", displayStyle = "checkbox" }) },
            { "displayStyle chips: one toggle button per option (several can be on)", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a" }, variant = "multipleSelection", displayStyle = "chips" }) },
            { "mutuallyExclusive, displayStyle chips: a segmented control at every width; a page wanting a menu when it does not fit sends a ViewThatFits of both", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a" }, variant = "mutuallyExclusive", displayStyle = "chips" }) },
            { "displayStyle menu, or no style: a menu, as wide as its widest option; a frame or fixedSize modifier sets its width", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a" }, variant = "mutuallyExclusive", displayStyle = "menu" }) },
            { "filterable", put("ChoicePicker", nil, { label = "Filter", options = opts(), value = { "a" }, variant = "mutuallyExclusive", filterable = true }) },
        } },
        { "Slider", "Slider", {
            { "min 0, max 10, value 4", put("Slider", nil, { label = "Volume", value = 4, min = 0, max = 10 }) },
        } },
        { "DateTimeInput", "DatePicker", {
            { "date", put("DateTimeInput", nil, { label = "Due", value = "2026-11-06", enableDate = true, enableTime = false }) },
            { "time", put("DateTimeInput", nil, { label = "At", value = "2026-11-06T14:30:00", enableDate = false, enableTime = true }) },
            { "date and time", put("DateTimeInput", nil, { label = "When", value = "2026-11-06T14:30:00", enableDate = true, enableTime = true }) },
        } },
    })
end

local JUSTIFY = { "start", "center", "end", "spaceBetween", "spaceEvenly" }
local ALIGN = { "start", "center", "end" }

local function pinned_list()
    local secs = {}
    for _, f in ipairs({ "a.rs", "b.rs", "c.rs" }) do
        local lines = {}
        for i = 1, 8 do
            table.insert(lines, txt(f .. " line " .. i, nil, { modifiers = { { font = { size = 11, design = "monospaced" } } } }))
        end
        table.insert(secs, put("Section", lines, {
            header = row({ txt(f, nil, { modifiers = { { fontWeight = "semibold" } } }) }, {
                justify = "start",
                modifiers = { { padding = { 4, 8 } }, { frame = { maxWidth = "infinity", alignment = "leading" } }, { background = "controlBackground" } },
            }),
        }))
    end
    return put("List", secs, {
        spacing = 0,
        pinnedViews = { "sectionHeaders" },
        scrollTo = 2,
        modifiers = { { frame = { height = 140 } } },
    })
end

-- A keyed Column of fields: Reverse flips the order, and each field keeps what was typed in it, because its `key` (not its
-- place) is its identity.
local function keyed_list(order)
    local fields = {}
    for _, k in ipairs(order) do
        table.insert(fields, put("TextField", nil, { key = k, label = "Field " .. k, modifiers = { { frame = { width = 180 } } } }))
    end
    fields.key = "keyed"
    return col({ col(fields, { align = "start" }), put("Button", nil, { child = txt("Reverse"), action = { type = "reverse" } }) }, { align = "start" })
end

local function layout_class(state)
    local row_opts = {}
    for _, j in ipairs(JUSTIFY) do
        table.insert(row_opts, { "justify " .. j, row({ txt("one"), txt("two"), txt("three") }, { justify = j }) })
    end
    table.insert(row_opts, { "spacing 24", row({ txt("one"), txt("two"), txt("three") }, { justify = "start", spacing = 24 }) })

    local col_opts = {}
    for _, a in ipairs(ALIGN) do
        table.insert(col_opts, { "align " .. a, col({ txt("short"), txt("a longer line") }, { align = a }) })
    end
    local boxes = {}
    for _ = 1, 2 do
        table.insert(boxes, col({ txt("128", "h4"), secondary("Ready") }, {
            align = "start",
            modifiers = { { padding = { 7, 8 } }, { overlay = { stroke = "separator", cornerRadius = 8 } }, { frame = { maxWidth = "infinity", alignment = "leading" } } },
        }))
    end
    table.insert(col_opts, { "in a row beside a Table: keeps its own height", row(boxes, { justify = "start" }) })
    table.insert(col_opts, { "children with a `key`: type in a field, then Reverse; each field moves with its text", keyed_list(state.order or { "a", "b", "c" }) })

    return klass("Containers and layout", "Arrange other components.", {
        { "Row", "HStack", row_opts },
        { "Column", "VStack", col_opts },
        { "List", "List", {
            { "vertical, with a frame height here: that tall, scrolling itself; with no frame a vertical List fills the pane under the rest of the page, which then stays put (a Luau agent view keeps its composer under its transcript this way)",
                put("List", { txt("alpha"), txt("beta"), txt("gamma") }, { direction = "vertical", modifiers = { { frame = { height = 80 } } } }) },
            { "horizontal", put("List", { txt("alpha"), txt("beta"), txt("gamma") }, { direction = "horizontal" }) },
            { "pinnedViews sectionHeaders, scrollTo: each Section's header stays at the top while its rows scroll; scrollTo names the child scrolled to the top when it changes, by its key or its place (here the second section)",
                pinned_list() },
        } },
        { "ViewThatFits", "ViewThatFits", {
            { "in horizontal: the first child whose ideal width fits; the first here has idealWidth 640, so it shows only when the space is at least 640 pt",
                put("ViewThatFits", {
                    txt("At least 640 pt across: this one", nil, { modifiers = { { frame = { minWidth = 640, idealWidth = 640, maxWidth = "infinity", alignment = "leading" } }, { padding = 8 }, { overlay = { stroke = "green" } } } }),
                    txt("Under 640 pt: this one", nil, { modifiers = { { padding = 8 }, { overlay = { stroke = "orange" } } } }),
                }, { ["in"] = "horizontal" }) },
            { "a row with a segmented control, then the same row with a menu: which one shows depends on the width alone when the neighbour has a fixed width",
                put("ViewThatFits", {
                    row({
                        put("ChoicePicker", nil, { label = "Scope", options = opts(), value = { "a" }, variant = "mutuallyExclusive", displayStyle = "chips", modifiers = { { fixedSize = { horizontal = true } } } }),
                        txt("3 files  +12 −4", nil, { modifiers = { { monospacedDigit = true }, { frame = { width = 160, alignment = "trailing" } } } }),
                    }, { justify = "spaceBetween" }),
                    row({
                        put("ChoicePicker", nil, { label = "Scope", options = opts(), value = { "a" }, variant = "mutuallyExclusive", displayStyle = "menu", modifiers = { { fixedSize = true } } }),
                        txt("3 files  +12 −4", nil, { modifiers = { { monospacedDigit = true }, { frame = { width = 160, alignment = "trailing" } } } }),
                    }, { justify = "spaceBetween" }),
                }, { ["in"] = "horizontal" }) },
        } },
        { "Card", "GroupBox", {
            { "one child", put("Card", nil, { child = col({ txt("Inside a card", "h4"), secondary("Body text") }, {
                align = "start",
                modifiers = { { padding = 12 }, { frame = { maxWidth = "infinity", alignment = "leading" } } },
            }) }) },
        } },
        { "Divider", "Divider", {
            { "horizontal", col({ txt("above"), put("Divider", nil, { axis = "horizontal" }), txt("below") }, { align = "start" }) },
            { "vertical", row({ txt("left"), put("Divider", nil, { axis = "vertical" }), txt("right") }, { justify = "start" }) },
        } },
    })
end

local function nav_class()
    return klass("Navigation and presentation", "Switch between or reveal content.", {
        { "Tabs", "TabView", {
            { "three tabs", put("Tabs", nil, { tabs = {
                { title = "One", child = txt("First pane") },
                { title = "Two", child = txt("Second pane") },
                { title = "Three", child = txt("Third pane") },
            } }) },
        } },
        { "Modal", "sheet", {
            { "button opens content in a sheet with a title bar and Done (`title`)",
                put("Modal", nil, {
                    title = "Details",
                    trigger = button(txt("Open")),
                    content = col({ txt("Modal title", "h3"), txt("The modal's content.") }, { modifiers = { { padding = 20 } } }),
                }) },
            { "presented true: dims its own pane and draws a card as wide as the pane inside a 24 pt margin, with a Close icon in the top right corner (a click on the dimmed pane and Esc close it too; the footer holds only actions); dismissAction is raised on close; actions (component ids) draw in the footer left of Close; not shown",
                secondary("Modal with presented=true, dismissAction and actions: [button ids], children are the card's content.") },
        } },
    })
end

-- ---- modifiers table: modifier, arguments, example
local function mod(t, ...)
    return txt(t, nil, { modifiers = { ... } })
end

local MW = { 150, 250 }

local function mcell(content, i)
    local mods
    if i < #MW then
        mods = { { frame = { width = MW[i + 1], alignment = "leading" } } }
    else
        mods = { { frame = { maxWidth = "infinity", alignment = "leading" } } }
    end
    table.insert(mods, { padding = { vertical = 8, trailing = 12 } })
    return col({ content }, { align = "start", modifiers = mods })
end

local function mrow(name, args, ex)
    return fits(col({
        row({
            mcell(txt(name, nil, { modifiers = { { fontWeight = "semibold" } } }), 0),
            mcell(secondary(args, "caption"), 1),
            mcell(ex, 2),
        }, { justify = "start", align = "start" }),
        put("Divider"),
    }), stacked({ txt(name, nil, { modifiers = { { fontWeight = "semibold" } } }), secondary(args, "caption"), ex }))
end

local function modifiers_class()
    local head = {}
    for i, t in ipairs({ "Modifier", "Arguments", "Example" }) do
        table.insert(head, mcell(secondary(t, "caption", { modifiers = { { fontWeight = "semibold" } } }), i - 1))
    end
    local colours = {}
    for _, c in ipairs({ "red", "orange", "green", "blue", "purple", "tint" }) do
        table.insert(colours, mod(c, { foregroundStyle = c }))
    end
    local mrows = {
        fits(col({ row(head, { justify = "start", align = "start" }), put("Divider") }), col({})),
        mrow("font", "a text style (caption, headline...) or {size, weight, design}", mod("monospaced 13", { font = { size = 13, design = "monospaced" } })),
        mrow("fontWeight", "light ... heavy", mod("semibold", { fontWeight = "semibold" })),
        mrow("monospacedDigit", "true", col({ mod("1111", { monospacedDigit = true }), mod("8888", { monospacedDigit = true }) }, { align = "start" })),
        mrow("tracking, textCase", "points; uppercase or lowercase", mod("heading", { tracking = 2 }, { textCase = "uppercase" }, { font = "caption" })),
        mrow("lineLimit", "a number", mod("A long line of text that is cut off where it stops fitting", { lineLimit = 1 })),
        mrow("multilineTextAlignment", "leading, center, trailing", mod("Right-aligned across a wide frame", { multilineTextAlignment = "trailing" }, { frame = { maxWidth = "infinity", alignment = "trailing" } })),
        mrow("textSelection", "true", mod("Select this text", { textSelection = true })),
        mrow("foregroundStyle", "primary, secondary, tertiary, red, orange, yellow, green, mint, teal, cyan, blue, indigo, purple, pink, brown, gray, black, white, tint", row(colours, { justify = "start" })),
        mrow("background", "a colour name, or separator, windowBackground, controlBackground, textBackground", mod("controlBackground", { padding = 8 }, { background = "controlBackground" })),
        mrow("padding", "n, [vertical, horizontal], [top, right, bottom, left], {horizontal, vertical, top, ...}", col({
            mod("padding 12", { padding = 12 }, { overlay = { stroke = "separator" } }),
            mod("[4, 16]", { padding = { 4, 16 } }, { overlay = { stroke = "separator" } }),
        }, { align = "start" })),
        mrow("frame", "{width, height, minWidth, idealWidth, maxWidth, minHeight, idealHeight, maxHeight, alignment}; a max may be infinity; an ideal is the size a ViewThatFits measures", mod("width 120", { frame = { width = 120, alignment = "leading" } }, { overlay = { stroke = "separator" } })),
        mrow("layoutPriority", "a number; the view with the higher priority is offered space first", row({
            mod("priority 1", { layoutPriority = 1 }, { overlay = { stroke = "separator" } }),
            mod("priority 0", { layoutPriority = 0 }, { overlay = { stroke = "separator" } }),
        }, { justify = "start" })),
        mrow("fixedSize", "true, or {horizontal, vertical}", row({
            mod("keeps its whole width", { fixedSize = true }, { overlay = { stroke = "separator" } }),
            mod("squeezed", { lineLimit = 1 }, { frame = { width = 50 } }, { overlay = { stroke = "separator" } }),
        }, { justify = "start" })),
        mrow("truncationMode", "head, middle, tail (with lineLimit)", mod("plugins/wheelhouse/panes/diff.luau", { lineLimit = 1 }, { truncationMode = "head" }, { frame = { width = 160, alignment = "leading" } })),
        mrow("overlay", "{stroke: colour, cornerRadius}", mod("outlined, radius 8", { padding = 10 }, { overlay = { stroke = "separator", cornerRadius = 8 } })),
        mrow("clipShape", "{cornerRadius}", mod("clipped, radius 12", { padding = 10 }, { background = "controlBackground" }, { clipShape = { cornerRadius = 12 } })),
        mrow("opacity", "0 to 1", mod("faded", { opacity = 0.4 })),
        mrow("disabled", "true", put("Button", nil, { child = txt("Disabled button"), action = act(), modifiers = { { disabled = true } } })),
        mrow("tint", "a colour name", put("Button", nil, { child = txt("Confirm"), variant = "primary", action = act(), modifiers = { { tint = "green" } } })),
        mrow("help, accessibilityLabel", "a string", put("Button", nil, { child = put("Icon", nil, { name = "play.fill" }), action = act(), modifiers = { { help = "Run build" }, { accessibilityLabel = "Run build" } } })),
        mrow("hoverEffect", "highlight: a rounded fill while hovered", put("Button", nil, { child = txt("Hover me"), variant = "borderless", action = act(), modifiers = { { padding = { 2, 5 } }, { hoverEffect = "highlight" } } })),
        mrow("hoverEffect (disabled)", "a disabled control never lights", put("Button", nil, { child = txt("Disabled"), variant = "borderless", action = act(), modifiers = { { padding = { 2, 5 } }, { hoverEffect = "highlight" }, { disabled = true } } })),
        mrow("pointerStyle", "link: the pointing hand (macOS)", put("Button", nil, { child = txt("Pointing hand"), variant = "borderless", action = act(), modifiers = { { pointerStyle = "link" } } })),
        mrow("(unknown name)", "draws a red marker", mod("bad modifier", { frobnicate = 1 })),
    }
    return boxed("Modifiers", "A modifiers list on any component; each entry is a SwiftUI modifier by its SwiftUI name.", mrows)
end

-- ---- SwiftUI's own controls under their SwiftUI names (ADR-0032)
local function mi(title)
    return put("Button", nil, { child = txt(title), action = act() })
end

local function chart_data()
    return { { x = "Mon", y = 3, series = "a" }, { x = "Tue", y = 5, series = "a" }, { x = "Wed", y = 2, series = "a" } }
end

local function system_class()
    return klass("SwiftUI controls", "Standard controls the library lacks, named as SwiftUI names them (ADR-0032). A property is the initializer's argument; children are its content.", {
        { "ProgressView", "ProgressView", {
            { "value 0.4 of 1", put("ProgressView", nil, { title = "Uploading", value = 0.4, total = 1 }) },
            { "no value: spinner", put("ProgressView", nil, { title = "Working" }) },
        } },
        { "Gauge", "Gauge", {
            { "linearCapacity", put("Gauge", nil, { title = "Disk", value = 40, min = 0, max = 100 }) },
            { "accessoryCircular", put("Gauge", nil, { title = "CPU", value = 70, min = 0, max = 100, style = "accessoryCircular" }) },
        } },
        { "Menu", "Menu", {
            { "children are items", put("Menu", { mi("Copy"), put("Divider"), mi("Delete") }, { title = "Actions" }) },
            { "keepOpen: stays open while items are ticked (iOS only; a Mac menu closes after each tick)",
                put("Menu", { put("CheckBox", nil, { label = "One", value = true }), put("CheckBox", nil, { label = "Two", value = false }) }, { title = "Pick several", keepOpen = true }) },
        } },
        { "Link", "Link", {
            { "title, destination", put("Link", nil, { title = "Apple", destination = "https://apple.com" }) },
        } },
        { "Form, Section", "Form, Section", {
            { "grouped", put("Form", { put("Section", { put("Stepper", nil, { title = "Count", value = 3, min = 0, max = 10 }) }, { title = "Counts" }) }, { style = "grouped" }) },
            { "header and footer: component ids drawn as the Section's header and footer views",
                put("Form", { put("Section", { put("Stepper", nil, { title = "Count", value = 3, min = 0, max = 10 }) }, {
                    header = row({ put("Icon", nil, { name = "number" }), txt("Counts") }, { justify = "start" }),
                    footer = secondary("Between 0 and 10", "caption"),
                }) }, { style = "grouped" }) },
        } },
        { "LabeledContent", "LabeledContent", {
            { "value", put("LabeledContent", nil, { title = "Branch", value = "main" }) },
            { "children as content", put("LabeledContent", { txt("custom") }, { title = "Child" }) },
        } },
        { "Grid, GridRow", "Grid, GridRow", {
            { "two rows", put("Grid", { put("GridRow", { txt("a"), txt("b") }), put("GridRow", { txt("c"), txt("d") }) }, { horizontalSpacing = 16 }) },
        } },
        { "Stepper", "Stepper", {
            { "min 0, max 10, step 1", put("Stepper", nil, { title = "Count", value = 4, min = 0, max = 10, step = 1 }) },
        } },
        { "ColorPicker", "ColorPicker", {
            { "value #RRGGBB", put("ColorPicker", nil, { title = "Tint", value = "#3478F6" }) },
        } },
        { "Popover", "popover", {
            { "child 0 trigger, child 1 content", put("Popover", { txt("Open popover"), txt("Popover content") }) },
        } },
        { "Alert", "alert", {
            { "presented false; not shown", put("Alert", nil, {
                title = "Delete?", message = "This cannot be undone.", presented = false,
                buttons = { { title = "Delete", role = "destructive", action = act() }, { title = "Cancel", role = "cancel", action = act() } },
                dismissAction = act(),
            }) },
        } },
        { "ConfirmationDialog", "confirmationDialog", {
            { "presented false; not shown", put("ConfirmationDialog", nil, {
                title = "Discard?", presented = false,
                buttons = { { title = "Discard", role = "destructive", action = act() } },
                dismissAction = act(),
            }) },
        } },
        { "NavigationSplitView", "NavigationSplitView", {
            -- The sidebar List carries a frame: a vertical List with none would make the surface fill the pane, and the gallery would stop scrolling.
            { "sidebar and detail", put("NavigationSplitView", { put("List", { txt("one"), txt("two") }, { modifiers = { { frame = { maxHeight = "infinity" } } } }), txt("Detail") }, { modifiers = { { frame = { height = 140 } } } }) },
        } },
        { "NavigationStack", "NavigationStack", {
            { "title", put("NavigationStack", { txt("Root") }, { title = "Stack", modifiers = { { frame = { height = 100 } } } }) },
        } },
        { "HSplitView, VSplitView", "HSplitView, VSplitView", {
            { "horizontal", put("HSplitView", { txt("left"), txt("right") }, { modifiers = { { frame = { height = 60 } } } }) },
            { "vertical", put("VSplitView", { txt("top"), txt("bottom") }, { modifiers = { { frame = { height = 100 } } } }) },
        } },
        { "Toolbar", "toolbar", {
            { "child 0 content, rest items", put("Toolbar", { txt("Content"), mi("Item") }, { placement = "automatic" }) },
        } },
        { "Chart", "Chart", {
            { "bar", put("Chart", nil, { mark = "bar", data = chart_data() }) },
            { "line", put("Chart", nil, { mark = "line", data = chart_data() }) },
        } },
        { "Map", "Map", {
            { "latitude, longitude, span, markers", put("Map", nil, { latitude = 40.7, longitude = -74.0, span = 0.2, markers = { { title = "NYC", latitude = 40.7, longitude = -74.0 } } }) },
        } },
        { "ShareLink", "ShareLink", {
            { "item", put("ShareLink", nil, { item = "https://apple.com", title = "Share" }) },
        } },
        { "PasteButton", "PasteButton", {
            { "raises action with text", put("PasteButton", nil, { action = act() }) },
        } },
    })
end

local function sample_columns(wide)
    return {
        { key = "id", label = "ID", width = 80, tree = false, wide = wide, sortable = true },
        { key = "title", label = "Title", tree = false, wide = false, sortable = true },
    }
end

local function sample_row(id, number, title, tone, view)
    local title_cell = { key = "title", text = title, view = view }
    if tone then title_cell.tone = tone end
    return { id = id, path = id, depth = 0, cells = { { key = "id", text = id .. "-" .. number, mono = true, sort = number }, title_cell } }
end

local function table_class()
    return klass("Table", "SwiftUI's Table of rows under section titles; rows carry cells, an optional fold and a context menu; the page may own the sort and the selection.", {
        { "Table", "Table", {
            { "one section, five rows; a cell with view (any component) draws it in place of its text, and its text and sort still sort the column (a Link, a Button and a ProgressView here); a column with sortable: true sorts the rows of each section when its header is clicked, by each cell's sort (a number or a string) or its text, and the rows keep the page's order until a header is clicked; every column divider can be dragged (a column with a width keeps it and can be dragged narrower, at least 40 pt; a column with no width takes what is left); with a frame height it is that tall and scrolls itself; with none it fills the pane under the rest of the page, which then stays put; on a phone it is a list of the same rows; the ID column is wide, so under 640 pt the section title moves to Title",
                put("Table", nil, {
                    rowAction = act(), foldAction = act(),
                    columns = sample_columns(true),
                    sections = { { title = "Ready", count = 5, rows = {
                        sample_row("a", 1, "First row"),
                        sample_row("b", 2, "Second row", "ok"),
                        sample_row("c", 3, "Link", nil, put("Link", nil, { title = "A Link in a cell", destination = "https://github.com" })),
                        sample_row("d", 4, "Button", nil, put("Button", nil, { action = act(), child = txt("A Button in a cell") })),
                        sample_row("e", 5, "ProgressView", nil, put("ProgressView", nil, { value = 0.6, total = 1 })),
                    } } },
                    sort = { key = "id", asc = true }, empty = "None", modifiers = { { frame = { height = 160 } } },
                }) },
            { "sortAction: the page owns the order; the header arrow shows sort {key, asc}, a click raises sortAction with the new {key, asc} and the rows stay as sent until the page replies",
                put("Table", nil, {
                    rowAction = act(), foldAction = act(), sortAction = act(),
                    columns = sample_columns(false),
                    sections = { { title = "", count = 2, rows = {
                        sample_row("b", 2, "Sent first by the page"),
                        sample_row("a", 1, "Sent second"),
                    } } },
                    sort = { key = "id", asc = false }, empty = "None", modifiers = { { frame = { height = 160 } } },
                }) },
            { "selectAction and selected: the page owns the selection; the rows whose ids are in selected draw selected, a click, a shift-click range, a ⌘-click, a click on empty ground or Escape raises selectAction with {ids} (every row selected now), and a double-click or Return raises rowAction with the row's {id}; without selectAction a plain click raises rowAction",
                put("Table", nil, {
                    rowAction = act(), foldAction = act(), sortAction = act(), selectAction = act(), selected = { "a", "c" },
                    columns = sample_columns(false),
                    sections = { { title = "", count = 3, rows = {
                        sample_row("a", 1, "Selected by the page"),
                        sample_row("b", 2, "Not selected"),
                        sample_row("c", 3, "Selected by the page"),
                    } } },
                    sort = { key = "id", asc = true }, empty = "None", modifiers = { { frame = { height = 160 } } },
                }) },
        } },
    })
end

-- ---- Canvas: a 16x16 RGBA gradient, base64 of its bytes
local ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

local function base64(bytes)
    local out = {}
    local n = #bytes
    for i = 1, n, 3 do
        local b1, b2, b3 = bytes[i], bytes[i + 1], bytes[i + 2]
        local v = b1 * 65536 + (b2 or 0) * 256 + (b3 or 0)
        local c1 = v // 262144 % 64
        local c2 = v // 4096 % 64
        local c3 = v // 64 % 64
        local c4 = v % 64
        local s = string.sub(ALPHABET, c1 + 1, c1 + 1) .. string.sub(ALPHABET, c2 + 1, c2 + 1)
        s ..= if b2 then string.sub(ALPHABET, c3 + 1, c3 + 1) else "="
        s ..= if b3 then string.sub(ALPHABET, c4 + 1, c4 + 1) else "="
        table.insert(out, s)
    end
    return table.concat(out)
end

local function canvas_gradient()
    local w, h = 16, 16
    local px = {}
    for y = 0, h - 1 do
        for x = 0, w - 1 do
            table.insert(px, x * 255 // (w - 1))
            table.insert(px, y * 255 // (h - 1))
            table.insert(px, 128)
            table.insert(px, 255)
        end
    end
    return base64(px)
end

local CANVAS_COMMANDS = {
    { "set", "fillStyle", "#33669933" }, { "rect", 0, 0, 640, 240 }, { "fill" },
    { "set", "strokeStyle", "accent" }, { "set", "lineWidth", 2 },
    { "beginPath" }, { "moveTo", 10, 30 }, { "lineTo", 60, 10 }, { "lineTo", 110, 30 }, { "stroke" },
    { "beginPath" }, { "moveTo", 130, 30 }, { "bezierCurveTo", 150, -10, 190, 70, 210, 30 }, { "stroke" },
    { "beginPath" }, { "moveTo", 230, 30 }, { "quadraticCurveTo", 260, -10, 290, 30 }, { "stroke" },
    { "set", "fillStyle", "ok" }, { "beginPath" }, { "arc", 330, 30, 20, 0, 4.2, false }, { "closePath" }, { "fill" },
    { "set", "fillStyle", "rgba(220, 60, 60, 0.8)" }, { "beginPath" }, { "arc", 390, 30, 20, 0, 4.2, true }, { "fill" },
    { "set", "fillStyle", "warn" }, { "beginPath" }, { "rect", 430, 10, 40, 40 }, { "fill" },
    { "set", "fillStyle", "purple" }, { "beginPath" }, { "roundRect", 490, 10, 60, 40, 12 }, { "fill" },
    { "set", "strokeStyle", "bad" }, { "set", "lineWidth", 1 }, { "beginPath" }, { "roundRect", 565, 10, 60, 40, 8 }, { "stroke" },
    { "save" }, { "set", "globalAlpha", 0.35 }, { "set", "fillStyle", "#f80" }, { "beginPath" }, { "rect", 20, 80, 60, 40 }, { "fill" },
    { "translate", 50, 0 }, { "beginPath" }, { "rect", 20, 80, 60, 40 }, { "fill" }, { "restore" },
    { "save" }, { "translate", 200, 100 }, { "rotate", 0.4 }, { "set", "fillStyle", "accent" }, { "beginPath" }, { "rect", -30, -15, 60, 30 }, { "fill" }, { "restore" },
    { "save" }, { "translate", 300, 80 }, { "scale", 2, 1 }, { "set", "strokeStyle", "fg" }, { "set", "lineWidth", 2 }, { "beginPath" }, { "arc", 20, 20, 15, 0, 6.2832, false }, { "stroke" }, { "restore" },
    { "set", "fillStyle", "line" }, { "beginPath" }, { "rect", 20, 150, 150, 26 }, { "fill" },
    { "set", "fillStyle", "fg" }, { "set", "font", "bold 13px sans-serif" }, { "fillText", "fillText, bold", 28, 168 },
    { "set", "font", "italic 12px serif" }, { "set", "fillStyle", "muted" }, { "fillText", "italic serif", 190, 168 },
    { "set", "font", "11px monospace" }, { "fillText", "monospace", 300, 168 },
    { "drawImage", canvas_gradient(), 16, 16, 20, 190, 16, 16 },
    { "drawImage", canvas_gradient(), 16, 16, 50, 190, 48, 48 },
    { "set", "font", "10px sans-serif" }, { "set", "fillStyle", "faint" }, { "fillText", "drawImage: 16x16 RGBA, 1x and 3x", 110, 214 },
}

local function own_class(data)
    local input = data.feed("gallery.input", {})
    local last = "no input yet"
    if type(input) == "table" and input.last then last = input.count .. " events; last: " .. input.last end
    return klass("Wheel's own component", "Only where neither the library nor SwiftUI has one.", {
        { "Canvas", "Canvas", {
            { "a display list with a camera and hit regions; needs a scene, not shown",
                secondary("Used by pages that draw graphs, such as the Beads dependency graph. A hit's tip grows to the height of its lines, and the hover outline follows the pointer when a scroll pans or zooms the canvas under it.") },
            { "commands: a Canvas 2D command list a pane script records (paths, curves, arc, rect, roundRect, styles, globalAlpha, save/restore, translate, scale, rotate, fillText, drawImage); no camera, controls, background or border, `height` points tall, as wide as it is offered; reports its size as canvas-size:<canvasKey>; pointerAction raises {kind, x, y, button, dx, dy}",
                put("Canvas", nil, { commands = CANVAS_COMMANDS, height = 240, canvasKey = "gallery" }) },
        } },
        { "Terminal", "Terminal", {
            { "pane: a terminal pane's id, height: points tall (default 320), as wide as it is offered; a pane script's host draws that pane's live terminal. Pane \"p1\" is not a terminal pane in most spaces, so the host shows its own note for it; a pane naming a live terminal pane draws it live",
                put("Terminal", nil, { pane = "p1", height = 120 }) },
        } },
        { "AgentChat", "AgentChat", {
            { "pane: an agent pane's id, height: points tall (default 320), as wide as it is offered. Pane \"p1\" is not an agent pane in most spaces, so the host shows its own note for it; a pane naming a live agent pane draws the agent's conversation and composer live",
                put("AgentChat", nil, { pane = "p1", height = 120 }) },
        } },
        { "WebView", "WebView", {
            { "url: an http or https address, height: points tall (default 320), as wide as it is offered; any other scheme shows a dashed note in place",
                put("WebView", nil, { url = "https://example.com", height = 220 }) },
            { "artifact: an artifact's name, resolved in this space as `wheel artifact` does (project, then machine); its page as a web pane shows it, reloading when its files change. \"gallery-sample\" is no artifact in most spaces, so wheeld's refusal shows in place; a space with that artifact shows its page",
                put("WebView", nil, { artifact = "gallery-sample", height = 220 }) },
        } },
        { "VideoStream", "VideoStream", {
            { "stream: <plugin>.<name>, a live H.264 or HEVC stream the plugin's backend serves (ADR-0053), height: points tall (default 320), as wide as it is offered, the picture fitted to it. input (default true): a click focuses it, and its keys, pointer and game controller events go to the backend; audio plays. The gallery's backend serves gallery.demo, a 320x180 test pattern at 30 fps with a 440 Hz tone. The phone draws a note in its place",
                put("VideoStream", nil, { stream = "gallery.demo", height = 200 }) },
            { "input = false: the picture and sound only; nothing is sent",
                put("VideoStream", nil, { stream = "gallery.demo", height = 120, input = false }) },
            { "what the backend got: its gallery.input feed", secondary(last) },
        } },
    })
end

-- ───────────────────────────── the module ─────────────────────────────

local function init(previous)
    return previous or {}
end

local function update(state, msg)
    if msg.type == "reverse" then
        local flipped = {}
        local order = state.order or { "a", "b", "c" }
        for i = #order, 1, -1 do table.insert(flipped, order[i]) end
        state.order = flipped
    end
    return state
end

local function view(state, data)
    local page = col({
        txt("Component gallery", "h1"),
        secondary("A table per class of control: the basic components with every option, the SwiftUI controls, modifiers, then Wheel's own component."),
        secondary("Updated with every change to a component or modifier. Body text is 13 pt on the Mac and the phone."),
        content_class(),
        controls_class(),
        inputs_class(),
        layout_class(state),
        nav_class(),
        system_class(),
        table_class(),
        modifiers_class(),
        own_class(data),
    }, { align = "start", modifiers = { { padding = 28 } } })
    return ui.Column{ align = "stretch", page }
end

return { init = init, update = update, view = view }
