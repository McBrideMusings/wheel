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
    ("Row", "HStack", [("justify " + j, row([txt("one"), txt("two"), txt("three")], justify=j)) for j in ("start", "center", "end", "spaceBetween", "spaceEvenly")] + [("spacing 24", row([txt("one"), txt("two"), txt("three")], justify="start", spacing=24))]),
    ("Column", "VStack", [("align " + a, col([txt("short"), txt("a longer line")], align=a)) for a in ("start", "center", "end")]),
    ("List", "List", [("vertical", put("List", [txt("alpha"), txt("beta"), txt("gamma")], direction="vertical")), ("horizontal", put("List", [txt("alpha"), txt("beta"), txt("gamma")], direction="horizontal"))]),
    ("Card", "GroupBox", [("one child", put("Card", child=col([txt("Inside a card", "h4"), secondary("Body text")], align="start", modifiers=[{"padding": 12}, {"frame": {"maxWidth": "infinity", "alignment": "leading"}}])))]),
    ("Divider", "Divider", [("horizontal", col([txt("above"), put("Divider", axis="horizontal"), txt("below")], align="start")), ("vertical", row([txt("left"), put("Divider", axis="vertical"), txt("right")], justify="start"))]),
])

nav = klass("Navigation and presentation", "Switch between or reveal content.", [
    ("Tabs", "TabView", [("three tabs", put("Tabs", tabs=[{"title": "One", "child": txt("First pane")}, {"title": "Two", "child": txt("Second pane")}, {"title": "Three", "child": txt("Third pane")}]))]),
    ("Modal", "sheet", [("button opens content", put("Modal", trigger=button(txt("Open")), content=col([txt("Modal title", "h3"), txt("The modal's content.")], modifiers=[{"padding": 20}]))),
                        ("presented true: dims and covers its own pane only, with a card and Close; dismissAction is raised on close; not shown", secondary("Modal with presented=true and dismissAction, children are the card's content."))]),
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

# ---- SwiftUI's own controls under their SwiftUI names (ADR-0032)
def mi(title):
    return put("Button", child=txt(title), action=act())


system = klass("SwiftUI controls", "Standard controls the library lacks, named as SwiftUI names them (ADR-0032). A property is the initializer's argument; children are its content.", [
    ("ProgressView", "ProgressView", [("value 0.4 of 1", put("ProgressView", title="Uploading", value=0.4, total=1)), ("no value: spinner", put("ProgressView", title="Working"))]),
    ("Gauge", "Gauge", [("linearCapacity", put("Gauge", title="Disk", value=40, min=0, max=100)), ("accessoryCircular", put("Gauge", title="CPU", value=70, min=0, max=100, style="accessoryCircular"))]),
    ("Menu", "Menu", [("children are items", put("Menu", [mi("Copy"), put("Divider"), mi("Delete")], title="Actions"))]),
    ("Link", "Link", [("title, destination", put("Link", title="Apple", destination="https://apple.com"))]),
    ("Form, Section", "Form, Section", [("grouped", put("Form", [put("Section", [put("Stepper", title="Count", value=3, min=0, max=10)], title="Counts")], style="grouped"))]),
    ("LabeledContent", "LabeledContent", [("value", put("LabeledContent", title="Branch", value="main")), ("children as content", put("LabeledContent", [txt("custom")], title="Child"))]),
    ("Grid, GridRow", "Grid, GridRow", [("two rows", put("Grid", [put("GridRow", [txt("a"), txt("b")]), put("GridRow", [txt("c"), txt("d")])], horizontalSpacing=16))]),
    ("Stepper", "Stepper", [("min 0, max 10, step 1", put("Stepper", title="Count", value=4, min=0, max=10, step=1))]),
    ("ColorPicker", "ColorPicker", [("value #RRGGBB", put("ColorPicker", title="Tint", value="#3478F6"))]),
    ("Popover", "popover", [("child 0 trigger, child 1 content", put("Popover", [txt("Open popover"), txt("Popover content")]))]),
    ("Alert", "alert", [("presented false; not shown", put("Alert", title="Delete?", message="This cannot be undone.", presented=False, buttons=[{"title": "Delete", "role": "destructive", "action": "act"}, {"title": "Cancel", "role": "cancel", "action": "act"}], dismissAction="act"))]),
    ("ConfirmationDialog", "confirmationDialog", [("presented false; not shown", put("ConfirmationDialog", title="Discard?", presented=False, buttons=[{"title": "Discard", "role": "destructive", "action": "act"}], dismissAction="act"))]),
    ("NavigationSplitView", "NavigationSplitView", [("sidebar and detail", put("NavigationSplitView", [put("List", [txt("one"), txt("two")]), txt("Detail")], modifiers=[{"frame": {"height": 140}}]))]),
    ("NavigationStack", "NavigationStack", [("title", put("NavigationStack", [txt("Root")], title="Stack", modifiers=[{"frame": {"height": 100}}]))]),
    ("HSplitView, VSplitView", "HSplitView, VSplitView", [("horizontal", put("HSplitView", [txt("left"), txt("right")], modifiers=[{"frame": {"height": 60}}])), ("vertical", put("VSplitView", [txt("top"), txt("bottom")], modifiers=[{"frame": {"height": 100}}]))]),
    ("Toolbar", "toolbar", [("child 0 content, rest items", put("Toolbar", [txt("Content"), mi("Item")], placement="automatic"))]),
    ("Chart", "Chart", [("bar", put("Chart", mark="bar", data=[{"x": "Mon", "y": 3, "series": "a"}, {"x": "Tue", "y": 5, "series": "a"}, {"x": "Wed", "y": 2, "series": "a"}])), ("line", put("Chart", mark="line", data=[{"x": "Mon", "y": 3, "series": "a"}, {"x": "Tue", "y": 5, "series": "a"}, {"x": "Wed", "y": 2, "series": "a"}]))]),
    ("Map", "Map", [("latitude, longitude, span, markers", put("Map", latitude=40.7, longitude=-74.0, span=0.2, markers=[{"title": "NYC", "latitude": 40.7, "longitude": -74.0}]))]),
    ("ShareLink", "ShareLink", [("item", put("ShareLink", item="https://apple.com", title="Share"))]),
    ("PasteButton", "PasteButton", [("raises action with text", put("PasteButton", action="act"))]),
])

own_table = klass("Table", "SwiftUI's Table of rows under section titles; rows carry cells, an optional fold and a context menu.", [
    ("Table", "Table", [("one section, two rows; the ID column is wide, so under 640 pt the section title moves to Title", put("Table", rowAction="act", foldAction="act",
                                                       columns=[{"key": "id", "label": "ID", "width": 80, "tree": False, "wide": True}, {"key": "title", "label": "Title", "width": None, "tree": False, "wide": False}],
                                                       sections=[{"title": "Ready", "count": 2, "rows": [
                                                           {"id": "a", "path": "a", "depth": 0, "cells": [{"key": "id", "text": "a-1", "mono": True}, {"key": "title", "text": "First row"}]},
                                                           {"id": "b", "path": "b", "depth": 0, "cells": [{"key": "id", "text": "b-2", "mono": True}, {"key": "title", "text": "Second row", "tone": "ok"}]}]}],
                                                       sort={"key": "id", "asc": True}, empty="None", modifiers=[{"frame": {"height": 160}}]))]),
])

own = klass("Wheel's own component", "Only where neither the library nor SwiftUI has one.", [
    ("Canvas", "Canvas", [("a display list with a camera and hit regions; needs a scene, not shown", secondary("Used by pages that draw graphs, such as the Beads dependency graph."))]),
])
ONLY = sys.argv[2] if len(sys.argv) > 2 else None  # a class name shows that class alone
page = col([
    txt("A2UI component gallery", "h1"),
    secondary("A table per class of control: the basic catalog's components with every option, the SwiftUI controls, modifiers, then Wheel's own component."),
    secondary("Updated with every change to a component or modifier."),
    *({"system": [system]}.get(ONLY) or [content, controls, inputs, layout, nav, system, own_table, modifiers, own]),
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
