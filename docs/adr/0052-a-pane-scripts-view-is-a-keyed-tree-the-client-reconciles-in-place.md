# ADR-0052: A pane script's view is a keyed tree the client reconciles in place

A plugin pane's `view` returns a tree of WheelUI components (ADR-0041), and each client draws it by reconciling it into the tree it already holds: WheelUI on the iPhone, GPUI on the Mac (ADR-0056). Each node's identity is its parent's identity and its `key` property, or, without one, its component name and its place among siblings of that name. A node whose identity survives keeps its view and its local state (a field's draft, a scroll position, a hover); only a property that differs from the last render is assigned, and only the views reading it are drawn again. A node that leaves is dropped at once. A script gives `key` to items of a list that can grow, shrink or reorder.

The tree never becomes a message. No component list, no data model, no ids invented for the wire and no action names: a control raises the table the script wrote with its event fields added, straight to `update`. No message format carries the view: there is no `createSurface`, `updateComponents` or `updateDataModel`.

A script's view is fine-grained: it declares `ui.state` values, `view` runs once, and a property given a state or a function of states is bound to it. When `update` or a feed sets a state, only the functions that read it run again, and each writes its one property on its one node; `ui.each(state, key, fn)` keeps a list's items by key. No event runs the whole `view` again.

Hover, pressed and focus are the client's: every control draws its platform's own feedback for them with no call into the script, and a script turns hover feedback off with `hoverEffect = "none"`.
