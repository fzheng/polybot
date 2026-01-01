//! Auto trading strategy - Two-leg arbitrage

#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::market::MarketWatcher;
use crate::paper::PaperTrader;
use crate::types::{AutoParams, Side, StrategyState, TradeResult};

/// Log entry for strategy actions
#[derive(Debug, Clone)]
pub struct StrategyLog {
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub is_error: bool,
}

/// Leg 1 trade info for tracking
#[derive(Debug, Clone)]
pub struct Leg1Info {
    pub side: Side,
    pub price: Decimal,
    pub shares: Decimal,
    pub token_id: String,
}

/// Auto trader implementing the two-leg strategy
pub struct AutoTrader {
    params: Arc<RwLock<AutoParams>>,
    state: Arc<RwLock<StrategyState>>,
    current_round: Arc<RwLock<Option<String>>>,
    /// Count of cycles completed for the current round (compared against max_cycles)
    cycles_this_round: Arc<RwLock<u32>>,
    leg1_info: Arc<RwLock<Option<Leg1Info>>>,
    leg1_result: Arc<RwLock<Option<TradeResult>>>,
    leg2_result: Arc<RwLock<Option<TradeResult>>>,
    logs: Arc<RwLock<Vec<StrategyLog>>>,
    total_profit: Arc<RwLock<Decimal>>,
    cycles_completed: Arc<RwLock<u32>>,
}

impl AutoTrader {
    /// Create a new auto trader
    pub fn new(params: AutoParams) -> Self {
        Self {
            params: Arc::new(RwLock::new(params)),
            state: Arc::new(RwLock::new(StrategyState::Watching)),
            current_round: Arc::new(RwLock::new(None)),
            cycles_this_round: Arc::new(RwLock::new(0)),
            leg1_info: Arc::new(RwLock::new(None)),
            leg1_result: Arc::new(RwLock::new(None)),
            leg2_result: Arc::new(RwLock::new(None)),
            logs: Arc::new(RwLock::new(Vec::new())),
            total_profit: Arc::new(RwLock::new(Decimal::ZERO)),
            cycles_completed: Arc::new(RwLock::new(0)),
        }
    }

    /// Update parameters
    pub async fn set_params(&self, params: AutoParams) {
        let msg = format!(
            "Params updated: shares={}, sum={}, move={}%, window={}min, cycles={}",
            params.shares, params.sum_target,
            params.move_pct * Decimal::ONE_HUNDRED, params.window_min, params.max_cycles
        );
        let mut p = self.params.write().await;
        *p = params;
        drop(p);
        self.log(&msg).await;
    }

    /// Get current state
    pub async fn get_state(&self) -> StrategyState {
        *self.state.read().await
    }

    /// Get current parameters
    pub async fn get_params(&self) -> AutoParams {
        self.params.read().await.clone()
    }

    /// Get logs
    pub async fn get_logs(&self) -> Vec<StrategyLog> {
        self.logs.read().await.clone()
    }

    /// Get total profit
    pub async fn get_total_profit(&self) -> Decimal {
        *self.total_profit.read().await
    }

    /// Get cycles completed
    pub async fn get_cycles_completed(&self) -> u32 {
        *self.cycles_completed.read().await
    }

    /// Process a tick - called regularly to update strategy
    /// paper_trader: Used for paper trading mode, None for live trading
    pub async fn tick(&self, watcher: &MarketWatcher, paper_trader: Option<&PaperTrader>) -> Result<()> {
        // Check if round changed
        let current_market = watcher.get_current_market().await;
        let current_slug = current_market.as_ref().map(|m| m.slug.clone());

        {
            let mut round = self.current_round.write().await;
            if *round != current_slug {
                // Round changed - reset cycle counter for new round
                let mut cycles = self.cycles_this_round.write().await;
                *cycles = 0;

                // Abandon current cycle if in progress
                if round.is_some() && self.get_state().await != StrategyState::Watching {
                    self.log("Round changed - abandoning cycle").await;
                    if let Some(pt) = paper_trader {
                        if let Some(slug) = round.as_ref() {
                            pt.abandon_cycle(slug).await;
                        }
                    }
                    self.reset_cycle().await;
                }
                *round = current_slug.clone();
            }
        }

        // Check if we've reached max cycles for this round
        let params = self.params.read().await;
        let max_cycles = params.max_cycles;
        drop(params);

        let cycles = *self.cycles_this_round.read().await;
        if cycles >= max_cycles {
            return Ok(()); // Don't start another cycle in the same round
        }

        // Process based on current state
        let state = self.get_state().await;

        match state {
            StrategyState::Watching => {
                self.watch_for_dump(watcher, paper_trader).await?;
            }
            StrategyState::WaitingForHedge { leg1_side, leg1_price } => {
                self.watch_for_hedge(watcher, paper_trader, leg1_side, leg1_price).await?;
            }
            StrategyState::Completed => {
                // Increment cycle counter and reset state
                let mut cycles = self.cycles_this_round.write().await;
                *cycles += 1;
                let current = *cycles;
                drop(cycles);

                self.reset_cycle().await;
                self.log(&format!("Cycle {} complete - {} for next round", current,
                    if current >= max_cycles { "waiting" } else { "ready" })).await;
            }
        }

        Ok(())
    }

    /// Watch for price dump during window
    async fn watch_for_dump(&self, watcher: &MarketWatcher, paper_trader: Option<&PaperTrader>) -> Result<()> {
        let params = self.params.read().await;

        // Check if we're within the window
        if !watcher.is_within_window(params.window_min).await {
            return Ok(()); // Outside window, just wait
        }

        // Check for dump
        let dump = watcher.detect_dump(3, params.move_pct).await; // 3 second window
        drop(params);

        if let Some((side, pct)) = dump {
            self.log(&format!(
                "Dump detected! {} dropped {}%",
                side,
                (pct * Decimal::ONE_HUNDRED).round_dp(2)
            )).await;

            // Execute Leg 1
            if let Err(e) = self.execute_leg1(watcher, paper_trader, side).await {
                self.log_error(&format!("Leg 1 failed: {}", e)).await;
            }
        }

        Ok(())
    }

    /// Execute Leg 1 - buy the side that dumped
    async fn execute_leg1(&self, watcher: &MarketWatcher, paper_trader: Option<&PaperTrader>, side: Side) -> Result<()> {
        let market = watcher.get_current_market().await
            .ok_or_else(|| anyhow::anyhow!("No active market"))?;

        let token_id = match side {
            Side::Up => &market.up_token_id,
            Side::Down => &market.down_token_id,
        };

        // Get current price
        let price = match side {
            Side::Up => watcher.get_up_price().await,
            Side::Down => watcher.get_down_price().await,
        }.ok_or_else(|| anyhow::anyhow!("No price available"))?;

        let params = self.params.read().await;
        let shares = params.shares;
        drop(params);

        self.log(&format!(
            "LEG 1: Buying {} {} shares at ${:.4}",
            shares, side, price
        )).await;

        // Execute trade based on mode
        if let Some(pt) = paper_trader {
            // Paper trading mode
            let result = pt.buy(token_id, side, shares, price, &market.slug).await?;
            let mut leg1 = self.leg1_result.write().await;
            *leg1 = Some(result);
            self.log("[PAPER] Leg 1 executed").await;
        } else if watcher.client().is_authenticated() {
            // Live trading mode - check balance first
            // TODO: Add balance check via API
            let result = watcher.client()
                .buy_at_ask(token_id, shares)
                .await?;

            let mut leg1 = self.leg1_result.write().await;
            *leg1 = Some(result);
            self.log("[LIVE] Leg 1 executed").await;
        } else {
            self.log_error("Not authenticated for live trading and no paper trader").await;
            return Ok(());
        }

        // Store leg1 info for profit calculation
        {
            let mut leg1_info = self.leg1_info.write().await;
            *leg1_info = Some(Leg1Info {
                side,
                price,
                shares,
                token_id: token_id.clone(),
            });
        }

        // Update state
        let mut state = self.state.write().await;
        *state = StrategyState::WaitingForHedge {
            leg1_side: side,
            leg1_price: price,
        };

        Ok(())
    }

    /// Watch for hedge opportunity
    async fn watch_for_hedge(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        leg1_side: Side,
        leg1_price: Decimal,
    ) -> Result<()> {
        let opposite_side = leg1_side.opposite();

        let opposite_ask = match opposite_side {
            Side::Up => watcher.get_up_price().await,
            Side::Down => watcher.get_down_price().await,
        };

        if let Some(ask) = opposite_ask {
            let sum = leg1_price + ask;

            let params = self.params.read().await;
            let sum_target = params.sum_target;
            drop(params);

            // Calculate time-decayed sum target (options-like theta decay)
            // Stays strict for first 10 min, then decays toward 0.99 in last 5 min
            let effective_sum_target = self.calculate_decayed_target(
                watcher,
                sum_target,
            ).await;

            // Check hedge condition
            if sum <= effective_sum_target {
                self.log(&format!(
                    "Hedge condition met! {:.4} + {:.4} = {:.4} <= {:.4}",
                    leg1_price, ask, sum, effective_sum_target
                )).await;

                if let Err(e) = self.execute_leg2(watcher, paper_trader, opposite_side, ask).await {
                    self.log_error(&format!("Leg 2 failed: {}", e)).await;
                }
            }
        }

        Ok(())
    }

    /// Execute Leg 2 - hedge by buying opposite side
    async fn execute_leg2(&self, watcher: &MarketWatcher, paper_trader: Option<&PaperTrader>, side: Side, price: Decimal) -> Result<()> {
        let market = watcher.get_current_market().await
            .ok_or_else(|| anyhow::anyhow!("No active market"))?;

        let token_id = match side {
            Side::Up => &market.up_token_id,
            Side::Down => &market.down_token_id,
        };

        let params = self.params.read().await;
        let shares = params.shares;
        drop(params);

        self.log(&format!(
            "LEG 2: Buying {} {} shares at ${:.4}",
            shares, side, price
        )).await;

        // Execute trade based on mode
        if let Some(pt) = paper_trader {
            // Paper trading mode
            let result = pt.buy(token_id, side, shares, price, &market.slug).await?;
            let mut leg2 = self.leg2_result.write().await;
            *leg2 = Some(result);
            self.log("[PAPER] Leg 2 executed").await;
        } else if watcher.client().is_authenticated() {
            // Live trading mode
            let result = watcher.client()
                .buy_at_ask(token_id, shares)
                .await?;

            let mut leg2 = self.leg2_result.write().await;
            *leg2 = Some(result);
            self.log("[LIVE] Leg 2 executed").await;
        } else {
            self.log_error("Not authenticated for live trading and no paper trader").await;
            return Ok(());
        }

        // Calculate profit using stored leg1 info
        let leg1_info = self.leg1_info.read().await;
        if let Some(l1) = leg1_info.as_ref() {
            let total_cost = l1.price * l1.shares + price * shares;
            let payout = shares; // $1 per share pair (assuming equal shares)
            let profit = payout - total_cost;
            let profit_pct = if total_cost > Decimal::ZERO {
                (profit / total_cost) * Decimal::ONE_HUNDRED
            } else {
                Decimal::ZERO
            };

            let mode = if paper_trader.is_some() { "PAPER" } else { "LIVE" };
            self.log(&format!(
                "[{}] CYCLE COMPLETE! Cost: ${:.4}, Payout: ${:.4}, Profit: ${:.4} ({:.2}%)",
                mode, total_cost, payout, profit, profit_pct
            )).await;

            // Update totals
            let mut total = self.total_profit.write().await;
            *total += profit;

            let mut cycles = self.cycles_completed.write().await;
            *cycles += 1;
        }

        // Update state
        let mut state = self.state.write().await;
        *state = StrategyState::Completed;

        Ok(())
    }

    /// Reset cycle state
    async fn reset_cycle(&self) {
        let mut state = self.state.write().await;
        *state = StrategyState::Watching;

        let mut leg1_info = self.leg1_info.write().await;
        *leg1_info = None;

        let mut leg1 = self.leg1_result.write().await;
        *leg1 = None;

        let mut leg2 = self.leg2_result.write().await;
        *leg2 = None;
    }

    /// Calculate time-decayed sum target based on time remaining in round.
    ///
    /// Uses options-like theta decay: stays flat for most of the round,
    /// then decays sharply in the final minutes (similar to how options
    /// lose time value exponentially as expiration approaches).
    ///
    /// - First 10 minutes: No decay, uses strict sum_target
    /// - Last 5 minutes: Exponential decay from sum_target toward 0.99
    ///
    /// The decay follows: target = sum_target + (0.99 - sum_target) * (1 - e^(-k*progress))
    /// where progress goes from 0 to 1 over the last 5 minutes.
    async fn calculate_decayed_target(
        &self,
        watcher: &MarketWatcher,
        sum_target: Decimal,
    ) -> Decimal {
        const DECAY_WINDOW_SECS: i64 = 300; // Last 5 minutes
        const MAX_TARGET: &str = "0.99"; // Never exceed this (1% minimum profit)

        let seconds_remaining = watcher.get_seconds_remaining().await.unwrap_or(900);

        if seconds_remaining >= DECAY_WINDOW_SECS {
            // Still plenty of time - use strict target (no decay)
            return sum_target;
        }

        if seconds_remaining <= 0 {
            // Round ended
            return Decimal::from_str_exact(MAX_TARGET).unwrap();
        }

        // Exponential decay similar to options theta
        // progress: 0.0 at 300s remaining, 1.0 at 0s remaining
        let progress = (DECAY_WINDOW_SECS - seconds_remaining) as f64 / DECAY_WINDOW_SECS as f64;

        // Decay constant - higher = faster decay. 3.0 gives ~95% decay at expiry
        let k = 3.0_f64;
        let decay_factor = 1.0 - (-k * progress).exp();

        let max_target = Decimal::from_str_exact(MAX_TARGET).unwrap();
        let range = max_target - sum_target;

        // Convert decay_factor to Decimal (multiply by 1000, divide later for precision)
        let decay_factor_decimal = Decimal::from_f64_retain(decay_factor)
            .unwrap_or(Decimal::ZERO);

        sum_target + (range * decay_factor_decimal)
    }

    /// Add a log entry
    async fn log(&self, message: &str) {
        let mut logs = self.logs.write().await;
        logs.push(StrategyLog {
            timestamp: Utc::now(),
            message: message.to_string(),
            is_error: false,
        });

        // Keep only last 100 logs
        if logs.len() > 100 {
            logs.remove(0);
        }

        tracing::info!("[Strategy] {}", message);
    }

    /// Add an error log entry
    async fn log_error(&self, message: &str) {
        let mut logs = self.logs.write().await;
        logs.push(StrategyLog {
            timestamp: Utc::now(),
            message: message.to_string(),
            is_error: true,
        });

        tracing::error!("[Strategy] {}", message);
    }
}
