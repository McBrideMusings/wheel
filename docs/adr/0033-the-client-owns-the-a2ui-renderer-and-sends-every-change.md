# ADR-0033: The client owns the A2UI renderer and sends every value a control writes

The client's page model (`SurfaceModel`) and the 18 basic components are Wheel's own code, not a package. The server sends `createSurface`, `updateComponents` and `updateDataModel`; the client draws the tree and keeps a data model of values.

A control that holds a value reads and writes the data model at the path its `value` is bound to (`{"path": "/q"}`), or at `/<id>` when it is not. A write shows at once, and the client calls the server's change tool, `data_changed` unless `createSurface` names another with `changeTool`, with `{path, value}` 200 ms after the last write to that path. A Button raises its action as before. A control knows nothing of either call.

The server owns every decision: it answers a change by sending the components or data that change implies. A server write to the data model raises no change. Adding a control to the client adds a name-to-SwiftUI mapping and no message.
