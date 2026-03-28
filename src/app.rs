use crate::keyring::{self, EnvEntry, Keyring};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;
use ratatui_textarea::{Input, TextArea};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use crate::ui;

// --- Messages ---

#[derive(Debug, Clone, PartialEq)]
pub enum MsgKind {
    Success,
    Warning,
    Error,
}

// --- Tabs ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Import,
    Store,
}

// --- Import types ---

#[derive(Debug, Clone, PartialEq)]
pub enum ImportStatus {
    New,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportPhase {
    Select,
}

#[derive(Debug, Clone)]
pub struct ImportCandidate {
    pub file: PathBuf,
    pub stage: String,
    pub selected: bool,
    pub perm_warn: bool,
    pub status: ImportStatus,
    pub file_vars: Vec<(String, String)>,
}

// --- Diff ---

#[derive(Debug, Clone)]
pub enum DiffKind {
    Added,
    Removed,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub key: String,
    pub old_val: Option<String>,
    pub new_val: Option<String>,
}

// --- Views (sub-views within Store tab) ---

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Tabs,
    Editor,
    Delete,
    NewEntry,
    Copy,
}

// --- App ---

pub struct App<'a> {
    pub keyring: Keyring,
    pub entries: Vec<EnvEntry>,
    pub active_tab: Tab,
    pub view: View,
    pub show_values: bool,
    pub should_quit: bool,
    pub message: Option<(MsgKind, String)>,
    // Store list
    pub store_list_state: ListState,
    // Textarea editor
    pub editor: TextArea<'a>,
    // New entry state
    pub new_path: String,
    pub new_stage: String,
    pub new_field: usize,
    // Delete confirmation
    pub delete_confirm: bool,
    // Copy state
    pub copy_path: String,
    pub copy_stage: String,
    pub copy_field: usize,
    // Import state
    pub import_candidates: Vec<ImportCandidate>,
    pub import_list_state: ListState,
    pub import_phase: ImportPhase,
    // Ticker
    pub tick: usize,
    // cwd for highlighting
    pub cwd: String,
}

impl<'a> App<'a> {
    pub fn new(keyring: Keyring, entries: Vec<EnvEntry>, cwd: &str) -> Self {
        let mut store_list_state = ListState::default();
        let preselect = entries.iter().position(|e| e.path == cwd).unwrap_or(0);
        if !entries.is_empty() {
            store_list_state.select(Some(preselect));
        }

        App {
            keyring,
            entries,
            active_tab: Tab::Store,
            view: View::Tabs,
            show_values: false,
            should_quit: false,
            message: None,
            store_list_state,
            editor: TextArea::default(),
            new_path: String::new(),
            new_stage: "default".to_string(),
            new_field: 0,
            delete_confirm: false,
            copy_path: String::new(),
            copy_stage: "default".to_string(),
            copy_field: 0,
            import_candidates: Vec::new(),
            import_list_state: ListState::default(),
            import_phase: ImportPhase::Select,
            tick: 0,
            cwd: cwd.to_string(),
        }
    }

    pub fn msg(&mut self, kind: MsgKind, text: impl Into<String>) {
        self.message = Some((kind, text.into()));
    }

    pub fn selected_entry(&self) -> Option<&EnvEntry> {
        self.store_list_state.selected().and_then(|i| self.entries.get(i))
    }

    pub fn selected_index(&self) -> usize {
        self.store_list_state.selected().unwrap_or(0)
    }

    pub fn enter_editor(&mut self) {
        if let Some(entry) = self.selected_entry() {
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
        self.new_path = self.cwd.clone();
        self.new_stage = "default".to_string();
        self.new_field = 0;
        self.view = View::NewEntry;
    }

    pub fn enter_delete(&mut self) {
        self.delete_confirm = false;
        self.view = View::Delete;
    }

    pub fn enter_copy(&mut self) {
        if self.selected_entry().is_some() {
            self.copy_path = self.cwd.clone();
            self.copy_stage = "default".to_string();
            self.copy_field = 0;
            self.view = View::Copy;
        }
    }

    pub fn editor_vars(&self) -> Vec<(String, String)> {
        let text = self.editor.lines().join("\n");
        keyring::parse_env_content(text.as_bytes())
    }

    /// Scan directory for .env* files and populate import candidates
    pub fn scan_env_files(&mut self) {
        let mut candidates = Vec::new();

        if let Ok(dir_entries) = std::fs::read_dir(&self.cwd) {
            for entry in dir_entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.starts_with(".env") || !entry.path().is_file() {
                    continue;
                }

                let stage = if name_str == ".env" {
                    "default".to_string()
                } else {
                    name_str.strip_prefix(".env.").unwrap_or("default").to_string()
                };

                let perm_warn = entry
                    .metadata()
                    .map(|m| m.permissions().mode() & 0o077 != 0)
                    .unwrap_or(false);

                let file_vars = std::fs::read(entry.path())
                    .map(|content| keyring::parse_env_content(&content))
                    .unwrap_or_default();

                let status = match self.entries.iter().find(|e| e.path == self.cwd && e.stage == stage) {
                    None => ImportStatus::New,
                    Some(e) if e.vars != file_vars => ImportStatus::Changed,
                    Some(_) => ImportStatus::Unchanged,
                };

                candidates.push(ImportCandidate {
                    file: entry.path(),
                    stage,
                    selected: status != ImportStatus::Unchanged,
                    perm_warn,
                    status,
                    file_vars,
                });
            }
        }

        candidates.sort_by(|a, b| a.file.cmp(&b.file));
        self.import_candidates = candidates;
        self.import_list_state = ListState::default();
        if !self.import_candidates.is_empty() {
            self.import_list_state.select(Some(0));
        }
        self.import_phase = ImportPhase::Select;
    }

    pub fn selected_import(&self) -> Option<&ImportCandidate> {
        self.import_list_state.selected().and_then(|i| self.import_candidates.get(i))
    }

    /// Compute diff for the currently selected import candidate
    pub fn current_diff(&self) -> Vec<DiffLine> {
        let Some(candidate) = self.selected_import() else {
            return Vec::new();
        };

        let old_vars = self
            .entries
            .iter()
            .find(|e| e.path == self.cwd && e.stage == candidate.stage)
            .map(|e| &e.vars[..])
            .unwrap_or(&[]);

        compute_diff(old_vars, &candidate.file_vars)
    }

    /// Update permission message when import cursor moves
    pub fn update_import_perm_msg(&mut self) {
        if let Some(c) = self.selected_import() {
            if c.perm_warn {
                let name = c.file.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mode = c.file.metadata()
                    .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
                    .unwrap_or_default();
                self.msg(MsgKind::Warning, format!("{name} ist fuer andere lesbar (mode: {mode}). chmod 600 empfohlen"));
            } else {
                self.message = None;
            }
        }
    }
}

pub fn compute_diff(
    old_vars: &[(String, String)],
    new_vars: &[(String, String)],
) -> Vec<DiffLine> {
    let old_map: HashMap<&str, &str> = old_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let new_map: HashMap<&str, &str> = new_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let mut lines = Vec::new();

    for (key, new_val) in new_vars {
        match old_map.get(key.as_str()) {
            None => lines.push(DiffLine {
                kind: DiffKind::Added, key: key.clone(),
                old_val: None, new_val: Some(new_val.clone()),
            }),
            Some(old_val) if *old_val != new_val.as_str() => lines.push(DiffLine {
                kind: DiffKind::Changed, key: key.clone(),
                old_val: Some(old_val.to_string()), new_val: Some(new_val.clone()),
            }),
            Some(old_val) => lines.push(DiffLine {
                kind: DiffKind::Unchanged, key: key.clone(),
                old_val: Some(old_val.to_string()), new_val: Some(new_val.clone()),
            }),
        }
    }

    for (key, old_val) in old_vars {
        if !new_map.contains_key(key.as_str()) {
            lines.push(DiffLine {
                kind: DiffKind::Removed, key: key.clone(),
                old_val: Some(old_val.clone()), new_val: None,
            });
        }
    }

    lines
}

// --- TUI entry point ---

pub async fn run_tui() -> Result<()> {
    let kr = Keyring::connect().await?;
    let entries = kr.load_all_entries().await?;
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut app = App::new(kr, entries, &cwd);
    app.scan_env_files();

    // Start on Import tab if there are actionable candidates
    let has_actionable = app.import_candidates.iter().any(|c| c.status != ImportStatus::Unchanged);
    if has_actionable {
        app.active_tab = Tab::Import;
        app.update_import_perm_msg();
    }

    let mut terminal = ratatui::init();
    let result = run_event_loop(&mut terminal, &mut app).await;
    ratatui::restore();

    let _ = app.keyring.lock().await;
    result
}

// --- Event loop ---

async fn run_event_loop(terminal: &mut DefaultTerminal, app: &mut App<'_>) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit {
            return Ok(());
        }

        app.tick = app.tick.wrapping_add(1);

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                    continue;
                }

                match app.view.clone() {
                    View::Tabs => handle_tabs_input(app, key).await?,
                    View::Editor => handle_editor_input(app, key).await?,
                    View::Delete => handle_delete_input(app, key).await?,
                    View::NewEntry => handle_new_entry_input(app, key).await?,
                    View::Copy => handle_copy_input(app, key).await?,
                }
            }
        }
    }
}

// --- Tab-level input ---

async fn handle_tabs_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    if key.code == KeyCode::Tab {
        app.active_tab = match app.active_tab {
            Tab::Import => Tab::Store,
            Tab::Store => Tab::Import,
        };
        app.message = None;
        if app.active_tab == Tab::Import {
            app.scan_env_files();
            app.update_import_perm_msg();
        }
        return Ok(());
    }

    match app.active_tab {
        Tab::Import => handle_import_input(app, key).await,
        Tab::Store => handle_store_input(app, key).await,
    }
}

// --- Import tab ---

async fn handle_import_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    handle_import_select(app, key).await
}

async fn handle_import_select(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(i) = app.import_list_state.selected() {
                if i > 0 {
                    app.import_list_state.select(Some(i - 1));
                    app.update_import_perm_msg();
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(i) = app.import_list_state.selected() {
                if i + 1 < app.import_candidates.len() {
                    app.import_list_state.select(Some(i + 1));
                    app.update_import_perm_msg();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(i) = app.import_list_state.selected() {
                if let Some(c) = app.import_candidates.get_mut(i) {
                    c.selected = !c.selected;
                }
            }
        }
        KeyCode::Enter => {
            let mut count = 0;
            let mut ids = Vec::new();
            let mut first_stage = String::new();

            for candidate in &app.import_candidates {
                if !candidate.selected {
                    continue;
                }
                let id = app.keyring
                    .import_env_file(&candidate.file, &app.cwd, &candidate.stage)
                    .await?;
                if first_stage.is_empty() {
                    first_stage = candidate.stage.clone();
                }
                ids.push(id);
                count += 1;
            }

            if count == 0 {
                app.msg(MsgKind::Warning, "Nichts ausgewaehlt");
            } else {
                app.entries = app.keyring.load_all_entries().await?;

                // Select first imported entry in store list
                let pos = app.entries.iter().position(|e| e.path == app.cwd && e.stage == first_stage);
                if let Some(i) = pos {
                    app.store_list_state.select(Some(i));
                }

                let id_list = ids.join(", ");
                app.msg(MsgKind::Success, format!("{count} importiert [{}]", id_list));
                app.active_tab = Tab::Store;
            }
        }
        _ => {}
    }
    Ok(())
}

// --- Store tab ---

async fn handle_store_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(i) = app.store_list_state.selected() {
                if i > 0 {
                    app.store_list_state.select(Some(i - 1));
                    app.message = None;
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(i) = app.store_list_state.selected() {
                if i + 1 < app.entries.len() {
                    app.store_list_state.select(Some(i + 1));
                    app.message = None;
                }
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
            app.scan_env_files();
            app.active_tab = Tab::Import;
            app.update_import_perm_msg();
        }
        _ => {}
    }
    Ok(())
}

// --- Editor (Esc = direkt speichern, kein Confirm) ---

async fn handle_editor_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.message = None;
        app.view = View::Tabs;
        return Ok(());
    }

    if key.code == KeyCode::Esc {
        // Validate
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
            app.msg(MsgKind::Error, format!("Zeile {} hat kein KEY=VALUE Format", nums.join(", ")));
            return Ok(());
        }

        // Save directly
        let idx = app.selected_index();
        if let Some(entry) = app.entries.get(idx) {
            let path = entry.path.clone();
            let stage = entry.stage.clone();
            let vars = app.editor_vars();
            app.keyring.save_envs(&path, &stage, &vars).await?;
            if let Some(e) = app.entries.get_mut(idx) {
                e.vars = vars;
            }
            app.msg(MsgKind::Success, "Gespeichert");
        }
        app.view = View::Tabs;
        return Ok(());
    }

    app.editor.input(Input::from(key));
    Ok(())
}

// --- Delete (J/N statt Pfad tippen) ---

async fn handle_delete_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Tabs,
        KeyCode::Char('j') | KeyCode::Char('y') => {
            let idx = app.selected_index();
            if let Some(entry) = app.entries.get(idx) {
                let path = entry.path.clone();
                let stage = entry.stage.clone();
                app.keyring.delete_entry(&path, &stage).await?;
                app.entries = app.keyring.load_all_entries().await?;
                if idx >= app.entries.len() && idx > 0 {
                    app.store_list_state.select(Some(idx - 1));
                } else if app.entries.is_empty() {
                    app.store_list_state.select(None);
                }
                app.msg(MsgKind::Success, "Geloescht");
            }
            app.view = View::Tabs;
        }
        KeyCode::Char('n') => {
            app.view = View::Tabs;
        }
        _ => {}
    }
    Ok(())
}

// --- New Entry ---

async fn handle_new_entry_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Tabs,
        KeyCode::Tab => app.new_field = if app.new_field == 0 { 1 } else { 0 },
        KeyCode::Enter => {
            if !app.new_path.is_empty() && !app.new_stage.is_empty() {
                app.keyring.save_envs(&app.new_path, &app.new_stage, &[]).await?;
                app.entries = app.keyring.load_all_entries().await?;
                let pos = app.entries.iter().position(|e| e.path == app.new_path && e.stage == app.new_stage);
                if let Some(i) = pos {
                    app.store_list_state.select(Some(i));
                }
                app.msg(MsgKind::Success, "Erstellt");
                app.view = View::Tabs;
                app.enter_editor();
            }
        }
        KeyCode::Char(c) => {
            if app.new_field == 0 { app.new_path.push(c); } else { app.new_stage.push(c); }
        }
        KeyCode::Backspace => {
            if app.new_field == 0 { app.new_path.pop(); } else { app.new_stage.pop(); }
        }
        _ => {}
    }
    Ok(())
}

// --- Copy ---

async fn handle_copy_input(app: &mut App<'_>, key: event::KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.view = View::Tabs,
        KeyCode::Tab => app.copy_field = if app.copy_field == 0 { 1 } else { 0 },
        KeyCode::Enter => {
            if !app.copy_path.is_empty() && !app.copy_stage.is_empty() {
                if let Some(entry) = app.entries.get(app.selected_index()) {
                    let vars = entry.vars.clone();
                    app.keyring.save_envs(&app.copy_path, &app.copy_stage, &vars).await?;
                    app.entries = app.keyring.load_all_entries().await?;
                    let pos = app.entries.iter().position(|e| e.path == app.copy_path && e.stage == app.copy_stage);
                    if let Some(i) = pos {
                        app.store_list_state.select(Some(i));
                    }
                    app.msg(MsgKind::Success, "Kopiert");
                }
                app.view = View::Tabs;
            }
        }
        KeyCode::Char(c) => {
            if app.copy_field == 0 { app.copy_path.push(c); } else { app.copy_stage.push(c); }
        }
        KeyCode::Backspace => {
            if app.copy_field == 0 { app.copy_path.pop(); } else { app.copy_stage.pop(); }
        }
        _ => {}
    }
    Ok(())
}
