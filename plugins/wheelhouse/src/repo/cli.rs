//! `wheelhouse repo`: the pane's branches as a command.

use clap::{Args as ClapArgs, Subcommand};

use super::*;

#[derive(ClapArgs)]
pub struct Args {
    /// The project directory (default: the current directory).
    #[arg(long, global = true)]
    dir: Option<String>,
    /// Print JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Every local branch with its worktree, state and PR.
    List {
        /// Only the branches that are safe to delete.
        #[arg(long)]
        dead: bool,
    },
    /// Delete a dead branch, and its worktree first when it has one.
    Delete { branch: String },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    match args.command {
        Cmd::List { dead } => {
            let snap = collect(&dir);
            if let Some(e) = &snap.error {
                anyhow::bail!("{e}");
            }
            let rows: Vec<&Branch> = snap.branches.iter().filter(|b| !dead || b.dead.is_some()).collect();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for b in rows {
                    let state = match (&b.dead, &b.blocked) {
                        (Some(w), None) => format!("dead: {w}"),
                        (Some(_), Some(w)) => format!("blocked: {w}"),
                        _ => String::new(),
                    };
                    println!("{}\t{}\t{}\t{}", b.branch, b.worktree.as_ref().map(|w| w.path.as_str()).unwrap_or("-"),
                        b.pr.as_ref().map(|p| format!("#{}", p.number)).unwrap_or_else(|| "-".into()), state);
                }
            }
        }
        Cmd::Delete { branch } => {
            let did = delete(&dir, &branch).map_err(anyhow::Error::msg)?;
            if args.json {
                println!("{}", serde_json::json!({ "branch": branch, "ok": true, "did": did }));
            } else {
                for d in did {
                    println!("{d}");
                }
            }
        }
    }
    Ok(())
}
