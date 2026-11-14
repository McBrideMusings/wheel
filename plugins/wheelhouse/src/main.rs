//! `wheelhouse`: a command line for a project's work, and, as `wheelhouse backend`, the Wheel
//! plugin backend that publishes the same data as feeds and actions for the package's Luau panes.

mod agents_cli;
mod backend;
mod beads;
mod core;
mod prs;
mod diff;
mod issues;
mod status;
mod repo;
mod work;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "wheelhouse", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the plugin feeds and actions to `wheeld` as a plugin backend: JSON-RPC 2.0 with
    /// Content-Length framing on standard input and output. One process serves every project;
    /// each feed names its project directory in its params.
    Backend,
    /// The agents Wheel runs in a project, and starting one.
    Agents(agents_cli::Args),
    /// A project's beads.
    Beads(beads::cli::Args),
    /// A project's branches and worktrees.
    Repo(repo::cli::Args),
    /// A project's uncommitted and branch diffs.
    Diff(diff::cli::Args),
    /// A project's open pull requests.
    Prs(prs::cli::Args),
    /// A project's tracker: list, create, update and close items.
    Issues(issues::cli::Args),
    /// A project's status against its vision.
    Project(status::cli::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Backend => backend::run().await,
        Command::Agents(args) => agents_cli::run(args, core::agents::Agents::new("wheel")).await,
        Command::Beads(args) => beads::cli::run(args),
        Command::Repo(args) => repo::cli::run(args),
        Command::Diff(args) => diff::cli::run(args),
        Command::Prs(args) => prs::cli::run(args),
        Command::Issues(args) => issues::cli::run(args),
        Command::Project(args) => status::cli::run(args),
    }
}
