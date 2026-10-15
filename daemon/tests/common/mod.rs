//! Shared by the daemon's integration tests.

use std::sync::Arc;
use tokio::task::JoinHandle;

/// A daemon serving in this process. `abort` stops it serving and ends its agent sessions, so the
/// agent processes do not run on until the test process exits.
pub struct Daemon {
    serving: JoinHandle<anyhow::Result<()>>,
    state: Arc<wheeld::AppState>,
}

impl Daemon {
    pub fn new(serving: JoinHandle<anyhow::Result<()>>, state: Arc<wheeld::AppState>) -> Self {
        Self { serving, state }
    }

    pub fn abort(&self) {
        self.serving.abort();
        self.state.stop_sessions();
    }
}
