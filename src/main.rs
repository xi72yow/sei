mod app;
mod keyring;
mod ui;

use clap::{Parser, Subcommand};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sei", about = "Save Env Inject — manage .env secrets in GNOME Keyring")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Import a .env file into the keyring
    Import {
        /// Path to .env file (default: .env in current directory)
        #[arg(short, long)]
        file: Option<PathBuf>,

        /// Stage name (default: "default")
        #[arg(short, long, default_value = "default")]
        stage: String,
    },

    /// Run a command with injected env vars (headless)
    Run {
        /// Stage name
        #[arg(short, long, default_value = "default")]
        stage: String,

        /// Project path (default: current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            // TUI Dashboard
            app::run_tui().await?;
        }
        Some(Command::Import { file, stage }) => {
            let env_file = file.unwrap_or_else(|| PathBuf::from(".env"));
            let path = std::env::current_dir()?;

            // Warn if .env file is group/world readable
            if let Ok(metadata) = env_file.metadata() {
                let mode = metadata.permissions().mode();
                if mode & 0o077 != 0 {
                    eprintln!(
                        "Warnung: {} ist fuer andere lesbar (mode: {:o}). Empfohlen: chmod 600",
                        env_file.display(),
                        mode & 0o777
                    );
                }
            }

            keyring::import_env_file(&env_file, &path.to_string_lossy(), &stage).await?;
            println!("Importiert in Keyring [{}]", stage);

            // Ask to delete the .env file
            eprint!(
                "{} loeschen? Die Secrets sind jetzt sicher im Keyring. [j/N] ",
                env_file.display()
            );
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if input.trim().eq_ignore_ascii_case("j") {
                std::fs::remove_file(&env_file)?;
                println!("{} geloescht.", env_file.display());
            } else {
                eprintln!(
                    "Hinweis: {} existiert noch — Secrets sind weiterhin als Datei lesbar.",
                    env_file.display()
                );
            }
        }
        Some(Command::Run { stage, path, cmd }) => {
            let path = match path {
                Some(p) => std::fs::canonicalize(p)?,
                None => std::env::current_dir()?,
            };
            let envs = keyring::load_envs(&path.to_string_lossy(), &stage).await?;

            if envs.is_empty() {
                eprintln!("No env vars found for {} [{}]", path.display(), stage);
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
