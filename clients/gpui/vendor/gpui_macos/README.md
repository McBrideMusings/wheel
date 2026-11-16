# gpui_macos

Copied from Zed's `gpui-embedded-in-gpui` branch at 7bc1c05 (`crates/gpui_macos`, with the repository's `LICENSE-APACHE`). `Cargo.toml` gives every `workspace = true` entry its concrete value from Zed's root `Cargo.toml`.

The only source change is the lock in `validate_menu_item` (`src/platform.rs:1397`). Upstream drops the guard only when the menu item has a GPUI action; otherwise it locks again while still holding the guard, and the main thread waits on itself. The guard is now dropped before the second lock on every path.
