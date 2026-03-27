use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::app::App;
use crate::tui::runner::RunStatus;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);

    // Title
    let config_label = app
        .run_config_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<none>".to_string());

    let title = Paragraph::new(format!("Running: {config_label}"))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    // Status bar
    let (status_text, status_color) = match &app.runner.status {
        RunStatus::Idle => ("● IDLE", Color::DarkGray),
        RunStatus::Starting => ("● STARTING", Color::Yellow),
        RunStatus::Running => ("● RUNNING", Color::Green),
        RunStatus::Stopped => ("● STOPPED", Color::DarkGray),
        RunStatus::Error(_e) => ("● ERROR", Color::Red),
    };

    let mut status_spans = vec![
        Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
    ];
    if let RunStatus::Error(ref e) = app.runner.status {
        status_spans.push(Span::raw(format!("  {e}")));
    }

    let status_bar = Paragraph::new(Line::from(status_spans))
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    frame.render_widget(status_bar, chunks[1]);

    // Log area
    let log_lines: Vec<Line> = app
        .runner
        .logs
        .iter()
        .map(|msg| {
            let color = if msg.contains("ERROR") || msg.contains("failed") {
                Color::Red
            } else if msg.contains("WARN") {
                Color::Yellow
            } else if msg.starts_with("Starting") || msg.starts_with("Bridge started") {
                Color::Green
            } else {
                Color::White
            };
            Line::from(Span::styled(msg.clone(), Style::default().fg(color)))
        })
        .collect();

    let log_panel = Paragraph::new(log_lines)
        .scroll((app.run_log_scroll, app.run_log_hscroll))
        .block(Block::default().borders(Borders::ALL).title(" Logs "));
    // Store the visible log area height (minus borders) for auto-scroll math
    app.log_viewport_height = chunks[2].height.saturating_sub(2);
    frame.render_widget(log_panel, chunks[2]);

    // Footer
    let auto_scroll_indicator = if app.run_auto_scroll { "[auto-scroll]" } else { "" };
    let footer_spans = vec![
        Span::styled(" q/Esc ", Style::default().fg(Color::Yellow)),
        Span::raw("Stop  "),
        Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow)),
        Span::raw("Scroll  "),
        Span::styled(" ←→ ", Style::default().fg(Color::Yellow)),
        Span::raw("Pan  "),
        Span::styled(" Home ", Style::default().fg(Color::Yellow)),
        Span::raw("Reset  "),
        Span::styled(" f ", Style::default().fg(Color::Yellow)),
        Span::raw("Follow  "),
        Span::styled(auto_scroll_indicator, Style::default().fg(Color::DarkGray)),
    ];
    let footer = Paragraph::new(Line::from(footer_spans))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, chunks[3]);
}
