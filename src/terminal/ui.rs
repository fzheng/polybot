//! Terminal UI rendering

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use rust_decimal::Decimal;

use super::app::App;

/// Draw the main UI
pub fn draw(f: &mut Frame, app: &App) {
    // Header height: 4 base lines + 2 if paper trading (balance/pnl)
    let header_height = if app.is_paper_trading() { 9 } else { 7 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),  // Header/prices
            Constraint::Min(10),    // Messages
            Constraint::Length(3),  // Input
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_messages(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(area);

    // Left side - prices
    draw_prices(f, app, chunks[0]);

    // Right side - status
    draw_status(f, app, chunks[1]);
}

fn draw_prices(f: &mut Frame, app: &App, area: Rect) {
    let up_price = app.up_price().unwrap_or(Decimal::ZERO);
    let down_price = app.down_price().unwrap_or(Decimal::ZERO);
    let sum = up_price + down_price;

    let up_color = if up_price > Decimal::ZERO { Color::Green } else { Color::DarkGray };
    let down_color = if down_price > Decimal::ZERO { Color::Red } else { Color::DarkGray };
    let sum_color = if sum > Decimal::ZERO && sum < Decimal::ONE {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let text = vec![
        Line::from(vec![
            Span::styled("UP:   ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", up_price),
                Style::default().fg(up_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("DOWN: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", down_price),
                Style::default().fg(down_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("SUM:  ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", sum),
                Style::default().fg(sum_color).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let block = Block::default()
        .title(" Prices ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let auto_status = if app.auto_enabled() {
        Span::styled("ON", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("OFF", Style::default().fg(Color::Red))
    };

    let snapshots = app.recorder_snapshots();

    let mut text = vec![
        Line::from(vec![
            Span::raw("Market: "),
            Span::styled(
                app.market_slug().unwrap_or_else(|| "---".to_string()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::raw("Time:   "),
            Span::styled(
                app.seconds_remaining()
                    .map(|s| format!("{}s", s))
                    .unwrap_or_else(|| "---".to_string()),
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::from(vec![
            Span::raw("Auto:   "),
            auto_status,
        ]),
    ];

    // Show paper trading balance or recording status
    if app.is_paper_trading() {
        let pnl = app.paper_pnl();
        let pnl_color = if pnl >= Decimal::ZERO { Color::Green } else { Color::Red };
        let pnl_sign = if pnl >= Decimal::ZERO { "+" } else { "" };

        text.push(Line::from(vec![
            Span::raw("Bal:    "),
            Span::styled(
                format!("${:.2}", app.paper_balance()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]));
        text.push(Line::from(vec![
            Span::raw("P&L:    "),
            Span::styled(
                format!("{}${:.2}", pnl_sign, pnl.abs()),
                Style::default().fg(pnl_color).add_modifier(Modifier::BOLD),
            ),
        ]));
    } else {
        text.push(Line::from(vec![
            Span::raw("Rec:    "),
            Span::styled(
                format!("{} snapshots", snapshots),
                Style::default().fg(Color::Blue),
            ),
        ]));
    }

    let title = if app.is_paper_trading() { " Paper Trading " } else { " Status " };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.is_paper_trading() { Color::Yellow } else { Color::Cyan }));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
    let messages = app.messages();

    // Calculate how many messages fit
    let height = area.height.saturating_sub(2) as usize;
    let skip = messages.len().saturating_sub(height);

    let items: Vec<ListItem> = messages
        .iter()
        .skip(skip)
        .map(|m| {
            let style = if m.starts_with('>') {
                Style::default().fg(Color::Cyan)
            } else if m.contains("ERROR") || m.contains("failed") || m.contains("error") {
                Style::default().fg(Color::Red)
            } else if m.contains("LEG") || m.contains("COMPLETE") {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else if m.contains("Warning") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(m.as_str(), style)))
        })
        .collect();

    let block = Block::default()
        .title(" Log ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let input = app.input();

    let block = Block::default()
        .title(" Command (type 'help' for commands) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(format!("> {}", input))
        .style(Style::default().fg(Color::White))
        .block(block);

    f.render_widget(paragraph, area);

    // Show cursor
    f.set_cursor_position((
        area.x + 3 + input.len() as u16,
        area.y + 1,
    ));
}
