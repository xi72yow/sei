use crate::keyring::{self, EnvEntry};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui_textarea::{Input, TextArea};
use std::time::Duration;

use crate::ui;

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Dashboard,
    Editor,
    Confirm(ConfirmAction),
    Delete,
    NewEntry,
    Copy,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    Save,
    Copy,
    Import,
}

pub struct App<'a> {
    pub entries: Vec<EnvEntry>,
    pub selected: usize,
    pub view: View,
    pub show_values: bool,
    pub should_quit: bool,
    pub status_msg: Option<String>,
    // Confirm overlay
    pub confirm_yes: bool,
    // Textarea editor
    pub editor: TextArea<'a>,
    // New entry state
    pub new_path: String,
    pub new_stage: String,
    pub new_field: usize, // 0 = path, 1 = stage
    // Delete confirmation
    pub delete_input: String,
    // Copy state
    pub copy_path: String,
    pub copy_stage: String,
    pub copy_field: usize, // 0 = path, 1 = stage
    // Ticker fuer lange Pfade
    pub tick: usize,
}

impl<'a> App<'a> {
    pub fn new(entries: Vec<EnvEntry>, preselect_path: &str) -> Self {
        let selected = entries
            .iter()
            .position(|e| e.path == preselect_path)
            .unwrap_or(0);

        App {
            entries,
            selected,
            view: View::Dashboard,
            show_values: false,
            should_quit: false,
            status_msg: None,
            confirm_yes: false,
            editor: TextArea::default(),
            new_path: String::new(),
            new_stage: "default".to_string(),
            new_field: 0,
            delete_input: String::new(),
            copy_path: String::new(),
            copy_stage: "default".to_string(),
            copy_field: 0,
            tick: 0,
        }
    }

    pub fn selected_entry(&self) -> Option<&EnvEntry> {
        self.entries.get(self.selected)
    }

    pub fn enter_editor(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            let content = keyring::serialize_env_vars(&entry.vars);
            let lines: Vec<String> = if content.is_empty() {
                vec![String::new()]
            } else {
                content.lines().map(|l| l.to_string()).collect()
            };
            self.editor = TextArea::new(lines);
            self.view = View::Editor;
        }
    }

    pub fn enter_new_entry(&mut self) {
        self.new_path = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        self.new_stage = "default".to_string();
        self.new_field = 0;
        self.view = View::NewEntry;
    }

    pub fn enter_delete(&mut self) {
        self.delete_input.clear();
        self.view = View::Delete;
    }

    pub fn enter_copy(&mut self) {
        if self.entries.get(self.selected).is_some() {
            self.copy_path = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            self.copy_stage = "default".to_string();
            self.copy_field = 0;
            self.view = View::Copy;
        }
    }

    pub fn show_confirm(&mut self, action: ConfirmAction) {
        self.confirm_yes = false;
        self.view = View::Confirm(action);
    }

    /// Parse editor content back to vars
    pub fn editor_vars(&self) -> Vec<(String, String)> {
        let text = self.editor.lines().join("\n");
        keyring::parse_env_content(text.as_bytes())
    }
}

pub async fn run_tui() -> Result<()> {
    let entries = keyring::load_all_entries().await?;
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut app = App::new(entries, &cwd);
    let mut terminal = ratatui::init();
    let result = run_event_loop(&mut terminal, &mut app).await;
    ratatui::restore();

    // Keyring sperren beim Beenden
    let _ = keyring::lock_collection().await;

    result
}

async fn run_event_loop(terminal: &mut DefaultTerminal, app: &mut App<'_>) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit {
            return Ok(());
        }

        app.tick = app.tick.wrapping_add(1);

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    app.should_quit = true;
                    continue;
                }

                match app.view.clone() {
                    View::Dashboard => handle_dashboard_input(app, key).await?,
                    View::Editor => handle_editor_input(app, key),
                    View::Delete => handle_delete_input(app, key).await?,
                    View::NewEntry => handle_new_entry_input(app, key).await?,
                    View::Copy => handle_copy_input(app, key).await?,
                    View::Confirm(action) => handle_confirm_input(app, key, action).await?,
                }
            }
        }
    }
}

async fn handle_dashboard_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    app.status_msg = None;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.selected > 0 {
                app.selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.selected + 1 < app.entries.len() {
                app.selected += 1;
            }
        }
        KeyCode::Char('e') | KeyCode::Enter => app.enter_editor(),
        KeyCode::Char('d') => {
            if !app.entries.is_empty() {
                app.enter_delete();
            }
        }
        KeyCode::Char('s') => app.show_values = !app.show_values,
        KeyCode::Char('n') => app.enter_new_entry(),
        KeyCode::Char('c') => {
            if !app.entries.is_empty() {
                app.enter_copy();
            }
        }
        KeyCode::Char('i') => {
            let cwd = std::env::current_dir()?;
            let env_file = cwd.join(".env");
            if env_file.exists() {
                app.show_confirm(ConfirmAction::Import);
            } else {
                app.status_msg = Some("No .env file in current directory".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_editor_input(app: &mut App<'_>, key: event::KeyEvent) {
    // Ctrl+Q → verwerfen ohne speichern
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.status_msg = None;
        app.view = View::Dashboard;
        return;
    }

    // Esc → Inhalt pruefen, dann Save-Confirm
    if key.code == KeyCode::Esc {
        let lines = app.editor.lines();
        let mut bad_lines = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if !trimmed.contains('=') {
                bad_lines.push(i + 1);
            }
        }
        if !bad_lines.is_empty() {
            let nums: Vec<String> = bad_lines.iter().map(|n| n.to_string()).collect();
            app.status_msg = Some(format!(
                "Fehler: Zeile {} hat kein KEY=VALUE Format",
                nums.join(", ")
            ));
            return;
        }
        app.status_msg = None;
        app.show_confirm(ConfirmAction::Save);
        return;
    }

    // Alles andere geht an den Textarea (inkl. Paste via Ctrl+V / Terminal-Paste)
    app.editor.input(Input::from(key));
}

async fn handle_delete_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Dashboard,
        KeyCode::Enter => {
            if let Some(entry) = app.entries.get(app.selected) {
                let expected = format!("{} [{}]", entry.path, entry.stage);
                if app.delete_input == expected {
                    keyring::delete_entry(&entry.path, &entry.stage).await?;
                    app.entries = keyring::load_all_entries().await?;
                    if app.selected >= app.entries.len() && app.selected > 0 {
                        app.selected -= 1;
                    }
                    app.status_msg = Some("Deleted".to_string());
                    app.view = View::Dashboard;
                } else {
                    app.status_msg = Some("Input mismatch — not deleted".to_string());
                    app.view = View::Dashboard;
                }
            }
        }
        KeyCode::Char(c) => app.delete_input.push(c),
        KeyCode::Backspace => {
            app.delete_input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_new_entry_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Dashboard,
        KeyCode::Tab => app.new_field = if app.new_field == 0 { 1 } else { 0 },
        KeyCode::Enter => {
            if !app.new_path.is_empty() && !app.new_stage.is_empty() {
                keyring::save_envs(&app.new_path, &app.new_stage, &[]).await?;
                app.entries = keyring::load_all_entries().await?;
                app.selected = app
                    .entries
                    .iter()
                    .position(|e| e.path == app.new_path && e.stage == app.new_stage)
                    .unwrap_or(0);
                app.status_msg = Some("Created".to_string());
                app.view = View::Dashboard;
                app.enter_editor();
            }
        }
        KeyCode::Char(c) => {
            if app.new_field == 0 {
                app.new_path.push(c);
            } else {
                app.new_stage.push(c);
            }
        }
        KeyCode::Backspace => {
            if app.new_field == 0 {
                app.new_path.pop();
            } else {
                app.new_stage.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_copy_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Dashboard,
        KeyCode::Tab => app.copy_field = if app.copy_field == 0 { 1 } else { 0 },
        KeyCode::Enter => {
            if !app.copy_path.is_empty() && !app.copy_stage.is_empty() {
                app.show_confirm(ConfirmAction::Copy);
            }
        }
        KeyCode::Char(c) => {
            if app.copy_field == 0 {
                app.copy_path.push(c);
            } else {
                app.copy_stage.push(c);
            }
        }
        KeyCode::Backspace => {
            if app.copy_field == 0 {
                app.copy_path.pop();
            } else {
                app.copy_stage.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_confirm_input(
    app: &mut App<'_>,
    key: event::KeyEvent,
    action: ConfirmAction,
) -> Result<()> {
    match key.code {
        KeyCode::Left | KeyCode::Right => app.confirm_yes = !app.confirm_yes,
        KeyCode::Esc => {
            match action {
                ConfirmAction::Save => app.view = View::Editor,
                ConfirmAction::Copy => app.view = View::Copy,
                ConfirmAction::Import => app.view = View::Dashboard,
            }
        }
        KeyCode::Enter => {
            if app.confirm_yes {
                match action {
                    ConfirmAction::Save => {
                        if let Some(entry) = app.entries.get(app.selected) {
                            let vars = app.editor_vars();
                            keyring::save_envs(&entry.path, &entry.stage, &vars).await?;
                            if let Some(e) = app.entries.get_mut(app.selected) {
                                e.vars = vars;
                            }
                            app.status_msg = Some("Saved".to_string());
                        }
                        app.view = View::Dashboard;
                    }
                    ConfirmAction::Copy => {
                        if let Some(entry) = app.entries.get(app.selected) {
                            let vars = entry.vars.clone();
                            keyring::save_envs(&app.copy_path, &app.copy_stage, &vars).await?;
                            app.entries = keyring::load_all_entries().await?;
                            app.selected = app
                                .entries
                                .iter()
                                .position(|e| {
                                    e.path == app.copy_path && e.stage == app.copy_stage
                                })
                                .unwrap_or(0);
                            app.status_msg = Some("Copied".to_string());
                        }
                        app.view = View::Dashboard;
                    }
                    ConfirmAction::Import => {
                        let cwd = std::env::current_dir()?;
                        let env_file = cwd.join(".env");
                        keyring::import_env_file(&env_file, &cwd.to_string_lossy(), "default")
                            .await?;
                        app.entries = keyring::load_all_entries().await?;
                        app.status_msg = Some("Imported .env".to_string());
                        app.view = View::Dashboard;
                    }
                }
            } else {
                // No — verwerfen und zurueck zum Dashboard
                app.view = View::Dashboard;
            }
        }
        _ => {}
    }
    Ok(())
}
