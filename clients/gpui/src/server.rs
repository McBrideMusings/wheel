//! The server as the client controls it (ServerControl.swift) and the words the client uses while a
//! restart waits (AppModel.serverUpdatedText). A client built for `PROTOCOL` that meets an older
//! wheeld hides what that wheeld lacks and offers Restart Server; it never restarts wheeld on its
//! own (ADR-0020). `restart` blocks on the commands, so `view/toasts.rs` runs it on a thread.

/// What the banner, the notice and Settings say while a restart waits.
pub const UPDATED: &str = "The server was updated.";
/// Why Restart Server is not offered for a wheeld the client has no commands for.
pub const UNAVAILABLE: &str = "This server was started by something other than Wheel, so Wheel cannot stop it.";

/// How the client stops and starts the wheeld it talks to: the commands in `WHEEL_SERVER_STOP`
/// and `WHEEL_SERVER_START` for a wheeld the app did not install (CommandControl.swift), or no
/// way at all. The launch agent the app's bundled wheeld runs as is wheel-ynjo.3's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Control {
    pub stop: Option<String>,
    pub start: Option<String>,
}

impl Control {
    pub fn from_env() -> Self {
        let read = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Control { stop: read("WHEEL_SERVER_STOP"), start: read("WHEEL_SERVER_START") }
    }
    /// Why the client cannot stop and start this server; None when it can.
    pub fn unavailable(&self) -> Option<&'static str> {
        (self.stop.is_none() && self.start.is_none()).then_some(UNAVAILABLE)
    }
    pub fn available(&self) -> bool {
        self.unavailable().is_none()
    }
    /// Stop, then start. Ok when both ran; else why not, in the Swift client's words.
    pub fn restart(&self) -> Result<(), String> {
        run(self.stop.as_deref())?;
        run(self.start.as_deref())
    }
}

fn run(command: Option<&str>) -> Result<(), String> {
    let Some(command) = command else { return Err("No command is set for this.".into()) };
    match std::process::Command::new("/bin/sh").arg("-c").arg(command).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("The command exited with {}.", status.code().unwrap_or(-1))),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_commands_means_no_restart_offered() {
        assert_eq!(Control::default().unavailable(), Some(UNAVAILABLE));
        assert!(Control { stop: Some("true".into()), start: None }.available());
    }

    #[test]
    fn restart_runs_stop_then_start_and_reports_the_first_failure() {
        let dir = std::env::temp_dir().join(format!("wheel-gpui-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let both = Control { stop: Some(format!("touch {}/stop", dir.display())), start: Some(format!("touch {}/start", dir.display())) };
        assert_eq!(both.restart(), Ok(()));
        assert!(dir.join("stop").exists() && dir.join("start").exists());
        let failing = Control { stop: Some("exit 3".into()), start: Some(format!("touch {}/never", dir.display())) };
        assert_eq!(failing.restart().unwrap_err(), "The command exited with 3.");
        assert!(!dir.join("never").exists(), "start ran after a failed stop");
        let half = Control { stop: None, start: Some("true".into()) };
        assert_eq!(half.restart().unwrap_err(), "No command is set for this.");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
