use crate::app::{App, ConfirmAction, View};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table};

const ACCENT: Color = Color::Magenta;
const DIM: Color = Color::DarkGray;

pub fn draw(frame: &mut Frame, app: &mut App) {
    match &app.view {
        View::Dashboard => draw_dashboard(frame, app),
        View::Editor => draw_editor(frame, app),
        View::Delete => {
            draw_dashboard(frame, app);
            draw_delete_popup(frame, app);
        }
        View::NewEntry => {
            draw_dashboard(frame, app);
            draw_new_entry_popup(frame, app);
        }
        View::Copy => {
            draw_dashboard(frame, app);
            draw_copy_popup(frame, app);
        }
        View::Confirm(action) => {
            match action {
                ConfirmAction::Save => draw_editor(frame, app),
                ConfirmAction::Copy => {
                    draw_dashboard(frame, app);
                    draw_copy_popup(frame, app);
                }
                ConfirmAction::Import => draw_dashboard(frame, app),
            }
            draw_confirm_popup(frame, app, &app.view.clone());
        }
    }
}

fn draw_dashboard(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[0]);

    draw_project_list(frame, app, main[0]);
    draw_details(frame, app, main[1]);
    draw_status_bar(frame, app, chunks[1]);
}

/// Kuerzt einen Pfad auf den letzten Ordnernamen
fn short_path(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// Marquee/Ticker: scrollt Text hin und her wenn er nicht reinpasst
/// Pausiert am Anfang und Ende fuer einige Sekunden
fn ticker(text: &str, width: usize, tick: usize) -> String {
    let text_len = text.chars().count();
    if text_len <= width || width == 0 {
        return text.to_string();
    }

    let max_offset = text_len - width;
    let pause = 20; // ~2 Sekunden Pause am Anfang/Ende (20 * 100ms)
    let scroll_steps = max_offset * 3; // alle 3 Ticks ein Zeichen
    let cycle = pause + scroll_steps + pause + scroll_steps;
    let pos = tick % cycle;

    let offset = if pos < pause {
        // Pause am Anfang
        0
    } else if pos < pause + scroll_steps {
        // Scroll vorwaerts
        (pos - pause) / 3
    } else if pos < pause + scroll_steps + pause {
        // Pause am Ende
        max_offset
    } else {
        // Scroll rueckwaerts
        max_offset - (pos - pause - scroll_steps - pause) / 3
    };

    text.chars().skip(offset).take(width).collect()
}

fn draw_project_list(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let style = if i == app.selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let marker = if i == app.selected { "▸ " } else { "  " };
            let text = format!("{}{} [{}]", marker, short_path(&entry.path), entry.stage);
            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Projects ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );

    frame.render_widget(list, area);
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
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);

    // Verfuegbare Breite fuer den Pfad (abzgl. "Path:  " = 7 Zeichen)
    let path_width = detail_chunks[0].width.saturating_sub(7) as usize;
    let path_display = ticker(&entry.path, path_width, app.tick);

    let header = Paragraph::new(vec![
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

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let msg = if let Some(ref status) = app.status_msg {
        status.clone()
    } else {
        "[E]dit  [D]elete  [C]opy  [S]how/hide  [N]ew  [I]mport .env  [Q]uit".to_string()
    };

    let bar = Paragraph::new(msg).style(Style::default().fg(DIM));
    frame.render_widget(bar, area);
}

fn draw_editor(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Header
    let title = if let Some(entry) = app.selected_entry() {
        format!(" Editor: {} [{}] ", entry.path, entry.stage)
    } else {
        " Editor ".to_string()
    };
    let header = Paragraph::new("  Format: KEY=VALUE (eine Zeile pro Variable, Paste mit Ctrl+Shift+V)")
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        );
    frame.render_widget(header, chunks[0]);

    // Textarea
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT));
    app.editor.set_block(block);
    app.editor.set_cursor_line_style(Style::default().bg(Color::DarkGray));
    frame.render_widget(&app.editor, chunks[1]);

    // Help bar / Fehlermeldung
    let (msg, style) = if let Some(ref status) = app.status_msg {
        (status.clone(), Style::default().fg(Color::Red))
    } else {
        ("Paste: Ctrl+Shift+V  |  Esc: speichern  |  Ctrl+Q: verwerfen".to_string(), Style::default().fg(DIM))
    };
    let bar = Paragraph::new(msg).style(style);
    frame.render_widget(bar, chunks[2]);
}

fn draw_delete_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, area);

    let Some(entry) = app.selected_entry() else {
        return;
    };

    let key_list = entry.vars.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ");

    let expected = format!("{} [{}]", entry.path, entry.stage);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Path:  ", Style::default().fg(DIM)),
            Span::raw(&entry.path),
        ]),
        Line::from(vec![
            Span::styled("Stage: ", Style::default().fg(DIM)),
            Span::raw(&entry.stage),
        ]),
        Line::from(vec![
            Span::styled("Keys:  ", Style::default().fg(DIM)),
            Span::raw(&key_list),
        ]),
    ];
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Type to confirm: {expected}")));
    lines.push(Line::from(Span::styled(
        format!("> {}", app.delete_input),
        Style::default().fg(Color::Red),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc: cancel",
        Style::default().fg(DIM),
    )));

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

    let path_style = if app.new_field == 0 {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };
    let stage_style = if app.new_field == 1 {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

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
        Line::from(Span::styled(
            "Tab: switch field  Enter: confirm  Esc: cancel",
            Style::default().fg(DIM),
        )),
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

    let Some(entry) = app.selected_entry() else {
        return;
    };

    let path_style = if app.copy_field == 0 {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };
    let stage_style = if app.copy_field == 1 {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

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
        Line::from(Span::styled(
            "Tab: switch field  Enter: confirm  Esc: cancel",
            Style::default().fg(DIM),
        )),
    ];

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" Copy to new project ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );

    frame.render_widget(popup, area);
}

fn draw_confirm_popup(frame: &mut Frame, app: &App, view: &View) {
    let area = centered_rect_abs(42, 8, frame.area());
    frame.render_widget(Clear, area);

    let action = match view {
        View::Confirm(a) => a,
        _ => return,
    };

    let message = match action {
        ConfirmAction::Save => "Save changes?",
        ConfirmAction::Copy => "Copy entry?",
        ConfirmAction::Import => "Import .env file?",
    };

    let yes_style = if app.confirm_yes {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let no_style = if !app.confirm_yes {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let lines = vec![
        Line::from(""),
        Line::from(message),
        Line::from(""),
        Line::from(vec![
            Span::raw("      "),
            Span::styled(" Yes ", yes_style),
            Span::raw("    "),
            Span::styled(" No ", no_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "←→: select  Enter: confirm  Esc: cancel",
            Style::default().fg(DIM),
        )),
    ];

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" Confirm ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
    );

    frame.render_widget(popup, area);
}

/// Zentriertes Rect mit absoluter Breite/Hoehe (fuer Overlays die immer gleich gross sein sollen)
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
