//! Wheel's GPUI client: connect to wheeld, show its spaces, and resize panes by dragging.
//!
//! `wheel-gpui [--url ws://127.0.0.1:7420/ws] [--token-file PATH] [--space ID]`
//! Defaults match the `wheel` CLI: `WHEEL_URL` and `WHEEL_TOKEN_FILE`, else the local daemon.
use anyhow::{Result, bail};
use std::path::PathBuf;
use wheel_gpui::{daemon::*, native::Bridge, open_window};

fn main() -> Result<()> {
    let (mut url, mut token_file, mut space) = (default_url(), default_token_file(), None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match arg.as_str() {
            "--url" => url = value("--url")?,
            "--token-file" => token_file = PathBuf::from(value("--token-file")?),
            "--space" => space = Some(value("--space")?),
            other => bail!("unknown argument {other}; usage: wheel-gpui [--url URL] [--token-file PATH] [--space ID]"),
        }
    }
    let endpoint = Endpoint::new(&url, &token_file)?;
    gpui::Application::with_platform(gpui_platform::current_platform(false)).run(move |cx| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let result: Result<()> = (|| {
            let bridge = Bridge::load()?;
            open_window(cx, &bridge, endpoint, None, space, false)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("wheel-gpui: {error:#}");
            cx.quit();
        }
    });
    Ok(())
}
