#!/usr/bin/env python3
"""A2UI gallery: an MCP server over stdio serving one page, a table per class of control.

Every change to how the client draws a component, a property or a modifier updates this file in the same change.
Register it in the project wheel.toml: [artifacts.gallery] mcp = "python3 tools/gallery/server.py", pane = true.
"""
import json, sys

CATALOG = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
URI = "a2ui://gallery"
MIME = "application/a2ui+json"
SURFACE = "gallery"

out = []
n = [0]


def put(component, children=None, **props):
    n[0] += 1
    cid = f"c{n[0]}"
    v = {"id": cid, "component": component}
    v.update(props)
    if children is not None:
        v["children"] = children
    out.append(v)
    return cid


def col(children, **p):
    return put("Column", children, **p)


def row(children, **p):
    return put("Row", children, **p)


def txt(t, variant=None, **p):
    if variant:
        p["variant"] = variant
    return put("Text", text=t, **p)


def secondary(t, variant=None, **p):
    mods = [{"foregroundStyle": "secondary"}] + p.pop("modifiers", [])
    return txt(t, variant, modifiers=mods, **p)


def act():
    return {"event": {"name": "act", "context": {}}}


def button(child, variant=None):
    p = {"variant": variant} if variant else {}
    return put("Button", child=child, action=act(), **p)


# ---- table layout: fixed-width cells so the columns line up
W = [130, 130, 230]


def cell(content, i):
    mods = [{"frame": {"width": W[i], "alignment": "leading"}}] if i < len(W) else [{"frame": {"maxWidth": "infinity", "alignment": "leading"}}]
    return col([content], align="start", modifiers=mods + [{"padding": {"vertical": 8, "trailing": 12}}])


def header():
    cells = [cell(secondary(t, "caption", modifiers=[{"fontWeight": "semibold"}]), i) for i, t in enumerate(("Component", "SwiftUI", "Option", "Example"))]
    return col([row(cells, justify="start", align="start"), put("Divider")])


def comp_rows(name, swiftui, options):
    """One table row per option; the component's name and SwiftUI class on its first row only."""
    rows = []
    for k, (opt, ex) in enumerate(options):
        cells = [cell(txt(name if k == 0 else "", modifiers=[{"fontWeight": "semibold"}]), 0),
                 cell(secondary(swiftui if k == 0 else ""), 1),
                 cell(secondary(opt, "caption"), 2),
                 cell(ex, 3)]
        rows.append(col([row(cells, justify="start", align="start"), put("Divider")]))
    return rows


def boxed(title, blurb, body):
    """One Card around a class: its heading, its blurb and its table."""
    inner = col([txt(title, "h2"), secondary(blurb, modifiers=[{"padding": {"bottom": 8}}]), *body], align="start",
                modifiers=[{"padding": 20}, {"frame": {"maxWidth": "infinity", "alignment": "leading"}}])
    return col([put("Card", child=inner)], modifiers=[{"padding": {"top": 24}}])


def klass(title, blurb, comps):
    rows = [header()]
    for c in comps:
        rows += comp_rows(*c)
    return boxed(title, blurb, rows)


OPTS = [{"label": "All", "value": "a"}, {"label": "Open", "value": "b"}, {"label": "Closed", "value": "c"}]
icons = lambda names: row([put("Icon", name=s) for s in names], justify="start")

# ============================================================ classes
content = klass("Content", "Static text and images. Apple's guidelines call read-only text a label: SwiftUI Text, AppKit NSTextField.", [
    ("Text", "Text", [(v, txt(("Heading " + v[1]) if v.startswith("h") else v.capitalize() if v == "body" else "Caption", v)) for v in ("h1", "h2", "h3", "h4", "h5", "body", "caption")]),
    ("Icon", "Image(systemName:)", [("SF Symbol names", icons(["play.fill", "stop.fill", "xmark", "checkmark.circle.fill", "xmark.circle.fill", "gearshape"])),
                                      ("material names", icons(["check", "close", "add", "delete", "edit", "home"]))]),
    ("Image, Video, AudioPlayer", "Image, VideoPlayer", [("each needs a URL; not shown", secondary("Image variants: icon, avatar, smallFeature, mediumFeature, largeFeature, header"))]),
])

controls = klass("Buttons", "Run an action. The label is a child component.", [
    ("Button", "Button", [("variant default", button(txt("Default"))), ("variant primary", button(txt("Primary"), "primary")), ("variant borderless", button(txt("Borderless"), "borderless")),
                          ("label is an Icon", button(put("Icon", name="play.fill"))),
                          ("label is Icon and Text", button(row([put("Icon", name="play.fill"), txt("Run build")], justify="start")))]),
])

inputs = klass("Selection and input", "Values the person sets.", [
    ("TextField", "TextField, SecureField, TextEditor", [("shortText", put("TextField", label="Name", value="", variant="shortText")), ("longText", put("TextField", label="Notes", value="", variant="longText")),
                                                          ("number", put("TextField", label="Count", value="12", variant="number")), ("obscured", put("TextField", label="Token", value="secret", variant="obscured"))]),
    ("CheckBox", "Toggle", [("off", put("CheckBox", label="Show closed", value=False)), ("on", put("CheckBox", label="Show closed", value=True))]),
    ("ChoicePicker", "Picker", [("mutuallyExclusive", put("ChoicePicker", label="Filter", options=OPTS, value=["b"], variant="mutuallyExclusive")),
                                ("multipleSelection", put("ChoicePicker", label="Filter", options=OPTS, value=["a", "c"], variant="multipleSelection")),
                                ("displayStyle checkbox", put("ChoicePicker", label="Filter", options=OPTS, value=["a"], variant="multipleSelection", displayStyle="checkbox")),
                                ("displayStyle chips", put("ChoicePicker", label="Filter", options=OPTS, value=["a"], variant="multipleSelection", displayStyle="chips")),
                                ("filterable", put("ChoicePicker", label="Filter", options=OPTS, value=["a"], variant="mutuallyExclusive", filterable=True))]),
    ("Slider", "Slider", [("min 0, max 10, value 4", put("Slider", label="Volume", value=4, min=0, max=10))]),
    ("DateTimeInput", "DatePicker", [("date", put("DateTimeInput", label="Due", value="2026-11-06", enableDate=True, enableTime=False)),
                                      ("time", put("DateTimeInput", label="At", value="2026-11-06T14:30:00", enableDate=False, enableTime=True)),
                                      ("date and time", put("DateTimeInput", label="When", value="2026-11-06T14:30:00", enableDate=True, enableTime=True))]),
])

layout = klass("Containers and layout", "Arrange other components.", [
    ("Row", "HStack", [("justify " + j, row([txt("one"), txt("two"), txt("three")], justify=j)) for j in ("start", "center", "end", "spaceBetween", "spaceEvenly")]),
    ("Column", "VStack", [("align " + a, col([txt("short"), txt("a longer line")], align=a)) for a in ("start", "center", "end")]),
    ("List", "List", [("vertical", put("List", [txt("alpha"), txt("beta"), txt("gamma")], direction="vertical")), ("horizontal", put("List", [txt("alpha"), txt("beta"), txt("gamma")], direction="horizontal"))]),
    ("Card", "GroupBox", [("one child", put("Card", child=col([txt("Inside a card", "h4"), secondary("Body text")], align="start", modifiers=[{"padding": 12}, {"frame": {"maxWidth": "infinity", "alignment": "leading"}}])))]),
    ("Divider", "Divider", [("horizontal", col([txt("above"), put("Divider", axis="horizontal"), txt("below")], align="start")), ("vertical", row([txt("left"), put("Divider", axis="vertical"), txt("right")], justify="start"))]),
])

nav = klass("Navigation and presentation", "Switch between or reveal content.", [
    ("Tabs", "TabView", [("three tabs", put("Tabs", tabs=[{"title": "One", "child": txt("First pane")}, {"title": "Two", "child": txt("Second pane")}, {"title": "Three", "child": txt("Third pane")}]))]),
    ("Modal", "sheet", [("button opens content", put("Modal", trigger=button(txt("Open")), content=col([txt("Modal title", "h3"), txt("The modal's content.")], modifiers=[{"padding": 20}])))]),
])


# ---- modifiers table: modifier, arguments, example
def mod(t, *mods):
    return txt(t, modifiers=list(mods))


MW = [150, 250]


def mcell(content, i):
    mods = [{"frame": {"width": MW[i], "alignment": "leading"}}] if i < len(MW) else [{"frame": {"maxWidth": "infinity", "alignment": "leading"}}]
    return col([content], align="start", modifiers=mods + [{"padding": {"vertical": 8, "trailing": 12}}])


def mrow(name, args, ex):
    return col([row([mcell(txt(name, modifiers=[{"fontWeight": "semibold"}]), 0), mcell(secondary(args, "caption"), 1), mcell(ex, 2)], justify="start", align="start"), put("Divider")])


mrows = [col([row([mcell(secondary(t, "caption", modifiers=[{"fontWeight": "semibold"}]), i) for i, t in enumerate(("Modifier", "Arguments", "Example"))], justify="start", align="start"), put("Divider")]),
         mrow("font", "a text style (caption, headline...) or {size, weight, design}", mod("monospaced 13", {"font": {"size": 13, "design": "monospaced"}})),
         mrow("fontWeight", "light ... heavy", mod("semibold", {"fontWeight": "semibold"})),
         mrow("monospacedDigit", "true", col([mod("1111", {"monospacedDigit": True}), mod("8888", {"monospacedDigit": True})], align="start")),
         mrow("tracking, textCase", "points; uppercase or lowercase", mod("heading", {"tracking": 2}, {"textCase": "uppercase"}, {"font": "caption"})),
         mrow("lineLimit", "a number", mod("A long line of text that is cut off where it stops fitting", {"lineLimit": 1})),
         mrow("multilineTextAlignment", "leading, center, trailing", mod("Right-aligned across a wide frame", {"multilineTextAlignment": "trailing"}, {"frame": {"maxWidth": "infinity", "alignment": "trailing"}})),
         mrow("textSelection", "true", mod("Select this text", {"textSelection": True})),
         mrow("foregroundStyle", "primary, secondary, tertiary, red, orange, yellow, green, mint, teal, cyan, blue, indigo, purple, pink, brown, gray, black, white, tint", row([mod(c, {"foregroundStyle": c}) for c in ("red", "orange", "green", "blue", "purple", "tint")], justify="start")),
         mrow("background", "a colour name, or separator, windowBackground, controlBackground, textBackground", mod("controlBackground", {"padding": 8}, {"background": "controlBackground"})),
         mrow("padding", "n, [vertical, horizontal], [top, right, bottom, left], {horizontal, vertical, top, ...}", col([mod("padding 12", {"padding": 12}, {"overlay": {"stroke": "separator"}}), mod("[4, 16]", {"padding": [4, 16]}, {"overlay": {"stroke": "separator"}})], align="start")),
         mrow("frame", "{width, height, minWidth, maxWidth, alignment}; maxWidth may be infinity", mod("width 120", {"frame": {"width": 120, "alignment": "leading"}}, {"overlay": {"stroke": "separator"}})),
         mrow("overlay", "{stroke: colour, cornerRadius}", mod("outlined, radius 8", {"padding": 10}, {"overlay": {"stroke": "separator", "cornerRadius": 8}})),
         mrow("clipShape", "{cornerRadius}", mod("clipped, radius 12", {"padding": 10}, {"background": "controlBackground"}, {"clipShape": {"cornerRadius": 12}})),
         mrow("opacity", "0 to 1", mod("faded", {"opacity": 0.4})),
         mrow("disabled", "true", put("Button", child=txt("Disabled button"), action=act(), modifiers=[{"disabled": True}])),
         mrow("help, accessibilityLabel", "a string", put("Button", child=put("Icon", name="play.fill"), action=act(), modifiers=[{"help": "Run build"}, {"accessibilityLabel": "Run build"}])),
         mrow("(unknown name)", "draws a red marker", mod("bad modifier", {"frobnicate": 1}))]
modifiers = boxed("Modifiers", "A modifiers list on any component; each entry is a SwiftUI modifier by its SwiftUI name.", mrows)

# ---- Wheel's own and planned
own = klass("Wheel's own component", "Only where neither the library nor SwiftUI has one.", [
    ("Canvas", "Canvas", [("a display list with a camera and hit regions; needs a scene, not shown", secondary("Used by pages that draw graphs, such as the Beads dependency graph."))]),
])
planned = klass("Planned", "Epic wheel-cmbu.20. Components named as SwiftUI names them.", [
    ("wheel-cmbu.20.3", "to add", [("controls", txt("ProgressView, Gauge, Menu, Link, Form, Section, LabeledContent, Grid, Stepper, ColorPicker"))]),
    ("wheel-cmbu.20.4", "to add", [("presentation and navigation", txt("Popover, Alert, ConfirmationDialog, NavigationSplitView, NavigationStack, HSplitView, VSplitView, Toolbar, Chart, Map, ShareLink, PasteButton"))]),
    ("wheel-cmbu.20.5", "to replace", [("Wheel's Table, Disclosure, ProgressBar", txt("SwiftUI Table, DisclosureGroup, ProgressView"))]),
    ("wheel-cmbu.20.8", "to delete", [("until pages stop using them", txt("Check, Select, Segmented, SearchField, Sheet, Label, Box")),
                                      ("Pill", txt("deleted: every page sends the library's Button (branch refactor/wheel-cmbu.20-button-not-pill, not installed yet)"))]),
])

page = col([
    txt("A2UI component gallery", "h1"),
    secondary("A table per class of control: the A2UI library's components with every option, then modifiers, then the one Wheel component and what is planned."),
    secondary("Updated with every change to a component or modifier. The disabled modifier needs the client built from refactor/wheel-cmbu.20-button-not-pill."),
    content, controls, inputs, layout, nav, modifiers, own, planned,
], align="start", modifiers=[{"padding": 28}])
out.append({"id": "root", "component": "Column", "align": "stretch", "children": [page]})

PAGE = json.dumps([
    {"version": "v0.9", "createSurface": {"surfaceId": SURFACE, "catalogId": CATALOG}},
    {"version": "v0.9", "updateComponents": {"surfaceId": SURFACE, "components": out}},
])
EMPTY = json.dumps([
    {"version": "v0.9", "createSurface": {"surfaceId": SURFACE, "catalogId": CATALOG}},
    {"version": "v0.9", "updateComponents": {"surfaceId": SURFACE, "components": [
        {"id": "root", "component": "Column", "align": "stretch", "children": ["page"]},
        {"id": "page", "component": "Column", "children": []}]}},
])
META = {"ui": {"resourceUri": URI, "mimeType": MIME}}


def reply(i, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": i, "result": result}) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method, i = msg.get("method"), msg.get("id")
    if i is None:
        continue
    if method == "initialize":
        reply(i, {"protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                  "capabilities": {"tools": {}, "resources": {"subscribe": True}},
                  "serverInfo": {"name": "gallery", "version": "0"}})
    elif method == "tools/list":
        reply(i, {"tools": [
            {"name": "get_gallery", "description": "The gallery page", "inputSchema": {"type": "object", "properties": {}}, "_meta": META},
            {"name": "act", "description": "Any control on the gallery: redraws it", "inputSchema": {"type": "object", "additionalProperties": True}, "_meta": META}]})
    elif method == "resources/list":
        reply(i, {"resources": [{"uri": URI, "name": "gallery", "mimeType": MIME}]})
    elif method == "resources/read":
        reply(i, {"contents": [{"uri": URI, "mimeType": MIME, "text": EMPTY}]})
    elif method == "resources/subscribe":
        reply(i, {})
    elif method == "tools/call":
        text = PAGE
        reply(i, {"content": [{"type": "resource", "resource": {"uri": URI, "mimeType": MIME, "text": text}}], "isError": False})
    else:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": i, "error": {"code": -32601, "message": "no such method"}}) + "\n")
        sys.stdout.flush()
