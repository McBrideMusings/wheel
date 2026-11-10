# ADR-0038: The page decides layout and style; the client draws what it says and adds no defaults

A fix to how a page looks or lays out starts in the page's renderer (the plugin server that sends the A2UI components and modifiers, ADR-0026, ADR-0033), not in the client. The client's job is to map each component and modifier a page names to its SwiftUI counterpart and apply it, so a page can say anything SwiftUI can say. The client holds no layout or style decision of its own beyond SwiftUI's native defaults.

The client therefore has no width thresholds (a pane under 640 pt turning a picker into a menu or dropping table columns), no forced frames (a vertical stack always `maxWidth: .infinity`), no hidden spacers, no fixed fonts, paddings, fills or borders on a control, and no fallback that picks one look over another. A control draws as the one SwiftUI control its `displayStyle` or variant names. A choice between two looks by available width is the page's: it sends a `ViewThatFits` (SwiftUI's own name, ADR-0031) holding both, in order.

When a page cannot express what it needs, the client gains the generic piece, named as SwiftUI names it (`layoutPriority`, `fixedSize`, `ViewThatFits`, a missing `frame` argument), and every page has it. A change to the client for one page's look, or a default that a page cannot undo, is a defect.

Before editing `apple/WheelUI/Sources/WheelUI/A2UI/` for a look or layout fix, an agent answers in one line: what the page sent, what the client did with it, and which SwiftUI piece the page could not name. If every piece was nameable, the fix is in the page.

This limits ADR-0020's "work goes in the client unless it needs the server": that rule decides between a client and the server for work the client owns, and the look of a page the server lays out is not that work.
