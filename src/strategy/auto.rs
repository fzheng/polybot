//! Auto trading strategy - Two-leg arbitrage with depth-aware sizing.
//!
//! This module implements the core arbitrage strategy for Polymarket BTC 15-minute
//! UP/DOWN prediction markets. The strategy exploits temporary price inefficiencies
//! where UP + DOWN ask prices sum to less than $1.00.
//!
//! # Strategy Overview
//!
//! The strategy operates in two legs:
//!
//! 1. **Leg 1 (Dump Detection)**: When a price drops significantly (configurable %),
//!    buy that side at the lower price.
//!
//! 2. **Leg 2 (Hedge)**: When the opposite side's price drops enough that
//!    UP + DOWN ≤ sum_target (e.g., $0.95), buy the opposite side to lock in profit.
//!
//! # Depth-Aware Sizing
//!
//! To handle thin order books, the strategy implements depth-aware sizing:
//!
//! - **Leg 1**: If available depth < desired shares, reduce order to match depth.
//!   This accepts smaller profit on fewer shares rather than risking slippage.
//!
//! - **Leg 2**: Uses iterative hedging with imbalance tracking. If depth is limited,
//!   fills what's available, tracks remaining imbalance, and retries on subsequent
//!   ticks until fully hedged.
//!
//! # Time Decay (Theta Decay)
//!
//! Near round end (last 5 minutes), the sum_target relaxes from strict (0.95) toward
//! 0.99, using an options-like theta decay curve. This allows completing partially
//! hedged positions even if perfect arbitrage isn't available.
//!
//! # Cycle Limiting
//!
//! The `max_cycles` parameter limits how many complete arbitrage cycles can execute
//! per 15-minute round, preventing overtrading in volatile conditions.

#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::live::LiveTrader;
use crate::market::MarketWatcher;
use crate::paper::PaperTrader;
use crate::types::{AutoParams, BuySell, Side, StrategyState, TradeResult, TradeStatus};

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
    /// Remaining imbalance from partial Leg 2 fills (shares needing hedge)
    leg2_imbalance: Arc<RwLock<Decimal>>,
    /// Flag indicating the trader wants to switch to the next market
    /// Set after completing a cycle when there's time remaining for more opportunities
    wants_next_market: Arc<RwLock<bool>>,
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
            leg2_imbalance: Arc::new(RwLock::new(Decimal::ZERO)),
            wants_next_market: Arc::new(RwLock::new(false)),
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

    /// Check if the trader wants to switch to the next market
    /// This is set after completing a cycle when there's time for more opportunities
    pub async fn wants_next_market(&self) -> bool {
        *self.wants_next_market.read().await
    }

    /// Acknowledge that we've switched to the next market
    /// Called by the app after successfully switching markets
    pub async fn acknowledge_market_switch(&self) {
        let mut wants = self.wants_next_market.write().await;
        *wants = false;
    }

    /// Process a tick - called regularly to update strategy
    /// paper_trader: Used for paper trading mode, None for live trading
    /// live_trader: Used for live trading mode via SDK, None for paper trading
    pub async fn tick(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        live_trader: Option<&LiveTrader>,
    ) -> Result<()> {
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
                self.watch_for_dump(watcher, paper_trader, live_trader).await?;
            }
            StrategyState::WaitingForHedge { leg1_side, leg1_price } => {
                self.watch_for_hedge(watcher, paper_trader, live_trader, leg1_side, leg1_price).await?;
            }
            StrategyState::Completed => {
                // Increment cycle counter and reset state
                let mut cycles = self.cycles_this_round.write().await;
                *cycles += 1;
                let current = *cycles;
                drop(cycles);

                self.reset_cycle().await;

                // Check if we've reached max cycles for this round
                if current >= max_cycles {
                    // Signal that we want to switch to next market for more opportunities
                    let mut wants = self.wants_next_market.write().await;
                    *wants = true;
                    drop(wants);
                    self.log(&format!(
                        "Cycle {} complete - requesting switch to next market",
                        current
                    )).await;
                } else {
                    self.log(&format!("Cycle {} complete - ready for more", current)).await;
                }
            }
        }

        Ok(())
    }

    /// Watch for price dump during window
    async fn watch_for_dump(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        live_trader: Option<&LiveTrader>,
    ) -> Result<()> {
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
            if let Err(e) = self.execute_leg1(watcher, paper_trader, live_trader, side).await {
                self.log_error(&format!("Leg 1 failed: {}", e)).await;
            }
        }

        Ok(())
    }

    /// Execute Leg 1 - buy the side that dumped
    /// Implements depth-aware sizing: reduces order to available depth if needed
    async fn execute_leg1(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        live_trader: Option<&LiveTrader>,
        side: Side,
    ) -> Result<()> {
        let market = watcher.get_current_market().await
            .ok_or_else(|| anyhow::anyhow!("No active market"))?;

        let token_id = match side {
            Side::Up => &market.up_token_id,
            Side::Down => &market.down_token_id,
        };

        // Get current price and order book
        let (price, available_depth) = match side {
            Side::Up => {
                let book = watcher.get_up_book().await;
                let price = book.as_ref().and_then(|b| b.best_ask);
                let depth = book.as_ref().and_then(|b| b.ask_size);
                (price, depth)
            }
            Side::Down => {
                let book = watcher.get_down_book().await;
                let price = book.as_ref().and_then(|b| b.best_ask);
                let depth = book.as_ref().and_then(|b| b.ask_size);
                (price, depth)
            }
        };

        let price = price.ok_or_else(|| anyhow::anyhow!("No price available"))?;

        let params = self.params.read().await;
        let desired_shares = params.shares;
        drop(params);

        // Depth-aware sizing: reduce to available depth if needed
        let shares = if let Some(depth) = available_depth {
            if depth < desired_shares {
                self.log(&format!(
                    "Depth limited: want {} shares but only {} available at best ask",
                    desired_shares, depth
                )).await;
                depth
            } else {
                desired_shares
            }
        } else {
            // No depth info available, proceed with full order (will rely on FOK rejection)
            desired_shares
        };

        if shares <= Decimal::ZERO {
            self.log("No depth available for Leg 1 - skipping").await;
            return Ok(());
        }

        self.log(&format!(
            "LEG 1: Buying {} {} shares at ${:.4}",
            shares, side, price
        )).await;

        // Execute trade based on mode
        let actual_filled: Decimal;
        if let Some(pt) = paper_trader {
            // Paper trading mode
            let result = pt.buy(token_id, side, shares, price, &market.slug).await?;
            actual_filled = result.filled_size;
            let mut leg1 = self.leg1_result.write().await;
            *leg1 = Some(result);
            self.log("[PAPER] Leg 1 executed").await;
        } else if let Some(lt) = live_trader {
            // Live trading mode via SDK - use FOK for immediate fill or reject
            let live_result = lt.buy_fok(token_id, shares, price).await?;

            if !live_result.filled {
                if let Some(err) = &live_result.error {
                    self.log_error(&format!("[LIVE] Leg 1 FOK rejected: {}", err)).await;
                } else {
                    self.log_error("[LIVE] Leg 1 FOK rejected (no fill)").await;
                }
                return Ok(());
            }

            actual_filled = live_result.filled_size;

            // Convert to TradeResult for consistency
            let result = TradeResult {
                order_id: live_result.order_id,
                token_id: token_id.clone(),
                side: BuySell::Buy,
                price: live_result.price,
                size: live_result.requested_size,
                filled_size: live_result.filled_size,
                status: if live_result.filled { TradeStatus::Filled } else { TradeStatus::Failed },
                timestamp: Utc::now(),
            };

            let mut leg1 = self.leg1_result.write().await;
            *leg1 = Some(result);
            self.log(&format!("[LIVE] Leg 1 executed: {} shares filled", actual_filled)).await;
        } else {
            self.log_error("Not authenticated for live trading and no paper trader").await;
            return Ok(());
        }

        // Store leg1 info for profit calculation (using actual filled shares)
        {
            let mut leg1_info = self.leg1_info.write().await;
            *leg1_info = Some(Leg1Info {
                side,
                price,
                shares: actual_filled,
                token_id: token_id.clone(),
            });
        }

        // Initialize imbalance tracking (need to hedge all Leg 1 shares)
        {
            let mut imbalance = self.leg2_imbalance.write().await;
            *imbalance = actual_filled;
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
    /// Implements iterative hedging: if partial fill, tracks imbalance and retries
    async fn watch_for_hedge(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        live_trader: Option<&LiveTrader>,
        leg1_side: Side,
        leg1_price: Decimal,
    ) -> Result<()> {
        let opposite_side = leg1_side.opposite();

        // Get current imbalance (remaining shares to hedge)
        let remaining_imbalance = *self.leg2_imbalance.read().await;
        if remaining_imbalance <= Decimal::ZERO {
            // Already fully hedged, complete the cycle
            let mut state = self.state.write().await;
            *state = StrategyState::Completed;
            return Ok(());
        }

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
                    "Hedge condition met! {:.4} + {:.4} = {:.4} <= {:.4} (imbalance: {} shares)",
                    leg1_price, ask, sum, effective_sum_target, remaining_imbalance
                )).await;

                if let Err(e) = self.execute_leg2(watcher, paper_trader, live_trader, opposite_side, ask, remaining_imbalance).await {
                    self.log_error(&format!("Leg 2 failed: {}", e)).await;
                }
            }
        }

        Ok(())
    }

    /// Execute Leg 2 - hedge by buying opposite side
    /// Implements depth-aware sizing and iterative hedging:
    /// - If depth < desired shares, fill what's available
    /// - Track remaining imbalance for next iteration
    /// - Complete cycle when fully hedged
    async fn execute_leg2(
        &self,
        watcher: &MarketWatcher,
        paper_trader: Option<&PaperTrader>,
        live_trader: Option<&LiveTrader>,
        side: Side,
        price: Decimal,
        desired_shares: Decimal,
    ) -> Result<()> {
        let market = watcher.get_current_market().await
            .ok_or_else(|| anyhow::anyhow!("No active market"))?;

        let token_id = match side {
            Side::Up => &market.up_token_id,
            Side::Down => &market.down_token_id,
        };

        // Get available depth at this price level
        let available_depth = match side {
            Side::Up => watcher.get_up_book().await.and_then(|b| b.ask_size),
            Side::Down => watcher.get_down_book().await.and_then(|b| b.ask_size),
        };

        // Depth-aware sizing: reduce to available depth if needed
        let shares = if let Some(depth) = available_depth {
            if depth < desired_shares {
                self.log(&format!(
                    "Leg 2 depth limited: want {} shares but only {} available",
                    desired_shares, depth
                )).await;
                depth
            } else {
                desired_shares
            }
        } else {
            // No depth info, proceed with full order
            desired_shares
        };

        if shares <= Decimal::ZERO {
            self.log("No depth available for Leg 2 - will retry next tick").await;
            return Ok(());
        }

        self.log(&format!(
            "LEG 2: Buying {} {} shares at ${:.4}",
            shares, side, price
        )).await;

        // Execute trade based on mode
        let actual_filled: Decimal;
        if let Some(pt) = paper_trader {
            // Paper trading mode
            let result = pt.buy(token_id, side, shares, price, &market.slug).await?;
            actual_filled = result.filled_size;
            let mut leg2 = self.leg2_result.write().await;
            *leg2 = Some(result);
            self.log(&format!("[PAPER] Leg 2 filled: {} shares", actual_filled)).await;
        } else if let Some(lt) = live_trader {
            // Live trading mode via SDK - use FOK for immediate fill or reject
            let live_result = lt.buy_fok(token_id, shares, price).await?;

            if !live_result.filled {
                if let Some(err) = &live_result.error {
                    self.log_error(&format!("[LIVE] Leg 2 FOK rejected: {}", err)).await;
                } else {
                    self.log("[LIVE] Leg 2 FOK not filled - will retry next tick").await;
                }
                return Ok(()); // Will retry next tick
            }

            actual_filled = live_result.filled_size;

            // Convert to TradeResult for consistency
            let result = TradeResult {
                order_id: live_result.order_id,
                token_id: token_id.clone(),
                side: BuySell::Buy,
                price: live_result.price,
                size: live_result.requested_size,
                filled_size: live_result.filled_size,
                status: if live_result.filled { TradeStatus::Filled } else { TradeStatus::Failed },
                timestamp: Utc::now(),
            };

            let mut leg2 = self.leg2_result.write().await;
            *leg2 = Some(result);
            self.log(&format!("[LIVE] Leg 2 filled: {} shares", actual_filled)).await;
        } else {
            self.log_error("Not authenticated for live trading and no paper trader").await;
            return Ok(());
        }

        // Update imbalance tracking
        let remaining_imbalance = {
            let mut imbalance = self.leg2_imbalance.write().await;
            *imbalance -= actual_filled;
            *imbalance
        };

        // Calculate profit using stored leg1 info
        let leg1_info = self.leg1_info.read().await;
        if let Some(l1) = leg1_info.as_ref() {
            // Cost calculation based on actual filled amounts
            let leg2_cost = price * actual_filled;
            let total_cost = l1.price * l1.shares + leg2_cost;

            // For partial hedges, we can still calculate per-share profit
            let hedged_pairs = actual_filled.min(l1.shares);
            let payout = hedged_pairs; // $1 per share pair
            let profit = payout - (l1.price * hedged_pairs + price * hedged_pairs);
            let profit_pct = if total_cost > Decimal::ZERO {
                (profit / (l1.price * hedged_pairs + price * hedged_pairs)) * Decimal::ONE_HUNDRED
            } else {
                Decimal::ZERO
            };

            let mode = if paper_trader.is_some() { "PAPER" } else { "LIVE" };

            if remaining_imbalance <= Decimal::ZERO {
                // Fully hedged - cycle complete
                self.log(&format!(
                    "[{}] CYCLE COMPLETE! Cost: ${:.4}, Payout: ${:.4}, Profit: ${:.4} ({:.2}%)",
                    mode, total_cost, payout, profit, profit_pct
                )).await;

                // Update totals
                let mut total = self.total_profit.write().await;
                *total += profit;

                let mut cycles = self.cycles_completed.write().await;
                *cycles += 1;

                // Update state to completed
                let mut state = self.state.write().await;
                *state = StrategyState::Completed;
            } else {
                // Partial hedge - log progress and continue
                self.log(&format!(
                    "[{}] Partial hedge: {} shares filled, {} imbalance remaining",
                    mode, actual_filled, remaining_imbalance
                )).await;

                // Track partial profit
                let mut total = self.total_profit.write().await;
                *total += profit;
            }
        }

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

        let mut imbalance = self.leg2_imbalance.write().await;
        *imbalance = Decimal::ZERO;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================================================
    // AutoTrader Creation & State Tests
    // ============================================================================

    #[tokio::test]
    async fn test_auto_trader_new() {
        let params = AutoParams::default();
        let trader = AutoTrader::new(params.clone());

        assert_eq!(trader.get_state().await, StrategyState::Watching);
        assert_eq!(trader.get_total_profit().await, Decimal::ZERO);
        assert_eq!(trader.get_cycles_completed().await, 0);

        let retrieved_params = trader.get_params().await;
        assert_eq!(retrieved_params.shares, params.shares);
        assert_eq!(retrieved_params.sum_target, params.sum_target);
    }

    #[tokio::test]
    async fn test_auto_trader_set_params() {
        let trader = AutoTrader::new(AutoParams::default());

        let new_params = AutoParams {
            shares: dec!(20),
            sum_target: dec!(0.92),
            move_pct: dec!(0.10),
            window_min: 4,
            max_cycles: 2,
        };

        trader.set_params(new_params.clone()).await;

        let retrieved = trader.get_params().await;
        assert_eq!(retrieved.shares, dec!(20));
        assert_eq!(retrieved.sum_target, dec!(0.92));
        assert_eq!(retrieved.move_pct, dec!(0.10));
        assert_eq!(retrieved.window_min, 4);
        assert_eq!(retrieved.max_cycles, 2);
    }

    #[tokio::test]
    async fn test_auto_trader_logs() {
        let trader = AutoTrader::new(AutoParams::default());

        // Initially empty
        let logs = trader.get_logs().await;
        assert!(logs.is_empty());

        // After setting params, should have a log entry
        trader.set_params(AutoParams::default()).await;
        let logs = trader.get_logs().await;
        assert_eq!(logs.len(), 1);
        assert!(logs[0].message.contains("Params updated"));
        assert!(!logs[0].is_error);
    }

    #[tokio::test]
    async fn test_auto_trader_reset_cycle() {
        let trader = AutoTrader::new(AutoParams::default());

        // Manually set state to WaitingForHedge
        {
            let mut state = trader.state.write().await;
            *state = StrategyState::WaitingForHedge {
                leg1_side: Side::Up,
                leg1_price: dec!(0.40),
            };

            let mut leg1_info = trader.leg1_info.write().await;
            *leg1_info = Some(Leg1Info {
                side: Side::Up,
                price: dec!(0.40),
                shares: dec!(10),
                token_id: "test-token".to_string(),
            });
        }

        // Reset the cycle
        trader.reset_cycle().await;

        // Verify state is back to Watching
        assert_eq!(trader.get_state().await, StrategyState::Watching);

        // Verify leg1_info is cleared
        let leg1_info = trader.leg1_info.read().await;
        assert!(leg1_info.is_none());
    }

    // ============================================================================
    // Leg1Info Tests
    // ============================================================================

    #[test]
    fn test_leg1_info() {
        let info = Leg1Info {
            side: Side::Down,
            price: dec!(0.35),
            shares: dec!(10),
            token_id: "down-token-123".to_string(),
        };

        assert_eq!(info.side, Side::Down);
        assert_eq!(info.price, dec!(0.35));
        assert_eq!(info.shares, dec!(10));
        assert_eq!(info.token_id, "down-token-123");
    }

    // ============================================================================
    // StrategyLog Tests
    // ============================================================================

    #[test]
    fn test_strategy_log() {
        let log = StrategyLog {
            timestamp: Utc::now(),
            message: "Test message".to_string(),
            is_error: false,
        };

        assert_eq!(log.message, "Test message");
        assert!(!log.is_error);
    }

    #[test]
    fn test_strategy_log_error() {
        let log = StrategyLog {
            timestamp: Utc::now(),
            message: "Error occurred".to_string(),
            is_error: true,
        };

        assert!(log.is_error);
    }

    // ============================================================================
    // Theta Decay Calculation Tests (isolated logic)
    // ============================================================================

    /// Test helper to calculate decay factor
    fn calculate_decay_factor(seconds_remaining: i64) -> f64 {
        const DECAY_WINDOW_SECS: i64 = 300; // 5 minutes

        if seconds_remaining >= DECAY_WINDOW_SECS {
            return 0.0; // No decay
        }
        if seconds_remaining <= 0 {
            return 1.0; // Full decay
        }

        let progress = (DECAY_WINDOW_SECS - seconds_remaining) as f64 / DECAY_WINDOW_SECS as f64;
        let k = 3.0_f64;
        1.0 - (-k * progress).exp()
    }

    /// Test helper to calculate decayed target
    fn calculate_test_decayed_target(seconds_remaining: i64, sum_target: Decimal) -> Decimal {
        let max_target = dec!(0.99);

        if seconds_remaining >= 300 {
            return sum_target;
        }
        if seconds_remaining <= 0 {
            return max_target;
        }

        let decay_factor = calculate_decay_factor(seconds_remaining);
        let range = max_target - sum_target;
        let decay_decimal = Decimal::from_f64_retain(decay_factor).unwrap_or(Decimal::ZERO);

        sum_target + (range * decay_decimal)
    }

    #[test]
    fn test_theta_decay_no_decay_outside_window() {
        // With 10 minutes remaining, should have no decay
        let target = calculate_test_decayed_target(600, dec!(0.95));
        assert_eq!(target, dec!(0.95));
    }

    #[test]
    fn test_theta_decay_at_boundary() {
        // At exactly 5 minutes (300s), should still be the original target
        let target = calculate_test_decayed_target(300, dec!(0.95));
        assert_eq!(target, dec!(0.95));
    }

    #[test]
    fn test_theta_decay_at_zero() {
        // At 0 seconds remaining, should be max target (0.99)
        let target = calculate_test_decayed_target(0, dec!(0.95));
        assert_eq!(target, dec!(0.99));
    }

    #[test]
    fn test_theta_decay_midway() {
        // At 150 seconds (halfway through decay window)
        let target = calculate_test_decayed_target(150, dec!(0.95));

        // Should be between 0.95 and 0.99
        assert!(target > dec!(0.95));
        assert!(target < dec!(0.99));

        // At halfway, with k=3.0, decay_factor ≈ 1 - e^(-1.5) ≈ 0.777
        // So target ≈ 0.95 + 0.04 * 0.777 ≈ 0.981
        // Allow some floating point tolerance
        assert!(target > dec!(0.97));
        assert!(target < dec!(0.985));
    }

    #[test]
    fn test_theta_decay_near_end() {
        // At 30 seconds remaining (near the end)
        let target = calculate_test_decayed_target(30, dec!(0.95));

        // Should be very close to 0.99
        assert!(target > dec!(0.985));
        assert!(target < dec!(0.99));
    }

    #[test]
    fn test_theta_decay_preserves_order() {
        // Decay should be monotonically increasing (target gets larger as time runs out)
        let t1 = calculate_test_decayed_target(250, dec!(0.95));
        let t2 = calculate_test_decayed_target(150, dec!(0.95));
        let t3 = calculate_test_decayed_target(50, dec!(0.95));

        assert!(t1 < t2);
        assert!(t2 < t3);
    }

    #[test]
    fn test_theta_decay_different_sum_targets() {
        // Test with different sum targets
        let t_95 = calculate_test_decayed_target(150, dec!(0.95));
        let t_92 = calculate_test_decayed_target(150, dec!(0.92));

        // Both should be between their start and 0.99
        assert!(t_95 > dec!(0.95) && t_95 < dec!(0.99));
        assert!(t_92 > dec!(0.92) && t_92 < dec!(0.99));

        // Lower sum target should have larger decay (more room to decay)
        // Actually, let's verify: 0.92 has 0.07 range, 0.95 has 0.04 range
        // With same decay factor, the absolute change is proportional to range
    }

    // ============================================================================
    // Profit Calculation Tests
    // ============================================================================

    #[test]
    fn test_profit_calculation_basic() {
        // Standard arbitrage scenario
        let leg1_price = dec!(0.35);
        let leg2_price = dec!(0.56);
        let shares = dec!(10);

        let total_cost = (leg1_price + leg2_price) * shares;
        let payout = shares; // $1 per share
        let profit = payout - total_cost;

        assert_eq!(total_cost, dec!(9.10));
        assert_eq!(profit, dec!(0.90));
    }

    #[test]
    fn test_profit_calculation_breakeven() {
        // Exactly $1 combined price = no profit
        let leg1_price = dec!(0.40);
        let leg2_price = dec!(0.60);
        let shares = dec!(10);

        let total_cost = (leg1_price + leg2_price) * shares;
        let payout = shares;
        let profit = payout - total_cost;

        assert_eq!(profit, Decimal::ZERO);
    }

    #[test]
    fn test_profit_calculation_loss() {
        // If sum > $1, we'd have a loss (shouldn't happen with proper hedge condition)
        let leg1_price = dec!(0.55);
        let leg2_price = dec!(0.50);
        let shares = dec!(10);

        let total_cost = (leg1_price + leg2_price) * shares;
        let payout = shares;
        let profit = payout - total_cost;

        assert!(profit < Decimal::ZERO);
        assert_eq!(profit, dec!(-0.50));
    }

    #[test]
    fn test_profit_percentage_calculation() {
        let leg1_price = dec!(0.35);
        let leg2_price = dec!(0.56);
        let shares = dec!(10);

        let total_cost = (leg1_price + leg2_price) * shares;
        let profit = shares - total_cost;
        let profit_pct = (profit / total_cost) * dec!(100);

        // $0.90 / $9.10 ≈ 9.89%
        assert!(profit_pct > dec!(9.8));
        assert!(profit_pct < dec!(10.0));
    }

    // ============================================================================
    // Hedge Condition Tests
    // ============================================================================

    #[test]
    fn test_hedge_condition_met() {
        let leg1_price = dec!(0.35);
        let opposite_ask = dec!(0.56);
        let sum_target = dec!(0.95);

        let sum = leg1_price + opposite_ask;
        assert_eq!(sum, dec!(0.91));
        assert!(sum <= sum_target);
    }

    #[test]
    fn test_hedge_condition_not_met() {
        let leg1_price = dec!(0.35);
        let opposite_ask = dec!(0.65);
        let sum_target = dec!(0.95);

        let sum = leg1_price + opposite_ask;
        assert_eq!(sum, dec!(1.00));
        assert!(sum > sum_target);
    }

    #[test]
    fn test_hedge_condition_boundary() {
        let leg1_price = dec!(0.35);
        let opposite_ask = dec!(0.60);
        let sum_target = dec!(0.95);

        let sum = leg1_price + opposite_ask;
        assert_eq!(sum, dec!(0.95));
        assert!(sum <= sum_target); // Equal to target is acceptable
    }

    // ============================================================================
    // Dump Detection Threshold Tests
    // ============================================================================

    #[test]
    fn test_dump_threshold_calculation() {
        let move_pct = dec!(0.15); // 15%
        let initial_price = dec!(0.50);
        let threshold_price = initial_price * (Decimal::ONE - move_pct);

        assert_eq!(threshold_price, dec!(0.425));

        // A drop from 0.50 to 0.42 should trigger
        let new_price = dec!(0.42);
        let drop_pct = (initial_price - new_price) / initial_price;
        assert!(drop_pct >= move_pct);
    }

    #[test]
    fn test_dump_detection_percentage() {
        let initial_price = dec!(0.50);
        let new_price = dec!(0.35);

        let drop_pct = (initial_price - new_price) / initial_price;
        // (0.50 - 0.35) / 0.50 = 0.15 / 0.50 = 0.30 (30%)

        assert_eq!(drop_pct, dec!(0.30));
        assert!(drop_pct >= dec!(0.15)); // Exceeds 15% threshold
    }
}
