//! `wheelhouse issues`: the page's GitHub issues as a command, with its writes.

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
    /// The issues that are open.
    List,
    /// Open an issue and print its number.
    Create { title: String },
    /// Close an issue.
    Close { number: i64 },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let dir = match args.dir {
        Some(d) => d,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let done = |number: i64, did: &str| {
        if args.json {
            println!("{}", serde_json::json!({ "number": number, "ok": true, "did": did }));
        } else {
            println!("{did} #{number}");
        }
    };
    match args.command {
        Cmd::List => {
            let rows: Vec<Issue> = list(&dir).map_err(anyhow::Error::msg)?.into_iter().filter(|i| i.state == "open").collect();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for i in rows {
                    println!("#{}\t{}\t{}\t{}", i.number, i.state, i.labels.join(","), i.title);
                }
            }
        }
        Cmd::Create { title } => done(create(&dir, &title).map_err(anyhow::Error::msg)?, "opened"),
        Cmd::Close { number } => {
            close(&dir, number).map_err(anyhow::Error::msg)?;
            done(number, "closed");
        }
    }
    Ok(())
}
