//! Hidden regression check for the client, driven by tools/check.py against a fixture wheeld:
//! layout from the daemon, one native view per web and terminal pane on its pane, separate
//! terminal sessions, keyboard focus following a click, a divider drag with NSEvents the GPUI
//! view receives, the daemon keeping the ratio, following another client's change, and surviving
//! sessions and windows (header chips clicked with NSEvents, a second window with its own
//! connection and view, `wheel client ls` reporting each), an agent pane whose reply's Markdown
//! is drawn as blocks (a fenced block with its language and syntax colours) and selected with the
//! pointer and copied with Command-C, and surviving
//! a browser pane sent to a file page by `wheel pane page`, serving `wheel pane console`, `eval`,
//! `dom`, `shot` and `record`, its address field typed into and its back button pressed with
//! wheeld's layout following, a failed load shown and cleared, every plugin page (Beads with its
//! Canvas graph, Issues, Repo, Runner and the whole component gallery) drawn by WheelUI's A2UI
//! renderer in one pane of a session on this repository, and surviving
//! a wheeld restart: the link reads down, comes back up on its own, and every native view is
//! opened again on the new daemon.
//! The window is never visible, key or active.
use anyhow::{Result, ensure};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use wheel_gpui::{browser::CHROME, daemon::*, header::{Chip, Hit}, layout::Rect, native::{Bridge, Native}, transcript::{Kind, ToolRow}, view::{Client, Drawn, Link, TITLE}};

async fn tick(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}
/// A web pane is ready when its page loaded, or when its artifact's A2UI page is drawn natively.
fn web_ready(s: &Value, pane: &str) -> bool {
    let p = &s["panes"][pane];
    p["loads"].as_array().is_some_and(|a| !a.is_empty()) || p["page"]["native"] == true
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
fn identity(status: &Value, pane: &str) -> String {
    status["panes"][pane]["view_identity"].as_str().unwrap_or_default().to_owned()
}
fn frame(status: &Value, pane: &str) -> [f64; 4] {
    let f = &status["panes"][pane]["frame_top"];
    [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
}
/// Whether frame `a` is pane `r`'s body: under the title, and for a browser pane under its chrome too.
fn on_body(a: [f64; 4], r: Rect, kind: &str) -> bool {
    let top = if kind == "browser" { TITLE + CHROME } else { TITLE };
    (a[0] - f64::from(r.x)).abs() < 1.0 && (a[1] - f64::from(r.y + top)).abs() < 1.0 && (a[2] - f64::from(r.w)).abs() < 1.0 && (a[3] - f64::from(r.h - top)).abs() < 1.0
}
fn viewport(status: &Value, pane: &str) -> String {
    status["panes"][pane]["viewport"].as_str().unwrap_or_default().to_owned()
}
fn last_grid(status: &Value, pane: &str) -> Value {
    status["panes"][pane]["grids"].as_array().and_then(|a| a.last()).cloned().unwrap_or(Value::Null)
}

/// What `wheel client ls --json` says each GPUI window shows: (window id, session, space, pane).
fn reported_views() -> Result<Vec<(String, Option<String>, Option<String>, Option<String>)>> {
    let cli = std::env::var("WHEEL_CLIENT_CLI")?;
    let out = std::process::Command::new(&cli)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "client", "ls", "--json"])
        .output()?;
    let clients: Vec<wheel_proto::ClientInfo> = serde_json::from_slice(&out.stdout)?;
    Ok(clients.into_iter().flat_map(|c| c.windows).filter(|w| w.window.starts_with("gpui-"))
        .map(|w| (w.window, w.session, w.space, w.pane)).collect())
}
async fn reported(window: &str, what: &str, want: impl Fn(&(String, Option<String>, Option<String>, Option<String>)) -> bool, cx: &mut AsyncApp) -> Result<()> {
    for _ in 0..60 {
        if reported_views()?.iter().any(|w| w.0 == window && want(w)) {
            return Ok(());
        }
        tick(cx, 100).await;
    }
    anyhow::bail!("wheeld never reported {what} for {window}: {:?}", reported_views()?)
}
/// Click a header chip with a real NSEvent pair on the window's own views, then draw.
async fn click_chip(window: gpui::WindowHandle<Client>, hit: Hit, cx: &mut AsyncApp) -> Result<()> {
    let (native, chip): (_, Option<Chip>) = window.update(cx, |c, _, _| (c.native(), c.header_chips().into_iter().find(|chip| chip.hit == hit)))?;
    let chip = chip.ok_or_else(|| anyhow::anyhow!("no header chip {hit:?}"))?;
    let at = [f64::from(chip.rect.x + chip.rect.w / 2.), f64::from(chip.rect.y + chip.rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":at}));
    for _ in 0..3 {
        tick(cx, 100).await;
        draw(window, None, cx)?;
    }
    Ok(())
}
fn shown(window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<(Option<String>, Option<String>)> {
    Ok(window.update(cx, |c, _, _| (c.session_id().map(str::to_owned), c.space_id().map(str::to_owned)))?)
}

/// Run `wheel` with `args` while the connection is pumped: a `pane page`, `shot` or `eval` waits on
/// wheeld, which waits on this client's answer, so the frames must flow while the command runs.
async fn run_pumped(args: &[&str], connection: &mut Connection, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<(bool, String, String)> {
    let cli = std::env::var("WHEEL_CLIENT_CLI")?;
    let mut child = std::process::Command::new(&cli)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
    let mut done = None;
    for _ in 0..600 {
        pump(connection, 100, layouts, window, cx).await?;
        if let Some(status) = child.try_wait()? {
            done = Some(status);
            break;
        }
    }
    if done.is_none() {
        child.kill()?;
    }
    let out = child.wait_with_output()?;
    Ok((done.is_some_and(|s| s.success()), String::from_utf8_lossy(&out.stdout).trim().to_owned(), String::from_utf8_lossy(&out.stderr).trim().to_owned()))
}

async fn pump(connection: &mut Connection, ms: u64, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<()> {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        while let Ok(event) = connection.incoming.try_recv() {
            if window.update(cx, |c, _, cx| {
                let layout = c.on_event(event);
                cx.notify();
                layout
            })? {
                *layouts += 1;
            }
        }
        tick(cx, 50).await;
    }
    Ok(())
}

/// Wait until a page's component count has held for a second: its data replies can follow its layout.
async fn settle(page_of: &impl Fn(&Value) -> Value, native: &Native, connection: &mut Connection, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<()> {
    let (mut last, mut held) = (u64::MAX, 0);
    for _ in 0..150 {
        pump(connection, 200, layouts, window, cx).await?;
        let count = page_of(&native.status())["components"].as_u64().unwrap_or(0);
        held = if count == last { held + 1 } else { 0 };
        if held >= 5 {
            return Ok(());
        }
        last = count;
    }
    anyhow::bail!("the page's components never settled: {}", page_of(&native.status()))
}

async fn experiment(output: PathBuf, cx: &mut AsyncApp) -> Result<Value> {
    let bridge = Bridge::load()?;
    let endpoint = Endpoint::new(&default_url(), &default_token_file())?;
    let mut connection = Connection::open(endpoint.clone());
    let window = cx.update(|app| wheel_gpui::create_window(app, &bridge, connection.send.clone(), endpoint, None, std::env::var("WHEEL_CLIENT_SPACE").ok(), true))?;
    let native = window.update(cx, |c, _, _| c.native())?;
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
    ensure!(native_panes.iter().any(|p| p.1 == "browser"), "the browser pane has no native view: {native_panes:?}");
    ensure!(webs.len() >= 2 && terms.len() >= 2, "fixture needs two web and two terminal panes: {native_panes:?}");
    // Each pane has its own view, loaded or replayed by the client itself.
    let loaded = wait(&native, "pages and terminals", |s| {
        webs.iter().all(|w| web_ready(s, w))
            && terms.iter().all(|t| viewport(s, t).contains(markers[t.as_str()].as_str().unwrap_or("?")))
    }, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    draw(window, Some(output.join("client-before.png")), cx)?;
    shot(&native, &output.join("before"), cx).await?;
    let before = native.status();
    // A web pane on an A2UI artifact draws the page natively, from the layout its MCP server sent,
    // and not the WKWebView; a plain HTML artifact keeps its WKWebView.
    let native_pages: Vec<&String> = webs.iter().copied().filter(|w| before["panes"][w.as_str()]["page"]["native"] == true).collect();
    ensure!(native_pages.len() == 1, "exactly one web pane should draw an A2UI page: {:?}", before["panes"]);
    ensure!(before["panes"][webs[0].as_str()]["page"]["native"] != true || native_pages[0] != webs[0], "the HTML artifact must stay in its web view");
    let page_state = before["panes"][native_pages[0].as_str()]["page"]["state"].clone();
    ensure!(page_state == "ready", "the native page is not ready: {page_state}");
    let page_png = output.join("before").join(format!("{}-page.png", native_pages[0]));
    let page_bytes = std::fs::metadata(&page_png).map(|m| m.len()).unwrap_or(0);
    ensure!(page_bytes > 5000, "the native page's picture is missing or blank: {} ({page_bytes} bytes)", page_png.display());
    // Every native view sits on its pane's body.
    let rects = window.update(cx, |c, _, _| c.scene().0.into_iter().map(|p| (p.leaf.id, p.rect)).collect::<Vec<_>>())?;
    let aligned: Vec<(String, bool)> = native_panes.iter().map(|(id, kind)| (id.clone(), on_body(frame(&before, id), rects.iter().find(|r| &r.0 == id).unwrap().1, kind))).collect();
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
    // Command-] from the focused terminal (a native view) moves the keyboard to the next pane in
    // layout order, and command-[ moves it back.
    let order: Vec<(String, String)> = window.update(cx, |c, _, _| c.scene().0.into_iter().map(|p| (p.leaf.id, p.leaf.kind)).collect())?;
    let at = order.iter().position(|p| p.0 == *target).unwrap();
    let next = order[(at + 1) % order.len()].clone();
    native.call(json!({"op":"post_key","text":"]","code":30,"command":true}));
    tick(cx, 300).await;
    let after = native.status();
    let client_focus = window.update(cx, |c, _, _| c.focus.current.clone())?;
    let next_has_it = if next.1 == "web" || next.1 == "terminal" || next.1 == "browser" { after["panes"][next.0.as_str()]["focused"] == true } else { after["root_focused"] == true };
    ensure!(client_focus.as_deref() == Some(next.0.as_str()) && next_has_it && after["panes"][target.as_str()]["focused"] == false,
        "command-] moved focus to {client_focus:?}, wanted {next:?}: {after}");
    native.call(json!({"op":"post_key","text":"[","code":33,"command":true}));
    tick(cx, 300).await;
    let back = native.status();
    let back_focus = window.update(cx, |c, _, _| c.focus.current.clone())?;
    ensure!(back_focus.as_deref() == Some(target.as_str()) && back["panes"][target.as_str()]["focused"] == true, "command-[ moved focus to {back_focus:?}");
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
    // The agent pane: a message typed into its composer reaches wheeld, and the fake agent's
    // turns come back as rows: tool calls with their statuses, a thought, an edit's path.
    let agent_pane = std::env::var("WHEEL_CLIENT_AGENT_PANE")?;
    let agent_name = std::env::var("WHEEL_CLIENT_AGENT")?;
    let agent_rect = rects.iter().find(|r| r.0 == agent_pane).map(|r| r.1).ok_or_else(|| anyhow::anyhow!("no agent pane {agent_pane} in the layout"))?;
    let agent_body = [f64::from(agent_rect.x + agent_rect.w / 2.), f64::from(agent_rect.y + agent_rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":agent_body}));
    native.call(json!({"op":"post_mouse","kind":"up","point":agent_body}));
    tick(cx, 300).await;
    ensure!(window.update(cx, |c, _, _| c.focus.current.clone())?.as_deref() == Some(agent_pane.as_str()), "clicking the agent pane did not focus it");
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let attached = rows(cx)?;
    ensure!(attached.0 == "ready", "the agent pane did not attach to a ready agent: state {:?}", attached.0);
    async fn type_line(native: &Native, text: &str, cx: &mut AsyncApp) {
        for ch in text.chars() {
            native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch)}));
            tick(cx, 15).await;
        }
        native.call(json!({"op":"post_key","text":"\r","code":36}));
    }
    type_line(&native, "show tools", cx).await;
    let mut tool_rows = 0usize;
    let mut failed_rows = 0usize;
    let mut thought_rows = 0usize;
    let mut sent_row = false;
    let mut turn_state = String::new();
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, rows) = rows(cx)?;
        sent_row = rows.iter().any(|r| matches!(&r.kind, Kind::User { text, queued: false } if text == "show tools"));
        tool_rows = rows.iter().filter(|r| matches!(r.kind, Kind::Tool { .. })).count();
        failed_rows = rows.iter().filter(|r| matches!(&r.kind, Kind::Tool { status, .. } if status == "failed")).count();
        thought_rows = rows.iter().filter(|r| matches!(r.kind, Kind::Thought(_))).count();
        turn_state = state;
        if sent_row && tool_rows >= 11 && turn_state == "ready" && rows.iter().all(|r| !r.open) {
            break;
        }
    }
    ensure!(sent_row && tool_rows == 11 && failed_rows == 1 && thought_rows == 1 && turn_state == "ready",
        "show tools turn: sent_row={sent_row} tool_rows={tool_rows} failed_rows={failed_rows} thought_rows={thought_rows} state={turn_state}");
    let cli_path = std::env::var("WHEEL_CLIENT_CLI")?;
    let read = std::process::Command::new(&cli_path)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "read", &agent_pane])
        .output()?;
    let read_text = String::from_utf8_lossy(&read.stdout).to_string();
    let sent_seen_by_wheeld = read_text.contains("show tools");
    ensure!(sent_seen_by_wheeld, "wheeld's transcript lacks the typed message: {read_text}{}", String::from_utf8_lossy(&read.stderr));
    let rows_before_diff = rows(cx)?.1.len();
    type_line(&native, "diff", cx).await;
    let mut diff_paths: Vec<String> = vec![];
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, rows) = rows(cx)?;
        // Only the diff turn's rows: the earlier turn edited files of its own.
        diff_paths = rows.iter().skip(rows_before_diff).filter_map(|r| match &r.kind { Kind::Tool { content, .. } => Some(ToolRow::diff_paths(content)), _ => None }).flatten().collect();
        if !diff_paths.is_empty() && state == "ready" {
            break;
        }
    }
    ensure!(diff_paths == ["src/a.rs"], "the diff turn's edited path: {diff_paths:?}");
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-agent.png")), cx)?;
    let state_after = rows(cx)?.0;
    let agent_report = json!({"pane": agent_pane, "agent": agent_name, "sent_seen_by_wheeld": sent_seen_by_wheeld, "tool_rows": tool_rows, "failed_rows": failed_rows,
        "thought_rows": thought_rows, "diff_paths": diff_paths, "state_after": state_after, "rows_total": rows(cx)?.1.len()});
    // Markdown: a message sent through the CLI comes back echoed as the reply, so its fenced
    // block is drawn as a code block with its language label and syntax colours; the first line
    // of that reply is selected with the pointer and copied with Command-C.
    let markdown_message = "Here is code:\n\n```rust\nfn main() { println!(\"hi\"); }\n```";
    let sent = std::process::Command::new(&cli_path)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "send", &agent_pane, markdown_message, "--timeout", "30"])
        .output()?;
    ensure!(sent.status.success(), "agent send failed: {}{}", String::from_utf8_lossy(&sent.stdout), String::from_utf8_lossy(&sent.stderr));
    let mut md_row = None;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, rows) = rows(cx)?;
        md_row = rows.iter().position(|r| matches!(&r.kind, Kind::Reply(t) if t.contains("```rust") && t.contains("println")));
        if md_row.is_some() && state == "ready" && rows.iter().all(|r| !r.open) {
            break;
        }
    }
    let rows_now = rows(cx)?.1;
    let md_row = md_row.ok_or_else(|| anyhow::anyhow!("no Markdown reply arrived: {rows_now:?}"))?;
    window.update(cx, |c, _, _| c.reveal(&agent_name, md_row))?;
    draw(window, None, cx)?;
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-markdown.png")), cx)?;
    let drawn = window.update(cx, |c, _, _| c.drawn_texts(&agent_name))?;
    let code_blocks: Vec<&Drawn> = drawn.iter().filter(|d| d.kind == "code").collect();
    ensure!(code_blocks.len() == 1 && code_blocks[0].language.as_deref() == Some("rust") && code_blocks[0].highlighted,
        "code blocks drawn: {:?}", code_blocks.iter().map(|d| (d.language.clone(), d.highlighted, d.text.clone())).collect::<Vec<_>>());
    let para = drawn.iter().find(|d| d.row == md_row && d.kind == "reply").ok_or_else(|| anyhow::anyhow!("the Markdown reply drew no paragraph"))?;
    let first_line_len = para.text.find('\n').unwrap_or(para.text.len());
    let (from, to) = window.update(cx, |c, _, _| (c.text_point(&agent_name, md_row, para.block, 0), c.text_point(&agent_name, md_row, para.block, first_line_len)))?;
    let from = from.ok_or_else(|| anyhow::anyhow!("no point for the line start"))?;
    let to = to.ok_or_else(|| anyhow::anyhow!("no point for the line end"))?;
    let (from, to) = ([f64::from(from.0) - 1., f64::from(from.1)], [f64::from(to.0) + 1., f64::from(to.1)]);
    native.call(json!({"op":"post_mouse","kind":"down","point":from}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[(from[0] + to[0]) / 2., from[1]]}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":to}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"up","point":to}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-selection.png")), cx)?;
    let selected = window.update(cx, |c, _, _| c.selected_text())?;
    let expected_line = para.text.lines().next().unwrap_or("").to_owned();
    ensure!(selected.as_deref() == Some(expected_line.as_str()), "selected {selected:?}, wanted {expected_line:?} (dragged {from:?} to {to:?}; {})", window.update(cx, |c, _, _| c.selection_debug(&agent_name))?);
    native.call(json!({"op":"post_key","text":"c","code":8,"command":true}));
    tick(cx, 300).await;
    let clipboard = cx.update(|app| app.read_from_clipboard().and_then(|i| i.text()));
    ensure!(clipboard.as_deref() == Some(expected_line.as_str()), "clipboard after Command-C: {clipboard:?}");
    let markdown_report = json!({"code_blocks": code_blocks.len(), "highlighted": code_blocks[0].highlighted, "language": code_blocks[0].language.clone(), "selected_text": selected, "clipboard": clipboard,
        "reply_row": md_row, "blocks": drawn.iter().filter(|d| d.row == md_row).map(|d| json!({"kind": d.kind, "text": d.text.clone()})).collect::<Vec<Value>>()});
    // The footer under the composer: the context meter (three turns answered, 10,000 tokens each
    // of a 200,000 window), the directory, the agent's home, and the config controls the agent
    // offers, sorted model first. A click on the meter reads the raw figures; a click on a
    // control opens its menu; picking a value asks wheeld and the control follows the agent's
    // own list.
    let footer_before = window.update(cx, |c, _, _| c.footer_texts(&agent_name))?;
    let item = |items: &[(String, String)], name: &str| items.iter().find(|(i, _)| i == name).map(|(_, t)| t.clone());
    let context_text = item(&footer_before, "context").ok_or_else(|| anyhow::anyhow!("no context item in the footer: {footer_before:?}"))?;
    ensure!(context_text.ends_with('%') && context_text != "0%", "the context meter after three turns reads {context_text:?} (items {footer_before:?})");
    ensure!(item(&footer_before, "directory").is_some_and(|d| d.starts_with('~') || d.starts_with('/')), "no directory in the footer: {footer_before:?}");
    ensure!(item(&footer_before, "home").is_some(), "no home in the footer: {footer_before:?}");
    let controls = window.update(cx, |c, _, _| c.config_controls(&agent_name))?;
    ensure!(controls.first().is_some_and(|(id, _)| id == "model") && controls.iter().any(|(id, _)| id == "mode"), "config controls drawn: {controls:?}");
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let meter = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "context"))?.ok_or_else(|| anyhow::anyhow!("the context meter was not drawn"))?;
    press(&native, meter);
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let raw_text = item(&window.update(cx, |c, _, _| c.footer_texts(&agent_name))?, "context").unwrap_or_default();
    ensure!(raw_text.contains(" / "), "the meter after a click reads {raw_text:?}, wanted the raw figures");
    draw(window, Some(output.join("client-footer.png")), cx)?;
    let model = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "model"))?.ok_or_else(|| anyhow::anyhow!("the model control was not drawn"))?;
    press(&native, model);
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let menu = window.update(cx, |c, _, _| c.open_menu())?;
    ensure!(menu == Some((agent_name.clone(), "model".into())), "after a click on the model control the open menu is {menu:?} (model at {model:?}, meter at {meter:?}, viewport {:?}, presses {:?})", window.update(cx, |_, w, _| w.viewport_size())?, window.update(cx, |c, _, _| c.presses.clone())?);
    draw(window, Some(output.join("client-config-menu.png")), cx)?;
    let before_set = controls.iter().find(|(id, _)| id == "model").map(|(_, t)| t.clone()).unwrap_or_default();
    let haiku = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "model:haiku"))?.ok_or_else(|| anyhow::anyhow!("the menu drew no haiku item"))?;
    press(&native, haiku);
    tick(cx, 200).await;
    let mut model_now = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        model_now = window.update(cx, |c, _, _| c.transcripts.get(&agent_name).and_then(|t| t.config.iter().find(|o| o.id == "model")).and_then(|o| match &o.kind { wheel_proto::ConfigKind::Select { current, .. } => Some(current.clone()), _ => None }))?;
        if model_now.as_deref() == Some("haiku") {
            break;
        }
    }
    ensure!(model_now.as_deref() == Some("haiku"), "the model after setting it to haiku is {model_now:?}");
    ensure!(window.update(cx, |c, _, _| c.open_menu())?.is_none(), "the menu stayed open after a pick");
    let model_after = window.update(cx, |c, _, _| c.config_controls(&agent_name))?.into_iter().find(|(id, _)| id == "model").map(|(_, t)| t).unwrap_or_default();
    press(&native, meter);
    tick(cx, 200).await;
    let footer_report = json!({"items": footer_before, "raw_text": raw_text, "controls": controls, "model_before": before_set, "model_after": model_after});
    // Statuses: the pane's dot and the space chip's marks follow `status_changed`. A slow turn
    // shows `working`; a permission request shows `needs_you` with the agent's reason until the
    // CLI answers it; after that the pane is idle again.
    let status_name = |s: &Option<wheel_proto::PaneStatus>| s.as_ref().map(wheel_gpui::status::rank).unwrap_or("none").to_owned();
    let status_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.pane_status(&agent_pane));
    let at_rest = status_now(cx)?;
    ensure!(matches!(at_rest, Some(wheel_proto::PaneStatus::Idle | wheel_proto::PaneStatus::Done)), "the agent pane's status between turns is {at_rest:?}");
    type_line(&native, "work slowly 3", cx).await;
    let mut saw_working = false;
    let mut marks_while_working = (vec![], 0);
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let status = status_now(cx)?;
        if status == Some(wheel_proto::PaneStatus::Working) && !saw_working {
            saw_working = true;
            draw(window, None, cx)?;
            draw(window, Some(output.join("client-status-working.png")), cx)?;
            marks_while_working = window.update(cx, |c, _, _| c.space_marks())?;
        }
        let (state, rows) = rows(cx)?;
        if saw_working && state == "ready" && rows.iter().all(|r| !r.open) && status != Some(wheel_proto::PaneStatus::Working) {
            break;
        }
    }
    let last_status = status_now(cx)?;
    ensure!(saw_working, "the pane never showed `working` during the slow turn: {last_status:?}");
    ensure!(marks_while_working.0.first() == Some(&"working"), "the space chip's marks while working: {marks_while_working:?}");
    type_line(&native, "permission", cx).await;
    let mut needs_you = None;
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if let Some(s @ wheel_proto::PaneStatus::NeedsYou { .. }) = status_now(cx)? {
            needs_you = Some(s);
            break;
        }
    }
    let last_status = status_now(cx)?;
    let needs_you = needs_you.ok_or_else(|| anyhow::anyhow!("the permission turn never showed `needs_you`: {last_status:?}"))?;
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-status-needs-you.png")), cx)?;
    let marks_needing = window.update(cx, |c, _, _| c.space_marks())?;
    ensure!(marks_needing.0.first() == Some(&"needs_you"), "the space chip's marks while a permission waits: {marks_needing:?}");
    let answered = std::process::Command::new(&cli_path)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "perm", "answer", &agent_pane, "deny"])
        .output()?;
    ensure!(answered.status.success(), "perm answer failed: {}{}", String::from_utf8_lossy(&answered.stdout), String::from_utf8_lossy(&answered.stderr));
    let mut after_answer = None;
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let status = status_now(cx)?;
        let (state, rows) = rows(cx)?;
        if !matches!(status, Some(wheel_proto::PaneStatus::NeedsYou { .. } | wheel_proto::PaneStatus::Working)) && state == "ready" && rows.iter().all(|r| !r.open) {
            after_answer = status;
            break;
        }
    }
    ensure!(matches!(after_answer, Some(wheel_proto::PaneStatus::Idle | wheel_proto::PaneStatus::Done)), "the pane after the answer is {after_answer:?}");
    draw(window, None, cx)?;
    let status_report = json!({"at_rest": status_name(&at_rest), "saw_working": saw_working, "marks_while_working": marks_while_working,
        "needs_you": wheel_gpui::status::label(&needs_you), "marks_needing": marks_needing, "after_answer": status_name(&after_answer),
        "dots_setting": window.update(cx, |c, _, _| c.status_dots.iter().copied().collect::<Vec<_>>())?});
    // Back to the terminal, so the focus checks below start where they did.
    native.call(json!({"op":"post_mouse","kind":"down","point":click}));
    native.call(json!({"op":"post_mouse","kind":"up","point":click}));
    tick(cx, 300).await;
    // Sessions and windows. The fixture holds two sessions; this window shows the first.
    let (first_session, first_space) = shown(window, cx)?;
    let (second_session, second_space) = (std::env::var("WHEEL_CLIENT_SECOND_SESSION")?, std::env::var("WHEEL_CLIENT_SECOND_SPACE")?);
    let id1 = window.update(cx, |c, _, _| c.window_id().to_owned())?;
    let chips_first = window.update(cx, |c, _, _| c.header_chips())?;
    ensure!(chips_first.iter().any(|c| c.hit == Hit::Session(second_session.clone())) && chips_first.iter().any(|c| c.hit == Hit::NewWindow), "header lacks the session or New Window chip: {chips_first:?}");
    reported(&id1, "the first session", |w| w.1 == first_session && w.2 == first_space, cx).await?;
    click_chip(window, Hit::Session(second_session.clone()), cx).await?;
    let on_second = shown(window, cx)?;
    ensure!(on_second == (Some(second_session.clone()), Some(second_space.clone())), "clicking the second session shows {on_second:?}");
    reported(&id1, "the second session and its space", |w| w.1.as_deref() == Some(second_session.as_str()) && w.2.as_deref() == Some(second_space.as_str()), cx).await?;
    let hidden_native = wait(&native, "the first space's views hidden", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == true), cx).await?;
    draw(window, Some(output.join("client-second-session.png")), cx)?;
    click_chip(window, Hit::Session(first_session.clone().unwrap()), cx).await?;
    ensure!(shown(window, cx)? == (first_session.clone(), first_space.clone()), "clicking the first session did not come back");
    wait(&native, "the first space's views shown again", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == false), cx).await?;
    // New Window opens a second window with a connection of its own, showing the same space.
    let before_windows = cx.update(|app| app.windows().len());
    click_chip(window, Hit::NewWindow, cx).await?;
    let windows: Vec<gpui::WindowHandle<Client>> = cx.update(|app| app.windows().into_iter().filter_map(|w| w.downcast::<Client>()).collect());
    ensure!(windows.len() == before_windows + 1, "New Window opened no window: {before_windows} -> {}", windows.len());
    let second = *windows.iter().find(|w| w.window_id() != window.window_id()).unwrap();
    let id2 = second.update(cx, |c, _, _| c.window_id().to_owned())?;
    ensure!(id1 != id2, "two windows share the id {id1}");
    let native2 = second.update(cx, |c, _, _| c.native())?;
    let mut opened = false;
    for _ in 0..100 {
        draw(second, None, cx)?;
        if native2.status()["panes"].as_object().is_some_and(|p| native_panes.iter().all(|(id, _)| p.contains_key(id))) {
            opened = true;
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(opened, "the second window never opened the space's views: {}", native2.status());
    ensure!(shown(second, cx)? == (first_session.clone(), first_space.clone()), "the second window shows {:?}", shown(second, cx)?);
    wait(&native2, "the second window's pages and terminals", |st| webs.iter().all(|w| web_ready(st, w)), cx).await?;
    let (s1, s2) = (native.status(), native2.status());
    let distinct = native_panes.iter().all(|(id, _)| identity(&s1, id) != identity(&s2, id));
    ensure!(distinct, "the two windows share native views");
    // Each window shows its own session: the second goes to the second session, the first stays.
    click_chip(second, Hit::Session(second_session.clone()), cx).await?;
    ensure!(shown(second, cx)?.0.as_deref() == Some(second_session.as_str()) && shown(window, cx)?.0 == first_session, "windows moved together: {:?} {:?}", shown(window, cx)?, shown(second, cx)?);
    reported(&id2, "the second window on the second session", |w| w.1.as_deref() == Some(second_session.as_str()), cx).await?;
    reported(&id1, "the first window still on the first session", |w| w.1 == first_session, cx).await?;
    draw(second, Some(output.join("client-second-window.png")), cx)?;
    draw(window, Some(output.join("client-first-window.png")), cx)?;
    let both = reported_views()?;
    // Closing the second window ends its connection: wheeld forgets its view, the first stays.
    second.update(cx, |_, w, _| w.remove_window())?;
    let mut gone = false;
    for _ in 0..60 {
        if reported_views()?.iter().all(|w| w.0 != id2) {
            gone = true;
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(gone, "wheeld still lists the closed window {id2}: {:?}", reported_views()?);
    ensure!(reported_views()?.iter().any(|w| w.0 == id1), "closing the second window dropped the first's view");
    ensure!(native.status()["panes"].as_object().is_some_and(|p| native_panes.iter().all(|(id, _)| p.contains_key(id))), "the first window lost its views");
    let sessions_windows = json!({"first_session": first_session, "second_session": second_session, "views_with_two_windows": both.iter().map(|w| json!([w.0, w.1, w.2, w.3])).collect::<Vec<_>>(),
        "second_session_hides_first_space_views": hidden_native["panes"].as_object().map(|p| p.len()), "views_distinct_between_windows": distinct});
    // Web panes: an artifact's page opens under the client's own `wheel-page` scheme, fetched from
    // wheeld with the token in a header (never in the page's address), in a store of its own; the
    // page's script reached wheeld through `.wheel/client.js`; a file written into the folder
    // reloads the pane; and the artifact palette, opened in a new empty pane with `a`, lists the
    // space's `pane = true` artifacts, filters as typed and opens the selected one on Return.
    let html_pane = webs.iter().copied().find(|w| before["panes"][w.as_str()]["page"]["native"] != true).ok_or_else(|| anyhow::anyhow!("no HTML web pane"))?.clone();
    let address = before["panes"][html_pane.as_str()]["url"].as_str().unwrap_or("").to_owned();
    ensure!(address == format!("wheel-page://pages/artifacts/{html_pane}/"), "the HTML pane's address is {address:?}");
    let http_status = before["panes"][html_pane.as_str()]["status"].clone();
    ensure!(http_status == 200, "the HTML pane's document status is {http_status}");
    let store = before["panes"][html_pane.as_str()]["store"].as_str().unwrap_or("").to_owned();
    ensure!(store.len() == 36, "the HTML pane has no page store: {store:?}");
    let run = |args: &[&str]| -> Result<std::process::Output> {
        Ok(std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args).output()?)
    };
    let seen = run(&["artifact", "state", &html_pane, "seen", "--wait", "--timeout", "30"])?;
    let seen_text = String::from_utf8_lossy(&seen.stdout).to_string();
    ensure!(seen.status.success() && (seen_text.contains("\"ok\": true") || seen_text.contains("\"ok\":true")), "the page's wheel.set never reached wheeld: {seen_text}{}", String::from_utf8_lossy(&seen.stderr));
    let loads_before = before["panes"][html_pane.as_str()]["loads"].as_array().map(Vec::len).unwrap_or(0);
    let html_dir = PathBuf::from(std::env::var("WHEEL_CLIENT_HTML")?);
    let page_v2 = std::fs::read_to_string(html_dir.join("index.html"))?.replace("Fixture artifact", "Fixture artifact v2");
    std::fs::write(html_dir.join("index.html"), page_v2)?;
    // `pane_reload` arrives over the client's own connection, so the wait pumps it.
    let mut reloaded = native.status();
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        reloaded = native.status();
        let p = &reloaded["panes"][html_pane.as_str()];
        if p["reloads"].as_u64().unwrap_or(0) >= 1 && p["loads"].as_array().and_then(|a| a.last()).and_then(Value::as_str) == Some("Fixture artifact v2") {
            break;
        }
    }
    ensure!(reloaded["panes"][html_pane.as_str()]["reloads"].as_u64().unwrap_or(0) >= 1, "the HTML pane never reloaded after its file changed: {}", reloaded["panes"][html_pane.as_str()]);
    let reloads = reloaded["panes"][html_pane.as_str()]["reloads"].clone();
    let title_after = reloaded["panes"][html_pane.as_str()]["loads"].as_array().and_then(|a| a.last()).cloned().unwrap_or_default();
    let split_out = run(&["pane", "split", &html_pane, "right"])?;
    ensure!(split_out.status.success(), "pane split failed: {}", String::from_utf8_lossy(&split_out.stderr));
    let new_pane = String::from_utf8_lossy(&split_out.stdout).lines().last().unwrap_or_default().trim().to_owned();
    let mut new_rect = None;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        new_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == new_pane && p.leaf.kind == "empty").map(|p| p.rect))?;
        if new_rect.is_some() {
            break;
        }
    }
    let new_rect = new_rect.ok_or_else(|| anyhow::anyhow!("the split pane {new_pane} never arrived as an empty pane"))?;
    draw(window, None, cx)?;
    let body = [f64::from(new_rect.x + new_rect.w / 2.), f64::from(new_rect.y + new_rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":body}));
    native.call(json!({"op":"post_mouse","kind":"up","point":body}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    ensure!(window.update(cx, |c, _, _| c.focus.current.clone())?.as_deref() == Some(new_pane.as_str()), "the click did not focus the empty pane");
    native.call(json!({"op":"post_key","text":"a","code":keycode('a')}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    ensure!(window.update(cx, |c, _, _| c.palette(&new_pane).is_some())?, "`a` in the empty pane opened no palette");
    let mut listed: Vec<String> = vec![];
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if let Some(rows) = window.update(cx, |c, _, _| c.palette(&new_pane).filter(|p| !p.is_loading()).map(|p| p.rows.iter().map(|r| r.id()).collect::<Vec<_>>()))? {
            listed = rows;
            break;
        }
    }
    let failure = window.update(cx, |c, _, _| c.palette(&new_pane).and_then(|p| p.failure().map(str::to_owned)))?;
    // The project's two artifacts, then the fixture's machine artifacts by name.
    let page_names: Vec<String> = serde_json::from_str(&std::env::var("WHEEL_CLIENT_PAGE_NAMES")?)?;
    let mut machine_names = page_names.clone();
    machine_names.sort();
    let want: Vec<String> = ["project/fixture-html", "project/fixture-page"].map(String::from).into_iter().chain(machine_names.iter().map(|n| format!("machine/{n}"))).collect();
    ensure!(listed == want, "the palette listed {listed:?} (failure {failure:?})");
    for ch in "html".chars() {
        native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch)}));
        tick(cx, 30).await;
    }
    draw(window, None, cx)?;
    let (query, filtered, footer_text) = window.update(cx, |c, _, _| c.palette(&new_pane).map(|p| (p.query.clone(), p.rows.iter().map(|r| r.id()).collect::<Vec<_>>(), p.footer())).unwrap_or_default())?;
    ensure!(query == "html" && filtered == ["project/fixture-html"], "typing html left query {query:?} and rows {filtered:?}");
    ensure!(footer_text == format!("1 of {}", want.len()), "the palette's footer reads {footer_text:?}");
    draw(window, Some(output.join("client-palette.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut kind_after = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        kind_after = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == new_pane).map(|p| (p.leaf.kind, p.leaf.label)))?;
        if kind_after.as_ref().is_some_and(|(k, _)| k == "web") {
            break;
        }
    }
    ensure!(kind_after == Some(("web".into(), "fixture-html".into())), "after Return the pane is {kind_after:?}");
    ensure!(window.update(cx, |c, _, _| c.palette(&new_pane).is_none())?, "the palette stayed open after opening an artifact");
    draw(window, None, cx)?;
    let opened = wait(&native, "the artifact opened from the palette", |s| web_ready(s, &new_pane), cx).await?;
    let opened_address = opened["panes"][new_pane.as_str()]["url"].as_str().unwrap_or("").to_owned();
    ensure!(opened_address == format!("wheel-page://pages/artifacts/{new_pane}/"), "the opened pane's address is {opened_address:?}");
    draw(window, Some(output.join("client-palette-opened.png")), cx)?;
    // The pages' own pixels: the reloaded HTML artifact and the one the palette opened.
    shot(&native, &output.join("web"), cx).await?;
    let web_report = json!({"pane": html_pane, "address": address, "http_status": http_status, "store": store, "client_state": seen_text.trim(),
        "loads_before_change": loads_before, "reloads": reloads, "title_after_change": title_after,
        "palette": {"pane": new_pane, "listed": listed, "query": query, "filtered": filtered, "footer": footer_text, "kind_after": kind_after, "opened_address": opened_address}});
    // The browser pane: `wheel pane page` sends it to a file page through wheeld; the page's
    // console reaches wheeld; `eval`, `dom`, `shot` (the pane and one element) and `record` run in
    // the pane's page; an address typed into the GPUI-drawn field goes through wheeld and the
    // layout follows; Back goes back and the layout follows; a missing file shows the failure
    // card in place of the page, and the next page clears it.
    let browser_pane = native_panes.iter().find(|p| p.1 == "browser").map(|p| p.0.clone()).ok_or_else(|| anyhow::anyhow!("no browser pane"))?;
    let pages = PathBuf::from(std::env::var("WHEEL_CLIENT_PAGES")?);
    let (one, two) = (pages.join("one.html"), pages.join("two.html"));
    let state_of = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.browser_state(&browser_pane).cloned());
    ensure!(state_of(cx)?.is_some_and(|s| s.address == "about:blank"), "the browser pane's chrome state before any page: {:?}", state_of(cx)?);
    let (ok, page_out, page_err) = run_pumped(&["pane", "page", &browser_pane, &one.to_string_lossy()], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "pane page failed: {page_out}{page_err}");
    let one_url = format!("file://{}", one.display());
    ensure!(page_out == one_url, "pane page printed {page_out:?}, wanted {one_url:?}");
    let mut on_one = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if let Some(s) = state_of(cx)?.filter(|s| s.url == one_url && s.title == "Fixture page one" && !s.loading) {
            on_one = Some(s);
            break;
        }
    }
    let last_state = state_of(cx)?;
    let on_one = on_one.ok_or_else(|| anyhow::anyhow!("the browser pane never reached page one: {last_state:?}"))?;
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-browser.png")), cx)?;
    let leaf_url = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == browser_pane).and_then(|p| p.leaf.url));
    ensure!(leaf_url(cx)? == Some(one_url.clone()), "wheeld's layout does not hold page one: {:?}", leaf_url(cx)?);
    let title_shown = window.update(cx, |c, _, _| c.browser_state(&browser_pane).map(wheel_gpui::browser::title))?;
    ensure!(title_shown.as_deref() == Some("Fixture page one"), "the pane's title reads {title_shown:?}");
    let (ok, console_out, console_err) = run_pumped(&["pane", "console", &browser_pane], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && console_out.contains("hello from one") && console_out.contains("careful from one"), "pane console: {console_out}{console_err}");
    let (ok, eval_out, eval_err) = run_pumped(&["pane", "eval", &browser_pane, "1 + 2"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && eval_out == "3", "pane eval 1 + 2: {eval_out}{eval_err}");
    let (ok, eval_title, _) = run_pumped(&["pane", "eval", &browser_pane, "document.title"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && eval_title == "\"Fixture page one\"", "pane eval document.title: {eval_title}");
    let (ok, eval_throw, eval_throw_err) = run_pumped(&["pane", "eval", &browser_pane, "throw new Error('nope')"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(!ok && eval_throw_err.contains("nope"), "a throwing eval should fail with its message: ok={ok} {eval_throw} {eval_throw_err}");
    let (ok, dom_out, dom_err) = run_pumped(&["pane", "dom", &browser_pane, "h1"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && dom_out == "<h1 id=\"headline\">Fixture page one</h1>", "pane dom h1: {dom_out}{dom_err}");
    let (ok, dom_all, _) = run_pumped(&["pane", "dom", &browser_pane], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && dom_all.starts_with("<html>") && dom_all.contains("class=\"note\""), "pane dom: {}", &dom_all[..dom_all.len().min(200)]);
    let shot_path = output.join("browser-shot.png");
    let (ok, shot_out, shot_err) = run_pumped(&["pane", "shot", &browser_pane, &shot_path.to_string_lossy()], &mut connection, &mut layouts, window, cx).await?;
    let shot_bytes = std::fs::read(&shot_path).unwrap_or_default();
    ensure!(ok && shot_bytes.starts_with(b"\x89PNG") && shot_bytes.len() > 2000, "pane shot: ok={ok} {} bytes {shot_out}{shot_err}", shot_bytes.len());
    let element_path = output.join("browser-shot-h1.png");
    let (ok, el_out, el_err) = run_pumped(&["pane", "shot", &browser_pane, &element_path.to_string_lossy(), "--selector", "h1"], &mut connection, &mut layouts, window, cx).await?;
    let element_bytes = std::fs::read(&element_path).unwrap_or_default();
    ensure!(ok && element_bytes.starts_with(b"\x89PNG") && element_bytes.len() < shot_bytes.len(), "pane shot --selector h1: ok={ok} {} bytes (page {}) {el_out}{el_err}", element_bytes.len(), shot_bytes.len());
    let (ok, none_out, none_err) = run_pumped(&["pane", "shot", &browser_pane, &output.join("browser-none.png").to_string_lossy(), "--selector", "#nothing"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(!ok && none_err.contains("no element matches"), "a shot of nothing should fail: ok={ok} {none_out} {none_err}");
    let movie_path = output.join("browser.mov");
    let (ok, record_out, record_err) = run_pumped(&["pane", "record", &browser_pane, &movie_path.to_string_lossy(), "--seconds", "1"], &mut connection, &mut layouts, window, cx).await?;
    let movie_bytes = std::fs::metadata(&movie_path).map(|m| m.len()).unwrap_or(0);
    ensure!(ok && movie_bytes > 1000, "pane record: ok={ok} {movie_bytes} bytes {record_out}{record_err}");
    // The address field: a click starts editing the page's path; the path is replaced with page
    // two's and Return sends it to wheeld, which answers `opened`; the view goes there and
    // reports the page, so the layout follows.
    let field = window.update(cx, |c, _, _| c.control_bounds(&browser_pane, "browser:address"))?.ok_or_else(|| anyhow::anyhow!("the address field was not drawn"))?;
    press(&native, field);
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let editing = window.update(cx, |c, _, _| c.address_edit())?;
    ensure!(editing == Some((browser_pane.clone(), one.to_string_lossy().into_owned())), "after a click on the field the edit is {editing:?}");
    ensure!(native.status()["root_focused"] == true, "the GPUI view does not hold the keyboard while the field is edited");
    // Backspace carries DEL (0x7f) on a Mac keyboard; an uppercase letter is its key with shift.
    for _ in 0..one.to_string_lossy().chars().count() {
        native.call(json!({"op":"post_key","text":"\u{7f}","code":51}));
    }
    tick(cx, 100).await;
    for ch in two.to_string_lossy().chars() {
        native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch),"shift":ch.is_ascii_uppercase()}));
        tick(cx, 10).await;
    }
    draw(window, None, cx)?;
    let typed_draft = window.update(cx, |c, _, _| c.address_edit())?.map(|(_, d)| d);
    ensure!(typed_draft.as_deref() == Some(&*two.to_string_lossy()), "the typed address reads {typed_draft:?}");
    draw(window, Some(output.join("client-browser-typing.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let two_url = format!("file://{}", two.display());
    let mut on_two = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if state_of(cx)?.is_some_and(|s| s.url == two_url && s.title == "Fixture page two") && leaf_url(cx)? == Some(two_url.clone()) {
            on_two = true;
            break;
        }
    }
    ensure!(on_two, "typing page two's path did not move the pane and the layout: state {:?} leaf {:?}", state_of(cx)?, leaf_url(cx)?);
    ensure!(window.update(cx, |c, _, _| c.address_edit())?.is_none(), "the field is still being edited after Return");
    let back = window.update(cx, |c, _, _| c.control_bounds(&browser_pane, "browser:back"))?.ok_or_else(|| anyhow::anyhow!("the back button was not drawn"))?;
    ensure!(state_of(cx)?.is_some_and(|s| s.can_go_back), "back is not enabled on page two");
    press(&native, back);
    let mut back_on_one = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if state_of(cx)?.is_some_and(|s| s.url == one_url && s.can_go_forward) && leaf_url(cx)? == Some(one_url.clone()) {
            back_on_one = true;
            break;
        }
    }
    ensure!(back_on_one, "Back did not return to page one with the layout following: state {:?} leaf {:?}", state_of(cx)?, leaf_url(cx)?);
    // A file that does not exist: wheeld reads the path as a page, the load fails, and the failure
    // card takes the body with the web view hidden under it; the next page clears it.
    let missing = pages.join("missing.html");
    let (ok, missing_out, missing_err) = run_pumped(&["pane", "page", &browser_pane, &missing.to_string_lossy()], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "pane page on a missing file should still be read as a page: {missing_out}{missing_err}");
    let mut failure = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if let Some(f) = state_of(cx)?.and_then(|s| s.failure) {
            failure = Some(f);
            break;
        }
    }
    let last_state = state_of(cx)?;
    let failure = failure.ok_or_else(|| anyhow::anyhow!("the missing file showed no failure: {last_state:?}"))?;
    ensure!(failure.message.contains("no such file") && failure.address == missing.to_string_lossy(), "the failure reads {failure:?}");
    draw(window, None, cx)?;
    ensure!(native.status()["panes"][browser_pane.as_str()]["hidden"] == true, "the web view still shows under the failure card");
    ensure!(window.update(cx, |c, _, _| c.control_bounds(&browser_pane, "browser:retry"))?.is_some(), "the failure card drew no Try Again");
    draw(window, Some(output.join("client-browser-failure.png")), cx)?;
    // WebKit stays on page one behind the card and reports it, so wheeld holds page one again and
    // another `pane page one` changes nothing (the Swift app behaves the same); page two clears it.
    let (ok, _, clear_err) = run_pumped(&["pane", "page", &browser_pane, &two.to_string_lossy()], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "pane page two after the failure: {clear_err}");
    let mut cleared = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if state_of(cx)?.is_some_and(|s| s.failure.is_none() && s.url == two_url && s.title == "Fixture page two") && native.status()["panes"][browser_pane.as_str()]["hidden"] == false {
            cleared = true;
            break;
        }
    }
    ensure!(cleared, "page two did not clear the failure: {:?}\nbridge logs: {}", state_of(cx)?, native.status()["logs"]);
    shot(&native, &output.join("browser"), cx).await?;
    let browser_logs = native.status()["logs"].clone();
    let browser_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == browser_pane).map(|p| [p.rect.x, p.rect.y, p.rect.w, p.rect.h]))?;
    let browser_report = json!({"pane": browser_pane, "rect": browser_rect, "page_printed": page_out, "state_on_one": on_one, "title_shown": title_shown, "console": console_out,
        "eval": {"sum": eval_out, "title": eval_title, "throw_error": eval_throw_err}, "dom_h1": dom_out, "dom_bytes": dom_all.len(),
        "shot_bytes": shot_bytes.len(), "shot_h1_bytes": element_bytes.len(), "shot_none_error": none_err, "record": {"bytes": movie_bytes, "printed": record_out},
        "typed": {"draft": typed_draft, "moved_to_two": on_two, "back_to_one": back_on_one}, "failure": failure, "cleared": cleared, "logs": browser_logs});
    // Plugin pages: a session on this repository holds one web pane, moved by `wheel pane kind`
    // through every machine artifact the fixture serves. Each is drawn by WheelUI's A2UI renderer
    // (the Swift app's own view), reaches `ready` with every component in the catalog, and is
    // pictured from the view's pixels. The Beads page's Graph view, chosen by writing its picker's
    // value into the page's data model, draws the dependency graph with Wheel's Canvas component.
    let (pages_session, pages_pane) = (std::env::var("WHEEL_CLIENT_PAGES_SESSION")?, std::env::var("WHEEL_CLIENT_PAGES_PANE")?);
    click_chip(window, Hit::Session(pages_session.clone()), cx).await?;
    ensure!(shown(window, cx)?.0.as_deref() == Some(pages_session.as_str()), "clicking the Pages session shows {:?}", shown(window, cx)?);
    let pages_dir = output.join("pages");
    let mut pages_report = serde_json::Map::new();
    let page_of = |s: &Value| s["panes"][pages_pane.as_str()]["page"].clone();
    let has_canvas = |page: &Value| page["types"].as_array().is_some_and(|t| t.iter().any(|t| t == "Canvas"));
    for (n, name) in page_names.iter().enumerate() {
        if n > 0 {
            let (ok, _, err) = run_pumped(&["pane", "kind", &pages_pane, "web", "--artifact", name, "--scope", "machine"], &mut connection, &mut layouts, window, cx).await?;
            ensure!(ok, "pane kind {name}: {err}");
        }
        let artifact = format!("machine/{name}");
        let drew_natively = |page: &Value| page["artifact"] == artifact.as_str() && page["native"] == true && page["state"] == "ready";
        let mut page = Value::Null;
        for _ in 0..300 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            page = page_of(&native.status());
            if drew_natively(&page) && page["components"].as_u64().unwrap_or(0) > 0 {
                break;
            }
        }
        ensure!(drew_natively(&page), "the {name} page never drew natively: {page}");
        ensure!(page["unsupported"].as_array().is_some_and(|u| u.is_empty()), "the {name} page asked for components the client lacks: {page}");
        settle(&page_of, &native, &mut connection, &mut layouts, window, cx).await?;
        draw(window, Some(output.join(format!("client-page-{name}.png"))), cx)?;
        shot(&native, &pages_dir.join(name), cx).await?;
        let png = pages_dir.join(name).join(format!("{pages_pane}-page.png"));
        let bytes = std::fs::metadata(&png).map(|m| m.len()).unwrap_or(0);
        ensure!(bytes > 5000, "the {name} page's picture is missing or blank: {} ({bytes} bytes)", png.display());
        let mut graph = Value::Null;
        if name == "beads" {
            // The views picker binds `/set_view` to an array holding the chosen value (wheelhouse's `Page::choose`).
            let set = native.call(json!({"op":"page_set","pane":pages_pane,"path":"/set_view","value":["graph"]}));
            ensure!(set["ok"] != false, "the bridge refused to write /set_view into the Beads page: {set}");
            for _ in 0..300 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                graph = page_of(&native.status());
                if has_canvas(&graph) {
                    break;
                }
            }
            ensure!(has_canvas(&graph), "the Beads Graph view drew no Canvas: {graph}");
            settle(&page_of, &native, &mut connection, &mut layouts, window, cx).await?;
            shot(&native, &pages_dir.join("beads-graph"), cx).await?;
        }
        let page = page_of(&native.status());
        pages_report.insert(name.clone(), json!({"state": page["state"], "components": page["components"], "types": page["types"], "notice": page["notice"],
            "png_bytes": bytes, "graph_types": graph["types"]}));
    }
    let pages_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == pages_pane).map(|p| [p.rect.x, p.rect.y, p.rect.w, p.rect.h]))?;
    let pages_report = json!({"pane": pages_pane, "rect": pages_rect, "pages": pages_report});
    click_chip(window, Hit::Session(first_session.clone().unwrap()), cx).await?;
    ensure!(shown(window, cx)? == (first_session.clone(), first_space.clone()), "clicking the first session after the pages did not come back");
    wait(&native, "the first space's views shown after the pages", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == false), cx).await?;
    // The command palette, the key bindings and the shortcut sheet (CommandActions.swift,
    // KeyBindings.swift, ShortcutSheet.swift): a bound key pressed while a terminal holds the
    // keyboard runs its action; the palette lists the verbs, the actions and the fixture's
    // `[runner]` entry and runs them; the `keybind` setting moves a key; ⌘/ shows the sheet;
    // ⌘W on a shell asks first.
    let leaves = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.scene().0.into_iter().map(|p| (p.leaf.id, p.leaf.kind)).collect::<Vec<_>>());
    let post = |native: &Native, ch: char, command: bool| native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch),"command":command}));
    let config = |args: &[&str]| -> Result<std::process::Output> {
        Ok(std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "config"]).args(args).output()?)
    };
    // The terminal takes the keyboard by a click, so ⌘K reaches the GPUI view only through the bridge's routing.
    let term_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| &p.leaf.id == terms[0]).map(|p| p.rect))?.ok_or_else(|| anyhow::anyhow!("no terminal pane"))?;
    let term_body = [f64::from(term_rect.x + term_rect.w / 2.), f64::from(term_rect.y + term_rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":term_body}));
    native.call(json!({"op":"post_mouse","kind":"up","point":term_body}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let routed_before = native.status();
    ensure!(routed_before["panes"][terms[0].as_str()]["focused"] == true, "clicking the terminal did not give it the keyboard");
    let panes_before = leaves(cx)?;
    post(&native, 'k', true);
    let mut root_rows = None;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        root_rows = window.update(cx, |c, _, _| c.command_palette.as_ref().map(|p| p.rows.iter().map(|r| r.id()).collect::<Vec<_>>()))?;
        if root_rows.is_some() {
            break;
        }
    }
    let actions_so_far = window.update(cx, |c, _, _| c.actions_log.clone())?;
    let root_rows = root_rows.ok_or_else(|| anyhow::anyhow!("⌘K in a terminal opened no command palette; held_before_key {:?}, actions {actions_so_far:?}", native.status()["held_before_key"]))?;
    // The fixture's runner and the space's artifacts arrive with `runners` and `artifacts`, after the palette opened.
    let mut root_rows = root_rows;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        root_rows = window.update(cx, |c, _, _| c.command_palette.as_ref().map(|p| p.rows.iter().map(|r| r.id()).collect::<Vec<_>>()).unwrap_or_default())?;
        if root_rows.iter().any(|r| r == "verb:run") && root_rows.iter().any(|r| r == "verb:showArtifact") {
            break;
        }
    }
    for want in ["verb:switchSpace", "verb:goToPane", "verb:closePane", "verb:newPane", "verb:changePaneType", "verb:showArtifact", "verb:run", "action:new-space", "action:close-pane", "action:command-palette", "action:shortcuts", "action:focus-next"] {
        ensure!(root_rows.iter().any(|r| r == want), "the palette's root list lacks {want}: {root_rows:?}");
    }
    for ch in "split r".chars() {
        post(&native, ch, false);
        tick(cx, 30).await;
    }
    draw(window, None, cx)?;
    let (query, filtered) = window.update(cx, |c, _, _| c.command_palette.as_ref().map(|p| (p.query.clone(), p.rows.iter().map(|r| r.id()).collect::<Vec<_>>())).unwrap_or_default())?;
    ensure!(query == "split r" && filtered.first().map(String::as_str) == Some("action:split-right"), "typing `split r` left query {query:?} and rows {filtered:?}");
    draw(window, Some(output.join("client-command-palette.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut after_split = panes_before.clone();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        after_split = leaves(cx)?;
        if after_split.len() == panes_before.len() + 1 {
            break;
        }
    }
    ensure!(after_split.len() == panes_before.len() + 1, "Split Right from the palette made no pane: {panes_before:?} -> {after_split:?}");
    ensure!(window.update(cx, |c, _, _| c.command_palette.is_none())?, "the palette stayed open after running an action");
    let split_pane = after_split.iter().find(|p| !panes_before.iter().any(|b| b.0 == p.0)).map(|p| p.0.clone()).unwrap();
    // Run… lists the fixture's runner; running it opens a terminal pane that prints its line.
    post(&native, 'k', true);
    tick(cx, 200).await;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.command_palette.as_ref().is_some_and(|p| p.rows.iter().any(|r| r.id() == "verb:run")))? {
            break;
        }
    }
    for ch in "fixture-echo".chars() {
        post(&native, ch, false);
        tick(cx, 30).await;
    }
    draw(window, None, cx)?;
    let runner_rows = window.update(cx, |c, _, _| c.command_palette.as_ref().map(|p| p.rows.iter().map(|r| (r.id(), r.item.title())).collect::<Vec<_>>()).unwrap_or_default())?;
    ensure!(runner_rows.first().map(|r| r.0.as_str()) == Some("action:run-fixture-echo") && runner_rows[0].1 == "Run fixture-echo", "typing the runner's name listed {runner_rows:?}");
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut after_run = after_split.clone();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        after_run = leaves(cx)?;
        if after_run.len() == after_split.len() + 1 {
            break;
        }
    }
    ensure!(after_run.len() == after_split.len() + 1, "Run fixture-echo made no pane: {after_split:?} -> {after_run:?}");
    let runner_pane = after_run.iter().find(|p| !after_split.iter().any(|b| b.0 == p.0)).map(|p| p.0.clone()).unwrap();
    ensure!(after_run.iter().any(|p| p.0 == runner_pane && p.1 == "terminal"), "the runner's pane is not a terminal: {after_run:?}");
    draw(window, None, cx)?;
    let ran = wait(&native, "the runner's terminal printing its line", |s| viewport(s, &runner_pane).lines().any(|l| l.trim() == "FIXTURE-RUNNER"), cx).await?;
    let runner_viewport = viewport(&ran, &runner_pane);
    // The shortcut sheet: ⌘/ opens it with the keymap's keys, Esc closes it.
    post(&native, '/', true);
    let mut sheet = None;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        sheet = window.update(cx, |c, _, _| c.shortcuts_open.then(|| c.shortcut_sheet()))?;
        if sheet.is_some() {
            break;
        }
    }
    let sheet = sheet.ok_or_else(|| anyhow::anyhow!("⌘/ opened no shortcut sheet"))?;
    ensure!(sheet[0].0 == "Panes" && sheet[0].1[0].keys == "⌘D" && sheet[1].1.iter().any(|r| r.id == "space-1" && r.keys == "⌘1"), "the sheet lists {:?}", sheet.iter().map(|g| (g.0, g.1.len())).collect::<Vec<_>>());
    draw(window, Some(output.join("client-shortcuts.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if !window.update(cx, |c, _, _| c.shortcuts_open)? {
            break;
        }
    }
    ensure!(!window.update(cx, |c, _, _| c.shortcuts_open)?, "Esc left the shortcut sheet open");
    // The `keybind` setting moves Split Right to ⌘E: the keymap, the sheet and the key follow.
    let set = config(&["set", "keybind", "cmd+e=split-right", "--machine"])?;
    ensure!(set.status.success(), "config set keybind failed: {}{}", String::from_utf8_lossy(&set.stdout), String::from_utf8_lossy(&set.stderr));
    let mut moved_key = String::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        moved_key = window.update(cx, |c, _, _| c.keymap.symbol("split-right"))?;
        if moved_key == "⌘E" {
            break;
        }
    }
    ensure!(moved_key == "⌘E", "after `keybind = cmd+e=split-right` Split Right is bound to {moved_key:?}");
    ensure!(window.update(cx, |c, _, _| c.keymap.action(&wheel_gpui::keymap::KeyChord::parse("cmd+d").unwrap()).map(str::to_owned))? == None, "⌘D is still bound after the setting moved it");
    // The empty pane the first split made has the keyboard; ⌘E splits it.
    let empty_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.id == split_pane).map(|p| p.rect))?.ok_or_else(|| anyhow::anyhow!("the split pane left"))?;
    let empty_body = [f64::from(empty_rect.x + empty_rect.w / 2.), f64::from(empty_rect.y + empty_rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":empty_body}));
    native.call(json!({"op":"post_mouse","kind":"up","point":empty_body}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    post(&native, 'e', true);
    let mut after_rebind = after_run.clone();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        after_rebind = leaves(cx)?;
        if after_rebind.len() == after_run.len() + 1 {
            break;
        }
    }
    ensure!(after_rebind.len() == after_run.len() + 1, "⌘E after the rebind split nothing: {after_run:?} -> {after_rebind:?}; actions {:?}", window.update(cx, |c, _, _| c.actions_log.clone())?);
    let unset = config(&["unset", "keybind", "--machine"])?;
    ensure!(unset.status.success(), "config unset keybind failed: {}", String::from_utf8_lossy(&unset.stderr));
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.keymap.symbol("split-right"))? == "⌘D" {
            break;
        }
    }
    ensure!(window.update(cx, |c, _, _| c.keymap.symbol("split-right"))? == "⌘D", "unsetting keybind did not bring ⌘D back");
    // ⌘W closes the focused empty pane with no question; on a shell it asks, and Esc declines.
    let focused_now = window.update(cx, |c, _, _| c.focus.current.clone())?;
    post(&native, 'w', true);
    let mut after_close = after_rebind.clone();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        after_close = leaves(cx)?;
        if after_close.len() == after_rebind.len() - 1 {
            break;
        }
    }
    ensure!(after_close.len() == after_rebind.len() - 1 && window.update(cx, |c, _, _| c.confirmation.is_none())?, "⌘W on empty pane {focused_now:?} closed nothing: {after_rebind:?} -> {after_close:?}");
    // The splits moved the terminal: click where it is now.
    draw(window, None, cx)?;
    let term_rect = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| &p.leaf.id == terms[0]).map(|p| p.rect))?.ok_or_else(|| anyhow::anyhow!("the terminal pane left"))?;
    let term_body = [f64::from(term_rect.x + term_rect.w / 2.), f64::from(term_rect.y + term_rect.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":term_body}));
    native.call(json!({"op":"post_mouse","kind":"up","point":term_body}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    wait(&native, "the terminal taking the keyboard again", |s| s["panes"][terms[0].as_str()]["focused"] == true, cx).await?;
    post(&native, 'w', true);
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if window.update(cx, |c, _, _| c.confirmation.is_some())? {
            break;
        }
    }
    let debug = (window.update(cx, |c, _, _| (c.actions_log.clone(), c.focus.current.clone(), c.keymap.symbol("close-pane")))?, native.status());
    let asked = window.update(cx, |c, _, _| c.confirmation.clone())?.ok_or_else(|| anyhow::anyhow!("⌘W on a terminal asked nothing: actions {:?}, focus {:?}, close-pane key {:?}, held_before_key {:?}, root_focused {:?}, terminal focused {:?}",
        debug.0.0, debug.0.1, debug.0.2, debug.1["held_before_key"], debug.1["root_focused"], debug.1["panes"][terms[0].as_str()]["focused"]))?;
    ensure!(asked.title == "Kill this pane's shell?" && asked.button == "Close Pane", "the question reads {asked:?}");
    draw(window, Some(output.join("client-confirm-close.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    tick(cx, 200).await;
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    ensure!(window.update(cx, |c, _, _| c.confirmation.is_none())? && leaves(cx)?.len() == after_close.len(), "Esc did not decline the close");
    let commands_report = json!({"root_rows": root_rows, "split_query": query, "split_pane": split_pane, "runner_pane": runner_pane, "runner_viewport": runner_viewport,
        "sheet_groups": sheet.iter().map(|g| json!({"name": g.0, "rows": g.1.len()})).collect::<Vec<_>>(), "rebound_key": moved_key,
        "panes": {"before": panes_before.len(), "after_split": after_split.len(), "after_run": after_run.len(), "after_rebind": after_rebind.len(), "after_close": after_close.len()},
        "confirmation": {"title": asked.title, "message": asked.message, "button": asked.button}, "actions": window.update(cx, |c, _, _| c.actions_log.clone())?});
    // Closing the second window above left `native_panes` as the first window's views; the
    // pane opened from the palette is one more of them.
    // wheeld stops, stays away for 1.5 s and starts again on the same port; tools/check.py does
    // the restart when it sees the request file.
    let (request, done) = (output.join("restart.request"), output.join("restart.done"));
    let _ = std::fs::remove_file(&done);
    let status_before_restart = native.status();
    let identities_before: Vec<(String, String)> = native_panes.iter().map(|(id, _)| (id.clone(), identity(&status_before_restart, id))).collect();
    let layouts_before_restart = layouts;
    std::fs::write(&request, "1.5")?;
    let (mut saw_down, mut down_reason, mut up_again) = (false, String::new(), false);
    let mut banner_while_down = String::new();
    for _ in 0..400 {
        pump(&mut connection, 50, &mut layouts, window, cx).await?;
        let (link, banner) = window.update(cx, |c, _, _| (c.link.clone(), c.link.banner()))?;
        if let Link::Down(reason) = &link {
            saw_down = true;
            down_reason = reason.clone();
            banner_while_down = banner;
        }
        if saw_down && link == Link::Up && done.exists() {
            up_again = true;
            break;
        }
    }
    ensure!(saw_down, "the client never reported wheeld down");
    ensure!(up_again, "the client did not come back up after the restart (layouts {layouts})");
    draw(window, None, cx)?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    ensure!(layouts > layouts_before_restart, "no layout after the restart");
    wait(&native, "native views on the restarted daemon", |s| {
        webs.iter().all(|w| web_ready(s, w))
            && terms.iter().all(|t| s["panes"][t.as_str()]["pixels"] == true)
    }, cx).await?;
    // A new PTY answers keys typed through the reopened view: only the shell's output holds 42.
    native.call(json!({"op":"pty","pane":terms[0],"text":"echo RECONNECTED-$((6*7))\r"}));
    let typed_after = match wait(&native, "a restarted terminal answering", |s| viewport(s, terms[0]).contains("RECONNECTED-42"), cx).await {
        Ok(s) => s,
        Err(error) => {
            let term = window.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| &p.leaf.id == terms[0]).and_then(|p| p.leaf.term))?;
            let cli = std::env::var("WHEEL_CLIENT_CLI")?;
            let read = std::process::Command::new(&cli)
                .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "term", "read", term.as_deref().unwrap_or("?")])
                .output()?;
            anyhow::bail!("{error:#}\ndaemon term {term:?} reads: {}{}", String::from_utf8_lossy(&read.stdout), String::from_utf8_lossy(&read.stderr));
        }
    };
    draw(window, Some(output.join("client-reconnected.png")), cx)?;
    let identities_after: Vec<(String, String)> = native_panes.iter().map(|(id, _)| (id.clone(), identity(&typed_after, id))).collect();
    ensure!(identities_before.iter().zip(&identities_after).all(|(a, b)| a.1 != b.1), "a native view survived the restart: {identities_before:?} {identities_after:?}");
    let banner_after = window.update(cx, |c, _, _| c.link.banner())?;
    ensure!(banner_after.is_empty(), "banner still shown after reconnect: {banner_after}");
    let final_status = native.status();
    ensure!(final_status["visible"] == false && final_status["key_window"] == false && final_status["app_active"] == false, "check took screen/focus");
    let per_pane = |s: &Value| -> Value {
        native_panes.iter().map(|(id, kind)| (id.clone(), json!({"kind":kind,"frame":frame(s,id),"grid":last_grid(s,id)}))).collect::<serde_json::Map<_, _>>().into()
    };
    Ok(json!({
        "host_pid": std::process::id(), "layouts_received": layouts, "page": {"pane": native_pages[0], "state": page_state, "png_bytes": page_bytes}, "first_layout": first,
        "native_panes": native_panes, "loaded_at_start": loaded["panes"],
        "native_aligned_with_panes": aligned, "own_session_only": own,
        "keyboard": {"clicked": target, "click": click, "focused": focused, "typed_viewport": viewport(&typed, target), "leaked_to_first": leaked},
        "before": per_pane(&before),
        "drag": {"split": split, "from": start, "to": moved, "parent": [parent.x, parent.y, parent.w, parent.h],
            "after": per_pane(&after_status), "layout_after": after_drag, "ratio_saved_by_wheeld": saved},
        "external_ratio_change": {"cli_exit": status.code(), "ratio_command": 0.3, "layout": external, "panes": per_pane(&external_status)},
        "sessions_windows": sessions_windows,
        "agent": agent_report,
        "markdown": markdown_report,
        "footer": footer_report,
        "status": status_report,
        "web": web_report,
        "browser": browser_report,
        "pages": pages_report,
        "commands": commands_report,
        "restart": {"saw_down": saw_down, "down_reason": down_reason, "banner_while_down": banner_while_down, "layouts_before": layouts_before_restart,
            "layouts_after": layouts, "views_before": identities_before, "views_after": identities_after,
            "terminal_viewport_after": viewport(&typed_after, terms[0]), "banner_after": banner_after},
        "final": {"visible": final_status["visible"], "key_window": final_status["key_window"], "app_active": final_status["app_active"], "errors": final_status["errors"]},
    }))
}

/// The ANSI keyboard's key code for a character GPUI reads back from `keyCode`.
fn keycode(ch: char) -> u16 {
    match ch {
        'a' => 0, 's' => 1, 'd' => 2, 'f' => 3, 'h' => 4, 'g' => 5, 'z' => 6, 'x' => 7, 'c' => 8, 'v' => 9, 'b' => 11, 'q' => 12, 'w' => 13, 'e' => 14,
        'r' => 15, 'y' => 16, 't' => 17, '1' => 18, '2' => 19, '3' => 20, '4' => 21, '6' => 22, '5' => 23, '=' => 24, '9' => 25, '7' => 26, '-' => 27,
        '8' => 28, '0' => 29, ']' => 30, 'o' => 31, 'u' => 32, '[' => 33, 'i' => 34, 'p' => 35, 'l' => 37, 'j' => 38, '\'' => 39, 'k' => 40, ';' => 41,
        '\\' => 42, ',' => 43, '/' => 44, 'n' => 45, 'm' => 46, '.' => 47, ' ' => 49, '_' => 27, ':' => 41, upper if upper.is_ascii_uppercase() => keycode(upper.to_ascii_lowercase()), other => panic!("no key code for {other:?}: add it to keycode()"),
    }
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
