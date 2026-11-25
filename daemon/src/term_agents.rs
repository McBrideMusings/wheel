//! A coding agent's TUI running in a terminal pane (Claude Code, Codex or pi), as `wheel agent`
//! verbs reach it. Wheel sees no events of such an agent, only which program holds the terminal's
//! foreground and what its hooks report (`wheel pane report`), so a message is typed into its
//! composer: pasted, then submitted with the key that program takes in every state.

use std::sync::Arc;
use std::time::Duration;

use wheel_proto::{ContextUse, PaneKind, ReportedStatus};

use crate::pty::Term;
use crate::resume::Harness;
use crate::server::AppState;

/// One message typed at a time, across every terminal: a send takes [`crate::submit::ENTER_DELAY`].
static TYPING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A terminal pane whose foreground program is a coding agent.
pub(crate) struct TermAgentRef {
    pub pane: String,
    pub term_id: String,
    pub term: Arc<Term>,
    pub harness: Harness,
    /// How full its context window is, as the terminal sampler last read it.
    pub context: Option<ContextUse>,
}

/// The bytes that put `text` in `harness`'s composer and the bytes that submit it.
///
/// The text goes in as a bracketed paste, so a newline inside it is a newline, never a submit.
/// Codex submits with Tab: it sends at once when idle and queues behind a running turn, where
/// Enter is refused in some states and only shows `tab to queue message`. Tab completes instead
/// in Codex's slash-command popup, so a `/command` takes Enter. Claude Code and pi take Enter in
/// every state, queueing mid-turn. A backslash just before Enter reads as a line continuation,
/// so a message ending in one gets a space after it.
pub(crate) fn keystrokes(harness: Harness, text: &str) -> Result<(Vec<u8>, &'static [u8]), String> {
    // A marker inside the text would end the paste early and type the rest as keys; a lone CR
    // is Enter to most TUIs.
    let text = text
        .replace("\x1b[200~", "")
        .replace("\x1b[201~", "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut text = text.trim_end().to_string();
    if text.trim().is_empty() {
        return Err("the message is empty".into());
    }
    if text.ends_with('\\') {
        text.push(' ');
    }
    let submit: &[u8] = match harness {
        Harness::Codex if !text.starts_with('/') => b"\t",
        _ => b"\r",
    };
    let paste = [b"\x1b[200~".as_slice(), text.as_bytes(), b"\x1b[201~"].concat();
    Ok((paste, submit))
}

impl AppState {
    /// The coding agent in terminal pane `target` (its id, or a mirror of it). None when `target`
    /// is not a terminal pane; an error when it is one with no coding agent in its foreground.
    pub(crate) fn terminal_agent(&self, target: &str) -> Option<Result<TermAgentRef, String>> {
        let (pane, term_id, context) = {
            let spaces = self.spaces.lock().unwrap();
            match wheel_proto::shown_pane(&spaces.spaces, target)? {
                (pane, PaneKind::Terminal { term, agent, .. }) => (
                    pane.to_string(),
                    term.clone(),
                    agent.as_ref().and_then(|a| a.context.clone()),
                ),
                _ => return None,
            }
        };
        let found = self
            .term(&term_id)
            .ok_or_else(|| format!("{pane}'s terminal is gone"))
            .and_then(|term| {
                let harness = term.foreground().and_then(crate::resume::harness_of).ok_or_else(|| {
                    format!(
                        "{pane} is a terminal pane with no coding agent running in it (Wheel recognises Claude Code, Codex and pi); type into it with `wheel term send`"
                    )
                })?;
                Ok(TermAgentRef { pane, term_id, term, harness, context })
            });
        Some(found)
    }

    /// What the agent in terminal `term_id` last reported through its hooks.
    pub(crate) fn term_agent_status(&self, term_id: &str) -> Option<ReportedStatus> {
        self.reports.lock().unwrap().get(term_id).map(|r| r.status)
    }

    /// Paste `text` into the agent's composer and submit it. Ok(true) when the agent was mid-turn,
    /// so the message is queued behind that turn.
    pub(crate) async fn term_agent_send(&self, found: &TermAgentRef, text: &str) -> Result<bool, String> {
        let status = self.term_agent_status(&found.term_id);
        if status == Some(ReportedStatus::NeedsYou) {
            return Err(format!(
                "{}'s {} waits on a permission prompt, and typed text would answer it; answer it in the pane, then send again",
                found.pane,
                found.harness.title()
            ));
        }
        let (paste, submit) = keystrokes(found.harness, text)?;
        // Held from the paste to the submit key, so a second send's paste never lands between them.
        let _typing = TYPING.lock().await;
        found.term.write(paste).map_err(|e| e.to_string())?;
        tokio::time::sleep(crate::submit::ENTER_DELAY).await;
        found.term.write(submit.to_vec()).map_err(|e| e.to_string())?;
        let queued = status == Some(ReportedStatus::Working);
        // The agent's own hook says so a moment later; marking it now keeps an `agent wait` sent
        // right after from reading the idle it reported before this message. Only for an agent whose
        // hooks report at all, and not for a slash command, which may run no turn and fire no Stop.
        if status.is_some() && !text.trim_start().starts_with('/') {
            self.reports.lock().unwrap().insert(
                found.term_id.clone(),
                crate::server::Report { status: ReportedStatus::Working, reason: None },
            );
            self.refresh_statuses();
        }
        tracing::info!(pane = %found.pane, term = %found.term_id, agent = found.harness.name(), ?status, queued, chars = text.len(), "agent send typed into a terminal agent");
        Ok(queued)
    }

    /// Wait until the agent's hooks report it idle, up to `timeout`; the status last seen and
    /// whether it was idle.
    pub(crate) async fn term_agent_wait(
        &self,
        found: &TermAgentRef,
        timeout: Duration,
    ) -> Result<(ReportedStatus, bool), String> {
        let mut global = self.global.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let Some(status) = self.term_agent_status(&found.term_id) else {
                return Err(format!(
                    "{}'s {} has reported no status, so Wheel cannot tell when it is ready: its hooks report only when it was started by Wheel's `{}` wrapper in a Wheel shell",
                    found.pane,
                    found.harness.title(),
                    found.harness.name()
                ));
            };
            if status == ReportedStatus::Idle {
                return Ok((status, true));
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(left, global.recv()).await.is_err() {
                return Ok((self.term_agent_status(&found.term_id).unwrap_or(status), false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_submits_with_tab_and_others_with_enter() {
        let (paste, submit) = keystrokes(Harness::Codex, "hello\nthere").unwrap();
        assert_eq!(paste, b"\x1b[200~hello\nthere\x1b[201~");
        assert_eq!(submit, b"\t");
        assert_eq!(keystrokes(Harness::Codex, "/status").unwrap().1, b"\r");
        assert_eq!(keystrokes(Harness::Claude, "hi").unwrap().1, b"\r");
        assert_eq!(keystrokes(Harness::Pi, "hi").unwrap().1, b"\r");
    }

    #[test]
    fn text_cannot_submit_early_or_continue_the_line() {
        let (paste, _) = keystrokes(Harness::Claude, "a\r\nb\rc \\\n").unwrap();
        assert_eq!(paste, b"\x1b[200~a\nb\nc \\ \x1b[201~");
        let (paste, _) = keystrokes(Harness::Codex, "x\x1b[201~y").unwrap();
        assert_eq!(paste, b"\x1b[200~xy\x1b[201~");
        assert!(keystrokes(Harness::Codex, " \n ").is_err());
    }
}
