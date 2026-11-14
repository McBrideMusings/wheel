//! `wheelhouse diff`: the pane's files as a command.

use clap::Args as ClapArgs;

use super::*;

#[derive(ClapArgs)]
pub struct Args {
    /// The project directory (default: the current directory).
    #[arg(long)]
    dir: Option<String>,
    /// uncommitted, default, base0 or base1.
    #[arg(long, default_value = "uncommitted")]
    scope: String,
    /// Print the hunks too, as JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let snap = collect(&dir);
    if let Some(e) = &snap.error {
        anyhow::bail!("{e}");
    }
    let keys: Vec<&str> = snap.scopes.iter().map(|s| s.key.as_str()).collect();
    let scope = snap.scopes.iter().find(|s| s.key == args.scope).ok_or_else(|| anyhow::anyhow!("no scope {}; scopes: {}", args.scope, keys.join(", ")))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(scope)?);
    } else {
        for f in &scope.files {
            println!("{}\t+{}\t-{}\t{}", f.status, f.add, f.del, f.path);
        }
    }
    Ok(())
}
