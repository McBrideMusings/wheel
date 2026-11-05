//! `wheelhouse project`: the page's status as a command, with its one write.

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
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Replace the vision, the first paragraph of docs/CONTEXT.md.
    Vision { text: String },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    if let Some(Cmd::Vision { text }) = args.command {
        tracker::set_vision(&dir, &text).map_err(anyhow::Error::msg)?;
        println!("{}", if args.json { serde_json::json!({ "ok": true }).to_string() } else { "vision set".into() });
        return Ok(());
    }
    let snap = collect(&dir);
    if let Some(e) = &snap.error {
        anyhow::bail!("{e}");
    }
    let c = &snap.counts;
    if args.json {
        println!("{}", serde_json::json!({ "name": snap.name, "vision": snap.vision, "counts": {
            "done": c.done, "in_progress": c.progress, "ready": c.ready, "blocked": c.blocked, "open": c.open, "deferred": c.deferred },
            "epics": snap.epics.iter().map(|e| serde_json::json!({ "id": e.id, "title": e.title, "done": e.done, "all": e.all })).collect::<Vec<_>>() }));
    } else {
        println!("{}", snap.name);
        if let Some(v) = &snap.vision {
            println!("{v}");
        }
        println!("done {}  in progress {}  ready {}  blocked {}  open {}  deferred {}", c.done, c.progress, c.ready, c.blocked, c.open, c.deferred);
        for e in &snap.epics {
            println!("{}\t{}/{}\t{}", e.id, e.done, e.all, e.title);
        }
    }
    Ok(())
}
