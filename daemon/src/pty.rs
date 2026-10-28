//! One terminal: a shell on a PTY, the screen it draws, and a broadcast of what comes next.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result};
use ghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use ghostty_vt::screen::Screen;
use ghostty_vt::terminal::{Mode, Options, Terminal};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, oneshot};
use wheel_proto::TermInfo;

use crate::shell::Launch;

/// Lines of history the daemon's copy of a screen keeps above it.
pub const SCROLLBACK_LINES: usize = 10_000;

/// Bytes of shell output that may wait for the screen thread. Past this the reader blocks, the
/// PTY fills, and the shell stalls until the screen thread catches up.
const SCREEN_BACKLOG_BYTES: usize = 1024 * 1024;

/// How much output is queued for the screen thread. Only the reader waits on it: resize, attach
/// and exit go through the same queue without counting, so they never block a caller.
struct Backlog {
    queued: Mutex<usize>,
    drained: Condvar,
    closed: AtomicBool,
}

impl Backlog {
    fn new() -> Arc<Backlog> {
        Arc::new(Backlog {
            queued: Mutex::new(0),
            drained: Condvar::new(),
            closed: AtomicBool::new(false),
        })
    }

    /// Wait for room, then count `n` more bytes. Returns at once once the screen thread is gone.
    fn reserve(&self, n: usize) {
        let mut queued = self.queued.lock().unwrap();
        while *queued >= SCREEN_BACKLOG_BYTES && !self.closed.load(Ordering::SeqCst) {
            queued = self.drained.wait(queued).unwrap();
        }
        *queued += n;
    }

    fn release(&self, n: usize) {
        let mut queued = self.queued.lock().unwrap();
        *queued = queued.saturating_sub(n);
        self.drained.notify_all();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _guard = self.queued.lock().unwrap();
        self.drained.notify_all();
    }
}

#[derive(Clone, Debug)]
pub enum TermEvent {
    Output(Arc<[u8]>),
    Exited(Option<u32>),
}

/// What an attach sees, taken at one instant: output that redraws the screen as it is now, the
/// exit status if the shell already ended, and a receiver for everything after.
pub struct Attachment {
    pub replay: Vec<u8>,
    pub exited: Option<Option<u32>>,
    pub rx: broadcast::Receiver<TermEvent>,
}

/// How [`Term::read_text`] ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadEnd {
    /// The text it waited for appeared.
    Matched,
    /// The time ran out.
    TimedOut,
    /// The shell ended first, with this status.
    Exited(Option<u32>),
}

/// Work for the thread that owns the screen. Output, exit and attach go through it in order, so
/// an attach never misses or repeats a byte and never misses the exit.
enum ScreenMsg {
    Output(Vec<u8>),
    Resize(u16, u16),
    Exited(Option<u32>),
    Attach(oneshot::Sender<Attachment>),
}

pub struct Term {
    pub id: String,
    pub pid: Option<u32>,
    pub cwd: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    input: std_mpsc::Sender<Vec<u8>>,
    screen: std_mpsc::Sender<ScreenMsg>,
    exited: AtomicBool,
    started: std::time::Instant,
    /// Set once a program other than the shell has held this terminal's foreground, read as the
    /// output arrives. A pane told to resume an agent forgets it once that has happened and the
    /// agent is not running, rather than after a new shell's grace period.
    job_seen: AtomicBool,
}

impl Term {
    /// Fork `launch`'s program, else the user's login shell, on a new PTY. Blocking: call from a
    /// blocking context.
    pub(crate) fn spawn(
        id: String,
        cwd: String,
        launch: Launch,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<Term>> {
        let mut cmd = match launch.program {
            Some(program) => {
                let mut cmd = CommandBuilder::new(program);
                cmd.args(&launch.args);
                cmd
            }
            None => CommandBuilder::new_default_prog(),
        };
        // A pane learns its space from the daemon; one inherited from a daemon started in an
        // older pane would name a space the pane may have left.
        cmd.env_remove("WHEEL_SPACE");
        for (key, value) in &launch.env {
            cmd.env(key, value);
        }
        Term::spawn_with(id, cwd, cmd, cols, rows)
    }

    fn spawn_with(
        id: String,
        cwd: String,
        mut cmd: CommandBuilder,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<Term>> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pty")?;
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn terminal process")?;
        drop(pair.slave);
        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let mut writer = pair.master.take_writer().context("take pty writer")?;
        let (input, input_rx) = std_mpsc::channel::<Vec<u8>>();
        let (screen, screen_rx) = std_mpsc::channel::<ScreenMsg>();
        let backlog = Backlog::new();
        let term = Arc::new(Term {
            id,
            pid,
            cwd,
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            input,
            screen,
            exited: AtomicBool::new(false),
            started: std::time::Instant::now(),
            job_seen: AtomicBool::new(false),
        });

        // Writes block when the shell stops reading, so they get their own thread.
        let wid = term.id.clone();
        std::thread::Builder::new()
            .name(format!("pty-w-{wid}"))
            .spawn(move || {
                for bytes in input_rx {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        tracing::warn!(term = %wid, "pty write failed; dropping input");
                        break;
                    }
                }
            })?;

        // libghostty's terminal stays on the thread that made it, so this thread owns the screen.
        let sid = term.id.clone();
        let (made, made_rx) = std_mpsc::channel();
        let screen_backlog = backlog.clone();
        std::thread::Builder::new()
            .name(format!("pty-s-{sid}"))
            .spawn(move || run_screen(sid, cols, rows, screen_rx, screen_backlog, made))?;
        made_rx
            .recv()
            .context("screen thread stopped")?
            .context("create screen")?;

        let t = term.clone();
        std::thread::Builder::new()
            .name(format!("pty-r-{}", t.id))
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            backlog.reserve(n);
                            let _ = t.screen.send(ScreenMsg::Output(buf[..n].to_vec()));
                            // A program the shell started draws as it starts, so its output is
                            // the moment to see it, however briefly it runs.
                            if !t.job_seen() && t.foreground().is_some_and(|g| Some(g) != t.pid) {
                                t.saw_job();
                            }
                        }
                    }
                }
                let code = child.wait().ok().map(|s| s.exit_code());
                tracing::info!(term = %t.id, ?code, "pty exited");
                t.exited.store(true, Ordering::SeqCst);
                let _ = t.screen.send(ScreenMsg::Exited(code));
            })?;
        Ok(term)
    }

    /// The screen as it is now, then everything after. None if the screen thread is gone.
    pub async fn attach(&self) -> Option<Attachment> {
        let (reply, rx) = oneshot::channel();
        self.screen.send(ScreenMsg::Attach(reply)).ok()?;
        rx.await.ok()
    }

    /// What `wheel term read --until` does: the screen as it is now, then live
    /// output until `until` appears (output already on the screen counts), the shell exits, or
    /// `timeout` passes. With no `until`, it reads until the shell exits or the time runs out.
    /// Returns everything read, and how it ended.
    pub async fn read_text(
        &self,
        until: Option<&str>,
        timeout: std::time::Duration,
    ) -> (Vec<u8>, ReadEnd) {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut finder = until.map(wheel_proto::TextFinder::new);
        let Some(mut attached) = self.attach().await else {
            return (Vec::new(), ReadEnd::Exited(None));
        };
        let mut read = std::mem::take(&mut attached.replay);
        if finder.as_mut().is_some_and(|f| f.feed(&read)) {
            return (read, ReadEnd::Matched);
        }
        if let Some(code) = attached.exited {
            return (read, ReadEnd::Exited(code));
        }
        loop {
            match tokio::time::timeout_at(deadline, attached.rx.recv()).await {
                Err(_) => return (read, ReadEnd::TimedOut),
                Ok(Ok(TermEvent::Output(bytes))) => {
                    read.extend_from_slice(&bytes);
                    if finder.as_mut().is_some_and(|f| f.feed(&bytes)) {
                        return (read, ReadEnd::Matched);
                    }
                }
                Ok(Ok(TermEvent::Exited(code))) => return (read, ReadEnd::Exited(code)),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return (read, ReadEnd::Exited(None))
                }
                // Output was dropped for this reader: start over from the screen as it is now.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    let Some(again) = self.attach().await else {
                        return (read, ReadEnd::Exited(None));
                    };
                    read = again.replay.clone();
                    attached = again;
                    attached.replay.clear();
                    if let Some(f) = finder.as_mut() {
                        f.clear();
                        if f.feed(&read) {
                            return (read, ReadEnd::Matched);
                        }
                    }
                    if let Some(code) = attached.exited {
                        return (read, ReadEnd::Exited(code));
                    }
                }
            }
        }
    }

    /// Queue input for the shell. Never blocks.
    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.input
            .send(bytes)
            .map_err(|_| anyhow::anyhow!("terminal {} no longer accepts input", self.id))
    }

    /// Resize the daemon's screen, then the PTY. In that order, the redraw the shell sends for
    /// the new size reaches a screen that already has it.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self.master.lock().unwrap();
        let _ = self.screen.send(ScreenMsg::Resize(cols, rows));
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Err(e) = master.resize(size) {
            if let Ok(old) = master.get_size() {
                let _ = self.screen.send(ScreenMsg::Resize(old.cols, old.rows));
            }
            return Err(e);
        }
        Ok(())
    }

    /// Hang up the shell (SIGHUP). Its exit arrives as usual once it is gone.
    pub fn kill(&self) {
        if let Err(e) = self.killer.lock().unwrap().kill() {
            // ESRCH: the shell already exited.
            tracing::debug!(term = %self.id, error = %e, "kill");
        }
    }

    /// How long ago this terminal started.
    pub fn age(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Record that a program other than the shell held this terminal's foreground.
    pub fn saw_job(&self) {
        self.job_seen.store(true, Ordering::SeqCst);
    }

    /// Whether a program other than the shell has held this terminal's foreground.
    pub fn job_seen(&self) -> bool {
        self.job_seen.load(Ordering::SeqCst)
    }

    /// The process group leader in the foreground of this terminal: the shell at its prompt, or
    /// the program the shell started and is waiting for.
    pub fn foreground(&self) -> Option<u32> {
        let leader = self.master.lock().unwrap().process_group_leader()?;
        u32::try_from(leader).ok().filter(|pid| *pid > 0)
    }

    pub fn info(&self) -> TermInfo {
        let size = self.master.lock().unwrap().get_size().ok();
        TermInfo {
            id: self.id.clone(),
            pid: self.pid,
            cwd: self.cwd.clone(),
            cols: size.map(|s| s.cols).unwrap_or(0),
            rows: size.map(|s| s.rows).unwrap_or(0),
            alive: !self.exited.load(Ordering::SeqCst),
        }
    }
}

/// Feed the shell's output through the daemon's copy of the screen and on to every client, until
/// the terminal and its reader are both gone. `made` reports whether the screen could be created.
fn run_screen(
    id: String,
    cols: u16,
    rows: u16,
    rx: std_mpsc::Receiver<ScreenMsg>,
    backlog: Arc<Backlog>,
    made: std_mpsc::Sender<Result<()>>,
) {
    let mut screen = match new_screen(cols, rows) {
        Ok(screen) => screen,
        Err(e) => {
            let _ = made.send(Err(anyhow::anyhow!("{e:?}")));
            return;
        }
    };
    let _ = made.send(Ok(()));
    let _close = CloseBacklog(backlog.clone());
    let (tx, _) = broadcast::channel(1024);
    let mut exited = None;
    for msg in rx {
        match msg {
            ScreenMsg::Output(bytes) => {
                let n = bytes.len();
                screen.vt_write(&bytes);
                // Replies to the shell's queries come from the attached clients, not from here.
                screen.events().for_each(drop);
                let _ = tx.send(TermEvent::Output(Arc::from(bytes)));
                backlog.release(n);
            }
            ScreenMsg::Resize(cols, rows) => {
                if let Err(e) = screen.resize(cols, rows, 0, 0) {
                    tracing::warn!(term = %id, cols, rows, error = ?e, "screen resize failed");
                }
            }
            ScreenMsg::Exited(code) => {
                exited = Some(code);
                let _ = tx.send(TermEvent::Exited(code));
            }
            ScreenMsg::Attach(reply) => {
                let replay = replay(&id, &screen);
                let _ = reply.send(Attachment {
                    replay,
                    exited,
                    rx: tx.subscribe(),
                });
            }
        }
    }
}

/// Wakes a reader still waiting for room when the screen thread ends, however it ends.
struct CloseBacklog(Arc<Backlog>);

impl Drop for CloseBacklog {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The daemon's copy of a screen: `SCROLLBACK_LINES` of history, and the unfinished tail of the
/// output kept so an attach can carry it.
fn new_screen(cols: u16, rows: u16) -> ghostty_vt::error::Result<Terminal> {
    let mut screen = Terminal::new(Options {
        cols,
        rows,
        ..Options::default()
    })?;
    screen
        .set_scrollback_max_lines(Some(SCROLLBACK_LINES))?
        .set_continuation_max_bytes(CONTINUATION_CAP)?;
    Ok(screen)
}

/// What an attaching client is sent: the screen as it is now (with the shell screen behind it
/// when a full-screen app holds the alternate screen), then the escape sequence or character the
/// last output left unfinished, so the live output that finishes it lands in the same place as on
/// the daemon's screen.
fn replay(id: &str, screen: &Terminal) -> Vec<u8> {
    let drawn = if matches!(screen.active_screen(), Ok(Screen::Alternate)) {
        both_screens(id, screen).or_else(|e| {
            tracing::warn!(
                term = %id,
                error = ?e,
                "shell screen snapshot failed; attaching the alternate screen alone"
            );
            snapshot(screen)
        })
    } else {
        snapshot(screen)
    };
    let mut replay = match drawn {
        Ok(replay) => replay,
        Err(e) => {
            tracing::warn!(term = %id, error = ?e, "screen snapshot failed; attaching blank");
            return Vec::new();
        }
    };
    // Past CONTINUATION_CAP the sequence is dropped from the replay (an OSC 52 clipboard write or
    // a Kitty image can run to megabytes).
    if let Ok(unfinished) = screen.continuation() {
        replay.extend_from_slice(&unfinished);
    }
    replay
}

/// VT output that draws `screen` on a reset terminal: its history and cells with their styles,
/// the modes that differ from the defaults (the alternate screen, cursor keys, mouse reporting,
/// bracketed paste), the scrolling region, the tab stops, and the cursor and pen last; the color
/// palette only when a program changed it, and the default foreground, background and cursor
/// colors only when they differ from the terminal's own.
fn snapshot(screen: &Terminal) -> ghostty_vt::error::Result<Vec<u8>> {
    let palette_changed = screen.color_palette()?.0 != screen.default_color_palette()?.0;
    let options = screen_options()
        .with_modes(true)
        .with_scrolling_region(true)
        .with_tabstops(true)
        .with_palette(palette_changed)
        .with_keyboard(true)
        .with_pwd(true);
    let mut out = Formatter::new(screen, options)?.format_alloc()?.to_vec();
    for (code, now, default) in [
        (10, screen.fg_color()?, screen.default_fg_color()?),
        (11, screen.bg_color()?, screen.default_bg_color()?),
        (12, screen.cursor_color()?, screen.default_cursor_color()?),
    ] {
        if let (Some(c), true) = (now, now != default) {
            out.extend_from_slice(
                format!("\x1b]{code};rgb:{:02x}/{:02x}/{:02x}\x1b\\", c.r, c.g, c.b).as_bytes(),
            );
        }
    }
    Ok(out)
}

/// What one screen carries besides its cells: the cursor, pen, hyperlink, protection, Kitty
/// keyboard flags and character sets.
fn screen_options<'t, 's>() -> FormatterOptions<'t, 's> {
    FormatterOptions::new()
        .with_format(Format::Vt)
        .with_cursor(true)
        .with_style(true)
        .with_hyperlink(true)
        .with_protection(true)
        .with_kitty_keyboard(true)
        .with_charsets(true)
}

/// VT output that draws `screen`, which a full-screen app holds on the alternate screen, on a
/// reset terminal: the primary screen's history and cells with the cursor and pen the app's exit
/// returns to, the switch to the alternate screen, then [`snapshot`] of the alternate screen.
///
/// The formatter reads only the active screen and writes the alternate-screen mode ahead of the
/// cells, and every mode that switches screens moves the cursor or erases the alternate screen, so
/// both screens are drawn from a copy of the daemon's screen.
fn both_screens(id: &str, screen: &Terminal) -> ghostty_vt::error::Result<Vec<u8>> {
    let encoded = ghostty_vt::snapshot::encode(screen)?;
    let mut copy = ghostty_vt::snapshot::Decoder::from_slice(&encoded).decode()?;
    let mut enter = Vec::new();
    for mode in [
        Mode::ALT_SCREEN_LEGACY,
        Mode::ALT_SCREEN,
        Mode::ALT_SCREEN_SAVE,
    ] {
        if copy.mode(mode)? {
            enter.extend_from_slice(format!("\x1b[?{}h", mode.value()).as_bytes());
            // Only the flag: the copy stays on the alternate screen.
            copy.set_mode(mode, false)?;
        }
    }
    let alternate = snapshot(&copy)?;
    // CAN ends whatever sequence the copy's parser was in; leaving 1049 restores the cursor the
    // app's entry saved, the one the client's own `ESC[?1049h` saves in turn.
    copy.vt_write(b"\x18\x1b[?1049l");
    let mut replay = Formatter::new(&copy, screen_options())?
        .format_alloc()?
        .to_vec();
    tracing::info!(
        term = %id,
        primary_bytes = replay.len(),
        alternate_bytes = alternate.len(),
        "attach draws the shell screen behind the alternate screen"
    );
    replay.extend_from_slice(&enter);
    // The alternate screen's cells are drawn from the top left with the default pen, not from
    // the shell's cursor with the shell's pen; the switch saved both for the app's exit.
    replay.extend_from_slice(b"\x1b[H\x1b[0m");
    replay.extend_from_slice(&alternate);
    Ok(replay)
}

/// The longest unfinished sequence the daemon's screen keeps for an attach.
const CONTINUATION_CAP: usize = 1 << 20;

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(t: &Terminal) -> String {
        let opts = FormatterOptions::new().with_format(Format::Plain);
        let text = Formatter::new(t, opts).unwrap().format_alloc().unwrap();
        String::from_utf8(text.to_vec()).unwrap()
    }

    fn styled(t: &Terminal) -> Vec<u8> {
        let opts = FormatterOptions::new()
            .with_format(Format::Vt)
            .with_style(true);
        Formatter::new(t, opts)
            .unwrap()
            .format_alloc()
            .unwrap()
            .to_vec()
    }

    /// Writes `first` to the daemon's screen, attaches a client, then writes `rest` to both, and
    /// returns them.
    fn attach_between(first: &[u8], rest: &[u8]) -> (Terminal, Terminal) {
        let mut daemon = new_screen(20, 3).unwrap();
        daemon.vt_write(first);
        let replay = replay("t", &daemon);
        daemon.vt_write(rest);
        let mut client = Terminal::new(Options {
            cols: 20,
            rows: 3,
            ..Options::default()
        })
        .unwrap();
        client.vt_write(&replay);
        client.vt_write(rest);
        (daemon, client)
    }

    /// The tab stops, palette and default colors a program set come back after a reattach.
    #[test]
    fn palette_default_colors_and_tab_stops_survive_an_attach() {
        // Clear every tab stop, set one at column 5, change palette entry 1, the default
        // foreground and background, then park the cursor on row 2 column 3.
        let program: &[u8] = b"\x1b[3g\x1b[1;6H\x1bH\x1b]4;1;rgb:12/34/56\x1b\\\
            \x1b]10;rgb:aa/bb/cc\x1b\\\x1b]11;rgb:01/02/03\x1b\\\x1b[2;4Hhi";
        let (mut daemon, mut client) = attach_between(program, b"");
        let cursor = |t: &Terminal| (t.cursor_x().unwrap(), t.cursor_y().unwrap());
        assert_eq!(cursor(&client), (5, 1));
        assert_eq!(cursor(&client), cursor(&daemon));
        let rgb = |c: Option<ghostty_vt::style::RgbColor>| c.map(|c| (c.r, c.g, c.b));
        assert_eq!(rgb(client.fg_color().unwrap()), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(rgb(client.bg_color().unwrap()), Some((1, 2, 3)));
        let red = |t: &Terminal| t.color_palette().unwrap().0[1];
        assert_eq!(red(&client), red(&daemon));
        assert_eq!(
            (red(&client).r, red(&client).g, red(&client).b),
            (0x12, 0x34, 0x56)
        );
        // A tab from column 0 stops at the one stop set, column 5, not at column 8.
        for t in [&mut daemon, &mut client] {
            t.vt_write(b"\x1b[1;1H\tX");
        }
        assert_eq!(plain(&client), plain(&daemon));
        assert_eq!(cursor(&client), (6, 0));
    }

    /// An unchanged palette and default colors add nothing to the replay.
    #[test]
    fn a_default_palette_adds_no_color_sequences() {
        let screen = new_screen(20, 3).unwrap();
        let text = String::from_utf8_lossy(&snapshot(&screen).unwrap()).into_owned();
        assert!(
            !text.contains("\x1b]4;") && !text.contains("\x1b]10;"),
            "{text:?}"
        );
    }

    /// The daemon's screen and a client given the replay end the same, even when an attach falls
    /// between two reads that split a sequence.
    #[test]
    fn a_client_attaching_mid_sequence_draws_what_the_daemon_draws() {
        let (daemon, client) = attach_between(b"ab\x1b[3", b"1mred\x1b[0m");
        assert_eq!(plain(&client), "abred");
        assert_eq!(plain(&client), plain(&daemon));
    }

    /// A client that attached while an app held the alternate screen gets the shell's screen
    /// and history back, and its cursor, when the app exits.
    #[test]
    fn the_shell_screen_returns_when_an_app_attached_on_the_alternate_screen_exits() {
        // The shell's pen is red when the app starts, and the app draws with the default pen.
        let shell: &[u8] = b"one\r\ntwo\r\nthree\r\nfour\r\n$ vim\x1b[31m";
        let app: &[u8] = b"\x1b[?1049h\x1b[0m\x1b[H~ vim text\x1b[3";
        let (mut daemon, mut client) = attach_between(&[shell, app].concat(), b"1mX\x1b[0m");
        assert_eq!(plain(&client), "~ vim textX");
        assert_eq!(plain(&client), plain(&daemon));
        assert_eq!(styled(&client), styled(&daemon));

        let exit: &[u8] = b"\x1b[?1049l";
        daemon.vt_write(exit);
        client.vt_write(exit);
        assert_eq!(plain(&client), "one\ntwo\nthree\nfour\n$ vim");
        assert_eq!(plain(&client), plain(&daemon));
        let cursor = |t: &Terminal| (t.cursor_x().unwrap(), t.cursor_y().unwrap());
        assert_eq!(cursor(&client), cursor(&daemon));
    }
}
