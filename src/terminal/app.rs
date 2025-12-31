//! Main terminal application

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use rust_decimal::Decimal;
use std::io;
use std::time::Duration;

use crate::config::Config;
use crate::market::MarketWatcher;
use crate::paper::PaperTrader;
use crate::recorder::Recorder;
use crate::strategy::AutoTrader;
use crate::types::{AutoParams, Side};

use super::ui;

/// Application state
pub struct App {
    config: Config,
    watcher: MarketWatcher,
    auto_trader: AutoTrader,
    recorder: Recorder,
    paper_trader: PaperTrader,
    record_only: bool,

    // UI state
    input: String,
    messages: Vec<String>,
    should_quit: bool,
    last_up_price: Option<Decimal>,
    last_down_price: Option<Decimal>,

    // Cached state for UI
    cached_balance: Decimal,
    cached_pnl: Decimal,
    cached_auto_enabled: bool,
    cached_market_slug: Option<String>,
    cached_seconds_remaining: Option<i64>,
}

impl App {
    /// Create a new application
    pub async fn new(config: Config, record_only: bool) -> Result<Self> {
        let auto_params = AutoParams {
            shares: config.trading.default_shares,
            sum_target: config.trading.default_sum_target,
            move_pct: config.trading.default_move_pct,
            window_min: config.trading.default_window_min,
        };

        let recorder = Recorder::new(config.recording.clone())?;
        let paper_trader = PaperTrader::new(config.paper_trading.clone())?;
        let starting_balance = paper_trader.get_starting_balance();

        let mut messages = vec![
            "Welcome to PolyBot - Polymarket Trading Bot".to_string(),
        ];

        if config.paper_trading.enabled {
            messages.push(format!(
                "PAPER TRADING MODE - Starting balance: ${:.2}",
                starting_balance
            ));
        } else {
            messages.push("LIVE TRADING MODE - Real orders will be placed!".to_string());
        }
        messages.push("Type 'help' for available commands".to_string());
        messages.push("".to_string());

        Ok(Self {
            watcher: MarketWatcher::new(config.clone()),
            auto_trader: AutoTrader::new(auto_params),
            recorder,
            paper_trader,
            record_only,
            input: String::new(),
            messages,
            should_quit: false,
            last_up_price: None,
            last_down_price: None,
            cached_balance: starting_balance,
            cached_pnl: Decimal::ZERO,
            cached_auto_enabled: false,
            cached_market_slug: None,
            cached_seconds_remaining: None,
            config,
        })
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal first so user sees progress
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Show initial UI with connecting message
        self.log("Connecting to Polymarket...");
        terminal.draw(|f| ui::draw(f, self))?;

        // Initialize market watcher
        if let Err(e) = self.watcher.start().await {
            self.log(&format!("Warning: {}", e));
        } else {
            self.log("Connected!");
        }

        // Main loop
        let tick_rate = Duration::from_millis(100);
        let mut last_tick = std::time::Instant::now();

        loop {
            // Draw UI
            terminal.draw(|f| ui::draw(f, self))?;

            // Handle events
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if crossterm::event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    // Only handle key press events (not release) to avoid double input on Windows
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.should_quit = true;
                        }
                        KeyCode::Char(c) => {
                            self.input.push(c);
                        }
                        KeyCode::Backspace => {
                            self.input.pop();
                        }
                        KeyCode::Enter => {
                            self.handle_command().await;
                        }
                        KeyCode::Esc => {
                            self.should_quit = true;
                        }
                        _ => {}
                    }
                }
            }

            // Tick processing
            if last_tick.elapsed() >= tick_rate {
                self.tick().await?;
                last_tick = std::time::Instant::now();
            }

            if self.should_quit {
                break;
            }
        }

        // Cleanup
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        self.watcher.stop().await;
        self.recorder.flush()?;
        if let Err(e) = self.paper_trader.flush().await {
            tracing::error!("Failed to flush paper trader: {}", e);
        }

        Ok(())
    }

    /// Process a tick
    async fn tick(&mut self) -> Result<()> {
        // Refresh market if needed
        if let Ok(changed) = self.watcher.refresh_market().await {
            if changed {
                if let Some(m) = self.watcher.get_current_market().await {
                    self.log(&format!("New round: {}", m.slug));
                }
            }
        }

        // Update cached market info for UI
        if let Some(m) = self.watcher.get_current_market().await {
            self.cached_market_slug = Some(m.slug.clone());
            self.cached_seconds_remaining = Some(m.seconds_remaining());
        } else {
            self.cached_market_slug = None;
            self.cached_seconds_remaining = None;
        }

        // Update cached auto_enabled for UI
        self.cached_auto_enabled = self.auto_trader.is_enabled().await;

        // Update prices
        self.last_up_price = self.watcher.get_up_price().await;
        self.last_down_price = self.watcher.get_down_price().await;

        // Update paper trader with current prices
        if self.config.paper_trading.enabled {
            self.paper_trader.update_prices(self.last_up_price, self.last_down_price).await;
            // Update cached values for UI
            self.cached_balance = self.paper_trader.get_balance().await;
            self.cached_pnl = self.paper_trader.get_total_pnl().await;
        }

        // Record snapshot
        if let Some(snapshot) = self.watcher.get_snapshot().await {
            if let Err(e) = self.recorder.record(&snapshot) {
                tracing::error!("Recording failed: {}", e);
            }
        }

        // Run auto trader (if not in record-only mode)
        if !self.record_only {
            if let Err(e) = self.auto_trader.tick(&self.watcher).await {
                self.log(&format!("Strategy error: {}", e));
            }
        }

        Ok(())
    }

    /// Handle a command input
    async fn handle_command(&mut self) {
        let input = std::mem::take(&mut self.input);
        let parts: Vec<&str> = input.trim().split_whitespace().collect();

        if parts.is_empty() {
            return;
        }

        self.log(&format!("> {}", input));

        match parts[0].to_lowercase().as_str() {
            "help" => self.show_help(),
            "status" => self.show_status().await,
            "buy" => self.handle_buy(&parts).await,
            "buyshares" => self.handle_buyshares(&parts).await,
            "auto" => self.handle_auto(&parts).await,
            "params" => self.show_params().await,
            "logs" => self.show_logs().await,
            "balance" | "bal" => self.show_balance().await,
            "positions" | "pos" => self.show_positions().await,
            "pnl" => self.show_pnl().await,
            "trades" => self.show_trades().await,
            "reset" => self.reset_paper_trading().await,
            "quit" | "exit" | "q" => self.should_quit = true,
            "clear" => self.messages.clear(),
            _ => self.log(&format!("Unknown command: {}", parts[0])),
        }
    }

    fn show_help(&mut self) {
        self.log("Available commands:");
        self.log("  status           - Show current market status");
        self.log("  buy up <usd>     - Buy UP shares for USD amount");
        self.log("  buy down <usd>   - Buy DOWN shares for USD amount");
        self.log("  buyshares up <n> - Buy N UP shares at best ask");
        self.log("  buyshares down <n> - Buy N DOWN shares at best ask");
        self.log("  auto on <shares> [sum] [move] [window]");
        self.log("                   - Enable auto trading");
        self.log("  auto off         - Disable auto trading");
        self.log("  params           - Show current parameters");
        self.log("  logs             - Show strategy logs");
        if self.config.paper_trading.enabled {
            self.log("  balance (bal)    - Show paper trading balance");
            self.log("  positions (pos)  - Show current positions");
            self.log("  pnl              - Show profit/loss summary");
            self.log("  trades           - Show recent trades");
            self.log("  reset            - Reset paper trading");
        }
        self.log("  clear            - Clear message log");
        self.log("  quit             - Exit the bot");
    }

    async fn show_status(&mut self) {
        let market = self.watcher.get_current_market().await;

        if let Some(m) = market {
            self.log(&format!("Market: {}", m.slug));
            self.log(&format!("Time remaining: {}s", m.seconds_remaining()));

            if let Some(up) = self.last_up_price {
                self.log(&format!("UP:   ${:.4}", up));
            }
            if let Some(down) = self.last_down_price {
                self.log(&format!("DOWN: ${:.4}", down));
            }

            if let (Some(up), Some(down)) = (self.last_up_price, self.last_down_price) {
                self.log(&format!("SUM:  ${:.4}", up + down));
            }
        } else {
            self.log("No active market found");
        }

        let state = self.auto_trader.get_state().await;
        self.log(&format!("Auto: {} ({:?})",
            if self.auto_trader.is_enabled().await { "ON" } else { "OFF" },
            state
        ));
    }

    async fn handle_buy(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            self.log("Usage: buy <up|down> <usd_amount>");
            return;
        }

        let side = match parts[1].to_lowercase().as_str() {
            "up" => Side::Up,
            "down" => Side::Down,
            _ => {
                self.log("Invalid side. Use 'up' or 'down'");
                return;
            }
        };

        let amount: Decimal = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => {
                self.log("Invalid amount");
                return;
            }
        };

        // Get current price for share calculation
        let price = match side {
            Side::Up => self.last_up_price,
            Side::Down => self.last_down_price,
        };

        let Some(price) = price else {
            self.log("No price available for this side");
            return;
        };

        // Calculate shares from USD amount
        let shares = amount / price;

        if self.config.paper_trading.enabled {
            // Execute paper trade
            let round_slug = self.watcher.get_current_market().await
                .map(|m| m.slug.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let token_id = format!("{}_{}", round_slug, side.as_str().to_lowercase());

            match self.paper_trader.buy(&token_id, side, shares, price, &round_slug).await {
                Ok(result) => {
                    self.log(&format!(
                        "[PAPER] Bought {:.2} {} shares at ${:.4} for ${:.2}",
                        result.filled_size, side, result.price, amount
                    ));
                    self.cached_balance = self.paper_trader.get_balance().await;
                }
                Err(e) => {
                    self.log(&format!("[PAPER] Buy failed: {}", e));
                }
            }
        } else {
            self.log(&format!("Buying ${} worth of {} shares...", amount, side));
            // TODO: Execute real trade when authenticated
            if !self.watcher.client().is_authenticated() {
                self.log("Not authenticated - configure private key for live trading");
            }
        }
    }

    async fn handle_buyshares(&mut self, parts: &[&str]) {
        if parts.len() < 3 {
            self.log("Usage: buyshares <up|down> <shares>");
            return;
        }

        let side = match parts[1].to_lowercase().as_str() {
            "up" => Side::Up,
            "down" => Side::Down,
            _ => {
                self.log("Invalid side. Use 'up' or 'down'");
                return;
            }
        };

        let shares: Decimal = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => {
                self.log("Invalid shares amount");
                return;
            }
        };

        let price = match side {
            Side::Up => self.last_up_price,
            Side::Down => self.last_down_price,
        };

        let Some(price) = price else {
            self.log("No price available");
            return;
        };

        if self.config.paper_trading.enabled {
            // Execute paper trade
            let round_slug = self.watcher.get_current_market().await
                .map(|m| m.slug.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let token_id = format!("{}_{}", round_slug, side.as_str().to_lowercase());

            match self.paper_trader.buy(&token_id, side, shares, price, &round_slug).await {
                Ok(result) => {
                    let cost = result.price * result.filled_size;
                    self.log(&format!(
                        "[PAPER] Bought {:.2} {} shares at ${:.4} (cost: ${:.2})",
                        result.filled_size, side, result.price, cost
                    ));
                    self.cached_balance = self.paper_trader.get_balance().await;
                }
                Err(e) => {
                    self.log(&format!("[PAPER] Buy failed: {}", e));
                }
            }
        } else {
            self.log(&format!("Buying {} {} shares at ${:.4}...", shares, side, price));
            // TODO: Execute real trade when authenticated
            if !self.watcher.client().is_authenticated() {
                self.log("Not authenticated - configure private key for live trading");
            }
        }
    }

    async fn handle_auto(&mut self, parts: &[&str]) {
        if parts.len() < 2 {
            self.log("Usage: auto on <shares> [sum=0.95] [move=0.15] [window=2]");
            self.log("       auto off");
            return;
        }

        match parts[1].to_lowercase().as_str() {
            "on" => {
                if parts.len() < 3 {
                    self.log("Usage: auto on <shares> [sum] [move] [window]");
                    return;
                }

                let shares: Decimal = match parts[2].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        self.log("Invalid shares amount");
                        return;
                    }
                };

                let sum = parts.get(3)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::new(95, 2));

                let move_pct = parts.get(4)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::new(15, 2));

                let window = parts.get(5)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(2u32);

                // Update the params
                let new_params = AutoParams {
                    shares,
                    sum_target: sum,
                    move_pct,
                    window_min: window,
                };
                self.auto_trader.set_params(new_params).await;

                self.log(&format!(
                    "Auto ON: {} shares, sum={}, move={}%, window={}min",
                    shares, sum, move_pct * Decimal::ONE_HUNDRED, window
                ));

                self.auto_trader.enable().await;
            }
            "off" => {
                self.auto_trader.disable().await;
                self.log("Auto trading disabled");
            }
            _ => {
                self.log("Usage: auto on/off");
            }
        }
    }

    async fn show_params(&mut self) {
        let params = self.auto_trader.get_params().await;
        self.log("Current parameters:");
        self.log(&format!("  shares:    {}", params.shares));
        self.log(&format!("  sum:       {}", params.sum_target));
        self.log(&format!("  move:      {}%", params.move_pct * Decimal::ONE_HUNDRED));
        self.log(&format!("  window:    {} min", params.window_min));
    }

    async fn show_logs(&mut self) {
        let logs = self.auto_trader.get_logs().await;
        if logs.is_empty() {
            self.log("No strategy logs yet");
        } else {
            self.log("Strategy logs:");
            for log in logs.iter().rev().take(10) {
                let prefix = if log.is_error { "ERROR" } else { "INFO" };
                self.log(&format!("  [{}] {}: {}",
                    log.timestamp.format("%H:%M:%S"),
                    prefix,
                    log.message
                ));
            }
        }
    }

    async fn show_balance(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        let balance = self.paper_trader.get_balance().await;
        let starting = self.paper_trader.get_starting_balance();
        let diff = balance - starting;
        let diff_str = if diff >= Decimal::ZERO {
            format!("+${:.2}", diff)
        } else {
            format!("-${:.2}", diff.abs())
        };

        self.log(&format!("Paper Trading Balance: ${:.2} ({})", balance, diff_str));
    }

    async fn show_positions(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        let positions = self.paper_trader.get_positions().await;
        if positions.is_empty() {
            self.log("No open positions");
        } else {
            self.log("Open positions:");
            for (token_id, pos) in positions.iter() {
                let pnl_str = if pos.unrealized_pnl >= Decimal::ZERO {
                    format!("+${:.2}", pos.unrealized_pnl)
                } else {
                    format!("-${:.2}", pos.unrealized_pnl.abs())
                };
                self.log(&format!(
                    "  {} {}: {} shares @ ${:.4} avg, value ${:.2} ({})",
                    pos.side, token_id, pos.shares, pos.avg_entry_price, pos.current_value, pnl_str
                ));
            }
        }
    }

    async fn show_pnl(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        let stats = self.paper_trader.get_stats().await;
        let total_pnl = self.paper_trader.get_total_pnl().await;

        self.log("Paper Trading P&L Summary:");
        self.log(&format!("  Total P&L:     ${:.2}", total_pnl));
        self.log(&format!("  Realized:      ${:.2}", stats.realized_pnl));
        self.log(&format!("  Unrealized:    ${:.2}", stats.unrealized_pnl));
        self.log(&format!("  Total trades:  {}", stats.total_trades));
        self.log(&format!("  Winners:       {}", stats.winning_trades));
        self.log(&format!("  Losers:        {}", stats.losing_trades));
        self.log(&format!("  Total fees:    ${:.2}", stats.total_fees));
        self.log(&format!("  Total slippage: ${:.2}", stats.total_slippage));
        if stats.best_trade > Decimal::ZERO {
            self.log(&format!("  Best trade:    ${:.2}", stats.best_trade));
        }
        if stats.worst_trade < Decimal::ZERO {
            self.log(&format!("  Worst trade:   ${:.2}", stats.worst_trade));
        }
        self.log(&format!("  Cycles done:   {}", stats.cycles_completed));
        self.log(&format!("  Abandoned:     {}", stats.cycles_abandoned));
    }

    async fn show_trades(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        let trades = self.paper_trader.get_recent_trades(10).await;
        if trades.is_empty() {
            self.log("No trades yet");
        } else {
            self.log("Recent trades:");
            for trade in trades {
                let action = match trade.buy_sell {
                    crate::types::BuySell::Buy => "BUY",
                    crate::types::BuySell::Sell => "SELL",
                };
                self.log(&format!(
                    "  [{}] {} {} {} @ ${:.4} (fee: ${:.4})",
                    trade.timestamp.format("%H:%M:%S"),
                    action,
                    trade.shares,
                    trade.side,
                    trade.price,
                    trade.fee
                ));
            }
        }
    }

    async fn reset_paper_trading(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        self.paper_trader.reset().await;
        self.cached_balance = self.paper_trader.get_balance().await;
        self.cached_pnl = Decimal::ZERO;
        self.log(&format!(
            "Paper trading reset. Balance: ${:.2}",
            self.cached_balance
        ));
    }

    fn log(&mut self, msg: &str) {
        self.messages.push(msg.to_string());
        // Keep only last 100 messages
        if self.messages.len() > 100 {
            self.messages.remove(0);
        }
    }

    // Getters for UI
    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    pub fn up_price(&self) -> Option<Decimal> {
        self.last_up_price
    }

    pub fn down_price(&self) -> Option<Decimal> {
        self.last_down_price
    }

    pub fn auto_enabled(&self) -> bool {
        self.cached_auto_enabled
    }

    pub fn market_slug(&self) -> Option<String> {
        self.cached_market_slug.clone()
    }

    pub fn seconds_remaining(&self) -> Option<i64> {
        self.cached_seconds_remaining
    }

    pub fn recorder_snapshots(&self) -> u64 {
        self.recorder.snapshots_written()
    }

    pub fn is_paper_trading(&self) -> bool {
        self.config.paper_trading.enabled
    }

    pub fn paper_balance(&self) -> Decimal {
        self.cached_balance
    }

    pub fn paper_pnl(&self) -> Decimal {
        self.cached_pnl
    }
}
