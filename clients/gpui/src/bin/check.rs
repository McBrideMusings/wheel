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

/// The CPU time this thread has used, in milliseconds.
fn thread_cpu_ms() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes only the timespec it is handed.
    let failed = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    assert_eq!(failed, 0, "clock_gettime(CLOCK_THREAD_CPUTIME_ID) failed");
    t.tv_sec as f64 * 1000. + t.tv_nsec as f64 / 1e6
}
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
    for _ in 0..200 {
        if native.status()["snapshots_pending"] == 0 {
            return Ok(());
        }
        tick(cx, 100).await;
    }
    let s = native.status();
    let waiting: Vec<Value> = s["snapshots_waiting"].as_array().cloned().unwrap_or_default().iter()
        .map(|id| { let p = &s["panes"][id.as_str().unwrap_or("")]; json!({"pane": id, "kind": p["kind"], "url": p["url"], "loads": p["loads"], "hidden": p["hidden"]}) }).collect();
    anyhow::bail!("WK snapshots unanswered after 20 s: {waiting:?}; bridge errors {}", s["errors"])
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

/// What every step of the check shares: the bridge, the first window's connection and views, the
/// count of layouts it received, the fixture's terminal markers and the output folder.
struct Run {
    bridge: std::rc::Rc<Bridge>,
    connection: Connection,
    layouts: usize,
    window: gpui::WindowHandle<Client>,
    native: std::rc::Rc<Native>,
    markers: Value,
    output: PathBuf,
}

async fn settled_once(native: &Native, terms: &[&String], before: &[Vec<Value>], before_status: &Value, what: &str) -> Result<Value> {
    let after = native.status();
    let mut resized = 0;
    let mut per = serde_json::Map::new();
    for (i, t) in terms.iter().enumerate() {
        let sent = after["panes"][t.as_str()]["sent_grids"].as_array().cloned().unwrap_or_default();
        let new = &sent[before[i].len().min(sent.len())..];
        let now = last_grid(&after, t);
        if now != last_grid(before_status, t) {
            ensure!(new.len() == 1 && new[0] == now, "{what}: terminal {t} sent {new:?} after the move, wanted only its last grid {now}");
            resized += 1;
        } else {
            ensure!(new.is_empty(), "{what}: terminal {t} kept its grid {now} and still sent {new:?}");
        }
        per.insert(t.to_string(), json!({"sent_new": new, "last_grid": now}));
    }
    ensure!(resized > 0, "{what}: no terminal changed size: {per:?}");
    Ok(Value::Object(per))
}
async fn type_line(native: &Native, text: &str, cx: &mut AsyncApp) {
    for ch in text.chars() {
        native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch)}));
        tick(cx, 15).await;
    }
    native.call(json!({"op":"post_key","text":"\r","code":36}));
}
async fn erase(native: &Native, count: usize, cx: &mut AsyncApp) {
    for _ in 0..count {
        native.call(json!({"op":"post_key","text":"\u{7f}","code":51}));
        tick(cx, 10).await;
    }
    tick(cx, 100).await;
}
async fn type_text(native: &Native, text: &str, cx: &mut AsyncApp) {
    for ch in text.chars() {
        native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch)}));
        tick(cx, 15).await;
    }
    tick(cx, 150).await;
}

/// Runs each step in order. A step takes the shared `Run` and what it reads from earlier steps, and
/// hands back the `Run` and what later steps and the report read.
async fn experiment(output: PathBuf, cx: &mut AsyncApp) -> Result<Value> {
    let (run, first, native_panes, loaded, before, page_state, page_bytes, rects, aligned, own, click, focused, typed, leaked, terminal_report, saved_file) = layout_and_terminals(output, cx).await?;
    let webs: Vec<&String> = native_panes.iter().filter(|p| p.1 == "web").map(|p| &p.0).collect();
    let terms: Vec<&String> = native_panes.iter().filter(|p| p.1 == "terminal").map(|p| &p.0).collect();
    let native_pages: Vec<&String> = webs.iter().copied().filter(|w| before["panes"][w.as_str()]["page"]["native"] == true).collect();
    let target = terms[1];
    let (run, split, parent, start, moved, drag_steps, drag_settled, window_resize, after_drag, after_status, saved, status, external, external_status, cli) = focus_and_resizes(run, terms.clone(), target, cx).await?;
    let (run, agent_pane, agent_name, agent_rect, cli_path, agent_report) = agent_turns(run, rects.clone(), cx).await?;
    let (run, markdown_report) = markdown_reply(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, footer_report) = footer(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, buttons_inside_short_pane, resolved_from_cli, status_report) = statuses(run, agent_pane.clone(), agent_name.clone(), agent_rect.clone(), cli_path.clone(), cx).await?;
    let (run, splits_above, perm_card, card_shown, answered_by_key, question_card, question_options, clicked, question_answer) = perm_cards(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, commands_listed, by_arrows, co_listed, by_tab, by_click, queue_ids, took_back, cleared_queue, queued_then_sent, saw_sending, control_return_ran, esc_stopped) = composer_and_queue(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, attachments_report) = attachments(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, conversations_from, listed, old_id, new_id, new_id_differs, recent_report, resumed_rows, codex_pane_streams) = conversations(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, fork_row, fork_text, fork_rows, reply_buttons, fork_report) = fork_and_reply_buttons(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), cx).await?;
    let (run, message_menu_report, scroll_report) = message_menu_and_scroll(run, agent_pane.clone(), agent_name.clone(), cli_path.clone(), old_id.clone(), splits_above.clone(), click.clone(), fork_row.clone(), fork_text.clone(), cx).await?;
    let (run, running_report) = running_strip(run, agent_pane.clone(), agent_name.clone(), cx).await?;
    let (run, running_full_report) = running_full_view(run, agent_pane.clone(), agent_name.clone(), cx).await?;
    let conversations_report = json!({"reply_buttons": reply_buttons, "recent": recent_report, "listed": listed, "new_id_differs": new_id_differs, "resumed_rows": resumed_rows, "codex_pane_streams": codex_pane_streams, "old_id": old_id, "new_id": new_id, "opened_from": conversations_from});
    let composer_report = json!({"commands_listed": commands_listed, "picked_by_arrows": by_arrows, "co_listed": co_listed, "picked_by_tab": by_tab, "picked_by_click": by_click,
        "queued_then_sent": queued_then_sent, "saw_sending_now": saw_sending, "cleared_queue": cleared_queue, "took_back": took_back, "control_return_ran": control_return_ran, "esc_stopped_with_draft": esc_stopped, "queue_actions": queue_ids});
    let perms_report = json!({"card_shown": card_shown, "answered_by_key": answered_by_key, "resolved_from_cli": resolved_from_cli,
        "question_options": question_options, "buttons_inside_short_pane": buttons_inside_short_pane, "question_answer": question_answer, "questions_clicked": clicked.len(), "card": perm_card, "question_card": question_card});
    let (run, first_session_id, fixture_space, sidebar_report) = sidebar(run, click.clone(), cx).await?;
    let (run, title_report) = title_and_bottom_rows(run, first_session_id.clone(), fixture_space.clone(), native_panes.clone(), cx).await?;
    let (run, restore, first_session, first_space, both, distinct, hidden_native, second, second_session, native2, id1, id2, f1, f2, dragged, drag_dir, from2, over_window, window2_sees, after_window, desk, little_id, little_owner, little_frame, over_desk_floats, aim, chip_expect, chip_over_window) = second_window_and_space_drags(run, saved_file.clone(), native_panes.clone(), webs.clone(), cx).await?;
    let (run, card, card_hint, onto_tabs_after, over_card, over_tabs, window2_tabs) = space_drag_to_popover_and_tabs(run, dragged.clone(), f1.clone(), f2.clone(), first_session.clone(), from2.clone(), id2.clone(), native2.clone(), second.clone(), second_session.clone(), cx).await?;
    let (run, pane_away) = pane_drag_out_of_window(run, f1.clone(), f2.clone(), id1.clone(), id2.clone(), second.clone(), native2.clone(), second_session.clone(), first_space.clone(), dragged.clone(), drag_dir.clone(), native_panes.clone(), cx).await?;
    let space_drags = json!({
        "window_frames": [f1, f2],
        "onto_window": {"target": over_window["target"], "hint": over_window["hint"], "window2_insertion": window2_sees["insertion"], "window2_chip": window2_sees["chip"], "session_after": after_window.0},
        "onto_desktop": {"drop": [desk.0, desk.1], "little": little_id, "owner": little_owner, "frame": little_frame},
        "onto_card": {"target": over_card["target"], "hint": card_hint, "card": [card.frame.x, card.frame.y, card.frame.w, card.frame.h]},
        "chip": {"aim": [aim.0, aim.1], "frame": chip_over_window["frame"], "expected_origin": [chip_expect.0, chip_expect.1]},
        "card": {"drop": [desk.0, desk.1], "frame": over_desk_floats["card"]["frame"], "chip_shown": over_desk_floats["chip"]["shown"]},
        "onto_tabs": {"target": over_tabs["target"], "hint": over_tabs["hint"], "window2_tab_bar": window2_tabs["tab_bar"], "session_after": onto_tabs_after},
        "pane_away": pane_away,
    });
    let sessions_windows = json!({"restore": restore, "space_drags": space_drags, "first_session": first_session, "second_session": second_session, "views_with_two_windows": both.iter().map(|w| json!([w.0, w.1, w.2, w.3])).collect::<Vec<_>>(),
        "second_session_hides_first_space_views": hidden_native["panes"].as_object().map(|p| p.len()), "views_distinct_between_windows": distinct});
    let (run, windows_report) = window_commands(run, first_session.clone(), id1.clone(), cx).await?;
    let (run, little_report) = little_windows(run, id1.clone(), cx).await?;
    let (run, page_names, web_report) = web_panes(run, webs.clone(), cli_path.clone(), before.clone(), cx).await?;
    let (run, browser_report) = browser_pane(run, native_panes.clone(), cx).await?;
    let (run, pages_report) = plugin_pages(run, page_names.clone(), first_session.clone(), first_space.clone(), native_panes.clone(), cx).await?;
    let (run, commands_report) = command_palette(run, cli_path.clone(), terms.clone(), cx).await?;
    let (run, settings_report) = settings_window(run, cli_path.clone(), cx).await?;
    let (run, notices_report) = notices(run, native_panes.clone(), cx).await?;
    let (run, debug, bearer, unauthorized, origin_refused, missing, state_keys, sleep_row, lists_seen, card_rows, snapshot_bytes, stopped, front_refused, fixture_space) = debug_server_and_processes(run, terms.clone(), cx).await?;
    let (run, debug, grid, grid_a, grid_b, grid_c, a, c, menu_ids, docked, area, popped_card_report) = pane_grid(run, first_session_id.clone(), debug, bearer.clone(), cx).await?;
    let (run, debug, card_drops_report, tray_report) = card_drops_and_tray(run, debug, bearer.clone(), grid.clone(), grid_a.clone(), grid_b.clone(), grid_c.clone(), area.clone(), cx).await?;
    let (run, debug, zoom_report) = zoom(run, debug, bearer.clone(), grid.clone(), grid_a.clone(), grid_b.clone(), grid_c.clone(), cli.clone(), cx).await?;
    let (run, debug, header_report, new_space) = pane_header(run, debug, bearer.clone(), grid.clone(), grid_a.clone(), grid_b.clone(), cli.clone(), fixture_space.clone(), cx).await?;
    let frame = |s: &Value, pane: &str| -> [f64; 4] {
        let f = &s["panes"][pane]["frame"];
        [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
    };
    let mut pane_grid_report = json!({"space": grid, "panes": [grid_a, grid_b, grid_c], "menu": menu_ids, "swap": {"from": a, "to": c}, "popped_card": popped_card_report, "tray": tray_report,
        "dock": {"second": frame(&docked, &grid_b), "first": frame(&docked, &grid_a)}, "moved_to": new_space});
    pane_grid_report["zoom"] = zoom_report;
    pane_grid_report["card_drops"] = card_drops_report;
    pane_grid_report["header"] = header_report;
    let debug_report = json!({"port": debug.port, "unauthorized": unauthorized, "origin_refused": origin_refused, "unknown_route": missing, "state_keys": state_keys,
        "sleep": sleep_row, "lists_in_5s": lists_seen, "card_rows": card_rows, "snapshot_bytes": snapshot_bytes, "stopped": stopped, "front_refused_hidden": front_refused});
    drop(debug);
    let (run, server_updated_report, identities_before, layouts_before_restart, done, request) = older_server(run, native_panes.clone(), cx).await?;
    let (run, server_lifecycle_report, saw_down, down_reason, banner_while_down, typed_after, identities_after, banner_after, final_status) = server_lifecycle_and_restart(run, native_panes.clone(), terms.clone(), webs.clone(), done.clone(), request.clone(), identities_before.clone(), layouts_before_restart.clone(), cx).await?;
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let per_pane = |s: &Value| -> Value {
        native_panes.iter().map(|(id, kind)| (id.clone(), json!({"kind":kind,"frame":frame(s,id),"grid":last_grid(s,id)}))).collect::<serde_json::Map<_, _>>().into()
    };
    let mut report = json!({
        "host_pid": std::process::id(), "layouts_received": layouts, "page": {"pane": native_pages[0], "state": page_state, "png_bytes": page_bytes}, "first_layout": first,
        "native_panes": native_panes, "loaded_at_start": loaded["panes"],
        "native_aligned_with_panes": aligned, "own_session_only": own,
        "keyboard": {"clicked": target, "click": click, "focused": focused, "typed_viewport": viewport(&typed, target), "leaked_to_first": leaked},
        "before": per_pane(&before),
        "drag": {"split": split, "from": start, "to": moved, "parent": [parent.x, parent.y, parent.w, parent.h],
            "after": per_pane(&after_status), "layout_after": after_drag, "ratio_saved_by_wheeld": saved, "steps": drag_steps, "settled": drag_settled, "window_resize": window_resize},
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
        "pane_grid": pane_grid_report,
        "windows": windows_report,
        "little_windows": little_report,
        "server_updated": server_updated_report,
        "server_lifecycle": server_lifecycle_report,
        "restart": {"saw_down": saw_down, "down_reason": down_reason, "banner_while_down": banner_while_down, "layouts_before": layouts_before_restart,
            "layouts_after": layouts, "views_before": identities_before, "views_after": identities_after,
            "terminal_viewport_after": viewport(&typed_after, terms[0]), "banner_after": banner_after},
        "final": {"visible": final_status["visible"], "key_window": final_status["key_window"], "app_active": final_status["app_active"], "errors": final_status["errors"]},
    });
    // Past the one `json!` the recursion limit allows.
    report["terminal"] = terminal_report;
    report["perms"] = perms_report;
    report["composer"] = composer_report;
    report["composer"]["attachment_path_in_message"] = attachments_report["attachment_path_in_message"].clone();
    report["composer"]["attachments"] = attachments_report;
    report["conversations"] = conversations_report;
    report["conversations"]["fork_rows"] = json!(fork_rows);
    report["conversations"]["fork"] = fork_report;
    report["running"] = running_report;
    report["running"]["full_view"] = running_full_report;
    report["message_menu"] = message_menu_report;
    report["scroll"] = scroll_report;
    // Last: it closes every window.
    report["menu_bar"] = menu_bar_check(&bridge, &agent_name, &mut connection, &mut layouts, window, &output, cx).await?;
    Ok(report)
}

/// Layout from wheeld, one native view per pane on its body, separate terminal sessions,
/// keyboard focus by a click, a dropped path and the `terminal.*` settings restyling the surface.
async fn layout_and_terminals(output: PathBuf, cx: &mut AsyncApp) -> Result<(Run, Value, Vec<(String, String)>, Value, Value, Value, u64, Vec<(String, Rect)>, Vec<(String, bool)>, Vec<(String, bool, bool)>, [f64; 2], Value, Value, bool, Value, PathBuf)> {
    let bridge = Bridge::load()?;
    let endpoint = Endpoint::new(&default_url(), &default_token_file())?;
    let mut connection = Connection::open(endpoint.clone());
    // The windows are saved as the app saves them (saved.rs), to a file of the check's own.
    let saved_file = output.join("saved-windows.json");
    let _ = std::fs::remove_file(&saved_file);
    cx.update(|app| wheel_gpui::saved::start(app, Some(saved_file.clone()), true));
    let window = cx.update(|app| wheel_gpui::create_window(app, &bridge, connection.send.clone(), endpoint, None, std::env::var("WHEEL_CLIENT_SPACE").ok(), true, wheel_gpui::Opening::Main, None))?;
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
    // The terminal pane (TerminalStyle.swift, the Ghostty surface): a dropped file's path reaches
    // the PTY of the terminal that holds it, and the `terminal.*` settings restyle the surface live
    // with the shell still running. Committed IME text is Ghostty's own path, which takes text only
    // while AppKit dispatches a dequeued event; this headless run has no event loop to dequeue one.
    let dropped = native.call(json!({"op":"term_drop","pane":target,"paths":["/tmp/a b(1).txt", "/tmp/c"]}));
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let drop_line = r"PTY:/tmp/a\ b\(1\).txt /tmp/c";
    let after_drop = wait(&native, "a dropped file's path in the terminal", |s| viewport(s, target).contains(drop_line), cx).await?;
    ensure!(dropped["dropped"] == true, "the drop pasted nothing: {dropped}");
    let other_term = terms.iter().copied().find(|t| *t != target).unwrap();
    ensure!(!viewport(&after_drop, other_term).contains("/tmp/c"), "the other terminal got the typed text: {}", viewport(&after_drop, other_term));
    let term_cli = std::env::var("WHEEL_CLIENT_CLI")?;
    let term_config = |args: &[&str]| -> Result<std::process::Output> {
        Ok(std::process::Command::new(&term_cli).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "config"]).args(args).output()?)
    };
    let config_of = |s: &Value, t: &str| s["panes"][t]["config"].as_str().unwrap_or_default().to_owned();
    // A key can appear more than once in the rendered config; the last line is the one in force.
    let last = |config: &str, key: &str| config.lines().rev().find_map(|l| l.strip_prefix(&format!("{key} = ")).map(str::to_owned)).unwrap_or_default();
    let status_before_look = native.status();
    let look_before = config_of(&status_before_look, target);
    let columns_before = last_grid(&status_before_look, target)[0].as_u64().unwrap_or(0);
    ensure!(last(&look_before, "font-size") == "14" && last(&look_before, "cursor-style") == "block" && look_before.contains("copy_to_clipboard"),
        "the terminal's config is not the Swift app's: {look_before}");
    for (key, value) in [("terminal.font.size", "18"), ("terminal.theme", "gruvbox"), ("terminal.cursor.style", "bar")] {
        let set = term_config(&["set", key, value, "--machine"])?;
        ensure!(set.status.success(), "config set {key} failed: {}", String::from_utf8_lossy(&set.stderr));
    }
    let mut restyled = Value::Null;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let s = native.status();
        let c = config_of(&s, target);
        // The surface itself took the new font when its grid has fewer columns.
        let columns = last_grid(&s, target)[0].as_u64().unwrap_or(0);
        if last(&c, "font-size") == "18" && last(&c, "cursor-style") == "bar" && s["panes"][target]["theme"] == "gruvbox" && columns > 0 && columns < columns_before {
            restyled = s;
            break;
        }
    }
    ensure!(!restyled.is_null(), "the terminal.* settings never restyled the terminal (grid {} columns before): {} {}", columns_before, config_of(&native.status(), target), last_grid(&native.status(), target));
    ensure!(restyled["panes"][target.as_str()]["restyled"] == json!([true, true]), "Ghostty refused the restyle: {}", restyled["panes"][target.as_str()]["restyled"]);
    // A restyled surface draws its next frame on its own schedule: shoot until it has one.
    let look_dir = output.join("terminal-look");
    let _ = std::fs::remove_dir_all(&look_dir);
    let look_png = look_dir.join(format!("{target}-terminal.png"));
    for _ in 0..20 {
        pump(&mut connection, 200, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        shot(&native, &look_dir, cx).await?;
        if look_png.exists() {
            break;
        }
    }
    ensure!(look_png.exists(), "the restyled terminal never drew a picture");
    native.call(json!({"op":"post_key","text":"s","code":1}));
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    wait(&native, "the restyled terminal still answering", |s| viewport(s, target).contains("PTY:s"), cx).await?;
    for key in ["terminal.font.size", "terminal.theme", "terminal.cursor.style"] {
        let unset = term_config(&["unset", key, "--machine"])?;
        ensure!(unset.status.success(), "config unset {key} failed: {}", String::from_utf8_lossy(&unset.stderr));
    }
    let reverted = |s: &Value| { let c = config_of(s, target); last(&c, "font-size") == "14" && last(&c, "cursor-style") == "block" && s["panes"][target.as_str()]["theme"] == "default" };
    let mut unset_status = native.status();
    for _ in 0..100 {
        if reverted(&unset_status) {
            break;
        }
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        unset_status = native.status();
    }
    let look_after = config_of(&unset_status, target);
    ensure!(reverted(&unset_status), "unsetting the terminal.* settings left the terminal restyled: theme {} {look_after}", unset_status["panes"][target.as_str()]["theme"]);
    let terminal_report = json!({"dropped": dropped, "drop_line": drop_line, "target_viewport": viewport(&after_drop, target),
        "other_viewport": viewport(&after_drop, other_term), "config_before": look_before, "config_restyled": config_of(&restyled, target),
        "config_after": look_after, "grids": terms.iter().map(|t| (t.to_string(), last_grid(&restyled, t))).collect::<serde_json::Map<_, _>>()});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, first, native_panes, loaded, before, page_state, page_bytes, rects, aligned, own, click, focused, typed, leaked, terminal_report, saved_file))
}

/// Command-] and command-[ move the keyboard; a divider drag and a window resize hold the
/// terminals' sizes until they end; another client's ratio change is followed.
async fn focus_and_resizes(run: Run, terms: Vec<&String>, target: &String, cx: &mut AsyncApp) -> Result<(Run, String, Rect, [f64; 2], [f64; 2], Vec<Value>, Value, Vec<Value>, Value, Value, Option<f64>, std::process::ExitStatus, Value, Value, String)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let snapshot = |cx: &mut AsyncApp| window.update(cx, |c, _, _| {
        let (leaves, dividers) = c.scene();
        json!({"leaves":leaves.iter().map(|p| json!({"id":p.leaf.id,"kind":p.leaf.kind,"label":p.leaf.label,"rect":[p.rect.x,p.rect.y,p.rect.w,p.rect.h],
            "native":c.native_panes.contains(&p.leaf.id)})).collect::<Vec<_>>(),
          "dividers":dividers.iter().map(|d| json!({"split":d.split,"horizontal":d.horizontal,"rect":[d.rect.x,d.rect.y,d.rect.w,d.rect.h]})).collect::<Vec<_>>()})
    });
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
    // While the divider is under the pointer, no terminal sends a size: every step is drawn and
    // reported, and only the grid each terminal ends at goes to wheeld once the drag lets go.
    let sent_of = |s: &Value| terms.iter().map(|t| s["panes"][t.as_str()]["sent_grids"].as_array().cloned().unwrap_or_default()).collect::<Vec<_>>();
    let held_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.state_json()["grid_motion"].clone());
    let before_drag = native.status();
    let sent_before_drag = sent_of(&before_drag);
    native.call(json!({"op":"post_mouse","kind":"down","point":start}));
    let mut drag_steps = vec![];
    for step in 1..=5 {
        let at = [start[0] + 140. * f64::from(step) / 5., start[1]];
        native.call(json!({"op":"post_mouse","kind":"drag","point":at}));
        pump(&mut connection, 50, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        pump(&mut connection, 250, &mut layouts, window, cx).await?;
        let mid = native.status();
        let motion = held_now(cx)?;
        ensure!(motion["resizes_held"] == true && motion["dragging_divider"] == true && sent_of(&mid) == sent_before_drag,
            "divider drag step {step}: held {motion}, sent {:?} before {sent_before_drag:?}", sent_of(&mid));
        drag_steps.push(json!({"at": at, "grids": terms.iter().map(|t| last_grid(&mid, t)).collect::<Vec<_>>()}));
    }
    native.call(json!({"op":"post_mouse","kind":"up","point":moved}));
    pump(&mut connection, 1500, &mut layouts, window, cx).await?;
    draw(window, None, cx)?;
    pump(&mut connection, 800, &mut layouts, window, cx).await?;
    // Each terminal the drag resized sent once, the grid it reported last; the others sent nothing.
    let drag_settled = settled_once(&native, &terms, &sent_before_drag, &before_drag, "the divider drag").await?;
    ensure!(held_now(cx)?["resizes_held"] == false, "the divider drag left the resizes held");
    // A window edge dragged, smaller and back: the bridge holds from AppKit's live resize
    // notifications, posted here for the window, while the client's own hold stays off.
    let window_size = window.update(cx, |_, w, _| w.viewport_size())?;
    let mut window_resize = vec![];
    for (what, from, to) in [("the window shrinking", 0., -180.), ("the window growing back", -180., 0.)] {
        let before = native.status();
        let sent_before = sent_of(&before);
        let begun = native.call(json!({"op":"live_resize","on":true}));
        ensure!(begun["held"] == true, "{what}: the posted live resize did not hold the resizes: {begun}");
        for step in 1..=3 {
            let w: f32 = from + (to - from) * step as f32 / 3.;
            window.update(cx, |_, win, _| win.resize(gpui::size(window_size.width + gpui::px(w), window_size.height)))?;
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(window, None, cx)?;
            pump(&mut connection, 250, &mut layouts, window, cx).await?;
            let mid = native.status();
            ensure!(mid["live_resize"] == true && mid["resizes_held"] == false && sent_of(&mid) == sent_before,
                "{what} step {step}: live_resize {} resizes_held {}, sent {:?} before {sent_before:?}", mid["live_resize"], mid["resizes_held"], sent_of(&mid));
        }
        let ended = native.call(json!({"op":"live_resize","on":false}));
        ensure!(ended["held"] == false, "{what}: the live resize's end left the resizes held: {ended}");
        draw(window, None, cx)?;
        pump(&mut connection, 800, &mut layouts, window, cx).await?;
        let settled = settled_once(&native, &terms, &sent_before, &before, what).await?;
        ensure!(native.status()["live_resize"] == false, "{what} left the window resizing");
        window_resize.push(json!({"what": what, "width": window.update(cx, |_, w, _| f32::from(w.viewport_size().width))?, "terminals": settled}));
    }
    ensure!(window.update(cx, |_, w, _| w.viewport_size())? == window_size, "the window did not come back to its size");
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, split, parent, start, moved, drag_steps, drag_settled, window_resize, after_drag, after_status, saved, status, external, external_status, cli))
}

/// The agent pane: a typed message reaches wheeld and the fake agent's turns come back as rows.
async fn agent_turns(run: Run, rects: Vec<(String, Rect)>, cx: &mut AsyncApp) -> Result<(Run, String, String, Rect, String, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, agent_pane, agent_name, agent_rect, cli_path, agent_report))
}

/// The agent pane's pane state as `/state` reports it.
fn pane_state(window: gpui::WindowHandle<Client>, pane: &str, cx: &mut AsyncApp) -> Result<Value> {
    Ok(window.update(cx, |c, _, _| c.state_json()["panes"][pane].clone())?)
}

/// Press a control the last frame drew, by its recorded name, with real mouse events.
async fn press_named(native: &Native, window: gpui::WindowHandle<Client>, agent: &str, name: &str, cx: &mut AsyncApp) -> Result<()> {
    let b = window.update(cx, |c, _, _| c.control_bounds(agent, name))?.ok_or_else(|| anyhow::anyhow!("{name} was not drawn"))?;
    let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":p}));
    native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    tick(cx, 150).await;
    draw(window, None, cx)
}

/// Pump wheeld's events and draw until `want` holds of the agent pane's `/state`, or fail with `what`.
async fn wait_pane(run: &mut Run, pane: &str, what: &str, want: impl Fn(&Value) -> bool, cx: &mut AsyncApp) -> Result<Value> {
    let mut state = Value::Null;
    for _ in 0..100 {
        pump(&mut run.connection, 100, &mut run.layouts, run.window, cx).await?;
        draw(run.window, None, cx)?;
        state = pane_state(run.window, pane, cx)?;
        if want(&state["footer"]) {
            return Ok(state["footer"].clone());
        }
    }
    let (focus, menus) = run.window.update(cx, |c, _, _| { let s = c.state_json(); (s["focus"].clone(), json!({"palette": s["palette"], "pane_menu": s["pane_menu"]})) })?;
    let shown: Value = state.as_object().map(|o| o.iter().filter(|(k, _)| k.as_str() != "footer").map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    anyhow::bail!("{what}: the pane's footer is {}; the pane is {shown}; focus {focus}; {menus}", state["footer"])
}

/// Send `text` to the agent in `pane` through `wheel agent send`, as background-footer.sh does.
fn agent_send(pane: &str, text: &str) -> Result<()> {
    let out = std::process::Command::new(std::env::var("WHEEL_CLIENT_CLI")?)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "send", "--timeout", "0", pane, text]).output()?;
    ensure!(out.status.success(), "wheel agent send {text:?}: {}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}

/// Background tasks (wheel-96jo): the fake agent starts a background shell and a subagent; the
/// strip above the composer has a row for each with its latest line and running time, and the
/// shell's call on the transcript reads `● running m:ss`.
async fn running_strip(run: Run, agent_pane: String, _agent_name: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    let mut run = run;
    let (window, native, output) = (run.window, run.native.clone(), run.output.clone());
    if window.update(cx, |c, _, _| c.focus.current.clone())?.as_deref() != Some(agent_pane.as_str()) {
        let f = pane_state(window, &agent_pane, cx)?["frame"].clone();
        let p = [f[0].as_f64().unwrap_or(0.) + f[2].as_f64().unwrap_or(0.) / 2., f[1].as_f64().unwrap_or(0.) + f[3].as_f64().unwrap_or(0.) / 2.];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
        tick(cx, 300).await;
    }
    let draft = pane_state(window, &agent_pane, cx)?["draft"].as_str().unwrap_or_default().chars().count();
    erase(&native, draft, cx).await;
    let before = wait_pane(&mut run, &agent_pane, "before any task", |f| f["tasks"].as_array().is_some_and(Vec::is_empty) || f.is_null(), cx).await?;
    agent_send(&agent_pane, "start background")?;
    let shown = |f: &Value| f["tasks"].as_array().is_some_and(|t| t.len() == 3 && t[1]["latest"] == "2 tool calls \u{b7} last: Bash \u{b7} 26.7k tokens" && t.iter().all(|t| t["strip_row_drawn"] == true))
        && f["marks"].as_array().is_some_and(|m| m.len() == 2);
    let footer = match wait_pane(&mut run, &agent_pane, "one shell and two subagents in the strip", shown, cx).await {
        Ok(footer) => footer,
        Err(e) => {
            let read = std::process::Command::new(std::env::var("WHEEL_CLIENT_CLI")?)
                .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "read", &agent_pane]).output()?;
            let text = String::from_utf8_lossy(&read.stdout);
            let tail: String = text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
            anyhow::bail!("{e}; wheeld's transcript ends: {tail}{}", String::from_utf8_lossy(&read.stderr));
        }
    };
    let (shell, sub) = (&footer["tasks"][0], &footer["tasks"][1]);
    ensure!(footer["background_text"] == "1 shell \u{b7} 2 agents", "the status line reads {}", footer["background_text"]);
    ensure!(shell["id"] == "bash-1" && shell["kind"] == "shell" && shell["command"] == "make deploy-client" && shell["call"] == "bg-call-1" && shell["latest"] == "line 3" && shell["output_bytes"] == 21,
        "the shell's strip row: {shell}");
    ensure!(sub["id"] == "agent-1" && sub["kind"] == "agent" && sub["call"] == "bg-call-2", "the subagent's strip row: {sub}");
    let clock = |t: &Value| t["elapsed"].as_str().is_some_and(|e| e.split(':').count() == 2 && e.split(':').all(|p| p.chars().all(|c| c.is_ascii_digit())));
    ensure!(clock(shell) && clock(sub), "running times are not m:ss: {} {}", shell["elapsed"], sub["elapsed"]);
    let mark = footer["marks"].as_array().and_then(|m| m.iter().find(|m| m["call"] == "bg-call-1")).cloned().unwrap_or(Value::Null);
    let text = mark["text"].as_str().unwrap_or_default();
    ensure!(text.starts_with("\u{25cf} running ") && clock(&json!({"elapsed": text.trim_start_matches("\u{25cf} running ")})), "the call's transcript row reads {mark}");
    ensure!(footer["ticking"] == true, "no redraw is armed to tick the clocks: {footer}");
    draw(window, Some(output.join("client-running-strip.png")), cx)?;
    let report = json!({"before": before, "footer": footer, "mark": mark});
    Ok((run, report))
}

/// The strip's row opened to its last lines, the full view over the transcript with the text
/// `task_output` returned, its tabs, Escape, Show in Transcript, and the tasks ending.
async fn running_full_view(run: Run, agent_pane: String, agent_name: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    let mut run = run;
    let (window, native, output) = (run.window, run.native.clone(), run.output.clone());
    press_named(&native, window, &agent_name, "task-row:bash-1", cx).await?;
    let open = wait_pane(&mut run, &agent_pane, "the shell's row open", |f| f["tasks"][0]["expanded"] == true, cx).await?;
    ensure!(open["tasks"][0]["detail"] == json!(["line 1", "line 2", "line 3"]), "the open row's lines: {}", open["tasks"][0]["detail"]);
    draw(window, Some(output.join("client-running-row-open.png")), cx)?;
    press_named(&native, window, &agent_name, "task-full:bash-1", cx).await?;
    let full = wait_pane(&mut run, &agent_pane, "the full view with the shell's output",
        |f| f["full_task"] == "bash-1" && f["full_view_drawn"] == true && f["tasks"][0]["output_read"] == "line 1\nline 2\nline 3\n", cx).await?;
    ensure!(full["tasks"][0]["output_read_bytes"] == 21, "the output read: {}", full["tasks"][0]);
    draw(window, Some(output.join("client-running-full.png")), cx)?;
    press_named(&native, window, &agent_name, "task-tab:agent-1", cx).await?;
    wait_pane(&mut run, &agent_pane, "the subagent's tab", |f| f["full_task"] == "agent-1", cx).await?;
    press_named(&native, window, &agent_name, "task-tab:bash-1", cx).await?;
    wait_pane(&mut run, &agent_pane, "the shell's tab", |f| f["full_task"] == "bash-1", cx).await?;
    native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    let escaped = wait_pane(&mut run, &agent_pane, "Escape closing the full view", |f| f["full_task"].is_null(), cx).await?;
    press_named(&native, window, &agent_name, "task-full:bash-1", cx).await?;
    wait_pane(&mut run, &agent_pane, "the full view again", |f| f["full_task"] == "bash-1" && f["full_view_drawn"] == true, cx).await?;
    press_named(&native, window, &agent_name, "task-reveal:bash-1", cx).await?;
    wait_pane(&mut run, &agent_pane, "Show in Transcript closing the full view", |f| f["full_task"].is_null(), cx).await?;
    let revealed = window.update(cx, |c, _, _| c.toast_log.iter().any(|l| l.contains("shows call bg-call-1")))?;
    ensure!(revealed, "the log does not say the transcript scrolled to bg-call-1");
    agent_send(&agent_pane, "end background")?;
    let ended = wait_pane(&mut run, &agent_pane, "the tasks ending",
        |f| f["tasks"].as_array().is_some_and(Vec::is_empty) && f["background_text"].is_null() && f["marks"].as_array().is_some_and(Vec::is_empty), cx).await?;
    ensure!(window.update(cx, |c, _, _| c.control_bounds(&agent_name, "task-row:bash-1"))?.is_none(), "a strip row is still drawn after the tasks ended");
    draw(window, Some(output.join("client-running-ended.png")), cx)?;
    Ok((run, json!({"open": open, "full": full, "escaped": escaped["full_task"], "revealed": revealed, "ended": ended})))
}

/// A Markdown reply drawn as blocks, its first line selected with the pointer and copied.
async fn markdown_reply(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, markdown_report))
}

/// The footer under the composer: the context meter, the config controls and their menus,
/// tooltips, and whole items in a narrow pane.
async fn footer(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    // The footer under the composer: the context meter (three turns answered, 10,000 tokens each
    // of a 200,000 window), the directory, the agent's home, and the config controls the agent
    // offers, sorted model first. A click on the meter reads the raw figures; a click on a
    // control opens its menu; picking a value asks wheeld and the control follows the agent's
    // own list.
    let footer_before = window.update(cx, |c, _, _| c.footer_items_all(&agent_name))?;
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
    let raw_text = item(&window.update(cx, |c, _, _| c.footer_texts(&agent_name))?.0, "context").unwrap_or_default();
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
    draw(window, None, cx)?;
    // The menu from the keyboard (wheel-ynjo.21): reopened, it starts on the value in force
    // (Haiku, the last of default, sonnet, haiku); Up moves to Sonnet and Return picks it.
    let model = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "model"))?.ok_or_else(|| anyhow::anyhow!("the model control was not drawn after the pick"))?;
    press(&native, model);
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let row_opened = window.update(cx, |c, _, _| (c.open_menu(), c.menu_row()))?;
    ensure!(row_opened == (Some((agent_name.clone(), "model".into())), 2), "the reopened model menu (menu, row) is {row_opened:?}, wanted the model menu on row 2 (Haiku)");
    native.call(json!({"op":"post_key","text":"\u{f700}","code":126}));
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let row_up = window.update(cx, |c, _, _| c.menu_row())?;
    ensure!(row_up == 1, "after Up the menu is on row {row_up}, wanted 1 (presses {:?})", window.update(cx, |c, _, _| c.presses.clone())?);
    draw(window, Some(output.join("client-config-menu-keys.png")), cx)?;
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut model_keyed = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        model_keyed = window.update(cx, |c, _, _| c.transcripts.get(&agent_name).and_then(|t| t.config.iter().find(|o| o.id == "model")).map(wheel_gpui::footer::control_current))?;
        if model_keyed.as_deref() == Some("sonnet") {
            break;
        }
    }
    ensure!(model_keyed.as_deref() == Some("sonnet"), "after Return on row 1 the model is {model_keyed:?}, wanted sonnet");
    ensure!(window.update(cx, |c, _, _| c.open_menu())?.is_none(), "the menu stayed open after Return");
    draw(window, None, cx)?;
    // Tooltips: resting the pointer on the meter, a control and the pane's status dot shows the
    // Swift client's words for each.
    let any: gpui::AnyWindowHandle = window.into();
    let mut tips = serde_json::Map::new();
    let (status_label, context_help, model_help) = window.update(cx, |c, _, _| {
        let t = c.transcripts.get(&agent_name);
        (c.pane_status(&agent_pane).map(|s| wheel_gpui::status::label(&s)),
         t.and_then(|t| t.context.as_ref()).map(wheel_gpui::footer::help_text),
         t.and_then(|t| t.config.iter().find(|o| o.id == "model")).map(wheel_gpui::footer::control_help))
    })?;
    for (what, owner, control, want) in [("context", agent_name.as_str(), "context", context_help), ("model", agent_name.as_str(), "model", model_help), ("dot", agent_pane.as_str(), "status-dot", status_label)] {
        let b = window.update(cx, |c, _, _| c.control_bounds(owner, control))?.ok_or_else(|| anyhow::anyhow!("{control} was not drawn for a hover"))?;
        let position = gpui::point(gpui::px(b.0 + b.2 / 2.), gpui::px(b.1 + b.3 / 2.));
        wheel_gpui::tooltip::take_shown();
        let mut shown = None;
        for _ in 0..20 {
            any.update(cx, |_, w, cx| {
                w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button: None, modifiers: Default::default() }), cx);
                w.draw(cx).clear();
            })?;
            tick(cx, 100).await;
            if let Some(text) = wheel_gpui::tooltip::take_shown() {
                shown = Some(text);
                break;
            }
        }
        if what == "context" {
            draw(window, Some(output.join("client-footer-tooltip.png")), cx)?;
        }
        ensure!(want.is_some() && shown == want, "hovering {what} at {b:?} showed {shown:?}, wanted {want:?}");
        tips.insert(what.into(), json!(shown));
        // Away from it, so the next hover starts a new tooltip.
        any.update(cx, |_, w, cx| {
            w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: gpui::point(gpui::px(1.), gpui::px(1.)), pressed_button: None, modifiers: Default::default() }), cx);
            w.draw(cx).clear();
        })?;
        tick(cx, 100).await;
    }
    // A narrow pane: every side-by-side split above the agent pane gives it 0.35 of its width
    // (a 335 pt pane), and the footer keeps whole items from the front with "…" for the rest,
    // each drawn inside the row. A wide pane first (0.9 of each), where every item fits. Splits
    // that stack the pane keep their ratio, so its footer stays in view.
    let set_split = |split: &str, ratio: f64| -> Result<()> {
        let out = std::process::Command::new(&cli_path)
            .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", split, &ratio.to_string()]).output()?;
        ensure!(out.status.success(), "pane ratio {split} {ratio}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Ok(())
    };
    let restore: Vec<_> = window.update(cx, |c, _, _| c.splits_to(&agent_pane))?.into_iter().filter(|s| s.horizontal).collect();
    ensure!(!restore.is_empty(), "the agent pane has no side-by-side split above it to narrow");
    let mut sized = vec![];
    let mut narrow_crop = None;
    for (share, other) in [(0.9, 0.1), (0.35, 0.65)] {
        for s in &restore {
            set_split(&s.id, if s.first { share } else { other })?;
        }
        for _ in 0..50 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            if window.update(cx, |c, _, _| c.splits_to(&agent_pane).iter().filter(|s| s.horizontal).all(|s| (s.ratio - if s.first { share } else { other }).abs() < 0.01))? {
                break;
            }
        }
        draw(window, None, cx)?;
        if share > 0.5 {
            draw(window, Some(output.join("client-footer-wide.png")), cx)?;
        } else {
            // The narrow pane's bottom: the end of its composer and the whole footer row.
            let (pane_rect, context) = window.update(cx, |c, _, _| (c.scene().0.into_iter().find(|p| p.leaf.id == agent_pane).map(|p| p.rect), c.control_bounds(&agent_name, "context")))?;
            let pane_rect = pane_rect.ok_or_else(|| anyhow::anyhow!("the narrow agent pane {agent_pane} was not placed"))?;
            let any: gpui::AnyWindowHandle = window.into();
            let crop = any.update(cx, |_, w, cx| -> Result<[u32; 4]> {
                w.draw(cx).clear();
                let image = w.render_to_image()?;
                let scale = image.width() as f32 / f32::from(w.viewport_size().width);
                let bottom = pane_rect.y + pane_rect.h;
                let top = context.map_or(bottom - 60., |b| b.1 - 40.).max(pane_rect.y);
                let [x, y] = [(pane_rect.x - 8.).max(0.), top].map(|v| (v * scale) as u32);
                let [x1, y1] = [pane_rect.x + pane_rect.w + 8., bottom + 8.].map(|v| (v * scale) as u32);
                let (w, h) = (x1.min(image.width()) - x, y1.min(image.height()) - y);
                image::imageops::crop_imm(&image, x, y, w, h).to_image().save(output.join("client-footer-narrow.png"))?;
                Ok([x, y, w, h])
            })??;
            narrow_crop = Some(json!({"pane": [pane_rect.x, pane_rect.y, pane_rect.w, pane_rect.h], "pixels": crop}));
        }
        sized.push(window.update(cx, |c, _, _| c.footer_texts(&agent_name))?);
    }
    let (footer_wide, footer_narrow) = (sized[0].clone(), sized[1].clone());
    let all_items = window.update(cx, |c, _, _| c.footer_items_all(&agent_name))?;
    for s in &restore {
        set_split(&s.id, s.ratio)?;
    }
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.splits_to(&agent_pane).iter().filter(|s| s.horizontal).zip(&restore).all(|(now, was)| (now.ratio - was.ratio).abs() < 0.01))? {
            break;
        }
    }
    draw(window, None, cx)?;
    ensure!(!footer_wide.1 && footer_wide.0 == all_items, "the wide footer {footer_wide:?} does not show every item of {all_items:?}");
    ensure!(footer_narrow.1 && !footer_narrow.0.is_empty() && footer_narrow.0.len() < all_items.len() && all_items.starts_with(&footer_narrow.0),
        "the narrow footer {footer_narrow:?} is not a shorter prefix of {all_items:?} with \u{2026}");
    let footer_report = json!({"items": footer_before, "raw_text": raw_text, "controls": controls, "model_before": before_set, "model_after": model_after,
        "menu_rows": {"opened": row_opened.1, "after_up": row_up}, "model_keyed": model_keyed, "tooltips": tips,
        "wide": {"items": footer_wide.0, "more": footer_wide.1}, "narrow": {"items": footer_narrow.0, "more": footer_narrow.1, "shot": narrow_crop}});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, footer_report))
}

/// The pane's status dot and the space chip's marks follow `status_changed`.
async fn statuses(run: Run, agent_pane: String, agent_name: String, agent_rect: Rect, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, bool, bool, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
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
    // The card this request draws above the composer leaves when the CLI answers it (wheel-ynjo.10.2).
    let card_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.perm_card_json(&agent_pane));
    let card_before_cli = !card_now(cx)?.is_null();
    // In this short pane the card's lines scroll and its buttons stay inside the pane.
    let short_card = card_now(cx)?;
    let short_allow = short_card["buttons"].as_array().and_then(|b| b.iter().find(|b| b["prominent"] == true)).and_then(|b| b["id"].as_str()).unwrap_or_default().to_owned();
    let short_button = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("perm:{short_allow}")))?;
    let pane_bottom = agent_rect.y + agent_rect.h;
    // The card clips what it cannot fit, so the button must lie inside the card as drawn too.
    let short_frame = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "perm-card"))?;
    let buttons_inside_short_pane = matches!((short_button, short_frame), (Some(b), Some(f)) if b.1 >= f.1 && b.1 + b.3 <= f.1 + f.3 && f.1 + f.3 <= pane_bottom);
    ensure!(buttons_inside_short_pane, "the card's Allow button {short_button:?} is not inside the card {short_frame:?} and the agent pane (bottom {pane_bottom}): {short_card}");
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
    let card_after_cli = card_now(cx)?;
    let card_control_after_cli = window.update(cx, |c, _, _| c.perm_buttons_drawn(&agent_name) > 0)?;
    let resolved_from_cli = card_before_cli && card_after_cli.is_null() && !card_control_after_cli;
    ensure!(resolved_from_cli, "the CLI's answer left the card: before {card_before_cli}, after {card_after_cli}, control drawn {card_control_after_cli}");
    let status_report = json!({"at_rest": status_name(&at_rest), "saw_working": saw_working, "marks_while_working": marks_while_working,
        "needs_you": wheel_gpui::status::label(&needs_you), "marks_needing": marks_needing, "after_answer": status_name(&after_answer),
        "dots_setting": window.update(cx, |c, _, _| c.status_dots.iter().copied().collect::<Vec<_>>())?});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, buttons_inside_short_pane, resolved_from_cli, status_report))
}

/// Permission and question cards: Return allows, a click on each first answer answers.
async fn perm_cards(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Vec<wheel_gpui::layout::SplitStep>, Value, bool, String, Value, usize, Vec<String>, String)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let card_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.perm_card_json(&agent_pane));
    // Permission and question cards (wheel-ynjo.10.2): a described permission draws its card
    // above the composer and Return from the empty composer allows it; a question draws one
    // button per answer and a click on the first answers it.
    let replies_since = |rows: &[wheel_gpui::transcript::Row], from: usize| rows.iter().skip(from).filter_map(|r| match &r.kind { Kind::Reply(x) => Some(x.clone()), _ => None }).collect::<Vec<_>>().join("\n");
    // The cards are taller than the fixture's agent pane: every split above the pane gives it 0.9
    // of its room until the question is answered, then takes its ratio back.
    let set_ratio = |split: &str, ratio: f64| -> Result<()> {
        let out = std::process::Command::new(&cli_path)
            .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", split, &ratio.to_string()]).output()?;
        ensure!(out.status.success(), "pane ratio {split} {ratio}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Ok(())
    };
    let splits_above = window.update(cx, |c, _, _| c.splits_to(&agent_pane))?;
    for s in &splits_above {
        set_ratio(&s.id, if s.first { 0.9 } else { 0.1 })?;
    }
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.splits_to(&agent_pane).iter().all(|s| (s.ratio - if s.first { 0.9 } else { 0.1 }).abs() < 0.01))? {
            break;
        }
    }
    let rows_before_perm = rows(cx)?.1.len();
    type_line(&native, "permission described", cx).await;
    let mut perm_card = Value::Null;
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        perm_card = card_now(cx)?;
        // The request names tool call call-1, whose row the agent sent first: the card draws it.
        if !perm_card.is_null() && !perm_card["tool"].is_null() {
            break;
        }
    }
    ensure!(!perm_card.is_null(), "the described permission drew no card");
    ensure!(perm_card["tool_call"] == "call-1", "the card names no tool call call-1: {perm_card}");
    let tool = &perm_card["tool"];
    ensure!(tool["glyph"] == "›_" && tool["action"] == "Run a command" && tool["content"].as_array().is_some(),
        "the card drew no Bash tool line for call-1: {perm_card}");
    draw(window, None, cx)?;
    ensure!(window.update(cx, |c, _, _| c.control_bounds(&agent_name, "perm-tool"))?.is_some(), "the card's tool line drew nowhere: {perm_card}");
    // The card is drawn exactly as tall as `perms::height` says, so the transcript gives up that much.
    // Control `perm-card` spans the box inside the card's border.
    let drawn_h = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "perm-card"))?.map(|b| b.3);
    let want_h = perm_card["height"].as_f64().unwrap_or_default() as f32 - wheel_gpui::perms::MARGIN - 2. * wheel_gpui::perms::BORDER;
    ensure!(drawn_h.is_some_and(|h| (h - want_h).abs() < 0.5), "the card drew {drawn_h:?} pt tall, perms::height says {want_h}: {perm_card}");
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-perm-card.png")), cx)?;
    let allow_id = perm_card["buttons"].as_array().and_then(|b| b.iter().find(|b| b["prominent"] == true)).and_then(|b| b["id"].as_str()).unwrap_or_default().to_owned();
    let card_shown = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("perm:{allow_id}")))?.is_some();
    ensure!(card_shown, "the card drew no Allow button: {perm_card}");
    ensure!(window.update(cx, |c, _, _| c.focus.current.clone())?.as_deref() == Some(agent_pane.as_str()), "the agent pane lost the keyboard before Return");
    native.call(json!({"op":"post_key","text":"\r","code":36}));
    let mut answered_by_key = String::new();
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, rows) = rows(cx)?;
        let text = replies_since(&rows, rows_before_perm);
        if card_now(cx)?.is_null() && text.contains("permission: allow") && state == "ready" {
            answered_by_key = "allow".into();
            break;
        }
        if text.contains("permission: deny") {
            answered_by_key = "deny".into();
            break;
        }
    }
    ensure!(answered_by_key == "allow", "Return on the card answered {answered_by_key:?}: {}", replies_since(&rows(cx)?.1, rows_before_perm));
    let rows_before_question = rows(cx)?.1.len();
    type_line(&native, "question", cx).await;
    let mut question_card = Value::Null;
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        question_card = card_now(cx)?;
        if question_card["question"] == true {
            break;
        }
    }
    ensure!(question_card["question"] == true, "the question drew no question card: {question_card}");
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-perm-question.png")), cx)?;
    let question_options = question_card["answers"].as_array().map_or(0, Vec::len);
    let first_answer = question_card["answers"][0]["id"].as_str().unwrap_or_default().to_owned();
    let answer_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("perm:{first_answer}")))?.ok_or_else(|| anyhow::anyhow!("the question's first answer was not drawn: {question_card}"))?;
    press(&native, answer_at);
    // The fake agent asks two questions, Color then Size: each card's first answer is clicked.
    let mut clicked = vec![question_card["id"].as_str().unwrap_or_default().to_owned()];
    let mut question_answer = String::new();
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let card = card_now(cx)?;
        if let Some(id) = card["id"].as_str().filter(|id| !clicked.iter().any(|c| c == id)) {
            draw(window, None, cx)?;
            let first = card["answers"][0]["id"].as_str().unwrap_or_default().to_owned();
            if let Some(at) = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("perm:{first}")))? {
                clicked.push(id.to_owned());
                press(&native, at);
            }
        }
        let (state, rows) = rows(cx)?;
        let text = replies_since(&rows, rows_before_question);
        if let Some(at) = text.find("question: ") && card.is_null() && state == "ready" {
            question_answer = text[at + 10..].lines().next().unwrap_or_default().to_owned();
            break;
        }
    }
    ensure!(question_answer == r#"allow answers={"Which color?":"Red","Which size?":"Small"}"#, "clicks on each question's first answer gave {question_answer:?}: cards clicked {clicked:?}, card now {}, presses {:?}, replies {:?}",
        card_now(cx)?, window.update(cx, |c, _, _| c.presses.clone())?, replies_since(&rows(cx)?.1, rows_before_question));
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, splits_above, perm_card, card_shown, answered_by_key, question_card, question_options, clicked, question_answer))
}

/// The composer's command list, the queue and its actions, Control-Return and Esc.
async fn composer_and_queue(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Vec<String>, String, Value, String, String, [String; 6], bool, bool, bool, bool, bool, bool)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    // The composer (wheel-ynjo.10.4.1): `/` lists the fake agent's commands (its `slash_commands`),
    // the arrows with Return, Tab and a click each pick one, and Esc shuts the list. Messages sent
    // during a slow turn show queued with their actions; Edit, Clear queue and Send now pressed
    // with the pointer, and Control-Return, change what wheeld holds.
    let pane_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.state_json()["panes"][agent_pane.as_str()].clone());
    let key = |native: &Native, text: &str, code: u16| {
        native.call(json!({"op":"post_key","text":text,"code":code}));
    };
    let draft_now = |cx: &mut AsyncApp| -> Result<String> { Ok(pane_now(cx)?["draft"].as_str().unwrap_or_default().to_owned()) };
    ensure!(draft_now(cx)?.is_empty(), "the composer holds {:?} before the command list check", draft_now(cx)?);
    type_text(&native, "/", cx).await;
    draw(window, None, cx)?;
    let first_list = pane_now(cx)?["composer"].clone();
    let commands_listed: Vec<String> = serde_json::from_value(first_list["listed"].clone()).unwrap_or_default();
    ensure!(commands_listed == ["clear", "compact", "review"], "a `/` draft listed {commands_listed:?}: composer {first_list}, draft {:?}", draft_now(cx)?);
    draw(window, Some(output.join("client-composer-commands.png")), cx)?;
    let list_rows_drawn = ["clear", "compact", "review"].iter().all(|n| window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("command:{n}"))).ok().flatten().is_some());
    ensure!(list_rows_drawn, "the list drew no row for each command: controls {:?}", window.update(cx, |c, _, _| c.control_bounds(&agent_name, "command-list"))?);
    key(&native, "\u{f701}", 125);
    tick(cx, 50).await;
    key(&native, "\u{f701}", 125);
    tick(cx, 100).await;
    let highlighted = pane_now(cx)?["composer"]["highlighted"].clone();
    key(&native, "\r", 36);
    tick(cx, 200).await;
    let by_arrows = draft_now(cx)?;
    ensure!(highlighted == 2 && by_arrows == "/review ", "Down Down Return: highlight {highlighted}, draft {by_arrows:?}");
    erase(&native, by_arrows.chars().count(), cx).await;
    type_text(&native, "/co", cx).await;
    let co_listed = pane_now(cx)?["composer"]["listed"].clone();
    key(&native, "\t", 48);
    tick(cx, 200).await;
    let by_tab = draft_now(cx)?;
    ensure!(co_listed[0] == "compact" && by_tab == "/compact ", "`/co` listed {co_listed} and Tab left {by_tab:?}");
    erase(&native, by_tab.chars().count(), cx).await;
    type_text(&native, "/", cx).await;
    draw(window, None, cx)?;
    let clear_row = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "command:clear"))?.ok_or_else(|| anyhow::anyhow!("no clear row drawn"))?;
    press(&native, clear_row);
    tick(cx, 200).await;
    let by_click = draft_now(cx)?;
    ensure!(by_click == "/clear ", "a click on the clear row left {by_click:?}");
    erase(&native, by_click.chars().count() - 1, cx).await;
    key(&native, "\u{1b}", 53);
    tick(cx, 200).await;
    let shut = pane_now(cx)?;
    ensure!(shut["draft"] == "/" && shut["composer"]["listed"] == json!([]), "Esc on the open list: {}", shut["composer"]);
    erase(&native, 1, cx).await;
    // The queue: a slow turn runs while two messages wait behind it.
    let queued_now = |cx: &mut AsyncApp| -> Result<Vec<u64>> { Ok(serde_json::from_value(pane_now(cx)?["composer"]["queued"].clone()).unwrap_or_default()) };
    // Scrolled to the first row, the column stops following; sending takes it back to its end and
    // it follows again, so the queue lines under the new messages draw.
    window.update(cx, |c, _, _| c.reveal(&agent_name, 0))?;
    draw(window, None, cx)?;
    let before_send = window.update(cx, |c, _, _| c.column_scroll(&agent_name))?;
    ensure!(before_send.is_some_and(|(top, _, following)| top == 0 && !following), "revealing the first row left the column at {before_send:?}");
    type_line(&native, "work slowly 20", cx).await;
    draw(window, None, cx)?;
    let after_send = window.update(cx, |c, _, _| c.column_scroll(&agent_name))?;
    let rows_after_send = rows(cx)?.1.len();
    ensure!(after_send.is_some_and(|(top, _, following)| top >= rows_after_send && following), "a sent message left the column away from its end: before {before_send:?}, after {after_send:?} of {rows_after_send} rows");
    let mut slow_busy = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "busy" {
            slow_busy = true;
            break;
        }
    }
    ensure!(slow_busy, "the slow turn never ran: state {:?}", rows(cx)?.0);
    type_line(&native, "queued one", cx).await;
    tick(cx, 100).await;
    type_line(&native, "queued two", cx).await;
    let mut waiting = vec![];
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        waiting = queued_now(cx)?;
        if waiting.len() == 2 {
            break;
        }
    }
    ensure!(waiting.len() == 2, "two messages sent during the slow turn left the queue {waiting:?}: state {:?}", rows(cx)?.0);
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-composer-queued.png")), cx)?;
    let (q1, q2) = (waiting[0], waiting[1]);
    let queue_ids = [format!("queue:now:{q1}"), format!("queue:edit:{q1}"), format!("queue:remove:{q1}"), "queue:clear".to_owned(), format!("queue:now:{q2}"), format!("queue:edit:{q2}")];
    let queue_drawn: Vec<bool> = queue_ids.iter().map(|id| window.update(cx, |c, _, _| c.control_bounds(&agent_name, id)).ok().flatten().is_some()).collect();
    ensure!(queue_drawn.iter().all(|d| *d), "the queued rows' actions {queue_ids:?} drawn: {queue_drawn:?}");
    let edit_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("queue:edit:{q2}")))?.unwrap_or_default();
    press(&native, edit_at);
    let mut took_back = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if draft_now(cx)? == "queued two" && queued_now(cx)? == [q1] {
            took_back = true;
            break;
        }
    }
    ensure!(took_back, "Edit on the second queued message: draft {:?}, queue {:?}, presses {:?}", draft_now(cx)?, queued_now(cx)?, window.update(cx, |c, _, _| c.presses.clone())?);
    erase(&native, "queued two".len(), cx).await;
    type_line(&native, "queued three", cx).await;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if queued_now(cx)?.len() == 2 {
            break;
        }
    }
    draw(window, None, cx)?;
    let clear_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "queue:clear"))?.ok_or_else(|| anyhow::anyhow!("no Clear queue with two queued: {:?}", queued_now(cx)))?;
    press(&native, clear_at);
    let mut cleared_queue = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if queued_now(cx)?.is_empty() && !rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::User { text, .. } if text.starts_with("queued "))) {
            cleared_queue = true;
            break;
        }
    }
    let agent_read = || -> Result<String> {
        let out = std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "read", &agent_pane]).output()?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    };
    let read_after_clear = agent_read()?;
    ensure!(cleared_queue && !read_after_clear.contains("(queued)"), "Clear queue: queue {:?}, wheeld's transcript still queues: {}", queued_now(cx)?, read_after_clear.lines().filter(|l| l.contains("(queued)")).collect::<Vec<_>>().join(" | "));
    type_line(&native, "send me now", cx).await;
    let mut now_turn = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if let [t] = queued_now(cx)?[..] {
            now_turn = Some(t);
            break;
        }
    }
    let now_turn = now_turn.ok_or_else(|| anyhow::anyhow!("`send me now` was never queued: state {:?}", rows(cx).map(|r| r.0)))?;
    draw(window, None, cx)?;
    let now_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("queue:now:{now_turn}")))?.ok_or_else(|| anyhow::anyhow!("no Send now on turn {now_turn}"))?;
    press(&native, now_at);
    let mut queued_then_sent = false;
    let mut saw_sending = false;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        saw_sending |= pane_now(cx)?["composer"]["sending_now"].as_array().is_some_and(|s| !s.is_empty());
        if rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::User { text, queued: false } if text == "send me now")) {
            queued_then_sent = true;
            break;
        }
    }
    let read_after_now = agent_read()?;
    ensure!(queued_then_sent && read_after_now.contains("> send me now") && !read_after_now.contains("> send me now (queued)"),
        "Send now on turn {now_turn}: ran {queued_then_sent}, wheeld's transcript tail: {}", read_after_now.lines().rev().take(6).collect::<Vec<_>>().join(" | "));
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "ready" {
            break;
        }
    }
    // Control-Return while a turn runs sends the draft to go first.
    type_line(&native, "work slowly 20", cx).await;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "busy" {
            break;
        }
    }
    type_text(&native, "urgent", cx).await;
    native.call(json!({"op":"post_key","text":"\r","code":36,"control":true}));
    let mut control_return_ran = false;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::User { text, queued: false } if text == "urgent")) {
            control_return_ran = true;
            break;
        }
    }
    ensure!(control_return_ran, "Control-Return during the slow turn: draft {:?}, queue {:?}, state {:?}", draft_now(cx)?, queued_now(cx)?, rows(cx)?.0);
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "ready" {
            break;
        }
    }
    // Esc with text in the box still stops a running turn, and the text stays.
    type_line(&native, "work slowly 20", cx).await;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "busy" {
            break;
        }
    }
    type_text(&native, "abc", cx).await;
    key(&native, "\u{1b}", 53);
    let mut esc_stopped = false;
    for _ in 0..80 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "ready" {
            esc_stopped = true;
            break;
        }
    }
    let kept = draft_now(cx)?;
    ensure!(esc_stopped && kept == "abc", "Esc with `abc` typed during a slow turn: stopped {esc_stopped}, state {:?}, draft {kept:?}", rows(cx)?.0);
    erase(&native, 3, cx).await;
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, commands_listed, by_arrows, co_listed, by_tab, by_click, queue_ids, took_back, cleared_queue, queued_then_sent, saw_sending, control_return_ran, esc_stopped))
}

/// Attachments: a dropped file and a pasted image become chips and go out as paths.
async fn attachments(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let pane_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.state_json()["panes"][agent_pane.as_str()].clone());
    let key = |native: &Native, text: &str, code: u16| {
        native.call(json!({"op":"post_key","text":text,"code":code}));
    };
    let draft_now = |cx: &mut AsyncApp| -> Result<String> { Ok(pane_now(cx)?["draft"].as_str().unwrap_or_default().to_owned()) };
    // Attachments (wheel-ynjo.10.4.2): a file dropped on the composer and an image pasted with
    // Command-V become chips; × on the image's chip deletes the copy it wrote; the message goes
    // out with each path in backticks on its own line after the text, and the chips empty. A
    // drop with nothing typed sends the path alone on Return.
    let attach_dir = std::env::temp_dir().join(format!("wheel-gpui-attach-{}", std::process::id()));
    std::fs::create_dir_all(&attach_dir)?;
    /// Removes the folders the step wrote, whether it passes or fails.
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for dir in &self.0 {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
    let mut cleanup = Cleanup(vec![attach_dir.clone()]);
    // The person's clipboard, put back once the paste steps are done.
    let clipboard_before = cx.update(|app| app.read_from_clipboard());
    let attach_file = attach_dir.join("notes for agent.txt");
    std::fs::write(&attach_file, "attached from the GPUI check\n".repeat(60))?;
    let log_tail = |cx: &mut AsyncApp| -> Result<Vec<String>> { Ok(window.update(cx, |c, _, _| c.toast_log.iter().rev().take(8).cloned().collect())?) };
    let attachments_now = |cx: &mut AsyncApp| -> Result<Vec<Value>> { Ok(pane_now(cx)?["composer"]["attachments"].as_array().cloned().unwrap_or_default()) };
    let clip_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "attach"))?.ok_or_else(|| anyhow::anyhow!("the composer drew no paperclip"))?;
    let drop_at = gpui::point(gpui::px(clip_at.0 + 120.), gpui::px(clip_at.1 + clip_at.3 / 2.));
    // Through the untyped handle: a typed update holds the Client, which the drop then updates.
    let drop_window: gpui::AnyWindowHandle = window.into();
    let drop_files = |files: Vec<PathBuf>, cx: &mut AsyncApp| drop_window.update(cx, |_, w, cx| {
        w.dispatch_event(gpui::PlatformInput::FileDrop(gpui::FileDropEvent::Entered { position: drop_at, paths: gpui::ExternalPaths(files.into()) }), cx);
        w.dispatch_event(gpui::PlatformInput::FileDrop(gpui::FileDropEvent::Pending { position: drop_at }), cx);
        // What `FileDropEvent::Submit` sends, without the app activation it asks for: the hidden
        // check host never takes the person's focus.
        w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Left, position: drop_at, modifiers: Default::default(), click_count: 1 }), cx);
    });
    drop_files(vec![attach_file.clone()], cx)?;
    tick(cx, 150).await;
    draw(window, None, cx)?;
    let dropped = attachments_now(cx)?;
    ensure!(dropped.len() == 1 && dropped[0]["name"] == "notes for agent.txt" && dropped[0]["detail"] == "2 KB", "a file dropped on the composer at {drop_at:?} (paperclip {clip_at:?}) gave chips {dropped:?}; log {:?}", log_tail(cx)?);
    let mut png = vec![];
    image::RgbaImage::from_fn(48, 32, |x, y| image::Rgba([(x * 5) as u8, 120, (y * 7) as u8, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let paste_image = |png: &[u8], cx: &mut AsyncApp| cx.update(|app| app.write_to_clipboard(gpui::ClipboardItem { entries: vec![gpui::ClipboardEntry::Image(gpui::Image::from_bytes(gpui::ImageFormat::Png, png.to_vec()))] }));
    paste_image(&png, cx);
    native.call(json!({"op":"post_key","text":"v","code":9,"command":true}));
    tick(cx, 300).await;
    draw(window, None, cx)?;
    let pasted = attachments_now(cx)?;
    let first_image = PathBuf::from(pasted.get(1).and_then(|c| c["path"].as_str()).unwrap_or_default());
    let attachments_folder = std::env::temp_dir().join("wheel-attachments");
    cleanup.0.extend(first_image.parent().map(std::path::Path::to_owned));
    ensure!(pasted.len() == 2 && pasted[1]["image"] == true && pasted[1]["name"].as_str().is_some_and(|n| n.starts_with("Image-") && n.ends_with(".png")) && first_image.starts_with(&attachments_folder) && first_image.exists(),
        "Command-V with an image on the clipboard gave chips {pasted:?}");
    let chips_drawn = ["attachment:notes for agent.txt", "attach"].iter().all(|id| window.update(cx, |c, _, _| c.control_bounds(&agent_name, id)).ok().flatten().is_some());
    ensure!(chips_drawn, "the chips row was not drawn: {:?}", window.update(cx, |c, _, _| c.control_bounds(&agent_name, "chips"))?);
    tick(cx, 300).await;
    draw(window, None, cx)?;
    draw(window, Some(output.join("client-composer-attachments.png")), cx)?;
    let image_name = pasted[1]["name"].as_str().unwrap_or_default().to_owned();
    let remove_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("attachment:remove:{image_name}")))?.ok_or_else(|| anyhow::anyhow!("no × on the pasted image's chip"))?;
    press(&native, remove_at);
    tick(cx, 200).await;
    draw(window, None, cx)?;
    let removed_copy = !first_image.parent().is_some_and(std::path::Path::exists) && attachments_now(cx)?.len() == 1;
    ensure!(removed_copy, "× on {image_name}: folder still there {}, chips {:?}", first_image.parent().is_some_and(std::path::Path::exists), attachments_now(cx)?);
    paste_image(&png, cx);
    native.call(json!({"op":"post_key","text":"v","code":9,"command":true}));
    tick(cx, 300).await;
    let image_path = attachments_now(cx)?.get(1).and_then(|c| c["path"].as_str()).unwrap_or_default().to_owned();
    ensure!(!image_path.is_empty(), "the second paste attached nothing: {:?}", attachments_now(cx)?);
    cleanup.0.extend(std::path::Path::new(&image_path).parent().map(std::path::Path::to_owned));
    if let Some(item) = clipboard_before {
        cx.update(|app| app.write_to_clipboard(item));
    }
    let rows_before_attach = rows(cx)?.1.len();
    type_line(&native, "see attached", cx).await;
    let expected_message = format!("see attached\n\n`{}`\n`{image_path}`", attach_file.display());
    let mut attach_sent = false;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, all) = rows(cx)?;
        attach_sent = all.iter().skip(rows_before_attach).any(|r| matches!(&r.kind, Kind::User { text, .. } if *text == expected_message));
        if attach_sent && state == "ready" {
            break;
        }
    }
    ensure!(attach_sent && attachments_now(cx)?.is_empty() && draft_now(cx)?.is_empty(), "the message with attachments: sent {attach_sent}, chips {:?}, draft {:?}, rows {:?}", attachments_now(cx)?, draft_now(cx)?, &rows(cx)?.1[rows_before_attach..]);
    let read_attach = std::process::Command::new(&cli_path)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "read", &agent_pane])
        .output()?;
    let read_attach = String::from_utf8_lossy(&read_attach.stdout).to_string();
    let attachment_path_in_message = read_attach.contains(&format!("`{}`", attach_file.display())) && read_attach.contains(&format!("`{image_path}`"));
    ensure!(attachment_path_in_message, "wheel agent read lacks the backticked paths {} and {image_path}: {}", attach_file.display(), read_attach.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect::<String>());
    drop_files(vec![attach_dir.clone()], cx)?;
    tick(cx, 150).await;
    let folder_chip = attachments_now(cx)?;
    ensure!(folder_chip.len() == 1 && folder_chip[0]["detail"] == "Folder", "a dropped folder gave chips {folder_chip:?}");
    key(&native, "\r", 36);
    let alone = format!("`{}`", attach_dir.display());
    let mut alone_sent = false;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, all) = rows(cx)?;
        alone_sent = all.iter().any(|r| matches!(&r.kind, Kind::User { text, .. } if *text == alone));
        if alone_sent && state == "ready" {
            break;
        }
    }
    ensure!(alone_sent && attachments_now(cx)?.is_empty(), "Return with only a folder attached: sent {alone_sent}, chips {:?}", attachments_now(cx)?);
    drop(cleanup);
    let attachments_report = json!({"dropped": dropped[0]["name"], "dropped_detail": dropped[0]["detail"], "pasted_image": image_name, "removed_copy": removed_copy,
        "message": expected_message, "attachment_path_in_message": attachment_path_in_message, "folder_alone_sent": alone_sent});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, attachments_report))
}

/// The Conversations popover, the Recent shortcut, and a Codex pane streaming a reply.
async fn conversations(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, &'static str, usize, String, String, bool, Value, usize, bool)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let pane_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.state_json()["panes"][agent_pane.as_str()].clone());
    // The Conversations popover (wheel-ynjo.10.5.1): the pane menu's Conversations… lists the
    // fake agent's conversations for its directory, New conversation empties the transcript and
    // wheeld names a new id, and the earlier conversation's row brings its rows back.
    let convs_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.conversations_json());
    let log_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.toast_log.clone());
    let rows_before_new = rows(cx)?.1.len();
    ensure!(rows_before_new > 0, "the agent has no rows before the conversations check");
    // The header's Conversations button by a real press when the pane is wide enough to draw it,
    // else the ⋯ menu that offers it once dropped (wheel-ynjo.8.4.1).
    let agent_header = pane_now(cx)?;
    let conversations_from = if let Some(f) = agent_header["header_buttons"]["conversations"]["frame"].as_array().cloned() {
        let at = [f[0].as_f64().unwrap_or(0.) + 9., f[1].as_f64().unwrap_or(0.) + 9.];
        native.call(json!({"op":"post_mouse","kind":"down","point":at}));
        native.call(json!({"op":"post_mouse","kind":"up","point":at}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        "button"
    } else {
        let level = agent_header["header_level"].as_u64().unwrap_or(0) as u8;
        ensure!(agent_header["more_menu"].as_array().is_some_and(|m| m.iter().any(|i| i == "conversations")), "the agent pane's header at level {level} offers Conversations nowhere: {agent_header}");
        window.update(cx, |c, _, _| c.open_pane_menu(&agent_pane, 400., 100., level))?;
        ensure!(window.update(cx, |c, _, _| c.choose_header_menu("conversations", &agent_pane))?, "the ⋯ menu did not run Conversations…");
        "menu"
    };
    let mut listing = Value::Null;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        listing = convs_now(cx)?;
        if listing["listing"] != "loading" {
            break;
        }
    }
    ensure!(listing["listing"] == "loaded", "the conversations popover never listed: {listing}");
    let listed = listing["rows"].as_array().map(Vec::len).unwrap_or(0);
    let old_id = listing["rows"].as_array().and_then(|r| r.iter().find(|r| r["current"] == true)).and_then(|r| r["id"].as_str()).map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("no conversation listed as current: {listing}"))?;
    draw(window, Some(output.join("client-conversations.png")), cx)?;
    // A search for words no conversation holds lists none; Backspace brings them back.
    type_text(&native, "zq", cx).await;
    let searched = convs_now(cx)?;
    ensure!(searched["query"] == "zq" && searched["rows"].as_array().is_some_and(Vec::is_empty), "searching `zq`: {searched}");
    erase(&native, 2, cx).await;
    ensure!(convs_now(cx)?["rows"].as_array().map(Vec::len) == Some(listed), "Backspace did not bring the {listed} rows back: {}", convs_now(cx)?);
    draw(window, None, cx)?;
    let new_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "conversation:new"))?.ok_or_else(|| anyhow::anyhow!("no New conversation row drawn"))?;
    press(&native, new_at);
    let mut emptied = false;
    for _ in 0..150 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if convs_now(cx)?.is_null() && rows(cx)?.1.is_empty() {
            emptied = true;
            break;
        }
    }
    let new_id = log_now(cx)?.iter().rev().find_map(|l| l.strip_prefix(&format!("agent {agent_name} is in conversation ")).map(str::to_owned));
    ensure!(emptied && new_id.is_some(), "New conversation: popover {}, {} rows, log {:?}", convs_now(cx)?, rows(cx)?.1.len(), log_now(cx)?.iter().rev().take(6).collect::<Vec<_>>());
    let new_id = new_id.unwrap_or_default();
    let new_id_differs = new_id != old_id;
    ensure!(new_id_differs, "New conversation kept id {old_id}");
    // The empty transcript's Recent shortcut (wheel-ynjo.10.5.4): the conversation just left is
    // among at most three cards, and Show all… opens the Conversations popover.
    let recent_report = {
        let recent_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.recent_json(&agent_name));
        let mut recent = Value::Null;
        for _ in 0..100 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(window, None, cx)?;
            recent = recent_now(cx)?;
            if recent["listing"] == "loaded" {
                break;
            }
        }
        let cards = recent["cards"].as_array().cloned().unwrap_or_default();
        ensure!(recent["listing"] == "loaded" && (1..=3).contains(&cards.len()), "the empty transcript's Recent shortcut: {recent}");
        ensure!(cards.iter().any(|c| c["id"] == old_id.as_str()) && cards.iter().all(|c| c["id"] != new_id.as_str()), "Recent should list {old_id} and not the current {new_id}: {recent}");
        draw(window, Some(output.join("client-recent-conversations.png")), cx)?;
        let card_drawn = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("recent:{old_id}")))?.is_some();
        ensure!(card_drawn, "the Recent card for {old_id} was listed but not drawn: {recent}");
        let show_all = window.update(cx, |c, _, _| c.control_bounds(&agent_name, "recent:show-all"))?.ok_or_else(|| anyhow::anyhow!("no Show all… drawn: {recent}"))?;
        press(&native, show_all);
        let mut opened = Value::Null;
        for _ in 0..100 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            opened = convs_now(cx)?;
            if opened["listing"] == "loaded" {
                break;
            }
        }
        let show_all_opens = opened["pane"] == agent_pane.as_str() && opened["listing"] == "loaded";
        ensure!(show_all_opens, "Show all… did not open the Conversations popover over {agent_pane}: {opened}");
        window.update(cx, |c, _, _| c.close_conversations())?;
        json!({"cards": cards.len(), "card_drawn": card_drawn, "show_all_opens": show_all_opens, "titles": cards.iter().map(|c| c["title"].clone()).collect::<Vec<_>>(),
            "ages": cards.iter().map(|c| c["age"].clone()).collect::<Vec<_>>()})
    };
    type_line(&native, "in the new one", cx).await;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "ready" && rows(cx)?.1.len() >= 2 {
            break;
        }
    }
    ensure!(window.update(cx, |c, _, _| c.choose_header_menu("conversations", &agent_pane))?, "Conversations… is gone after the switch");
    let mut old_row = None;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        old_row = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("conversation:{old_id}")))?;
        if old_row.is_some() {
            break;
        }
    }
    let old_row = old_row.ok_or_else(|| anyhow::anyhow!("the second listing has no row for {old_id}: {}", convs_now(cx).unwrap_or_default()))?;
    let relisted = convs_now(cx)?;
    ensure!(relisted["rows"].as_array().is_some_and(|r| r.iter().any(|r| r["id"] == new_id.as_str() && r["current"] == true)), "the new conversation {new_id} is not the current row: {relisted}");
    press(&native, old_row);
    let mut resumed_rows = 0;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (_, held) = rows(cx)?;
        if convs_now(cx)?.is_null() && held.iter().any(|r| matches!(&r.kind, Kind::User { text, .. } if text == "show tools")) {
            resumed_rows = held.len();
            break;
        }
    }
    ensure!(resumed_rows > 0, "resuming {old_id}: popover {}, rows {:?}", convs_now(cx)?, rows(cx)?.1.iter().take(4).map(|r| format!("{:?}", r.kind)).collect::<Vec<_>>());
    // A Codex pane (wheel-fake-agent app-server) streams a reply to a message typed into it.
    let wheel = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args).output()?;
        ensure!(out.status.success(), "wheel {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let codex_pane = wheel(&["pane", "split", &agent_pane, "right"])?.lines().last().unwrap_or_default().to_owned();
    let codex_started: Value = serde_json::from_str(&wheel(&["pane", "start", &codex_pane, "agent", "--agent", "codex"])?)?;
    let codex_agent = codex_started["agent"].as_str().unwrap_or_default().to_owned();
    let codex_rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&codex_agent).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    for _ in 0..150 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if codex_rows(cx)?.0 == "ready" {
            break;
        }
    }
    ensure!(codex_rows(cx)?.0 == "ready", "the Codex pane {codex_pane} ({codex_agent}) never came up ready: {:?}", codex_rows(cx)?.0);
    // A click on a pane's body gives it the keyboard, as it does the agent pane above.
    let click_pane = |native: &Native, pane: &str, cx: &mut AsyncApp| -> Result<()> {
        draw(window, None, cx)?;
        let f = window.update(cx, |c, _, _| c.state_json()["panes"][pane]["frame"].clone())?;
        let f: Vec<f64> = serde_json::from_value(f.clone()).map_err(|_| anyhow::anyhow!("pane {pane} has no frame: {f}"))?;
        let at = [f[0] + f[2] / 2., f[1] + f[3] / 2.];
        native.call(json!({"op":"post_mouse","kind":"down","point":at}));
        native.call(json!({"op":"post_mouse","kind":"up","point":at}));
        Ok(())
    };
    click_pane(&native, &codex_pane, cx)?;
    tick(cx, 300).await;
    ensure!(window.update(cx, |c, _, _| c.focus.current.clone())?.as_deref() == Some(codex_pane.as_str()), "clicking the Codex pane did not focus it");
    type_line(&native, "hello codex", cx).await;
    let mut codex_pane_streams = false;
    for _ in 0..150 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if codex_rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::Reply(t) if !t.is_empty())) {
            codex_pane_streams = true;
            break;
        }
    }
    draw(window, Some(output.join("client-codex-pane.png")), cx)?;
    ensure!(codex_pane_streams, "the Codex pane streamed no reply: state {:?}, rows {:?}", codex_rows(cx)?.0, codex_rows(cx)?.1.iter().map(|r| format!("{:?}", r.kind)).collect::<Vec<_>>());
    wheel(&["pane", "close", &codex_pane])?;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.state_json()["panes"][codex_pane.as_str()].is_null())? {
            break;
        }
    }
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, conversations_from, listed, old_id, new_id, new_id_differs, recent_report, resumed_rows, codex_pane_streams))
}

/// Fork from a reply, and the reply's copy and play buttons.
async fn fork_and_reply_buttons(run: Run, agent_pane: String, agent_name: String, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, usize, String, usize, Value, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let wheel = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args).output()?;
        ensure!(out.status.success(), "wheel {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let click_pane = |native: &Native, pane: &str, cx: &mut AsyncApp| -> Result<()> {
        draw(window, None, cx)?;
        let f = window.update(cx, |c, _, _| c.state_json()["panes"][pane]["frame"].clone())?;
        let f: Vec<f64> = serde_json::from_value(f.clone()).map_err(|_| anyhow::anyhow!("pane {pane} has no frame: {f}"))?;
        let at = [f[0] + f[2] / 2., f[1] + f[3] / 2.];
        native.call(json!({"op":"post_mouse","kind":"down","point":at}));
        native.call(json!({"op":"post_mouse","kind":"up","point":at}));
        Ok(())
    };
    let log_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.toast_log.clone());
    // Fork from a reply (wheel-ynjo.10.5.2): the last reply of the resumed conversation's first
    // turn draws the branch button on a line under it; a press forks the conversation through that
    // turn into a new agent pane right of this one, and `wheel agent read` gives the fork's rows,
    // which hold that one prompt and end at that reply.
    let fork_lines: Vec<usize> = serde_json::from_value(window.update(cx, |c, _, _| c.state_json()["reply_ends"][agent_name.as_str()].clone())?).unwrap_or_default();
    let (fork_row, fork_text) = window.update(cx, |c, _, _| {
        let t = &c.transcripts[&agent_name];
        let can = c.can_fork(&agent_name);
        let ix = fork_lines.iter().copied().find(|&i| t.fork_turns(i, can).is_some());
        let text = ix.and_then(|i| match &t.rows[i].kind { Kind::Reply(x) => Some(x.clone()), _ => None });
        (ix, text.unwrap_or_default())
    })?;
    let fork_row = fork_row.ok_or_else(|| anyhow::anyhow!("no reply of {agent_name} draws a fork line: {fork_lines:?}"))?;
    let first_turn = window.update(cx, |c, _, _| c.transcripts[&agent_name].fork_turns(fork_row, c.can_fork(&agent_name)))?;
    ensure!(first_turn == Some(1), "the first fork line, row {fork_row}, forks through {first_turn:?} turns");
    window.update(cx, |c, _, _| c.reveal(&agent_name, fork_row))?;
    let mut fork_at = None;
    for _ in 0..20 {
        draw(window, None, cx)?;
        fork_at = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("fork:{fork_row}")))?;
        if fork_at.is_some() {
            break;
        }
        tick(cx, 100).await;
    }
    let fork_at = fork_at.ok_or_else(|| anyhow::anyhow!("row {fork_row} of {agent_name} drew no fork button"))?;
    // The reply's other buttons (wheel-ynjo.10.5.5): copy and play on the same line, left of the
    // fork button, as RowView.swift's `actions`. Copy puts the reply's Markdown on the clipboard;
    // play has the app's synthesizer read it, which turns the button to stop, and a second press
    // 150 ms later stops it, before the reply's first words are over.
    let reply_buttons = {
        let at = |id: &str, cx: &mut AsyncApp| -> Result<Option<(f32, f32, f32, f32)>> { Ok(window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("{id}:{fork_row}")))?) };
        let mut buttons = vec![];
        let mut lefts = vec![];
        for id in ["copy", "play", "fork"] {
            if let Some(b) = at(id, cx)? {
                buttons.push(id);
                lefts.push(b.0);
            }
        }
        ensure!(buttons == ["copy", "play", "fork"] && lefts.windows(2).all(|w| w[0] < w[1]), "the turn end {fork_row} drew buttons {buttons:?} at x {lefts:?}");
        let key = window.update(cx, |c, _, _| format!("{agent_name}#{}", c.transcripts[&agent_name].rows[fork_row].id))?;
        press(&native, at("copy", cx)?.unwrap());
        tick(cx, 150).await;
        let copied = cx.update(|app| app.read_from_clipboard().and_then(|i| i.text())).unwrap_or_default();
        ensure!(copied == fork_text, "the copy button put {copied:?} on the clipboard, not {fork_text:?}");
        let playing_now = |cx: &mut AsyncApp| -> Result<Value> { Ok(window.update(cx, |c, _, _| c.state_json()["read_aloud"].clone())?) };
        // Each press logs the bridge's answer, what the synthesizer reads after it.
        let answers = |cx: &mut AsyncApp| -> Result<Vec<String>> { Ok(log_now(cx)?.into_iter().filter(|l| l.contains(&format!("read aloud row {fork_row}: "))).collect()) };
        // The press logs its answer as it runs, so the log changing (it is capped, so never its
        // length) means the newest answer line is this press's.
        macro_rules! play_answer {
            () => {{
                let before = log_now(cx)?;
                press(&native, at("play", cx)?.unwrap());
                let mut answer = None;
                for _ in 0..100 {
                    if log_now(cx)? != before {
                        answer = answers(cx)?.last().cloned();
                        break;
                    }
                    tick(cx, 20).await;
                }
                answer.ok_or_else(|| anyhow::anyhow!("the play button got no answer from the bridge"))?
            }};
        }
        let want = json!(key).to_string();
        let heard = play_answer!();
        ensure!(heard.ends_with(&format!(": {want}")), "the play button's press answered {heard:?}, not {want}");
        // The window hears what the synthesizer reads by an event and draws the stop button; a
        // press then stops it. A reply that finished first (the event of a short one can come
        // after its end) is spoken again, and the press that restarts it is the next try.
        let (mut playing, mut stopped, mut restarts) = (Value::Null, false, 0);
        while !stopped && restarts < 8 {
            playing = Value::Null;
            let mut shown = false;
            for _ in 0..100 {
                tick(cx, 20).await;
                if playing_now(cx)? == json!(key) {
                    shown = true;
                    break;
                }
            }
            if shown {
                draw(window, Some(output.join("client-reply-buttons.png")), cx)?;
                playing = json!(key);
            }
            let answer = play_answer!();
            if answer.ends_with(": null") {
                stopped = true;
            } else {
                ensure!(answer.ends_with(&format!(": {want}")), "a press on play answered {answer:?}");
                restarts += 1;
            }
        }
        ensure!(stopped && playing == json!(key), "the play button never showed and stopped the reply: shown {playing}, {restarts} restart(s)");
        let mut cleared = Value::Null;
        for _ in 0..100 {
            cleared = playing_now(cx)?;
            if cleared.is_null() {
                break;
            }
            tick(cx, 20).await;
        }
        draw(window, None, cx)?;
        ensure!(cleared.is_null(), "a second press on play left the window reading {cleared}");
        json!({"row": fork_row, "buttons": buttons, "x": lefts, "copied": copied == fork_text, "playing": playing, "log": heard, "stopped": stopped, "restarts": restarts})
    };
    press(&native, fork_at);
    let mut forked = None;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        forked = log_now(cx)?.iter().rev().find_map(|l| l.strip_prefix("forked into ").map(str::to_owned));
        if forked.is_some() {
            break;
        }
    }
    let refused = window.update(cx, |c, _, _| c.state_json()["composer_errors"][agent_name.as_str()].clone())?;
    let forked = forked.ok_or_else(|| anyhow::anyhow!("the fork never started: refused {refused}, log {:?}", log_now(cx).unwrap_or_default().iter().rev().take(6).collect::<Vec<_>>()))?;
    let (fork_agent, fork_pane) = forked.split_once(" in pane ").map(|(a, p)| (a.to_owned(), p.to_owned())).unwrap_or_default();
    let mut fork_rows = vec![];
    for _ in 0..50 {
        let read: Value = serde_json::from_str(&wheel(&["agent", "read", &fork_agent, "--json"])?)?;
        let events: Vec<wheel_proto::StampedEvent> = serde_json::from_value(read["events"].clone())?;
        let mut t = wheel_gpui::transcript::Transcript::default();
        for e in &events {
            t.event(&e.event);
        }
        fork_rows = t.rows;
        if !fork_rows.is_empty() {
            break;
        }
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
    }
    let prompts = fork_rows.iter().filter(|r| matches!(r.kind, Kind::User { .. })).count();
    let ends_at_reply = matches!(fork_rows.iter().rev().find(|r| matches!(r.kind, Kind::Reply(_))).map(|r| &r.kind), Some(Kind::Reply(x)) if *x == fork_text);
    ensure!(prompts == 1 && ends_at_reply, "the fork {fork_agent} holds {prompts} prompts and ends {:?}, not at {fork_text:?}", fork_rows.last().map(|r| &r.kind));
    let mut fork_pane_right = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        let panes = window.update(cx, |c, _, _| c.state_json()["panes"].clone())?;
        let (a, f) = (&panes[agent_pane.as_str()]["frame"], &panes[fork_pane.as_str()]["frame"]);
        if let (Some(a), Some(f)) = (a.as_array(), f.as_array()) {
            fork_pane_right = f[0].as_f64() > a[0].as_f64();
            break;
        }
    }
    draw(window, Some(output.join("client-fork.png")), cx)?;
    ensure!(fork_pane_right, "the fork's pane {fork_pane} is not right of {agent_pane}");
    wheel(&["pane", "close", &fork_pane])?;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.state_json()["panes"][fork_pane.as_str()].is_null())? {
            break;
        }
    }
    let fork_report = json!({"row": fork_row, "lines": fork_lines, "agent": fork_agent, "pane": fork_pane, "prompts": prompts, "ends_at_reply": ends_at_reply, "right_of_source": fork_pane_right});
    let fork_rows = fork_rows.len();
    click_pane(&native, &agent_pane, cx)?;
    tick(cx, 300).await;
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, fork_row, fork_text, fork_rows, reply_buttons, fork_report))
}

/// The message menu, a 1000-message conversation scrolling within a frame, and the way
/// back to the terminal.
async fn message_menu_and_scroll(run: Run, agent_pane: String, agent_name: String, cli_path: String, old_id: String, splits_above: Vec<wheel_gpui::layout::SplitStep>, click: [f64; 2], fork_row: usize, fork_text: String, cx: &mut AsyncApp) -> Result<(Run, Value, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let rows = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.transcripts.get(&agent_name).map(|t| (t.state.clone(), t.rows.clone())).unwrap_or_default());
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
    let set_ratio = |split: &str, ratio: f64| -> Result<()> {
        let out = std::process::Command::new(&cli_path)
            .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", split, &ratio.to_string()]).output()?;
        ensure!(out.status.success(), "pane ratio {split} {ratio}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Ok(())
    };
    let pane_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.state_json()["panes"][agent_pane.as_str()].clone());
    let draft_now = |cx: &mut AsyncApp| -> Result<String> { Ok(pane_now(cx)?["draft"].as_str().unwrap_or_default().to_owned()) };
    let key = |native: &Native, text: &str, code: u16| {
        native.call(json!({"op":"post_key","text":text,"code":code}));
    };
    let wheel = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy()]).args(args).output()?;
        ensure!(out.status.success(), "wheel {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    // The message menu (wheel-ynjo.10.5.3): a control-click on the reply the fork came from opens
    // its three items; Copy message as Markdown puts the reply's source on the clipboard, Copy
    // message the text as drawn, and Attach message as context quotes it at the top of the draft
    // and gives the composer the keyboard. A right press on the person's message opens it too, and
    // a press on a tool row opens none.
    let menu_window: gpui::AnyWindowHandle = window.into();
    let message_at = |row: usize, cx: &mut AsyncApp| -> Result<Option<(f32, f32, f32, f32)>> { Ok(window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("message:{row}")))?) };
    let menu_now = |cx: &mut AsyncApp| -> Result<Value> { Ok(window.update(cx, |c, _, _| c.state_json()["message_menu"].clone())?) };
    let open_on = |row: usize, cx: &mut AsyncApp| -> Result<(f32, f32, f32, f32)> {
        window.update(cx, |c, _, _| c.reveal(&agent_name, row))?;
        for _ in 0..20 {
            draw(window, None, cx)?;
            if let Some(b) = message_at(row, cx)? {
                return Ok(b);
            }
        }
        Err(anyhow::anyhow!("row {row} of {agent_name} recorded no message bounds"))
    };
    let mut message_menu_report = json!({});
    let mut menu_items = Value::Null;
    for (action, then) in [("markdown", "copied_markdown"), ("copy", "copied_plain"), ("attach", "attached")] {
        let b = open_on(fork_row, cx)?;
        let p = [f64::from(b.0 + 12.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p,"control":true}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p,"control":true}));
        tick(cx, 150).await;
        draw(window, None, cx)?;
        let menu = menu_now(cx)?;
        ensure!(menu["open"]["row"] == json!(fork_row) && menu["open"]["agent"] == json!(agent_name), "a control-click on reply {fork_row} at {p:?} left the message menu {menu} (presses {:?})", window.update(cx, |c, _, _| c.presses.clone())?);
        if menu_items.is_null() {
            menu_items = menu["items"].clone();
            draw(window, Some(output.join("client-message-menu.png")), cx)?;
        }
        let item = window.update(cx, |c, _, _| c.control_bounds(&agent_name, &format!("message-menu:{action}")))?.ok_or_else(|| anyhow::anyhow!("the message menu drew no {action} item"))?;
        press(&native, item);
        tick(cx, 150).await;
        draw(window, None, cx)?;
        ensure!(menu_now(cx)?["open"].is_null(), "the message menu stayed open after {action}");
        let clipboard = cx.update(|app| app.read_from_clipboard().and_then(|i| i.text())).unwrap_or_default();
        match action {
            "markdown" => {
                ensure!(clipboard == fork_text, "Copy message as Markdown put {clipboard:?} on the clipboard, not {fork_text:?}");
                message_menu_report[then] = json!(true);
            }
            "copy" => {
                ensure!(!clipboard.is_empty() && fork_text.contains(clipboard.lines().next().unwrap_or_default().trim()), "Copy message put {clipboard:?} on the clipboard for {fork_text:?}");
                message_menu_report[then] = json!(clipboard);
            }
            _ => {
                let draft = draft_now(cx)?;
                let quote_first = fork_text.lines().next().unwrap_or_default();
                let focused = window.update(cx, |c, _, _| c.state_json()["focus_key"].clone())?;
                ensure!(draft.starts_with(&format!("> {quote_first}")) && draft.ends_with("\n\n"), "after Attach message as context the draft is {draft:?}");
                ensure!(focused == json!(agent_pane), "after Attach message as context the focus is {focused}, not {agent_pane}");
                message_menu_report["attached_draft_starts_with_quote"] = json!(true);
                message_menu_report["draft"] = json!(draft);
                message_menu_report["focus_after_attach"] = focused;
                draw(window, Some(output.join("client-message-attached.png")), cx)?;
                erase(&native, draft.chars().count(), cx).await;
                ensure!(draft_now(cx)?.is_empty(), "the attached quote did not erase: {:?}", draft_now(cx)?);
            }
        }
    }
    message_menu_report["copied_markdown"] = json!(message_menu_report["copied_markdown"] == true);
    message_menu_report["items"] = json!(menu_items.as_array().map(|a| a.iter().filter(|t| *t != "-").cloned().collect::<Vec<_>>()).unwrap_or_default());
    message_menu_report["lines"] = menu_items;
    // A right press on the person's message, then a press away closes it.
    let user_row = window.update(cx, |c, _, _| c.transcripts[&agent_name].rows.iter().take(fork_row).rposition(|r| matches!(r.kind, Kind::User { .. })))?
        .ok_or_else(|| anyhow::anyhow!("no message of the person's before reply {fork_row}"))?;
    let b = open_on(user_row, cx)?;
    let at = gpui::point(gpui::px(b.0 + 12.), gpui::px(b.1 + b.3 / 2.));
    menu_window.update(cx, |_, w, cx| {
        w.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Right, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false }), cx);
        w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Right, position: at, modifiers: Default::default(), click_count: 1 }), cx);
    })?;
    tick(cx, 100).await;
    draw(window, None, cx)?;
    let on_user = menu_now(cx)?;
    ensure!(on_user["open"]["row"] == json!(user_row), "a right press on the person's message {user_row} left the menu {on_user}");
    message_menu_report["right_press_on_user_row"] = json!(user_row);
    key(&native, "\u{1b}", 53);
    tick(cx, 100).await;
    draw(window, None, cx)?;
    ensure!(menu_now(cx)?["open"].is_null(), "Escape left the message menu open: {}", menu_now(cx)?);
    let tool_row = window.update(cx, |c, _, _| c.transcripts[&agent_name].rows.iter().position(|r| matches!(r.kind, Kind::Tool { .. })))?;
    if let Some(row) = tool_row {
        let opened = window.update(cx, |c, _, _| c.open_message_menu(&agent_name, row, 300., 300.))?;
        ensure!(!opened && menu_now(cx)?["open"].is_null(), "tool row {row} opened a message menu");
        message_menu_report["tool_row_opens_none"] = json!(row);
    }
    // A 1000-message conversation scrolls without dropping frames (wheel-ynjo.10.6): the check
    // writes a Claude Code transcript of 500 turns beside the one the agent is in, resumes it,
    // scrolls the column from its end to its top and back with wheel events, and times each frame
    // (the event and the whole draw). The column follows the stream at its end and stops while
    // scrolled up.
    // The fixture gives the stand-in Claude its own home (`agent.claude.home`); its store is there.
    let claude_home = PathBuf::from(wheel(&["config", "get", "agent.claude.home"])?);
    ensure!(claude_home.is_absolute(), "agent.claude.home is unset: {claude_home:?}");
    let store = std::fs::read_dir(claude_home.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|dir| dir.join(format!("{old_id}.jsonl")).exists())
        .ok_or_else(|| anyhow::anyhow!("no store under {claude_home:?} holds {old_id}.jsonl"))?;
    let seed_id = format!("00000000-0000-4000-8000-{:012x}", now_ms() & 0xffff_ffff_ffff);
    let seed_path = store.join(format!("{seed_id}.jsonl"));
    let mut seed = String::new();
    for i in 0..500 {
        let reply = match i % 4 {
            0 => format!("Turn {i} reads the **parser** and the `lexer` together.\n\nThe second paragraph says what pass {i} changed, long enough to wrap across the column at its usual width, so the row takes two or three lines of the pane."),
            1 => format!("Three things in turn {i}:\n\n- the first item\n- the second, with `code`\n- the third"),
            2 => format!("Turn {i} runs:\n\n```rust\nfn main() {{\n    println!(\"{i}\");\n}}\n```"),
            _ => format!("## Turn {i}\n\n| file | lines |\n| --- | --- |\n| a.rs | {i} |\n| b.rs | 4 |"),
        };
        for entry in [
            json!({"type": "user", "sessionId": seed_id, "message": {"role": "user", "content": format!("Question {i}: what does pass {i} change?")}}),
            json!({"type": "assistant", "sessionId": seed_id, "message": {"role": "assistant", "content": [{"type": "text", "text": reply}]}}),
            json!({"type": "system", "subtype": "turn_duration", "sessionId": seed_id}),
        ] {
            seed.push_str(&entry.to_string());
            seed.push('\n');
        }
    }
    // The seeded transcript sits in the Claude Code store the fake agent reads, so it goes on every
    // way out of this section, a failed step included.
    struct RemovedOnDrop(PathBuf);
    impl Drop for RemovedOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    std::fs::write(&seed_path, seed)?;
    let seed_file = RemovedOnDrop(seed_path);
    window.update(cx, |c, _, _| c.switch_conversation(&agent_name, Some(&seed_id)))?;
    let mut seeded = 0;
    for _ in 0..300 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        let (state, held) = rows(cx)?;
        if state == "ready" && held.len() >= 1000 {
            seeded = held.len();
            break;
        }
    }
    ensure!(seeded >= 1000, "resuming the seeded {seed_id}: {} rows, state {:?}", rows(cx)?.1.len(), rows(cx)?.0);
    draw(window, None, cx)?;
    let pane_frame: Vec<f64> = serde_json::from_value(window.update(cx, |c, _, _| c.state_json()["panes"][agent_pane.as_str()]["frame"].clone())?)?;
    let wheel_at = gpui::point(gpui::px((pane_frame[0] + pane_frame[2] / 2.) as f32), gpui::px((pane_frame[1] + pane_frame[3] * 0.3) as f32));
    let column_now = |cx: &mut AsyncApp| window.update(cx, |c, _, _| c.column_scroll(&agent_name));
    // One wheel step and the frame it causes, timed together in milliseconds: the main thread's
    // CPU time, which a busy machine's preemption leaves alone, and the wall time beside it.
    let step = |dy: f32, cx: &mut AsyncApp| -> Result<(f64, f64)> {
        let any: gpui::AnyWindowHandle = window.into();
        let (started, cpu_started) = (std::time::Instant::now(), thread_cpu_ms());
        any.update(cx, |_, w, cx| {
            w.dispatch_event(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent { position: wheel_at, delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))), modifiers: Default::default(), touch_phase: gpui::TouchPhase::Moved }), cx);
            w.draw(cx).clear();
        })?;
        Ok((thread_cpu_ms() - cpu_started, started.elapsed().as_secs_f64() * 1000.))
    };
    let at_end = column_now(cx)?;
    ensure!(at_end.is_some_and(|(top, _, following)| top == seeded && following), "the resumed column is not at its end and following: {at_end:?} of {seeded} rows");
    let frames_before = window.update(cx, |c, _, _| c.frames)?;
    let (mut times, mut walls) = (vec![], vec![]);
    let mut tops = std::collections::HashSet::new();
    let mut reached_top = false;
    for _ in 0..4000 {
        let (cpu, wall) = step(120., cx)?;
        times.push(cpu);
        walls.push(wall);
        let now = column_now(cx)?.unwrap_or_default();
        tops.insert(now.0);
        if now.0 == 0 && now.1 == 0. {
            reached_top = true;
            break;
        }
    }
    ensure!(reached_top, "4000 wheel steps up never reached the top: {:?}", column_now(cx)?);
    let first_drawn = window.update(cx, |c, _, _| c.drawn_texts(&agent_name).iter().any(|d| d.row == 0 && d.text.starts_with("Question 0")))?;
    ensure!(first_drawn, "at the top the first row is not drawn: {:?}", window.update(cx, |c, _, _| c.drawn_texts(&agent_name).iter().take(3).map(|d| (d.row, d.text.clone())).collect::<Vec<_>>())?);
    let drawn_at_top = window.update(cx, |c, _, _| c.drawn_texts(&agent_name).iter().map(|d| d.row).collect::<std::collections::BTreeSet<_>>().len())?;
    draw(window, Some(output.join("client-transcript-top.png")), cx)?;
    // Scrolled up, a reply to a message sent from elsewhere (the CLI) leaves the column where it
    // is; a message the person sends from the composer takes it back to the end instead.
    let sent_up = std::process::Command::new(&cli_path)
        .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "agent", "send", &agent_pane, "while scrolled up", "--timeout", "30"])
        .output()?;
    ensure!(sent_up.status.success(), "agent send while scrolled up failed: {}{}", String::from_utf8_lossy(&sent_up.stdout), String::from_utf8_lossy(&sent_up.stderr));
    let mut answered_up = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        let (state, held) = rows(cx)?;
        if state == "ready" && held.iter().any(|r| matches!(&r.kind, Kind::Reply(t) if t.contains("while scrolled up"))) {
            answered_up = true;
            break;
        }
    }
    let stayed = column_now(cx)?;
    ensure!(answered_up && stayed.is_some_and(|(top, offset, following)| top == 0 && offset == 0. && !following), "a reply while scrolled to the top moved the column: answered {answered_up}, column {stayed:?}");
    let total = rows(cx)?.1.len();
    let mut reached_end = false;
    for _ in 0..4000 {
        let (cpu, wall) = step(-120., cx)?;
        times.push(cpu);
        walls.push(wall);
        if column_now(cx)?.is_some_and(|(top, _, following)| top == total && following) {
            reached_end = true;
            break;
        }
    }
    ensure!(reached_end, "4000 wheel steps down never reached the end and followed again: {:?} of {total}", column_now(cx)?);
    let frames = window.update(cx, |c, _, _| c.frames)? - frames_before;
    // Back at the end, the next reply is followed.
    type_line(&native, "back at the end", cx).await;
    let mut followed = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        let (state, held) = rows(cx)?;
        if state == "ready" && held.iter().any(|r| matches!(&r.kind, Kind::Reply(t) if t.contains("back at the end"))) {
            let now = column_now(cx)?;
            followed = now.is_some_and(|(top, _, following)| top == held.len() && following);
            break;
        }
    }
    ensure!(followed, "the reply at the end was not followed: {:?} of {}", column_now(cx)?, rows(cx)?.1.len());
    draw(window, Some(output.join("client-transcript-end.png")), cx)?;
    let worst = times.iter().copied().fold(0., f64::max);
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let dropped = times.iter().filter(|t| **t > 1000. / 60.).count();
    let worst_wall = walls.iter().copied().fold(0., f64::max);
    let mut walls_sorted = walls.clone();
    walls_sorted.sort_by(f64::total_cmp);
    let median_wall = walls_sorted[walls_sorted.len() / 2];
    let scroll_report = json!({"rows": seeded, "steps": times.len(), "frames": frames, "worst_frame_ms": (worst * 100.).round() / 100., "median_frame_ms": (median * 100.).round() / 100., "worst_wall_ms": (worst_wall * 100.).round() / 100., "median_wall_ms": (median_wall * 100.).round() / 100., "dropped": dropped, "rows_passed": tops.len(), "rows_drawn_at_top": drawn_at_top, "stayed_while_scrolled_up": true, "followed_at_end": followed});
    ensure!(dropped == 0 && worst <= 1000. / 60., "scrolling 1000 rows took over a frame's CPU time: {scroll_report}");
    // A step that waits without working (a blocking read, a lock) costs no CPU time; the median
    // wall time catches one that every step pays, while a busy machine's stray preemptions do not.
    ensure!(median_wall <= 1000. / 60., "scrolling 1000 rows took over a frame of wall time at the median: {scroll_report}");
    drop(seed_file);
    // Back to the conversation the later sections know.
    window.update(cx, |c, _, _| c.switch_conversation(&agent_name, Some(&old_id)))?;
    for _ in 0..200 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if rows(cx)?.0 == "ready" && rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::User { text, .. } if text == "show tools")) {
            break;
        }
    }
    ensure!(rows(cx)?.1.iter().any(|r| matches!(&r.kind, Kind::User { text, .. } if text == "show tools")), "switching back to {old_id} after the scroll check");
    for s in &splits_above {
        set_ratio(&s.id, s.ratio)?;
    }
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| c.splits_to(&agent_pane))? == splits_above {
            break;
        }
    }
    draw(window, None, cx)?;
    // Back to the terminal, so the focus checks below start where they did.
    native.call(json!({"op":"post_mouse","kind":"down","point":click}));
    native.call(json!({"op":"post_mouse","kind":"up","point":click}));
    tick(cx, 300).await;
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, message_menu_report, scroll_report))
}

/// The spaces sidebar: rows, the context menu, rename, reorder, a pane moved by its header,
/// the hover preview, close and the toggle.
async fn sidebar(run: Run, click: [f64; 2], cx: &mut AsyncApp) -> Result<(Run, String, String, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    // The row goes when wheeld's layout without sb5 arrives.
    let mut sb5_gone = false;
    for _ in 0..100 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if !session_order(cx)?.iter().any(|(id, _)| id == &sb5) {
            sb5_gone = true;
            break;
        }
    }
    ensure!(window.update(cx, |c, _, _| c.confirmation.is_none())? && sb5_gone, "sb5 is still listed 10 s after its close button");
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, first_session_id, fixture_space, sidebar_report))
}

/// The title row and the bottom row: commands, the mode menu, the session menu, tabs, the
/// strip and the Usage window.
async fn title_and_bottom_rows(run: Run, first_session_id: String, fixture_space: String, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, title_report))
}

/// A second window with its own connection, restored windows, and a space dragged between
/// windows and onto the desktop.
async fn second_window_and_space_drags(run: Run, saved_file: PathBuf, native_panes: Vec<(String, String)>, webs: Vec<&String>, cx: &mut AsyncApp) -> Result<(Run, Value, Option<String>, Option<String>, Vec<(String, Option<String>, Option<String>, Option<String>)>, bool, Value, gpui::WindowHandle<Client>, String, std::rc::Rc<Native>, String, String, Vec<f32>, Vec<f32>, String, PathBuf, [f64; 2], Value, Value, (Vec<String>, Vec<String>), (f32, f32), String, Option<String>, Vec<f32>, Value, (f32, f32), (f32, f32), Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let row_frame = |state: &Value, id: &str| -> Option<[f64; 4]> {
        state["rows"].as_array()?.iter().find(|r| r["id"] == id).map(|r| [r["frame"][0].as_f64().unwrap(), r["frame"][1].as_f64().unwrap(), r["frame"][2].as_f64().unwrap(), r["frame"][3].as_f64().unwrap()])
    };
    let centre = |f: [f64; 4]| [f[0] + f[2] / 2., f[1] + f[3] / 2.];
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
    // A drag's chip and card are app windows too; only the main windows count.
    let before_windows = cx.update(|app| app.windows().into_iter().filter(|w| w.downcast::<Client>().is_some()).count());
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
    // A space dragged between windows (wheel-ynjo.7.5.2; SpaceDrag.swift, SpaceDropping.swift):
    // window 2 moves beside window 1, a new space in window 1's session is dragged by a real
    // press onto window 2's sidebar, then from there onto the desktop, then onto a popover card.
    let frame_of = |n: &Native| -> Vec<f32> { n.call(json!({"op":"frame"}))["frame"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default() };
    let f1 = frame_of(&native);
    native2.call(json!({"op":"move_window","point":[f64::from(f1[0] + f1[2] + 200.), f64::from(f1[1])]}));
    let mut f2 = vec![];
    for _ in 0..20 {
        tick(cx, 100).await;
        f2 = frame_of(&native2);
        if f2.len() == 4 && f2[0] >= f1[0] + f1[2] {
            break;
        }
    }
    ensure!(f1.len() == 4 && f2.len() == 4 && f2[0] >= f1[0] + f1[2] + 150., "window 2 did not move beside window 1: {f1:?} {f2:?}");
    // Windows restored at launch (wheel-ynjo.7.5.3; AppModel.openLaunchWindows, WindowManager's
    // saved frames): the file names both windows with their sessions and frames; opening what it
    // saved, as launch does, opens two windows there; with the setting off, one.
    // Launch drops a frame whose centre is on no screen, and beside window 1 window 2 may be off
    // this machine's screen: for the restore it moves to a place on screen, then back.
    native2.call(json!({"op":"move_window","point":[f64::from(f1[0] + 160.), f64::from(f1[1] + 90.)]}));
    let mut f2_on = vec![];
    for _ in 0..20 {
        tick(cx, 100).await;
        f2_on = frame_of(&native2);
        if f2_on.len() == 4 && (f2_on[0] - (f1[0] + 160.)).abs() <= 1. {
            break;
        }
    }
    ensure!(f2_on.len() == 4 && (f2_on[0] - (f1[0] + 160.)).abs() <= 1., "window 2 did not move onto the screen for the restore: {f2_on:?}");
    let restore = restore_check(&saved_file, (&f1, &f2_on), (&first_session, &second_session), (window, second), first_space.as_deref(), cx).await?;
    native2.call(json!({"op":"move_window","point":[f64::from(f2[0]), f64::from(f2[1])]}));
    for _ in 0..20 {
        tick(cx, 100).await;
        if frame_of(&native2).first().is_some_and(|x| (x - f2[0]).abs() <= 1.) {
            break;
        }
    }
    ensure!(frame_of(&native2).first().is_some_and(|x| (x - f2[0]).abs() <= 1.), "window 2 did not move back beside window 1: {:?}", frame_of(&native2));
    let drag_dir = std::env::temp_dir().join(format!("wheel-gpui-space-drag-{}", std::process::id()));
    std::fs::create_dir_all(&drag_dir)?;
    let (ok, out, err) = run_pumped(&["space", "new", &drag_dir.to_string_lossy(), "--name", "dragged"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space new for the drag failed: {err}");
    let dragged = out.trim().to_owned();
    let (ok, out, err) = run_pumped(&["space", "move", &dragged, &id1], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space move to window 1 failed: {err} {out}");
    let spaces_of = |c: &Client, session: &str| -> Vec<String> { c.layout_lists().0.into_iter().find(|s| s.id == session).map(|s| s.spaces).unwrap_or_default() };
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        draw(second, None, cx)?;
        if window.update(cx, |c, _, _| c.sidebar_json()["rows"].as_array().is_some_and(|r| r.iter().any(|r| r["id"] == dragged)))? {
            break;
        }
    }
    let rows1 = window.update(cx, |c, _, _| c.sidebar_json())?;
    let from = centre(row_frame(&rows1, &dragged).ok_or_else(|| anyhow::anyhow!("window 1's sidebar has no row for {dragged}: {}", rows1["rows"]))?);
    let rows2 = second.update(cx, |c, _, _| c.sidebar_json())?;
    let top2 = rows2["rows"][0]["frame"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect::<Vec<_>>()).unwrap_or_default();
    ensure!(top2.len() == 4, "window 2's sidebar has no rows: {}", rows2["rows"]);
    // Window 2's first row's top edge, on screen, then in window 1's own coordinates.
    let aim = (f2[0] + top2[0] + 20., f2[1] + top2[1] + 2.);
    let local1 = [f64::from(aim.0 - f1[0]), f64::from(aim.1 - f1[1])];
    native.call(json!({"op":"post_mouse","kind":"down","point":from}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":[from[0], from[1] - 10.]}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":local1}));
    tick(cx, 150).await;
    draw(window, None, cx)?;
    draw(second, Some(output.join("client-space-drag-window.png")), cx)?;
    let over_window = window.update(cx, |c, _, _| c.sidebar_json()["drag"].clone())?;
    let window2_sees = second.update(cx, |c, _, _| c.sidebar_json())?["hovered"].clone();
    let window1_chip = window.update(cx, |c, _, _| c.sidebar_json()["hovered"]["chip"].clone())?;
    ensure!(over_window["target"] == json!(format!("sidebar of {id2} at 0")) && over_window["hint"].as_str().is_some_and(|h| h.starts_with("Place “dragged” here in “"))
        && over_window["measured"]["windows"].as_array().map(Vec::len) == Some(2),
        "the drag from window 1 did not aim at window 2's sidebar: {over_window}; window 1 at {f1:?}, window 2 at {f2:?}, aimed at {aim:?}");
    ensure!(window2_sees["insertion"] == 0 && window2_sees["chip"].is_array() && window1_chip.is_null(), "window 2 does not draw the bar and the chip: it sees {window2_sees}, window 1's chip {window1_chip}");
    // The chip is a window of its own beside the pointer (DragChipWindow), kept on the pointer's
    // screen, and the window under the pointer draws none of its own; its frame is measured
    // through the bridge, since a hidden check orders no window front.
    let floats = |cx: &mut AsyncApp| cx.update(|_| wheel_gpui::view::float::state());
    let rect4 = |v: &Value| -> Vec<f32> { v.as_array().map(|a| a.iter().map(|n| n.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default() };
    let near = |a: &[f32], b: &[f32]| a.len() == 4 && b.len() == 4 && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1.);
    let mut chip_over_window = Value::Null;
    for _ in 0..20 {
        tick(cx, 100).await;
        chip_over_window = floats(cx)["chip"].clone();
        if chip_over_window["shown"] == true && near(&rect4(&chip_over_window["frame"]), &rect4(&chip_over_window["asked"])) {
            break;
        }
    }
    let chip_asked = rect4(&chip_over_window["asked"]);
    let aim_screen = rect4(&native.call(json!({"op":"screen_at","point":[aim.0, aim.1]}))["frame"]);
    ensure!(chip_asked.len() == 4 && aim_screen.len() == 4, "no chip window over window 2's sidebar: {chip_over_window}, screen {aim_screen:?}");
    let chip_expect = wheel_gpui::sidebar::chip_origin(aim.0, aim.1, (chip_asked[2], chip_asked[3]), wheel_gpui::layout::Rect { x: aim_screen[0], y: aim_screen[1], w: aim_screen[2], h: aim_screen[3] });
    ensure!(chip_over_window["shown"] == true && chip_over_window["text"] == "dragged" && (chip_asked[0] - chip_expect.0).abs() <= 1. && (chip_asked[1] - chip_expect.1).abs() <= 1.
        && near(&rect4(&chip_over_window["frame"]), &chip_asked) && window2_sees["floating"] == true && floats(cx)["card"]["shown"] != true,
        "the chip window does not follow the pointer at {aim:?}: {chip_over_window}, expected its top left at {chip_expect:?} on the screen {aim_screen:?}; window 2 sees {window2_sees}");
    native.call(json!({"op":"post_mouse","kind":"up","point":local1}));
    let mut moved_over = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(second, None, cx)?;
        // Both windows hear wheeld's layout, each on its own connection.
        if second.update(cx, |c, _, _| spaces_of(c, &second_session).first() == Some(&dragged) && c.space_id() == Some(dragged.as_str()))?
            && window.update(cx, |c, _, _| spaces_of(c, &second_session).first() == Some(&dragged))?
        {
            moved_over = true;
            break;
        }
    }
    let after_window = window.update(cx, |c, _, _| (spaces_of(c, &second_session), spaces_of(c, first_session.as_deref().unwrap_or_default())))?;
    let drag_lines = |h: gpui::WindowHandle<Client>, cx: &mut AsyncApp| -> Result<Vec<String>> { Ok(h.update(cx, |c, _, _| c.toast_log.iter().filter(|l| l.contains("space drag") || l.contains("little window") || l.contains("layout")).rev().take(12).cloned().collect())?) };
    ensure!(moved_over && !after_window.1.contains(&dragged), "the drop on window 2's sidebar left {dragged} in {after_window:?}; window 2 shows {:?} and its layout holds {:?}; window 1 logged {:?}; window 2 logged {:?}",
        shown(second, cx)?, second.update(cx, |c, _, _| (spaces_of(c, &second_session), spaces_of(c, first_session.as_deref().unwrap_or_default())))?, drag_lines(window, cx)?, drag_lines(second, cx)?);
    ensure!(window.update(cx, |c, _, _| c.sidebar_json()["hovered"].is_null())? && second.update(cx, |c, _, _| c.sidebar_json())?["hovered"].is_null(), "the drag stayed drawn after the drop");
    // From window 2 onto the desktop between the windows: a little window opens on the space,
    // centred on the drop.
    let rows2 = second.update(cx, |c, _, _| c.sidebar_json())?;
    let from2 = centre(row_frame(&rows2, &dragged).ok_or_else(|| anyhow::anyhow!("window 2's sidebar has no row for {dragged}"))?);
    let desk = (f1[0] + f1[2] + 100., f2[1] + 300.);
    let local2 = [f64::from(desk.0 - f2[0]), f64::from(desk.1 - f2[1])];
    native2.call(json!({"op":"post_mouse","kind":"down","point":from2}));
    native2.call(json!({"op":"post_mouse","kind":"drag","point":[from2[0], from2[1] + 10.]}));
    tick(cx, 100).await;
    draw(second, None, cx)?;
    native2.call(json!({"op":"post_mouse","kind":"drag","point":local2}));
    tick(cx, 150).await;
    draw(second, None, cx)?;
    let over_desk = second.update(cx, |c, _, _| c.sidebar_json())?["drag"].clone();
    ensure!(over_desk["target"] == "the desktop" && over_desk["hint"] == "Release to open “dragged” in a little window" && over_desk["screen"] == json!([desk.0, desk.1]),
        "the drag from window 2 did not aim at the desktop at {desk:?}: {over_desk}");
    // Over the desktop the preview card replaces the chip, centred on the pointer (DesktopDropOverlay).
    let card_expect = wheel_gpui::sidebar::card_frame(desk.0, desk.1);
    let card_expect = [card_expect.x, card_expect.y, card_expect.w, card_expect.h];
    let mut over_desk_floats = Value::Null;
    for _ in 0..20 {
        tick(cx, 100).await;
        over_desk_floats = floats(cx);
        if over_desk_floats["card"]["shown"] == true && near(&rect4(&over_desk_floats["card"]["frame"]), &card_expect) {
            break;
        }
    }
    ensure!(over_desk_floats["card"]["shown"] == true && over_desk_floats["card"]["text"] == "dragged" && near(&rect4(&over_desk_floats["card"]["asked"]), &card_expect)
        && near(&rect4(&over_desk_floats["card"]["frame"]), &card_expect) && over_desk_floats["chip"]["shown"] == false,
        "over the desktop at {desk:?} the card is not at {card_expect:?} with the chip hidden: {over_desk_floats}");
    if let Some(card_window) = cx.update(|_| wheel_gpui::view::float::card_window()) {
        let _ = card_window.update(cx, |_, w, cx| -> Result<()> {
            w.draw(cx).clear();
            w.render_to_image()?.save(output.join("client-space-drag-desk-card.png"))?;
            Ok(())
        });
    }
    native2.call(json!({"op":"post_mouse","kind":"up","point":local2}));
    let mut little = None;
    for _ in 0..30 {
        tick(cx, 100).await;
        little = cx.update(|_| wheel_gpui::windows::with(|r| r.little_showing(&dragged).map(|l| (l.id.clone(), l.owner.clone()))));
        if little.is_some() {
            break;
        }
    }
    let (little_id, little_owner) = little.ok_or_else(|| anyhow::anyhow!("the drop on the desktop opened no little window for {dragged}"))?;
    let after_drop_floats = floats(cx);
    ensure!(after_drop_floats["card"]["shown"] == false && after_drop_floats["chip"]["shown"] == false, "the card or the chip stayed up after the drop: {after_drop_floats}");
    let little_handle = cx.update(|_| wheel_gpui::window_handle(&little_id)).ok_or_else(|| anyhow::anyhow!("no handle for little window {little_id}"))?;
    let little_native = little_handle.update(cx, |c, _, _| c.native())?;
    let expect = (desk.0 - 240., desk.1 - 165.);
    let mut little_frame = vec![];
    for _ in 0..20 {
        draw(little_handle, None, cx)?;
        tick(cx, 100).await;
        little_frame = frame_of(&little_native);
        if little_frame.len() == 4 && (little_frame[0] - expect.0).abs() < 2. && (little_frame[1] - expect.1).abs() < 2. {
            break;
        }
    }
    ensure!(little_frame.len() == 4 && (little_frame[0] - expect.0).abs() < 2. && (little_frame[1] - expect.1).abs() < 2. && little_owner.as_deref() == Some(id2.as_str()),
        "the little window is at {little_frame:?}, owned by {little_owner:?}; expected its top left at {expect:?}, owned by {id2}");
    draw(little_handle, Some(output.join("client-space-drag-little.png")), cx)?;
    little_handle.update(cx, |c, _, _| c.little_back())?;
    for _ in 0..30 {
        draw(little_handle, None, cx).ok();
        tick(cx, 100).await;
        if cx.update(|_| wheel_gpui::window_handle(&little_id)).is_none() {
            break;
        }
    }
    ensure!(cx.update(|_| wheel_gpui::window_handle(&little_id)).is_none(), "the little window {little_id} did not close");
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, restore, first_session, first_space, both, distinct, hidden_native, second, second_session, native2, id1, id2, f1, f2, dragged, drag_dir, from2, over_window, window2_sees, after_window, desk, little_id, little_owner, little_frame, over_desk_floats, aim, chip_expect, chip_over_window))
}

/// A space dragged onto the menu bar popover's session card and between title-row tabs.
async fn space_drag_to_popover_and_tabs(run: Run, dragged: String, f1: Vec<f32>, f2: Vec<f32>, first_session: Option<String>, from2: [f64; 2], id2: String, native2: std::rc::Rc<Native>, second: gpui::WindowHandle<Client>, second_session: String, cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::sidebar::DropCard, Value, Vec<String>, Value, Value, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let spaces_of = |c: &Client, session: &str| -> Vec<String> { c.layout_lists().0.into_iter().find(|s| s.id == session).map(|s| s.spaces).unwrap_or_default() };
    let drag_lines = |h: gpui::WindowHandle<Client>, cx: &mut AsyncApp| -> Result<Vec<String>> { Ok(h.update(cx, |c, _, _| c.toast_log.iter().filter(|l| l.contains("space drag") || l.contains("little window") || l.contains("layout")).rev().take(12).cloned().collect())?) };
    // From window 2 onto the first session's card in the menu bar popover, which is in front of
    // window 1: the space goes first in that session.
    let menu = app_menu_bar(&bridge, cx)?;
    cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
    let mut popover = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        popover = menu.update(cx, |m, _| m.popover());
        if popover.is_some() {
            break;
        }
    }
    let popover_any: gpui::AnyWindowHandle = popover.ok_or_else(|| anyhow::anyhow!("the popover did not open for the card drop"))?.into();
    let first_card = first_session.clone().unwrap_or_default();
    let mut cards = None;
    for _ in 0..30 {
        let _ = popover_any.update(cx, |_, w, cx| w.draw(cx).clear());
        tick(cx, 100).await;
        cards = cx.update(|cx| menu.read(cx).drop_popover(cx)).filter(|p| p.cards.iter().any(|c| c.session == first_card && !c.rows.is_empty()));
        if cards.is_some() {
            break;
        }
    }
    let cards = cards.ok_or_else(|| anyhow::anyhow!("the popover never measured the first session's card"))?;
    let card = cards.cards.iter().find(|c| c.session == first_card).unwrap().clone();
    let on_card = (card.rows[0].x + 20., card.rows[0].y + 2.);
    let local_card = [f64::from(on_card.0 - f2[0]), f64::from(on_card.1 - f2[1])];
    native2.call(json!({"op":"post_mouse","kind":"down","point":from2}));
    native2.call(json!({"op":"post_mouse","kind":"drag","point":[from2[0], from2[1] + 10.]}));
    tick(cx, 100).await;
    draw(second, None, cx)?;
    native2.call(json!({"op":"post_mouse","kind":"drag","point":local_card}));
    tick(cx, 150).await;
    draw(second, None, cx)?;
    let _ = popover_any.update(cx, |_, w, cx| -> Result<()> {
        w.draw(cx).clear();
        w.render_to_image()?.save(output.join("client-space-drag-card.png"))?;
        Ok(())
    });
    let over_card = second.update(cx, |c, _, _| c.sidebar_json())?["drag"].clone();
    ensure!(over_card["target"] == json!(format!("card of session {first_card} at 0")) && over_card["measured"]["popover"].as_u64().is_some_and(|n| n >= 2),
        "the drag from window 2 did not aim at the first session's card at {on_card:?} (popover {:?}): {over_card}", cards.frame);
    let card_hint = over_card["hint"].clone();
    native2.call(json!({"op":"post_mouse","kind":"up","point":local_card}));
    let mut on_first = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if window.update(cx, |c, _, _| spaces_of(c, &first_card).first() == Some(&dragged))? {
            on_first = true;
            break;
        }
    }
    ensure!(on_first, "the drop on the card left the first session with {:?}", window.update(cx, |c, _, _| spaces_of(c, &first_card))?);
    cx.update(|cx| menu.update(cx, |m, cx| m.toggle_popover(None, cx)));
    // With both sidebars hidden, a press on window 1's tab drags the space onto window 2's tab
    // strip, between its first and second tabs (SpaceDropping's tabs case).
    for h in [window, second] {
        h.update(cx, |c, _, cx| {
            c.set_sidebar(false);
            cx.notify();
        })?;
        draw(h, None, cx)?;
    }
    let tab_of = |c: &Client, space: &str| c.title_row().chips.iter().find(|ch| ch.hit == wheel_gpui::header::Hit::Space(space.to_owned())).map(|ch| ch.rect);
    let mut tab_from = None;
    for _ in 0..30 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        tab_from = window.update(cx, |c, _, _| tab_of(c, &dragged))?;
        if tab_from.is_some() {
            break;
        }
    }
    let tab_from = tab_from.ok_or_else(|| anyhow::anyhow!("window 1 shows no tab for {dragged}: {:?}", window.update(cx, |c, _, _| c.title_row().chips.iter().map(|ch| ch.label.clone()).collect::<Vec<_>>()).ok()))?;
    let tabs2 = second.update(cx, |c, _, _| c.title_row().tabs)?;
    let first_tab2 = tabs2.first().copied().ok_or_else(|| anyhow::anyhow!("window 2 shows no tabs"))?;
    // Just past the first tab's middle: the bar goes between the first and the second tab.
    let on_tabs = (f2[0] + first_tab2.x + first_tab2.w * 0.75, f2[1] + first_tab2.y + first_tab2.h / 2.);
    let local_tabs = [f64::from(on_tabs.0 - f1[0]), f64::from(on_tabs.1 - f1[1])];
    let tab_centre = [f64::from(tab_from.x + tab_from.w / 2.), f64::from(tab_from.y + tab_from.h / 2.)];
    native.call(json!({"op":"post_mouse","kind":"down","point":tab_centre}));
    native.call(json!({"op":"post_mouse","kind":"drag","point":[tab_centre[0] + 10., tab_centre[1]]}));
    tick(cx, 100).await;
    draw(window, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":local_tabs}));
    tick(cx, 150).await;
    draw(window, None, cx)?;
    draw(second, Some(output.join("client-space-drag-tabs.png")), cx)?;
    let over_tabs = window.update(cx, |c, _, _| c.sidebar_json()["drag"].clone())?;
    let window2_tabs = second.update(cx, |c, _, _| c.sidebar_json())?["hovered"].clone();
    ensure!(over_tabs["target"] == json!(format!("tabs of {id2} at 1")) && over_tabs["hint"].as_str().is_some_and(|h| h.starts_with("Place “dragged” here in “")) && window2_tabs["tab_bar"] == 1,
        "the drag from window 1's tab did not aim between window 2's first two tabs at {on_tabs:?}: {over_tabs}; window 2 sees {window2_tabs}; window 2's tabs {tabs2:?}");
    native.call(json!({"op":"post_mouse","kind":"up","point":local_tabs}));
    let mut onto_tabs_after = vec![];
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(second, None, cx)?;
        onto_tabs_after = second.update(cx, |c, _, _| spaces_of(c, &second_session))?;
        if onto_tabs_after.get(1) == Some(&dragged) && window.update(cx, |c, _, _| spaces_of(c, &second_session).get(1) == Some(&dragged))? {
            break;
        }
    }
    ensure!(onto_tabs_after.get(1) == Some(&dragged), "the drop on window 2's tabs left the second session with {onto_tabs_after:?}; window 1 logged {:?}", drag_lines(window, cx)?);
    for h in [window, second] {
        h.update(cx, |c, _, cx| {
            c.set_sidebar(true);
            cx.notify();
        })?;
        draw(h, None, cx)?;
    }
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, card, card_hint, onto_tabs_after, over_card, over_tabs, window2_tabs))
}

/// A pane dragged by its header out of its window, and the second window closing.
async fn pane_drag_out_of_window(run: Run, f1: Vec<f32>, f2: Vec<f32>, id1: String, id2: String, second: gpui::WindowHandle<Client>, native2: std::rc::Rc<Native>, second_session: String, first_space: Option<String>, dragged: String, drag_dir: PathBuf, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let row_frame = |state: &Value, id: &str| -> Option<[f64; 4]> {
        state["rows"].as_array()?.iter().find(|r| r["id"] == id).map(|r| [r["frame"][0].as_f64().unwrap(), r["frame"][1].as_f64().unwrap(), r["frame"][2].as_f64().unwrap(), r["frame"][3].as_f64().unwrap()])
    };
    let floats = |cx: &mut AsyncApp| cx.update(|_| wheel_gpui::view::float::state());
    let spaces_of = |c: &Client, session: &str| -> Vec<String> { c.layout_lists().0.into_iter().find(|s| s.id == session).map(|s| s.spaces).unwrap_or_default() };
    let frame_of = |n: &Native| -> Vec<f32> { n.call(json!({"op":"frame"}))["frame"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default() };
    // A pane dragged by its header out of its window (wheel-ynjo.8.3; PaneMove.swift's awayTarget,
    // armHold, dropAway): from a space of window 1 onto a row of window 2, held there until
    // window 2 shows that space, then onto a side of a pane in it; from window 2 onto the
    // desktop, where it moves into a new space in a little window; and from there onto window
    // 1's sidebar room, where it moves into a new space of window 1's session.
    let mut pane_away = {
        let grid_of = |c: &Client| -> Vec<(String, wheel_gpui::layout::Rect)> { c.layers().first().map(|l| l.leaves.iter().map(|p| (p.leaf.id.clone(), p.rect)).collect()).unwrap_or_default() };
        let holding = |c: &Client, pane: &str| -> Option<String> { c.layout_lists().1.into_iter().find(|s| s.site_of(pane).is_some()).map(|s| s.id) };
        let pane_lines = |h: gpui::WindowHandle<Client>, cx: &mut AsyncApp| -> Result<Vec<String>> { Ok(h.update(cx, |c, _, _| c.toast_log.iter().filter(|l| l.contains("pane drag") || l.contains("little window")).rev().take(12).cloned().collect())?) };
        let pane_dir = std::env::temp_dir().join(format!("wheel-gpui-pane-away-{}", std::process::id()));
        std::fs::create_dir_all(&pane_dir)?;
        let (ok, out, err) = run_pumped(&["space", "new", &pane_dir.to_string_lossy(), "--name", "paneaway"], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel space new for the pane drag failed: {err}");
        let away_space = out.trim().to_owned();
        let (ok, out, err) = run_pumped(&["space", "move", &away_space, &id1], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel space move of the pane's space to window 1 failed: {err} {out}");
        let mut moved_pane = None;
        for _ in 0..50 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            window.update(cx, |c, _, cx| {
                c.select_space(&away_space);
                cx.notify();
            })?;
            draw(window, None, cx)?;
            moved_pane = window.update(cx, |c, _, _| (c.space_id() == Some(away_space.as_str())).then(|| grid_of(c)).and_then(|g| g.first().cloned()))?;
            if moved_pane.is_some() {
                break;
            }
        }
        let (moved, moved_rect) = moved_pane.ok_or_else(|| anyhow::anyhow!("window 1 never showed {away_space} with a pane"))?;
        let label = window.update(cx, |c, _, _| c.layout_lists().1.iter().flat_map(|s| s.shown()).find(|l| l.id == moved).map(|l| c.pane_label(l)))?.unwrap_or_default();
        let shown2 = second.update(cx, |c, _, _| c.space_id().map(str::to_owned))?;
        let candidates: Vec<String> = second.update(cx, |c, _, _| spaces_of(c, &second_session))?.into_iter().filter(|s| Some(s) != shown2.as_ref()).collect();
        let held = candidates.iter().find(|s| **s == dragged).or(candidates.first()).cloned().ok_or_else(|| anyhow::anyhow!("the second session has no space window 2 does not show"))?;
        let rows2 = second.update(cx, |c, _, _| c.sidebar_json())?;
        let row_held = row_frame(&rows2, &held).ok_or_else(|| anyhow::anyhow!("window 2's sidebar has no row for {held}"))?;
        let on_row = (f2[0] + row_held[0] as f32 + 20., f2[1] + row_held[1] as f32 + 5.);
        let local_row = [f64::from(on_row.0 - f1[0]), f64::from(on_row.1 - f1[1])];
        let grip = [f64::from(moved_rect.x + 30.), f64::from(moved_rect.y + 11.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":grip}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        native.call(json!({"op":"post_mouse","kind":"drag","point":[grip[0] + 20., grip[1] + 20.]}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(window, None, cx)?;
        native.call(json!({"op":"post_mouse","kind":"drag","point":local_row}));
        tick(cx, 150).await;
        draw(window, None, cx)?;
        draw(second, None, cx)?;
        let over_row = window.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
        let row_seen = second.update(cx, |c, _, _| c.sidebar_json())?;
        let row_outlined = row_seen["rows"].as_array().and_then(|r| r.iter().find(|r| r["id"] == held.as_str())).map(|r| r["pane_over"].clone()).unwrap_or_default();
        ensure!(over_row["away"] == format!("the end of space {held}") && over_row["hold"] == json!([id2, held]) && over_row["measured"].as_array().map(Vec::len) == Some(2)
            && over_row["hint"].as_str().is_some_and(|h| h.starts_with(&format!("Move “{label}” to the end of “"))) && row_outlined == true,
            "the pane drag from window 1 did not aim at window 2's row for {held} at {on_row:?}: {over_row}; window 2's row outlined {row_outlined}; window 1 logged {:?}", pane_lines(window, cx)?);
        let mut chip_pane = Value::Null;
        for _ in 0..20 {
            chip_pane = floats(cx)["chip"].clone();
            if chip_pane["shown"] == true && chip_pane["text"] == label.as_str() {
                break;
            }
            tick(cx, 100).await;
        }
        ensure!(chip_pane["shown"] == true && chip_pane["text"] == label.as_str(), "no chip window names {label} over window 2's row: {chip_pane}");
        // Held over the row, window 2 shows that space after HOLD_SECONDS.
        let held_from = std::time::Instant::now();
        ensure!(shown(second, cx)?.1.as_deref() != Some(held.as_str()), "window 2 showed {held} before the hold ran its time");
        let mut held_after = None;
        for _ in 0..40 {
            tick(cx, 50).await;
            draw(window, None, cx)?;
            draw(second, None, cx)?;
            if second.update(cx, |c, _, _| c.space_id() == Some(held.as_str()))? {
                held_after = Some(held_from.elapsed().as_millis());
                break;
            }
        }
        let switched = window.update(cx, |c, _, _| c.sidebar_json()["pane_drag"]["switched"].clone())?;
        ensure!(held_after.is_some_and(|ms| ms >= 250) && switched == json!([[id2, shown2]]), "holding over window 2's row did not show {held} there: it shows {:?}, switched {switched}; window 1 logged {:?}", shown(second, cx)?, pane_lines(window, cx)?);
        // Then onto the left side of a pane of that space in window 2's grid.
        let (target, target_rect) = second.update(cx, |c, _, _| grid_of(c).into_iter().find(|(id, _)| *id != moved))?.ok_or_else(|| anyhow::anyhow!("window 2 shows no pane of {held}"))?;
        let target_empty = second.update(cx, |c, _, _| c.layout_lists().1.iter().flat_map(|s| s.shown()).any(|l| l.id == target && l.kind == "empty"))?;
        let on_grid = (f2[0] + target_rect.x + 8., f2[1] + target_rect.y + target_rect.h / 2.);
        let local_grid = [f64::from(on_grid.0 - f1[0]), f64::from(on_grid.1 - f1[1])];
        native.call(json!({"op":"post_mouse","kind":"drag","point":local_grid}));
        tick(cx, 150).await;
        draw(window, None, cx)?;
        draw(second, Some(output.join("client-pane-away-grid.png")), cx)?;
        let over_grid = window.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
        let lit = second.update(cx, |c, _, _| c.grid_drop_highlight())?;
        let lit_expect = wheel_gpui::grid_drop::highlight(wheel_gpui::grid_drop::Zone::Edge(wheel_proto::Dir::Left), target_rect);
        ensure!(over_grid["away"] == format!("{target} left in space {held}") && over_grid["hint"].as_str().is_some_and(|h| h.starts_with(&format!("Dock “{label}” on the left of {target}"))) && lit == Some(lit_expect),
            "the pane drag did not aim at the left of {target} in window 2 at {on_grid:?}: {over_grid}; window 2 lights {lit:?}, not {lit_expect:?}");
        native.call(json!({"op":"post_mouse","kind":"up","point":local_grid}));
        let mut docked = false;
        for _ in 0..50 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(window, None, cx)?;
            draw(second, None, cx)?;
            if second.update(cx, |c, _, _| c.space_id() == Some(held.as_str()) && holding(c, &moved).as_deref() == Some(held.as_str()) && grid_of(c).iter().any(|(id, _)| *id == moved))?
                && window.update(cx, |c, _, _| holding(c, &moved).as_deref() == Some(held.as_str()))?
            {
                docked = true;
                break;
            }
        }
        let docked_grid = second.update(cx, |c, _, _| grid_of(c))?;
        // A target space whose only pane is empty loses it to the pane that arrives (wheeld's pane_move).
        let left_of = if target_empty {
            docked_grid.len() == 1 && docked_grid[0].0 == moved
        } else {
            docked_grid.iter().find(|(id, _)| *id == moved).zip(docked_grid.iter().find(|(id, _)| *id == target)).is_some_and(|(m, t)| m.1.x < t.1.x)
        };
        ensure!(docked && left_of && window.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].is_null())? && floats(cx)["chip"]["shown"] == false,
            "the drop left {moved} in {:?}, window 2 showing {:?} with {docked_grid:?}; window 1 logged {:?}", window.update(cx, |c, _, _| holding(c, &moved))?, shown(second, cx)?, pane_lines(window, cx)?);
        draw(second, Some(output.join("client-pane-away-docked.png")), cx)?;
        // From window 2 onto the desktop: the pane moves into a new space in a little window, grown
        // from a card centred on the drop.
        let moved_rect2 = docked_grid.iter().find(|(id, _)| *id == moved).map(|m| m.1).unwrap();
        let grip2 = [f64::from(moved_rect2.x + 30.), f64::from(moved_rect2.y + 11.)];
        let desk_pane = (f1[0] + f1[2] + 100., f2[1] + 300.);
        let local_desk = [f64::from(desk_pane.0 - f2[0]), f64::from(desk_pane.1 - f2[1])];
        native2.call(json!({"op":"post_mouse","kind":"down","point":grip2}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        native2.call(json!({"op":"post_mouse","kind":"drag","point":[grip2[0] + 20., grip2[1] + 20.]}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(second, None, cx)?;
        native2.call(json!({"op":"post_mouse","kind":"drag","point":local_desk}));
        tick(cx, 150).await;
        draw(second, None, cx)?;
        let over_desk_pane = second.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
        let desk_chip_text = format!("{label} → new little window");
        let mut desk_chip = Value::Null;
        for _ in 0..20 {
            desk_chip = floats(cx)["chip"].clone();
            if desk_chip["text"] == desk_chip_text.as_str() {
                break;
            }
            tick(cx, 100).await;
        }
        ensure!(over_desk_pane["away"] == "the desktop" && over_desk_pane["hint"] == format!("Release to move “{label}” into a new space in a little window") && desk_chip["shown"] == true && desk_chip["text"] == desk_chip_text.as_str(),
            "the pane drag from window 2 did not aim at the desktop at {desk_pane:?}: {over_desk_pane}; chip {desk_chip}");
        native2.call(json!({"op":"post_mouse","kind":"up","point":local_desk}));
        let mut pane_little = None;
        for _ in 0..50 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(second, None, cx)?;
            let new_space = second.update(cx, |c, _, _| holding(c, &moved))?.filter(|s| *s != held);
            pane_little = new_space.clone().and_then(|s| cx.update(|_| wheel_gpui::windows::with(|r| r.little_showing(&s).map(|l| (s.clone(), l.id.clone(), l.owner.clone())))));
            if pane_little.is_some() {
                break;
            }
        }
        let (pane_space, pane_little_id, pane_little_owner) = pane_little.ok_or_else(|| anyhow::anyhow!("the pane's drop on the desktop opened no little window: {moved} is in {:?}; window 2 logged {:?}", second.update(cx, |c, _, _| holding(c, &moved)).ok(), pane_lines(second, cx).ok()))?;
        let pane_little_handle = cx.update(|_| wheel_gpui::window_handle(&pane_little_id)).ok_or_else(|| anyhow::anyhow!("no handle for little window {pane_little_id}"))?;
        let pane_little_native = pane_little_handle.update(cx, |c, _, _| c.native())?;
        let pane_little_expect = (desk_pane.0 - 240., desk_pane.1 - 165.);
        let mut pane_little_frame = vec![];
        for _ in 0..20 {
            draw(pane_little_handle, None, cx)?;
            tick(cx, 100).await;
            pane_little_frame = frame_of(&pane_little_native);
            if pane_little_frame.len() == 4 && (pane_little_frame[0] - pane_little_expect.0).abs() < 2. && (pane_little_frame[1] - pane_little_expect.1).abs() < 2. {
                break;
            }
        }
        ensure!(pane_little_frame.len() == 4 && (pane_little_frame[0] - pane_little_expect.0).abs() < 2. && (pane_little_frame[1] - pane_little_expect.1).abs() < 2. && floats(cx)["chip"]["shown"] == false,
            "the little window for {pane_space} is at {pane_little_frame:?}; expected its top left at {pane_little_expect:?}");
        draw(pane_little_handle, Some(output.join("client-pane-away-little.png")), cx)?;
        pane_little_handle.update(cx, |c, _, _| c.little_back())?;
        for _ in 0..30 {
            draw(pane_little_handle, None, cx).ok();
            tick(cx, 100).await;
            if cx.update(|_| wheel_gpui::window_handle(&pane_little_id)).is_none() {
                break;
            }
        }
        ensure!(cx.update(|_| wheel_gpui::window_handle(&pane_little_id)).is_none(), "the little window {pane_little_id} did not close");
        // From window 2, which shows the new space, onto the room under window 1's rows: the pane
        // moves into a new space in window 1's session.
        let mut moved_rect3 = None;
        for _ in 0..30 {
            second.update(cx, |c, _, cx| {
                c.select_space(&pane_space);
                cx.notify();
            })?;
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(second, None, cx)?;
            moved_rect3 = second.update(cx, |c, _, _| (c.space_id() == Some(pane_space.as_str())).then(|| grid_of(c).into_iter().find(|(id, _)| *id == moved).map(|m| m.1)).flatten())?;
            if moved_rect3.is_some() {
                break;
            }
        }
        let moved_rect3 = moved_rect3.ok_or_else(|| anyhow::anyhow!("window 2 never showed {pane_space} holding {moved}: it shows {:?}", shown(second, cx).ok()))?;
        let grip3 = [f64::from(moved_rect3.x + 30.), f64::from(moved_rect3.y + 11.)];
        let bar1 = window.update(cx, |c, _, _| c.sidebar_rect())?;
        let on_room = (f1[0] + bar1.x + 30., f1[1] + bar1.y + bar1.h - 30.);
        let local_room = [f64::from(on_room.0 - f2[0]), f64::from(on_room.1 - f2[1])];
        native2.call(json!({"op":"post_mouse","kind":"down","point":grip3}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        native2.call(json!({"op":"post_mouse","kind":"drag","point":[grip3[0] + 20., grip3[1] + 20.]}));
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        draw(second, None, cx)?;
        native2.call(json!({"op":"post_mouse","kind":"drag","point":local_room}));
        tick(cx, 150).await;
        draw(second, None, cx)?;
        draw(window, Some(output.join("client-pane-away-room.png")), cx)?;
        let over_room = second.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
        let room_outlined = window.update(cx, |c, _, _| c.sidebar_json()["pane_new_space"].clone())?;
        ensure!(over_room["away"] == format!("a new space in window {id1}") && over_room["hint"].as_str().is_some_and(|h| h.starts_with(&format!("Move “{label}” into a new space in “"))) && room_outlined == true,
            "the pane drag from window 2 did not aim at window 1's sidebar room at {on_room:?}: {over_room}; window 1 outlines it {room_outlined}");
        native2.call(json!({"op":"post_mouse","kind":"up","point":local_room}));
        let mut room_space = None;
        for _ in 0..50 {
            pump(&mut connection, 100, &mut layouts, window, cx).await?;
            draw(second, None, cx)?;
            draw(window, None, cx)?;
            room_space = window.update(cx, |c, _, _| holding(c, &moved).filter(|s| *s != pane_space && c.session_spaces().iter().any(|x| x.id == *s)))?;
            if room_space.is_some() && second.update(cx, |c, _, _| holding(c, &moved) == room_space)? {
                break;
            }
        }
        let room_space = room_space.ok_or_else(|| anyhow::anyhow!("the drop on window 1's sidebar room left {moved} in {:?}; window 2 logged {:?}", window.update(cx, |c, _, _| holding(c, &moved)).ok(), pane_lines(second, cx).ok()))?;
        let pane_away = json!({
            "pane": moved, "label": label,
            "onto_row": {"away": over_row["away"], "hint": over_row["hint"], "hold": over_row["hold"], "row_outlined": row_outlined, "chip": chip_pane["text"], "held_ms": held_after, "switched": switched},
            "onto_grid": {"away": over_grid["away"], "hint": over_grid["hint"], "lit": lit.map(|r| [r.x, r.y, r.w, r.h]), "pane_left_of_target": left_of, "target_was_empty": target_empty},
            "onto_desktop": {"away": over_desk_pane["away"], "chip": desk_chip["text"], "space": pane_space, "little": pane_little_id, "owner": pane_little_owner, "frame": pane_little_frame},
            "onto_room": {"away": over_room["away"], "hint": over_room["hint"], "outlined": room_outlined, "space": room_space},
        });
        for space in [&room_space, &pane_space, &away_space] {
            // A space its last pane left may be gone already.
            if !window.update(cx, |c, _, _| c.layout_lists().1.iter().any(|x| x.id == **space))? {
                continue;
            }
            let (ok, _, err) = run_pumped(&["space", "close", space], &mut connection, &mut layouts, window, cx).await?;
            ensure!(ok, "wheel space close {space} after the pane drags failed: {err}");
        }
        let _ = std::fs::remove_dir_all(&pane_dir);
        pane_away
    };
    let (ok, _, err) = run_pumped(&["space", "close", &dragged], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space close for the dragged space failed: {err}");
    let _ = std::fs::remove_dir_all(&drag_dir);
    let first_side = Side { h: window, native: native.clone(), f: f1.clone(), id: id1.clone() };
    let second_side = Side { h: second, native: native2.clone(), f: f2.clone(), id: id2.clone() };
    if let Some(space) = first_space.clone() {
        window.update(cx, |c, _, cx| {
            c.select_space(&space);
            cx.notify();
        })?;
    }
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    // The last case closes the second window mid-drag; its connection ends: wheeld forgets its
    // view, the first stays.
    pane_away["more"] = pane_drag_cases(&first_side, &second_side, &bridge, &mut connection, &mut layouts, cx).await?;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, pane_away))
}

/// `wheel window` and the window menus: rename, new, move, close and Rename Session….
async fn window_commands(run: Run, first_session: Option<String>, id1: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    let before_new = cx.update(|app| app.windows().into_iter().filter(|w| w.downcast::<Client>().is_some()).count());
    let (ok, out, err) = run_pumped(&["window", "new", "--name", "Second"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window new failed: {err}");
    let new_id = out.trim().to_owned();
    let made = cx.update(|_| wheel_gpui::window_handle(&new_id)).ok_or_else(|| anyhow::anyhow!("wheel window new printed {new_id:?}, which is no window"))?;
    ensure!(cx.update(|app| app.windows().into_iter().filter(|w| w.downcast::<Client>().is_some()).count()) == before_new + 1, "wheel window new opened no window");
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
    // The last window on a session that holds spaces closes at once, by `wheel window close` and
    // by Close Window and by the red light: its session runs on in the background, and a new
    // window shows it again.
    let (ok, out, err) = run_pumped(&["window", "ls", "--json"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window ls failed: {err}");
    let listed_now = window_ls(&out)?;
    let background_space = listed_now["background"][0]["id"].as_str().map(str::to_owned).ok_or_else(|| anyhow::anyhow!("no background space to open a window on: {listed_now}"))?;
    let mut closed_last = vec![];
    for how in ["wheel window close", "Close Window", "the red light"] {
        let (ok, out, err) = run_pumped(&["window", "new", "--name", "Last", "--space", &background_space], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel window new --space {background_space} failed: {err}");
        let last_id = out.trim().to_owned();
        let last = cx.update(|_| wheel_gpui::window_handle(&last_id)).ok_or_else(|| anyhow::anyhow!("wheel window new printed {last_id:?}, which is no window"))?;
        let mut session = None;
        for _ in 0..30 {
            session = last.update(cx, |c, _, _| (c.space_id() == Some(background_space.as_str())).then(|| c.session_id().map(str::to_owned)).flatten())?;
            if session.is_some() {
                break;
            }
            tick(cx, 100).await;
        }
        let session = session.ok_or_else(|| anyhow::anyhow!("the window opened on {background_space} never showed it"))?;
        ensure!(wheel_gpui::windows::with(|r| r.windows.iter().filter(|w| w.session.as_deref() == Some(session.as_str())).count()) == 1, "another window shows session {session}");
        if how == "Close Window" {
            last.update(cx, |c, _, cx| c.run_action("close-window", cx))?;
            // A hidden window renders only when drawn, and its render is what removes it.
            draw(last, None, cx)?;
        } else if how == "the red light" {
            let closes = gpui::AnyWindowHandle::from(last).update(cx, |_, window, cx| {
                let now = wheel_gpui::red_light(window, cx);
                if now {
                    window.remove_window();
                }
                now
            })?;
            ensure!(closes, "the red light on the last window of session {session} kept it open");
        } else {
            let (ok, _, err) = run_pumped(&["window", "close", &last_id], &mut connection, &mut layouts, window, cx).await?;
            ensure!(ok, "wheel window close on the last window of session {session} failed: {err}");
        }
        for _ in 0..20 {
            if cx.update(|_| wheel_gpui::window_handle(&last_id)).is_none() {
                break;
            }
            tick(cx, 100).await;
        }
        ensure!(cx.update(|_| wheel_gpui::window_handle(&last_id)).is_none(), "{how} left the last window of session {session} open");
        pump(&mut connection, 300, &mut layouts, window, cx).await?;
        let (ok, out, err) = run_pumped(&["window", "ls", "--json"], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel window ls failed: {err}");
        let after = window_ls(&out)?;
        let running = window.update(cx, |c, _, _| c.layout_lists().0.iter().any(|s| s.id == session && s.spaces.contains(&background_space)))?;
        ensure!(running && after["background"].as_array().is_some_and(|b| b.iter().any(|s| s["id"] == json!(background_space))),
            "after {how} closed the last window, session {session} is listed {running} and the background reads {}", after["background"]);
        closed_last.push(json!({"how": how, "window": last_id, "session": session, "running": running, "background": after["background"]}));
    }
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
    let windows_report = json!({"ls": listed, "new": new_id, "moved": moved, "closed_last": closed_last, "session_renamed_from": old_session_name});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, windows_report))
}

/// Little windows: open, the short agent pane, back, the red light and Bring Back.
async fn little_windows(run: Run, id1: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let window_ls = |out: &str| -> Result<Value> { serde_json::from_str(out).map_err(|e| anyhow::anyhow!("wheel window ls --json printed {out:?}: {e}")) };
    // Little windows (wheel-ynjo.7.5.1; LittleWindow.swift, AppModel+Windows.swift): the space
    // menu's Open in Little Window opens the shown space in a window of its own, owned by this one,
    // whose tab and row dim and whose grid shows the placeholder; the strip's back icon, the red
    // light and Bring Back each close it, and the space's grid comes home.
    let little_space = window.update(cx, |c, _, _| c.space_id().map(str::to_owned))?.ok_or_else(|| anyhow::anyhow!("the window shows no space to open in a little window"))?;
    let home_panes = window.update(cx, |c, _, _| c.native_panes.clone())?;
    let menu_title = |cx: &mut AsyncApp| -> Result<Option<String>> {
        Ok(window.update(cx, |c, _, _| c.space_menu_items(&little_space).into_iter().find(|i| i.id == "little").map(|i| i.title))?)
    };
    ensure!(menu_title(cx)?.as_deref() == Some("Open in Little Window"), "the space menu offers {:?}", menu_title(cx)?);
    let open_little = |cx: &mut AsyncApp| -> Result<gpui::WindowHandle<Client>> {
        window.update(cx, |c, _, cx| {
            c.run_space_menu("little", &little_space, cx);
            cx.notify();
        })?;
        let id = window.update(cx, |c, _, _| c.little_showing(&little_space))?.ok_or_else(|| anyhow::anyhow!("Open in Little Window opened none: {}", window.update(cx, |c, _, _| c.little_json()).unwrap_or_default()))?;
        cx.update(|_| wheel_gpui::window_handle(&id)).ok_or_else(|| anyhow::anyhow!("little window {id} has no handle"))
    };
    let little = open_little(cx)?;
    let little_id = little.update(cx, |c, _, _| c.window_id().to_owned())?;
    let mut little_up = false;
    for _ in 0..50 {
        pump(&mut connection, 100, &mut layouts, window, cx).await?;
        if little.update(cx, |c, _, _| c.session_id().is_some() && c.space_id() == Some(little_space.as_str()))? {
            draw(little, None, cx)?;
            if little.update(cx, |c, _, _| home_panes.iter().all(|p| c.native_panes.contains(p)))? {
                little_up = true;
                break;
            }
        }
    }
    ensure!(little_up, "the little window never showed the space's panes: {:?} of {home_panes:?}", little.update(cx, |c, _, _| (c.space_id().map(str::to_owned), c.native_panes.clone()))?);
    draw(window, None, cx)?;
    let state = window.update(cx, |c, _, _| c.little_json())?;
    ensure!(state["space_away"] == true && window.update(cx, |c, _, _| c.scene().0.is_empty() && c.native_panes.is_empty())?, "the owner still draws the space away in its little window: {state}");
    ensure!(state["littles"] == json!([{"id": little_id, "space": little_space, "owner": id1, "pinned": false}]), "the app's list of little windows reads {}", state["littles"]);
    ensure!(little.update(cx, |c, _, _| c.little_json())?["mode"] == json!({"owner": id1, "space": little_space}), "the little window does not know its owner");
    ensure!(menu_title(cx)?.as_deref() == Some("Go to Little Window"), "with the little window open the menu offers {:?}", menu_title(cx)?);
    let (ok, out, err) = run_pumped(&["window", "ls", "--json"], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel window ls failed: {err}");
    let little_ls = window_ls(&out)?;
    ensure!(little_ls["little"][0]["id"] == json!(little_id) && little_ls["little"][0]["space"]["id"] == json!(little_space) && little_ls["little"][0]["owner"] == json!(id1), "wheel window ls lists the little windows as {}", little_ls["little"]);
    ensure!(little_ls["windows"].as_array().is_some_and(|w| w.iter().all(|w| w["id"] != json!(little_id))), "a little window is listed as a main window: {little_ls}");
    draw(window, Some(output.join("client-little-placeholder.png")), cx)?;
    draw(little, Some(output.join("client-little-window.png")), cx)?;
    // The little window's agent pane is shorter than its label, composer and footer: nothing it
    // draws lands below its frame, where the window's bottom inset shows one flat colour.
    let short_pane = little.update(cx, |c, _, _| c.scene().0.into_iter().find(|p| p.leaf.kind == "agent"))?.ok_or_else(|| anyhow::anyhow!("the little window shows no agent pane"))?;
    let short_rect = short_pane.rect;
    let little_any: gpui::AnyWindowHandle = little.into();
    let (short_height, short_from, short_colours, short_inside) = little_any.update(cx, |_, w, cx| -> Result<(u32, u32, std::collections::BTreeSet<u32>, usize)> {
        w.draw(cx).clear();
        let image = w.render_to_image()?;
        let scale = image.width() as f32 / f32::from(w.viewport_size().width);
        let (x0, x1) = (((short_rect.x + 4.) * scale) as u32, ((short_rect.x + short_rect.w - 4.) * scale) as u32);
        let y0 = ((short_rect.y + short_rect.h) * scale).ceil() as u32 + 1;
        let mut colours = std::collections::BTreeSet::new();
        for y in y0..image.height() {
            for x in x0..x1.min(image.width()) {
                let [r, g, b, _] = image.get_pixel(x, y).0;
                colours.insert(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b));
            }
        }
        // The body under the title still draws its label and composer.
        let mut inside = std::collections::BTreeSet::new();
        for y in ((short_rect.y + TITLE) * scale) as u32..((short_rect.y + short_rect.h) * scale) as u32 {
            for x in x0..x1.min(image.width()) {
                inside.insert(image.get_pixel(x, y).0);
            }
        }
        Ok((image.height(), y0, colours, inside.len()))
    })??;
    ensure!(short_rect.h < 140. && short_from < short_height, "the little window's agent pane is not short, or reaches the window's bottom: {short_rect:?}, window {short_height} px, strip from {short_from}");
    ensure!(short_colours.len() == 1, "the little window's agent pane ({short_rect:?}) draws below its frame: {} colours under it, {:x?}", short_colours.len(), short_colours.iter().take(8).collect::<Vec<_>>());
    ensure!(short_inside > 1, "the little window's agent pane ({short_rect:?}) draws nothing in its body: {short_inside} colour");
    let little_short = json!({"pane": [short_rect.x, short_rect.y, short_rect.w, short_rect.h], "rows_below": short_height - short_from, "colours_below": short_colours.len(), "colours_inside": short_inside});
    // Go to Little Window from the menu again opens no second one.
    window.update(cx, |c, _, cx| c.run_space_menu("little", &little_space, cx))?;
    ensure!(window.update(cx, |c, _, _| c.little_json())?["littles"].as_array().map(Vec::len) == Some(1), "Go to Little Window opened a second little window");
    let wait_closed = async |id: &str, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp| -> Result<bool> {
        for _ in 0..40 {
            pump(connection, 100, layouts, window, cx).await?;
            // A hidden window renders only when drawn, and its render is what removes it.
            if let Some(open) = cx.update(|_| wheel_gpui::window_handle(id)) {
                let _ = draw(open, None, cx);
            }
            if cx.update(|_| wheel_gpui::window_handle(id)).is_none() {
                draw(window, None, cx)?;
                return Ok(window.update(cx, |c, _, _| !c.space_away() && c.little_showing(&little_space).is_none())?);
            }
        }
        Ok(false)
    };
    // The strip's back icon.
    little.update(cx, |c, _, cx| {
        c.little_back();
        cx.notify();
    })?;
    ensure!(wait_closed(&little_id, &mut connection, &mut layouts, cx).await?, "the back icon left the little window open or the owner's grid away");
    ensure!(window.update(cx, |c, _, _| home_panes.iter().all(|p| c.native_panes.contains(p)))?, "the owner did not take its space's panes back: {:?}", window.update(cx, |c, _, _| c.native_panes.clone())?);
    draw(window, Some(output.join("client-little-back.png")), cx)?;
    // The red light: the window may close at once, and the space comes home.
    let little = open_little(cx)?;
    let red_id = little.update(cx, |c, _, _| c.window_id().to_owned())?;
    let closes = gpui::AnyWindowHandle::from(little).update(cx, |_, window, cx| {
        let now = wheel_gpui::red_light(window, cx);
        if now {
            window.remove_window();
        }
        now
    })?;
    ensure!(closes, "a little window's red light asked a question");
    ensure!(wait_closed(&red_id, &mut connection, &mut layouts, cx).await?, "the red light left the owner's grid away");
    // Bring Back from the owner's placeholder.
    let little = open_little(cx)?;
    let brought_id = little.update(cx, |c, _, _| c.window_id().to_owned())?;
    window.update(cx, |c, _, cx| c.bring_back(&little_space, cx))?;
    ensure!(wait_closed(&brought_id, &mut connection, &mut layouts, cx).await?, "Bring Back left the little window open");
    let little_report = json!({"short_agent_pane": little_short, "space": little_space, "owner": id1, "panes": home_panes, "ls": little_ls["little"], "closed_by": ["back", "red light", "bring back"],
        "pictures": ["client-little-placeholder.png", "client-little-window.png", "client-little-back.png"]});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, little_report))
}

/// Web panes: artifacts under the `wheel-page` scheme, reloads, and the palette's artifacts.
async fn web_panes(run: Run, webs: Vec<&String>, cli_path: String, before: Value, cx: &mut AsyncApp) -> Result<(Run, Vec<String>, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, page_names, web_report))
}

/// The browser pane: `wheel pane page`, console, eval, dom, shot and record, the address
/// field, back, and a failed load.
async fn browser_pane(run: Run, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let press = |native: &Native, b: (f32, f32, f32, f32)| {
        let p = [f64::from(b.0 + b.2 / 2.), f64::from(b.1 + b.3 / 2.)];
        native.call(json!({"op":"post_mouse","kind":"down","point":p}));
        native.call(json!({"op":"post_mouse","kind":"up","point":p}));
    };
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, browser_report))
}

/// Every plugin page drawn by the A2UI renderer in one pane of a session on this repository.
async fn plugin_pages(run: Run, page_names: Vec<String>, first_session: Option<String>, first_space: Option<String>, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, pages_report))
}

/// The command palette, key bindings, Run…, the shortcut sheet and ⌘W.
async fn command_palette(run: Run, cli_path: String, terms: Vec<&String>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, commands_report))
}

/// The Settings window and each of its tabs.
async fn settings_window(run: Run, cli_path: String, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let post = |native: &Native, ch: char, command: bool| native.call(json!({"op":"post_key","text":ch.to_string(),"code":keycode(ch),"command":command}));
    let config = |args: &[&str]| -> Result<std::process::Output> {
        Ok(std::process::Command::new(&cli_path).args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "config"]).args(args).output()?)
    };
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, settings_report))
}

/// Notices, the pairing window and the quarantine card.
async fn notices(run: Run, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
        copied = wheel_gpui::view::copied_pairing_code().is_some();
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, notices_report))
}

/// The debug server's routes and the Processes card.
async fn debug_server_and_processes(run: Run, terms: Vec<&String>, cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::debug::Server, String, u16, u16, u16, Vec<String>, Value, usize, Value, u64, bool, u16, String)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, debug, bearer, unauthorized, origin_refused, missing, state_keys, sleep_row, lists_seen, card_rows, snapshot_bytes, stopped, front_refused, fixture_space))
}

/// The pane grid: a space of its own, splits, a real header drag, dock, and a popped card.
async fn pane_grid(run: Run, first_session_id: String, mut debug: wheel_gpui::debug::Server, bearer: String, cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::debug::Server, String, String, String, String, [f64; 4], [f64; 4], Vec<String>, Value, [f64; 4], Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let auth: [(&str, &str); 1] = [("Authorization", &bearer)];
    macro_rules! call {
        ($method:literal, $target:expr) => {
            http(&mut debug, $method, &$target, &auth, &mut connection, &mut layouts, window, cx).await?
        };
    }
    macro_rules! state {
        () => { call!("GET", "/state").1 };
    }
    // The pane grid (wheel-ynjo.8.1): a space of its own, shown by a focus request; the header's
    // menu splits it; a real header drag onto another pane's middle swaps the two, a drag held
    // on an edge docks beside it; the header's buttons pop and background a pane; a focus request
    // moves the pane focus; the menu moves a pane into a new space.
    macro_rules! until {
        ($what:expr, $cond:expr) => {{
            let mut got = Value::Null;
            let mut held = false;
            for _ in 0..60 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                draw(window, None, cx)?;
                got = state!();
                // A frame read while the grid animates is a step of the motion, not where it ends.
                if got["grid_motion"]["moving"] != true && ($cond)(&got) {
                    held = true;
                    break;
                }
            }
            ensure!(held, "{} never happened: {}", $what, got);
            got
        }};
    }
    let grid_dir = std::env::temp_dir().join(format!("wheel-gpui-grid-{}", std::process::id()));
    std::fs::create_dir_all(&grid_dir)?;
    let (ok, out, err) = run_pumped(&["space", "new", &grid_dir.to_string_lossy(), "--name", "grid", "--session", &first_session_id], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space new grid failed: {err}");
    let grid = out.trim().to_owned();
    let (ok, _, err) = run_pumped(&["space", "focus", &grid], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space focus failed: {err}");
    let shown = until!("the grid space showing after wheel space focus", |s: &Value| s["active"] == grid.as_str());
    let grid_panes = |s: &Value| -> Vec<String> {
        s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).and_then(|x| x["panes"].as_array())
            .map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let grid_a = grid_panes(&shown).first().cloned().ok_or_else(|| anyhow::anyhow!("the grid space has no pane"))?;
    let menu_ids: Vec<String> = shown["panes"][grid_a.as_str()]["menu"].as_array().map(|m| m.iter().filter_map(|i| i["id"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let here = shown["panes"][grid_a.as_str()]["menu"].as_array().and_then(|m| m.iter().find(|i| i["id"] == format!("move-space:{grid}")).cloned()).unwrap_or_default();
    ensure!(here["enabled"] == false && here["title"].as_str().is_some_and(|t| t.ends_with("(here)")), "the pane's own space is not offered as here: {here}");
    ensure!(shown["panes"][grid_a.as_str()]["header_buttons"]["split-right"]["frame"].is_array(), "the header reports no buttons: {}", shown["panes"][grid_a.as_str()]);
    let (code, _) = call!("POST", format!("/panemenu?action=run&pane={grid_a}&item=split-right"));
    ensure!(code == 200, "split-right got {code}");
    // The split's new pane takes the focus once wheeld makes it (AppModel.split's `showing: .pane`).
    let two = until!("a second pane, focused, after Split Right", |s: &Value| grid_panes(s).len() == 2 && grid_panes(s).iter().any(|p| p != &grid_a && s["panes"][p.as_str()]["focused"] == true));
    let grid_b = grid_panes(&two).into_iter().find(|p| p != &grid_a).unwrap_or_default();
    // Split Down by a real press on the header's button: it splits and starts no drag.
    let button = two["panes"][grid_b.as_str()]["header_buttons"]["split-down"]["frame"].clone();
    let at = [button[0].as_f64().unwrap_or(0.) + 9., button[1].as_f64().unwrap_or(0.) + 9.];
    native.call(json!({"op":"post_mouse","kind":"down","point":at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":at}));
    let three = until!("a third pane after a press on Split Down", |s: &Value| grid_panes(s).len() == 3);
    ensure!(three["sidebar"]["pane_drag"].is_null(), "a press on a header button started a drag: {}", three["sidebar"]["pane_drag"]);
    let grid_c = grid_panes(&three).into_iter().find(|p| p != &grid_a && p != &grid_b).unwrap_or_default();
    let frame = |s: &Value, pane: &str| -> [f64; 4] {
        let f = &s["panes"][pane]["frame"];
        [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
    };
    // The real drag: press the first pane's header, move onto the third's middle, release.
    let (a, c) = (frame(&three, &grid_a), frame(&three, &grid_c));
    let grip = [a[0] + 30., a[1] + 11.];
    let middle = [c[0] + c[2] / 2., c[1] + c[3] / 2.];
    let pressed = native.call(json!({"op":"post_mouse","kind":"down","point":grip}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    let dragged = native.call(json!({"op":"post_mouse","kind":"drag","point":[grip[0] + 20., grip[1] + 20.]}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":middle}));
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    let over = state!();
    let log_tail: Vec<Value> = over["log"].as_array().map(|l| l.iter().rev().take(8).cloned().collect()).unwrap_or_default();
    ensure!(over["sidebar"]["pane_drag"]["drop"] == json!({"target": grid_c, "zone": "center"}), "the drag over the third pane's middle reads {} (press at {grip:?} hit {pressed}, drag hit {dragged}; frames {a:?} {c:?}; log {log_tail:?}; presses {:?})", over["sidebar"]["pane_drag"], window.update(cx, |c, _, _| c.presses.iter().rev().take(4).cloned().collect::<Vec<_>>())?);
    ensure!(over["grid_drop_highlight"] == json!(c), "the swap lights {}, not the whole target {c:?}", over["grid_drop_highlight"]);
    draw(window, Some(output.join("client-pane-swap.png")), cx)?;
    native.call(json!({"op":"post_mouse","kind":"up","point":middle}));
    let swapped = until!("the swap", |s: &Value| frame(s, &grid_a) == c && frame(s, &grid_c) == a);
    ensure!(swapped["sidebar"]["pane_drag"].is_null() && swapped["grid_drop_highlight"].is_null(), "the drag outlived its drop: {}", swapped["sidebar"]["pane_drag"]);
    // Dock: the second pane held on the left quarter of the first (now bottom right).
    let (code, _) = call!("POST", format!("/panedrag?action=start&pane={grid_b}"));
    ensure!(code == 200, "panedrag start got {code}");
    let (code, _) = call!("POST", format!("/panedrag?action=hover&target=pane:{grid_a}:left"));
    ensure!(code == 200, "panedrag hover got {code}");
    let docking = state!();
    let f = frame(&docking, &grid_a);
    ensure!(docking["grid_drop_highlight"] == json!([f[0], f[1], f[2] * 0.5, f[3]]), "the dock lights {}, not the left half of {f:?}", docking["grid_drop_highlight"]);
    ensure!(docking["sidebar"]["pane_drag"]["hint"] == format!("Dock {grid_b} on the left of {grid_a}"), "the dock hint reads {}", docking["sidebar"]["pane_drag"]["hint"]);
    draw(window, Some(output.join("client-pane-dock.png")), cx)?;
    let (code, _) = call!("POST", "/panedrag?action=drop");
    ensure!(code == 200, "panedrag drop got {code}");
    let docked = until!("the dock", |s: &Value| { let (b, a) = (frame(s, &grid_b), frame(s, &grid_a)); b[0] >= 0. && b[0] < a[0] && b[1] == a[1] && b[3] == a[3] });
    // A focus request for a pane moves this window's focus to it (the header press above focused the first).
    ensure!(docked["panes"][grid_c.as_str()]["focused"] == false, "the third pane was focused before the request: {}", docked["focus_key"]);
    let (ok, _, err) = run_pumped(&["pane", "focus", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane focus failed: {err}");
    until!("the third pane focused by wheel pane focus", |s: &Value| s["panes"][grid_c.as_str()]["focused"] == true);
    // The header's buttons: Pop out, then back with the CLI; Background, then back.
    let site_list = |s: &Value, key: &str| s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).map(|x| x[key].clone()).unwrap_or_default();
    // The popped pane draws as a card over the grid (wheel-ynjo.7.3). The third pane and the first
    // become terminals, so the card holds a native view that must sit on top of the grid's.
    for pane in [&grid_c, &grid_a] {
        let (ok, _, err) = run_pumped(&["pane", "kind", pane, "terminal"], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel pane kind {pane} terminal failed: {err}");
        until!("a terminal view in the pane", |s: &Value| s["panes"][pane.as_str()]["native"] == true);
    }
    let (code, _) = call!("POST", format!("/panemenu?action=run&pane={grid_c}&item=pop"));
    ensure!(code == 200, "pop got {code}");
    let popped = until!("the third pane popped", |s: &Value| site_list(s, "popped") == json!([grid_c]));
    ensure!(!grid_panes(&popped).contains(&grid_c), "a popped pane stayed in the grid: {:?}", grid_panes(&popped));
    let four = |v: &Value| -> [f64; 4] { [0, 1, 2, 3].map(|i| v[i].as_f64().unwrap_or(-1.)) };
    let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5);
    let body_of = |f: [f64; 4]| [f[0], f[1] + f64::from(TITLE), f[2], f[3] - f64::from(TITLE)];
    let card_now = |s: &Value| four(&s["cards"][0]["frame"]);
    let carded = until!("the popped pane in a card with its view shown", |s: &Value| s["cards"].as_array().is_some_and(|c| c.len() == 1)
        && s["panes"][grid_c.as_str()]["bridge"]["hidden"] == false && near(four(&s["panes"][grid_c.as_str()]["bridge"]["frame_top"]), body_of(card_now(s))));
    ensure!(carded["cards"][0]["panes"] == json!([grid_c]), "the card holds {}, not the popped pane", carded["cards"][0]["panes"]);
    let card_before = card_now(&carded);
    ensure!(near(frame(&carded, &grid_c), card_before), "the popped pane fills {:?}, not its card {card_before:?}", frame(&carded, &grid_c));
    let area = four(&json!(window.update(cx, |c, _, _| { let r = c.grid_area(); [r.x, r.y, r.w, r.h] })?));
    ensure!(card_before[0] >= area[0] && card_before[1] >= area[1] && card_before[0] + card_before[2] <= area[0] + area[2] + 0.5 && card_before[1] + card_before[3] <= area[1] + area[3] + 0.5,
        "the card {card_before:?} is not inside the grid {area:?}");
    // The grid's terminal has the card cut out of it, so the card's own view is the one on top there.
    let holes = carded["panes"][grid_a.as_str()]["bridge"]["holes"].clone();
    ensure!(holes.as_array().is_some_and(|h| h.len() == 1 && near(four(&h[0]), card_before)), "the grid terminal under the card has holes {holes}, not the card {card_before:?}");
    let card_holes = carded["panes"][grid_c.as_str()]["bridge"]["holes"].clone();
    ensure!(card_holes.as_array().is_some_and(|h| h.len() == 1 && near(four(&h[0]), four(&carded["cards"][0]["grip"]))), "the card's view has holes {card_holes}, not its resize corner {}", carded["cards"][0]["grip"]);
    // Where the card covers the grid terminal, the pointer lands on the card's view, and the grid
    // terminal's mask keeps the rects around the hole.
    let inside = [card_before[0] + 30., card_before[1] + f64::from(TITLE) + 30.];
    let hit = native.call(json!({"op":"hit","point":inside}));
    ensure!(hit["pane"] == grid_c.as_str(), "a point inside the card's body at {inside:?} hits {hit}, not the card's terminal {grid_c}");
    let rects = carded["panes"][grid_a.as_str()]["bridge"]["mask_rects"].as_u64().unwrap_or(0);
    ensure!(rects > 1, "the grid terminal's mask keeps {rects} rect(s): the card was not cut out of it");
    draw(window, Some(output.join("client-popped-card.png")), cx)?;
    // A real drag on the card's header moves it, and its view goes with it.
    let header_at = [card_before[0] + 40., card_before[1] + 11.];
    let moved_by = [-80., 50.];
    native.call(json!({"op":"post_mouse","kind":"down","point":header_at}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    for step in 1..=4 {
        let k = f64::from(step) / 4.;
        native.call(json!({"op":"post_mouse","kind":"drag","point":[header_at[0] + moved_by[0] * k, header_at[1] + moved_by[1] * k]}));
        pump(&mut connection, 60, &mut layouts, window, cx).await?;
    }
    let dragging = state!();
    ensure!(dragging["card_drag"]["pane"] == grid_c.as_str() && dragging["sidebar"]["pane_drag"].is_null(), "the header drag reads card {} and pane drag {} (presses {})", dragging["card_drag"], dragging["sidebar"]["pane_drag"], dragging["presses"]);
    draw(window, Some(output.join("client-popped-card-moving.png")), cx)?;
    native.call(json!({"op":"post_mouse","kind":"up","point":[header_at[0] + moved_by[0], header_at[1] + moved_by[1]]}));
    let want = [card_before[0] + moved_by[0], card_before[1] + moved_by[1], card_before[2], card_before[3]];
    let card_state = until!("the card moved by its header", |s: &Value| s["card_drag"].is_null() && near(card_now(s), want)
        && near(four(&s["panes"][grid_c.as_str()]["bridge"]["frame_top"]), body_of(want)));
    let card_moved = card_now(&card_state);
    // Its corner resizes it.
    let corner = four(&card_state["cards"][0]["grip"]);
    let at = [corner[0] + corner[2] / 2., corner[1] + corner[3] / 2.];
    native.call(json!({"op":"post_mouse","kind":"down","point":at}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[at[0] + 40., at[1] + 30.]}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"up","point":[at[0] + 40., at[1] + 30.]}));
    let want = [card_moved[0], card_moved[1], card_moved[2] + 40., card_moved[3] + 30.];
    let resized = until!("the card resized by its corner", |s: &Value| s["card_drag"].is_null() && near(card_now(s), want));
    let card_resized = card_now(&resized);
    draw(window, Some(output.join("client-popped-card-moved.png")), cx)?;
    let (ok, _, err) = run_pumped(&["pane", "pop", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane pop failed: {err}");
    let back = until!("the third pane back in the grid", |s: &Value| grid_panes(s).contains(&grid_c) && s["cards"].as_array().is_some_and(Vec::is_empty));
    ensure!(back["panes"][grid_a.as_str()]["bridge"]["holes"] == json!([]), "the grid terminal kept holes after the card left: {}", back["panes"][grid_a.as_str()]["bridge"]["holes"]);
    // Popped again, it comes back where it was left.
    let (ok, _, err) = run_pumped(&["pane", "pop", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane pop again failed: {err}");
    let again = until!("the card back where it was left", |s: &Value| s["cards"].as_array().is_some_and(|c| c.len() == 1));
    ensure!(near(card_now(&again), card_resized), "popped again the card is at {:?}, not where it was left {card_resized:?}", card_now(&again));
    let (ok, _, err) = run_pumped(&["pane", "pop", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane pop back failed: {err}");
    until!("the third pane back in the grid again", |s: &Value| grid_panes(s).contains(&grid_c));
    let popped_card_report = json!({"pane": grid_c, "grid": area, "before": card_before, "moved": card_moved, "resized": card_resized, "popped_again": card_now(&again),
        "grid_view_holes": holes, "card_view_holes": card_holes, "hit_inside_card": hit["pane"], "grid_mask_rects": rects});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, debug, grid, grid_a, grid_b, grid_c, a, c, menu_ids, docked, area, popped_card_report))
}

/// Drops onto and off a card, and the tray.
async fn card_drops_and_tray(run: Run, mut debug: wheel_gpui::debug::Server, bearer: String, grid: String, grid_a: String, grid_b: String, grid_c: String, area: [f64; 4], cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::debug::Server, Value, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let auth: [(&str, &str); 1] = [("Authorization", &bearer)];
    macro_rules! call {
        ($method:literal, $target:expr) => {
            http(&mut debug, $method, &$target, &auth, &mut connection, &mut layouts, window, cx).await?
        };
    }
    macro_rules! state {
        () => { call!("GET", "/state").1 };
    }
    macro_rules! until {
        ($what:expr, $cond:expr) => {{
            let mut got = Value::Null;
            let mut held = false;
            for _ in 0..60 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                draw(window, None, cx)?;
                got = state!();
                // A frame read while the grid animates is a step of the motion, not where it ends.
                if got["grid_motion"]["moving"] != true && ($cond)(&got) {
                    held = true;
                    break;
                }
            }
            ensure!(held, "{} never happened: {}", $what, got);
            got
        }};
    }
    let grid_panes = |s: &Value| -> Vec<String> {
        s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).and_then(|x| x["panes"].as_array())
            .map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let frame = |s: &Value, pane: &str| -> [f64; 4] {
        let f = &s["panes"][pane]["frame"];
        [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
    };
    let site_list = |s: &Value, key: &str| s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).map(|x| x[key].clone()).unwrap_or_default();
    let four = |v: &Value| -> [f64; 4] { [0, 1, 2, 3].map(|i| v[i].as_f64().unwrap_or(-1.)) };
    let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5);
    let card_now = |s: &Value| four(&s["cards"][0]["frame"]);
    // Drops onto and off a card (wheel-ynjo.26): the second pane dragged by its header onto the
    // card's pane docks on that pane's nearest side inside the card (the two trees never swap), and
    // the card dragged by its header to the grid's left edge lights that edge, then sends both its
    // panes to the tray. Both come back to the grid after.
    let (ok, _, err) = run_pumped(&["pane", "pop", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane pop for the card drops failed: {err}");
    let recarded = until!("the third pane in its card again", |s: &Value| s["cards"].as_array().is_some_and(|c| c.len() == 1) && s["panes"][grid_b.as_str()]["frame"].is_array());
    let card = card_now(&recarded);
    let b_at = frame(&recarded, &grid_b);
    let b_grip = [b_at[0] + 30., b_at[1] + 11.];
    let in_card = |p: [f64; 2], f: [f64; 4]| p[0] >= f[0] && p[0] <= f[0] + f[2] && p[1] >= f[1] && p[1] <= f[1] + f[3];
    ensure!(!in_card(b_grip, card), "the second pane's header at {b_grip:?} is under the card {card:?}");
    let onto = [card[0] + 20., card[1] + card[3] * 0.5];
    native.call(json!({"op":"post_mouse","kind":"down","point":b_grip}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[b_grip[0] + 20., b_grip[1] + 20.]}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":onto}));
    pump(&mut connection, 300, &mut layouts, window, cx).await?;
    let over_card = state!();
    let c_at = frame(&over_card, &grid_c);
    ensure!(over_card["sidebar"]["pane_drag"]["drop"] == json!({"target": grid_c, "zone": "left"}), "the drag onto the card's pane at {onto:?} reads {} (card {card:?})", over_card["sidebar"]["pane_drag"]);
    ensure!(over_card["grid_drop_highlight"] == json!([c_at[0], c_at[1], c_at[2] * 0.5, c_at[3]]), "the dock onto the card lights {}, not the left half of {c_at:?}", over_card["grid_drop_highlight"]);
    draw(window, Some(output.join("client-card-dock.png")), cx)?;
    native.call(json!({"op":"post_mouse","kind":"up","point":onto}));
    let docked_in_card = until!("the second pane docked left of the third in the card", |s: &Value| site_list(s, "popped") == json!([grid_b, grid_c])
        && s["cards"].as_array().is_some_and(|c| c.len() == 1 && c[0]["panes"] == json!([grid_b, grid_c])));
    let (b_in, c_in) = (frame(&docked_in_card, &grid_b), frame(&docked_in_card, &grid_c));
    ensure!(b_in[0] < c_in[0] && b_in[1] == c_in[1] && !grid_panes(&docked_in_card).contains(&grid_b), "in the card the second pane is at {b_in:?} and the third at {c_in:?}");
    // The card's header dragged to the grid's left edge.
    let card = card_now(&docked_in_card);
    let head = [card[0] + 40., card[1] + 11.];
    let edge_at = [area[0] + 6., head[1]];
    native.call(json!({"op":"post_mouse","kind":"down","point":head}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    for step in 1..=4 {
        let k = f64::from(step) / 4.;
        native.call(json!({"op":"post_mouse","kind":"drag","point":[head[0] + (edge_at[0] - head[0]) * k, head[1]]}));
        pump(&mut connection, 60, &mut layouts, window, cx).await?;
    }
    let bar = [area[0], area[1], f64::from(wheel_gpui::popped::EDGE_BAR), area[3]];
    let at_edge = until!("the left edge lit with the card held over it", |s: &Value| s["card_drag"]["edge"] == "left" && near(four(&s["edge_bar"]), bar)
        && grid_panes(s).iter().any(|p| s["panes"][p.as_str()]["bridge"]["holes"].as_array().is_some_and(|h| h.iter().any(|r| near(four(r), bar)))));
    draw(window, Some(output.join("client-card-edge.png")), cx)?;
    native.call(json!({"op":"post_mouse","kind":"up","point":edge_at}));
    let to_tray = until!("both card panes in the tray", |s: &Value| s["cards"].as_array().is_some_and(Vec::is_empty) && s["edge_bar"].is_null()
        && site_list(s, "backgrounded").as_array().is_some_and(|b| b.len() == 2 && b.contains(&json!(grid_b)) && b.contains(&json!(grid_c))));
    let card_drops_report = json!({"dock": over_card["sidebar"]["pane_drag"]["drop"], "dock_highlight": over_card["grid_drop_highlight"], "card_panes": docked_in_card["cards"][0]["panes"],
        "edge": at_edge["card_drag"]["edge"], "edge_bar": at_edge["edge_bar"], "backgrounded": site_list(&to_tray, "backgrounded"), "tray_chip": to_tray["tray"]["chip"]});
    for pane in [&grid_b, &grid_c] {
        let (ok, _, err) = run_pumped(&["pane", "background", pane], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel pane background {pane} back to the grid failed: {err}");
    }
    until!("both panes back in the grid", |s: &Value| grid_panes(s).contains(&grid_b) && grid_panes(s).contains(&grid_c) && s["tray"]["chip"].is_null());
    let (code, _) = call!("POST", format!("/panemenu?action=run&pane={grid_c}&item=background"));
    ensure!(code == 200, "background got {code}");
    // The tray (wheel-ynjo.8.5): the chip at its place on the grid's right edge, cut out of the
    // grid's terminal; a click opens its list, a drag pins it to another edge, a row brings the pane back.
    let trayed = until!("the third pane backgrounded with the tray chip shown", |s: &Value| site_list(s, "backgrounded") == json!([grid_c]) && s["tray"]["chip"].is_array());
    let chip = four(&trayed["tray"]["chip"]);
    ensure!(trayed["tray"]["place"] == "right" && trayed["tray"]["rows"][0]["pane"] == grid_c.as_str() && trayed["tray"]["rows"][0]["title"] == "Terminal" && trayed["tray"]["open"] == false,
        "the tray reads {}", trayed["tray"]);
    ensure!((chip[0] + chip[2] + 8. - (area[0] + area[2])).abs() < 0.5, "the chip {chip:?} is not on the grid's right edge {area:?}");
    let tray_holes = until!("the chip cut out of the grid terminal", |s: &Value| s["panes"][grid_a.as_str()]["bridge"]["holes"].as_array().is_some_and(|h| h.len() == 1 && near(four(&h[0]), chip)));
    let chip_hit = native.call(json!({"op":"hit","point":[chip[0] + chip[2] / 2., chip[1] + chip[3] / 2.]}));
    ensure!(chip_hit["pane"].is_null(), "a press on the chip lands in pane {}, not on the chip", chip_hit["pane"]);
    let centre = |r: [f64; 4]| [r[0] + r[2] / 2., r[1] + r[3] / 2.];
    let click_at = |at: [f64; 2]| {
        native.call(json!({"op":"post_mouse","kind":"down","point":at}));
        native.call(json!({"op":"post_mouse","kind":"up","point":at}));
    };
    click_at(centre(chip));
    let listed = until!("the tray list open", |s: &Value| s["tray"]["open"] == true && s["tray"]["list"].is_array());
    let list = four(&listed["tray"]["list"]);
    ensure!(list[0] + list[2] <= chip[0] + 0.5, "the list {list:?} does not open left of the chip {chip:?}");
    let list_holes = until!("the list cut out of the grid terminal", |s: &Value| s["panes"][grid_a.as_str()]["bridge"]["holes"].as_array().is_some_and(|h| h.len() == 2 && near(four(&h[1]), list)));
    draw(window, Some(output.join("client-tray-list.png")), cx)?;
    let (code, escaped) = call!("POST", "/key?keys=escape");
    ensure!(code == 200 && escaped["taken"] == true, "Esc did not close the tray list: {escaped}");
    until!("the tray list closed by Esc", |s: &Value| s["tray"]["open"] == false);
    // A press inside the grid terminal's native view closes the tray list, the pane menu and the
    // space menu (wheel-ynjo.27): the press goes to the terminal alone, never GPUI's drawing.
    let in_term = { let f = frame(&listed, &grid_a); [f[0] + 30., f[1] + f[3] - 30.] };
    let term_hit = native.call(json!({"op":"hit","point":in_term}));
    ensure!(term_hit["pane"] == grid_a.as_str(), "a press at {in_term:?} lands in {term_hit}, not the grid terminal");
    let press_term = |what: &str| -> Result<Value> {
        let down = native.call(json!({"op":"post_mouse","kind":"down","point":in_term}));
        native.call(json!({"op":"post_mouse","kind":"up","point":in_term}));
        ensure!(down["target"] == term_hit["class"], "the press with the {what} open went to {}, not the terminal's {}", down["target"], term_hit["class"]);
        Ok(down["target"].clone())
    };
    click_at(centre(chip));
    until!("the tray list open again for the terminal press", |s: &Value| s["tray"]["open"] == true);
    let press_target = press_term("tray list")?;
    until!("the tray list closed by a press in the terminal", |s: &Value| s["tray"]["open"] == false);
    draw(window, Some(output.join("client-tray-terminal-press.png")), cx)?;
    let (code, _) = call!("POST", format!("/panemenu?action=open&pane={grid_a}&x=420&y=120"));
    ensure!(code == 200 && state!()["pane_menu"]["pane"] == grid_a.as_str(), "the pane menu did not open for the terminal press");
    press_term("pane menu")?;
    until!("the pane menu closed by a press in the terminal", |s: &Value| s["pane_menu"].is_null());
    let (code, _) = call!("POST", format!("/spacemenu?space={grid}"));
    ensure!(code == 200 && state!()["space_menu"]["space"] == grid.as_str(), "the space menu did not open for the terminal press");
    press_term("space menu")?;
    until!("the space menu closed by a press in the terminal", |s: &Value| s["space_menu"].is_null());
    // A drag to the grid's top edge pins the chip there.
    let from = centre(chip);
    let to = [area[0] + area[2] * 0.5, area[1] + 20.];
    native.call(json!({"op":"post_mouse","kind":"down","point":from}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[from[0] - 30., from[1] - 30.]}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":to}));
    pump(&mut connection, 100, &mut layouts, window, cx).await?;
    let mid_drag = state!();
    ensure!(mid_drag["tray"]["dragging"] == true && mid_drag["tray"]["open"] == false, "mid-drag the tray reads {}", mid_drag["tray"]);
    native.call(json!({"op":"post_mouse","kind":"up","point":to}));
    let moved_tray = until!("the chip pinned to the top edge", |s: &Value| s["tray"]["place"] == "up" && s["tray"]["dragging"].is_null());
    let moved_chip = four(&moved_tray["tray"]["chip"]);
    ensure!((moved_chip[1] - (area[1] + 8.)).abs() < 0.5 && (centre(moved_chip)[0] - to[0]).abs() < 0.5 && moved_tray["tray"]["open"] == false,
        "the dragged chip sits at {moved_chip:?}, not on the top edge at {to:?}");
    click_at(centre(moved_chip));
    let below = until!("the list open below the moved chip", |s: &Value| s["tray"]["open"] == true);
    let below_list = four(&below["tray"]["list"]);
    ensure!(below_list[1] >= moved_chip[1] + moved_chip[3], "the list {below_list:?} does not open below the chip {moved_chip:?}");
    draw(window, Some(output.join("client-tray-moved.png")), cx)?;
    // Its row's pop-out button pops the pane out; Background puts it back in the tray.
    click_at(centre(four(&below["tray"]["rows"][0]["pop"])));
    until!("the tray's pop-out button popped the pane", |s: &Value| site_list(s, "popped") == json!([grid_c]) && s["tray"]["chip"].is_null() && s["tray"]["open"] == false);
    let (ok, _, err) = run_pumped(&["pane", "background", &grid_c], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane background failed: {err}");
    let again = until!("the pane back in the tray", |s: &Value| site_list(s, "backgrounded") == json!([grid_c]) && s["tray"]["chip"].is_array());
    click_at(centre(four(&again["tray"]["chip"])));
    let reopened = until!("the tray list open again", |s: &Value| s["tray"]["open"] == true);
    click_at(centre(four(&reopened["tray"]["rows"][0]["frame"])));
    let back_from_tray = until!("the row brought the pane back to the grid", |s: &Value| grid_panes(s).contains(&grid_c) && s["panes"][grid_c.as_str()]["site"] == "grid" && s["tray"]["chip"].is_null());
    ensure!(back_from_tray["panes"][grid_a.as_str()]["bridge"]["holes"] == json!([]), "the grid terminal kept holes after the chip went: {}", back_from_tray["panes"][grid_a.as_str()]["bridge"]["holes"]);
    let tray_report = json!({"chip": chip, "list": list, "grid_view_holes": tray_holes["panes"][grid_a.as_str()]["bridge"]["holes"], "with_list": list_holes["panes"][grid_a.as_str()]["bridge"]["holes"],
        "moved_place": moved_tray["tray"]["place"], "moved_chip": moved_chip, "list_below": below_list, "row": reopened["tray"]["rows"][0],
        "terminal_press": {"at": in_term, "target": press_target, "closed": ["tray list", "pane menu", "space menu"]}});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, debug, card_drops_report, tray_report))
}

/// Zoom and grid motion, Reduce Motion, and a layout change while zoomed.
async fn zoom(run: Run, mut debug: wheel_gpui::debug::Server, bearer: String, grid: String, grid_a: String, grid_b: String, grid_c: String, cli: String, cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::debug::Server, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let auth: [(&str, &str); 1] = [("Authorization", &bearer)];
    macro_rules! call {
        ($method:literal, $target:expr) => {
            http(&mut debug, $method, &$target, &auth, &mut connection, &mut layouts, window, cx).await?
        };
    }
    macro_rules! state {
        () => { call!("GET", "/state").1 };
    }
    macro_rules! until {
        ($what:expr, $cond:expr) => {{
            let mut got = Value::Null;
            let mut held = false;
            for _ in 0..60 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                draw(window, None, cx)?;
                got = state!();
                // A frame read while the grid animates is a step of the motion, not where it ends.
                if got["grid_motion"]["moving"] != true && ($cond)(&got) {
                    held = true;
                    break;
                }
            }
            ensure!(held, "{} never happened: {}", $what, got);
            got
        }};
    }
    // A motion's frame mid-way: the check holds the grid's motion clock at a fraction of its
    // length (`Client::motion_hold`) before the step that starts it, and this waits for the motion.
    macro_rules! held_motion {
        ($what:expr) => {{
            let mut got = Value::Null;
            for _ in 0..60 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                draw(window, None, cx)?;
                got = state!();
                if got["grid_motion"]["moving"] == true {
                    break;
                }
            }
            ensure!(got["grid_motion"]["moving"] == true, "{} never started a motion: {}", $what, got["grid_motion"]);
            got
        }};
    }
    let hold_motion = |at: Option<f32>, cx: &mut AsyncApp| window.update(cx, |c, _, _| c.motion_hold = at);
    let grid_panes = |s: &Value| -> Vec<String> {
        s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).and_then(|x| x["panes"].as_array())
            .map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let frame = |s: &Value, pane: &str| -> [f64; 4] {
        let f = &s["panes"][pane]["frame"];
        [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
    };
    let four = |v: &Value| -> [f64; 4] { [0, 1, 2, 3].map(|i| v[i].as_f64().unwrap_or(-1.)) };
    let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5);
    let centre = |r: [f64; 4]| [r[0] + r[2] / 2., r[1] + r[3] / 2.];
    // Zoom and grid motion (wheel-ynjo.8.2): the header's zoom button grows the first pane over
    // the grid while the others slide out past their nearest edge, slowed ten times by
    // WHEEL_SLOW_ANIMATIONS so a frame mid-way can be read. Its terminal sends no resize until the
    // motion ends, then one. A double-click on the header puts the grid back, and ⇧⌘↩ zooms again.
    let zoom_area = four(&json!(window.update(cx, |c, _, _| { let r = c.grid_area(); [r.x, r.y, r.w, r.h] })?));
    let before_zoom = state!();
    let a_from = frame(&before_zoom, &grid_a);
    let sent = |s: &Value| s["panes"][grid_a.as_str()]["bridge"]["sent_grids"].as_array().cloned().unwrap_or_default();
    let sent_before = sent(&before_zoom).len();
    let zoom_button = before_zoom["panes"][grid_a.as_str()]["header_buttons"]["zoom"].clone();
    ensure!(zoom_button["enabled"] == true && zoom_button["on"] == false && before_zoom["grid_motion"]["zoomed"].is_null(), "the zoom button reads {zoom_button} before a zoom");
    // SAFETY: only the main thread reads the variable (`motion::duration`), and this runs on it.
    unsafe { std::env::set_var("WHEEL_SLOW_ANIMATIONS", "10") };
    hold_motion(Some(0.3), cx)?;
    let zoom_at = centre(four(&zoom_button["frame"]));
    native.call(json!({"op":"post_mouse","kind":"down","point":zoom_at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":zoom_at}));
    let mid = held_motion!("the zoom button");
    draw(window, Some(output.join("client-grid-zoom-mid.png")), cx)?;
    let a_mid = frame(&mid, &grid_a);
    ensure!(mid["grid_motion"]["zoomed"] == grid_a.as_str() && mid["grid_motion"]["resizes_held"] == true && mid["grid_motion"]["length_ms"] == 2200,
        "held at 0.3 of its length the zoom's motion reads {}", mid["grid_motion"]);
    let held_progress = mid["grid_motion"]["progress"].as_f64().unwrap_or(-1.);
    ensure!((held_progress - 0.561).abs() < 0.01, "held at 0.3 of its length the zoom reads progress {held_progress}, not 0.561");
    // Each side the pane does not already fill grows part of the way; at least one does.
    let between = |i: usize| a_from[i] >= zoom_area[i] - 1. || (a_mid[i] > a_from[i] + 1. && a_mid[i] < zoom_area[i] - 1.);
    let grows = a_from[2] < zoom_area[2] - 1. || a_from[3] < zoom_area[3] - 1.;
    ensure!(grows && between(2) && between(3), "mid-zoom the first pane is {a_mid:?}, not between {a_from:?} and the grid {zoom_area:?}");
    ensure!(sent(&mid).len() == sent_before, "the terminal sent a resize mid-zoom: {:?}", sent(&mid));
    hold_motion(None, cx)?;
    let zoomed = until!("the zoom finished and the terminal's size sent", |s: &Value| s["grid_motion"]["resizes_held"] == false && sent(s).len() > sent_before);
    let apart = |f: [f64; 4], a: [f64; 4]| f[0] >= a[0] + a[2] || a[0] >= f[0] + f[2] || f[1] >= a[1] + a[3] || a[1] >= f[1] + f[3];
    ensure!(near(frame(&zoomed, &grid_a), zoom_area), "the zoomed pane is {:?}, not the grid {zoom_area:?}", frame(&zoomed, &grid_a));
    for pane in [&grid_b, &grid_c] {
        ensure!(apart(frame(&zoomed, pane), zoom_area), "{pane} is at {:?}, inside the grid {zoom_area:?} while another is zoomed", frame(&zoomed, pane));
    }
    ensure!(zoomed["panes"][grid_c.as_str()]["bridge"]["hidden"] == true, "the slid-out terminal's view still shows: {}", zoomed["panes"][grid_c.as_str()]["bridge"]);
    let (grids, sent_now) = (zoomed["panes"][grid_a.as_str()]["bridge"]["grids"].clone(), sent(&zoomed));
    ensure!(sent_now.len() == sent_before + 1 && grids.as_array().and_then(|g| g.last()) == sent_now.last(), "the zoom sent {:?} after {sent_before}, the terminal's last grid is {:?}", sent_now, grids.as_array().and_then(|g| g.last()));
    let unzoom_button = &zoomed["panes"][grid_a.as_str()]["header_buttons"]["zoom"];
    ensure!(unzoom_button["on"] == true && unzoom_button["label"] == "Unzoom Pane (⇧⌘↩)", "the zoomed pane's button reads {unzoom_button}");
    draw(window, Some(output.join("client-grid-zoomed.png")), cx)?;
    let title_at = [zoom_area[0] + 40., zoom_area[1] + 11.];
    for clicks in [1, 2] {
        native.call(json!({"op":"post_mouse","kind":"down","point":title_at,"clicks":clicks}));
        native.call(json!({"op":"post_mouse","kind":"up","point":title_at,"clicks":clicks}));
    }
    let unzoomed = until!("the header's double-click put the grid back", |s: &Value| s["grid_motion"]["zoomed"].is_null() && near(frame(s, &grid_a), a_from));
    ensure!(unzoomed["sidebar"]["pane_drag"].is_null() && unzoomed["panes"][grid_c.as_str()]["bridge"]["hidden"] == false, "after the unzoom the drag reads {} and the third view {}", unzoomed["sidebar"]["pane_drag"], unzoomed["panes"][grid_c.as_str()]["bridge"]);
    unsafe { std::env::remove_var("WHEEL_SLOW_ANIMATIONS") };
    // A double-click zooms too, at normal speed; ⇧⌘↩ puts the grid back and zooms again.
    let header_at = [a_from[0] + 40., a_from[1] + 11.];
    for clicks in [1, 2] {
        native.call(json!({"op":"post_mouse","kind":"down","point":header_at,"clicks":clicks}));
        native.call(json!({"op":"post_mouse","kind":"up","point":header_at,"clicks":clicks}));
    }
    let clicked_zoom = until!("the header's double-click zoomed the pane", |s: &Value| s["grid_motion"]["zoomed"] == grid_a.as_str() && near(frame(s, &grid_a), zoom_area));
    let normal_ms = clicked_zoom["grid_motion"]["length_ms"].clone();
    ensure!(normal_ms == 220, "the motion lasts {normal_ms} ms with WHEEL_SLOW_ANIMATIONS unset");
    for (zoomed_after, what) in [(false, "⇧⌘↩ put the grid back"), (true, "⇧⌘↩ zoomed the focused pane")] {
        let (code, keyed) = call!("POST", "/key?keys=shift%2Bcmd%2Benter");
        ensure!(code == 200 && keyed["taken"] == true, "⇧⌘↩ got {code} {keyed}");
        until!(what, |s: &Value| if zoomed_after { s["grid_motion"]["zoomed"] == grid_a.as_str() && near(frame(s, &grid_a), zoom_area) } else { s["grid_motion"]["zoomed"].is_null() && near(frame(s, &grid_a), a_from) });
    }
    // A split ends the zoom: its new pane would sit hidden behind the zoomed one.
    let (code, _) = call!("POST", format!("/panemenu?action=run&pane={grid_a}&item=split-right"));
    ensure!(code == 200, "split-right got {code}");
    let split_unzoomed = until!("the split ended the zoom", |s: &Value| grid_panes(s).len() == 4 && s["grid_motion"]["zoomed"].is_null());
    let grid_d = grid_panes(&split_unzoomed).into_iter().find(|p| ![&grid_a, &grid_b, &grid_c].contains(&p)).unwrap_or_default();
    let (ok, _, err) = run_pumped(&["pane", "close", &grid_d], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel pane close {grid_d} failed: {err}");
    until!("the split's pane closed", |s: &Value| grid_panes(s).len() == 3);
    let motions: Vec<Value> = state!()["log"].as_array().map(|l| l.iter().filter(|x| x.as_str().is_some_and(|t| t.contains("grid motion: ") && !t.ends_with("done"))).cloned().collect()).unwrap_or_default();
    ensure!(motions.iter().any(|m| m.as_str().is_some_and(|t| t.contains("grid motion: layout"))), "no layout change animated: {motions:?}");
    let zoom_report = json!({"area": zoom_area, "from": a_from, "mid": a_mid, "mid_progress": mid["grid_motion"]["progress"], "zoomed": frame(&zoomed, &grid_a),
        "slid_out": [frame(&zoomed, &grid_b), frame(&zoomed, &grid_c)], "sent_grids": sent_now, "grids": grids, "lagged_reports": zoomed["panes"][grid_a.as_str()]["bridge"]["lagged_reports"], "normal_ms": normal_ms, "motions": motions});
    // Reduce Motion (wheel-ynjo.8.6), stood in for by the bridge's override: the zoom grows the
    // first pane over the others, which keep their frames and fade, their views hidden by the end;
    // the unzoom fades them back. Slowed ten times again so a frame mid-way can be read.
    let reduced = native.call(json!({"op":"reduce_motion","set":true}));
    ensure!(reduced["reduce_motion"] == true, "the bridge's reduce_motion override answered {reduced}");
    let still = until!("the view heard Reduce Motion on", |s: &Value| s["grid_motion"]["reduce_motion"] == true);
    let (b_from, c_from) = (frame(&still, &grid_b), frame(&still, &grid_c));
    let still_button = four(&still["panes"][grid_a.as_str()]["header_buttons"]["zoom"]["frame"]);
    unsafe { std::env::set_var("WHEEL_SLOW_ANIMATIONS", "10") };
    hold_motion(Some(0.3), cx)?;
    for kind in ["down", "up"] {
        native.call(json!({"op":"post_mouse","kind":kind,"point":centre(still_button)}));
    }
    let rmid = held_motion!("the zoom button under Reduce Motion");
    draw(window, Some(output.join("client-grid-zoom-reduced-mid.png")), cx)?;
    let opacity = |s: &Value, p: &str| s["panes"][p]["opacity"].as_f64().unwrap_or(-1.);
    let alpha = |s: &Value, p: &str| s["panes"][p]["bridge"]["alpha"].as_f64().unwrap_or(-1.);
    ensure!(rmid["grid_motion"]["zoomed"] == grid_a.as_str(), "held at 0.3 of its length the reduced zoom's motion reads {}", rmid["grid_motion"]);
    ensure!(near(frame(&rmid, &grid_b), b_from) && near(frame(&rmid, &grid_c), c_from), "mid reduced zoom the others moved: {:?} {:?} from {b_from:?} {c_from:?}", frame(&rmid, &grid_b), frame(&rmid, &grid_c));
    let (o_mid, a_mid_alpha) = (opacity(&rmid, &grid_c), alpha(&rmid, &grid_c));
    ensure!(o_mid > 0.05 && o_mid < 0.95 && (a_mid_alpha - o_mid).abs() < 0.2 && rmid["panes"][grid_c.as_str()]["bridge"]["hidden"] == false,
        "mid reduced zoom the third pane draws at {o_mid}, its view at alpha {a_mid_alpha}: {}", rmid["panes"][grid_c.as_str()]["bridge"]);
    hold_motion(None, cx)?;
    let rzoomed = until!("the reduced zoom finished", |s: &Value| s["grid_motion"]["moving"] == false && s["grid_motion"]["resizes_held"] == false && near(frame(s, &grid_a), zoom_area));
    for pane in [&grid_b, &grid_c] {
        ensure!(opacity(&rzoomed, pane) == 0., "{pane} draws at {} after the reduced zoom", opacity(&rzoomed, pane));
    }
    ensure!(near(frame(&rzoomed, &grid_b), b_from) && near(frame(&rzoomed, &grid_c), c_from) && rzoomed["panes"][grid_c.as_str()]["bridge"]["hidden"] == true,
        "after the reduced zoom the others are at {:?} {:?}, the third view {}", frame(&rzoomed, &grid_b), frame(&rzoomed, &grid_c), rzoomed["panes"][grid_c.as_str()]["bridge"]);
    draw(window, Some(output.join("client-grid-zoomed-reduced.png")), cx)?;
    // A layout change while zoomed starts a layout motion, which is no fade: the covered panes stay out.
    let zsplit = window.update(cx, |c, _, _| c.splits_to(&grid_c))?.into_iter().next().ok_or_else(|| anyhow::anyhow!("the third pane has no split above it"))?;
    let set_ratio = |ratio: f64| -> Result<()> {
        let out = std::process::Command::new(&cli)
            .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", &zsplit.id, &ratio.to_string()]).output()?;
        ensure!(out.status.success(), "pane ratio {} {ratio}: {}", zsplit.id, String::from_utf8_lossy(&out.stderr));
        Ok(())
    };
    hold_motion(Some(0.3), cx)?;
    set_ratio(if zsplit.ratio > 0.45 { 0.35 } else { 0.55 })?;
    let laid = held_motion!("the new ratio of the zoomed grid");
    ensure!(opacity(&laid, &grid_c) == 0. && laid["panes"][grid_c.as_str()]["bridge"]["hidden"] == true,
        "a layout change while zoomed showed the third pane at {}: {} {}", opacity(&laid, &grid_c), laid["grid_motion"], laid["panes"][grid_c.as_str()]["bridge"]);
    hold_motion(None, cx)?;
    until!("the layout motion ended", |s: &Value| s["grid_motion"]["moving"] == false);
    set_ratio(zsplit.ratio)?;
    until!("the ratio went back", |s: &Value| s["grid_motion"]["moving"] == false && near(frame(s, &grid_c), c_from) && opacity(s, &grid_c) == 0.);
    let unzoom_at = four(&rzoomed["panes"][grid_a.as_str()]["header_buttons"]["zoom"]["frame"]);
    for kind in ["down", "up"] {
        native.call(json!({"op":"post_mouse","kind":kind,"point":centre(unzoom_at)}));
    }
    pump(&mut connection, 600, &mut layouts, window, cx).await?;
    let rback = state!();
    let o_back = opacity(&rback, &grid_c);
    ensure!(rback["grid_motion"]["zoomed"].is_null() && o_back > 0.05 && o_back < 0.95 && rback["panes"][grid_c.as_str()]["bridge"]["hidden"] == false,
        "600 ms into the reduced unzoom the third pane draws at {o_back}: {} {}", rback["grid_motion"], rback["panes"][grid_c.as_str()]["bridge"]);
    let rdone = until!("the reduced unzoom finished", |s: &Value| s["grid_motion"]["moving"] == false && near(frame(s, &grid_a), a_from));
    ensure!(opacity(&rdone, &grid_c) == 1. && alpha(&rdone, &grid_c) == 1. && rdone["panes"][grid_c.as_str()]["bridge"]["hidden"] == false,
        "after the reduced unzoom the third pane draws at {}, its view {}", opacity(&rdone, &grid_c), rdone["panes"][grid_c.as_str()]["bridge"]);
    unsafe { std::env::remove_var("WHEEL_SLOW_ANIMATIONS") };
    native.call(json!({"op":"reduce_motion","set":null}));
    let mut zoom_report = zoom_report;
    zoom_report["reduced"] = json!({"others": [b_from, c_from], "mid": {"progress": rmid["grid_motion"]["progress"], "opacity": o_mid, "alpha": a_mid_alpha, "zoomed_pane": frame(&rmid, &grid_a)},
        "zoomed": {"opacity": opacity(&rzoomed, &grid_c), "hidden": rzoomed["panes"][grid_c.as_str()]["bridge"]["hidden"]}, "unzoom_mid_opacity": o_back});
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, debug, zoom_report))
}

/// The pane header's layout and ⋯ menu, Move Pane to New Space, and the debug report.
async fn pane_header(run: Run, mut debug: wheel_gpui::debug::Server, bearer: String, grid: String, grid_a: String, grid_b: String, cli: String, fixture_space: String, cx: &mut AsyncApp) -> Result<(Run, wheel_gpui::debug::Server, Value, String)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    let auth: [(&str, &str); 1] = [("Authorization", &bearer)];
    macro_rules! call {
        ($method:literal, $target:expr) => {
            http(&mut debug, $method, &$target, &auth, &mut connection, &mut layouts, window, cx).await?
        };
    }
    macro_rules! state {
        () => { call!("GET", "/state").1 };
    }
    macro_rules! until {
        ($what:expr, $cond:expr) => {{
            let mut got = Value::Null;
            let mut held = false;
            for _ in 0..60 {
                pump(&mut connection, 100, &mut layouts, window, cx).await?;
                draw(window, None, cx)?;
                got = state!();
                // A frame read while the grid animates is a step of the motion, not where it ends.
                if got["grid_motion"]["moving"] != true && ($cond)(&got) {
                    held = true;
                    break;
                }
            }
            ensure!(held, "{} never happened: {}", $what, got);
            got
        }};
    }
    let grid_panes = |s: &Value| -> Vec<String> {
        s["spaces"].as_array().and_then(|a| a.iter().find(|x| x["id"] == grid.as_str())).and_then(|x| x["panes"].as_array())
            .map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let frame = |s: &Value, pane: &str| -> [f64; 4] {
        let f = &s["panes"][pane]["frame"];
        [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(-1.))
    };
    // The header's layout (wheel-ynjo.8.4.1): a placement moves a control from the bar to the
    // menu or out of both, Reset puts it back, and a pane narrowed by every split above it
    // drops controls group by group, its ⋯ menu offering each one dropped, until widened again.
    let strings = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().or_else(|| x["id"].as_str()).map(str::to_owned)).collect()).unwrap_or_default() };
    let wide = state!();
    let header_kind = wide["panes"][grid_a.as_str()]["kind"].as_str().unwrap_or_default().to_owned();
    let wide_order = strings(&wide["panes"][grid_a.as_str()]["header_order"]);
    ensure!(wide["panes"][grid_a.as_str()]["header_level"] == 0 && wide_order == ["split-right", "split-down", "pop", "background", "zoom", "menu", "close"], "a wide {header_kind} header: level {} order {wide_order:?}", wide["panes"][grid_a.as_str()]["header_level"]);
    let (code, _) = call!("POST", format!("/paneheader?kind={header_kind}&control=zoom&placement=menu"));
    ensure!(code == 200, "paneheader zoom menu got {code}");
    let (code, _) = call!("POST", format!("/paneheader?kind={header_kind}&control=pop&placement=off"));
    ensure!(code == 200, "paneheader pop off got {code}");
    let placed = state!();
    let (placed_order, placed_menu) = (strings(&placed["panes"][grid_a.as_str()]["header_order"]), strings(&placed["panes"][grid_a.as_str()]["menu"]));
    ensure!(!placed_order.contains(&"zoom".to_owned()) && placed_menu.contains(&"zoom".to_owned()), "Zoom set to the menu: bar {placed_order:?}, menu {placed_menu:?}");
    ensure!(!placed_order.contains(&"pop".to_owned()) && !placed_menu.contains(&"pop".to_owned()), "Pop out set to Off: bar {placed_order:?}, menu {placed_menu:?}");
    ensure!(placed["header_layout"][header_kind.as_str()] == json!({"zoom": "menu", "pop": "off"}), "the layout reads {}", placed["header_layout"]);
    let (code, _) = call!("POST", "/settings?action=open");
    ensure!(code == 200, "settings open got {code}");
    let (code, _) = call!("POST", "/settings?action=tab&tab=general");
    ensure!(code == 200, "settings tab general got {code}");
    draw(window, Some(output.join("client-pane-header-settings.png")), cx)?;
    let (code, _) = call!("POST", "/settings?action=close");
    ensure!(code == 200, "settings close got {code}");
    let (code, _) = call!("POST", format!("/paneheader?kind={header_kind}&reset=1"));
    ensure!(code == 200, "paneheader reset got {code}");
    let reset = until!("Reset put the header back", |s: &Value| strings(&s["panes"][grid_a.as_str()]["header_order"]) == wide_order && s["header_layout"].get(header_kind.as_str()).is_none());
    let splits = window.update(cx, |c, _, _| c.splits_to(&grid_a))?;
    ensure!(!splits.is_empty(), "the grid's first pane has no split above it");
    let narrow_ratio = |split: &str, ratio: f64| -> Result<()> {
        let out = std::process::Command::new(&cli)
            .args(["--url", &default_url(), "--token-file", &default_token_file().to_string_lossy(), "pane", "ratio", split, &ratio.to_string()]).output()?;
        ensure!(out.status.success(), "pane ratio {split} {ratio}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Ok(())
    };
    let mut narrowed = vec![];
    let mut last_width = frame(&wide, &grid_a)[2];
    // About 287, 201, 172 and 96 pt of the grid's 957: every control, then without Pop Out and
    // Background, then without the splits, then only the menu and Close.
    for (share, level) in [(0.3, 0), (0.21, 1), (0.18, 3), (0.1, 4)] {
        for s in &splits {
            narrow_ratio(&s.id, if s.first { share } else { 1. - share })?;
        }
        let narrow = until!("the narrowed header dropping controls", |s: &Value| (level == 0 || s["panes"][grid_a.as_str()]["header_level"].as_u64().unwrap_or(0) > 0) && frame(s, &grid_a)[2] < last_width - 1.);
        last_width = frame(&narrow, &grid_a)[2];
        let (order, more) = (strings(&narrow["panes"][grid_a.as_str()]["header_order"]), strings(&narrow["panes"][grid_a.as_str()]["more_menu"]));
        for id in &wide_order {
            let dropped = !order.contains(id);
            let offered = more.contains(id);
            ensure!(!dropped || offered || id == "menu", "{id} dropped from a {} pt header (level {}) and not in its ⋯ menu {more:?}", frame(&narrow, &grid_a)[2], narrow["panes"][grid_a.as_str()]["header_level"]);
        }
        ensure!(order.ends_with(&["menu".to_owned(), "close".to_owned()]), "a narrow header kept {order:?}");
        ensure!(narrow["panes"][grid_a.as_str()]["header_level"] == level, "a {} pt header is at level {}, not {level}: {order:?}", frame(&narrow, &grid_a)[2], narrow["panes"][grid_a.as_str()]["header_level"]);
        narrowed.push(json!({"share": share, "width": frame(&narrow, &grid_a)[2], "level": narrow["panes"][grid_a.as_str()]["header_level"], "bar": order, "menu": more}));
    }
    // The ⋯ button by a real press opens the menu at the header's level.
    let narrow_now = state!();
    let dots = narrow_now["panes"][grid_a.as_str()]["header_buttons"]["menu"]["frame"].clone();
    let at = [dots[0].as_f64().unwrap_or(0.) + 9., dots[1].as_f64().unwrap_or(0.) + 9.];
    native.call(json!({"op":"post_mouse","kind":"down","point":at}));
    native.call(json!({"op":"post_mouse","kind":"up","point":at}));
    let dots_open = until!("the ⋯ press opened the pane menu", |s: &Value| s["pane_menu"]["pane"] == grid_a.as_str());
    ensure!(dots_open["pane_menu"]["level"] == narrow_now["panes"][grid_a.as_str()]["header_level"], "the ⋯ menu opened at level {} for a header at {}", dots_open["pane_menu"]["level"], narrow_now["panes"][grid_a.as_str()]["header_level"]);
    draw(window, Some(output.join("client-header-narrow-menu.png")), cx)?;
    let (code, _) = call!("POST", "/key?keys=escape");
    ensure!(code == 200 && state!()["pane_menu"].is_null(), "Esc left the ⋯ menu open");
    for s in &splits {
        narrow_ratio(&s.id, s.ratio)?;
    }
    let widened = until!("the widened header drawing every control again", |s: &Value| s["panes"][grid_a.as_str()]["header_level"] == 0 && strings(&s["panes"][grid_a.as_str()]["header_order"]) == wide_order);
    let header_report = json!({"kind": header_kind, "wide": wide_order, "placed": {"bar": placed_order, "menu": placed_menu}, "reset": strings(&reset["panes"][grid_a.as_str()]["header_order"]), "narrowed": narrowed,
        "dots_level": dots_open["pane_menu"]["level"], "widened": frame(&widened, &grid_a)});
    // The menu drawn, and Esc closing it.
    let (code, _) = call!("POST", format!("/panemenu?action=open&pane={grid_a}&x=420&y=120"));
    ensure!(code == 200, "panemenu open got {code}");
    let opened = state!();
    ensure!(opened["pane_menu"]["pane"] == grid_a.as_str(), "the pane menu did not open: {}", opened["pane_menu"]);
    draw(window, Some(output.join("client-pane-menu.png")), cx)?;
    let (code, escaped) = call!("POST", "/key?keys=escape");
    ensure!(code == 200 && escaped["taken"] == true && state!()["pane_menu"].is_null(), "Esc left the pane menu open: {escaped}");
    // Move Pane to New Space: the second pane leaves for a space of its own.
    let spaces_before = state!()["spaces"].as_array().map(Vec::len).unwrap_or(0);
    let (code, _) = call!("POST", format!("/panemenu?action=run&pane={grid_b}&item=move-new-space"));
    ensure!(code == 200, "move-new-space got {code}");
    let moved_out = until!("the second pane in a new space", |s: &Value| s["spaces"].as_array().is_some_and(|a| a.len() == spaces_before + 1) && !grid_panes(s).contains(&grid_b));
    let new_space = moved_out["panes"][grid_b.as_str()]["space"].as_str().unwrap_or_default().to_owned();
    ensure!(!new_space.is_empty() && new_space != grid, "the second pane is in {new_space:?}");
    // Back to the fixture space by a focus request, and the two spaces made here go.
    let (ok, _, err) = run_pumped(&["space", "focus", &fixture_space], &mut connection, &mut layouts, window, cx).await?;
    ensure!(ok, "wheel space focus back failed: {err}");
    until!("the fixture space showing again", |s: &Value| s["active"] == fixture_space.as_str());
    for space in [&new_space, &grid] {
        let (ok, _, err) = run_pumped(&["space", "close", space], &mut connection, &mut layouts, window, cx).await?;
        ensure!(ok, "wheel space close {space} failed: {err}");
    }
    pump(&mut connection, 500, &mut layouts, window, cx).await?;
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, debug, header_report, new_space))
}

/// An older wheeld behind a protocol-lowering proxy shows only "The server was updated."
async fn older_server(run: Run, native_panes: Vec<(String, String)>, cx: &mut AsyncApp) -> Result<(Run, Value, Vec<(String, String)>, usize, PathBuf, PathBuf)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    // An older wheeld (wheel-ynjo.2): the fixture daemon behind a proxy that lowers the protocol
    // number in its `status` by one. The window over it never comes up: it shows the notice with
    // Restart Server, whose click runs `WHEEL_SERVER_STOP` then `WHEEL_SERVER_START`.
    let proxy = OlderProxy::start(&format!("{}?token={}", default_url(), std::fs::read_to_string(default_token_file())?.trim()), PROTOCOL - 1)?;
    let older = Endpoint::new(&proxy.url, &default_token_file())?;
    let mut older_connection = Connection::open(older.clone());
    let older_window = cx.update(|app| wheel_gpui::create_window(app, &bridge, older_connection.send.clone(), older, None, None, true, wheel_gpui::Opening::Main, None))?;
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
        restart_log = wheel_gpui::view::server::with(|s| s.log.iter().rev().find(|l| l.starts_with("server: restart")).cloned()).flatten();
        if restart_log.as_deref() == Some("server: restarted from the app") {
            break;
        }
    }
    ensure!(restart_log.as_deref() == Some("server: restarted from the app"), "Restart Server ended with {restart_log:?}");
    ensure!(stop_file.exists() && start_file.exists(), "Restart Server did not run both commands: stop {} start {}", stop_file.exists(), start_file.exists());
    older_window.update(cx, |c, _, _| c.dismiss_toast(&server_toast))?;
    let (after_dismiss, older_log): (Vec<String>, Vec<String>) = older_window.update(cx, |c, _, _| (c.toasts().iter().map(|t| t.id.clone()).collect(), c.toast_log.iter().chain(wheel_gpui::view::server::with(|s| s.log.clone()).unwrap_or_default().iter()).filter(|l| l.starts_with("server")).cloned().collect()))?;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, server_updated_report, identities_before, layouts_before_restart, done, request))
}

/// The server lifecycle control, and a wheeld restart every native view survives.
async fn server_lifecycle_and_restart(run: Run, native_panes: Vec<(String, String)>, terms: Vec<&String>, webs: Vec<&String>, done: PathBuf, request: PathBuf, identities_before: Vec<(String, String)>, layouts_before_restart: usize, cx: &mut AsyncApp) -> Result<(Run, Value, bool, String, String, Value, Vec<(String, String)>, String, Value)> {
    #[allow(unused_variables, unused_mut)]
    let Run { bridge, mut connection, mut layouts, window, native, markers, output } = run;
    // The server lifecycle (wheel-ynjo.3): the fixture wheeld is one this app did not install, so
    // the control is the commands kind, the word over it reads Running, and no source line is
    // logged (there is no bundled source to compare). A terminated wheeld sends `stopping` first:
    // the client expects it back for 10 s, with no "unreachable" notice meanwhile, and says so when
    // it answers again.
    // The window's own lines and the app's server control's: one control serves every window.
    let log_lines = |c: &Client, prefix: &str| -> Vec<String> {
        c.toast_log.iter().chain(wheel_gpui::view::server::with(|s| s.log.clone()).unwrap_or_default().iter()).filter(|l| l.starts_with(prefix)).cloned().collect()
    };
    let restart_expected = || wheel_gpui::view::server::with(|s| s.watch.restart_expected).unwrap_or(false);
    let control_kind = wheel_gpui::view::server::with(|s| s.control.kind().to_owned()).unwrap_or_default();
    let (word_before, control_log): (String, Vec<String>) = window.update(cx, |c, _, _| (c.server_status_word().word, log_lines(c, "daemon ")))?;
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
            (c.link.clone(), c.link.banner(), restart_expected(), c.toasts().iter().any(|t| t.id == "unreachable"), c.server_status_word().word)
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
    let (expected_after, server_log): (bool, Vec<String>) = window.update(cx, |c, _, _| (restart_expected(), log_lines(c, "server: wheeld")))?;
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
    Ok((Run { bridge, connection, layouts, window, native, markers, output }, server_lifecycle_report, saw_down, down_reason, banner_while_down, typed_after, identities_after, banner_after, final_status))
}

/// The app's one menu bar item, started the first time a step needs it, as the app starts it at launch.
fn app_menu_bar(bridge: &std::rc::Rc<Bridge>, cx: &mut AsyncApp) -> Result<gpui::Entity<wheel_gpui::view::MenuBar>> {
    if let Some(menu) = cx.update(|_| wheel_gpui::view::menu_bar()) {
        return Ok(menu);
    }
    let endpoint = Endpoint::new(&default_url(), &default_token_file())?;
    Ok(cx.update(|cx| wheel_gpui::view::start_menu_bar(cx, bridge, endpoint, true)))
}

/// The menu bar item (wheel-ynjo.7.4.1): its own connection lists every session's spaces, it
/// counts a fixture permission where it waits, its popover draws the cards, New Window opens a
/// window, and closing every window leaves the app running with the item.
#[allow(clippy::type_complexity)]
async fn restore_check(file: &std::path::Path, frames: (&[f32], &[f32]), sessions: (&Option<String>, &String), handles: (gpui::WindowHandle<Client>, gpui::WindowHandle<Client>), little_space: Option<&str>, cx: &mut AsyncApp) -> Result<Value> {
    use wheel_gpui::saved::Saved;
    let file = file.to_path_buf();
    // A little window on window 1's space, saved and restored with the two main windows.
    let little_space = little_space.ok_or_else(|| anyhow::anyhow!("window 1 shows no space"))?.to_owned();
    let little_id = cx.update(|app| handles.0.update(app, |c, _, cx| c.open_little(&little_space, cx)))?.ok_or_else(|| anyhow::anyhow!("no little window opened on {little_space}"))?;
    let little = wheel_gpui::window_handle(&little_id).ok_or_else(|| anyhow::anyhow!("no handle for little window {little_id}"))?;
    let near = |a: &[f32], b: &[f32]| a.len() == 4 && b.len() == 4 && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1.);
    let names = |s: &Saved, session: Option<&str>, frame: &[f32]| s.windows.iter().any(|w| w.session.as_deref() == session && w.frame.is_some_and(|f| near(&f, frame)));
    let mut saved = Saved::load(Some(&file));
    for _ in 0..40 {
        if saved.windows.len() == 2 && names(&saved, sessions.0.as_deref(), frames.0) && names(&saved, Some(sessions.1), frames.1) && saved.littles.iter().any(|l| l.space == little_space && l.frame.is_some()) {
            break;
        }
        tick(cx, 100).await;
        saved = Saved::load(Some(&file));
    }
    ensure!(saved.restore && saved.windows.len() == 2 && names(&saved, sessions.0.as_deref(), frames.0) && names(&saved, Some(sessions.1), frames.1),
        "the saved windows do not name both windows at {frames:?} on {sessions:?}: {saved:?}");
    let little_saved = saved.littles.iter().find(|l| l.space == little_space).cloned().ok_or_else(|| anyhow::anyhow!("the saved windows hold no little window on {little_space}: {saved:?}"))?;
    ensure!(little_saved.frame.is_some(), "the little window was saved with no frame: {little_saved:?}");
    let (bridge, endpoint, hidden) = handles.0.update(cx, |c, _, _| c.opener())?;
    // Every handle `open_saved` returned, measured until each shows its session at its frame.
    async fn settle(opened: &[gpui::WindowHandle<Client>], want: &[(Option<String>, Option<String>, Option<Vec<f32>>)], cx: &mut AsyncApp) -> Result<Vec<Value>> {
        let near = |a: &[f32], b: &[f32]| a.len() == 4 && b.len() == 4 && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1.);
        let mut seen = vec![];
        for _ in 0..80 {
            seen.clear();
            let mut all = true;
            for (handle, (session, want_space, frame)) in opened.iter().zip(want) {
                draw(*handle, None, cx)?;
                let native = handle.update(cx, |c, _, _| c.native())?;
                let at: Vec<f32> = native.call(json!({"op": "frame"}))["frame"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default();
                let (on, space) = shown(*handle, cx)?;
                all &= session.as_ref().is_none_or(|s| on.as_ref() == Some(s)) && want_space.as_ref().is_none_or(|s| space.as_ref() == Some(s)) && frame.as_ref().is_none_or(|f| near(f, &at));
                seen.push(json!({"session": on, "space": space, "frame": at}));
            }
            if all {
                return Ok(seen);
            }
            tick(cx, 100).await;
        }
        anyhow::bail!("the reopened windows never settled: want {want:?}, saw {seen:?}")
    }
    let close_all = |opened: &[gpui::WindowHandle<Client>], cx: &mut AsyncApp| -> Result<()> {
        for handle in opened {
            handle.update(cx, |_, w, _| w.remove_window())?;
        }
        Ok(())
    };
    let plan = saved.launch_plan();
    let opened = cx.update(|app| wheel_gpui::open_saved(app, &bridge, endpoint.clone(), None, hidden, &saved))?;
    ensure!(opened.len() == 3, "opening the saved windows opened {} window(s), not 2 and a little one", opened.len());
    // What each reopened window must show, from the frames this check measured, not from the plan.
    let want: Vec<(Option<String>, Option<String>, Option<Vec<f32>>)> = plan.iter().map(|l| match l {
        wheel_gpui::saved::Launch::Main(w) => {
            let frame = if w.session.as_deref() == Some(sessions.1.as_str()) { frames.1 } else { frames.0 };
            (w.session.clone(), None, Some(frame.to_vec()))
        }
        wheel_gpui::saved::Launch::Little(l) => (None, Some(l.space.clone()), little_saved.frame.map(|f| f.to_vec())),
    }).collect();
    let reopened = settle(&opened, &want, cx).await?;
    close_all(&opened, cx)?;
    // Off: the file says so, and the same launch opens one window on the focused window's session.
    cx.update(|_| wheel_gpui::saved::set_restores(false));
    let off = Saved::load(Some(&file));
    ensure!(!off.restore, "turning Restore windows at launch off left the file at {off:?}");
    let focused_session = off.windows.iter().find(|w| w.focused).and_then(|w| w.session.clone());
    let one = cx.update(|app| wheel_gpui::open_saved(app, &bridge, endpoint, None, hidden, &off))?;
    ensure!(one.len() == 1, "with the setting off, launch opened {} windows", one.len());
    let collapsed = settle(&one, &[(focused_session.clone(), None, None)], cx).await?;
    close_all(&one, cx)?;
    close_all(&[little], cx)?;
    cx.update(|_| wheel_gpui::saved::set_restores(true));
    for _ in 0..10 {
        tick(cx, 100).await;
    }
    let after = Saved::load(Some(&file));
    ensure!(after.restore && after.windows.len() == 2 && after.littles.is_empty(), "after closing the reopened windows the file holds {after:?}");
    Ok(json!({
        "saved": saved.windows.iter().map(|w| json!({"session": w.session, "active": w.active, "frame": w.frame, "focused": w.focused})).collect::<Vec<_>>(),
        "saved_little": {"space": little_saved.space, "frame": little_saved.frame},
        "reopened": reopened, "off": {"restore": off.restore, "focused_session": focused_session, "opened": collapsed}, "file_after": after.windows.len(),
    }))
}
async fn menu_bar_check(bridge: &std::rc::Rc<Bridge>, agent: &str, connection: &mut Connection, layouts: &mut usize, window: gpui::WindowHandle<Client>, output: &std::path::Path, cx: &mut AsyncApp) -> Result<Value> {
    let menu = app_menu_bar(bridge, cx)?;
    let state = |cx: &mut AsyncApp| menu.update(cx, |m, cx| m.state_json(cx));
    let mut menu_state = Value::Null;
    for _ in 0..100 {
        pump(connection, 100, layouts, window, cx).await?;
        menu_state = state(cx);
        if menu_state["up"] == true && menu_state["cards"].as_array().is_some_and(|c| !c.is_empty()) {
            break;
        }
    }
    ensure!(menu_state["up"] == true, "the menu bar's connection never came up: {menu_state}");
    ensure!(menu_state["item"]["status_item"] == true && menu_state["item"]["in_menu_bar"] == false && menu_state["item"]["template"] == true,
        "the status item is missing, in the menu bar during a hidden check, or not a template image: {}", menu_state["item"]);
    // Every session's spaces, in wheeld's order, as the window's own layout lists them.
    let (sessions, _) = window.update(cx, |c, _, _| c.layout_lists())?;
    let listed: Vec<(String, Vec<String>)> = menu_state["cards"].as_array().unwrap().iter()
        .map(|c| (c["session"].as_str().unwrap_or("").to_owned(), c["rows"].as_array().unwrap().iter().map(|r| r["space"].as_str().unwrap_or("").to_owned()).collect())).collect();
    let want: Vec<(String, Vec<String>)> = sessions.iter().map(|s| (s.id.clone(), s.spaces.clone())).collect();
    ensure!(listed == want, "the popover's cards list {listed:?}, wheeld's sessions hold {want:?}");
    // The badge counts the spaces in sessions no window shows and in ownerless little windows.
    let spaces_now = window.update(cx, |c, _, _| c.layout_lists().1)?;
    let background = wheel_gpui::windows::with(|r| wheel_gpui::windows::ls(r, &sessions, &spaces_now)["background"].as_array().map(|a| a.len()).unwrap_or(0)
        + r.littles.iter().filter(|l| l.owner.is_none()).count());
    ensure!(menu_state["item"]["badge"].as_u64() == Some(background as u64), "the item's badge reads {}, the windows leave {background} spaces unshown", menu_state["item"]["badge"]);
    let badge_before = menu_state["item"]["badge"].clone();
    // A permission the fixture agent raises counts in the row of the space holding it.
    connection.send.send(wheel_gpui::daemon::frame(&ClientMsg::AgentSend { req: None, agent: agent.to_owned(), text: "permission".into() }))?;
    let mut waiting_state = Value::Null;
    for _ in 0..300 {
        pump(connection, 100, layouts, window, cx).await?;
        waiting_state = state(cx);
        // Both connections hear the request: the menu bar's own and the window's.
        if waiting_state["waiting"].as_u64().unwrap_or(0) > 0 && window.update(cx, |c, _, _| !c.perms.is_empty())? {
            break;
        }
    }
    let fixture_waiting = window.update(cx, |c, _, _| c.perms.len())?;
    let row_waiting: u64 = waiting_state["cards"].as_array().unwrap().iter().flat_map(|c| c["rows"].as_array().unwrap().iter()).map(|r| r["waiting"].as_u64().unwrap_or(0)).sum();
    ensure!(fixture_waiting > 0 && waiting_state["waiting"].as_u64() == Some(fixture_waiting as u64) && row_waiting == fixture_waiting as u64,
        "the fixture has {fixture_waiting} permission(s) waiting; the menu bar holds {} and its rows count {row_waiting}", waiting_state["waiting"]);
    // The popover opens as the item's click opens it, and draws the cards.
    let clicked = cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
    let mut open_state = Value::Null;
    for _ in 0..50 {
        tick(cx, 100).await;
        open_state = state(cx);
        if open_state["popover"]["open"] == true {
            break;
        }
    }
    ensure!(open_state["popover"]["open"] == true, "pressing the status item ({clicked}) opened no popover: {}", open_state["popover"]);
    let popover = menu.update(cx, |m, _| m.popover()).ok_or_else(|| anyhow::anyhow!("no popover handle"))?;
    let popover_any: gpui::AnyWindowHandle = popover.into();
    // The popover fits the height its content measures (fit_window), keeping its top edge.
    open_state = popover_settle(popover_any, &menu, cx, popover_fitted).await;
    ensure!(popover_fitted(&open_state), "the popover never fitted its content: {}", open_state["popover"]);
    let picture = output.join("client-menubar-popover.png");
    let drawn = popover_any.update(cx, |_, w, cx| -> Result<(u32, u32)> {
        w.draw(cx).clear();
        let image = w.render_to_image()?;
        image.save(&picture)?;
        Ok((image.width(), image.height()))
    })??;
    let frame_h = open_state["popover"]["frame"][3].as_f64().unwrap_or(0.);
    ensure!((frame_h - open_state["height"].as_f64().unwrap_or(-1.)).abs() < 1., "the popover is {frame_h} tall, its closed server line and cards need {}", open_state["height"]);
    let server_line = server_line_check(popover_any, &menu, window, output, frame_h, cx).await?;
    let open_state = state(cx);
    let frame_h = open_state["popover"]["frame"][3].as_f64().unwrap_or(0.);
    // New Window: a press and release on its row, as the person clicks it.
    let windows_before = wheel_gpui::windows::with(|r| r.windows.len());
    let new_y = frame_h as f32 - wheel_gpui::menubar::PAD - 2. * wheel_gpui::menubar::ROW - wheel_gpui::menubar::ROW_GAP + wheel_gpui::menubar::ROW / 2.;
    let at = gpui::point(gpui::px(wheel_gpui::menubar::WIDTH / 2.), gpui::px(new_y));
    popover_any.update(cx, |_, w, cx| {
        w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx);
        w.draw(cx).clear();
        w.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false }), cx);
        w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1 }), cx);
    })?;
    let mut windows_after = windows_before;
    for _ in 0..50 {
        tick(cx, 100).await;
        windows_after = wheel_gpui::windows::with(|r| r.windows.len());
        if windows_after > windows_before {
            break;
        }
    }
    ensure!(windows_after == windows_before + 1, "New Window left {windows_after} windows open, {windows_before} before");
    let after_new = state(cx);
    let popover_window_open = popover_any.update(cx, |_, _, _| ()).is_ok();
    ensure!(after_new["popover"]["open"] == false && !popover_window_open, "the popover stayed open after New Window: handle held {}, window open {popover_window_open}", after_new["popover"]["open"]);
    // The pairing row and Settings… (wheel-ynjo.7.4.3, 7.4.6): the popover opened again copies the
    // code through the menu bar's own connection and opens no window, and Settings… opens the
    // focused window's Settings and closes the popover.
    let pairing = {
        cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
        let mut reopened = Value::Null;
        for _ in 0..50 {
            tick(cx, 100).await;
            reopened = state(cx);
            if reopened["popover"]["open"] == true {
                break;
            }
        }
        ensure!(reopened["popover"]["open"] == true, "the popover did not open again: {}", reopened["popover"]);
        let popover: gpui::AnyWindowHandle = menu.update(cx, |m, _| m.popover()).ok_or_else(|| anyhow::anyhow!("no popover handle"))?.into();
        let shown = popover_settle(popover, &menu, cx, |s| popover_fitted(s) && s["popover"]["controls"]["pairing-copy"].is_array() && s["pairing"]["disabled"].is_null()).await;
        ensure!(popover_fitted(&shown) && shown["pairing"]["disabled"].is_null() && shown["popover"]["controls"]["pairing-copy"].is_array()
            && shown["popover"]["controls"]["settings"].is_array() && shown["popover"]["controls"]["pairing-qr"].is_array(),
            "the pairing row reads disabled {} with controls {}", shown["pairing"]["disabled"], shown["popover"]["controls"]);
        let row_is_address = shown["pairing"]["row"].as_str().is_some_and(|r| r.starts_with("wss://"));
        ensure!(row_is_address, "the pairing row's second line is not an address");
        let height = shown["popover"]["frame"][3].as_f64().unwrap_or(0.);
        ensure!((height - shown["height"].as_f64().unwrap_or(-1.)).abs() < 1., "the popover is {height} tall, its rows need {}", shown["height"]);
        let target = cx.update(|cx| wheel_gpui::view::focused_client(cx)).ok_or_else(|| anyhow::anyhow!("no focused window"))?;
        let copied_before = wheel_gpui::view::copied_pairing_code();
        let windows_before_copy = wheel_gpui::windows::with(|r| r.windows.len());
        let picture = output.join("client-menubar-pairing.png");
        popover_picture(popover, &picture, cx)?;
        popover_press(popover, &shown, "pairing-copy", cx)?;
        let mut copied = None;
        for _ in 0..100 {
            tick(cx, 100).await;
            copied = wheel_gpui::view::copied_pairing_code();
            if copied.is_some() && copied != copied_before {
                break;
            }
        }
        ensure!(copied.is_some() && copied != copied_before, "Copy pairing code in the popover copied nothing (had a code before: {})", copied_before.is_some());
        ensure!(wheel_gpui::windows::with(|r| r.windows.len()) == windows_before_copy, "Copy pairing code opened a window");
        let copy_line = menu_log_line(&menu, "pairing code copied", cx);
        ensure!(copy_line.as_deref().is_some_and(|l| l.contains("\"copied\":true")), "the bridge did not write the concealed item: {copy_line:?}");
        let after_copy = popover_settle(popover, &menu, cx, |s| s["popover"]["controls"]["settings"].is_array()).await;
        ensure!(after_copy["popover"]["open"] == true, "the popover closed after Copy");
        let settings_before = target.update(cx, |c, _, _| c.settings_open)?;
        ensure!(!settings_before, "the focused window's Settings was open before Settings…");
        popover_press(popover, &after_copy, "settings", cx)?;
        let mut opened = false;
        for _ in 0..50 {
            tick(cx, 100).await;
            opened = target.update(cx, |c, _, _| c.settings_open)?;
            if opened && state(cx)["popover"]["open"] == false {
                break;
            }
        }
        let popover_gone = popover.update(cx, |_, _, _| ()).is_err();
        ensure!(opened && popover_gone && state(cx)["popover"]["open"] == false, "Settings… opened Settings {opened}, closed the popover {popover_gone}");
        target.update(cx, |c, _, cx| {
            c.close_settings();
            cx.notify();
        })?;
        // Show QR code: the focused window's pairing window opens and the popover closes.
        cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
        let mut again = Value::Null;
        for _ in 0..50 {
            tick(cx, 100).await;
            again = state(cx);
            if again["popover"]["open"] == true {
                break;
            }
        }
        ensure!(again["popover"]["open"] == true, "the popover did not open for Show QR code: {}", again["popover"]);
        let popover: gpui::AnyWindowHandle = menu.update(cx, |m, _| m.popover()).ok_or_else(|| anyhow::anyhow!("no popover handle"))?.into();
        let ready = popover_settle(popover, &menu, cx, |s| popover_fitted(s) && s["popover"]["controls"]["pairing-qr"].is_array()).await;
        ensure!(ready["popover"]["controls"]["pairing-qr"].is_array(), "no Show QR code control: {}", ready["popover"]["controls"]);
        popover_press(popover, &ready, "pairing-qr", cx)?;
        let mut qr_open = false;
        for _ in 0..100 {
            tick(cx, 100).await;
            qr_open = target.update(cx, |c, _, _| c.pairing.open && !c.pairing.window_tokens.is_empty())?;
            if qr_open && state(cx)["popover"]["open"] == false {
                break;
            }
        }
        let qr_popover_gone = popover.update(cx, |_, _, _| ()).is_err();
        ensure!(qr_open && qr_popover_gone, "Show QR code opened the pairing window with a code {qr_open}, closed the popover {qr_popover_gone}");
        target.update(cx, |c, _, cx| {
            c.close_pairing_window();
            cx.notify();
        })?;
        json!({"row_is_address": row_is_address, "height": height, "copied": true, "copy_line": copy_line, "popover_stayed": true, "settings_opened": opened, "popover_closed": popover_gone,
            "qr_window_opened": qr_open, "qr_popover_closed": qr_popover_gone, "picture": picture})
    };
    let space_rows = space_rows_check(&menu, output, cx).await?;
    // A Dock click with every window minimised (wheel-ynjo.7.6): AppKit finds no visible window and
    // GPUI calls `reopen` though windows are open; it opens one on the first session no window shows.
    let windows_before_dock = wheel_gpui::windows::with(|r| r.windows.len());
    let (sessions_now, _) = window.update(cx, |c, _, _| c.layout_lists())?;
    let dock_session = wheel_gpui::windows::with(|r| {
        sessions_now.iter().find(|s| !r.shows_session(&s.id)).map(|s| s.id.clone()).or_else(|| r.focused().and_then(|w| w.session.clone()))
    });
    cx.update(|cx| wheel_gpui::view::reopen(cx));
    let mut windows_after_dock = windows_before_dock;
    for _ in 0..50 {
        tick(cx, 100).await;
        windows_after_dock = wheel_gpui::windows::with(|r| r.windows.len());
        if windows_after_dock > windows_before_dock {
            break;
        }
    }
    let dock_opened = wheel_gpui::windows::with(|r| r.windows.last().and_then(|w| w.session.clone()));
    ensure!(windows_after_dock == windows_before_dock + 1 && dock_opened == dock_session,
        "a Dock click with {windows_before_dock} window(s) open and none visible left {windows_after_dock} open, the newest on {dock_opened:?}; wanted one more, on {dock_session:?}");
    // Answer the permission, so the fixture is left as it was.
    let answered = window.update(cx, |c, _, cx| {
        let perm = c.perms.first().cloned();
        if let Some(perm) = &perm && let Some(option) = perm.options.iter().find(|o| o.kind.starts_with("reject")).or(perm.options.first()) {
            c.answer_perm(&perm.id, &option.id);
        }
        cx.notify();
        perm.map(|p| p.id)
    })?;
    // Closing every window leaves the app running with its item, its connection and its counts.
    let handles: Vec<gpui::AnyWindowHandle> = cx.update(|cx| cx.windows());
    for handle in &handles {
        let _ = handle.update(cx, |_, w, _| w.remove_window());
    }
    for _ in 0..50 {
        tick(cx, 100).await;
        if cx.update(|cx| cx.windows().is_empty()) && state(cx)["waiting"] == 0 {
            break;
        }
    }
    let open_windows = cx.update(|cx| cx.windows().len());
    let still = state(cx);
    ensure!(open_windows == 0 && still["item"]["status_item"] == true && still["up"] == true,
        "after closing every window: {open_windows} windows, the menu bar reads {still}");
    let every_space: usize = still["cards"].as_array().unwrap().iter().map(|c| c["rows"].as_array().map(|r| r.len()).unwrap_or(0)).sum();
    ensure!(still["item"]["badge"].as_u64() == Some(every_space as u64), "with no window, every space ({every_space}) is unshown; the badge reads {}", still["item"]["badge"]);
    let closed_copy = closed_copy_check(&menu, cx).await?;
    Ok(json!({"item": menu_state["item"], "cards": menu_state["cards"], "badge_before": badge_before, "background_counted": background,
        "fixture_waiting": fixture_waiting, "rows_waiting": row_waiting, "dock_while_waiting": waiting_state["item"]["dock"], "answered": answered,
        "popover": {"frame": open_state["popover"]["frame"], "height": open_state["height"], "picture": picture, "pixels": [drawn.0, drawn.1]},
        "new_window": {"before": windows_before, "after": windows_after, "popover_closed": after_new["popover"]["open"] == false},
        "pairing": pairing,
        "space_rows": space_rows,
        "server_line": server_line,
        "dock_reopen": {"before": windows_before_dock, "after": windows_after_dock, "session": dock_opened, "wanted": dock_session},
        "all_closed": {"windows": open_windows, "running": true, "badge": still["item"]["badge"], "label": still["item"]["label"], "spaces": every_space, "copy": closed_copy},
        "log": still["log"]}))
}

/// The newest line of the menu bar's log that starts with `prefix`.
fn menu_log_line(menu: &gpui::Entity<wheel_gpui::view::MenuBar>, prefix: &str, cx: &mut AsyncApp) -> Option<String> {
    menu.update(cx, |m, cx| m.state_json(cx))["log"].as_array().and_then(|l| l.iter().rev().filter_map(|l| l.as_str()).find(|l| l.starts_with(prefix)).map(str::to_owned))
}

/// How many lines of the menu bar's log start with `prefix`.
fn menu_log_count(menu: &gpui::Entity<wheel_gpui::view::MenuBar>, prefix: &str, cx: &mut AsyncApp) -> usize {
    menu.update(cx, |m, cx| m.state_json(cx))["log"].as_array().map(|l| l.iter().filter_map(|l| l.as_str()).filter(|l| l.starts_with(prefix)).count()).unwrap_or(0)
}

/// Copy pairing code with every window closed (wheel-ynjo.7.4.6): the popover's button copies
/// through the menu bar's own connection, no window opens, and the bridge writes the concealed item.
async fn closed_copy_check(menu: &gpui::Entity<wheel_gpui::view::MenuBar>, cx: &mut AsyncApp) -> Result<Value> {
    let state = |cx: &mut AsyncApp| menu.update(cx, |m, cx| m.state_json(cx));
    ensure!(wheel_gpui::windows::with(|r| r.windows.len()) == 0, "a window is open before the copy");
    cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
    let mut opened = Value::Null;
    for _ in 0..50 {
        tick(cx, 100).await;
        opened = state(cx);
        if opened["popover"]["open"] == true {
            break;
        }
    }
    ensure!(opened["popover"]["open"] == true, "the popover did not open with no window: {}", opened["popover"]);
    let popover: gpui::AnyWindowHandle = menu.update(cx, |m, _| m.popover()).ok_or_else(|| anyhow::anyhow!("no popover handle"))?.into();
    let shown = popover_settle(popover, menu, cx, |s| popover_fitted(s) && s["popover"]["controls"]["pairing-copy"].is_array() && s["pairing"]["disabled"].is_null()).await;
    ensure!(shown["pairing"]["disabled"].is_null() && shown["popover"]["controls"]["pairing-copy"].is_array(), "with no window the pairing row reads disabled {}", shown["pairing"]["disabled"]);
    let before = wheel_gpui::view::copied_pairing_code();
    let copies_before = menu_log_count(menu, "pairing code copied", cx);
    // Two presses before wheeld answers are one copy; the second is logged and ignored.
    popover_press(popover, &shown, "pairing-copy", cx)?;
    popover_press(popover, &shown, "pairing-copy", cx)?;
    let mut copied = None;
    for _ in 0..100 {
        tick(cx, 100).await;
        copied = wheel_gpui::view::copied_pairing_code();
        if copied.is_some() && copied != before {
            break;
        }
    }
    ensure!(copied.is_some() && copied != before, "Copy pairing code with no window copied nothing (had a code before: {})", before.is_some());
    let windows = wheel_gpui::windows::with(|r| r.windows.len());
    ensure!(windows == 0, "Copy pairing code with no window opened {windows} window(s)");
    tick(cx, 500).await;
    let line = menu_log_line(menu, "pairing code copied", cx);
    ensure!(line.as_deref().is_some_and(|l| l.contains("\"copied\":true")), "the bridge did not write the concealed item: {line:?}");
    let copies = menu_log_count(menu, "pairing code copied", cx) - copies_before;
    let ignored = menu_log_count(menu, "popover: pairing copy already in flight", cx);
    ensure!(copies == 1 && ignored >= 1, "two quick presses made {copies} copies and {ignored} ignored line(s); the menu logged {:?}", menu.update(cx, |m, cx| m.state_json(cx))["log"]);
    Ok(json!({"windows": windows, "copied": true, "line": line, "copies_from_two_presses": copies, "second_press_ignored": ignored}))
}

/// The popover's rows and cards act (wheel-ynjo.7.4.4): a click on a row's name shows its space in
/// a window, the hovered row's Little Window button opens it in a little one, a right-click opens
/// the space's menu (Rename runs in the window holding it), and a card's New Window and Close
/// buttons open a window on the session and ask before closing it.
async fn space_rows_check(menu: &gpui::Entity<wheel_gpui::view::MenuBar>, output: &std::path::Path, cx: &mut AsyncApp) -> Result<Value> {
    let state = |cx: &mut AsyncApp| menu.update(cx, |m, cx| m.state_json(cx));
    cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
    let mut reopened = Value::Null;
    for _ in 0..50 {
        tick(cx, 100).await;
        reopened = state(cx);
        if reopened["popover"]["open"] == true {
            break;
        }
    }
    ensure!(reopened["popover"]["open"] == true, "the popover did not open for the rows: {}", reopened["popover"]);
    let popover: gpui::AnyWindowHandle = menu.update(cx, |m, _| m.popover()).ok_or_else(|| anyhow::anyhow!("no popover handle"))?.into();
    let ready = popover_settle(popover, &menu, cx, |s| popover_fitted(s) && s["popover"]["controls"].as_object().is_some_and(|c| c.keys().any(|k| k.starts_with("row-")))).await;
    let rows: Vec<(String, String, bool)> = ready["cards"].as_array().unwrap().iter()
        .flat_map(|c| c["rows"].as_array().unwrap().iter().map(move |r| (c["session"].as_str().unwrap_or("").to_owned(), r["space"].as_str().unwrap_or("").to_owned(), r["shown"] == true && r["in_little"] == false)))
        .collect();
    let (session, space) = rows.iter().find(|(_, _, shown)| !shown).map(|(n, s, _)| (n.clone(), s.clone()))
        .ok_or_else(|| anyhow::anyhow!("every row's space is shown already: {:?}", rows))?;
    // A click on its name: a window of its session shows it.
    popover_press(popover, &ready, &format!("row-{space}"), cx)?;
    let mut shown_in = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        shown_in = wheel_gpui::windows::with(|r| r.windows.iter().find(|w| w.active.as_deref() == Some(space.as_str())).map(|w| w.id.clone()));
        if shown_in.is_some() {
            break;
        }
    }
    ensure!(shown_in.is_some(), "clicking the row of {space} showed it in no window");
    let after_click = popover_settle(popover, &menu, cx, |s| s["cards"].as_array().unwrap().iter().flat_map(|c| c["rows"].as_array().unwrap().iter()).any(|r| r["space"] == space.as_str() && r["shown"] == true)).await;
    let row_shown = after_click["cards"].as_array().unwrap().iter().flat_map(|c| c["rows"].as_array().unwrap().iter()).any(|r| r["space"] == space.as_str() && r["shown"] == true);
    ensure!(row_shown, "the row of {space} has no shown dot after its window showed it");
    // The pointer over the row shows its Little Window button; pressing it opens a little window.
    popover_hover(popover, &after_click, &format!("row-{space}"), cx)?;
    let little_name = format!("little-{space}");
    let hovered = popover_settle(popover, &menu, cx, |s| s["popover"]["controls"][little_name.as_str()].is_array()).await;
    ensure!(hovered["popover"]["controls"][little_name.as_str()].is_array(), "hovering the row of {space} showed no Little Window button: {}", hovered["popover"]["controls"]);
    let hover_picture = output.join("client-menubar-row-hover.png");
    popover_picture(popover, &hover_picture, cx)?;
    popover_press(popover, &hovered, &little_name, cx)?;
    let mut little = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        little = wheel_gpui::windows::with(|r| r.little_showing(&space).map(|l| (l.id.clone(), l.owner.clone())));
        if little.is_some() {
            break;
        }
    }
    ensure!(little.is_some(), "the Little Window button opened no little window for {space}");
    let dimmed_state = popover_settle(popover, &menu, cx, |s| s["cards"].as_array().unwrap().iter().flat_map(|c| c["rows"].as_array().unwrap().iter()).any(|r| r["space"] == space.as_str() && r["in_little"] == true)).await;
    let dimmed_row = dimmed_state["cards"].as_array().unwrap().iter().flat_map(|c| c["rows"].as_array().unwrap().iter()).find(|r| r["space"] == space.as_str()).cloned().unwrap_or(Value::Null);
    let owned = little.as_ref().is_some_and(|(_, owner)| owner.is_some());
    ensure!(dimmed_row["in_little"] == true && dimmed_row["dimmed"] == owned, "the row of {space} in a little window (owned {owned}) reads {dimmed_row}");
    // A right-click opens the space's menu; Rename Space… opens the rename in the window holding it.
    popover_right_press(popover, &dimmed_state, &format!("row-{space}"), cx)?;
    let menu_open = popover_settle(popover, &menu, cx, |s| s["space_menu"]["space"] == space.as_str()).await;
    let built = menu_open["space_menu"]["built"].clone();
    let titles: Vec<String> = built.as_array().map(|a| a.iter().filter_map(|i| i["title"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
    ensure!(menu_open["space_menu"]["space"] == space.as_str() && titles.iter().any(|t| t == "Go to Little Window") && titles.iter().any(|t| t == "Rename Space…"),
        "the right-click opened the menu {} for {space}", menu_open["space_menu"]);
    // The window holding it (WindowLayout.window(holding:)): the focused one when it shows the session.
    let holder = wheel_gpui::windows::with(|r| {
        let holds = |w: &&wheel_gpui::windows::Record| w.session.as_deref() == Some(session.as_str());
        r.focused().filter(holds).or_else(|| r.windows.iter().find(holds)).map(|w| w.id.clone())
    }).ok_or_else(|| anyhow::anyhow!("no window shows session {session}"))?;
    cx.update(|cx| menu.update(cx, |m, _| m.bridge_call(json!({"op": "choose_context_menu", "item": "rename"}))));
    let holder_handle = cx.update(|_| wheel_gpui::window_handle(&holder)).ok_or_else(|| anyhow::anyhow!("no handle for window {holder}"))?;
    let mut renaming = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        renaming = holder_handle.update(cx, |c, _, _| c.rename.as_ref().map(|r| r.space.clone()))?;
        if renaming.is_some() {
            break;
        }
    }
    ensure!(renaming.as_deref() == Some(space.as_str()), "Rename Space… from the row's menu opened the rename for {renaming:?} in window {holder}");
    holder_handle.update(cx, |c, _, cx| {
        c.cancel_rename();
        cx.notify();
    })?;
    // The card's New Window: one more window, on the card's session.
    let ready = popover_settle(popover, &menu, cx, |s| s["popover"]["controls"][format!("new-window-{session}").as_str()].is_array()).await;
    let windows_before = wheel_gpui::windows::with(|r| r.windows.len());
    popover_press(popover, &ready, &format!("new-window-{session}"), cx)?;
    let mut opened_on = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        opened_on = wheel_gpui::windows::with(|r| (r.windows.len() > windows_before).then(|| r.windows.last().and_then(|w| w.session.clone())).flatten());
        if opened_on.is_some() {
            break;
        }
    }
    ensure!(opened_on.as_deref() == Some(session.as_str()), "the card's New Window opened a window on {opened_on:?}, wanted {session}");
    // The card's Close asks in an alert; Cancel keeps the session.
    let ready = popover_settle(popover, &menu, cx, |s| s["popover"]["controls"][format!("close-session-{session}").as_str()].is_array()).await;
    popover_press(popover, &ready, &format!("close-session-{session}"), cx)?;
    let mut asked = Value::Null;
    for _ in 0..50 {
        tick(cx, 100).await;
        asked = cx.update(|cx| menu.update(cx, |m, _| m.bridge_call(json!({"op": "state"}))))["alert"].clone();
        if asked["id"] == format!("close-session:{session}").as_str() {
            break;
        }
    }
    ensure!(asked["id"] == format!("close-session:{session}").as_str() && asked["informative"].as_str().is_some_and(|i| i.ends_with("Every window showing it closes.")),
        "the card's Close asked {asked}");
    cx.update(|cx| menu.update(cx, |m, _| m.bridge_call(json!({"op": "answer_alert", "button": 1}))));
    let answered_line = format!("alert close-session:{session} answered with button 1");
    let cancelled = popover_settle(popover, &menu, cx, |s| s["log"].as_array().is_some_and(|l| l.iter().any(|x| x == answered_line.as_str()))).await;
    let kept = cancelled["cards"].as_array().unwrap().iter().any(|c| c["session"] == session.as_str());
    ensure!(kept && cancelled["log"].as_array().is_some_and(|l| l.iter().any(|x| x == answered_line.as_str())), "Cancel in the close question: answered {}, session kept {kept}",
        cancelled["log"].as_array().is_some_and(|l| l.iter().any(|x| x == answered_line.as_str())));
    let picture = output.join("client-menubar-rows.png");
    popover_picture(popover, &picture, cx)?;
    cx.update(|cx| menu.update(cx, |m, cx| m.toggle_popover(None, cx)));
    Ok(json!({"space": space, "session": session, "click_showed_in": shown_in, "little": little.map(|(id, owner)| json!({"id": id, "owner": owner})), "dimmed": dimmed_row["dimmed"],
        "menu": titles, "rename_in": holder, "new_window_on": opened_on, "close_asked": asked["message"], "close_cancelled_kept": kept, "pictures": [hover_picture, picture]}))
}

/// The popover's frame is the height its content measured.
fn popover_fitted(s: &Value) -> bool {
    let (frame, content) = (s["popover"]["frame"][3].as_f64().unwrap_or(0.), s["popover"]["content_height"].as_f64().unwrap_or(-1.));
    content > 0. && (frame - content).abs() < 1.
}

/// Draw the popover and wait, up to 5 s, until the menu bar's state passes `until`; the last state.
async fn popover_settle(popover: gpui::AnyWindowHandle, menu: &gpui::Entity<wheel_gpui::view::MenuBar>, cx: &mut AsyncApp, until: impl Fn(&Value) -> bool) -> Value {
    let mut s = Value::Null;
    for _ in 0..50 {
        let _ = popover.update(cx, |_, w, cx| {
            w.draw(cx).clear();
        });
        tick(cx, 100).await;
        s = menu.update(cx, |m, cx| m.state_json(cx));
        if until(&s) {
            break;
        }
    }
    s
}

/// Press and release the popover's control `name` at its centre, as the person clicks it.
fn popover_press(popover: gpui::AnyWindowHandle, s: &Value, name: &str, cx: &mut AsyncApp) -> Result<()> {
    let c = &s["popover"]["controls"][name];
    let r: Vec<f32> = c.as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default();
    ensure!(r.len() == 4, "the popover has no {name} control: {}", s["popover"]["controls"]);
    let at = gpui::point(gpui::px(r[0] + r[2] / 2.), gpui::px(r[1] + r[3] / 2.));
    popover.update(cx, |_, w, cx| {
        w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx);
        w.draw(cx).clear();
        w.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false }), cx);
        w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1 }), cx);
    })?;
    Ok(())
}

/// Move the pointer to the centre of the popover's control `name`, as the person hovers it.
fn popover_hover(popover: gpui::AnyWindowHandle, s: &Value, name: &str, cx: &mut AsyncApp) -> Result<()> {
    let at = popover_centre(s, name)?;
    popover.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx);
    })?;
    Ok(())
}

/// Right-press the popover's control `name` at its centre, as the person opens its menu.
fn popover_right_press(popover: gpui::AnyWindowHandle, s: &Value, name: &str, cx: &mut AsyncApp) -> Result<()> {
    let at = popover_centre(s, name)?;
    popover.update(cx, |_, w, cx| {
        w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx);
        w.draw(cx).clear();
        w.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Right, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false }), cx);
        w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Right, position: at, modifiers: Default::default(), click_count: 1 }), cx);
    })?;
    Ok(())
}

fn popover_centre(s: &Value, name: &str) -> Result<gpui::Point<gpui::Pixels>> {
    let r: Vec<f32> = s["popover"]["controls"][name].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default();
    ensure!(r.len() == 4, "the popover has no {name} control: {}", s["popover"]["controls"]);
    Ok(gpui::point(gpui::px(r[0] + r[2] / 2.), gpui::px(r[1] + r[3] / 2.)))
}

fn popover_picture(popover: gpui::AnyWindowHandle, path: &std::path::Path, cx: &mut AsyncApp) -> Result<()> {
    popover.update(cx, |_, w, cx| -> Result<()> {
        w.draw(cx).clear();
        w.render_to_image()?.save(path)?;
        Ok(())
    })?
}

/// The menu bar's server line and the status item's right-click menu (wheel-ynjo.7.4.2), over the
/// fixture wheeld, whose control is `WHEEL_SERVER_STOP` and `WHEEL_SERVER_START` (each touches a
/// file and leaves wheeld running): the word, the details opened, Stop Server's question cancelled
/// and then answered, Start Server, the right-click menu stopped and running, Stop Server… asking
/// in an alert that is cancelled, and Restart Server from the menu running both commands.
async fn server_line_check(popover: gpui::AnyWindowHandle, menu: &gpui::Entity<wheel_gpui::view::MenuBar>, window: gpui::WindowHandle<Client>, output: &std::path::Path, closed_height: f64, cx: &mut AsyncApp) -> Result<Value> {
    let server_log = || wheel_gpui::view::server::with(|s| s.log.clone()).unwrap_or_default();
    let bridge_state = |cx: &mut AsyncApp| menu.update(cx, |m, _| m.bridge_call(json!({"op": "state"})));
    let (stop_file, start_file) = (output.join("server.stop"), output.join("server.start"));
    let closed = menu.update(cx, |m, cx| m.state_json(cx));
    let word = closed["server"]["word"].as_str().unwrap_or_default().to_owned();
    ensure!(word.starts_with("Running") && closed["server"]["colour"] == "30d158" && closed["server"]["expanded"] == false,
        "over the fixture wheeld the server line reads {}", closed["server"]);
    // Pressing the line opens the details, and the popover grows to hold them.
    popover_press(popover, &closed, "server", cx)?;
    let open = popover_settle(popover, menu, cx, |s| s["server"]["expanded"] == true && popover_fitted(s)).await;
    let details: Vec<String> = open["server"]["details"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let open_height = open["popover"]["frame"][3].as_f64().unwrap_or(0.);
    ensure!(open["server"]["expanded"] == true && popover_fitted(&open) && open_height > closed_height, "the details did not open: {} tall, {closed_height} closed; {}", open_height, open["popover"]);
    ensure!(details.iter().any(|l| l.starts_with("wheeld up ") && l.ends_with(" spaces")) && details.last().is_some_and(|l| l.contains("paired")),
        "the details read {details:?}");
    ensure!(open["popover"]["controls"]["stop"].is_array(), "no Stop Server under the details: {}", open["popover"]["controls"]);
    let picture_open = output.join("client-menubar-server.png");
    popover_picture(popover, &picture_open, cx)?;
    // Stop Server asks first; Cancel leaves it running.
    popover_press(popover, &open, "stop", cx)?;
    let asking = popover_settle(popover, menu, cx, |s| s["server"]["confirming"] == true && s["popover"]["controls"]["confirm-stop"].is_array()).await;
    ensure!(asking["server"]["confirming"] == true && asking["popover"]["controls"]["cancel"].is_array(), "Stop Server asked nothing: {}", asking["server"]);
    let picture_question = output.join("client-menubar-stop-question.png");
    popover_picture(popover, &picture_question, cx)?;
    popover_press(popover, &asking, "cancel", cx)?;
    let cancelled = popover_settle(popover, menu, cx, |s| s["server"]["confirming"] == false).await;
    ensure!(cancelled["server"]["confirming"] == false && cancelled["server"]["stopped_by_menu"] == false, "Cancel did not take the question back: {}", cancelled["server"]);
    // Asked again and answered: the stop command runs and the line reads Stopped.
    let _ = std::fs::remove_file(&stop_file);
    popover_press(popover, &cancelled, "stop", cx)?;
    let asking = popover_settle(popover, menu, cx, |s| s["popover"]["controls"]["confirm-stop"].is_array() && s["server"]["confirming"] == true).await;
    popover_press(popover, &asking, "confirm-stop", cx)?;
    let stopped = popover_settle(popover, menu, cx, |s| s["server"]["stopped_by_menu"] == true && s["popover"]["controls"]["start"].is_array()).await;
    let stopped_details = stopped["server"]["details"].clone();
    ensure!(stopped["server"]["stopped_by_menu"] == true && stopped["server"]["word"] == "Stopped" && stop_file.exists()
        && stopped_details[0].as_str().is_some_and(|l| l.starts_with("Stopped from this menu.")), "Stop Server ended with {} (stop command ran: {})", stopped["server"], stop_file.exists());
    let window_word = window.update(cx, |c, _, _| c.server_status_word().word)?;
    ensure!(window_word == "Stopped", "the window's word reads {window_word} after Stop Server");
    // The window says the server is stopped in place of its grid (ServerStoppedNotice).
    let window_notice = |cx: &mut AsyncApp| -> Result<Value> {
        draw(window, None, cx)?;
        Ok(window.update(cx, |c, _, _| c.state_json()["server"]["notice"].clone())?)
    };
    let mut notice = Value::Null;
    for _ in 0..50 {
        notice = window_notice(cx)?;
        if notice["grid_drawn"] == 0 {
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(notice["title"] == "Wheel Server is stopped" && notice["hint"] == "Start it from the Wheel item in the menu bar." && notice["grid_drawn"] == 0,
        "the window shows {notice} while the server is stopped from the menu");
    let picture_notice = output.join("client-server-stopped-notice.png");
    draw(window, Some(picture_notice.clone()), cx)?;
    let picture_stopped = output.join("client-menubar-stopped.png");
    popover_picture(popover, &picture_stopped, cx)?;
    // The right-click menu while stopped: Start Server first.
    menu.update(cx, |m, _| m.bridge_call(json!({"op": "press_status_item", "button": "right"})));
    let menu_stopped = popover_settle(popover, menu, cx, |s| s["status_menu"]["menu"][0] == "Start Server").await["status_menu"]["menu"].clone();
    ensure!(menu_stopped == json!(["Start Server", "Restart Server", "-", "Quit Wheel"]), "the right-click menu while stopped reads {menu_stopped}");
    // Start Server runs the start command and the line reads Running again.
    let _ = std::fs::remove_file(&start_file);
    popover_press(popover, &stopped, "start", cx)?;
    let started = popover_settle(popover, menu, cx, |s| s["server"]["stopped_by_menu"] == false && s["popover"]["controls"]["stop"].is_array()).await;
    ensure!(started["server"]["stopped_by_menu"] == false && started["server"]["word"].as_str().is_some_and(|w| w.starts_with("Running")) && start_file.exists(),
        "Start Server ended with {} (start command ran: {})", started["server"], start_file.exists());
    // Started again, the window draws its grid in place of the notice.
    let mut grid_back = 0;
    for _ in 0..50 {
        draw(window, None, cx)?;
        let (notice, drawn) = window.update(cx, |c, _, _| (c.state_json()["server"]["notice"].clone(), c.grid_drawn_count()))?;
        if notice.is_null() && drawn > 0 {
            grid_back = drawn;
            break;
        }
        tick(cx, 100).await;
    }
    ensure!(grid_back > 0, "Start Server left the window without its grid");
    // The right-click menu while running: Stop Server… asks in an alert; Cancel leaves it running.
    menu.update(cx, |m, _| m.bridge_call(json!({"op": "press_status_item", "button": "right"})));
    let menu_running = popover_settle(popover, menu, cx, |s| s["status_menu"]["menu"][0] == "Stop Server…").await["status_menu"]["menu"].clone();
    ensure!(menu_running == json!(["Stop Server…", "Restart Server", "-", "Quit Wheel"]), "the right-click menu while running reads {menu_running}");
    menu.update(cx, |m, _| m.bridge_call(json!({"op": "choose_status_menu", "item": "stop"})));
    let mut alert = Value::Null;
    for _ in 0..50 {
        tick(cx, 100).await;
        alert = bridge_state(cx)["alert"].clone();
        if alert.is_object() {
            break;
        }
    }
    let spaces = menu.update(cx, |m, cx| m.state_json(cx))["spaces"].as_u64().unwrap_or(0);
    ensure!(alert["message"] == "Stop Wheel Server?" && alert["informative"] == format!("Terminals and running agents in {spaces} spaces stop.") && alert["buttons"] == json!(["Stop Server", "Cancel"]),
        "Stop Server… from the menu asked {alert}");
    menu.update(cx, |m, _| m.bridge_call(json!({"op": "answer_alert", "button": 1})));
    let after_alert = popover_settle(popover, menu, cx, |s| s["log"].as_array().is_some_and(|l| l.iter().any(|l| l == "alert stop answered with button 1"))).await;
    ensure!(after_alert["server"]["stopped_by_menu"] == false && after_alert["server"]["busy"].is_null(), "cancelling the alert stopped the server: {}", after_alert["server"]);
    // Restart Server from the menu runs the stop and start commands.
    let _ = std::fs::remove_file(&stop_file);
    let _ = std::fs::remove_file(&start_file);
    let restarts_before = server_log().iter().filter(|l| *l == "server: restarted from the app").count();
    menu.update(cx, |m, _| m.bridge_call(json!({"op": "choose_status_menu", "item": "restart"})));
    for _ in 0..50 {
        tick(cx, 100).await;
        if server_log().iter().filter(|l| *l == "server: restarted from the app").count() > restarts_before {
            break;
        }
    }
    let restarts_after = server_log().iter().filter(|l| *l == "server: restarted from the app").count();
    ensure!(restarts_after == restarts_before + 1 && stop_file.exists() && start_file.exists(),
        "Restart Server from the menu: {restarts_before} restarts before, {restarts_after} after, stop ran {}, start ran {}", stop_file.exists(), start_file.exists());
    // Pressing the line again closes the details, and the popover shrinks back.
    let now = menu.update(cx, |m, cx| m.state_json(cx));
    popover_press(popover, &now, "server", cx)?;
    let closed_again = popover_settle(popover, menu, cx, |s| s["server"]["expanded"] == false && popover_fitted(s)).await;
    let closed_again_height = closed_again["popover"]["frame"][3].as_f64().unwrap_or(0.);
    ensure!((closed_again_height - closed_height).abs() < 1., "closing the details left the popover {closed_again_height} tall, {closed_height} before");
    let log = server_log();
    Ok(json!({"word": word, "colour": closed["server"]["colour"], "details": details, "heights": {"closed": closed_height, "open": open_height, "closed_again": closed_again_height},
        "stop_question": {"shown": asking["server"]["confirming"], "cancelled": cancelled["server"]["confirming"] == false},
        "stopped": {"word": stopped["server"]["word"], "details": stopped_details, "window_word": window_word, "stop_ran": true, "window_notice": notice, "notice_picture": picture_notice},
        "started": {"word": started["server"]["word"], "start_ran": true, "window_grid_panes": grid_back},
        "menu": {"stopped": menu_stopped, "running": menu_running}, "alert": alert, "restart": {"before": restarts_before, "after": restarts_after},
        "update_dot": closed["told"]["update"], "pictures": [picture_open, picture_question, picture_stopped],
        "log": log.iter().filter(|l| l.starts_with("server: ")).cloned().collect::<Vec<_>>()}))
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

// MARK: Pane drags that leave or stay in a window (wheel-ynjo.8.9)

/// A window as the pane drag steps drive it: its handle, bridge half, screen frame and id.
#[derive(Clone)]
struct Side {
    h: gpui::WindowHandle<Client>,
    native: std::rc::Rc<Native>,
    f: Vec<f32>,
    id: String,
}

/// A space of its own in `Side`'s session holding the pane the steps drag.
struct PaneRig {
    space: String,
    pane: String,
    rect: Rect,
    label: String,
    dir: PathBuf,
    /// The space `side` showed before the rig's was selected.
    back: Option<String>,
}

fn away_grid(c: &Client) -> Vec<(String, Rect)> {
    c.layers().first().map(|l| l.leaves.iter().map(|p| (p.leaf.id.clone(), p.rect)).collect()).unwrap_or_default()
}

fn away_holding(c: &Client, pane: &str) -> Option<String> {
    c.layout_lists().1.into_iter().find(|s| s.site_of(pane).is_some()).map(|s| s.id)
}

fn away_lines(h: gpui::WindowHandle<Client>, cx: &mut AsyncApp) -> Vec<String> {
    h.update(cx, |c, _, _| c.toast_log.iter().filter(|l| l.contains("pane drag") || l.contains("little window")).rev().take(12).cloned().collect()).unwrap_or_default()
}

fn grip_of(r: Rect) -> [f64; 2] {
    [f64::from(r.x + 30.), f64::from(r.y + 11.)]
}

/// Where `to` (screen) is in `from`'s window.
fn local_to(from: &Side, to: (f32, f32)) -> [f64; 2] {
    [f64::from(to.0 - from.f[0]), f64::from(to.1 - from.f[1])]
}

/// A space of its own in `side`'s session with one pane, shown in `side`.
async fn pane_rig_open(side: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<PaneRig> {
    let back = side.h.update(cx, |c, _, _| c.space_id().map(str::to_owned))?;
    let dir = std::env::temp_dir().join(format!("wheel-gpui-pane-rig-{}-{}", std::process::id(), side.id));
    std::fs::create_dir_all(&dir)?;
    let (ok, out, err) = run_pumped(&["space", "new", &dir.to_string_lossy(), "--name", "panerig"], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel space new for the pane rig failed: {err}");
    let space = out.trim().to_owned();
    let (ok, out, err) = run_pumped(&["space", "move", &space, &side.id], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel space move of the pane rig to {} failed: {err} {out}", side.id);
    let mut found = None;
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        side.h.update(cx, |c, _, cx| {
            c.select_space(&space);
            cx.notify();
        })?;
        draw(side.h, None, cx)?;
        found = side.h.update(cx, |c, _, _| (c.space_id() == Some(space.as_str())).then(|| away_grid(c)).and_then(|g| g.first().cloned()))?;
        if found.is_some() {
            break;
        }
    }
    let (pane, rect) = found.ok_or_else(|| anyhow::anyhow!("{} never showed the pane rig's space {space} with a pane", side.id))?;
    let label = side.h.update(cx, |c, _, _| c.layout_lists().1.iter().flat_map(|s| s.shown()).find(|l| l.id == pane).map(|l| c.pane_label(l)))?.unwrap_or_default();
    Ok(PaneRig { space, pane, rect, label, dir, back })
}

/// Close the rig's space when it is still there (a pane that left it may have taken it along).
async fn pane_rig_close(side: &Side, rig: PaneRig, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<()> {
    pump(connection, 300, layouts, pumper, cx).await?;
    if side.h.update(cx, |c, _, _| c.layout_lists().1.iter().any(|s| s.id == rig.space)).unwrap_or(false) {
        let (ok, _, err) = run_pumped(&["space", "close", &rig.space], connection, layouts, pumper, cx).await?;
        ensure!(ok, "wheel space close {} after the pane rig failed: {err}", rig.space);
    }
    let _ = std::fs::remove_dir_all(&rig.dir);
    // The window shows what it showed before the rig.
    if let Some(back) = rig.back.filter(|b| *b != rig.space) {
        side.h.update(cx, |c, _, cx| {
            if c.layout_lists().1.iter().any(|s| s.id == back) {
                c.select_space(&back);
            }
            cx.notify();
        })?;
        draw(side.h, None, cx)?;
    }
    Ok(())
}

/// A second space in `side`'s session, so its sidebar and tabs have one it does not show for the
/// pane to be held over; the space's id.
async fn pane_spare_open(side: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<(String, PathBuf)> {
    let dir = std::env::temp_dir().join(format!("wheel-gpui-pane-spare-{}-{}", std::process::id(), side.id));
    std::fs::create_dir_all(&dir)?;
    let (ok, out, err) = run_pumped(&["space", "new", &dir.to_string_lossy(), "--name", "panespare"], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel space new for the spare space failed: {err}");
    let space = out.trim().to_owned();
    let (ok, out, err) = run_pumped(&["space", "move", &space, &side.id], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel space move of the spare space to {} failed: {err} {out}", side.id);
    let mut listed = false;
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        draw(side.h, None, cx)?;
        if side.h.update(cx, |c, _, _| c.session_spaces().iter().any(|s| s.id == space))? {
            listed = true;
            break;
        }
    }
    ensure!(listed, "{} never listed the spare space {space}", side.id);
    Ok((space, dir))
}

/// The first row of `dst`'s sidebar that is not the space it shows.
fn other_row(dst: &Side, cx: &mut AsyncApp) -> Result<(String, [f64; 4])> {
    let rows = dst.h.update(cx, |c, _, _| c.sidebar_json())?;
    rows["rows"].as_array().and_then(|r| r.iter().find(|r| r["active"] == false)).map(|r| (r["id"].as_str().unwrap_or_default().to_owned(), [0, 1, 2, 3].map(|i| r["frame"][i].as_f64().unwrap_or(0.))))
        .ok_or_else(|| anyhow::anyhow!("{}'s sidebar has a row for no space it does not show: {rows}", dst.id))
}

/// Press the rig's pane header in `src`, drag it a little, then onto `target` (screen), and let the
/// windows redraw.
async fn pane_press_and_move(src: &Side, dst: Option<&Side>, rig: &PaneRig, target: (f32, f32), cx: &mut AsyncApp) -> Result<()> {
    let grip = grip_of(rig.rect);
    src.native.call(json!({"op":"post_mouse","kind":"down","point":grip}));
    tick(cx, 100).await;
    src.native.call(json!({"op":"post_mouse","kind":"drag","point":[grip[0] + 20., grip[1] + 20.]}));
    tick(cx, 100).await;
    draw(src.h, None, cx)?;
    src.native.call(json!({"op":"post_mouse","kind":"drag","point":local_to(src, target)}));
    tick(cx, 150).await;
    draw(src.h, None, cx)?;
    if let Some(dst) = dst {
        draw(dst.h, None, cx)?;
    }
    Ok(())
}

/// Hold the pane over `dst`'s row for `held` until `dst` shows that space; the space `dst` showed before.
async fn pane_hold_over_row(src: &Side, dst: &Side, rig: &PaneRig, held: &str, row: [f64; 4], cx: &mut AsyncApp) -> Result<Option<String>> {
    let before = dst.h.update(cx, |c, _, _| c.space_id().map(str::to_owned))?;
    pane_press_and_move(src, Some(dst), rig, (dst.f[0] + row[0] as f32 + 20., dst.f[1] + row[1] as f32 + 5.), cx).await?;
    let aim = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    ensure!(aim["away"] == format!("the end of space {held}") && aim["hold"] == json!([dst.id, held]), "the pane drag did not aim at {}'s row for {held}: {aim}; {} logged {:?}", dst.id, src.id, away_lines(src.h, cx));
    let mut switched = false;
    for _ in 0..40 {
        tick(cx, 50).await;
        draw(src.h, None, cx)?;
        draw(dst.h, None, cx)?;
        if dst.h.update(cx, |c, _, _| c.space_id() == Some(held))? {
            switched = true;
            break;
        }
    }
    ensure!(switched, "holding over {}'s row for {held} never showed it there: it shows {:?}; {} logged {:?}", dst.id, shown(dst.h, cx)?, src.id, away_lines(src.h, cx));
    Ok(before)
}

/// Wait for `dst` to show `space` again, redrawing both windows.
async fn pane_wait_shows(src: &Side, dst: &Side, space: Option<&str>, cx: &mut AsyncApp) -> Result<bool> {
    for _ in 0..40 {
        tick(cx, 50).await;
        draw(src.h, None, cx).ok();
        draw(dst.h, None, cx)?;
        if dst.h.update(cx, |c, _, _| c.space_id() == space)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A hold switched a window, then the pane is released over nothing (its own place in the grid):
/// the window shows what it showed before (AppModel.endAway).
async fn pane_hold_then_release(src: &Side, dst: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let rig = pane_rig_open(src, pumper, connection, layouts, cx).await?;
    let (held, row) = other_row(dst, cx)?;
    let before = pane_hold_over_row(src, dst, &rig, &held, row, cx).await?;
    // Back over its own place: no drop target, no away target.
    src.native.call(json!({"op":"post_mouse","kind":"drag","point":[f64::from(rig.rect.x + rig.rect.w / 2.), f64::from(rig.rect.y + rig.rect.h / 2.)]}));
    tick(cx, 150).await;
    draw(src.h, None, cx)?;
    let aim = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    ensure!(aim["away"].is_null() && aim["drop"].is_null() && aim["hold"].is_null(), "the pane over its own place aims at {aim}");
    src.native.call(json!({"op":"post_mouse","kind":"up","point":[f64::from(rig.rect.x + rig.rect.w / 2.), f64::from(rig.rect.y + rig.rect.h / 2.)]}));
    let restored = pane_wait_shows(src, dst, before.as_deref(), cx).await?;
    let (stays, ended) = src.h.update(cx, |c, _, _| (away_holding(c, &rig.pane), c.sidebar_json()["pane_drag"].is_null()))?;
    ensure!(restored && stays.as_deref() == Some(rig.space.as_str()) && ended, "a release over nothing left {} showing {:?}, not {before:?}; the pane is in {stays:?}, drag ended {ended}; {} logged {:?}", dst.id, shown(dst.h, cx)?, src.id, away_lines(src.h, cx));
    let report = json!({"held": held, "switched_to": held, "restored_to": before, "pane_stayed": stays});
    pane_rig_close(src, rig, pumper, connection, layouts, cx).await?;
    Ok(report)
}

/// Escape after a hold with the pointer still: the cancel puts the window back at the next redraw
/// (Client::cancel_pane_drag's restore).
async fn pane_hold_then_escape(src: &Side, dst: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let rig = pane_rig_open(src, pumper, connection, layouts, cx).await?;
    let (held, row) = other_row(dst, cx)?;
    let before = pane_hold_over_row(src, dst, &rig, &held, row, cx).await?;
    src.native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    let restored = pane_wait_shows(src, dst, before.as_deref(), cx).await?;
    let ended = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].is_null())?;
    ensure!(restored && ended, "Escape after the hold left {} showing {:?}, not {before:?}; drag ended {ended}; {} logged {:?}", dst.id, shown(dst.h, cx)?, src.id, away_lines(src.h, cx));
    src.native.call(json!({"op":"post_mouse","kind":"up","point":local_to(src, (dst.f[0] + row[0] as f32 + 20., dst.f[1] + row[1] as f32 + 5.))}));
    let report = json!({"held": held, "restored_to": before, "drag_ended": ended});
    pane_rig_close(src, rig, pumper, connection, layouts, cx).await?;
    Ok(report)
}

/// With both sidebars hidden the tab strip takes the sidebar's place: a hold over a tab shows its
/// space, the tab outlines, and the strip's room past the tabs is a new space.
async fn pane_drag_over_tabs(src: &Side, dst: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    for s in [src, dst] {
        s.h.update(cx, |c, _, cx| {
            c.set_sidebar(false);
            cx.notify();
        })?;
        draw(s.h, None, cx)?;
    }
    let rig = pane_rig_open(src, pumper, connection, layouts, cx).await?;
    let before = dst.h.update(cx, |c, _, _| c.space_id().map(str::to_owned))?;
    let (held, tab) = dst.h.update(cx, |c, _, _| c.title_row().chips.iter().find_map(|ch| match &ch.hit { Hit::Space(s) if Some(s.as_str()) != c.space_id() => Some((s.clone(), ch.rect)), _ => None }))?
        .ok_or_else(|| anyhow::anyhow!("{} shows no tab for a space it does not show", dst.id))?;
    pane_press_and_move(src, Some(dst), &rig, (dst.f[0] + tab.x + tab.w / 2., dst.f[1] + tab.y + tab.h / 2.), cx).await?;
    let on_tab = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    let outlined = dst.h.update(cx, |c, _, _| c.sidebar_json()["pane_hover"].clone())?;
    ensure!(on_tab["away"] == format!("the end of space {held}") && on_tab["hold"] == json!([dst.id, held]) && outlined == format!("the end of space {held}"),
        "the pane drag did not aim at {}'s tab for {held}: {on_tab}; that window sees {outlined}; {} logged {:?}", dst.id, src.id, away_lines(src.h, cx));
    ensure!(pane_wait_shows(src, dst, Some(held.as_str()), cx).await?, "holding over {}'s tab for {held} never showed it there", dst.id);
    // Past the last tab, in the strip's room: a new space in that window.
    let (track, tabs) = dst.h.update(cx, |c, _, _| { let r = c.title_row(); (r.track, r.tabs) })?;
    let track = track.ok_or_else(|| anyhow::anyhow!("{} has no tab strip", dst.id))?;
    let room = tabs.last().map(|t| t.x + t.w).unwrap_or(track.x);
    // The strip is the tabs' own track (SpacesView's tabStripFrames), so its room is the inset
    // past the last tab, not a gap beyond it.
    ensure!(room < track.x + track.w, "{}'s last tab ends at {room}, past the strip's end {}: no strip past the tabs to release on", dst.id, track.x + track.w);
    let on_room = (dst.f[0] + (room + track.x + track.w) / 2., dst.f[1] + track.y + track.h / 2.);
    src.native.call(json!({"op":"post_mouse","kind":"drag","point":local_to(src, on_room)}));
    tick(cx, 150).await;
    draw(src.h, None, cx)?;
    draw(dst.h, None, cx)?;
    let in_room = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    let new_space = dst.h.update(cx, |c, _, _| c.sidebar_json()["pane_new_space"].clone())?;
    ensure!(in_room["away"] == format!("a new space in window {}", dst.id) && new_space == true, "the pane drag did not aim at {}'s strip room at {on_room:?}: {in_room}; that window outlines it {new_space}", dst.id);
    src.native.call(json!({"op":"post_key","text":"\u{1b}","code":53}));
    let restored = pane_wait_shows(src, dst, before.as_deref(), cx).await?;
    ensure!(restored, "Escape over the tab strip left {} showing {:?}, not {before:?}", dst.id, shown(dst.h, cx)?);
    src.native.call(json!({"op":"post_mouse","kind":"up","point":local_to(src, on_room)}));
    pane_rig_close(src, rig, pumper, connection, layouts, cx).await?;
    for s in [src, dst] {
        s.h.update(cx, |c, _, cx| {
            c.set_sidebar(true);
            cx.notify();
        })?;
        draw(s.h, None, cx)?;
    }
    Ok(json!({"held": held, "tab_hint": on_tab["hint"], "outlined": outlined, "strip_room": in_room["away"], "room_outlined": new_space, "restored_to": before}))
}

/// The menu bar popover's rows are End targets for a pane, and nothing is held over them.
async fn pane_drag_onto_popover_row(src: &Side, pumper: gpui::WindowHandle<Client>, bridge: &std::rc::Rc<Bridge>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let rig = pane_rig_open(src, pumper, connection, layouts, cx).await?;
    let menu = app_menu_bar(bridge, cx)?;
    cx.update(|cx| menu.update(cx, |m, _| m.press_item()));
    let mut popover = None;
    for _ in 0..50 {
        tick(cx, 100).await;
        popover = menu.update(cx, |m, _| m.popover());
        if popover.is_some() {
            break;
        }
    }
    let popover_any: gpui::AnyWindowHandle = popover.ok_or_else(|| anyhow::anyhow!("the popover did not open for the pane drag"))?.into();
    let mut target = None;
    for _ in 0..30 {
        let _ = popover_any.update(cx, |_, w, cx| w.draw(cx).clear());
        tick(cx, 100).await;
        target = cx.update(|cx| menu.read(cx).drop_popover(cx)).and_then(|p| p.cards.iter().find_map(|c| c.spaces.iter().zip(&c.rows).find(|(s, _)| **s != rig.space).map(|(s, r)| (s.clone(), *r))));
        if target.is_some() {
            break;
        }
    }
    let (space, row) = target.ok_or_else(|| anyhow::anyhow!("the popover never measured a row for a space other than {}", rig.space))?;
    pane_press_and_move(src, None, &rig, (row.x + 20., row.y + 2.), cx).await?;
    let over = src.h.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    ensure!(over["away"] == format!("the end of space {space}") && over["hold"].is_null() && over["hint"].as_str().is_some_and(|h| h.starts_with(&format!("Move “{}” to the end of “", rig.label))),
        "the pane drag did not aim at the popover's row for {space} at {:?}: {over}; {} logged {:?}", (row.x + 20., row.y + 2.), src.id, away_lines(src.h, cx));
    src.native.call(json!({"op":"post_mouse","kind":"up","point":local_to(src, (row.x + 20., row.y + 2.))}));
    let mut landed = None;
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        landed = src.h.update(cx, |c, _, _| away_holding(c, &rig.pane))?;
        if landed.as_deref() == Some(space.as_str()) {
            break;
        }
    }
    cx.update(|cx| menu.update(cx, |m, cx| m.toggle_popover(None, cx)));
    ensure!(landed.as_deref() == Some(space.as_str()), "the drop on the popover's row for {space} left {} in {landed:?}; {} logged {:?}", rig.pane, src.id, away_lines(src.h, cx));
    let (ok, _, err) = run_pumped(&["pane", "close", &rig.pane], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel pane close {} after the popover drop failed: {err}", rig.pane);
    let report = json!({"space": space, "hint": over["hint"], "hold": over["hold"], "landed": landed});
    pane_rig_close(src, rig, pumper, connection, layouts, cx).await?;
    Ok(report)
}

/// A drag inside a little window showing the pane's space docks in that window's own grid
/// (PaneTarget::Home for a little window).
async fn pane_drag_in_little_window(owner: &Side, peer: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let rig = pane_rig_open(owner, pumper, connection, layouts, cx).await?;
    let (ok, out, err) = run_pumped(&["pane", "split", &rig.pane, "right"], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel pane split for the little window failed: {err}");
    let other = out.lines().last().unwrap_or_default().trim().to_owned();
    let mut two = false;
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        draw(owner.h, None, cx)?;
        if owner.h.update(cx, |c, _, _| away_grid(c).len() == 2)? {
            two = true;
            break;
        }
    }
    ensure!(two, "the rig's space never showed two panes after the split");
    owner.h.update(cx, |c, _, cx| {
        c.run_space_menu("little", &rig.space, cx);
        cx.notify();
    })?;
    let little_id = owner.h.update(cx, |c, _, _| c.little_showing(&rig.space))?.ok_or_else(|| anyhow::anyhow!("Open in Little Window opened none"))?;
    let little = cx.update(|_| wheel_gpui::window_handle(&little_id)).ok_or_else(|| anyhow::anyhow!("little window {little_id} has no handle"))?;
    let mut grid = vec![];
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        draw(little, None, cx)?;
        grid = little.update(cx, |c, _, _| away_grid(c))?;
        if grid.len() == 2 {
            break;
        }
    }
    ensure!(grid.len() == 2, "the little window drew {grid:?}, not the two panes of {}", rig.space);
    let native = little.update(cx, |c, _, _| c.native())?;
    let frame_now = |n: &Native| -> Vec<f32> { n.call(json!({"op":"frame"}))["frame"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect()).unwrap_or_default() };
    // Beside both main windows: windows that are never on screen have no stacking order, so the
    // pointer must be over the little window alone for it to be the one under it.
    let beside = (owner.f[0] + owner.f[2]).max(peer.f[0] + peer.f[2]) + 40.;
    native.call(json!({"op": "move_window", "point": [f64::from(beside), f64::from(owner.f[1])]}));
    let mut frame = vec![];
    for _ in 0..30 {
        draw(little, None, cx)?;
        tick(cx, 100).await;
        frame = frame_now(&native);
        if frame.len() == 4 && (frame[0] - beside).abs() < 2. {
            break;
        }
    }
    ensure!(frame.len() == 4 && (frame[0] - beside).abs() < 2., "the little window {little_id} is at {frame:?}, not moved to x {beside}");
    let (moved, moved_rect) = grid.iter().find(|(id, _)| *id == rig.pane).cloned().unwrap();
    let (target, target_rect) = grid.iter().find(|(id, _)| *id == other).cloned().ok_or_else(|| anyhow::anyhow!("the little window shows no pane {other}: {grid:?}"))?;
    // The moved pane goes on the target's side farther from it, whichever that is.
    let side = if moved_rect.x < target_rect.x { wheel_proto::Dir::Right } else { wheel_proto::Dir::Left };
    let on = if side == wheel_proto::Dir::Right { (target_rect.x + target_rect.w - 8., target_rect.y + target_rect.h / 2.) } else { (target_rect.x + 8., target_rect.y + target_rect.h / 2.) };
    let grip = grip_of(moved_rect);
    native.call(json!({"op":"post_mouse","kind":"down","point":grip}));
    tick(cx, 100).await;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[grip[0] + 20., grip[1] + 20.]}));
    tick(cx, 100).await;
    draw(little, None, cx)?;
    native.call(json!({"op":"post_mouse","kind":"drag","point":[f64::from(on.0), f64::from(on.1)]}));
    tick(cx, 150).await;
    draw(little, None, cx)?;
    let aim = little.update(cx, |c, _, _| c.sidebar_json()["pane_drag"].clone())?;
    let zone = wheel_gpui::grid_drop::Zone::Edge(side).label();
    ensure!(aim["away"].is_null() && aim["drop"] == json!({"target": target, "zone": zone}), "the drag inside the little window {little_id} at {on:?} aims at {aim}, not the {zone} of {target}; little frame {frame:?}, windows {:?} and {:?}", owner.f, peer.f);
    native.call(json!({"op":"post_mouse","kind":"up","point":[f64::from(on.0), f64::from(on.1)]}));
    let mut docked = false;
    for _ in 0..50 {
        pump(connection, 100, layouts, pumper, cx).await?;
        draw(little, None, cx)?;
        let now = little.update(cx, |c, _, _| away_grid(c))?;
        let (m, t) = (now.iter().find(|(id, _)| *id == moved), now.iter().find(|(id, _)| *id == target));
        if m.zip(t).is_some_and(|(m, t)| if side == wheel_proto::Dir::Right { m.1.x > t.1.x } else { m.1.x < t.1.x }) {
            docked = true;
            break;
        }
    }
    ensure!(docked, "the drop in the little window did not dock {moved} on the {zone} of {target}: {:?}", little.update(cx, |c, _, _| away_grid(c))?);
    little.update(cx, |c, _, _| c.little_back())?;
    for _ in 0..30 {
        draw(little, None, cx).ok();
        tick(cx, 100).await;
        if cx.update(|_| wheel_gpui::window_handle(&little_id)).is_none() {
            break;
        }
    }
    let report = json!({"pane": moved, "target": target, "zone": zone, "docked": docked, "little": little_id});
    pane_rig_close(owner, rig, pumper, connection, layouts, cx).await?;
    Ok(report)
}

/// The source window closing in the middle of a pane drag puts the windows a hold switched back
/// (wheel-ynjo.8.8): `src` drags over `dst`'s row until `dst` shows that space, then `src` closes.
async fn pane_drag_source_closes(src: &Side, dst: &Side, pumper: gpui::WindowHandle<Client>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let rig = pane_rig_open(src, pumper, connection, layouts, cx).await?;
    let (held, row) = other_row(dst, cx)?;
    let before = pane_hold_over_row(src, dst, &rig, &held, row, cx).await?;
    src.h.update(cx, |_, w, _| w.remove_window())?;
    let restored = pane_wait_shows(src, dst, before.as_deref(), cx).await?;
    ensure!(restored, "{} closed in the middle of the drag and {} still shows {:?}, not {before:?}", src.id, dst.id, shown(dst.h, cx)?);
    let (ok, _, err) = run_pumped(&["space", "close", &rig.space], connection, layouts, pumper, cx).await?;
    ensure!(ok, "wheel space close {} after the source closed failed: {err}", rig.space);
    let _ = std::fs::remove_dir_all(&rig.dir);
    Ok(json!({"held": held, "restored_to": before, "closed": src.id}))
}

/// The pane drag cases wheel-ynjo.8.9 adds, in one place; the last closes the second window.
async fn pane_drag_cases(first: &Side, second: &Side, bridge: &std::rc::Rc<Bridge>, connection: &mut Connection, layouts: &mut usize, cx: &mut AsyncApp) -> Result<Value> {
    let pumper = first.h;
    let spare_first = pane_spare_open(first, pumper, connection, layouts, cx).await?;
    let spare_second = pane_spare_open(second, pumper, connection, layouts, cx).await?;
    let restore = pane_hold_then_release(first, second, pumper, connection, layouts, cx).await?;
    let escape = pane_hold_then_escape(first, second, pumper, connection, layouts, cx).await?;
    let tabs = pane_drag_over_tabs(first, second, pumper, connection, layouts, cx).await?;
    let popover = pane_drag_onto_popover_row(first, pumper, bridge, connection, layouts, cx).await?;
    let little = pane_drag_in_little_window(first, second, pumper, connection, layouts, cx).await?;
    let closes = pane_drag_source_closes(second, first, pumper, connection, layouts, cx).await?;
    for (space, dir) in [spare_first, spare_second] {
        // The spare may be gone with the window that held it.
        let _ = run_pumped(&["space", "close", &space], connection, layouts, pumper, cx).await?;
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(json!({"restore_after_drop": restore, "escape_after_hold": escape, "tab_strip": tabs, "popover_row": popover, "little_home": little, "source_closes": closes}))
}
