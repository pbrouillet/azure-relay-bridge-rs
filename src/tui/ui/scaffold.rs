use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::tui::app::{App, ConfigKind};
use crate::tui::forms::TextField;

pub fn render_choose_type(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    let title = Paragraph::new("Scaffold Config — Choose Type")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    let items = [
        ("Client (LocalForward)", "Listen locally, forward via relay"),
        ("Server (RemoteForward)", "Accept relay connections, forward to local service"),
    ];

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            let style = if i == app.scaffold_type_index {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.scaffold_type_index { "▸ " } else { "  " };
            ListItem::new(vec![
                Line::from(Span::styled(format!("{prefix}{name}"), style)),
                Line::from(Span::styled(
                    format!("    {desc}"),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let menu = List::new(list_items).block(
        Block::default().borders(Borders::ALL).title(" Select Config Type "),
    );
    frame.render_widget(menu, chunks[1]);

    render_footer(frame, chunks[2], &[("↑↓", "Navigate"), ("Enter", "Select"), ("Esc", "Back")]);
}

pub fn render_connection(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    let kind_label = match app.scaffold_kind {
        ConfigKind::Client => "Client",
        ConfigKind::Server => "Server",
    };
    let title = Paragraph::new(format!("Scaffold {kind_label} — Connection Settings"))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    let fields = app.connection_form.fields();
    render_form_fields(frame, chunks[1], &fields, app.connection_form.active_field);

    render_footer(frame, chunks[2], &[
        ("Tab", "Next field"),
        ("Shift+Tab", "Prev field"),
        ("Enter", "Continue"),
        ("Esc", "Back"),
    ]);
}

pub fn render_forwards(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    let kind_label = match app.scaffold_kind {
        ConfigKind::Client => "Client — Local Forwards",
        ConfigKind::Server => "Server — Remote Forwards",
    };
    let title = Paragraph::new(format!("Scaffold {kind_label}"))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    if app.editing_forward {
        // Render the active form
        match app.scaffold_kind {
            ConfigKind::Client => {
                if let Some(f) = app.local_forwards.get(app.forward_list_index) {
                    let fields = f.fields();
                    render_form_fields(frame, chunks[1], &fields, f.active_field);
                }
            }
            ConfigKind::Server => {
                if let Some(f) = app.remote_forwards.get(app.forward_list_index) {
                    let fields = f.fields();
                    render_form_fields(frame, chunks[1], &fields, f.active_field);
                }
            }
        }
        render_footer(frame, chunks[2], &[
            ("Tab", "Next"),
            ("Shift+Tab", "Prev"),
            ("Enter/Esc", "Done editing"),
        ]);
    } else {
        // Render the forward list
        let items: Vec<ListItem> = match app.scaffold_kind {
            ConfigKind::Client => app
                .local_forwards
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let style = if i == app.forward_list_index {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if i == app.forward_list_index { "▸ " } else { "  " };
                    ListItem::new(Span::styled(format!("{prefix}{}", f.summary()), style))
                })
                .collect(),
            ConfigKind::Server => app
                .remote_forwards
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let style = if i == app.forward_list_index {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if i == app.forward_list_index { "▸ " } else { "  " };
                    ListItem::new(Span::styled(format!("{prefix}{}", f.summary()), style))
                })
                .collect(),
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Forwarding Entries "),
        );
        frame.render_widget(list, chunks[1]);

        render_footer(frame, chunks[2], &[
            ("a", "Add"),
            ("e/Enter", "Edit"),
            ("d", "Delete"),
            ("p", "Preview YAML"),
            ("Esc", "Back"),
        ]);
    }
}

pub fn render_preview(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    let title = Paragraph::new("YAML Preview")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    let yaml_lines: Vec<Line> = app
        .preview_yaml
        .lines()
        .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(Color::Green))))
        .collect();

    let preview = Paragraph::new(yaml_lines)
        .scroll((app.preview_scroll, 0))
        .block(Block::default().borders(Borders::ALL).title(" Config YAML "));
    frame.render_widget(preview, chunks[1]);

    let footer_items = vec![
        ("↑↓", "Scroll"),
        ("s", "Save"),
        ("Esc", "Back"),
    ];
    if let Some(ref msg) = app.status_message {
        // Show status in footer area
        render_footer_with_status(frame, chunks[2], &footer_items, msg);
        return;
    }
    render_footer(frame, chunks[2], &footer_items);
}

/// Render a list of text fields as a form.
fn render_form_fields(frame: &mut Frame, area: ratatui::layout::Rect, fields: &[&TextField], active: usize) {
    let constraints: Vec<Constraint> = fields
        .iter()
        .map(|_| Constraint::Length(3))
        .collect();

    let field_chunks = Layout::vertical(constraints).split(area);

    for (i, field) in fields.iter().enumerate() {
        let is_active = i == active;
        let border_style = if is_active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let display_value = if field.value.is_empty() && !is_active {
            Span::styled("(empty)", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw(&field.value)
        };

        let input = Paragraph::new(Line::from(display_value)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(" {} ", field.label)),
        );
        frame.render_widget(input, field_chunks[i]);

        // Show cursor for active field
        if is_active {
            frame.set_cursor_position((
                field_chunks[i].x + field.cursor as u16 + 1,
                field_chunks[i].y + 1,
            ));
        }
    }
}

fn render_footer(frame: &mut Frame, area: ratatui::layout::Rect, keys: &[(&str, &str)]) {
    let spans: Vec<Span> = keys
        .iter()
        .flat_map(|(key, desc)| {
            vec![
                Span::styled(format!(" {key} "), Style::default().fg(Color::Yellow)),
                Span::raw(format!("{desc}  ")),
            ]
        })
        .collect();

    let footer = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, area);
}

fn render_footer_with_status(frame: &mut Frame, area: ratatui::layout::Rect, keys: &[(&str, &str)], status: &str) {
    let mut spans: Vec<Span> = keys
        .iter()
        .flat_map(|(key, desc)| {
            vec![
                Span::styled(format!(" {key} "), Style::default().fg(Color::Yellow)),
                Span::raw(format!("{desc}  ")),
            ]
        })
        .collect();

    spans.push(Span::styled(
        format!("│ {status}"),
        Style::default().fg(Color::Green),
    ));

    let footer = Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, area);
}
