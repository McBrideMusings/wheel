//! One terminal: a shell on a PTY, the screen it draws, and a broadcast of what comes next.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::terminal::{Options, Terminal};
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
    let options = Options {
        cols,
        rows,
        max_scrollback: SCROLLBACK_LINES,
    };
    let mut screen = match Terminal::new(options) {
        Ok(screen) => screen,
        Err(e) => {
            let _ = made.send(Err(anyhow::anyhow!("{e:?}")));
            return;
        }
    };
    let _ = made.send(Ok(()));
    let (tx, _) = broadcast::channel(1024);
    let mut exited = None;
    let mut pending = Pending::default();
    for msg in rx {
        match msg {
            ScreenMsg::Output(bytes) => {
                screen.vt_write(&bytes);
                pending.feed(&bytes);
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
                let mut replay = snapshot(&screen).unwrap_or_else(|e| {
                    tracing::warn!(term = %id, error = ?e, "screen snapshot failed; attaching blank");
                    Vec::new()
                });
                // The snapshot leaves the client's parser at rest; an escape sequence or character
                // the last output left unfinished goes after it, so the live output that
                // finishes it lands in the same place as on the daemon's screen.
                replay.extend_from_slice(&pending.bytes);
                let _ = reply.send(Attachment {
                    replay,
                    exited,
                    rx: tx.subscribe(),
                });
            }
        }
    }
}

/// VT output that draws `screen` on a reset terminal: its history and cells with their styles,
/// the modes that differ from the defaults (the alternate screen, cursor keys, mouse reporting,
/// bracketed paste), the scrolling region, and the cursor last. Tab stops are left out: setting
/// them moves the cursor after it is placed.
fn snapshot(screen: &Terminal) -> libghostty_vt::error::Result<Vec<u8>> {
    let options = FormatterOptions::new()
        .with_format(Format::Vt)
        .with_modes(true)
        .with_scrolling_region(true)
        .with_keyboard(true)
        .with_pwd(true)
        .with_cursor(true)
        .with_style(true)
        .with_hyperlink(true)
        .with_protection(true)
        .with_kitty_keyboard(true)
        .with_charsets(true);
    Ok(Formatter::new(screen, options)?
        .format_alloc(None)?
        .to_vec())
}

/// The longest unfinished sequence kept for an attach; past it the sequence is dropped from the
/// replay (an OSC 52 clipboard write or a Kitty image can run to megabytes).
const PENDING_CAP: usize = 1 << 20;

/// The output bytes since the terminal's parser last came to rest: an escape sequence or UTF-8
/// character that the output so far has started but not finished. A small copy of the VT parser's
/// states (ECMA-48: escape, CSI, OSC and the DCS/SOS/PM/APC strings), enough to know when a
/// sequence ends, never what it means.
#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    state: Parse,
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Parse {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    /// OSC: ends at BEL or ST.
    Osc,
    /// DCS, SOS, PM or APC: ends at ST.
    Text,
    /// ESC inside an OSC or string: `\` makes it ST.
    TextEscape,
    /// A UTF-8 character with this many continuation bytes still to come.
    Utf8(u8),
}

impl Pending {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state = self.step(b);
            if self.state == Parse::Ground {
                self.bytes.clear();
            } else if self.bytes.len() < PENDING_CAP {
                self.bytes.push(b);
            }
        }
    }

    fn step(&mut self, b: u8) -> Parse {
        const ESC: u8 = 0x1b;
        const BEL: u8 = 0x07;
        // CAN and SUB abort any sequence.
        if matches!(b, 0x18 | 0x1a) && self.state != Parse::Ground {
            return Parse::Ground;
        }
        match self.state {
            Parse::Ground | Parse::Utf8(_) if b == ESC => self.restart(),
            Parse::Ground => match b {
                0xc2..=0xdf => Parse::Utf8(1),
                0xe0..=0xef => Parse::Utf8(2),
                0xf0..=0xf4 => Parse::Utf8(3),
                _ => Parse::Ground,
            },
            Parse::Utf8(left) => match b {
                0x80..=0xbf if left > 1 => Parse::Utf8(left - 1),
                0x80..=0xbf => Parse::Ground,
                // A broken character: the parser drops it and reads this byte afresh.
                _ => {
                    self.bytes.clear();
                    self.state = Parse::Ground;
                    self.step(b)
                }
            },
            Parse::Escape => match b {
                ESC => self.restart(),
                b'[' => Parse::Csi,
                b']' => Parse::Osc,
                b'P' | b'X' | b'^' | b'_' => Parse::Text,
                0x20..=0x2f => Parse::EscapeIntermediate,
                // C0 controls run inside a sequence without ending it.
                0x00..=0x1f => Parse::Escape,
                _ => Parse::Ground,
            },
            Parse::EscapeIntermediate => match b {
                ESC => self.restart(),
                0x30..=0x7e => Parse::Ground,
                _ => Parse::EscapeIntermediate,
            },
            Parse::Csi => match b {
                ESC => self.restart(),
                0x40..=0x7e => Parse::Ground,
                _ => Parse::Csi,
            },
            Parse::Osc if b == BEL => Parse::Ground,
            Parse::Osc | Parse::Text => match b {
                ESC => Parse::TextEscape,
                _ => self.state,
            },
            Parse::TextEscape => match b {
                b'\\' => Parse::Ground,
                // Any other byte ends the string; the ESC starts a new sequence.
                _ => {
                    self.bytes.clear();
                    self.bytes.push(ESC);
                    self.state = Parse::Escape;
                    self.step(b)
                }
            },
        }
    }

    /// An ESC starts a new sequence, dropping the one in progress.
    fn restart(&mut self) -> Parse {
        self.bytes.clear();
        Parse::Escape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(chunks: &[&[u8]]) -> Vec<u8> {
        let mut p = Pending::default();
        for chunk in chunks {
            p.feed(chunk);
        }
        p.bytes
    }

    #[test]
    fn finished_output_leaves_nothing_pending() {
        assert_eq!(pending(&[b"plain \x1b[1;31mred\x1b[0m \xc3\xa9"]), b"");
        assert_eq!(
            pending(&[b"\x1b]0;title\x07", b"\x1b]8;;http://x\x1b\\"]),
            b""
        );
        assert_eq!(pending(&[b"\x1b(B\x1b7\x1bP+q\x1b\\"]), b"");
    }

    #[test]
    fn a_sequence_split_across_reads_stays_pending_until_it_ends() {
        assert_eq!(pending(&[b"text\x1b[3"]), b"\x1b[3");
        assert_eq!(pending(&[b"text\x1b[3", b"1"]), b"\x1b[31");
        assert_eq!(pending(&[b"text\x1b[3", b"1m"]), b"");
        assert_eq!(pending(&[b"\x1b]0;half a ti"]), b"\x1b]0;half a ti");
        assert_eq!(pending(&[b"\x1b]0;t\x1b"]), b"\x1b]0;t\x1b");
        assert_eq!(pending(&[b"\x1b]0;t\x1b", b"\\"]), b"");
        assert_eq!(pending(&[b"caf\xc3"]), b"\xc3");
        assert_eq!(pending(&[b"\xe2\x94"]), b"\xe2\x94");
        assert_eq!(pending(&[b"\x1b"]), b"\x1b");
    }

    #[test]
    fn a_new_escape_or_cancel_drops_the_unfinished_sequence() {
        assert_eq!(pending(&[b"\x1b[12\x1b[3"]), b"\x1b[3");
        assert_eq!(pending(&[b"\x1b[12\x18"]), b"");
        // An OSC cut off by another sequence: the ESC starts the new one.
        assert_eq!(pending(&[b"\x1b]0;t\x1b[1"]), b"\x1b[1");
        assert_eq!(pending(&[b"\xc3A\x1b["]), b"\x1b[");
    }

    /// The daemon's screen and a client given the snapshot plus the pending bytes end the same,
    /// even when an attach falls between two reads that split a sequence.
    #[test]
    fn a_client_attaching_mid_sequence_draws_what_the_daemon_draws() {
        let options = Options {
            cols: 20,
            rows: 3,
            max_scrollback: 100,
        };
        let mut daemon = Terminal::new(options).unwrap();
        let mut p = Pending::default();
        let first: &[u8] = b"ab\x1b[3";
        daemon.vt_write(first);
        p.feed(first);
        let mut replay = snapshot(&daemon).unwrap();
        replay.extend_from_slice(&p.bytes);
        let rest: &[u8] = b"1mred\x1b[0m";
        daemon.vt_write(rest);
        let mut client = Terminal::new(options).unwrap();
        client.vt_write(&replay);
        client.vt_write(rest);
        let plain = |t: &Terminal| {
            let opts = FormatterOptions::new().with_format(Format::Plain);
            Formatter::new(t, opts)
                .unwrap()
                .format_alloc(None)
                .unwrap()
                .to_vec()
        };
        assert_eq!(plain(&client), b"abred");
        assert_eq!(plain(&client), plain(&daemon));
    }
}
