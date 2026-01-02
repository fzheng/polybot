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

/// Format seconds remaining as "Xm Ys" or "Ys" if less than 1 minute
fn format_time_remaining(seconds: i64) -> String {
    if seconds < 0 {
        return "0s".to_string();
    }
    if seconds < 60 {
        format!("{}s", seconds)
    } else {
        let mins = seconds / 60;
        let secs = seconds % 60;
        format!("{}m {}s", mins, secs)
    }
}

/// Draw the main UI
pub fn draw(f: &mut Frame, app: &App) {
    // Main layout: header row, middle row (positions + history), log, input
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),   // Header row (prices + status)
            Constraint::Length(10),  // Middle row (positions + history) - increased for multi-market
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
    let sum = up_price + down_price;

    let up_color = if up_price > Decimal::ZERO { Color::Green } else { Color::DarkGray };
    let down_color = if down_price > Decimal::ZERO { Color::Red } else { Color::DarkGray };
    let sum_color = if sum > Decimal::ZERO && sum < Decimal::ONE {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    // Format depth levels (up to 3 levels shown inline)
    let up_levels = app.up_ask_levels();
    let down_levels = app.down_ask_levels();

    let up_depth_str = format_depth_levels(up_levels);
    let down_depth_str = format_depth_levels(down_levels);

    let text = vec![
        Line::from(vec![
            Span::styled("UP:   ", Style::default().fg(Color::White)),
            Span::styled(
                format!("${:.4}", up_price),
                Style::default().fg(up_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", up_depth_str),
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
                format!(" {}", down_depth_str),
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

/// Format depth levels for display (up to 3 levels)
/// Returns format like: "x10@.45 x20@.46 x15@.47" or "no depth" if empty
fn format_depth_levels(levels: &[crate::types::OrderBookLevel]) -> String {
    if levels.is_empty() {
        return "no depth".to_string();
    }

    levels.iter()
        .take(3)
        .map(|level| format!("x{:.0}@{:.2}", level.size, level.price))
        .collect::<Vec<_>>()
        .join(" ")
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mode_status = if app.is_paper_mode() {
        Span::styled("PAPER", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("LIVE", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    };

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
                    .map(|s| format_time_remaining(s))
                    .unwrap_or_else(|| "---".to_string()),
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::from(vec![
            Span::raw("Mode:   "),
            mode_status,
        ]),
    ];

    // Show balance and P&L
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

    let (title, border_color) = if app.is_paper_trading() {
        (" Paper Trading ", Color::Yellow)
    } else {
        (" Live Trading ", Color::Red)
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn draw_live_position(f: &mut Frame, app: &App, area: Rect) {
    let multi_pos = app.multi_market_positions();
    let mut text = Vec::new();

    if multi_pos.positions.is_empty() {
        text.push(Line::from(Span::styled("No position", Style::default().fg(Color::DarkGray))));
    } else {
        // Get the active market slug for comparison
        let active_slug = multi_pos.positions.iter()
            .find(|(_, _, is_active)| *is_active)
            .map(|(slug, _, _)| slug.as_str());

        let mut first = true;
        for (slug, pos, is_active) in &multi_pos.positions {
            // Add separator between positions
            if !first {
                text.push(Line::from(""));
            }
            first = false;

            // Extract short slug (timestamp only)
            let short_slug = slug.split('-').last().unwrap_or(slug);

            // Determine label based on position relative to active market
            let (label, color) = if *is_active {
                // This is the currently running market (by wall clock time)
                // Check if it's hedged (has both UP and DOWN)
                if pos.up_shares > Decimal::ZERO && pos.down_shares > Decimal::ZERO {
                    ("Active (hedged)", Color::Green)
                } else {
                    ("Active", Color::Cyan)
                }
            } else if let Some(active) = active_slug {
                // Compare slugs to determine if this is past or future
                if slug.as_str() < active {
                    // Earlier timestamp = previous market (pending settlement)
                    if pos.up_shares > Decimal::ZERO && pos.down_shares > Decimal::ZERO {
                        ("Settling (hedged)", Color::Magenta)
                    } else {
                        ("Settling", Color::Magenta)
                    }
                } else {
                    // Later timestamp = future market (early entry)
                    ("Upcoming", Color::Yellow)
                }
            } else {
                // No active market found, just show as pending
                ("Pending", Color::DarkGray)
            };

            text.push(Line::from(Span::styled(
                format!("[{}] {}", short_slug, label),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            add_position_lines(&mut text, pos);
        }
    }

    let block = Block::default()
        .title(" Positions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

/// Helper to add position detail lines to text vector
fn add_position_lines(text: &mut Vec<Line<'_>>, pos: &crate::paper::LivePositionSummary) {
    if pos.up_shares > Decimal::ZERO {
        text.push(Line::from(vec![
            Span::styled(" UP:   ", Style::default().fg(Color::Green)),
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
            Span::styled(" DOWN: ", Style::default().fg(Color::Red)),
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
            format!(" Avg: ${:.4} (+{:.1}%)", avg_cost, profit_margin * Decimal::ONE_HUNDRED)
        } else {
            format!(" Avg: ${:.4} (LOSS)", avg_cost)
        };
        text.push(Line::from(Span::styled(status_text, Style::default().fg(status_color))));
    }
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
