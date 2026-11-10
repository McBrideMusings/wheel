//! Wheel's GPUI client: connect to wheeld, show its spaces, and resize panes by dragging.
//!
//! `wheel-gpui [--url ws://127.0.0.1:7420/ws] [--token-file PATH] [--space ID]`
//! Defaults match the `wheel` CLI: `WHEEL_URL` and `WHEEL_TOKEN_FILE`, else the local daemon.
use anyhow::{Result, bail};
use futures::StreamExt as _;
use std::path::PathBuf;
use wheel_gpui::{daemon::*, layout::parse_spaces, native::Native, open_window};

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
        cx.spawn(async move |cx| {
            let result: Result<()> = async {
                let native = Native::load()?;
                let mut connection = Connection::open(&endpoint)?;
                let window = open_window(cx, &native, connection.send.clone(), endpoint, space, false)?;
                while let Some(message) = connection.incoming.next().await {
                    if let Some(spaces) = parse_spaces(&message) {
                        window.update(cx, |client, _, cx| {
                            client.apply(spaces);
                            cx.notify();
                        })?;
                    }
                }
                bail!("the connection to wheeld closed")
            }
            .await;
            if let Err(error) = result {
                eprintln!("wheel-gpui: {error:#}");
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}
