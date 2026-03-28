mod app;
mod keyring;
mod ui;

use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sei", about = "Save Env Inject — manage .env secrets in GNOME Keyring")]
struct Cli {
    /// Show entries with masked values (for AI agents / scripts)
    #[arg(long)]
    agent: bool,

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

    // Let --help, --agent, -h, help, run through to clap
    let is_flag_or_subcommand = args.first().is_some_and(|a| {
        a.starts_with('-') || a == "run" || a == "help"
    });

    if !is_flag_or_subcommand && !args.is_empty() {
        // ID shorthand: sei 001 cmd args...
        if args.len() >= 2 {
            if let Some(id) = args.first() {
                if id.len() == 3 && id.chars().all(|c| c.is_ascii_digit()) {
                    let cmd = &args[1..];
                    return run_with_envs(keyring::load_envs_by_id(id).await?, cmd).await;
                }
            }
        }

        // Inline picker: sei <cmd args...>
        return run_with_picker(&args).await;
    }

    let cli = Cli::parse();

    if cli.agent {
        return agent_info().await;
    }

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

            run_with_envs(envs, &cmd).await?;
        }
    }

    Ok(())
}

async fn run_with_envs(envs: Vec<(String, String)>, cmd: &[String]) -> anyhow::Result<()> {
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

/// --agent: show all entries with masked values, grouped by path
async fn agent_info() -> anyhow::Result<()> {
    let kr = keyring::Keyring::connect().await?;
    let entries = kr.load_all_entries().await?;
    kr.lock().await?;

    if entries.is_empty() {
        println!("sei: no entries in keyring");
        return Ok(());
    }

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Group by path
    let mut by_path: std::collections::BTreeMap<&str, Vec<&keyring::EnvEntry>> =
        std::collections::BTreeMap::new();
    for entry in &entries {
        by_path.entry(&entry.path).or_default().push(entry);
    }

    println!("sei-secrets");
    println!();

    for (path, path_entries) in &by_path {
        let marker = if *path == cwd { " (cwd)" } else { "" };
        println!("{}{}", path, marker);
        for entry in path_entries {
            let keys: Vec<&str> = entry.vars.iter().map(|(k, _)| k.as_str()).collect();
            let name_info = if entry.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", entry.name)
            };
            println!(
                "  {}{} [{}]  {} keys: {}",
                entry.id,
                name_info,
                entry.stage,
                entry.vars.len(),
                keys.join(", ")
            );
        }
        println!();
    }

    println!("Usage: sei <ID> <command>");
    println!("Example: sei {} podman compose up", entries[0].id);

    Ok(())
}

/// Inline picker: show entries, let user select, run command
async fn run_with_picker(cmd: &[String]) -> anyhow::Result<()> {
    let kr = keyring::Keyring::connect().await?;
    let entries = kr.load_all_entries().await?;

    if entries.is_empty() {
        eprintln!("No entries in keyring. Run `sei` to create some.");
        std::process::exit(1);
    }

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let selected = inline_pick(&entries, &cwd)?;

    match selected {
        Some(idx) => {
            let entry = &entries[idx];
            let envs: Vec<(String, String)> = entry.vars.clone();
            kr.lock().await?;
            run_with_envs(envs, cmd).await
        }
        None => {
            kr.lock().await?;
            eprintln!("Cancelled");
            std::process::exit(1);
        }
    }
}

/// Minimal inline terminal picker with Tab for Local/All
fn inline_pick(entries: &[keyring::EnvEntry], cwd: &str) -> anyhow::Result<Option<usize>> {
    let local_indices: Vec<usize> = entries.iter().enumerate()
        .filter(|(_, e)| e.path == cwd)
        .map(|(i, _)| i)
        .collect();
    let all_indices: Vec<usize> = (0..entries.len()).collect();

    let has_local = !local_indices.is_empty();
    let mut show_local = has_local;
    let mut stdout = std::io::stdout();
    let mut cursor: usize = 0;
    let max_visible = 8;
    // Reserve max space (entries + header + help)
    let reserve = max_visible + 2;

    terminal::enable_raw_mode()?;
    crossterm::execute!(stdout, crossterm::cursor::Hide)?;

    for _ in 0..reserve {
        write!(stdout, "\r\n")?;
    }
    write!(stdout, "\x1b[{}A", reserve)?;
    stdout.flush()?;

    let result = (|| -> anyhow::Result<Option<usize>> {
        loop {
            let indices = if show_local { &local_indices } else { &all_indices };
            let count = indices.len();
            if cursor >= count && count > 0 {
                cursor = count - 1;
            }
            let visible = max_visible.min(count);

            let offset = if cursor >= visible {
                cursor - visible + 1
            } else {
                0
            };

            // Header: Local/All tabs
            let local_label = if show_local { "\x1b[1;35mLocal\x1b[0m" } else { "\x1b[90mLocal\x1b[0m" };
            let all_label = if !show_local { "\x1b[1;35mAll\x1b[0m" } else { "\x1b[90mAll\x1b[0m" };
            write!(stdout, "\r {local_label} │ {all_label}\x1b[K\r\n")?;

            // Entries
            for i in 0..max_visible {
                let list_idx = offset + i;
                if list_idx < count {
                    let entry_idx = indices[list_idx];
                    let entry = &entries[entry_idx];
                    let name = entry.display_name();

                    if list_idx == cursor {
                        write!(stdout, "\r\x1b[1;35m▸ {} {} [{}]\x1b[0m\x1b[K\r\n", entry.id, name, entry.stage)?;
                    } else {
                        write!(stdout, "\r  {} {} [{}]\x1b[K\r\n", entry.id, name, entry.stage)?;
                    }
                } else {
                    write!(stdout, "\r\x1b[K\r\n")?;
                }
            }

            // Help
            let tab_hint = if has_local { "Tab local/all │ " } else { "" };
            write!(stdout, "\r\x1b[90m↑↓ select │ {tab_hint}Enter run │ Esc cancel\x1b[0m\x1b[K")?;
            stdout.flush()?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if cursor > 0 { cursor -= 1; }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let indices = if show_local { &local_indices } else { &all_indices };
                        if cursor + 1 < indices.len() { cursor += 1; }
                    }
                    KeyCode::Tab => {
                        if has_local {
                            show_local = !show_local;
                            cursor = 0;
                        }
                    }
                    KeyCode::Enter => {
                        let indices = if show_local { &local_indices } else { &all_indices };
                        if let Some(&entry_idx) = indices.get(cursor) {
                            // Clear
                            write!(stdout, "\r\x1b[{}A", max_visible + 1)?;
                            for _ in 0..reserve {
                                write!(stdout, "\r\x1b[K\r\n")?;
                            }
                            write!(stdout, "\x1b[{}A", reserve)?;
                            stdout.flush()?;
                            return Ok(Some(entry_idx));
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        write!(stdout, "\r\x1b[{}A", max_visible + 1)?;
                        for _ in 0..reserve {
                            write!(stdout, "\r\x1b[K\r\n")?;
                        }
                        write!(stdout, "\x1b[{}A", reserve)?;
                        stdout.flush()?;
                        return Ok(None);
                    }
                    _ => {}
                }
            }

            // Move back to top for redraw
            write!(stdout, "\r\x1b[{}A", max_visible + 1)?;
        }
    })();

    crossterm::execute!(stdout, crossterm::cursor::Show)?;
    terminal::disable_raw_mode()?;
    write!(stdout, "\r")?;
    stdout.flush()?;

    result
}
