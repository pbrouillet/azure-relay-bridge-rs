use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::tui::app::App;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);

    // Title
    let title = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Azure Relay Bridge",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Interactive Configuration Tool",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    // Menu items
    let items = ["Scaffold Config", "Browse Configs", "Run", "Quit"];
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == app.menu_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == app.menu_index { "▸ " } else { "  " };
            ListItem::new(Line::from(Span::styled(
                format!("{prefix}{item}"),
                style,
            )))
        })
        .collect();

    let menu = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Menu "),
    );

    let menu_area = centered_rect(40, 30, chunks[1]);
    frame.render_widget(menu, menu_area);

    // Footer
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow)),
        Span::raw("Navigate  "),
        Span::styled(" Enter ", Style::default().fg(Color::Yellow)),
        Span::raw("Select  "),
        Span::styled(" q ", Style::default().fg(Color::Yellow)),
        Span::raw("Quit"),
    ]))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, chunks[2]);
}

/// Create a centered rect of a given percentage width and height.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
