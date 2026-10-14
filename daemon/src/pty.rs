//! One terminal: a shell on a PTY, the screen it draws, and a broadcast of what comes next.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use ghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use ghostty_vt::screen::Screen;
use ghostty_vt::terminal::{Mode, Options, Terminal};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, oneshot};
use wheel_proto::TermInfo;

/// Lines of history the daemon's copy of a screen keeps above it.
pub const SCROLLBACK_LINES: usize = 10_000;

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
}

impl Term {
    /// Fork the user's login shell on a new PTY. Blocking: call from a blocking context.
    pub fn spawn(id: String, cwd: String, cols: u16, rows: u16) -> Result<Arc<Term>> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pty")?;
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        let mut child = pair.slave.spawn_command(cmd).context("spawn shell")?;
        drop(pair.slave);
        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let mut writer = pair.master.take_writer().context("take pty writer")?;
        let (input, input_rx) = std_mpsc::channel::<Vec<u8>>();
        let (screen, screen_rx) = std_mpsc::channel::<ScreenMsg>();
        let term = Arc::new(Term {
            id,
            pid,
            cwd,
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            input,
            screen,
            exited: AtomicBool::new(false),
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
        std::thread::Builder::new()
            .name(format!("pty-s-{sid}"))
            .spawn(move || run_screen(sid, cols, rows, screen_rx, made))?;
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
                            let _ = t.screen.send(ScreenMsg::Output(buf[..n].to_vec()));
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

    /// What `wheel term read --until` does, for the lead: the screen as it is now, then live
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
    let (tx, _) = broadcast::channel(1024);
    let mut exited = None;
    for msg in rx {
        match msg {
            ScreenMsg::Output(bytes) => {
                screen.vt_write(&bytes);
                // Replies to the shell's queries come from the attached clients, not from here.
                screen.events().for_each(drop);
                let _ = tx.send(TermEvent::Output(Arc::from(bytes)));
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
/// bracketed paste), the scrolling region, and the cursor and pen last. Tab stops are left out:
/// setting them moves the cursor after it is placed.
fn snapshot(screen: &Terminal) -> ghostty_vt::error::Result<Vec<u8>> {
    let options = screen_options()
        .with_modes(true)
        .with_scrolling_region(true)
        .with_keyboard(true)
        .with_pwd(true);
    Ok(Formatter::new(screen, options)?.format_alloc()?.to_vec())
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
