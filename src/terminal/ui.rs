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
    // Main layout: header row, middle row (positions + history), log, input
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),   // Header row (prices + status)
            Constraint::Length(6),   // Middle row (positions + history)
            Constraint::Min(8),      // Log
            Constraint::Length(3),   // Input
        ])
        .split(f.area());

    draw_header(f, app, main_chunks[0]);
    draw_middle_row(f, app, main_chunks[1]);
    draw_messages(f, app, main_chunks[2]);
    draw_input(f, app, main_chunks[3]);
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

fn draw_middle_row(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(area);

    draw_live_position(f, app, chunks[0]);
    draw_history(f, app, chunks[1]);
}

fn draw_prices(f: &mut Frame, app: &App, area: Rect) {
    let up_price = app.up_price().unwrap_or(Decimal::ZERO);
    let down_price = app.down_price().unwrap_or(Decimal::ZERO);
    let up_size = app.up_ask_size();
    let down_size = app.down_ask_size();
    let sum = up_price + down_price;

    let up_color = if up_price > Decimal::ZERO { Color::Green } else { Color::DarkGray };
    let down_color = if down_price > Decimal::ZERO { Color::Red } else { Color::DarkGray };
    let sum_color = if sum > Decimal::ZERO && sum < Decimal::ONE {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    // Format sizes if available
    let up_size_str = up_size.map(|s| format!(" x{:.0}", s)).unwrap_or_default();
    let down_size_str = down_size.map(|s| format!(" x{:.0}", s)).unwrap_or_default();

    let text = vec![
        Line::from(vec![
            Span::styled("UP:   ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", up_price),
                Style::default().fg(up_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                up_size_str,
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("DOWN: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", down_price),
                Style::default().fg(down_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                down_size_str,
                Style::default().fg(Color::DarkGray),
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
    let mode_status = if app.is_paper_mode() {
        Span::styled("PAPER", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("LIVE", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
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
            Span::raw("Mode:   "),
            mode_status,
        ]),
    ];

    // Show balance and P&L (paper or live) or recording status
    if app.is_paper_trading() || app.is_live_trading() {
        let pnl = app.trading_pnl();
        let pnl_color = if pnl >= Decimal::ZERO { Color::Green } else { Color::Red };
        let pnl_sign = if pnl >= Decimal::ZERO { "+" } else { "-" };

        text.push(Line::from(vec![
            Span::styled("Bal: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("${:.2}", app.trading_balance()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  P&L: ", Style::default().fg(Color::DarkGray)),
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

fn draw_live_position(f: &mut Frame, app: &App, area: Rect) {
    let pos = app.live_position();
    let mut text = Vec::new();

    let has_position = pos.up_shares > Decimal::ZERO || pos.down_shares > Decimal::ZERO;

    if has_position {
        if pos.up_shares > Decimal::ZERO {
            text.push(Line::from(vec![
                Span::styled("UP:   ", Style::default().fg(Color::Green)),
                Span::styled(
                    format!("{:.0}", pos.up_shares),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" @ ${:.4}", pos.up_avg_price),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        if pos.down_shares > Decimal::ZERO {
            text.push(Line::from(vec![
                Span::styled("DOWN: ", Style::default().fg(Color::Red)),
                Span::styled(
                    format!("{:.0}", pos.down_shares),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" @ ${:.4}", pos.down_avg_price),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        // Show arbitrage status if we have both sides
        if let Some(avg_cost) = pos.avg_cost_per_pair {
            let profit_margin = Decimal::ONE - avg_cost;
            let status_color = if avg_cost < Decimal::ONE { Color::Green } else { Color::Red };
            let status_text = if avg_cost < Decimal::ONE {
                format!("Avg: ${:.4} (+{:.1}%)", avg_cost, profit_margin * Decimal::ONE_HUNDRED)
            } else {
                format!("Avg: ${:.4} (LOSS)", avg_cost)
            };
            text.push(Line::from(Span::styled(status_text, Style::default().fg(status_color))));
        }
    } else {
        text.push(Line::from(Span::styled("No position", Style::default().fg(Color::DarkGray))));
    }

    let block = Block::default()
        .title(" Position ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn draw_history(f: &mut Frame, app: &App, area: Rect) {
    let history = app.settled_history();
    let height = area.height.saturating_sub(2) as usize;

    let items: Vec<ListItem> = if history.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No settled rounds",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        history
            .iter()
            .rev()
            .take(height)
            .map(|round| {
                let pnl_color = if round.net_pnl >= Decimal::ZERO { Color::Green } else { Color::Red };
                let pnl_sign = if round.net_pnl >= Decimal::ZERO { "+" } else { "" };

                // Extract just the timestamp part from slug for brevity
                let short_slug = round.round_slug.split('-').last().unwrap_or(&round.round_slug);

                // Format: "timestamp | bet $X | SIDE won | P&L"
                Line::from(vec![
                    Span::styled(
                        format!("{} ", short_slug),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("${:.0} ", round.total_cost),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("{} ", round.winning_side),
                        Style::default().fg(if round.winning_side == crate::types::Side::Up { Color::Green } else { Color::Red }),
                    ),
                    Span::styled(
                        format!("{}${:.2}", pnl_sign, round.net_pnl.abs()),
                        Style::default().fg(pnl_color).add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .map(ListItem::new)
            .collect()
    };

    let block = Block::default()
        .title(" History ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let list = List::new(items).block(block);
    f.render_widget(list, area);
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
