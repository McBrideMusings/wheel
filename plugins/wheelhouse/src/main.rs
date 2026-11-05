//! `wheelhouse`: a command line for a project's work, and, as `wheelhouse serve`, the same tools
//! as an MCP server whose pages a host (Wheel) draws as native controls. It runs with no host:
//! every page is also a command.

mod beads;
mod core;
mod serve;
mod view;

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
    /// A project's beads.
    Beads(beads::cli::Args),
}

/// Every view `serve` offers.
fn registry() -> view::Registry {
    view::Registry::default().with(beads::serve::BeadsView::new())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => serve::run(registry()).await,
        Command::Beads(args) => beads::cli::run(args),
    }
}
