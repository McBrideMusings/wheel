# ADR-0039: The GPUI Mac client is its own Cargo workspace

`clients/gpui` is a Rust and GPUI client for macOS that speaks the same WebSocket protocol as the iOS app. It is its own Cargo workspace, so the daemon's workspace and `Cargo.lock` never depend on GPUI and `wheeld` depends on no client (ADR-0020). It draws every pane kind itself, with the native views ADR-0056 names placed as children of its window.
