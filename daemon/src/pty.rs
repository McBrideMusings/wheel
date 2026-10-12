//! One terminal: a shell on a PTY, the tail of its output, and a broadcast of what comes next.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;
use wheel_proto::TermInfo;

/// How much output a terminal keeps for replay to a client that attaches later.
pub const REPLAY_CAP: usize = 1 << 20;

#[derive(Clone, Debug)]
pub enum TermEvent {
    Output(Arc<[u8]>),
    Exited(Option<u32>),
}

/// What an attach sees, taken at one instant: the buffered output, the exit status if the shell
/// already ended, and a receiver for everything after.
pub struct Attachment {
    pub replay: Vec<u8>,
    pub exited: Option<Option<u32>>,
    pub rx: broadcast::Receiver<TermEvent>,
}

/// The replay buffer and exit status share one lock with every broadcast send, so an attach
/// never misses or repeats a byte and never misses the exit.
struct Tail {
    bytes: VecDeque<u8>,
    exited: Option<Option<u32>>,
}

pub struct Term {
    pub id: String,
    pub pid: Option<u32>,
    pub cwd: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    input: std_mpsc::Sender<Vec<u8>>,
    tail: Mutex<Tail>,
    tx: broadcast::Sender<TermEvent>,
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
        let (tx, _) = broadcast::channel(1024);
        let term = Arc::new(Term {
            id,
            pid,
            cwd,
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            input,
            tail: Mutex::new(Tail {
                bytes: VecDeque::with_capacity(64 * 1024),
                exited: None,
            }),
            tx,
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

        let t = term.clone();
        std::thread::Builder::new()
            .name(format!("pty-r-{}", t.id))
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => t.record(&buf[..n]),
                    }
                }
                let code = child.wait().ok().map(|s| s.exit_code());
                tracing::info!(term = %t.id, ?code, "pty exited");
                let mut tail = t.tail.lock().unwrap();
                tail.exited = Some(code);
                let _ = t.tx.send(TermEvent::Exited(code));
            })?;
        Ok(term)
    }

    fn record(&self, bytes: &[u8]) {
        let mut tail = self.tail.lock().unwrap();
        tail.bytes.extend(bytes);
        let excess = tail.bytes.len().saturating_sub(REPLAY_CAP);
        tail.bytes.drain(..excess);
        let _ = self.tx.send(TermEvent::Output(Arc::from(bytes)));
    }

    pub fn attach(&self) -> Attachment {
        let tail = self.tail.lock().unwrap();
        Attachment {
            replay: tail.bytes.iter().copied().collect(),
            exited: tail.exited,
            rx: self.tx.subscribe(),
        }
    }

    /// Queue input for the shell. Never blocks.
    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        self.input
            .send(bytes)
            .map_err(|_| anyhow::anyhow!("terminal {} no longer accepts input", self.id))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
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
        let tail = self.tail.lock().unwrap();
        TermInfo {
            id: self.id.clone(),
            pid: self.pid,
            cwd: self.cwd.clone(),
            cols: size.map(|s| s.cols).unwrap_or(0),
            rows: size.map(|s| s.rows).unwrap_or(0),
            alive: tail.exited.is_none(),
            buffered_bytes: tail.bytes.len(),
        }
    }
}
