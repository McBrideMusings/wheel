//! `wheel-runner`: runs the commands a project's `wheel.toml` `[runner]` table names and keeps
//! each run's log, status and exit code. As `wheel-runner backend` it is the backend of Wheel's
//! `runner` plugin; the other commands work with no host.

mod backend;
mod check;
mod config;
mod guards;
mod runs;
mod status;
mod store;
mod sweep;

use std::io::{BufRead, Write};
use std::sync::Arc;

use clap::{Parser, Subcommand};

use runs::{Runs, StartError};

#[derive(Parser)]
#[command(name = "wheel-runner", version, about)]
struct Cli {
    /// The project's directory. Default: the current directory.
    #[arg(long, global = true)]
    dir: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the runner's feeds and actions to `wheeld` as a plugin backend: JSON-RPC 2.0 on
    /// standard input and output.
    Backend,
    /// The commands the project's [runner] table names, and this machine's.
    Ls,
    /// Run a [runner] command here, showing its output; exits with the command's exit code.
    Run {
        name: String,
        /// Answer yes to the command's confirm question.
        #[arg(long, short)]
        yes: bool,
        /// Run in this git worktree of the project; the entry must set `worktree = true`.
        #[arg(long)]
        worktree: Option<String>,
    },
    /// A command's status check: whether the thing it runs is up, and what it reads.
    Check { name: String },
    /// The project's runs, newest first.
    Runs,
    /// A run's log.
    Log {
        id: String,
        /// Only the last this many lines.
        #[arg(long)]
        tail: Option<usize>,
    },
    /// End a running run and everything it started.
    Stop { id: String },
    /// The status line.
    Status,
    #[command(name = "__watch", hide = true)]
    Watch { id: String, folder: String },
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// `HH:MM:SS`, UTC, of the Unix time `ts`.
pub fn clock(ts: i64) -> String {
    let s = ts.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// `0:42`, `1:12`, `1:02:03`: a length of time in seconds.
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn asked_yes(question: &str) -> bool {
    // SAFETY: isatty only reads the descriptor.
    if unsafe { libc::isatty(0) } == 0 {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dir = cli.dir.clone().or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned())).unwrap_or_default();
    let project = Runs::new(&dir);
    match cli.command {
        Command::Watch { id, folder } => runs::watch(&id, std::path::Path::new(&folder)),
        Command::Backend => tokio::runtime::Runtime::new()?.block_on(backend::run())?,
        Command::Ls => {
            for r in config::list(&config::machine_file(), &dir) {
                let ask = r.confirm.as_deref().map(|q| format!("  (asks: {q})")).unwrap_or_default();
                println!("{:<20} {}{ask}", r.name, r.command);
            }
        }
        Command::Run { name, yes, worktree } => {
            let project = match &worktree {
                Some(w) => project.in_worktree(w),
                None => project,
            };
            let Some(runner) = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == name) else {
                anyhow::bail!("no runner named {name}");
            };
            let mirror: runs::Mirror = Arc::new(|line| println!("{line}"));
            let started = match project.start(&runner, yes.then_some(runner.command.as_str()), Some(mirror.clone())) {
                Err(StartError::Confirm(q)) if asked_yes(&q) => project.start(&runner, Some(runner.command.as_str()), Some(mirror)),
                Err(StartError::Guard(m)) => anyhow::bail!("{name} did not start: {m}"),
                Err(StartError::Confirm(q)) => anyhow::bail!("{name} asks first: {q} (run with --yes to answer yes)"),
                other => other,
            };
            let started = started.map_err(|e| anyhow::anyhow!("{e:?}"))?;
            eprintln!("run {}  log {}", started.record.id, started.record.log);
            let record = started.done.join().map_err(|_| anyhow::anyhow!("the run's thread panicked"))?;
            std::process::exit(record.exit_code.unwrap_or(1));
        }
        Command::Check { name } => {
            let runner = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == name)
                .ok_or_else(|| anyhow::anyhow!("no runner named {name}"))?;
            let check = runner.status.ok_or_else(|| anyhow::anyhow!("{name} has no status check"))?;
            let reading = check::run(&check, std::path::Path::new(&dir));
            println!("{} {}", if reading.up { "up" } else { "down" }, reading.text);
            std::process::exit(if reading.up { 0 } else { 1 });
        }
        Command::Runs => {
            for r in project.list() {
                let code = r.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into());
                println!("{}  {:<10} {:<5} {}  {}", r.id, format!("{:?}", r.state).to_lowercase(), code, clock(r.started), r.name);
            }
        }
        Command::Log { id, tail } => {
            let (text, _) = project.log(&id, 0, tail).map_err(|e| anyhow::anyhow!(e))?;
            print!("{text}");
        }
        Command::Stop { id } => {
            project.stop(&id).map_err(|e| anyhow::anyhow!(e))?;
            println!("stopping {id}");
        }
        Command::Status => println!("{}", status::line(project.latest().as_ref(), now())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_duration_format() {
        assert_eq!(clock(3 * 3600 + 7 * 60 + 9), "03:07:09");
        assert_eq!(clock(86400 + 5), "00:00:05");
        assert_eq!(duration(72), "1:12");
        assert_eq!(duration(3723), "1:02:03");
    }
}
