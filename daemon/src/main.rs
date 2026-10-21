use std::io::Read as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// The wheel daemon: owns terminals and agent sessions; clients attach over a WebSocket.
#[derive(Parser)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = wheel_proto::DEFAULT_LISTEN)]
    listen: String,
    /// Which daemon this is: its files default to ~/Library/Application Support/<instance> and
    /// ~/Library/Logs/<instance>. The dev build runs as wheel-dev.
    #[arg(long, default_value = wheel_proto::DEFAULT_INSTANCE)]
    instance: String,
    /// This instance's own directory: its token, saved layout and `machine` artifacts.
    /// Defaults to ~/Library/Application Support/<instance>.
    #[arg(long)]
    support_dir: Option<PathBuf>,
    /// Directory for wheeld.log. Defaults to ~/Library/Logs/<instance>.
    #[arg(long)]
    log_dir: Option<PathBuf>,
    /// File holding the client token; created with mode 0600 if missing.
    /// Defaults to `token` in the support directory.
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// File the spaces and their layouts are saved to and restored from.
    /// Defaults to `spaces.json` in the support directory.
    #[arg(long)]
    state_file: Option<PathBuf>,
    /// Read Claude usage from this JSON file instead of the usage endpoint, for checks that need a
    /// known reading. The file holds the endpoint's answer, plus optional top-level `plan` and
    /// `email`; `{"fail": "<why>"}` makes the read fail and `{"not_logged_in": "<why>"}` makes the
    /// login missing. The file is read again on every read.
    #[arg(long)]
    usage_fixture: Option<PathBuf>,
    /// Take PATH from the user's interactive login shell rather than the environment wheeld
    /// started in. The app's launch agent passes it: launchd starts wheeld with only the system's
    /// PATH, where neither an agent's program nor most tools a terminal user expects are found.
    #[arg(long)]
    path_from_login_shell: bool,
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

fn main() -> anyhow::Result<()> {
    // A refresh's warden is this binary, and it must be watching before anything else runs.
    if std::env::args().nth(1).as_deref() == Some(wheeld::WARDEN_ARG) {
        wheeld::warden();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(Args::parse()))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let log_dir = args
        .log_dir
        .unwrap_or_else(|| home().join("Library/Logs").join(&args.instance));
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

    let support = match args.support_dir {
        Some(dir) => dir,
        None => wheel_proto::support_dir(&args.instance)?,
    };
    let token_path = args.token_file.unwrap_or_else(|| support.join("token"));
    let token = load_token(&token_path)?;
    // Caught rather than fatal, so main returns and `_guard` flushes the log's last lines.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    let listener_addr = listener.local_addr()?;
    tracing::info!(listen = %args.listen, instance = %args.instance, log = %log_path.display(), token_file = %token_path.display(), "wheeld listening");
    tracing::info!(support = %support.display(), "instance directory");
    let warden = std::env::current_exe()?;
    tracing::info!(warden = %warden.display(), "refresh warden program");
    let state = wheeld::AppState::new(
        log_path.display().to_string(),
        token,
        support.clone(),
        warden,
    );
    let state_file = args
        .state_file
        .unwrap_or_else(|| support.join("spaces.json"));
    // Before the layout, because restoring starts the terminals and agents that run with it.
    let own = std::env::var("PATH").unwrap_or_default();
    if args.path_from_login_shell {
        let shell = wheeld::login_path::shell();
        let (login_path, again) = tokio::task::spawn_blocking({
            let (shell, own) = (shell.clone(), own.clone());
            move || wheeld::login_path::ask(&shell, &own, 1)
        })
        .await?;
        state.set_login_path(login_path);
        if again {
            state.ask_login_path_again(shell, own);
        }
    } else {
        state.set_login_path(wheel_proto::LoginPath {
            path: own,
            how: "the PATH wheeld was started with; it was not asked to take the login shell's"
                .into(),
            problem: None,
        });
    }
    if let Some(path) = args.usage_fixture {
        state.set_usage_source(std::sync::Arc::new(wheeld::usage::FileUsage { path }));
    }
    // Before the layout, because restoring opens the terminals and agents that carry it.
    state.set_control(
        format!("ws://{}/ws", listener_addr),
        std::path::absolute(&token_path)?,
    );
    state.log_agents();
    if let Some(listen) = state.remote_listen() {
        // Not fatal: a port already taken must not take the local listener down with it.
        if let Err(e) = wheeld::remote::start(&state, &listen, &support).await {
            tracing::error!(%listen, error = %e, "the remote listener did not start; remote access is off");
            wheeld::remote::failed(&state, &listen, format!("{e:#}"));
        }
    }
    state.restore_spaces(state_file).await?;
    // The state this keeps is for the flush below: a browser pane's page is saved a moment after
    // it changes, so a daemon that stops in that moment has to write it before it goes — whichever
    // way it stops, a signal or serving ending on its own.
    enum Stopped {
        /// Serving ended on its own, with what it returned.
        Served(anyhow::Result<()>),
        /// A signal arrived, named for the log.
        Signal(&'static str),
    }
    let stopping = state.clone();
    let stopped = tokio::select! {
        served = wheeld::serve(listener, state) => Stopped::Served(served),
        _ = sigterm.recv() => Stopped::Signal("SIGTERM"),
        _ = sigint.recv() => Stopped::Signal("SIGINT"),
    };
    stopping.save_layout_now();
    match stopped {
        Stopped::Served(served) => served,
        Stopped::Signal(signal) => {
            tracing::info!(signal = %signal, "wheeld stopping");
            Ok(())
        }
    }
}
