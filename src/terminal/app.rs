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
use chrono::Local;
use rust_decimal::Decimal;
use std::io;
use std::time::Duration;

use crate::api::PolymarketClient;
use crate::config::Config;
use crate::market::MarketWatcher;
use crate::paper::{PaperTrader, LivePositionSummary, SettledRound};
use crate::recorder::Recorder;
use crate::strategy::AutoTrader;
use crate::types::{AutoParams, Side};

use super::ui;

/// Previous market info for settlement
#[derive(Clone)]
struct PreviousMarket {
    slug: String,
    up_token_id: String,
    down_token_id: String,
    /// Final prices before round ended (captured before round change)
    final_up_price: Option<Decimal>,
    final_down_price: Option<Decimal>,
}

/// Application state
pub struct App {
    config: Config,
    client: PolymarketClient,
    watcher: MarketWatcher,
    auto_trader: AutoTrader,
    recorder: Recorder,
    paper_trader: PaperTrader,
    paper_mode: bool,  // true = paper trading, false = live trading
    pending_mode_switch: Option<bool>,  // Pending mode switch awaiting confirmation

    // UI state
    input: String,
    messages: Vec<String>,
    should_quit: bool,
    last_up_price: Option<Decimal>,
    last_down_price: Option<Decimal>,

    // Track previous market for settlement
    previous_market: Option<PreviousMarket>,
    // Pending settlements that were deferred (market not yet resolved)
    pending_settlements: Vec<PreviousMarket>,
    // Last time we tried to retry pending settlements
    last_settlement_retry: Option<std::time::Instant>,

    // Order book info for UI
    cached_up_ask_size: Option<Decimal>,
    cached_down_ask_size: Option<Decimal>,

    // Cached state for UI (paper trading)
    cached_balance: Decimal,
    cached_pnl: Decimal,
    cached_market_slug: Option<String>,
    cached_seconds_remaining: Option<i64>,

    // Cached positions and history for UI (paper trading)
    cached_live_position: LivePositionSummary,
    cached_history: Vec<SettledRound>,

    // Cached state for live trading (fetched from Polymarket API)
    cached_live_balance: Option<Decimal>,
    last_live_balance_fetch: Option<std::time::Instant>,

    // Track how many strategy logs we've already shown in UI
    last_strategy_log_count: usize,
}

impl App {
    /// Create a new application
    pub async fn new(config: Config) -> Result<Self> {
        let auto_params = AutoParams {
            shares: config.trading.default_shares,
            sum_target: config.trading.default_sum_target,
            move_pct: config.trading.default_move_pct,
            window_min: config.trading.default_window_min,
            max_cycles: config.trading.default_max_cycles,
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

        let paper_mode = config.paper_trading.enabled;
        let client = PolymarketClient::new(&config);

        Ok(Self {
            client,
            watcher: MarketWatcher::new(config.clone()),
            auto_trader: AutoTrader::new(auto_params),
            recorder,
            paper_trader,
            paper_mode,
            pending_mode_switch: None,
            input: String::new(),
            messages,
            should_quit: false,
            last_up_price: None,
            last_down_price: None,
            previous_market: None,
            cached_up_ask_size: None,
            cached_down_ask_size: None,
            cached_balance: starting_balance,
            cached_pnl: Decimal::ZERO,
            cached_market_slug: None,
            cached_seconds_remaining: None,
            cached_live_position: LivePositionSummary::default(),
            cached_history: Vec::new(),
            pending_settlements: Vec::new(),
            last_settlement_retry: None,
            cached_live_balance: None,
            last_live_balance_fetch: None,
            last_strategy_log_count: 0,
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
                let new_market = self.watcher.get_current_market().await;

                // Settle previous market's positions if round changed
                // Use the CAPTURED final prices from previous_market (not current prices)
                if let (Some(prev), Some(new)) = (&self.previous_market, &new_market) {
                    if prev.slug != new.slug && self.paper_mode {
                        // Settle positions from the previous round using captured final prices
                        let prev_clone = prev.clone();
                        if !self.settle_previous_round(prev_clone.clone()).await {
                            // Add to pending settlements for retry
                            self.pending_settlements.push(prev_clone);
                        }
                    }
                }

                // Log the new round
                if let Some(m) = &new_market {
                    self.log(&format!("New round: {}", m.slug));

                    // Update previous market tracker with current prices as initial
                    // These will be updated each tick until next round change
                    self.previous_market = Some(PreviousMarket {
                        slug: m.slug.clone(),
                        up_token_id: m.up_token_id.clone(),
                        down_token_id: m.down_token_id.clone(),
                        final_up_price: self.last_up_price,
                        final_down_price: self.last_down_price,
                    });
                }
            }
        }

        // Update cached market info for UI
        if let Some(m) = self.watcher.get_current_market().await {
            self.cached_market_slug = Some(m.slug.clone());
            self.cached_seconds_remaining = Some(m.seconds_remaining());

            // Initialize previous_market if not set
            if self.previous_market.is_none() {
                self.previous_market = Some(PreviousMarket {
                    slug: m.slug.clone(),
                    up_token_id: m.up_token_id.clone(),
                    down_token_id: m.down_token_id.clone(),
                    final_up_price: None,
                    final_down_price: None,
                });
            }
        } else {
            self.cached_market_slug = None;
            self.cached_seconds_remaining = None;
        }

        // Update prices
        self.last_up_price = self.watcher.get_up_price().await;
        self.last_down_price = self.watcher.get_down_price().await;

        // Keep previous_market prices updated for settlement fallback
        if let Some(ref mut prev) = self.previous_market {
            prev.final_up_price = self.last_up_price;
            prev.final_down_price = self.last_down_price;
        }

        // Update order book sizes for UI
        if let Some(book) = self.watcher.get_up_book().await {
            self.cached_up_ask_size = book.ask_size;
        }
        if let Some(book) = self.watcher.get_down_book().await {
            self.cached_down_ask_size = book.ask_size;
        }

        // Update paper trader with current prices
        if self.config.paper_trading.enabled {
            self.paper_trader.update_prices(self.last_up_price, self.last_down_price).await;
            // Update cached values for UI
            self.cached_balance = self.paper_trader.get_balance().await;
            self.cached_pnl = self.paper_trader.get_total_pnl().await;

            // Update cached live position for current round
            if let Some(slug) = &self.cached_market_slug {
                self.cached_live_position = self.paper_trader.get_live_position_summary(slug).await;
            }

            // Update cached history
            self.cached_history = self.paper_trader.get_settled_rounds().await;

            // Retry any pending settlements (e.g., markets that weren't resolved yet)
            // Only retry every 30 seconds to avoid spamming logs
            let should_retry = self.last_settlement_retry
                .map(|t| t.elapsed() >= Duration::from_secs(30))
                .unwrap_or(true);

            if should_retry && !self.pending_settlements.is_empty() {
                self.last_settlement_retry = Some(std::time::Instant::now());
                self.retry_pending_settlements().await;
            }
        }

        // Fetch live trading balance from Polymarket API (every 30 seconds)
        if !self.paper_mode && self.watcher.client().is_authenticated() {
            let should_fetch = self.last_live_balance_fetch
                .map(|t| t.elapsed() >= Duration::from_secs(30))
                .unwrap_or(true);

            if should_fetch {
                self.last_live_balance_fetch = Some(std::time::Instant::now());
                match self.client.get_balance().await {
                    Ok(balance) => {
                        self.cached_live_balance = Some(balance);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch live balance: {}", e);
                    }
                }
            }
        }

        // Record snapshot
        if let Some(snapshot) = self.watcher.get_snapshot().await {
            if let Err(e) = self.recorder.record(&snapshot) {
                tracing::error!("Recording failed: {}", e);
            }
        }

        // Run auto trader
        let paper_trader = if self.paper_mode {
            Some(&self.paper_trader)
        } else {
            None
        };
        if let Err(e) = self.auto_trader.tick(&self.watcher, paper_trader).await {
            self.log(&format!("Strategy error: {}", e));
        }

        // Forward new strategy logs to UI
        let strategy_logs = self.auto_trader.get_logs().await;
        if strategy_logs.len() > self.last_strategy_log_count {
            for log in strategy_logs.iter().skip(self.last_strategy_log_count) {
                // Only show important logs (LEG, COMPLETE, PAPER, LIVE, errors)
                if log.message.contains("LEG")
                    || log.message.contains("COMPLETE")
                    || log.message.contains("[PAPER]")
                    || log.message.contains("[LIVE]")
                    || log.message.contains("Dump")
                    || log.message.contains("Hedge")
                    || log.is_error
                {
                    self.messages.push(format!("[Auto] {}", log.message));
                    // Keep only last 100 messages
                    if self.messages.len() > 100 {
                        self.messages.remove(0);
                    }
                }
            }
            self.last_strategy_log_count = strategy_logs.len();
        }

        Ok(())
    }

    /// Settle positions from a previous round
    /// Returns true if settled, false if deferred
    async fn settle_previous_round(&mut self, prev: PreviousMarket) -> bool {
        // Check if we have any positions for this round
        if !self.paper_trader.has_positions_for_round(&prev.slug).await {
            return true; // Nothing to settle
        }

        // Fetch the actual outcome from Polymarket API
        let winning_side = match self.client.get_market_outcome(&prev.slug).await {
            Ok(Some(side)) => {
                self.log(&format!("Fetched outcome for {}: {} won", prev.slug, side));
                side
            }
            Ok(None) => {
                // Market not yet resolved, defer settlement
                let short_slug = prev.slug.split('-').last().unwrap_or(&prev.slug);
                self.log(&format!("[{}] Waiting for {} to resolve...",
                    Local::now().format("%H:%M:%S"),
                    short_slug
                ));
                return false;
            }
            Err(e) => {
                self.log(&format!("Failed to fetch outcome for {}: {}", prev.slug, e));
                // Fall back to price-based detection (use captured final prices)
                if let (Some(up_price), Some(down_price)) = (prev.final_up_price, prev.final_down_price) {
                    if up_price > down_price {
                        self.log("Falling back to price-based resolution: UP");
                        Side::Up
                    } else {
                        self.log("Falling back to price-based resolution: DOWN");
                        Side::Down
                    }
                } else {
                    self.log("No price data available, cannot settle");
                    return false;
                }
            }
        };

        self.log(&format!(
            "Settling round {} - {} won",
            prev.slug, winning_side
        ));

        match self
            .paper_trader
            .settle_round(&prev.up_token_id, &prev.down_token_id, winning_side, &prev.slug)
            .await
        {
            Ok(pnl) => {
                let pnl_str = if pnl >= Decimal::ZERO {
                    format!("+${:.2}", pnl)
                } else {
                    format!("-${:.2}", pnl.abs())
                };
                self.log(&format!("Round settled: {}", pnl_str));
                true
            }
            Err(e) => {
                self.log(&format!("Settlement error: {}", e));
                false
            }
        }
    }

    /// Try to settle any pending settlements
    async fn retry_pending_settlements(&mut self) {
        if self.pending_settlements.is_empty() {
            return;
        }

        let mut still_pending = Vec::new();
        let pending = std::mem::take(&mut self.pending_settlements);

        for prev in pending {
            if !self.settle_previous_round(prev.clone()).await {
                still_pending.push(prev);
            }
        }

        self.pending_settlements = still_pending;
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
            "buy" => self.handle_buy(&parts).await,
            "buyshares" => self.handle_buyshares(&parts).await,
            "params" => self.handle_params(&parts).await,
            "mode" => self.handle_mode(&parts).await,
            "yes" | "y" => self.confirm_mode_switch().await,
            "no" | "n" => self.cancel_mode_switch(),
            "balance" | "bal" => self.show_balance().await,
            "positions" | "pos" => self.show_positions().await,
            "book" => self.show_order_book().await,
            "trades" => self.show_trades().await,
            "reset" => self.reset_paper_trading().await,
            "quit" | "exit" | "q" => self.should_quit = true,
            _ => self.log(&format!("Unknown command: {}", parts[0])),
        }
    }

    fn show_help(&mut self) {
        self.log("Commands:");
        self.log("  buy up|down <usd>     - Buy shares for USD amount");
        self.log("  buyshares up|down <n> - Buy N shares at best ask");
        self.log("  params [shares] [sum] [move] [window]");
        self.log("                        - Show/update strategy params");
        self.log("  mode paper|live       - Switch trading mode");
        self.log("  book                  - Show order book details");
        self.log("  balance               - Show detailed balance");
        self.log("  positions             - Show all positions by token");
        self.log("  trades                - Show recent trades");
        self.log("  reset                 - Reset paper trading");
        self.log("  quit                  - Exit");
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
            // Execute paper trade - use actual token IDs from market
            let market = self.watcher.get_current_market().await;
            let Some(market) = market else {
                self.log("No active market");
                return;
            };
            let round_slug = market.slug.clone();
            let token_id = match side {
                Side::Up => market.up_token_id.clone(),
                Side::Down => market.down_token_id.clone(),
            };

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
            // Execute paper trade - use actual token IDs from market
            let market = self.watcher.get_current_market().await;
            let Some(market) = market else {
                self.log("No active market");
                return;
            };
            let round_slug = market.slug.clone();
            let token_id = match side {
                Side::Up => market.up_token_id.clone(),
                Side::Down => market.down_token_id.clone(),
            };

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

    async fn handle_params(&mut self, parts: &[&str]) {
        if parts.len() < 2 {
            // Show current params
            let params = self.auto_trader.get_params().await;
            self.log("Current strategy parameters:");
            self.log(&format!("  shares:     {}", params.shares));
            self.log(&format!("  sum:        {} (theta decay to 0.99 in last 5 min)", params.sum_target));
            self.log(&format!("  move:       {}%", params.move_pct * Decimal::ONE_HUNDRED));
            self.log(&format!("  window:     {} min", params.window_min));
            self.log(&format!("  max_cycles: {}", params.max_cycles));
            self.log("");
            self.log("To update: params <shares> [sum] [move] [window] [cycles]");
            return;
        }

        // Update params
        let shares: Decimal = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => {
                self.log("Invalid shares amount");
                return;
            }
        };

        let sum = parts.get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(Decimal::new(95, 2));

        let move_pct = parts.get(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(Decimal::new(15, 2));

        let window = parts.get(4)
            .and_then(|s| s.parse().ok())
            .unwrap_or(2u32);

        let max_cycles = parts.get(5)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1u32);

        let new_params = AutoParams {
            shares,
            sum_target: sum,
            move_pct,
            window_min: window,
            max_cycles,
        };
        self.auto_trader.set_params(new_params).await;

        self.log(&format!(
            "Params updated: {} shares, sum={}, move={}%, window={}min, cycles={}",
            shares, sum, move_pct * Decimal::ONE_HUNDRED, window, max_cycles
        ));
    }

    async fn handle_mode(&mut self, parts: &[&str]) {
        if parts.len() < 2 {
            self.log("Usage: mode paper | mode live");
            return;
        }

        match parts[1].to_lowercase().as_str() {
            "paper" => {
                if self.paper_mode {
                    self.log("Already in PAPER trading mode");
                } else {
                    self.paper_mode = true;
                    self.log("Switched to PAPER trading mode");
                }
            }
            "live" => {
                if !self.paper_mode {
                    self.log("Already in LIVE trading mode");
                } else {
                    // Require confirmation for live trading
                    if !self.watcher.client().is_authenticated() {
                        self.log("Cannot switch to LIVE mode - not authenticated");
                        self.log("Configure private key in config or POLYMARKET_PRIVATE_KEY env var");
                        return;
                    }
                    self.pending_mode_switch = Some(false); // false = switch to live
                    self.log("WARNING: You are about to switch to LIVE trading mode!");
                    self.log("Real orders will be placed with real money.");
                    self.log("Type 'yes' or 'y' to confirm, 'no' or 'n' to cancel");
                }
            }
            _ => {
                self.log("Usage: mode paper | mode live");
            }
        }
    }

    async fn confirm_mode_switch(&mut self) {
        if let Some(to_live) = self.pending_mode_switch.take() {
            if !to_live {
                // Confirming switch to live
                self.paper_mode = false;
                self.log("CONFIRMED: Now in LIVE trading mode");
                self.log("Real orders will be placed!");
            }
        } else {
            self.log("Nothing to confirm");
        }
    }

    fn cancel_mode_switch(&mut self) {
        if self.pending_mode_switch.take().is_some() {
            self.log("Mode switch cancelled");
        } else {
            self.log("Nothing to cancel");
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

    async fn show_trades(&mut self) {
        if !self.config.paper_trading.enabled {
            self.log("Paper trading is disabled");
            return;
        }

        let trades = self.paper_trader.get_recent_trades(20).await;
        if trades.is_empty() {
            self.log("No trades yet");
        } else {
            self.log("Recent trades (grouped by round):");

            // Group trades by round_slug
            let mut by_round: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
            for trade in trades {
                by_round.entry(trade.round_slug.clone()).or_default().push(trade);
            }

            // Sort rounds by most recent trade timestamp
            let mut rounds: Vec<_> = by_round.into_iter().collect();
            rounds.sort_by(|a, b| {
                let a_time = a.1.iter().map(|t| t.timestamp).max();
                let b_time = b.1.iter().map(|t| t.timestamp).max();
                b_time.cmp(&a_time) // Most recent first
            });

            for (round_slug, round_trades) in rounds.iter().take(5) {
                // Extract just the timestamp part from slug for brevity
                let short_slug = round_slug.split('-').last().unwrap_or(round_slug);

                // Calculate totals for this round
                let mut up_shares = Decimal::ZERO;
                let mut down_shares = Decimal::ZERO;
                let mut total_cost = Decimal::ZERO;

                for trade in round_trades {
                    match trade.side {
                        crate::types::Side::Up => up_shares += trade.shares,
                        crate::types::Side::Down => down_shares += trade.shares,
                    }
                    total_cost += trade.total_cost;
                }

                // Show round header with summary
                let balance_str = if up_shares == down_shares && up_shares > Decimal::ZERO {
                    "BALANCED".to_string()
                } else if up_shares > Decimal::ZERO && down_shares > Decimal::ZERO {
                    format!("IMBALANCED: {}U/{}D", up_shares, down_shares)
                } else {
                    format!("{}U/{}D", up_shares, down_shares)
                };

                self.log(&format!("  [{}] ${:.2} - {}", short_slug, total_cost, balance_str));

                // Show individual trades
                for trade in round_trades {
                    let action = match trade.buy_sell {
                        crate::types::BuySell::Buy => "BUY",
                        crate::types::BuySell::Sell => "SELL",
                    };
                    self.log(&format!(
                        "    {} {} {} @ ${:.4}",
                        action,
                        trade.shares,
                        trade.side,
                        trade.price
                    ));
                }
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

    async fn show_order_book(&mut self) {
        let up_book = self.watcher.get_up_book().await;
        let down_book = self.watcher.get_down_book().await;

        self.log("Order Book:");

        if let Some(book) = up_book {
            let ask_str = book.best_ask.map(|p| format!("${:.4}", p)).unwrap_or_else(|| "---".to_string());
            let size_str = book.ask_size.map(|s| format!("{:.2}", s)).unwrap_or_else(|| "---".to_string());
            self.log(&format!("  UP:   {} x {} (ask)", ask_str, size_str));
        } else {
            self.log("  UP:   No data");
        }

        if let Some(book) = down_book {
            let ask_str = book.best_ask.map(|p| format!("${:.4}", p)).unwrap_or_else(|| "---".to_string());
            let size_str = book.ask_size.map(|s| format!("{:.2}", s)).unwrap_or_else(|| "---".to_string());
            self.log(&format!("  DOWN: {} x {} (ask)", ask_str, size_str));
        } else {
            self.log("  DOWN: No data");
        }

        // Show sum
        if let (Some(up_price), Some(down_price)) = (self.last_up_price, self.last_down_price) {
            let sum = up_price + down_price;
            let arb_status = if sum < Decimal::ONE {
                format!("ARB: {:.2}% profit potential", (Decimal::ONE - sum) * Decimal::ONE_HUNDRED)
            } else {
                "No arbitrage".to_string()
            };
            self.log(&format!("  SUM: ${:.4} - {}", sum, arb_status));
        }
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

    pub fn is_paper_mode(&self) -> bool {
        self.paper_mode
    }

    pub fn market_slug(&self) -> Option<String> {
        self.cached_market_slug.clone()
    }

    pub fn seconds_remaining(&self) -> Option<i64> {
        self.cached_seconds_remaining
    }

    pub fn is_paper_trading(&self) -> bool {
        self.config.paper_trading.enabled && self.paper_mode
    }

    /// Get current trading balance (paper or live)
    pub fn trading_balance(&self) -> Decimal {
        if self.paper_mode {
            self.cached_balance
        } else {
            // Live trading - use cached balance from API
            self.cached_live_balance.unwrap_or(Decimal::ZERO)
        }
    }

    /// Get realized P&L (paper only - live P&L requires session tracking)
    pub fn trading_pnl(&self) -> Decimal {
        if self.paper_mode {
            self.cached_pnl
        } else {
            // Live trading - P&L requires tracking session start balance
            // For now, show 0 since we don't track session start
            Decimal::ZERO
        }
    }

    pub fn up_ask_size(&self) -> Option<Decimal> {
        self.cached_up_ask_size
    }

    pub fn down_ask_size(&self) -> Option<Decimal> {
        self.cached_down_ask_size
    }

    pub fn live_position(&self) -> &LivePositionSummary {
        &self.cached_live_position
    }

    pub fn settled_history(&self) -> &[SettledRound] {
        &self.cached_history
    }
}
