//! `wheelhouse`: a command line for a project's work, and, as `wheelhouse serve`, the same tools
//! as an MCP server whose pages a host (Wheel) draws as native controls. It runs with no host:
//! every page is also a command.

mod agents_cli;
mod backend;
mod beads;
mod core;
mod prs;
mod diff;
mod issues;
mod status;
mod repo;
mod serve;
mod view;
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
    /// Serve every view over MCP on standard input and output, for a host. One process serves
    /// every artifact and every project the host has open; the host names the artifact, and so
    /// the view, in each request.
    Serve,
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

/// Every view `serve` offers.
fn registry() -> view::Registry {
    view::Registry::default()
        .with(issues::serve::IssuesView::new())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => serve::run(registry()).await,
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
