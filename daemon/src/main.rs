use std::io::Read as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// The wheel daemon: owns terminals and agent sessions; clients attach over a WebSocket.
#[derive(Parser)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = wheel_proto::DEFAULT_LISTEN)]
    listen: String,
    /// Directory for wheeld.log. Defaults to ~/Library/Logs/wheel.
    #[arg(long)]
    log_dir: Option<PathBuf>,
    /// File holding the client token; created with mode 0600 if missing.
    /// Defaults to ~/Library/Application Support/wheel/token.
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Command that runs the lead's ACP agent.
    #[arg(long, default_value = "claude-agent-acp")]
    lead_cmd: String,
    /// Working directory for the lead session. Defaults to the home directory.
    #[arg(long)]
    lead_cwd: Option<PathBuf>,
    /// Run without a lead session.
    #[arg(long)]
    no_lead: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Read the client token, creating a random one readable only by this user if none exists.
fn load_token(path: &Path) -> anyhow::Result<String> {
    if let Ok(token) = std::fs::read_to_string(path) {
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        anyhow::ensure!(
            mode & 0o077 == 0,
            "{} is readable by other users (mode {mode:o}); chmod 600 it",
            path.display()
        );
        return Ok(token.trim().to_string());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    std::io::Write::write_all(&mut file, token.as_bytes())?;
    Ok(token)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let log_dir = args
        .log_dir
        .unwrap_or_else(|| home().join("Library/Logs/wheel"));
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("wheeld.log");
    let (file_writer, _guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(&log_dir, "wheeld.log"));
    let filter = EnvFilter::try_from_env("WHEEL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    let token_path = match args.token_file {
        Some(path) => path,
        None => wheel_proto::default_token_path()?,
    };
    let token = load_token(&token_path)?;
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, log = %log_path.display(), token_file = %token_path.display(), "wheeld listening");
    let state = wheeld::AppState::new(log_path.display().to_string(), token);
    if !args.no_lead {
        state.start_lead(wheeld::lead::LeadConfig {
            command: args.lead_cmd,
            cwd: args.lead_cwd.unwrap_or_else(home),
            context: wheeld::lead::ORCHESTRATOR_CONTEXT.to_string(),
        });
    }
    wheeld::serve(listener, state).await
}
