mod app;
mod keyring;
mod ui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sei", about = "Save Env Inject — manage .env secrets in GNOME Keyring")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a command with injected env vars (headless)
    Run {
        /// Stage name
        #[arg(short, long, default_value = "default")]
        stage: String,

        /// Project path (default: current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Entry ID (3-digit, e.g. 001)
        #[arg(short, long)]
        id: Option<String>,

        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // ID shorthand: sei 001 cmd args...
    if args.len() >= 2 {
        if let Some(id) = args.first() {
            if id.len() == 3 && id.chars().all(|c| c.is_ascii_digit()) {
                let cmd = &args[1..];
                return run_with_id(id, cmd).await;
            }
        }
    }

    let cli = Cli::parse();

    match cli.command {
        None => {
            app::run_tui().await?;
        }
        Some(Command::Run { stage, path, id, cmd }) => {
            let envs = if let Some(id) = id {
                keyring::load_envs_by_id(&id).await?
            } else {
                let path = match path {
                    Some(p) => std::fs::canonicalize(p)?,
                    None => std::env::current_dir()?,
                };
                keyring::load_envs(&path.to_string_lossy(), &stage).await?
            };

            if envs.is_empty() {
                eprintln!("No env vars found");
                std::process::exit(1);
            }

            let status = tokio::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .envs(envs)
                .status()
                .await?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

async fn run_with_id(id: &str, cmd: &[String]) -> anyhow::Result<()> {
    let envs = keyring::load_envs_by_id(id).await?;

    if envs.is_empty() {
        eprintln!("No env vars found for ID {id}");
        std::process::exit(1);
    }

    let status = tokio::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .envs(envs)
        .status()
        .await?;
    std::process::exit(status.code().unwrap_or(1));
}
