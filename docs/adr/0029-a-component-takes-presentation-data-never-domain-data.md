# ADR-0029: A component takes presentation data, never domain data

A component in the client's catalog (the A2UI components a plugin's server lays out, ADR-0026) takes presentation data: rows and columns, shapes and paths, text, styles, and the actions a tap raises. It never takes a plugin's domain data (beads, lanes, statuses), and no client code knows what a plugin's data means. The plugin decides what a thing looks like and sends the result.

The canvas component takes a display list in the canvas-2D vocabulary (`path`, `fill`, `stroke`, `text`, `image`, `clip`, `transform`), which SwiftUI's `Canvas` and the web's `<canvas>` both draw, plus the three things a canvas lacks: a camera the client pans and zooms without asking the server, hit regions that carry an id, an action and an accessibility label, and a zoom level below which a shape is not drawn. The table component takes columns and rows. Layout, colour, labels and arrowheads are the plugin server's.

A new kind of surface is a new generic component, never a component named for one plugin.
