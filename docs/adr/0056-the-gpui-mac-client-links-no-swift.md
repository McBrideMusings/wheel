# ADR-0056: The GPUI Mac client links no Swift

The GPUI client in `clients/gpui` is Rust only: it links no Swift and nothing `xcodebuild` makes, so a Mac deploy builds with cargo alone. `apple/` holds the iOS app and the packages it uses (WheelKit, WheelUI, WheelLuau); there is no `apple/Mac`, and the files the Mac build needs (`embed-wheeld.sh`, `wheel-server.sh`, the app and server icons) live under `clients/gpui`.

- **Terminal:** GPUI draws it from `ghostty-vt`, the crate `wheeld` already uses, and owns its look (font, colours, padding).
- **Web and browser panes:** `WKWebView`s made from Rust through `objc2-web-kit`, placed as native children of the GPUI window.
- **Pane scripts:** Luau runs in the GPUI process through `mlua` with its `luau` feature, one sandboxed VM per pane with ADR-0041's budget and memory limit, and GPUI reconciles and draws the keyed tree of ADR-0052 itself. Components and modifiers keep their WheelUI and SwiftUI names (ADR-0031), so one script draws alike on the Mac and the iPhone, and an embedded agent chat is the GPUI agent view.
- **`VideoStream`:** a base component on ADR-0053's generic socket; the client decodes it in Rust with VideoToolbox through `objc2`.
- **Permission cards and the menu bar item:** GPUI draws the cards, and the `NSStatusItem` and its icon are made in Rust through `objc2-app-kit`.
