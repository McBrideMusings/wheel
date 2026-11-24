# ADR-0001: Native clients over a Rust daemon

Every piece of state — terminals and their screens, agent conversations, spaces, layouts and permission requests — lives in one Rust daemon, `wheeld`, shipped as a single binary. Clients hold no state the daemon does not; they attach over a WebSocket and render what it sends. The shipping clients are a Rust and GPUI app for macOS (ADR-0040) and a SwiftUI app for iOS. A client for any other platform is a new client speaking the same protocol.
