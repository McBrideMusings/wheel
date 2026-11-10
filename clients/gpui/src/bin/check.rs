//! Hidden regression check for the client, driven by tools/check.py against a fixture wheeld:
//! layout from the daemon, native views on their panes, a divider drag with real NSEvents the
//! GPUI view receives, the daemon keeping the ratio, and following another client's change.
//! The window is never visible, key or active.
use anyhow::{Result, ensure};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use wheel_gpui::{daemon::*, layout::{Rect, parse_spaces}, native::Native, open_window, view::{Client, TITLE}};

async fn tick(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}
async fn wait(native: &Native, what: &str, pred: impl Fn(&Value) -> bool, cx: &mut AsyncApp) -> Result<Value> {
    for _ in 0..200 {
        let s = native.status();
        if pred(&s) {
            return Ok(s);
        }
        tick(cx, 100).await;
    }
    anyhow::bail!("timed out waiting for {what}: {}", native.status())
}
fn draw(window: gpui::WindowHandle<Client>, path: Option<PathBuf>, cx: &mut AsyncApp) -> Result<()> {
    let window: gpui::AnyWindowHandle = window.into();
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        if let Some(path) = path {
            window.render_to_image()?.save(path)?;
        }
        anyhow::Ok(())
    })??;
    Ok(())
}
/// The native views' own pixels: WKWebView's snapshot and Ghostty's IOSurface.
async fn shot(native: &Native, directory: &std::path::Path, cx: &mut AsyncApp) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    native.call(json!({"op":"snapshot","directory":directory}));
    wait(native, "WK snapshot", |s| s["snapshot"] != "pending", cx).await?;
    Ok(())
}
fn rects(status: &Value) -> Vec<[f64; 4]> {
    status["frames_top"].as_array().unwrap().iter().map(|f| [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap())).collect()
}
fn near(a: [f64; 4], r: Rect) -> bool {
    (a[0] - f64::from(r.x)).abs() < 1.0 && (a[1] - f64::from(r.y + TITLE)).abs() < 1.0 && (a[2] - f64::from(r.w)).abs() < 1.0 && (a[3] - f64::from(r.h - TITLE)).abs() < 1.0
}

async fn experiment(output: PathBuf, cx: &mut AsyncApp) -> Result<Value> {
    let native = Native::load()?;
    let endpoint = Endpoint::new(&default_url(), &default_token_file())?;
    let mut connection = Connection::open(&endpoint)?;
    let out = connection.send.clone();
    let window = open_window(cx, &native, out.clone(), endpoint, std::env::var("WHEEL_CLIENT_SPACE").ok(), true)?;
    let mut layouts = 0usize;
    // Apply every layout wheeld sends (the status reply, then the broadcast after each change).
    async fn pump(connection: &mut Connection, ms: u64, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<()> {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            while let Ok(Some(message)) = connection.incoming.try_next() {
                if let Some(spaces) = parse_spaces(&message) {
                    window.update(cx, |c, _, cx| {
                        c.apply(spaces);
                        cx.notify();
                    })?;
                    *layouts += 1;
                }
            }
            tick(cx, 50).await;
        }
        Ok(())
    }
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    ensure!(layouts > 0, "no layout received from wheeld");
    draw(window, None, cx)?;
    let snapshot = |cx: &mut AsyncApp| window.update(cx, |c, _, _| {
        let (leaves, dividers) = c.scene();
        json!({"leaves":leaves.iter().map(|p| json!({"id":p.leaf.id,"kind":p.leaf.kind,"label":p.leaf.label,"rect":[p.rect.x,p.rect.y,p.rect.w,p.rect.h],
            "native":c.native_panes.0.as_deref()==Some(&p.leaf.id)||c.native_panes.1.as_deref()==Some(&p.leaf.id)})).collect::<Vec<_>>(),
          "dividers":dividers.iter().map(|d| json!({"split":d.split,"horizontal":d.horizontal,"rect":[d.rect.x,d.rect.y,d.rect.w,d.rect.h]})).collect::<Vec<_>>()})
    });
    let first = snapshot(cx)?;
    // The client attaches the terminal and loads the page itself when it draws them.
    wait(&native, "HTML", |s| s["loads"].as_array().is_some_and(|a| !a.is_empty()), cx).await?;
    wait(&native, "terminal replay", |s| s["viewport"].as_str().is_some_and(|v| v.contains("terminal-42")), cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    draw(window, Some(output.join("client-before.png")), cx)?;
    shot(&native, &output.join("before"), cx).await?;
    let before = native.status();
    let scene = window.update(cx, |c, _, _| {
        let (leaves, _) = c.scene();
        let find = |id: &Option<String>| id.as_ref().and_then(|id| leaves.iter().find(|p| &p.leaf.id == id)).map(|p| p.rect);
        (find(&c.native_panes.0), find(&c.native_panes.1))
    })?;
    let frames = rects(&before);
    let aligned = [scene.0.map(|r| near(frames[0], r)), scene.1.map(|r| near(frames[1], r))];
    ensure!(aligned == [Some(true), Some(true)], "native views not on their leaves: {frames:?} vs {scene:?}");
    // Drag the divider between the columns with NSEvents the GPUI view receives.
    let (split, bar, parent) = window.update(cx, |c, _, _| {
        let (_, dividers) = c.scene();
        let d = dividers.iter().find(|d| d.horizontal).unwrap();
        (d.split.clone(), d.rect, d.parent)
    })?;
    let start = [f64::from(bar.x + bar.w / 2.), f64::from(bar.y + bar.h / 2.)];
    let moved = [start[0] + 140., start[1]];
    native.call(json!({"op":"post_mouse","kind":"down","point":start}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":[(start[0] + moved[0]) / 2., start[1]]}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":moved}));
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let during = native.status();
    native.call(json!({"op":"post_mouse","kind":"up","point":moved}));
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    let after_drag = snapshot(cx)?;
    let after_status = native.status();
    draw(window, Some(output.join("client-dragged.png")), cx)?;
    shot(&native, &output.join("dragged"), cx).await?;
    // The daemon holds the new ratio: ask for a fresh layout and read it.
    window.update(cx, |c, _, _| c.request_status(2))?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    let saved = window.update(cx, |c, _, _| c.tree_ratio(&split))?;
    // Another client changes the same split: this one follows the broadcast.
    let cli = std::env::var("WHEEL_CLIENT_CLI")?;
    let status = std::process::Command::new(&cli)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", &split, "0.3"])
        .status()?;
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    let external = snapshot(cx)?;
    let external_status = native.status();
    draw(window, Some(output.join("client-external.png")), cx)?;
    shot(&native, &output.join("external"), cx).await?;
    let final_status = native.status();
    ensure!(final_status["visible"] == false && final_status["key_window"] == false && final_status["app_active"] == false, "check took screen/focus");
    Ok(json!({
        "host_pid": std::process::id(), "layouts_received": layouts, "first_layout": first,
        "native_aligned_with_leaves": aligned, "native_frames_before": frames, "grid_before": before["grids"].as_array().and_then(|a| a.last()),
        "drag": {"split": split, "from": start, "to": moved, "parent": [parent.x, parent.y, parent.w, parent.h],
            "frames_during_drag": rects(&during), "frames_after_release": rects(&after_status),
            "grid_after": after_status["grids"].as_array().and_then(|a| a.last()), "layout_after": after_drag, "ratio_saved_by_wheeld": saved},
        "external_ratio_change": {"cli_exit": status.code(), "ratio_command": 0.3, "layout": external,
            "frames": rects(&external_status), "grid": external_status["grids"].as_array().and_then(|a| a.last())},
        "final": {"visible": final_status["visible"], "key_window": final_status["key_window"], "app_active": final_status["app_active"], "errors": final_status["errors"]},
    }))
}

fn main() -> Result<()> {
    let output = PathBuf::from(std::env::var("WHEEL_CLIENT_RESULTS")?);
    std::fs::create_dir_all(&output)?;
    let passed = std::sync::Arc::new(std::sync::Mutex::new(false));
    let done = passed.clone();
    gpui::Application::with_platform(gpui_platform::current_platform(true)).run(move |cx| {
        cx.spawn(async move |cx| {
            match experiment(output.clone(), cx).await {
                Ok(report) => {
                    std::fs::write(output.join("report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
                    *done.lock().unwrap() = true;
                    println!("client checks passed");
                }
                Err(error) => eprintln!("client check failed: {error:#}"),
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    ensure!(*passed.lock().unwrap(), "client check did not pass");
    Ok(())
}
