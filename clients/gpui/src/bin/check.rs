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
//! an older wheeld (the fixture daemon behind a proxy that lowers the protocol number in its
//! `status`): a second window shows only "The server was updated." with Restart Server, whose
//! click runs the stop and start commands, and surviving
//! a wheeld restart: the link reads down, comes back up on its own, and every native view is
//! opened again on the new daemon.
//! The window is never visible, key or active.
use anyhow::{Result, ensure};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use wheel_proto::{ClientMsg, ServerMsg};
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
/// Switch the window to `session` the way a person does: a real click on the title row's session
/// menu button, then one on the session's row.
async fn switch_session(window: gpui::WindowHandle<Client>, session: &str, cx: &mut AsyncApp) -> Result<()> {
    click_chip(window, Hit::SessionMenu, cx).await?;
    let (native, row) = window.update(cx, |c, _, _| (c.native(), c.title_menu_placed().and_then(|(_, rows)| rows.into_iter().find(|(r, _)| r.id == format!("session:{session}")).map(|(_, r)| r))))?;
    let row = row.ok_or_else(|| anyhow::anyhow!("the session menu lists no {session}"))?;
    let at = [f64::from(row.x + row.w / 2.), f64::from(row.y + row.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":at}));
    for _ in 0..3 {
        tick(cx, 100).await;
        draw(window, None, cx)?;
    }
    ensure!(window.update(cx, |c, _, _| c.title_menu.is_none())?, "the session menu stayed open after the row was clicked");
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

/// One request to the client's debug server from a thread, answered while this check keeps
/// pumping the connection and handing the server's requests to the window, as `serve_debug` does
/// in the real binary. Returns the status code and the JSON body.
async fn http(debug: &mut wheel_gpui::debug::Server, method: &str, target: &str, headers: &[(&str, &str)], connection: &mut Connection, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<(u16, Value)> {
    use std::io::{Read as _, Write as _};
    let port = debug.port;
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    let request = format!("{head}\r\n");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<u8>> {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
            stream.write_all(request.as_bytes())?;
            let mut out = Vec::new();
            stream.read_to_end(&mut out)?;
            Ok(out)
        })();
        let _ = tx.send(result);
    });
    let raw = loop {
        pump(connection, 50, layouts, window, cx).await?;
        while let Ok(req) = debug.requests.try_recv() {
            req.respond(wheel_gpui::answer_debug(window, &req, cx)?);
        }
        if let Ok(result) = rx.try_recv() {
            break result?;
        }
    };
    let text = String::from_utf8_lossy(&raw).into_owned();
    let code: u16 = text.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| anyhow::anyhow!("no status line in {text:?}"))?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Ok((code, serde_json::from_str(body).unwrap_or(Value::String(body.to_owned()))))
}

async fn pump(connection: &mut Connection, ms: u64, layouts: &mut usize, window: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Result<()> {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        while let Ok(event) = connection.incoming.try_recv() {
            if cx.update(|app| wheel_gpui::deliver(app, window, event))? {
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
    // The spaces sidebar (wheel-ynjo.5). Four more spaces join the first session, so the sidebar
    // lists five rows; then each thing the Swift sidebar does is driven here with real events or
    // the client's own entry points, and the layout wheeld broadcasts shows the result.
    let sidebar_state = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.sidebar_json());
    let session_order = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.session_spaces().iter().map(|s| (s.id.clone(), s.name.clone())).collect::<Vec<_>>());
    let (first_session, first_space) = shown(window, cx)?;
    let first_session_id = first_session.clone().ok_or_else(|| anyhow::anyhow!("no session shown"))?;
    let fixture_space = first_space.clone().ok_or_else(|| anyhow::anyhow!("no space shown"))?;
    let opening = sidebar_state(cx)?;
    ensure!(opening["open"] == true && opening["rows"].as_array().map(Vec::len) == Some(1), "the sidebar should open with the fixture's one row: {opening}");
    ensure!(!window.update(cx, |c, _, _| c.header_chips())?.iter().any(|c| matches!(c.hit, Hit::Space(_))), "the spaces row of chips is drawn while the sidebar is open");
    let grid_left = window.update(cx, |c, _, _| c.grid_area().x)?;
    ensure!(grid_left > wheel_gpui::sidebar::WIDTH, "the grid does not sit right of the sidebar: x {grid_left}");
    let mut made: Vec<String> = vec![];
    for n in 2..=5 {
        let dir = output.join(format!("sidebar-space-{n}"));
        std::fs::create_dir_all(&dir)?;
        let name = format!("sb{n}");
        let (ok, out, err) = run_pumped(&["space", "new", &dir.to_string_lossy(), "--name", &name, "--session", &first_session_id], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel space new {name} failed: {err}");
        made.push(out.lines().last().unwrap_or_default().trim().to_owned());
    }
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let five = sidebar_state(cx)?;
    let row_names: Vec<String> = five["rows"].as_array().unwrap_or(&vec![]).iter().map(|r| r["name"].as_str().unwrap_or_default().to_owned()).collect();
    ensure!(row_names.len() == 5 && row_names[1..] == ["sb2", "sb3", "sb4", "sb5"], "five rows in session order expected: {row_names:?}");
    ensure!(five["rows"][0]["active"] == true && !five["rows"][0]["dots"].as_array().map(Vec::is_empty).unwrap_or(true), "the fixture row should be active with the agent's mark: {}", five["rows"][0]);
    let rows_y: Vec<f64> = five["rows"].as_array().unwrap().iter().map(|r| r["frame"][1].as_f64().unwrap()).collect();
    ensure!(rows_y.windows(2).all(|w| w[1] > w[0]), "rows out of order: {rows_y:?}");
    let row_frame = |state: &Value, id: &str| -> Option<[f64; 4]> {
        state["rows"].as_array()?.iter().find(|r| r["id"] == id).map(|r| [r["frame"][0].as_f64().unwrap(), r["frame"][1].as_f64().unwrap(), r["frame"][2].as_f64().unwrap(), r["frame"][3].as_f64().unwrap()])
    };
    let centre = |f: [f64; 4]| [f[0] + f[2] / 2., f[1] + f[3] / 2.];
    let (sb2, sb3, sb4, sb5) = (made[0].clone(), made[1].clone(), made[2].clone(), made[3].clone());
    // A key as the field gets it, through the same `key()` every real key takes.
    let type_keys = |cx: &mut AsyncApp, keys: &[(&str, Option<&str>)]| -> Result<()> {
        for (key, ch) in keys {
            let keystroke = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: (*key).to_owned(), key_char: ch.map(str::to_owned) };
            window.update(cx, |c, _, cx| { c.key(&gpui::KeyDownEvent { keystroke, is_held: false, prefer_character_input: true }, cx); })?;
        }
        Ok(())
    };
    // The context menu opens on a real control-click (what a right press also does) and lists
    // what the Swift menu lists.
    let sb2_at = centre(row_frame(&five, &sb2).unwrap());
    native.call(json!({"op":"post_mouse","kind":"down","point":sb2_at,"control":true}));
    native.call(json!({"op":"post_mouse","kind":"up","point":sb2_at,"control":true}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let with_menu = sidebar_state(cx)?;
    let menu_ids: Vec<String> = with_menu["menu"]["items"].as_array().unwrap_or(&vec![]).iter().map(|i| i["id"].as_str().unwrap_or_default().to_owned()).collect();
    ensure!(with_menu["menu"]["space"] == sb2 && menu_ids.contains(&"rename".to_owned()) && menu_ids.contains(&"close-others".to_owned()) && menu_ids.iter().any(|i| i.starts_with("session:")), "the control-click did not open sb2's menu: {}", with_menu["menu"]);
    let left_disabled = with_menu["menu"]["items"].as_array().unwrap().iter().find(|i| i["id"] == "left").map(|i| i["enabled"] == false).unwrap_or(false);
    ensure!(!left_disabled, "sb2 is second, so Move Left is enabled: {}", with_menu["menu"]);
    // Rename…: the row becomes a field; an empty name is refused in place; Return sends the name
    // and the layout broadcast shows it.
    window.update(cx, |c, _, cx| c.run_space_menu("rename", &sb2, cx))?;
    let renaming = sidebar_state(cx)?;
    ensure!(renaming["menu"].is_null() && renaming["renaming"]["space"] == sb2 && renaming["renaming"]["draft"] == "sb2", "Rename did not open the field: {}", renaming["renaming"]);
    type_keys(cx, &[("backspace", None), ("backspace", None), ("backspace", None), ("enter", None)])?;
    let refused = sidebar_state(cx)?;
    ensure!(refused["renaming"]["error"] == "A space needs a name.", "an empty name should be refused in place: {}", refused["renaming"]);
    type_keys(cx, &[("r", Some("r")), ("e", Some("e")), ("n", Some("n")), ("a", Some("a")), ("m", Some("m")), ("e", Some("e")), ("d", Some("d")), ("enter", None)])?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let renamed = sidebar_state(cx)?;
    ensure!(renamed["renaming"].is_null(), "the rename field stayed open: {}", renamed["renaming"]);
    ensure!(session_order(cx)?.iter().any(|(id, name)| id == &sb2 && name == "renamed"), "wheeld did not list the new name: {:?}", session_order(cx)?);
    // Escape cancels a rename and leaves the name alone.
    window.update(cx, |c, _, _| c.begin_rename(&sb3))?;
    type_keys(cx, &[("x", Some("x")), ("escape", None)])?;
    ensure!(sidebar_state(cx)?["renaming"].is_null() && session_order(cx)?.iter().any(|(id, name)| id == &sb3 && name == "sb3"), "Escape did not cancel the rename");
    // Drag to reorder: a real press on sb5's row, moved to the top of the list, shows the bar
    // above the first row and the hint; releasing puts sb5 first.
    let before_drag = sidebar_state(cx)?;
    let from = centre(row_frame(&before_drag, &sb5).unwrap());
    let top = row_frame(&before_drag, &fixture_space).unwrap();
    let to = [top[0] + 20., top[1] + 2.];
    native.call(json!({"op":"post_mouse","kind":"down","point":from}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":[from[0], from[1] - 10.]}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":to}));
    tick(cx, 150).await;
    draw(window, None, cx)?;
    let dragging = sidebar_state(cx)?;
    ensure!(dragging["drag"]["space"] == sb5 && dragging["drag"]["insertion"] == 0 && dragging["drag"]["hint"].as_str().is_some_and(|h| h.starts_with("Place “sb5” here")), "the drag did not aim at the top: {}", dragging["drag"]);
    native.call(json!({"op":"post_mouse","kind":"up","point":to}));
    pump(&mut connection, 1000, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let order_after_drag: Vec<String> = session_order(cx)?.into_iter().map(|(id, _)| id).collect();
    ensure!(order_after_drag.first() == Some(&sb5) && order_after_drag.get(1) == Some(&fixture_space) && sidebar_state(cx)?["drag"].is_null(), "the drop did not put sb5 first: {order_after_drag:?}");
    ensure!(shown(window, cx)?.1.as_deref() == Some(sb5.as_str()), "the press on sb5's row should have shown sb5 (a sidebar selects on the press)");
    // Move Left from the menu, the keyboardless way, puts the fixture space back first.
    window.update(cx, |c, _, cx| c.run_space_menu("left", &fixture_space, cx))?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    ensure!(session_order(cx)?.first().map(|(id, _)| id.clone()) == Some(fixture_space.clone()), "Move Left did not move the fixture space first: {:?}", session_order(cx)?);
    // A pane dragged by its header onto another space's row moves there: sb3's pane, shown and
    // dragged onto sb4's row, lands at the end of sb4, and sb3 keeps one empty pane.
    let sb3_at = centre(row_frame(&sidebar_state(cx)?, &sb3).unwrap());
    native.call(json!({"op":"post_mouse","kind":"down","point":sb3_at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":sb3_at}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    ensure!(shown(window, cx)?.1.as_deref() == Some(sb3.as_str()), "a click on sb3's row did not show sb3");
    let (sb3_pane, sb3_rect) = window.update(cx, |c, _, _| c.scene().0.first().map(|p| (p.leaf.id.clone(), p.rect)))?.ok_or_else(|| anyhow::anyhow!("sb3 shows no pane"))?;
    let grab = [f64::from(sb3_rect.x + 60.), f64::from(sb3_rect.y + TITLE / 2.)];
    let sb4_at = centre(row_frame(&sidebar_state(cx)?, &sb4).unwrap());
    native.call(json!({"op":"post_mouse","kind":"down","point":grab}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":[grab[0] - 10., grab[1]]}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":sb4_at}));
    tick(cx, 150).await;
    draw(window, Some(output.join("client-pane-drag.png")), cx)?;
    let pane_dragging = sidebar_state(cx)?;
    let sb4_row = pane_dragging["rows"].as_array().unwrap().iter().find(|r| r["id"] == sb4).cloned().unwrap_or_default();
    ensure!(pane_dragging["pane_drag"]["pane"] == sb3_pane && sb4_row["pane_over"] == true && pane_dragging["pane_drag"]["hint"].as_str().is_some_and(|h| h.contains("to the end of “sb4”")), "the pane drag did not aim at sb4's row: {}", pane_dragging["pane_drag"]);
    native.call(json!({"op":"post_mouse","kind":"up","point":sb4_at}));
    pump(&mut connection, 1000, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let holder = window.update(cx, |c, _, _| c.sidebar_json()["rows"].as_array().unwrap().iter().map(|r| (r["id"].as_str().unwrap().to_owned(), r["panes"].as_u64().unwrap())).collect::<Vec<_>>())?;
    let pane_space = window.update(cx, |c, _, _| c.state_json()["panes"][sb3_pane.as_str()]["space"].clone())?;
    // sb4's only pane was empty, so wheeld drops it: the moved pane is now sb4's one pane, and sb3 got a new empty one.
    let sb3_panes = window.update(cx, |c, _, _| c.state_json()["spaces"].as_array().unwrap().iter().find(|s| s["id"] == sb3).map(|s| s["panes"].clone()).unwrap_or_default())?;
    ensure!(pane_space == sb4 && holder.iter().any(|(id, n)| id == &sb4 && *n == 1) && sb3_panes.as_array().is_some_and(|p| p.len() == 1 && p[0] != sb3_pane), "the pane did not move to sb4: pane in {pane_space}, rows {holder:?}, sb3 holds {sb3_panes}");
    // The hover preview: a pointer held over the fixture's row opens, after its delay, the card
    // with that space's six panes, the agent pane's last line among them; moving off closes it.
    let fixture_at = centre(row_frame(&sidebar_state(cx)?, &fixture_space).unwrap());
    native.call(json!({"op":"post_mouse","kind":"move","point":fixture_at}));
    let mut preview = Value::Null;
    for _ in 0..8 {
        tick(cx, 100).await;
        draw(window, None, cx)?;
        preview = sidebar_state(cx)?["preview"].clone();
        if !preview.is_null() {
            break;
        }
    }
    ensure!(preview["space"] == fixture_space && preview["panes"].as_array().map(Vec::len) == Some(6), "the preview did not open on the fixture space: {preview}");
    let agent_last = preview["panes"].as_array().unwrap().iter().find(|p| p["kind"] == "agent").map(|p| p["last"].clone()).unwrap_or_default();
    ensure!(agent_last.as_str().is_some_and(|l| !l.is_empty()), "the agent pane's last line is missing from the preview: {preview}");
    pump(&mut connection, 200, &mut layouts, window, cx).await?;
    draw(window, Some(output.join("client-sidebar.png")), cx)?;
    let far = [f64::from(grid_left + 200.), f64::from(TITLE + 300.)];
    native.call(json!({"op":"post_mouse","kind":"move","point":far}));
    tick(cx, 250).await;
    draw(window, None, cx)?;
    ensure!(sidebar_state(cx)?["preview"].is_null(), "the preview stayed open after the pointer left the row");
    // The row's close button closes the space: at once, since nothing runs in sb5 (a space with a
    // shell or an agent asks first; Close Other Spaces below asks).
    let close_at = {
        let s = sidebar_state(cx)?;
        let r = s["rows"].as_array().unwrap().iter().find(|r| r["id"] == sb5).cloned().unwrap();
        [r["close"][0].as_f64().unwrap() + r["close"][2].as_f64().unwrap() / 2., r["close"][1].as_f64().unwrap() + r["close"][3].as_f64().unwrap() / 2.]
    };
    native.call(json!({"op":"post_mouse","kind":"down","point":close_at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":close_at}));
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    ensure!(window.update(cx, |c, _, _| c.confirmation.is_none())? && !session_order(cx)?.iter().any(|(id, _)| id == &sb5), "sb5 is still listed after its close button");
    // Move to <the other session> takes a row out of this window's list.
    let second_session_id = std::env::var("WHEEL_CLIENT_SECOND_SESSION")?;
    window.update(cx, |c, _, cx| c.run_space_menu(&format!("session:{second_session_id}"), &sb4, cx))?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    let second_holds = window.update(cx, |c, _, _| c.state_json()["sessions"].as_array().unwrap().iter().find(|s| s["id"] == second_session_id).map(|s| s["spaces"].clone()).unwrap_or_default())?;
    ensure!(!session_order(cx)?.iter().any(|(id, _)| id == &sb4) && second_holds.as_array().is_some_and(|l| l.last() == Some(&json!(sb4))), "sb4 did not move to the end of the other session: it holds {second_holds}");
    // The toggle hides the sidebar: the spaces row of chips comes back and the grid takes the width.
    window.update(cx, |c, _, _| c.toggle_sidebar())?;
    draw(window, None, cx)?;
    let hidden_bar = sidebar_state(cx)?;
    let chips_with_spaces = window.update(cx, |c, _, _| c.header_chips().iter().filter(|c| matches!(c.hit, Hit::Space(_))).count())?;
    ensure!(hidden_bar["open"] == false && chips_with_spaces == 3 && window.update(cx, |c, _, _| c.grid_area().x)? == 0., "hiding the sidebar should show three space chips over a full-width grid: {chips_with_spaces} chips");
    window.update(cx, |c, _, _| c.toggle_sidebar())?;
    // Close Other Spaces in Window… from the fixture row's menu closes the two made here.
    window.update(cx, |c, _, cx| c.run_space_menu("close-others", &fixture_space, cx))?;
    let asked = window.update(cx, |c, _, _| c.confirmation.as_ref().map(|c| (c.title.clone(), c.button.clone())))?;
    ensure!(asked.as_ref().map(|a| a.1.as_str()) == Some("Close Other Spaces") && asked.as_ref().is_some_and(|a| a.0.contains("2 other spaces")), "Close Other Spaces did not ask as expected: {asked:?}");
    window.update(cx, |c, _, _| c.answer_confirmation(true))?;
    pump(&mut connection, 1200, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    let left_over: Vec<String> = session_order(cx)?.into_iter().map(|(id, _)| id).collect();
    ensure!(left_over == [fixture_space.clone()], "only the fixture space should remain in the first session: {left_over:?}");
    // Back on the fixture space, as the sections below expect.
    window.update(cx, |c, _, _| c.select_space(&fixture_space))?;
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    ensure!(shown(window, cx)?.1.as_deref() == Some(fixture_space.as_str()), "the fixture space is not shown again");
    let sidebar_report = json!({"rows_with_five": row_names, "menu": menu_ids, "renamed": "renamed", "refusal": refused["renaming"]["error"],
        "drag_hint": dragging["drag"]["hint"], "order_after_drag": order_after_drag, "pane_drag_hint": pane_dragging["pane_drag"]["hint"], "pane_moved_to": pane_space,
        "preview_panes": preview["panes"].as_array().map(Vec::len), "preview_agent_last": agent_last, "closed_by_button": sb5, "moved_to_other_session": sb4, "left_over": left_over});
    native.call(json!({"op":"post_mouse","kind":"down","point":click}));
    native.call(json!({"op":"post_mouse","kind":"up","point":click}));
    tick(cx, 300).await;
    // The title row and the bottom row (wheel-ynjo.6; ADR-0037, ADR-0034). The fixture's machine
    // file has `build` (a state command: icon, tint and badge every second) and `vpn` entries and
    // hides the pin; the project's file puts `vpn` on the right with its own command and adds
    // `deploy`, which has none. wheeld reads usage from the fixture's file: two Claude logins, one
    // signed out, and a Codex Pro Lite login.
    let title_state = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.title_json());
    let row_ids = |t: &Value| t["row"].as_array().map(|r| r.iter().filter_map(|c| c["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
    let chip_of = |t: &Value, id: &str| t["row"].as_array().and_then(|r| r.iter().find(|c| c["id"] == id).cloned()).unwrap_or(Value::Null);
    let mut title = Value::Null;
    for _ in 0..60 {
        pump(&mut connection, 200, &mut layouts, window, cx).await?;
        title = title_state(cx)?;
        if chip_of(&title, "plugin-build")["badge"] == "2" && chip_of(&title, "plugin-vpn")["icon"] == "lock.open" {
            break;
        }
    }
    draw(window, None, cx)?;
    let order = row_ids(&title);
    ensure!(order == ["sidebar", "session", "mode", "plugin-build", "plugin-deploy", "title", "plugin-vpn", "processes", "palette", "new-space"], "the title row's order: {order:?} ({title})");
    let build = chip_of(&title, "plugin-build");
    ensure!(build["icon"] == "hammer.fill" && build["tint"] == "green" && build["badge"] == "2", "the machine entry's state did not land: {build}");
    let vpn = chip_of(&title, "plugin-vpn");
    ensure!(vpn["label"] == "Project VPN" && vpn["pressable"] == true, "the project's vpn entry: {vpn}");
    ensure!(chip_of(&title, "plugin-deploy")["pressable"] == false, "deploy has no command and should not be pressable");
    let frames: Vec<[f64; 4]> = title["row"].as_array().unwrap().iter().filter(|c| c["id"] != "title").map(|c| [c["frame"][0].as_f64().unwrap(), c["frame"][1].as_f64().unwrap(), c["frame"][2].as_f64().unwrap(), c["frame"][3].as_f64().unwrap()]).collect();
    ensure!(frames.iter().all(|f| f[1] == 6. && f[2] == 28. && f[3] == 26.), "every button sits in a 28×26 slot at y 6: {frames:?}");
    // A real click on vpn runs the project's command in the space's directory.
    let project_dir = std::path::PathBuf::from(std::env::var("WHEEL_CLIENT_HTML")?).parent().unwrap().to_path_buf();
    let pressed_file = project_dir.join("pressed.txt");
    if let Err(e) = std::fs::remove_file(&pressed_file) && e.kind() != std::io::ErrorKind::NotFound {
        anyhow::bail!("could not clear {}: {e}", pressed_file.display());
    }
    click_chip(window, Hit::Plugin("vpn".into()), cx).await?;
    let mut pressed = String::new();
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if let Ok(text) = std::fs::read_to_string(&pressed_file)
            && !text.trim().is_empty()
        {
            pressed = text.trim().to_owned();
            break;
        }
    }
    ensure!(pressed == "project-pressed", "the vpn press did not run the project's command: {pressed:?}");
    window.update(cx, |c, _, _| c.press_plugin("deploy"))?;
    let log_has = |cx: &mut AsyncApp, needle: &str| -> Result<bool> { Ok(window.update(cx, |c, _, _| c.state_json()["log"].as_array().is_some_and(|l| l.iter().any(|e| e.as_str().is_some_and(|e| e.contains(needle)))))?) };
    ensure!(log_has(cx, "title button deploy: an indicator")?, "a press on deploy should be ignored and logged");
    // The mode menu, opened by a real click: Desk hides every pane and draws the placeholder.
    click_chip(window, Hit::Mode, cx).await?;
    let menu = title_state(cx)?["menu"].clone();
    ensure!(menu["kind"] == "mode" && menu["rows"][0]["id"] == "mode:spaces" && menu["rows"][0]["checked"] == true && menu["rows"][1]["id"] == "mode:desk", "the mode menu: {menu}");
    window.update(cx, |c, _, _| c.run_title_menu("mode:desk"))?;
    draw(window, None, cx)?;
    wait(&native, "every pane hidden in desk mode", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == true), cx).await?;
    draw(window, Some(output.join("client-desk.png")), cx)?;
    let mode_after = title_state(cx)?["mode"].clone();
    ensure!(mode_after == "desk" && title_state(cx)?["menu"].is_null(), "desk mode did not take: {mode_after}");
    window.update(cx, |c, _, _| c.set_mode("spaces"))?;
    draw(window, None, cx)?;
    wait(&native, "the panes shown again in spaces mode", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == false), cx).await?;
    // The pin (its button is hidden by the fixture's entry, so through the action the button runs).
    window.update(cx, |c, _, _| c.set_pinned(true))?;
    ensure!(title_state(cx)?["pinned"] == true && log_has(cx, " pinned")?, "the pin did not take");
    window.update(cx, |c, _, _| c.set_pinned(false))?;
    ensure!(title_state(cx)?["pinned"] == false && log_has(cx, " unpinned")?, "the pin did not come off");
    // The session menu, opened by a real click: the sessions with the shown one checked, then the verbs; Esc closes it.
    click_chip(window, Hit::SessionMenu, cx).await?;
    let session_menu = title_state(cx)?["menu"].clone();
    let menu_ids: Vec<String> = session_menu["rows"].as_array().unwrap_or(&vec![]).iter().map(|r| r["id"].as_str().unwrap_or_default().to_owned()).collect();
    let session_count = window.update(cx, |c, _, _| c.state_json()["sessions"].as_array().map(Vec::len).unwrap_or(0))?;
    ensure!(session_menu["kind"] == "session" && menu_ids.len() == session_count + 3 && menu_ids[session_count..] == ["new-session", "rename-session", "close-session"] && menu_ids[0] == format!("session:{first_session_id}") && session_menu["rows"][0]["checked"] == true, "the session menu: {session_menu}");
    window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke: wheel_gpui::keymap::KeyChord::parse("escape").unwrap().keystroke(), is_held: false, prefer_character_input: false }, cx))?;
    ensure!(title_state(cx)?["menu"].is_null(), "Esc did not close the session menu");
    // The sidebar hidden: the spaces become tabs in the title row, 220 pt wide for one space, and the title goes.
    window.update(cx, |c, _, _| c.set_sidebar(false))?;
    draw(window, Some(output.join("client-title-row.png")), cx)?;
    let tabs = title_state(cx)?;
    let tab_ids: Vec<String> = row_ids(&tabs).into_iter().filter(|id| id.starts_with("tab:")).collect();
    ensure!(tab_ids == [format!("tab:{fixture_space}")] && tabs["tabs"]["tab_width"] == 220. && !row_ids(&tabs).iter().any(|id| id == "title"), "the tabs with the sidebar hidden: {tabs}");
    window.update(cx, |c, _, _| c.set_sidebar(true))?;
    draw(window, None, cx)?;
    // The bottom row: the badge, and the strip from the fixture's readings (the folder label prints
    // for Claude's two logins, not for Codex's one; the signed-out login shows its note).
    ensure!(tabs["connection_badge"] == "Connected to wheeld", "the badge: {}", tabs["connection_badge"]);
    let usage = window.update(cx, |c, _, _| c.usage_json())?;
    let strip_rows = usage["strip"].as_array().cloned().unwrap_or_default();
    let summary: Vec<String> = strip_rows.iter().map(|e| format!("{} {} [{}] {}", e["agent"].as_str().unwrap_or("?"), e["account"].as_str().unwrap_or("-"),
        e["labels"].as_array().map(|l| l.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
        e["rows"].as_array().map(|r| r.iter().map(|w| format!("{}={}%", w["id"].as_str().unwrap_or("?"), w["pct"].as_f64().unwrap_or(0.))).collect::<Vec<_>>().join(" ")).unwrap_or_default())).collect();
    ensure!(summary == ["claude .claude [.claude] 5h=3% weekly=86%", "claude .claude-work [.claude-work] ", "codex .codex-work [] weekly=92%"], "the strip: {summary:?} ({usage})");
    ensure!(strip_rows[1]["note"].as_str().is_some_and(|n| n.starts_with("signed out")), "the signed-out login's note: {}", strip_rows[1]);
    // The Usage window; Full on Claude's weekly window writes `usage.meters` for every Claude login
    // and the strip's row gains its pace tick and reset clock.
    window.update(cx, |c, _, _| c.open_usage())?;
    draw(window, None, cx)?;
    ensure!(window.update(cx, |c, _, _| c.usage_open)?, "the Usage window did not open");
    window.update(cx, |c, _, _| c.set_meter("claude", "weekly", Some(wheel_proto::UsageMeterLevel::Full)))?;
    let mut meters: Vec<Value> = vec![];
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        meters = window.update(cx, |c, _, _| c.usage_json()["meters"].as_array().cloned().unwrap_or_default())?;
        if meters.iter().any(|m| m == "claude:weekly:full") {
            break;
        }
    }
    ensure!(meters == ["claude:5h:meter", "claude:weekly:full", "codex:5h:meter", "codex:weekly:meter"], "usage.meters after Full: {meters:?}");
    draw(window, Some(output.join("client-usage.png")), cx)?;
    let full_row = window.update(cx, |c, _, _| c.usage_json()["strip"][0]["rows"][1].clone())?;
    ensure!(full_row["level"] == "full" && full_row["resets"].is_string() && full_row["pace"].is_number(), "the full row: {full_row}");
    window.update(cx, |c, _, _| c.set_meter("claude", "weekly", Some(wheel_proto::UsageMeterLevel::Meter)))?;
    pump(&mut connection, 600, &mut layouts, window, cx).await?;
    window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke: wheel_gpui::keymap::KeyChord::parse("escape").unwrap().keystroke(), is_held: false, prefer_character_input: false }, cx))?;
    draw(window, None, cx)?;
    ensure!(!window.update(cx, |c, _, _| c.usage_open)?, "Esc did not close the Usage window");
    let title_report = json!({"order": order, "build": build, "vpn": vpn, "pressed": pressed, "mode_after": mode_after, "session_menu": menu_ids, "tabs": tab_ids, "tab_width": tabs["tabs"]["tab_width"],
        "badge": tabs["connection_badge"], "strip": summary, "meters_after_full": meters, "full_row": full_row});
    // Sessions and windows. The fixture holds two sessions; this window shows the first.
    let (first_session, first_space) = shown(window, cx)?;
    let (second_session, second_space) = (std::env::var("WHEEL_CLIENT_SECOND_SESSION")?, std::env::var("WHEEL_CLIENT_SECOND_SPACE")?);
    let id1 = window.update(cx, |c, _, _| c.window_id().to_owned())?;
    let chips_first = window.update(cx, |c, _, _| c.header_chips())?;
    ensure!(chips_first.iter().any(|c| c.hit == Hit::SessionMenu) && chips_first.iter().any(|c| c.hit == Hit::Title), "the title row lacks the session menu or the title: {chips_first:?}");
    reported(&id1, "the first session", |w| w.1 == first_session && w.2 == first_space, cx).await?;
    switch_session(window, &second_session, cx).await?;
    let on_second = shown(window, cx)?;
    ensure!(on_second == (Some(second_session.clone()), Some(second_space.clone())), "clicking the second session shows {on_second:?}");
    reported(&id1, "the second session and its space", |w| w.1.as_deref() == Some(second_session.as_str()) && w.2.as_deref() == Some(second_space.as_str()), cx).await?;
    let hidden_native = wait(&native, "the first space's views hidden", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == true), cx).await?;
    draw(window, Some(output.join("client-second-session.png")), cx)?;
    switch_session(window, first_session.as_deref().unwrap(), cx).await?;
    ensure!(shown(window, cx)? == (first_session.clone(), first_space.clone()), "clicking the first session did not come back");
    wait(&native, "the first space's views shown again", |st| native_panes.iter().all(|(id, _)| st["panes"][id.as_str()]["hidden"] == false), cx).await?;
    // New Window opens a second window with a connection of its own, showing the same space.
    let before_windows = cx.update(|app| app.windows().len());
    window.update(cx, |c, _, cx| c.run_action("new-window", cx))?;
    for _ in 0..3 {
        tick(cx, 100).await;
        draw(window, None, cx)?;
    }
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
    switch_session(second, &second_session, cx).await?;
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
    // `wheel window` and the window menus (wheel-ynjo.7.2; AppModel+Windows.swift): this app
    // offers to run window commands; wheeld forwards each to the newest window that offered, which
    // answers from the app's list of windows and acts on the others through their handles.
    let window_ls = |out: &str| -> Result<Value> { serde_json::from_str(out).map_err(|e| anyhow::anyhow!("wheel window ls --json printed {out:?}: {e}")) };
    let (ok, out, err) = run_pumped(&["window", "ls", "--json"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window ls failed: {err}");
    let listed = window_ls(&out)?;
    let first_space_name = window.update(cx, |c, _, _| c.space_id().and_then(|s| c.space_name(s)))?;
    ensure!(listed["windows"].as_array().map(Vec::len) == Some(1) && listed["windows"][0]["id"] == json!(id1), "wheel window ls lists {listed}");
    ensure!(listed["windows"][0]["title"] == json!(first_space_name) && listed["windows"][0]["session"] == json!(first_session) && listed["windows"][0]["focused"] == true, "the window reads {}", listed["windows"][0]);
    ensure!(listed["background"].as_array().is_some_and(|b| !b.is_empty()), "the second session's spaces are not the background: {listed}");
    // Rename by id, then by the new name; an empty name clears it.
    let (ok, _, err) = run_pumped(&["window", "rename", &id1, "Notes"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && window.update(cx, |c, _, _| c.window_name.clone())?.as_deref() == Some("Notes"), "wheel window rename left the name {:?}: {err}", window.update(cx, |c, _, _| c.window_name.clone())?);
    let (_, out, _) = run_pumped(&["window", "ls"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(out.contains(&format!("{id1}  Notes  ")), "wheel window ls prints {out:?}");
    let (ok, _, err) = run_pumped(&["window", "rename", "Notes", ""], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok && window.update(cx, |c, _, _| c.window_name.is_none())?, "an empty name kept the name: {err}");
    let (ok, _, err) = run_pumped(&["window", "rename", "Nowhere", "x"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(!ok && err.contains("no window Nowhere; the windows are"), "renaming a window that is not there said {err:?}");
    // A new window, named, on the session the focused window shows.
    let before_new = cx.update(|app| app.windows().len());
    let (ok, out, err) = run_pumped(&["window", "new", "--name", "Second"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window new failed: {err}");
    let new_id = out.trim().to_owned();
    let made = cx.update(|_| wheel_gpui::window_handle(&new_id)).ok_or_else(|| anyhow::anyhow!("wheel window new printed {new_id:?}, which is no window"))?;
    ensure!(cx.update(|app| app.windows().len()) == before_new + 1, "wheel window new opened no window");
    for _ in 0..30 {
        if made.update(cx, |c, _, _| c.session_id().is_some())? {
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(made.update(cx, |c, _, _| (c.window_name.clone(), c.session_id().map(str::to_owned)))? == (Some("Second".into()), first_session.clone()), "the new window is {:?}", made.update(cx, |c, _, _| (c.window_name.clone(), c.session_id().map(str::to_owned)))?);
    // `wheel space move` puts a new space in the named window's session and shows it there.
    let dir = std::env::temp_dir().join(format!("wheel-gpui-window-move-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let (ok, out, err) = run_pumped(&["space", "new", &dir.to_string_lossy(), "--name", "moved"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space new failed: {err}");
    let moved = out.trim().to_owned();
    let (ok, out, err) = run_pumped(&["space", "move", "moved", "Second"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space move failed: {err} {out}");
    let mut placed = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if made.update(cx, |c, _, _| c.space_id() == Some(moved.as_str()))? {
            placed = true;
            break;
        }
    }
    ensure!(placed, "the moved space never showed in the window: {:?}", shown(made, cx)?);
    ensure!(window.update(cx, |c, _, _| c.session_spaces().iter().any(|s| s.id == moved))?, "the moved space is not in the session the window shows");
    let (ok, _, err) = run_pumped(&["space", "move", "moved", "background"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(!ok && err.contains("a space is always in a session"), "moving to the background said {err:?}");
    let (ok, _, err) = run_pumped(&["space", "close", &moved], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space close failed: {err}");
    let _ = std::fs::remove_dir_all(&dir);
    // Two windows show the session: closing one asks nothing.
    let (ok, _, err) = run_pumped(&["window", "close", "Second"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window close failed: {err}");
    for _ in 0..20 {
        if cx.update(|_| wheel_gpui::window_handle(&new_id)).is_none() {
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(cx.update(|_| wheel_gpui::window_handle(&new_id)).is_none(), "the second window on a shared session stayed open");
    // The last window on a session with spaces asks; Escape and Cancel keep it, as the sheet does.
    let (ok, _, err) = run_pumped(&["window", "close", &id1], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window close on the last window failed: {err}");
    let summary = window.update(cx, |c, _, _| c.closing_window.then(|| c.close_summary()))?;
    let expect_summary = window.update(cx, |c, _, _| {
        let held = c.session_spaces();
        let panes: usize = held.iter().map(|s| wheel_gpui::layout::leaves_of(&s.root).len()).sum();
        let session = c.layout_lists().0.into_iter().find(|s| Some(s.id.as_str()) == c.session_id()).map(|s| s.name).unwrap_or_default();
        format!("The “{session}” session holds {} {} with {panes} {}.", held.len(), if held.len() == 1 { "space" } else { "spaces" }, if panes == 1 { "pane" } else { "panes" })
    })?;
    ensure!(summary.as_deref() == Some(expect_summary.as_str()), "the close question reads {summary:?}, not {expect_summary:?}");
    draw(window, Some(output.join("client-window-close.png")), cx)?;
    let escape = wheel_gpui::keymap::KeyChord::parse("escape").unwrap().keystroke();
    window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke: escape.clone(), is_held: false, prefer_character_input: false }, cx))?;
    ensure!(window.update(cx, |c, _, _| !c.closing_window)?, "Escape left the close question up");
    ensure!(!window.update(cx, |c, _, cx| { let now = c.ask_close_window(); cx.notify(); now })?, "the last window closed with no question");
    window.update(cx, |c, _, _| c.answer_window_close(wheel_gpui::view::CloseAnswer::Cancel))?;
    ensure!(window.update(cx, |c, _, _| !c.closing_window)? && cx.update(|_| wheel_gpui::window_handle(&id1)).is_some(), "Cancel closed the window");
    // Rename Window… from the palette's action: the title row shows the field over the tabs.
    window.update(cx, |c, _, cx| c.run_action("rename-window", cx))?;
    for ch in ["D", "e", "s", "k"] {
        let keystroke = gpui::Keystroke { modifiers: gpui::Modifiers { shift: ch == "D", ..Default::default() }, key: ch.to_lowercase(), key_char: Some(ch.into()) };
        window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke, is_held: false, prefer_character_input: false }, cx))?;
    }
    ensure!(window.update(cx, |c, _, _| c.renaming_window() && c.header_chips().iter().any(|chip| chip.hit == Hit::Title))?, "Rename Window shows no field in the title row");
    draw(window, Some(output.join("client-window-rename.png")), cx)?;
    let enter = wheel_gpui::keymap::KeyChord::parse("enter").unwrap().keystroke();
    window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke: enter.clone(), is_held: false, prefer_character_input: false }, cx))?;
    ensure!(window.update(cx, |c, _, _| (c.window_name.clone(), c.window_title()))? == (Some("Desk".into()), "Desk".into()), "Rename Window named it {:?}", window.update(cx, |c, _, _| c.window_name.clone())?);
    draw(window, Some(output.join("client-window-named.png")), cx)?;
    window.update(cx, |c, _, _| c.set_window_name(None))?;
    // Rename Session… from the session menu: a card with the name in a field; Return sends it.
    let old_session_name = window.update(cx, |c, _, _| c.layout_lists().0.into_iter().find(|s| Some(s.id.as_str()) == c.session_id()).map(|s| s.name))?.unwrap_or_default();
    window.update(cx, |c, _, _| {
        c.toggle_title_menu(wheel_gpui::view::TitleMenu::Session);
        c.run_title_menu("rename-session");
    })?;
    ensure!(window.update(cx, |c, _, _| c.name_edit.as_ref().map(|e| e.draft.clone()))? == Some(old_session_name.clone()), "Rename Session… opened no field holding {old_session_name:?}");
    window.update(cx, |c, _, _| { if let Some(e) = c.name_edit.as_mut() { e.draft = "Renamed".into(); } })?;
    draw(window, Some(output.join("client-session-rename.png")), cx)?;
    window.update(cx, |c, _, cx| c.key(&gpui::KeyDownEvent { keystroke: enter.clone(), is_held: false, prefer_character_input: false }, cx))?;
    let mut renamed = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.layout_lists().0.iter().any(|s| Some(s.id.as_str()) == c.session_id() && s.name == "Renamed"))? {
            renamed = true;
            break;
        }
    }
    ensure!(renamed, "the session was never renamed");
    let back = first_session.clone().unwrap_or_default();
    connection.send.send(wheel_gpui::daemon::frame(&ClientMsg::SessionRename { req: None, session: back, name: old_session_name.clone() }))?;
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    let windows_report = json!({"ls": listed, "new": new_id, "moved": moved, "close_summary": summary, "session_renamed_from": old_session_name});
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
    // Backspace carries DEL (0x7f) on a Mac keyboard; an uppercase letter and `_` (the temp dir's random suffix may hold one) are their keys with shift.
    for _ in 0..one.to_string_lossy().chars().count() {
        native.call(json!({"op":"post_key","text":"\u{7f}","code":51}));
    }
    tick(cx, 100).await;
    for ch in two.to_string_lossy().chars() {
        native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch),"shift":ch.is_ascii_uppercase() || ch == '_'}));
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
    switch_session(window, &pages_session, cx).await?;
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
    switch_session(window, first_session.as_deref().unwrap(), cx).await?;
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
    // The Settings window (SettingsWindow.swift and its tabs): ⌘, opens it over the window with
    // the daemon's rows; a field typed and typed_saved reaches wheeld's wheel.toml and `wheel config`
    // reads it; a write from another client (the CLI here) shows in the open window; the Chat
    // page's controls, search and toml; a project's own file; the built-ins, guidance, keys and
    // devices pages; Esc closes it.
    use wheel_gpui::settings::{Layer, Tab, Topic};
    let config_get = |key: &str, space: Option<&str>| -> Result<String> {
        let mut args = vec!["get", key];
        if let Some(space) = space {
            args.extend(["--space", space]);
        }
        let out = config(&args)?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    // A typed character that needs Shift on a Mac keyboard (`:`, an uppercase letter) is posted with it.
    let type_text = |native: &Native, text: &str| {
        for ch in text.chars() {
            let shifted = ch.is_ascii_uppercase() || ":\"<>?{}|~!@#$%^&*()_+".contains(ch);
            native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch),"shift":shifted}));
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    post(&native, ',', true);
    let mut settings_loaded = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        settings_loaded = window.update(cx, |c, _, _| c.settings_open && c.settings.loaded)?;
        if settings_loaded {
            break;
        }
    }
    ensure!(settings_loaded, "⌘, opened no Settings window or it never loaded: (open, actions) {:?}", window.update(cx, |c, _, _| (c.settings_open, c.actions_log.clone()))?);
    let (tab_at_open, global_file, project_at_open, rows_per_topic) = window.update(cx, |c, _, _| {
        let per_topic: Vec<(String, usize)> = Topic::ALL.iter().map(|t| (t.title().to_owned(), c.settings.settings_in(Layer::Global, *t).len())).collect();
        (c.settings.tab, c.settings.file(Layer::Global), c.settings.project.clone(), per_topic)
    })?;
    ensure!(tab_at_open == Tab::General && global_file.as_deref().is_some_and(|f| f.ends_with(".toml")), "the window opened on {tab_at_open:?} with machine file {global_file:?}");
    ensure!(rows_per_topic.iter().all(|(_, n)| *n > 0), "a topic has no rows: {rows_per_topic:?}");
    ensure!(project_at_open.as_deref() == Some(std::env::var("WHEEL_CLIENT_SPACE")?.as_str()), "the Project tab starts on {project_at_open:?}, not the shown space");
    // The status reply filled the General page's boxes.
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.host.is_some() && c.settings.log_shipping.is_some())? {
            break;
        }
    }
    let (folder_title, logs_title, command_line_state) = window.update(cx, |c, _, _| (c.settings.host.as_ref().map(wheel_gpui::settings::folder_access_title), wheel_gpui::settings::log_shipping_title(c.settings.log_shipping.as_ref()), c.settings.command_line.state.clone()))?;
    ensure!(folder_title.is_some(), "the General page never heard wheeld's status");
    draw(window, Some(output.join("client-settings-general.png")), cx)?;
    // A text field: typed, typed_saved with Return, read back by `wheel config get`.
    let homepage = window.update(cx, |c, _, _| c.settings.field(Layer::Global, "browser.homepage"))?;
    let homepage_before = config_get("browser.homepage", None)?;
    window.update(cx, |c, _, _| c.edit_setting(homepage.clone()))?;
    type_text(&native, "https://typed.test");
    draw(window, None, cx)?;
    let (typed_text, typed_dirty) = window.update(cx, |c, _, _| (c.settings.text(&homepage), c.settings.dirty(&homepage)))?;
    ensure!(typed_text == "https://typed.test" && typed_dirty, "typing into the field left {typed_text:?} (dirty {typed_dirty})");
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut typed_saved = String::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&homepage).as_deref() == Some("https://typed.test") && c.settings.drafts.is_empty() && c.settings.saving.is_empty())? {
            typed_saved = config_get("browser.homepage", None)?;
            break;
        }
    }
    ensure!(typed_saved == "https://typed.test", "Return did not save the field: wheel config get reads {typed_saved:?}, model {:?}", window.update(cx, |c, _, _| (c.settings.stored(&homepage), c.settings.refusals.clone(), c.settings.drafts.clone()))?);
    // Another client writes: the window shows the new value without being asked.
    let other = config(&["set", "browser.homepage", "https://other.test", "--machine"])?;
    ensure!(other.status.success(), "config set from the CLI failed: {}", String::from_utf8_lossy(&other.stderr));
    let mut seen_other = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        seen_other = window.update(cx, |c, _, _| c.settings.stored(&homepage))?;
        if seen_other.as_deref() == Some("https://other.test") {
            break;
        }
    }
    ensure!(seen_other.as_deref() == Some("https://other.test"), "the CLI's write never reached the window: {seen_other:?}");
    // A refused value shows under the field and stays in it.
    let int_field = window.update(cx, |c, _, _| c.settings.settings.iter().find(|s| matches!(s.kind, wheel_proto::SettingKind::Int | wheel_proto::SettingKind::Float)).map(|s| c.settings.field(Layer::Global, &s.key)))?
        .ok_or_else(|| anyhow::anyhow!("the daemon lists no number key to refuse a word for"))?;
    window.update(cx, |c, _, _| c.edit_setting(int_field.clone()))?;
    type_text(&native, "big");
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut refusal = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        refusal = window.update(cx, |c, _, _| c.settings.refusals.get(&int_field).cloned())?;
        if refusal.is_some() {
            break;
        }
    }
    ensure!(refusal.is_some(), "wheeld accepted `big` for {}, or the refusal never showed", int_field.key);
    window.update(cx, |c, _, _| c.settings.revert(&int_field))?;
    // The clear button: the key leaves the file and the default is back.
    window.update(cx, |c, _, _| c.clear_setting(&homepage))?;
    let mut cleared = String::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&homepage).is_none() && c.settings.saving.is_empty())? {
            cleared = config_get("browser.homepage", None)?;
            break;
        }
    }
    ensure!(cleared == homepage_before, "clearing left wheel config get at {cleared:?}, was {homepage_before:?}");
    // The Chat page: a switch written by its control, the search, the count and the toml.
    window.update(cx, |c, _, _| c.settings_tab(Tab::Chat))?;
    let chat_bool = window.update(cx, |c, _, _| c.settings.settings_in(Layer::Global, Topic::Chat).into_iter().find(|s| s.kind == wheel_proto::SettingKind::Bool).map(|s| (s.key.clone(), s.value.clone())))?;
    let (bool_key, bool_before) = chat_bool.ok_or_else(|| anyhow::anyhow!("the Chat topic lists no switch"))?;
    let flipped = if bool_before == "true" { "false" } else { "true" };
    let bool_field = window.update(cx, |c, _, _| c.settings.field(Layer::Global, &bool_key))?;
    window.update(cx, |c, _, _| c.write_control(&bool_field, flipped.to_owned()))?;
    let mut bool_after = String::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&bool_field).as_deref() == Some(flipped))? {
            bool_after = config_get(&bool_key, None)?;
            break;
        }
    }
    ensure!(bool_after == flipped, "the switch for {bool_key} did not write {flipped}: config get {bool_after:?}");
    let (chat_changed, chat_toml) = window.update(cx, |c, _, _| {
        c.settings.page_mut(Topic::Chat).show_toml = true;
        (c.settings.topic_changed(Topic::Chat).len(), c.settings.topic_toml(Topic::Chat))
    })?;
    ensure!(chat_changed >= 1 && chat_toml.contains(&format!("= {flipped}")), "the Chat footer counts {chat_changed} changed and the toml reads {chat_toml:?}");
    window.update(cx, |c, _, _| c.settings_edit = Some(wheel_gpui::view::SettingsEdit::Search(Topic::Chat)))?;
    for ch in "font".chars() {
        post(&native, ch, false);
        tick(cx, 20).await;
    }
    draw(window, Some(output.join("client-settings-chat.png")), cx)?;
    let (chat_query, chat_rows, chat_all) = window.update(cx, |c, _, _| (c.settings.page(Topic::Chat).query.clone(), c.settings.topic_rows(Topic::Chat).iter().map(|s| s.key.clone()).collect::<Vec<_>>(), c.settings.settings_in(Layer::Global, Topic::Chat).len()))?;
    ensure!(chat_query == "font" && !chat_rows.is_empty() && chat_rows.len() < chat_all, "searching `font` on Chat left query {chat_query:?} and rows {chat_rows:?} of {chat_all}");
    // Reset all over two set keys: two unsets in flight, both answered, both gone from the file.
    let (second_field, second_value) = window.update(cx, |c, _, _| c.settings.settings_in(Layer::Global, Topic::Chat).into_iter().filter(|s| s.key != bool_key).find_map(|s| match s.kind {
        wheel_proto::SettingKind::Bool => Some((c.settings.field(Layer::Global, &s.key), if s.value == "true" { "false".to_owned() } else { "true".to_owned() })),
        wheel_proto::SettingKind::Choice => s.choices.iter().find(|ch| **ch != s.value).map(|ch| (c.settings.field(Layer::Global, &s.key), ch.clone())),
        _ => None,
    }))?.ok_or_else(|| anyhow::anyhow!("the Chat topic lists no second switch or choice"))?;
    window.update(cx, |c, _, _| {
        c.settings_edit = None;
        c.settings.page_mut(Topic::Chat).query.clear();
        c.write_control(&second_field, second_value.clone());
    })?;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&second_field).is_some() && c.settings.saving.is_empty())? {
            break;
        }
    }
    ensure!(window.update(cx, |c, _, _| c.settings.stored(&second_field).as_deref() == Some(second_value.as_str()))?, "writing {second_value} to {} did not land: refusals {:?}", second_field.key, window.update(cx, |c, _, _| c.settings.refusals.clone())?);
    let reset_keys: Vec<String> = window.update(cx, |c, _, _| c.settings.topic_changed(Topic::Chat).iter().map(|s| s.key.clone()).collect())?;
    ensure!(reset_keys.len() >= 2, "Reset all needs two set keys, has {reset_keys:?}");
    window.update(cx, |c, _, _| {
        for key in &reset_keys {
            let field = c.settings.field(Layer::Global, key);
            c.clear_setting(&field);
        }
    })?;
    let mut reset_left = reset_keys.len();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        reset_left = window.update(cx, |c, _, _| c.settings.topic_changed(Topic::Chat).len())?;
        if reset_left == 0 && window.update(cx, |c, _, _| c.settings.saving.is_empty())? {
            break;
        }
    }
    ensure!(reset_left == 0 && config_get(&bool_key, None)? == bool_before, "Reset all left {reset_left} keys set; {bool_key} reads {:?}", config_get(&bool_key, None)?);
    // The Project tab: the fixture's space, its own wheel.toml, a value written there.
    window.update(cx, |c, _, _| c.settings_tab(Tab::Project))?;
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    let project_space = std::env::var("WHEEL_CLIENT_SPACE")?;
    let (project_file, project_topics) = window.update(cx, |c, _, _| (c.settings.file(Layer::Project), Topic::ALL.iter().filter(|t| !c.settings.settings_in(Layer::Project, **t).is_empty()).map(|t| t.title()).collect::<Vec<_>>()))?;
    ensure!(project_file.as_deref().is_some_and(|f| f.ends_with("wheel.toml")) && project_topics.contains(&"General"), "the Project tab shows file {project_file:?} and topics {project_topics:?}");
    let project_field = window.update(cx, |c, _, _| c.settings.field(Layer::Project, "browser.homepage"))?;
    ensure!(project_field.project.as_deref() == Some(project_space.as_str()), "the project field names {:?}", project_field.project);
    window.update(cx, |c, _, _| c.edit_setting(project_field.clone()))?;
    type_text(&native, "https://project.test");
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut project_value = String::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&project_field).as_deref() == Some("https://project.test"))? {
            project_value = config_get("browser.homepage", Some(&project_space))?;
            break;
        }
    }
    ensure!(project_value == "https://project.test", "the project's value did not land: config get --space reads {project_value:?}");
    let machine_value_under_project = config_get("browser.homepage", None)?;
    ensure!(machine_value_under_project == homepage_before, "writing the project's file changed the machine's value to {machine_value_under_project:?}");
    draw(window, Some(output.join("client-settings-project.png")), cx)?;
    window.update(cx, |c, _, _| c.clear_setting(&project_field))?;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.stored(&project_field).is_none() && c.settings.saving.is_empty())? {
            break;
        }
    }
    ensure!(config_get("browser.homepage", Some(&project_space))? == homepage_before, "clearing the project's value did not bring the layer below back");
    // Artifacts: the built-ins wheeld carries.
    window.update(cx, |c, _, _| c.settings_tab(Tab::Artifacts))?;
    let mut builtins = Vec::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.builtins.loaded)? {
            builtins = window.update(cx, |c, _, _| c.settings.builtins.builtins.iter().map(|b| json!({"name": b.name, "shipped": b.shipped_version, "installed": b.installed_version, "action": format!("{:?}", wheel_gpui::settings::builtin_action(b))})).collect())?;
            break;
        }
    }
    ensure!(!builtins.is_empty(), "the Artifacts tab listed no built-ins: {:?}", window.update(cx, |c, _, _| c.settings.builtins.clone())?);
    draw(window, Some(output.join("client-settings-artifacts.png")), cx)?;
    // Guidance: the brief and the files, and Open in Editor answered with the path (not launched here).
    window.update(cx, |c, _, _| c.settings_tab(Tab::Guidance))?;
    let mut guidance_segments = Vec::new();
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.settings.guidance.loaded())? {
            guidance_segments = window.update(cx, |c, _, _| c.settings.guidance.segments().iter().map(|s| json!({"id": s.id, "bytes": s.text.len()})).collect())?;
            break;
        }
    }
    ensure!(guidance_segments.len() >= 2, "the Guidance tab never loaded: {:?}", window.update(cx, |c, _, _| c.settings.guidance.error.clone())?);
    window.update(cx, |c, _, _| c.open_guidance_file(None))?;
    let mut guidance_opened = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        guidance_opened = window.update(cx, |c, _, _| c.settings.opened_file.clone())?;
        if guidance_opened.is_some() {
            break;
        }
    }
    ensure!(guidance_opened.as_ref().is_some_and(|(_, how)| how == "not launched"), "Open in Editor on a hidden window: {guidance_opened:?}");
    draw(window, Some(output.join("client-settings-guidance.png")), cx)?;
    // Keys: the keybind field and the actions table; Devices: what wheeld says about remote access.
    window.update(cx, |c, _, _| c.settings_tab(Tab::Keys))?;
    draw(window, None, cx)?;
    let keys_rows = window.update(cx, |c, _, _| c.settings.settings_in(Layer::Global, Topic::Keys).iter().map(|s| s.key.clone()).collect::<Vec<_>>())?;
    ensure!(keys_rows == ["keybind"], "the Keys page lists {keys_rows:?}");
    window.update(cx, |c, _, _| c.settings_tab(Tab::Devices))?;
    let mut devices_known = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        devices_known = window.update(cx, |c, _, _| c.settings.devices.info.is_some())?;
        if devices_known {
            break;
        }
    }
    ensure!(devices_known, "the Devices tab never heard remote_info");
    let qr_rows = window.update(cx, |c, _, _| c.native().call(json!({"op":"qr","text":"{\"url\":\"wss://127.0.0.1:7430/ws\"}"}))["rows"].as_array().map(|r| r.len()).unwrap_or(0))?;
    ensure!(qr_rows >= 21, "the bridge's qr op drew {qr_rows} rows");
    let devices_state = window.update(cx, |c, _, _| json!({"listen": c.settings.devices.info.as_ref().and_then(|i| i.listen.clone()), "unavailable": c.settings.devices.unavailable(), "paired": c.settings.devices.paired.len(), "qr_rows": qr_rows}))?;
    draw(window, Some(output.join("client-settings-devices.png")), cx)?;
    // Esc closes the window and the keyboard goes back.
    native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        if !window.update(cx, |c, _, _| c.settings_open)? {
            break;
        }
    }
    ensure!(!window.update(cx, |c, _, _| c.settings_open)?, "Esc left the Settings window open");
    let settings_report = json!({"rows_per_topic": rows_per_topic, "machine_file": global_file, "project_file": project_file, "folder_access": folder_title, "log_shipping": logs_title, "command_line": command_line_state,
        "typed_saved": typed_saved, "cli_write_seen": seen_other, "refusal": refusal, "cleared_to": cleared, "chat": {"switch": bool_key, "written": bool_after, "changed": chat_changed, "toml": chat_toml, "query": chat_query, "rows": chat_rows, "all": chat_all, "reset_keys": reset_keys},
        "project": {"space": project_space, "value": project_value, "machine_unchanged": machine_value_under_project, "topics": project_topics}, "builtins": builtins, "guidance": {"segments": guidance_segments, "opened": guidance_opened},
        "devices": devices_state});
    // Notices (Toasts.swift), the pairing window (PairingWindow.swift) and the quarantine card
    // (QuarantineCard.swift). `wheel notify` posts a toast to every client; it names a pane, so
    // the notice gets Focus pane and Close pane after Copy; `--clear` takes it down.
    let wheel = |args: &[&str]| -> Result<std::process::Output> {
        let cli = std::env::var("WHEEL_CLIENT_CLI")?;
        Ok(std::process::Command::new(&cli).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args).output()?)
    };
    let toast_pane = native_panes[0].0.clone();
    let posted = wheel(&["notify", "The fixture says hello", "--id", "check", "--level", "warning", "--pane", &toast_pane])?;
    ensure!(posted.status.success(), "wheel notify failed: {}", String::from_utf8_lossy(&posted.stderr));
    let mut toast_ids: Vec<String> = vec![];
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        toast_ids = window.update(cx, |c, _, _| c.toasts().iter().map(|t| t.id.clone()).collect())?;
        if toast_ids.iter().any(|id| id == "notify:check") {
            break;
        }
    }
    ensure!(toast_ids.iter().any(|id| id == "notify:check"), "the notify toast never showed: {toast_ids:?}");
    let toast = window.update(cx, |c, _, _| c.toasts().into_iter().find(|t| t.id == "notify:check"))?.unwrap();
    ensure!(toast.text == "The fixture says hello" && toast.level == wheel_gpui::toasts::Level::Warning, "toast read {toast:?}");
    let toast_actions: Vec<&str> = toast.actions.iter().map(|a| a.title).collect();
    ensure!(toast_actions == ["Focus pane", "Close pane"], "a toast about pane {toast_pane} offers {toast_actions:?}");
    // A second notice with the same id replaces the first; a long one draws as a card.
    let long = "A second notice with the same id replaces the first one, and one this long is a card rather than a pill.";
    wheel(&["notify", long, "--id", "check"])?;
    let mut replaced = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        replaced = window.update(cx, |c, _, _| c.toasts().iter().filter(|t| t.id == "notify:check").count() == 1 && c.toasts().iter().any(|t| t.text == long && !t.compact()))?;
        if replaced {
            break;
        }
    }
    ensure!(replaced, "the second notify did not replace the first");
    // A refused layout op is the layout-error notice, naming the pane the message names.
    window.update(cx, |c, _, _| c.send_layout(|req| ClientMsg::PaneClose { req: Some(req), pane: "p999999".into() }))?;
    let mut layout_error = None;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        layout_error = window.update(cx, |c, _, _| c.toast_sources.layout_error.clone())?;
        if layout_error.is_some() {
            break;
        }
    }
    ensure!(layout_error.is_some(), "closing a pane that does not exist raised no layout error");
    draw(window, Some(output.join("client-toasts.png")), cx)?;
    let stacked: Vec<String> = window.update(cx, |c, _, _| c.toasts().iter().map(|t| t.id.clone()).collect())?;
    ensure!(stacked == ["layout-error", "notify:check"], "the stack reads {stacked:?}");
    // Dismissing: the layout error's close button forgets it; `--clear` takes the posted one down.
    window.update(cx, |c, _, _| {
        let t = c.toasts().into_iter().find(|t| t.id == "layout-error").unwrap();
        c.dismiss_toast(&t);
    })?;
    wheel(&["notify", "--clear", "check"])?;
    let mut cleared = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        cleared = window.update(cx, |c, _, _| c.toasts().is_empty())?;
        if cleared {
            break;
        }
    }
    ensure!(cleared, "the toasts did not clear");
    // The pairing window: the fixture's wheeld listens for remote clients on an ephemeral loopback
    // port, so a code is issued and the window draws it. A frame saying a paired device connected
    // names it and, with the switch on, closes the window after 1.5 s, cancelling its codes.
    let pairing_reason = window.update(cx, |c, _, _| c.pairing_disabled_reason())?;
    ensure!(pairing_reason.is_none(), "pairing is off: {pairing_reason:?}");
    window.update(cx, |c, _, _| c.show_pairing_window())?;
    let mut pairing_rows = 0;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        pairing_rows = window.update(cx, |c, _, _| c.settings.devices.pairing_json(now_ms()).map(|j| c.qr_rows(&j).len()).unwrap_or(0))?;
        if pairing_rows > 0 {
            break;
        }
    }
    ensure!(pairing_rows >= 21, "the pairing window drew {pairing_rows} QR rows");
    let window_tokens = window.update(cx, |c, _, _| c.pairing.window_tokens.len())?;
    ensure!(window_tokens == 1, "the window holds {window_tokens} codes");
    window.update(cx, |c, _, _| c.pairing.details_open = true)?;
    draw(window, Some(output.join("client-pairing.png")), cx)?;
    // Copy code: the bridge writes a concealed item that clears itself after a minute.
    window.update(cx, |c, _, _| c.copy_pairing_code())?;
    let mut copied = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        copied = window.update(cx, |c, _, _| c.pairing.copied.is_some())?;
        if copied {
            break;
        }
    }
    ensure!(copied, "Copy code left nothing on the clipboard");
    let copy_answer = window.update(cx, |c, _, _| c.toast_log.iter().rev().find(|l| l.starts_with("pairing code copied")).cloned())?;
    ensure!(copy_answer.as_deref().is_some_and(|l| l.contains("\"copied\":true")), "the bridge did not write the concealed item: {copy_answer:?}");
    let device = ServerMsg::DeviceConnected { device: "d1".into(), name: "Check phone".into(), address: "127.0.0.1:5".into(), paired: true };
    window.update(cx, |c, _, _| c.on_event(Event::Message(Box::new(device))))?;
    let (connected, close_at) = window.update(cx, |c, _, _| (c.pairing.connected.clone(), c.pairing.close_at))?;
    ensure!(connected.as_deref() == Some("Check phone") && close_at.is_some(), "the window did not take the connection: {connected:?} {close_at:?}");
    let mut pairing_closed = false;
    for _ in 0..60 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        pairing_closed = window.update(cx, |c, _, _| !c.pairing.open)?;
        if pairing_closed {
            break;
        }
    }
    ensure!(pairing_closed, "the pairing window stayed open after a device connected");
    let pairing_events = window.update(cx, |c, _, _| c.pairing.events.clone())?;
    ensure!(pairing_events.len() == 2 && pairing_events[1].contains("“Check phone” connected from 127.0.0.1:5 and paired"), "the events read {pairing_events:?}");
    let cancelled = window.update(cx, |c, _, _| c.toast_log.iter().filter(|l| l.starts_with("pairing window closed: cancelled code")).count())?;
    // The window showed two codes: the one it asked for and the one Copy code made while it was open.
    ensure!(cancelled == 2, "closing cancelled {cancelled} codes, not the window's two");
    // The quarantine card: a pane the last run died drawing is held out of the grid; Open Anyway draws it.
    let held = native_panes[0].0.clone();
    window.update(cx, |c, _, _| c.quarantine.hold(wheel_gpui::crash_guard::Crash::parse(&format!("space:x\npane:{held}\n"), None).unwrap()))?;
    draw(window, Some(output.join("client-quarantine.png")), cx)?;
    // The held pane's native view is gone from the window; Open Anyway brings it back.
    let held_hidden = window.update(cx, |c, _, _| !c.native_panes.contains(&held))?;
    ensure!(held_hidden, "the quarantined pane {held} still has its native view");
    window.update(cx, |c, _, _| c.quarantine.release_pane(&held))?;
    draw(window, None, cx)?;
    let released = window.update(cx, |c, _, _| c.native_panes.contains(&held))?;
    ensure!(released, "Open Anyway did not bring pane {held} back");
    let toast_log = window.update(cx, |c, _, _| c.toast_log.clone())?;
    let notices_report = json!({"toast": {"text": toast.text, "actions": toast_actions, "stack": stacked, "layout_error": layout_error},
        "pairing": {"qr_rows": pairing_rows, "connected": connected, "events": pairing_events, "closed": pairing_closed},
        "quarantine": {"pane": held, "held": held_hidden}, "log": toast_log});
    // Closing the second window above left `native_panes` as the first window's views; the
    // pane opened from the palette is one more of them.
    // wheeld stops, stays away for 1.5 s and starts again on the same port; tools/check.py does
    // the restart when it sees the request file.
    // The debug server (wheel-ynjo.18): the same loopback endpoint the Swift app has, guarded the
    // same way, reading the client and driving its process card.
    let mut debug = wheel_gpui::debug::start(0, &default_token_file())?;
    let token = std::fs::read_to_string(default_token_file())?.trim().to_owned();
    let bearer = format!("Bearer {token}");
    let auth: [(&str, &str); 1] = [("Authorization", &bearer)];
    macro_rules! call {
        ($method:literal, $target:expr) => {
            http(&mut debug, $method, &$target, &auth, &mut connection, &mut layouts, window, cx).await?
        };
    }
    macro_rules! state {
        () => { call!("GET", "/state").1 };
    }
    let (unauthorized, _) = http(&mut debug, "GET", "/state", &[], &mut connection, &mut layouts, window, cx).await?;
    ensure!(unauthorized == 401, "a request without the token got {unauthorized}");
    let (origin_refused, _) = http(&mut debug, "GET", "/state", &[auth[0], ("Origin", "http://example.test")], &mut connection, &mut layouts, window, cx).await?;
    ensure!(origin_refused == 403, "a browser origin got {origin_refused}");
    let (missing, _) = call!("GET", "/nothing");
    ensure!(missing == 404, "an unknown route got {missing}");
    let (code, state) = call!("GET", "/state");
    ensure!(code == 200, "GET /state got {code}: {state}");
    let fixture_space = std::env::var("WHEEL_CLIENT_SPACE")?;
    ensure!(state["active"] == fixture_space, "state shows {} active, not the fixture space {fixture_space}", state["active"]);
    ensure!(state["connection"] == "connected" && state["hidden"] == true, "state: {}", state);
    let state_keys: Vec<String> = state.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    ensure!(state["panes"][terms[1].as_str()]["kind"] == "terminal", "state lacks the second terminal pane: {}", state["panes"]);
    // The fixture's terminals sit in a `read` loop that echoes lines: Ctrl-D ends it, and the
    // shell under it runs the sleep, which the list then shows under that pane.
    let sleep_term = state["panes"][terms[1].as_str()]["term"].as_str().unwrap_or_default().to_owned();
    let (sent, _, err) = run_pumped(&["term", "send", &sleep_term, "\u{4}"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(sent, "wheel term send failed: {err}");
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    let (sent, _, err) = run_pumped(&["term", "send", &sleep_term, "sleep 4243", "--enter"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(sent, "wheel term send failed: {err}");
    // The Processes chip opens the card (a real click), and the card lists on its own: nothing
    // below asks `/procs?action=list` until the timer is proven.
    click_chip(window, Hit::Processes, cx).await?;
    let opened = state!();
    ensure!(opened["processes_card"]["space"] == fixture_space, "the card did not open for the space: {}", opened["processes_card"]);
    let mut sleep_row = Value::Null;
    let mut lists_seen = 0usize;
    for _ in 0..30 {
        pump(&mut connection, 250, &mut layouts, window, cx).await?;
        // The timer fires a redraw; the hidden window still needs a draw to render, so each pass draws.
        draw(window, None, cx)?;
        let s = state!();
        lists_seen = s["log"].as_array().map(|l| l.iter().filter(|e| e.as_str().is_some_and(|e| e.contains(" processes: "))).count()).unwrap_or(0);
        if let Some(row) = s["procs"][fixture_space.as_str()].as_array().and_then(|a| a.iter().find(|p| p["command"] == "sleep 4243" && p["pane"] == terms[1].as_str())) {
            sleep_row = row.clone();
            break;
        }
    }
    ensure!(!sleep_row.is_null(), "the sleep never listed under pane {} ({lists_seen} lists seen): {}", terms[1], state!()["procs"]);
    // The card asks again every 2 s while open: a second list line within 5 s.
    for _ in 0..20 {
        if lists_seen >= 2 {
            break;
        }
        pump(&mut connection, 250, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        lists_seen = state!()["log"].as_array().map(|l| l.iter().filter(|e| e.as_str().is_some_and(|e| e.contains(" processes: "))).count()).unwrap_or(0);
    }
    ensure!(lists_seen >= 2, "the open card listed only {lists_seen} time(s) in 5 s: its 2 s refresh never ran");
    let sleep_pid = sleep_row["pid"].as_u64().unwrap_or_default();
    // Typing filters the rows; a click on the row (through its route) opens its details.
    call!("POST", "/type?text=sleep%204");
    let (code, _) = call!("POST", format!("/procs?action=choose&pid={sleep_pid}"));
    ensure!(code == 200, "choose got {code}");
    let card = state!();
    ensure!(card["processes_card"]["filter"] == "sleep 4", "typing did not reach the filter: {}", card["processes_card"]);
    let card_rows = card["processes_card"]["rows"].clone();
    ensure!(card_rows.as_array().is_some_and(|r| r.iter().all(|p| p["name"] == "sleep 4243") && !r.is_empty()), "the filtered card rows are not the sleep: {card_rows}");
    ensure!(card["processes_card"]["chosen"] == sleep_pid, "the chosen row is not the sleep: {}", card["processes_card"]);
    let shot_path = output.join("client-processes.png");
    let (code, snap) = call!("POST", format!("/window/snapshot?path={}", shot_path.display()));
    ensure!(code == 200, "snapshot got {code}: {snap}");
    let snapshot_bytes = std::fs::metadata(&shot_path).map(|m| m.len()).unwrap_or(0);
    ensure!(snapshot_bytes > 5000, "the window snapshot is missing or blank: {snapshot_bytes} bytes");
    // Stop takes the row out of the list the card asks for again on its own.
    let (code, _) = call!("POST", format!("/procs?space={fixture_space}&action=stop&pid={sleep_pid}"));
    ensure!(code == 200, "stop got {code}");
    let mut stopped = false;
    for _ in 0..30 {
        pump(&mut connection, 250, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        let s = state!();
        if !s["procs"][fixture_space.as_str()].as_array().is_some_and(|a| a.iter().any(|p| p["pid"] == sleep_pid)) {
            stopped = true;
            break;
        }
    }
    ensure!(stopped, "pid {sleep_pid} never left the list after Stop");
    let (code, escaped) = call!("POST", "/key?keys=escape");
    ensure!(code == 200 && escaped["taken"] == true, "escape got {code}: {escaped}");
    let closed = state!();
    ensure!(closed["processes_card"].is_null(), "the card stayed open after Esc: {}", closed["processes_card"]);
    // The same frame without the card differs from the one with it: the card was drawn.
    let closed_shot = output.join("client-processes-closed.png");
    let (code, _) = call!("POST", format!("/window/snapshot?path={}", closed_shot.display()));
    ensure!(code == 200, "second snapshot got {code}");
    let closed_bytes = std::fs::read(&closed_shot)?;
    ensure!(closed_bytes != std::fs::read(&shot_path)?, "the snapshot with the card open is identical to the one without it");
    std::fs::remove_file(&closed_shot)?;
    // A hidden window refuses the routes that would show it or feed it input.
    let (front_refused, _) = call!("POST", "/window/front");
    ensure!(front_refused == 403, "a hidden window answered the front route with {front_refused}");
    let debug_report = json!({"port": debug.port, "unauthorized": unauthorized, "origin_refused": origin_refused, "unknown_route": missing, "state_keys": state_keys,
        "sleep": sleep_row, "lists_in_5s": lists_seen, "card_rows": card_rows, "snapshot_bytes": snapshot_bytes, "stopped": stopped, "front_refused_hidden": front_refused});
    drop(debug);
    // An older wheeld (wheel-ynjo.2): the fixture daemon behind a proxy that lowers the protocol
    // number in its `status` by one. The window over it never comes up: it shows the notice with
    // Restart Server, whose click runs `WHEEL_SERVER_STOP` then `WHEEL_SERVER_START`.
    let proxy = OlderProxy::start(&format!("{}?token={}", default_url(), std::fs::read_to_string(default_token_file())?.trim()), PROTOCOL - 1)?;
    let older = Endpoint::new(&proxy.url, &default_token_file())?;
    let mut older_connection = Connection::open(older.clone());
    let older_window = cx.update(|app| wheel_gpui::create_window(app, &bridge, older_connection.send.clone(), older, None, None, true))?;
    let mut older_layouts = 0usize;
    let mut older_link = Link::Connecting;
    for _ in 0..100 {
        pump(&mut older_connection, 100, &mut older_layouts, older_window, cx).await?;
        older_link = older_window.update(cx, |c, _, _| c.link.clone())?;
        if matches!(older_link, Link::Incompatible { .. }) {
            break;
        }
    }
    ensure!(older_link == (Link::Incompatible { server: PROTOCOL - 1, client: PROTOCOL }), "the window over the older wheeld read {older_link:?}");
    ensure!(older_layouts == 0, "a layout came through from the older wheeld");
    draw(older_window, None, cx)?;
    let (older_banner, older_toasts, older_state) = older_window.update(cx, |c, _, _| (c.link.banner(), c.toasts(), c.state_json()))?;
    ensure!(older_banner == "The server was updated.", "the banner over the older wheeld reads {older_banner:?}");
    let server_toast = older_toasts.iter().find(|t| t.id == "server-update").cloned().ok_or_else(|| anyhow::anyhow!("no server-update notice over the older wheeld: {older_toasts:?}"))?;
    let server_toast_actions: Vec<&str> = server_toast.actions.iter().map(|a| a.title).collect();
    ensure!(server_toast.text == "The server was updated." && server_toast_actions == ["Restart Server"], "the server notice reads {server_toast:?}");
    let older_view = json!({"server_protocol": older_state["server_protocol"], "connection": older_state["connection"], "spaces": older_state["spaces"]});
    ensure!(older_view == json!({"server_protocol": {"version": PROTOCOL - 1, "client": PROTOCOL}, "connection": "incompatible", "spaces": []}), "the older window's state reads {older_view}");
    draw(older_window, Some(output.join("client-server-updated.png")), cx)?;
    let (stop_file, start_file) = (output.join("server.stop"), output.join("server.start"));
    older_window.update(cx, |c, _, _| c.run_toast_action(&server_toast.actions[0].act))?;
    let mut restart_log = None;
    for _ in 0..50 {
        pump(&mut older_connection, 100, &mut older_layouts, older_window, cx).await?;
        draw(older_window, None, cx)?;
        restart_log = older_window.update(cx, |c, _, _| c.toast_log.iter().rev().find(|l| l.starts_with("server: restart")).cloned())?;
        if restart_log.as_deref() == Some("server: restarted from the app") {
            break;
        }
    }
    ensure!(restart_log.as_deref() == Some("server: restarted from the app"), "Restart Server ended with {restart_log:?}");
    ensure!(stop_file.exists() && start_file.exists(), "Restart Server did not run both commands: stop {} start {}", stop_file.exists(), start_file.exists());
    older_window.update(cx, |c, _, _| c.dismiss_toast(&server_toast))?;
    let (after_dismiss, older_log): (Vec<String>, Vec<String>) = older_window.update(cx, |c, _, _| (c.toasts().iter().map(|t| t.id.clone()).collect(), c.toast_log.iter().filter(|l| l.starts_with("server")).cloned().collect()))?;
    ensure!(after_dismiss.is_empty(), "the server notice stayed after its close: {after_dismiss:?}");
    older_window.update(cx, |_, w, _| w.remove_window())?;
    drop(older_connection);
    let proxy_port = proxy.port;
    let proxy_frames = proxy.stop();
    ensure!(proxy_frames >= 1, "the proxy rewrote no status reply: the older window never met the lowered protocol");
    // The first window, on the fixture wheeld itself, has nothing to offer.
    let (main_state, main_toasts) = window.update(cx, |c, _, _| (c.state_json(), c.toasts()))?;
    ensure!(main_state["server_protocol"] == json!({"version": PROTOCOL, "client": PROTOCOL}), "the fixture wheeld's protocol reads {}", main_state["server_protocol"]);
    ensure!(!main_toasts.iter().any(|t| t.id == "server-update"), "a server notice over the fixture wheeld: {main_toasts:?}");
    let server_updated_report = json!({"proxy_port": proxy_port, "proxy_frames_rewritten": proxy_frames, "server_protocol": older_state["server_protocol"], "connection": older_state["connection"],
        "banner": older_banner, "toast": {"text": server_toast.text, "actions": server_toast_actions}, "restart_log": restart_log, "stop_ran": stop_file.exists(), "start_ran": start_file.exists(),
        "after_dismiss": after_dismiss, "log": older_log, "main_server_protocol": main_state["server_protocol"]});
    let (request, done) = (output.join("restart.request"), output.join("restart.done"));
    let _ = std::fs::remove_file(&done);
    let status_before_restart = native.status();
    let identities_before: Vec<(String, String)> = native_panes.iter().map(|(id, _)| (id.clone(), identity(&status_before_restart, id))).collect();
    let layouts_before_restart = layouts;
    // The server lifecycle (wheel-ynjo.3): the fixture wheeld is one this app did not install, so
    // the control is the commands kind, the word over it reads Running, and no source line is
    // logged (there is no bundled source to compare). A terminated wheeld sends `stopping` first:
    // the client expects it back for 10 s, with no "unreachable" notice meanwhile, and says so when
    // it answers again.
    let log_lines = |c: &Client, prefix: &str| -> Vec<String> { c.toast_log.iter().filter(|l| l.starts_with(prefix)).cloned().collect() };
    let (control_kind, word_before, control_log): (String, String, Vec<String>) = window.update(cx, |c, _, _| (c.server_control.kind().into(), c.server_status_word().word, log_lines(c, "daemon ")))?;
    ensure!(control_kind == "commands" && word_before == "Running", "over the fixture wheeld the control is {control_kind} and the word {word_before}");
    ensure!(control_log.is_empty(), "a bundled-server line over the fixture wheeld: {control_log:?}");
    std::fs::write(&request, "1.5")?;
    let (mut saw_down, mut down_reason, mut up_again) = (false, String::new(), false);
    let mut banner_while_down = String::new();
    let (mut expected_while_down, mut unreachable_while_expected, mut word_while_down) = (false, false, String::new());
    for _ in 0..400 {
        pump(&mut connection, 50, &mut layouts, window, cx).await?;
        let (link, banner, expected, unreachable, word) = window.update(cx, |c, _, cx| {
            c.tick_server(cx);
            (c.link.clone(), c.link.banner(), c.server_watch.restart_expected, c.toasts().iter().any(|t| t.id == "unreachable"), c.server_status_word().word)
        })?;
        if let Link::Down(reason) = &link {
            saw_down = true;
            down_reason = reason.clone();
            banner_while_down = banner;
            expected_while_down |= expected;
            unreachable_while_expected |= expected && unreachable;
            word_while_down = word;
        }
        if saw_down && link == Link::Up && done.exists() {
            up_again = true;
            break;
        }
    }
    ensure!(saw_down, "the client never reported wheeld down");
    ensure!(up_again, "the client did not come back up after the restart (layouts {layouts})");
    let (expected_after, server_log): (bool, Vec<String>) = window.update(cx, |c, _, _| (c.server_watch.restart_expected, log_lines(c, "server: wheeld")))?;
    ensure!(expected_while_down, "wheeld's `stopping` did not put the client in its restart window: {server_log:?}");
    ensure!(!unreachable_while_expected, "an unreachable notice showed while a restart was expected");
    ensure!(word_while_down == "Stopped", "the word while wheeld was down reads {word_while_down}");
    ensure!(server_log.iter().any(|l| l == "server: wheeld is stopping; waiting 10 s for it to answer again") && server_log.iter().any(|l| l == "server: wheeld answers again"), "the restart window's log reads {server_log:?}");
    ensure!(!expected_after, "a restart is still expected after wheeld answered again");
    let server_lifecycle_report = json!({"control": control_kind, "word_before": word_before, "word_while_down": word_while_down, "expected_while_down": expected_while_down,
        "unreachable_while_expected": unreachable_while_expected, "expected_after": expected_after, "log": server_log});
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
        "settings": settings_report,
        "notices": notices_report,
        "debug": debug_report,
        "sidebar": sidebar_report,
        "title_row": title_report,
        "windows": windows_report,
        "server_updated": server_updated_report,
        "server_lifecycle": server_lifecycle_report,
        "restart": {"saw_down": saw_down, "down_reason": down_reason, "banner_while_down": banner_while_down, "layouts_before": layouts_before_restart,
            "layouts_after": layouts, "views_before": identities_before, "views_after": identities_after,
            "terminal_viewport_after": viewport(&typed_after, terms[0]), "banner_after": banner_after},
        "final": {"visible": final_status["visible"], "key_window": final_status["key_window"], "app_active": final_status["app_active"], "errors": final_status["errors"]},
    }))
}

/// The fixture wheeld as an older one: a loopback WebSocket in front of it that passes every frame
/// through and writes `protocol` in each `status` reply down to `protocol`. The rest of the reply
/// is the real wheeld's, so the client meets exactly what an older daemon's number looks like.
struct OlderProxy {
    url: String,
    port: u16,
    rewritten: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl OlderProxy {
    fn start(upstream: &str, protocol: u32) -> Result<Self> {
        use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (rewritten, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (count, stopping, upstream) = (rewritten.clone(), stop.clone(), upstream.to_owned());
        listener.set_nonblocking(true)?;
        std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let Ok(mut client) = tungstenite::accept(stream) else { continue };
                let Ok((mut server, _)) = tungstenite::connect(&upstream) else { continue };
                client.get_mut().set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                if let tungstenite::stream::MaybeTlsStream::Plain(s) = server.get_mut() {
                    s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                }
                let timed_out = |e: &tungstenite::Error| matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
                'session: while !stopping.load(Ordering::Relaxed) {
                    match client.read() {
                        Ok(m @ tungstenite::Message::Text(_)) => { if server.send(m).is_err() { break 'session; } }
                        Ok(tungstenite::Message::Close(_)) => break 'session,
                        Ok(_) => {}
                        Err(e) if timed_out(&e) => {}
                        Err(_) => break 'session,
                    }
                    match server.read() {
                        Ok(tungstenite::Message::Text(t)) => {
                            let mut frame: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                            if frame["ev"] == "status" {
                                frame["protocol"] = json!(protocol);
                                count.fetch_add(1, Ordering::Relaxed);
                            }
                            if client.send(tungstenite::Message::text(frame.to_string())).is_err() { break 'session; }
                        }
                        Ok(tungstenite::Message::Close(_)) => break 'session,
                        Ok(_) => {}
                        Err(e) if timed_out(&e) => {}
                        Err(_) => break 'session,
                    }
                }
                let _ = client.close(None);
                let _ = server.close(None);
            }
        });
        Ok(Self { url: format!("ws://127.0.0.1:{port}/ws"), port, rewritten, stop })
    }
    /// End the proxy; how many `status` replies it rewrote.
    fn stop(self) -> usize {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.rewritten.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Unix milliseconds now.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
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
