# ADR-0001: Native Swift clients over a Rust daemon

Every piece of state — terminals and their screens, ACP sessions, spaces, layouts and permission requests — lives in one Rust daemon, `wheeld`, shipped as a single binary. The clients are SwiftUI apps for macOS and iOS that share Swift packages and hold no state the daemon does not; they attach over a WebSocket and render what it sends. A client for any other platform is a new client speaking the same protocol.
