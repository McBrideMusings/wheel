//! `wheelhouse issues`: the page's tracker as a command, with its writes.

use clap::{Args as ClapArgs, Subcommand};

use crate::core::tracker;

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
    /// The items that are not closed.
    List {
        /// Only the items `bd ready` lists.
        #[arg(long)]
        ready: bool,
    },
    /// Create an item and print its id.
    Create {
        title: String,
        #[arg(long, default_value = "task")]
        r#type: String,
        #[arg(long, default_value_t = 2)]
        priority: i64,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Set an item's status, priority or title.
    Update {
        id: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
        #[arg(long)]
        title: Option<String>,
    },
    /// Close an item.
    Close {
        id: String,
        #[arg(long, default_value = "")]
        reason: String,
    },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let done = |id: &str, did: &str| {
        if args.json {
            println!("{}", serde_json::json!({ "id": id, "ok": true, "did": did }));
        } else {
            println!("{did} {id}");
        }
    };
    match args.command {
        Cmd::List { ready } => {
            let snap = collect(&dir);
            if let Some(e) = &snap.error {
                anyhow::bail!("{e}");
            }
            let rows: Vec<&Item> = snap.items.iter().filter(|i| i.status != "closed" && (!ready || i.ready)).collect();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for i in rows {
                    println!("{}\t{}\tP{}\t{}\t{}", i.id, i.status, i.priority.unwrap_or(2), i.kind, i.title);
                }
            }
        }
        Cmd::Create { title, r#type, priority, description } => {
            let id = tracker::create(&dir, &title, &r#type, priority, &description).map_err(anyhow::Error::msg)?;
            done(&id, "created");
        }
        Cmd::Update { id, status, priority, title } => {
            tracker::update(&dir, &id, status.as_deref(), priority, title.as_deref()).map_err(anyhow::Error::msg)?;
            done(&id, "updated");
        }
        Cmd::Close { id, reason } => {
            tracker::close(&dir, &id, &reason).map_err(anyhow::Error::msg)?;
            done(&id, "closed");
        }
    }
    Ok(())
}
