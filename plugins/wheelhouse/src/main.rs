//! `wheelhouse`: a command line for a project's work, and, as `wheelhouse serve`, the same tools
//! as an MCP server whose pages a host (Wheel) draws as native controls. It runs with no host:
//! every page is also a command.

mod beads;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "wheelhouse", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the pages over MCP on standard input and output, for a host. One process serves every
    /// project the host has open.
    Serve,
    /// A project's beads.
    Beads(beads::cli::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve => beads::serve::run().await,
        Command::Beads(args) => beads::cli::run(args),
    }
}
