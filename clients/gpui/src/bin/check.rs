//! Hidden regression check for the client, driven by tools/check.py against a fixture wheeld:
//! layout from the daemon, one native view per web and terminal pane on its pane, separate
//! terminal sessions, keyboard focus following a click, a divider drag with NSEvents the GPUI
//! view receives, the daemon keeping the ratio, and following another client's change.
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
/// Every native pane's own pixels: WKWebView snapshots and Ghostty IOSurface readbacks.
async fn shot(native: &Native, directory: &std::path::Path, cx: &mut AsyncApp) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    native.call(json!({"op":"snapshot","directory":directory}));
    wait(native, "WK snapshots", |s| s["snapshots_pending"] == 0, cx).await?;
    Ok(())
}
fn frame(status: &Value, pane: &str) -> [f64; 4] {
    let f = &status["panes"][pane]["frame_top"];
    [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
}
fn on_body(a: [f64; 4], r: Rect) -> bool {
    (a[0] - f64::from(r.x)).abs() < 1.0 && (a[1] - f64::from(r.y + TITLE)).abs() < 1.0 && (a[2] - f64::from(r.w)).abs() < 1.0 && (a[3] - f64::from(r.h - TITLE)).abs() < 1.0
}
fn viewport(status: &Value, pane: &str) -> String {
    status["panes"][pane]["viewport"].as_str().unwrap_or_default().to_owned()
}
fn last_grid(status: &Value, pane: &str) -> Value {
    status["panes"][pane]["grids"].as_array().and_then(|a| a.last()).cloned().unwrap_or(Value::Null)
}

async fn pump(connection: &mut Connection, ms: u64, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<()> {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        while let Ok(message) = connection.incoming.try_recv() {
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

async fn experiment(output: PathBuf, cx: &mut AsyncApp) -> Result<Value> {
    let native = Native::load()?;
    let endpoint = Endpoint::new(&default_url(), &default_token_file())?;
    let mut connection = Connection::open(&endpoint)?;
    let window = open_window(cx, &native, connection.send.clone(), endpoint, std::env::var("WHEEL_CLIENT_SPACE").ok(), true)?;
    let markers: Value = serde_json::from_str(&std::env::var("WHEEL_CLIENT_MARKERS")?)?;
    let mut layouts = 0usize;
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    ensure!(layouts > 0, "no layout received from wheeld");
    draw(window, None, cx)?;
    let snapshot = |cx: &mut AsyncApp| window.update(cx, |c, _, _| {
        let (leaves, dividers) = c.scene();
        json!({"leaves":leaves.iter().map(|p| json!({"id":p.leaf.id,"kind":p.leaf.kind,"label":p.leaf.label,"rect":[p.rect.x,p.rect.y,p.rect.w,p.rect.h],
            "native":c.native_panes.contains(&p.leaf.id)})).collect::<Vec<_>>(),
          "dividers":dividers.iter().map(|d| json!({"split":d.split,"horizontal":d.horizontal,"rect":[d.rect.x,d.rect.y,d.rect.w,d.rect.h]})).collect::<Vec<_>>()})
    });
    let first = snapshot(cx)?;
    let native_panes: Vec<(String, String)> = window.update(cx, |c, _, _| {
        let (leaves, _) = c.scene();
        leaves.iter().filter(|p| c.native_panes.contains(&p.leaf.id)).map(|p| (p.leaf.id.clone(), p.leaf.kind.clone())).collect()
    })?;
    let webs: Vec<&String> = native_panes.iter().filter(|p| p.1 == "web").map(|p| &p.0).collect();
    let terms: Vec<&String> = native_panes.iter().filter(|p| p.1 == "terminal").map(|p| &p.0).collect();
    ensure!(webs.len() >= 2 && terms.len() >= 2, "fixture needs two web and two terminal panes: {native_panes:?}");
    // Each pane has its own view, loaded or replayed by the client itself.
    let loaded = wait(&native, "pages and terminals", |s| {
        webs.iter().all(|w| s["panes"][w.as_str()]["loads"].as_array().is_some_and(|a| !a.is_empty()))
            && terms.iter().all(|t| viewport(s, t).contains(markers[t.as_str()].as_str().unwrap_or("?")))
    }, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    draw(window, Some(output.join("client-before.png")), cx)?;
    shot(&native, &output.join("before"), cx).await?;
    let before = native.status();
    // Every native view sits on its pane's body.
    let rects = window.update(cx, |c, _, _| c.scene().0.into_iter().map(|p| (p.leaf.id, p.rect)).collect::<Vec<_>>())?;
    let aligned: Vec<(String, bool)> = native_panes.iter().map(|(id, _)| (id.clone(), on_body(frame(&before, id), rects.iter().find(|r| &r.0 == id).unwrap().1))).collect();
    ensure!(aligned.iter().all(|a| a.1), "native views not on their panes: {aligned:?}");
    // Terminals are separate sessions: each shows only its own marker.
    let own: Vec<(String, bool, bool)> = terms.iter().map(|t| {
        let others = terms.iter().filter(|o| o != &t).any(|o| viewport(&before, t).contains(markers[o.as_str()].as_str().unwrap()));
        (t.to_string(), viewport(&before, t).contains(markers[t.as_str()].as_str().unwrap()), !others)
    }).collect();
    ensure!(own.iter().all(|o| o.1 && o.2), "terminal sessions mixed: {own:?}");
    // A click gives the second terminal keyboard focus; keys reach only its PTY.
    let target = terms[1];
    let r = rects.iter().find(|r| r.0 == *target).unwrap().1;
    let click = [f64::from(r.x + r.w / 2.), f64::from(r.y + r.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":click}));
    native.call(json!({"op":"post_mouse","kind":"up","point":click}));
    tick(cx, 300).await;
    let focused = native.status()["panes"][target.as_str()]["focused"].clone();
    native.call(json!({"op":"post_key","text":"k","code":40}));
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let typed = wait(&native, "typed key in second terminal", |s| viewport(s, target).contains("PTY:k"), cx).await?;
    let leaked = viewport(&typed, terms[0]).contains("PTY:k");
    ensure!(focused == true && !leaked, "keyboard focus: focused={focused} leaked_to_first={leaked}");
    // Drag the first divider between columns with NSEvents the GPUI view receives.
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
    native.call(json!({"op":"post_mouse","kind":"up","point":moved}));
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    let after_drag = snapshot(cx)?;
    let after_status = native.status();
    draw(window, Some(output.join("client-dragged.png")), cx)?;
    shot(&native, &output.join("dragged"), cx).await?;
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
    let per_pane = |s: &Value| -> Value {
        native_panes.iter().map(|(id, kind)| (id.clone(), json!({"kind":kind,"frame":frame(s,id),"grid":last_grid(s,id)}))).collect::<serde_json::Map<_, _>>().into()
    };
    Ok(json!({
        "host_pid": std::process::id(), "layouts_received": layouts, "first_layout": first,
        "native_panes": native_panes, "loaded_at_start": loaded["panes"],
        "native_aligned_with_panes": aligned, "own_session_only": own,
        "keyboard": {"clicked": target, "click": click, "focused": focused, "typed_viewport": viewport(&typed, target), "leaked_to_first": leaked},
        "before": per_pane(&before),
        "drag": {"split": split, "from": start, "to": moved, "parent": [parent.x, parent.y, parent.w, parent.h],
            "after": per_pane(&after_status), "layout_after": after_drag, "ratio_saved_by_wheeld": saved},
        "external_ratio_change": {"cli_exit": status.code(), "ratio_command": 0.3, "layout": external, "panes": per_pane(&external_status)},
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
