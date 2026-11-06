# ADR-0031: A page uses the system's controls and a modifiers list; Wheel adds no wrapper components

The client draws the components of the A2UI library (`a2ui-swift`) and SwiftUI's own controls under their SwiftUI names (`Text`, `Button`, `Toggle`, `Picker`, `Table`, `DisclosureGroup`, `ProgressView`, `Menu`, `Form`, and the rest). A page styles any of them with a `modifiers` list whose entries are SwiftUI modifier names with SwiftUI's own arguments (`font`, `foregroundStyle`, `padding`, `frame`, `background`, `overlay`). One mapper in the client applies each entry in order; a name it does not map draws a visible marker, never nothing.

Wheel adds a component only when neither the library nor SwiftUI has one. Today that is `Canvas`, a display list the client pans and zooms. A new component needs an ADR that names the library and SwiftUI components it checked, written before the code.

A component never exists to reproduce another surface's look, to carry a style a modifier can express, or to hold one page's layout. A page that needs a look the modifiers cannot express asks for the modifier, which every page then has.

This limits ADR-0029: a component still takes presentation data, never domain data, and a new kind of surface is still a generic component, but only after the check above.
