use crate::app::{App, DiffKind, ImportStatus, MsgKind, Tab, View};
use crate::keyring;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Tabs,
};

const ACCENT: Color = Color::Magenta;
const DIM: Color = Color::DarkGray;

// --- Top-level draw ---

pub fn draw(frame: &mut Frame, app: &mut App) {
    match &app.view {
        View::Tabs => draw_tabs(frame, app),
        View::Editor => draw_editor(frame, app),
        View::Delete => {
            draw_tabs(frame, app);
            draw_delete_popup(frame, app);
        }
        View::NewEntry => {
            draw_tabs(frame, app);
            draw_new_entry_popup(frame, app);
        }
        View::Copy => {
            draw_tabs(frame, app);
            draw_copy_popup(frame, app);
        }
    }
}

// --- Tab layout ---

fn draw_tabs(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Tab bar
            Constraint::Min(1),   // Content
            Constraint::Length(1), // Message
            Constraint::Length(1), // Keys
        ])
        .split(area);

    draw_tab_bar(frame, app, chunks[0]);

    match app.active_tab {
        Tab::Import => draw_import_tab(frame, app, chunks[1]),
        Tab::Store => draw_store_tab(frame, app, chunks[1]),
    }

    draw_message_bar(frame, app, chunks[2]);
    draw_keys_bar(frame, app, chunks[3]);
}

fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    // Badge for actionable imports
    let actionable = app
        .import_candidates
        .iter()
        .filter(|c| c.status != ImportStatus::Unchanged)
        .count();
    let import_title = if actionable > 0 {
        format!("Import ({actionable})")
    } else {
        "Import".to_string()
    };

    let titles = vec![import_title, "Store".to_string()];
    let selected = match app.active_tab {
        Tab::Import => 0,
        Tab::Store => 1,
    };

    // Layout: tabs left, cwd right
    let tab_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(app.cwd.len() as u16 + 2)])
        .split(area);

    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(DIM))
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .divider("│");

    frame.render_widget(tabs, tab_chunks[0]);

    let cwd_display = &app.cwd;
    let cwd_para = Paragraph::new(Span::styled(cwd_display, Style::default().fg(DIM)))
        .alignment(Alignment::Right);
    frame.render_widget(cwd_para, tab_chunks[1]);
}

fn draw_message_bar(frame: &mut Frame, app: &App, area: Rect) {
    let para = if let Some((ref kind, ref text)) = app.message {
        let (icon, color) = match kind {
            MsgKind::Success => ("✓ ", Color::Green),
            MsgKind::Warning => ("⚠ ", Color::Yellow),
            MsgKind::Error => ("✗ ", Color::Red),
        };
        Paragraph::new(Line::from(vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::styled(text.as_str(), Style::default().fg(color)),
        ]))
    } else {
        Paragraph::new("")
    };
    frame.render_widget(para, area);
}

fn draw_keys_bar(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match &app.view {
        View::Editor => "Paste: Ctrl+Shift+V │ Esc: save │ Ctrl+Q: discard",
        View::Delete => "←→ select │ Y/N │ Enter confirm │ Esc cancel",
        View::NewEntry | View::Copy => "Tab: switch field │ Enter: confirm │ Esc: cancel",
        View::Tabs => match app.active_tab {
            Tab::Import => "↑↓ nav │ Space toggle │ Enter import │ Tab Store │ Esc quit",
            Tab::Store => "[E]dit │ [D]elete │ [C]opy │ [S]how │ [N]ew │ [I]mport │ Tab Import │ Esc quit",
        },
    };
    let bar = Paragraph::new(keys).style(Style::default().fg(DIM));
    frame.render_widget(bar, area);
}

// --- Import tab ---

fn draw_import_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_import_file_list(frame, app, chunks[0]);
    draw_import_diff(frame, app, chunks[1]);
}

fn draw_import_file_list(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.import_candidates.is_empty() {
        let block = Block::default()
            .title(" .env Dateien ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT));
        let empty = Paragraph::new("No .env* files found.").block(block);
        frame.render_widget(empty, area);
        return;
    }

    let cwd = &app.cwd;
    let items: Vec<ListItem> = app
        .import_candidates
        .iter()
        .map(|c| {
            let checkbox = if c.selected { "[x]" } else { "[ ]" };
            let name = c.file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let existing_id = app.entries.iter()
                .find(|e| e.path == *cwd && e.stage == c.stage)
                .map(|e| e.id.clone());

            let (status_label, status_color) = match c.status {
                ImportStatus::New => ("neu".to_string(), Color::Green),
                ImportStatus::Changed => {
                    let label = match &existing_id {
                        Some(id) => format!("upd {id}"),
                        None => "upd".to_string(),
                    };
                    (label, Color::Yellow)
                }
                ImportStatus::Unchanged => {
                    let label = match &existing_id {
                        Some(id) => format!("= {id}"),
                        None => "=".to_string(),
                    };
                    (label, DIM)
                }
            };

            let warn = if c.perm_warn { " ⚠" } else { "" };

            let style = if c.selected {
                Style::default()
            } else {
                Style::default().fg(DIM)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{checkbox} {name}"), style),
                Span::styled(format!(" ({status_label})"), Style::default().fg(status_color)),
                Span::styled(warn, Style::default().fg(Color::Yellow)),
            ]))
        })
        .collect();

    let count = app.import_candidates.len();
    let title = format!(" .env Dateien ({count}) ");

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        )
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut app.import_list_state);

    // Scrollbar
    if count > area.height.saturating_sub(2) as usize {
        let mut scrollbar_state = ScrollbarState::new(count)
            .position(app.import_list_state.selected().unwrap_or(0));
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn draw_import_diff(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Diff ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));

    let Some(candidate) = app.selected_import() else {
        frame.render_widget(Paragraph::new("").block(block), area);
        return;
    };

    let diff = app.current_diff();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header: file name, stage, and existing entry info
    let file_name = candidate.file.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Check if entry exists in keyring
    let existing = app.entries.iter().find(|e| e.path == app.cwd && e.stage == candidate.stage);

    let header_height: u16 = if existing.is_some() { 4 } else { 2 };
    let header_area = Rect::new(inner.x, inner.y, inner.width, header_height);
    let diff_area = Rect::new(inner.x, inner.y + header_height, inner.width, inner.height.saturating_sub(header_height));

    let mut header_lines = vec![
        Line::from(vec![
            Span::styled(&file_name, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" → [{}]", candidate.stage), Style::default().fg(DIM)),
        ]),
    ];

    if let Some(entry) = existing {
        header_lines.push(Line::from(vec![
            Span::styled("Loaded: ", Style::default().fg(DIM)),
            Span::styled(&entry.id, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {} keys  updated {}", entry.vars.len(), keyring::relative_time(entry.updated_at)), Style::default().fg(DIM)),
        ]));
    }

    header_lines.push(Line::from(""));

    let header = Paragraph::new(header_lines);
    frame.render_widget(header, header_area);

    if diff.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("(leer)", Style::default().fg(DIM))),
            diff_area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for d in &diff {
        match d.kind {
            DiffKind::Added => {
                lines.push(Line::from(Span::styled(
                    format!("+ {}={}", d.key, d.new_val.as_deref().unwrap_or("")),
                    Style::default().fg(Color::Green),
                )));
            }
            DiffKind::Removed => {
                lines.push(Line::from(Span::styled(
                    format!("- {}={}", d.key, d.old_val.as_deref().unwrap_or("")),
                    Style::default().fg(Color::Red),
                )));
            }
            DiffKind::Changed => {
                lines.push(Line::from(Span::styled(
                    format!("- {}={}", d.key, d.old_val.as_deref().unwrap_or("")),
                    Style::default().fg(Color::Red),
                )));
                lines.push(Line::from(Span::styled(
                    format!("+ {}={}", d.key, d.new_val.as_deref().unwrap_or("")),
                    Style::default().fg(Color::Green),
                )));
            }
            DiffKind::Unchanged => {
                lines.push(Line::from(Span::styled(
                    format!("  {}={}", d.key, d.new_val.as_deref().unwrap_or("")),
                    Style::default().fg(DIM),
                )));
            }
        }
    }

    let total_lines = lines.len();
    frame.render_widget(Paragraph::new(lines), diff_area);

    // Scrollbar for diff
    if total_lines > diff_area.height as usize {
        let mut scrollbar_state = ScrollbarState::new(total_lines).position(0);
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, diff_area, &mut scrollbar_state);
    }
}

// --- Store tab ---

fn draw_store_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_project_list(frame, app, chunks[0]);
    draw_details(frame, app, chunks[1]);
}

fn short_path(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

fn ticker(text: &str, width: usize, tick: usize) -> String {
    let text_len = text.chars().count();
    if text_len <= width || width == 0 {
        return text.to_string();
    }

    let max_offset = text_len - width;
    let pause = 20;
    let scroll_steps = max_offset * 3;
    let cycle = pause + scroll_steps + pause + scroll_steps;
    let pos = tick % cycle;

    let offset = if pos < pause {
        0
    } else if pos < pause + scroll_steps {
        (pos - pause) / 3
    } else if pos < pause + scroll_steps + pause {
        max_offset
    } else {
        max_offset - (pos - pause - scroll_steps - pause) / 3
    };

    text.chars().skip(offset).take(width).collect()
}

fn draw_project_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let cwd = &app.cwd;

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let is_cwd = entry.path == *cwd;
            let mut style = Style::default();
            if is_cwd {
                style = style.fg(Color::Cyan);
            }
            let text = format!("{} {} [{}]", entry.id, short_path(&entry.path), entry.stage);
            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Projects ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        )
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut app.store_list_state);

    // Scrollbar
    let count = app.entries.len();
    if count > area.height.saturating_sub(2) as usize {
        let mut scrollbar_state = ScrollbarState::new(count)
            .position(app.store_list_state.selected().unwrap_or(0));
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));

    let Some(entry) = app.selected_entry() else {
        let empty = Paragraph::new("No entries. Press [N] to create one.").block(block);
        frame.render_widget(empty, area);
        return;
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let detail_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(1)])
        .split(inner);

    let path_width = detail_chunks[0].width.saturating_sub(7) as usize;
    let path_display = ticker(&entry.path, path_width, app.tick);

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("ID:    ", Style::default().fg(DIM)),
            Span::styled(&entry.id, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("Path:  ", Style::default().fg(DIM)),
            Span::raw(path_display),
        ]),
        Line::from(vec![
            Span::styled("Stage: ", Style::default().fg(DIM)),
            Span::raw(&entry.stage),
        ]),
        Line::from(vec![
            Span::styled("Keys:  ", Style::default().fg(DIM)),
            Span::raw(entry.vars.len().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Created: ", Style::default().fg(DIM)),
            Span::raw(keyring::relative_time(entry.created_at)),
        ]),
        Line::from(vec![
            Span::styled("Updated: ", Style::default().fg(DIM)),
            Span::raw(keyring::relative_time(entry.updated_at)),
        ]),
    ]);
    frame.render_widget(header, detail_chunks[0]);

    let rows: Vec<Row> = entry
        .vars
        .iter()
        .map(|(k, v)| {
            let val = if app.show_values {
                v.clone()
            } else {
                "••••••••".to_string()
            };
            Row::new(vec![k.clone(), val])
        })
        .collect();

    let table = Table::new(rows, [Constraint::Percentage(40), Constraint::Percentage(60)])
        .header(
            Row::new(vec!["Key", "Value"])
                .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        );

    frame.render_widget(table, detail_chunks[1]);
}

// --- Editor ---

fn draw_editor(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1), // Message
            Constraint::Length(1), // Keys
        ])
        .split(area);

    let title = if let Some(entry) = app.selected_entry() {
        format!(" Editor: {} [{}] ", entry.path, entry.stage)
    } else {
        " Editor ".to_string()
    };
    let header = Paragraph::new(
        "  Format: KEY=VALUE (one per line, paste with Ctrl+Shift+V)",
    )
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );
    frame.render_widget(header, chunks[0]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));
    app.editor.set_block(block);
    app.editor.set_cursor_line_style(Style::default().bg(Color::DarkGray));
    frame.render_widget(&app.editor, chunks[1]);

    draw_message_bar(frame, app, chunks[2]);
    draw_keys_bar(frame, app, chunks[3]);
}

// --- Popups (Store sub-views) ---

fn draw_delete_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect_abs(45, 11, frame.area());
    frame.render_widget(Clear, area);

    let Some(entry) = app.selected_entry() else { return; };

    let yes_style = if app.delete_yes {
        Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let no_style = if !app.delete_yes {
        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Delete entry?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Path:  ", Style::default().fg(DIM)),
            Span::raw(&entry.path),
        ]),
        Line::from(vec![
            Span::styled("  Stage: ", Style::default().fg(DIM)),
            Span::raw(&entry.stage),
        ]),
        Line::from(vec![
            Span::styled("  Keys:  ", Style::default().fg(DIM)),
            Span::raw(entry.vars.len().to_string()),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("      "),
            Span::styled(" Yes ", yes_style),
            Span::raw("    "),
            Span::styled(" No ", no_style),
        ]),
        Line::from(""),
    ];

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" Delete ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
    );
    frame.render_widget(popup, area);
}

fn draw_new_entry_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 30, frame.area());
    frame.render_widget(Clear, area);

    let path_style = if app.new_field == 0 { Style::default().fg(ACCENT) } else { Style::default() };
    let stage_style = if app.new_field == 1 { Style::default().fg(ACCENT) } else { Style::default() };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Path:  ", Style::default().fg(DIM)),
            Span::styled(&app.new_path, path_style),
        ]),
        Line::from(vec![
            Span::styled("Stage: ", Style::default().fg(DIM)),
            Span::styled(&app.new_stage, stage_style),
        ]),
        Line::from(""),
    ];

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" New Entry ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );
    frame.render_widget(popup, area);
}

fn draw_copy_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 35, frame.area());
    frame.render_widget(Clear, area);

    let Some(entry) = app.selected_entry() else { return; };

    let path_style = if app.copy_field == 0 { Style::default().fg(ACCENT) } else { Style::default() };
    let stage_style = if app.copy_field == 1 { Style::default().fg(ACCENT) } else { Style::default() };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("From:  ", Style::default().fg(DIM)),
            Span::raw(format!("{} [{}]", entry.path, entry.stage)),
        ]),
        Line::from(vec![
            Span::styled("Vars:  ", Style::default().fg(DIM)),
            Span::raw(format!("{} keys", entry.vars.len())),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path:  ", Style::default().fg(DIM)),
            Span::styled(&app.copy_path, path_style),
        ]),
        Line::from(vec![
            Span::styled("Stage: ", Style::default().fg(DIM)),
            Span::styled(&app.copy_stage, stage_style),
        ]),
        Line::from(""),
    ];

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" Copy ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );
    frame.render_widget(popup, area);
}

// --- Layout helpers ---

fn centered_rect_abs(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
