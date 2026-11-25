use std::io::Read as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// The wheel daemon: owns terminals and agents; clients attach over a WebSocket.
#[derive(Parser)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = wheel_proto::DEFAULT_LISTEN)]
    listen: String,
    /// Which daemon this is: its files default to ~/Library/Application Support/<instance> and
    /// ~/Library/Logs/<instance> on macOS, ~/.local/share/<instance> and ~/.local/state/<instance>
    /// on Linux. The dev build runs as wheel-dev.
    #[arg(long, default_value = wheel_proto::DEFAULT_INSTANCE)]
    instance: String,
    /// This instance's own directory: its token and saved layout.
    /// Defaults to ~/Library/Application Support/<instance> (~/.local/share/<instance> on Linux).
    #[arg(long)]
    support_dir: Option<PathBuf>,
    /// This machine's `wheel.toml`: its settings and `machine` artifact entries, with those artifacts'
    /// folders in `artifacts/` beside it. Defaults to
    /// ~/.wheel/wheel.toml; a test or swarm daemon names its own so it never touches the real one.
    #[arg(long)]
    machine_file: Option<PathBuf>,
    /// Directory for wheeld.log. Defaults to ~/Library/Logs/<instance> (~/.local/state/<instance>
    /// on Linux).
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
    /// The `wheel` program to link into terminals and agents. The app's launch agent names
    /// `Helpers/wheel` in the app that installed this wheeld; a build run from cargo's target
    /// directory names its own.
    #[arg(long)]
    cli: Option<PathBuf>,
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
    /// Print the hash of the source this wheeld was built from and exit. The app's build writes it
    /// beside the bundled wheeld, so the app can tell whether the running one differs.
    #[arg(long)]
    source_hash: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Ask the wheeld running at `--listen` (its token from `--token-file` or this instance's
    /// support directory) for a one-time pairing code, and print it as JSON and as a terminal QR:
    /// what a device pairs with when this host has no screen and no `wheel`.
    Pair {
        /// The address the code names, one of those `wheel remote info` lists; a Tailscale
        /// address when there is one, else the first IP.
        #[arg(long)]
        address: Option<String>,
    },
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
    let args = Args::parse();
    if args.source_hash {
        println!("{}", env!("WHEELD_SOURCE_HASH"));
        return Ok(());
    }
    if let Some(Command::Pair { address }) = args.command {
        // Where the setting that turns remote access on lives, for the error when it is off.
        let setting = format!(
            "{} in {}",
            match args.instance == wheel_proto::DEFAULT_INSTANCE {
                true => "remote.listen".to_string(),
                false => format!("remote.{}.listen", args.instance),
            },
            args.machine_file.unwrap_or_else(|| home().join(".wheel/wheel.toml")).display()
        );
        let token_file = match args.token_file {
            Some(path) => path,
            None => match args.support_dir {
                Some(dir) => dir,
                None => wheel_proto::support_dir(&args.instance)?,
            }
            .join("token"),
        };
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(pair(&args.listen, &token_file, address, &setting));
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

/// `wheeld pair`: a client of the running wheeld, asking it the two things a pairing code holds.
async fn pair(
    listen: &str,
    token_file: &Path,
    address: Option<String>,
    setting: &str,
) -> anyhow::Result<()> {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;
    use wheel_proto::{ClientMsg, ServerMsg};
    let token = std::fs::read_to_string(token_file)
        .map_err(|e| anyhow::anyhow!("read token from {}: {e}", token_file.display()))?;
    let url = format!("ws://{listen}/ws?token={}", token.trim());
    let wait = std::time::Duration::from_secs(10);
    let (mut ws, _) = tokio::time::timeout(wait, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| anyhow::anyhow!("wheeld at {listen} took no connection within 10s"))?
        .map_err(|e| anyhow::anyhow!("connect to wheeld at {listen}: {e}"))?;
    let mut call = async |msg: ClientMsg, req: u64| -> anyhow::Result<ServerMsg> {
        ws.send(Message::Text(serde_json::to_string(&msg)?.into())).await?;
        tokio::time::timeout(wait, async {
            while let Some(frame) = ws.next().await {
                let Message::Text(text) = frame? else { continue };
                // Frames this client did not ask for, such as pushed events, are skipped.
                let Ok(reply) = serde_json::from_str::<ServerMsg>(&text) else { continue };
                if reply.req() != Some(req) {
                    continue;
                }
                if let ServerMsg::Error { message, .. } = reply {
                    anyhow::bail!("{message}");
                }
                return Ok(reply);
            }
            anyhow::bail!("wheeld closed the connection")
        })
        .await
        .map_err(|_| anyhow::anyhow!("no reply from wheeld within 10s"))?
    };
    let ServerMsg::Remote { listen: remote, addresses, port, fingerprint, problem, .. } =
        call(ClientMsg::RemoteInfo { req: Some(1) }, 1).await?
    else {
        anyhow::bail!("unexpected reply to remote_info");
    };
    anyhow::ensure!(remote.is_some(), "remote access is off: set {setting}");
    if let Some(problem) = problem {
        anyhow::bail!("remote access did not start: {problem}");
    }
    let address =
        wheel_proto::pairing::choose_address(&addresses, address.as_deref()).map_err(anyhow::Error::msg)?;
    let ServerMsg::Pairing { token, .. } = call(ClientMsg::PairingIssue { req: Some(2) }, 2).await? else {
        anyhow::bail!("unexpected reply to pairing_issue");
    };
    let code = wheel_proto::pairing::code(&fingerprint, &token, &address, port);
    println!("{code}");
    print!("{}", wheel_proto::pairing::qr(&code).map_err(anyhow::Error::msg)?);
    Ok(())
}

async fn run(args: Args) -> anyhow::Result<()> {
    let log_dir = match args.log_dir {
        Some(dir) => dir,
        None => wheel_proto::log_dir(&args.instance)?,
    };
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("wheeld.log");
    let support = match args.support_dir {
        Some(dir) => dir,
        None => wheel_proto::support_dir(&args.instance)?,
    };
    // Rolled by the same rule as the journal: past its cap once the log store has every line.
    let log_file = wheeld::logship::RollingFile::open(
        log_path.clone(),
        wheeld::logship::position_path(&support, &log_path),
    )?;
    let (file_writer, _guard) = tracing_appender::non_blocking(log_file);
    let filter = EnvFilter::try_from_env("WHEEL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file_writer).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

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
    let machine_file = args
        .machine_file
        .unwrap_or_else(|| home().join(".wheel/wheel.toml"));
    tracing::info!(machine_file = %machine_file.display(), "machine wheel.toml");
    let state = wheeld::AppState::with_machine_file(
        log_path.display().to_string(),
        token,
        support.clone(),
        machine_file,
        warden,
    );
    anyhow::ensure!(
        !state.server_id().is_empty(),
        "cannot write {}: this server needs an id clients can know it by",
        support.join("server-id").display()
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
        let source = |agent| {
            std::sync::Arc::new(wheeld::usage::FileUsage {
                path: path.clone(),
                agent,
            })
        };
        state.set_usage_sources(source("claude"), source("codex"));
    }
    // Before the layout, because restoring opens the terminals and agents that carry it.
    state.set_control(
        format!("ws://{}/ws", listener_addr),
        std::path::absolute(&token_path)?,
        args.cli,
    );
    state.log_agents();
    state.start_title_buttons();
    if let Some(listen) = state.remote_listen(&args.instance) {
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
    stopping.stop_streams();
    stopping.stop_backends().await;
    stopping.save_layout_now();
    // After the flush, so a slow client never delays saving; before the exit, so a client sees
    // `stopping` and not just a dropped socket.
    stopping.announce_stopping().await;
    match stopped {
        Stopped::Served(served) => served,
        Stopped::Signal(signal) => {
            tracing::info!(signal = %signal, "wheeld stopping");
            Ok(())
        }
    }
}
