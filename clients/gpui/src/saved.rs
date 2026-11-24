//! The windows file's writer, wired to GPUI's quit and to the app's list of windows. The file's
//! format and the writer's state are `wheel_gpui_core::saved`.
pub use wheel_gpui_core::saved::*;
use std::path::PathBuf;

/// Start writing the windows to `file` after every change, once per app; `restore` is the
/// setting as the file had it.
pub fn start(cx: &mut gpui::App, file: Option<PathBuf>, restore: bool) {
    if !begin(file, restore) {
        return;
    }
    // GPUI runs the quit observers, then drops every window: the drop must not empty the file.
    cx.on_app_quit(|_| {
        save();
        stop();
        async {}
    })
    .detach();
    let mut changes = crate::windows::with(|r| r.watch());
    cx.spawn(async move |_| {
        use futures::StreamExt as _;
        while changes.next().await.is_some() {
            // A drag moves the window many times a frame apart: one write for every change waiting.
            while let Ok(Some(())) = changes.try_next() {}
            save();
        }
    })
    .detach();
}
