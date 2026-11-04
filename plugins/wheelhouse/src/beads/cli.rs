//! `wheelhouse beads`: the page's list as a command.

use super::*;
use clap::{Args as ClapArgs, Subcommand};

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
    /// The rows of a view, as the page lists them.
    List {
        /// list, epics, deps or unlocks.
        #[arg(long, default_value = "list")]
        view: String,
        /// Words every row must contain.
        #[arg(long, default_value = "")]
        search: String,
        /// A status: in progress, ready, blocked, open, deferred or done.
        #[arg(long)]
        status: Option<String>,
        #[arg(long = "type")]
        kind: Option<String>,
        #[arg(long)]
        label: Option<String>,
        /// A priority, 0 to 4.
        #[arg(long)]
        pri: Option<String>,
        /// Show done beads too.
        #[arg(long)]
        all: bool,
    },
    /// One bead with what it waits on, what waits on it and its children.
    Show { id: String },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let snap = collect(&dir);
    anyhow::ensure!(snap.has_repo, "{dir} has no .beads directory");
    if let Some(e) = &snap.error {
        anyhow::bail!("{e}");
    }
    let ix = Ix::new(&snap);
    match args.command {
        Cmd::List { view, search, status, kind, label, pri, all } => {
            let mut ui = Ui { view, q: search, hide: !all, ..Ui::default() };
            for (k, v) in [("status", status), ("type", kind), ("label", label), ("pri", pri)] {
                if let Some(v) = v {
                    ui.f.insert(k.into(), v);
                }
            }
            let rows = build(&ix, &ui);
            if args.json {
                let out: Vec<Value> = rows
                    .iter()
                    .map(|r| json!({ "id": r.b.id, "title": r.b.title, "status": kind_of(r.b), "type": r.b.kind,
                        "priority": r.b.priority, "depth": r.depth }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for r in &rows {
                    println!("{}{}\t{}\t{}\t{}", "  ".repeat(r.depth), ix.short(&r.b.id), kind_of(r.b),
                        r.b.priority.map(|p| format!("P{p}")).unwrap_or_default(), r.b.title);
                }
            }
        }
        Cmd::Show { id } => {
            let b = ix.by_id.get(id.as_str()).copied().ok_or_else(|| anyhow::anyhow!("no bead {id}"))?;
            let names = |v: Vec<&Bead>| v.iter().map(|x| x.id.clone()).collect::<Vec<_>>();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&json!({
                    "id": b.id, "title": b.title, "status": kind_of(b), "type": b.kind, "priority": b.priority,
                    "labels": b.labels, "owner": b.owner, "parent": b.parent, "description": b.description,
                    "depends_on": names(ix.blockers(b)), "unlocks": names(ix.unlocked(b)), "children": names(ix.children(b)),
                }))?);
            } else {
                println!("{}  {}\n{} · {} · {}", b.id, b.title, kind_of(b), b.kind,
                    b.priority.map(|p| format!("P{p}")).unwrap_or_else(|| "no priority".into()));
                for (label, v) in [("Depends on", ix.blockers(b)), ("Unlocks", ix.unlocked(b)), ("Children", ix.children(b))] {
                    println!("{label}: {}", names(v).join(", "));
                }
                if !b.description.is_empty() {
                    println!("\n{}", b.description);
                }
            }
        }
    }
    Ok(())
}
