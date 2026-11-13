//! Wheel's GPUI client: connect to wheeld, show its spaces, and resize panes by dragging.
//!
//! `wheel-gpui [--url ws://127.0.0.1:7420/ws] [--token-file PATH] [--space ID] [--hidden]`
//! Defaults match the Mac app: the wheeld this bundle carries (`Resources/WheelDaemon.plist`,
//! which the app copies out and runs as a launch agent at launch) unless `WHEEL_URL` names another
//! daemon, then `WHEEL_URL` and `WHEEL_TOKEN_FILE` as the `wheel` CLI reads them, else the local daemon.
//! `WHEEL_DEBUG_PORT` starts the debug server (`debug.rs`) on that port; `--hidden` (or
//! `WHEEL_HIDDEN=1`) opens the window in test mode: never shown, never key, never active.
use anyhow::{Result, bail};
use std::path::PathBuf;
use wheel_gpui::{daemon::*, native::Bridge};

fn main() -> Result<()> {
    let (mut url, mut token_file, mut space) = (None::<String>, std::env::var_os("WHEEL_TOKEN_FILE").map(PathBuf::from), None);
    let mut hidden = std::env::var("WHEEL_HIDDEN").is_ok_and(|v| v == "1");
    let debug_port = std::env::var("WHEEL_DEBUG_PORT").ok().map(|p| p.parse::<u16>().map_err(|_| anyhow::anyhow!("WHEEL_DEBUG_PORT must be a port number, not {p:?}"))).transpose()?;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| anyhow::anyhow!("{name} needs a value"));
        match arg.as_str() {
            "--url" => url = Some(value("--url")?),
            "--token-file" => token_file = Some(PathBuf::from(value("--token-file")?)),
            "--space" => space = Some(value("--space")?),
            "--hidden" => hidden = true,
            other => bail!("unknown argument {other}; usage: wheel-gpui [--url URL] [--token-file PATH] [--space ID] [--hidden]"),
        }
    }
    // The bundled wheeld only when neither `--url` nor `WHEEL_URL` names another daemon: a window
    // over another daemon never registers or restarts this bundle's launch agent.
    let service = (url.is_none() && std::env::var_os("WHEEL_URL").is_none()).then(wheel_gpui::server::Service::bundled).flatten();
    let endpoint = match service {
        Some(service) => {
            eprintln!("wheel-gpui: this app carries {} ({} on {}), source {}", service.label, service.instance, service.listen, service.bundled_source.as_deref().unwrap_or("none recorded"));
            Endpoint::for_service(service, token_file.clone())?
        }
        None => Endpoint::new(&url.unwrap_or_else(default_url), &token_file.unwrap_or_else(default_token_file))?,
    };
    let token_file = endpoint.token_file().to_path_buf();
    let debug = debug_port.map(|port| wheel_gpui::debug::start(port, &token_file)).transpose()?;
    if let Some(debug) = &debug {
        eprintln!("wheel-gpui: debug state at http://127.0.0.1:{}/state", debug.port);
    }
    // Closing the last window leaves the app running with its menu bar item (GPUI's default on
    // macOS quits only when asked); the Dock icon then opens a window.
    let app = gpui::Application::with_platform(gpui_platform::current_platform(hidden));
    app.on_reopen(wheel_gpui::view::reopen);
    app.run(move |cx| {
        let result: Result<()> = (|| {
            if let Some(crash) = wheel_gpui::crash_guard::start(&wheel_gpui::crash_guard::default_marks_file(), &wheel_gpui::crash_guard::default_reports()) {
                eprintln!("wheel-gpui: the last run quit while drawing {:?} / {:?}; holding it out", crash.space, crash.pane);
            }
            let bridge = Bridge::load()?;
            wheel_gpui::view::start_menu_bar(cx, &bridge, endpoint.clone(), hidden);
            let file = wheel_gpui::saved::default_file(&endpoint.ws, hidden);
            let windows = wheel_gpui::launch(cx, &bridge, endpoint, space, hidden, file)?;
            if let (Some(debug), Some(window)) = (debug, windows.last()) {
                wheel_gpui::serve_debug(cx, *window, debug);
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
