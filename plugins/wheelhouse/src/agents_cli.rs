//! `wheelhouse agents`: the agents Wheel runs in a project, and starting one, as commands.

use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use crate::core::agents::{in_project, Agents};
use crate::core::project::Origin;

#[derive(ClapArgs)]
pub struct Args {
    /// The project directory (default: the current directory).
    #[arg(long, global = true)]
    dir: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The project's agent panes and what each is doing, as JSON.
    Ls,
    /// Start an agent in a new pane beside a pane and give it a brief; prints the new pane.
    Spawn {
        brief: String,
        /// The pane to split (default: the pane running this command, `$WHEEL_PANE`).
        #[arg(long)]
        pane: Option<String>,
        /// Which agent to run, by name.
        #[arg(long)]
        agent: Option<String>,
        /// The model to run it on.
        #[arg(long)]
        model: Option<String>,
    },
}

pub async fn run(args: Args, agents: Agents) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => Origin::from_map(&Default::default()).dir,
    };
    match args.command {
        Cmd::Ls => {
            let mut changes = agents.changes();
            agents.acquire();
            // The first list the daemon sends is the answer.
            let got = tokio::time::timeout(Duration::from_secs(10), changes.changed()).await;
            let all = agents.all();
            agents.release();
            got.map_err(|_| anyhow::anyhow!("wheel agent watch sent no list within 10 seconds"))??;
            println!("{}", serde_json::to_string_pretty(&json!(in_project(&all, &dir).iter().map(|a| json!({
                "space": a.space, "pane": a.pane, "agent": a.agent, "harness": a.harness, "model": a.model, "status": a.status,
                "items": a.work.as_ref().map(|w| w.items.iter().map(|i| format!("{} {}", i.kind, i.id)).collect::<Vec<_>>()).unwrap_or_default(),
                "stage": a.work.as_ref().and_then(|w| w.stage.clone()), "note": a.work.as_ref().and_then(|w| w.note.clone()),
            })).collect::<Vec<_>>()))?);
            Ok(())
        }
        Cmd::Spawn { brief, pane, agent, model } => {
            let mut a = serde_json::Map::new();
            a.insert("brief".into(), brief.into());
            if let Some(v) = agent {
                a.insert("agent".into(), v.into());
            }
            if let Some(v) = model {
                a.insert("model".into(), v.into());
            }
            let pane = pane.or_else(|| std::env::var("WHEEL_PANE").ok()).ok_or_else(|| anyhow::anyhow!("give --pane or run in a Wheel pane"))?;
            let origin = Origin { dir, pane, artifact: None, data: None };
            let result = agents.spawn(&origin, &a);
            let text = serde_json::to_value(&result.content[0])?["text"].as_str().unwrap_or("").to_string();
            if result.is_error == Some(true) {
                anyhow::bail!("{text}");
            }
            println!("{text}");
            Ok(())
        }
    }
}
