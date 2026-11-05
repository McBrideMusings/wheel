//! `wheelhouse prs`: the page's list as a command.

use clap::Args as ClapArgs;

use super::*;

#[derive(ClapArgs)]
pub struct Args {
    /// The project directory (default: the current directory).
    #[arg(long)]
    dir: Option<String>,
    /// Only one bucket: mine, needs-me, waiting or other.
    #[arg(long)]
    bucket: Option<String>,
    /// Print JSON.
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
    let rows: Vec<&Pr> = snap.prs.iter().filter(|p| args.bucket.as_deref().map_or(true, |b| b == p.bucket)).collect();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for p in rows {
            println!("#{}\t{}\t{}\t{}\t{}", p.number, p.bucket, p.checks, p.author, p.title);
        }
    }
    Ok(())
}
