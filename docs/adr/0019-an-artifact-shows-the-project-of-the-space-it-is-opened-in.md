# ADR-0019: An artifact shows the project of the space it is opened in

An artifact pane shows the project of its own space, read from the space's directory: its refresh command runs there with `WHEEL_SPACE_DIR`, and resolves a worktree to its repo. A machine artifact is one page and one command for every space, and each space gets its own project's data from it. No artifact shows another space's data, and no artifact has a picker, dropdown or setting for anything the space's directory already says. Any other choice a page would offer is a default the page takes from that context.
