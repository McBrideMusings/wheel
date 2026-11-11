//! Wheel's GPUI client: connect to wheeld, show its spaces, and resize panes by dragging.
//!
//! `wheel-gpui [--url ws://127.0.0.1:7420/ws] [--token-file PATH] [--space ID] [--hidden]`
//! Defaults match the `wheel` CLI: `WHEEL_URL` and `WHEEL_TOKEN_FILE`, else the local daemon.
//! `WHEEL_DEBUG_PORT` starts the debug server (`debug.rs`) on that port; `--hidden` (or
//! `WHEEL_HIDDEN=1`) opens the window in test mode: never shown, never key, never active.
use anyhow::{Result, bail};
use std::path::PathBuf;
use wheel_gpui::{daemon::*, native::Bridge, open_window};

fn main() -> Result<()> {
    let (mut url, mut token_file, mut space) = (default_url(), default_token_file(), None);
    let mut hidden = std::env::var("WHEEL_HIDDEN").is_ok_and(|v| v == "1");
    let debug_port = std::env::var("WHEEL_DEBUG_PORT").ok().map(|p| p.parse::<u16>().map_err(|_| anyhow::anyhow!("WHEEL_DEBUG_PORT must be a port number, not {p:?}"))).transpose()?;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match arg.as_str() {
            "--url" => url = value("--url")?,
            "--token-file" => token_file = PathBuf::from(value("--token-file")?),
            "--space" => space = Some(value("--space")?),
            "--hidden" => hidden = true,
            other => bail!("unknown argument {other}; usage: wheel-gpui [--url URL] [--token-file PATH] [--space ID] [--hidden]"),
        }
    }
    let endpoint = Endpoint::new(&url, &token_file)?;
    let debug = debug_port.map(|port| wheel_gpui::debug::start(port, &token_file)).transpose()?;
    if let Some(debug) = &debug {
        eprintln!("wheel-gpui: debug state at http://127.0.0.1:{}/state", debug.port);
    }
    gpui::Application::with_platform(gpui_platform::current_platform(hidden)).run(move |cx| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let result: Result<()> = (|| {
            if let Some(crash) = wheel_gpui::crash_guard::start(&wheel_gpui::crash_guard::default_marks_file(), &wheel_gpui::crash_guard::default_reports()) {
                eprintln!("wheel-gpui: the last run quit while drawing {:?} / {:?}; holding it out", crash.space, crash.pane);
            }
            let bridge = Bridge::load()?;
            let window = open_window(cx, &bridge, endpoint, None, space, hidden)?;
            if let Some(debug) = debug {
                wheel_gpui::serve_debug(cx, window, debug);
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("wheel-gpui: {error:#}");
            cx.quit();
        }
    });
    Ok(())
}
