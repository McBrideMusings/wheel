# ADR-0027: A pane is in the grid, popped out or backgrounded

wheeld holds, for each space, three places a pane can be: the grid (`root`, one split tree), popped out (`popped`, a list of split trees, each a floating window over the grid and splittable like the grid), and backgrounded (`backgrounded`, the space's tray: one pane leaf each, running and shown nowhere). A pane is in exactly one. `pane_set_site` moves a pane between them keeping its id, process and scrollback; a popped tree that loses its last pane is gone; a pane returning to the grid goes where `panes.placement` puts a split with no side, and takes the place of a grid that holds only one empty pane. Backgrounding is per pane, even out of a popped tree.

The tray holds leaves, not bare ids, because a client draws a tray entry from the pane's kind and status. A backgrounded pane keeps reporting: its status counts toward its space's, so tab status, the menu bar item and phone push fire as for any pane, and it is never moved by needing the person.

Where a popped window sits and how big it is, and where the tray sits, belong to each client (ADR-0022); wheeld holds no geometry. The protocol version rose with the new fields and op (ADR-0020).
