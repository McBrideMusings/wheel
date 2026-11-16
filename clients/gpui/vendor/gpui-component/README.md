# gpui-component (vendored)

Vendored from crates.io `gpui-component` 0.5.1 (Apache-2.0, see `LICENSE-APACHE`), patched to compile against Zed's `gpui-embedded-in-gpui` branch of `gpui`, which is the GPUI the Wheel client pins. `Cargo.toml` points `gpui` and `gpui-macros` at that git branch.

API changes adapted:

- `gpui::Corner` became `gpui::Anchor` (eight variants); exhaustive matches gained the centre variants.
- `Bounds::from_corner_and_size` became `Bounds::from_anchor_and_size`.
- `Anchor::other_side_corner_along` became `other_side_along`.
- `FocusHandle::focus`, `Window::focus`, `Window::focus_next` and `Window::focus_prev` take `cx`.
- `Styled::flex_grow()` and `flex_shrink()` became `flex_grow_1()` and `flex_shrink_1()`.
- `ShapedLine`/`WrappedLine` `paint` takes an alignment and an alignment width.
- `BoxShadow` has an `inset` field.
- `Corners::all` needs `Half`, so `Corners<bool>` is written out.
- `gpui::Timer` is gone; `smol::Timer` replaces it.
- `Entity::update` on an async context returns the value, not a `Result`.
- `ScrollHandle::max_offset` returns a `Point`, so content size adds `max_offset().into()` to the viewport size.
- `UniformList::track_scroll` takes a reference.
- `TextStyleRefinement` is no longer wrapped in an `Option` in `StyleRefinement`.
