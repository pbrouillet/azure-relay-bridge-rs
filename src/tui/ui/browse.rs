use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::tui::app::{App, BrowseEntry};

pub fn render_list(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    // Title shows the current directory path
    let dir_display = app.browse_dir.display().to_string();
    let title = Paragraph::new(dir_display)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM).title(" Browse "));
    frame.render_widget(title, chunks[0]);

    if app.browse_entries.is_empty() {
        let empty = Paragraph::new("Empty directory.")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(empty, chunks[1]);
    } else {
        let items: Vec<ListItem> = app
            .browse_entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let is_selected = i == app.browse_index;
                let style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let prefix = if is_selected { "▸ " } else { "  " };

                match entry {
                    BrowseEntry::ParentDir => {
                        ListItem::new(Line::from(Span::styled(
                            format!("{prefix}📁 .."),
                            style,
                        )))
                    }
                    BrowseEntry::Directory { name, .. } => {
                        ListItem::new(Line::from(Span::styled(
                            format!("{prefix}📁 {name}"),
                            style,
                        )))
                    }
                    BrowseEntry::ConfigFile { name, error, .. } => {
                        let status = if error.is_some() {
                            Span::styled(" ✗", Style::default().fg(Color::Red))
                        } else {
                            Span::styled(" ✓", Style::default().fg(Color::Green))
                        };
                        ListItem::new(Line::from(vec![
                            Span::styled(format!("{prefix}📄 {name}"), style),
                            status,
                        ]))
                    }
                }
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL),
        );
        frame.render_widget(list, chunks[1]);
    }

    let footer_spans = vec![
        Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow)),
        Span::raw("Navigate  "),
        Span::styled(" Enter ", Style::default().fg(Color::Yellow)),
        Span::raw("Open  "),
        Span::styled(" Bksp ", Style::default().fg(Color::Yellow)),
        Span::raw("Parent  "),
        Span::styled(" ~ ", Style::default().fg(Color::Yellow)),
        Span::raw("Home  "),
        Span::styled(" Esc ", Style::default().fg(Color::Yellow)),
        Span::raw("Back"),
    ];
    let footer = Paragraph::new(Line::from(footer_spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, chunks[2]);
}

pub fn render_detail(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    let (name, path, config, error) = match &app.selected_config {
        Some(BrowseEntry::ConfigFile { name, path, config, error }) => {
            (name.as_str(), path, config.as_ref(), error.as_deref())
        }
        _ => return,
    };

    let title = Paragraph::new(format!("Config: {name}"))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    let content = if let Some(err) = error {
        vec![
            Line::from(Span::styled(
                format!("Path: {}", path.display()),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("Error: {err}"),
                Style::default().fg(Color::Red),
            )),
        ]
    } else if let Some(config) = config {
        let mut lines = vec![
            Line::from(Span::styled(
                format!("Path: {}", path.display()),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
        ];

        if let Some(ref ep) = config.azure_relay_endpoint {
            lines.push(config_line("Endpoint", ep));
        }
        if let Some(ref cs) = config.azure_relay_connection_string {
            let masked = if cs.len() > 20 {
                format!("{}...", &cs[..20])
            } else {
                cs.clone()
            };
            lines.push(config_line("Connection String", &masked));
        }
        if let Some(ref ll) = config.log_level {
            lines.push(config_line("Log Level", ll));
        }
        if let Some(gp) = config.gateway_ports {
            lines.push(config_line("Gateway Ports", &gp.to_string()));
        }
        if let Some(ct) = config.connect_timeout {
            lines.push(config_line("Connect Timeout", &ct.to_string()));
        }
        if let Some(ka) = config.keep_alive_interval {
            lines.push(config_line("Keep Alive", &ka.to_string()));
        }

        if !config.local_forward.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Local Forwards ({})", config.local_forward.len()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            for lf in &config.local_forward {
                lines.push(Line::from(Span::styled(
                    format!("  relay: {}", lf.relay_name),
                    Style::default().fg(Color::White),
                )));
                for b in &lf.bindings {
                    let addr = b.bind_address.as_deref().unwrap_or("*");
                    lines.push(Line::from(Span::styled(
                        format!("    {}:{}", addr, b.bind_port),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        if !config.remote_forward.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Remote Forwards ({})", config.remote_forward.len()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            for rf in &config.remote_forward {
                lines.push(Line::from(Span::styled(
                    format!("  relay: {}", rf.relay_name),
                    Style::default().fg(Color::White),
                )));
                for b in &rf.bindings {
                    let host = b.host.as_deref().unwrap_or("localhost");
                    lines.push(Line::from(Span::styled(
                        format!("    → {}:{}", host, b.host_port),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        lines
    } else {
        vec![Line::from("No data available.")]
    };

    let detail = Paragraph::new(content)
        .scroll((app.detail_scroll, 0))
        .block(Block::default().borders(Borders::ALL).title(" Details "));
    frame.render_widget(detail, chunks[1]);

    let footer_spans = vec![
        Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow)),
        Span::raw("Scroll  "),
        Span::styled(" r ", Style::default().fg(Color::Yellow)),
        Span::raw("Run  "),
        Span::styled(" d ", Style::default().fg(Color::Yellow)),
        Span::raw("Delete  "),
        Span::styled(" Esc ", Style::default().fg(Color::Yellow)),
        Span::raw("Back"),
    ];
    let footer = Paragraph::new(Line::from(footer_spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, chunks[2]);
}

fn config_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(Color::Cyan)),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}
