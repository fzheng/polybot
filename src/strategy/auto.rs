//! Auto trading strategy - Two-leg arbitrage

#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::market::MarketWatcher;
use crate::types::{AutoParams, Side, StrategyState, TradeResult};

/// Log entry for strategy actions
#[derive(Debug, Clone)]
pub struct StrategyLog {
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub is_error: bool,
}

/// Auto trader implementing the two-leg strategy
pub struct AutoTrader {
    params: Arc<RwLock<AutoParams>>,
    state: Arc<RwLock<StrategyState>>,
    current_round: Arc<RwLock<Option<String>>>,
    leg1_result: Arc<RwLock<Option<TradeResult>>>,
    leg2_result: Arc<RwLock<Option<TradeResult>>>,
    logs: Arc<RwLock<Vec<StrategyLog>>>,
    enabled: Arc<RwLock<bool>>,
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
            leg1_result: Arc::new(RwLock::new(None)),
            leg2_result: Arc::new(RwLock::new(None)),
            logs: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(RwLock::new(false)),
            total_profit: Arc::new(RwLock::new(Decimal::ZERO)),
            cycles_completed: Arc::new(RwLock::new(0)),
        }
    }

    /// Enable auto trading
    pub async fn enable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = true;
        self.log("Auto trading ENABLED").await;
    }

    /// Disable auto trading
    pub async fn disable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = false;
        self.log("Auto trading DISABLED").await;
    }

    /// Check if auto trading is enabled
    pub async fn is_enabled(&self) -> bool {
        *self.enabled.read().await
    }

    /// Update parameters
    pub async fn set_params(&self, params: AutoParams) {
        let msg = format!(
            "Params updated: shares={}, sum={}, move={}%, window={}min",
            params.shares, params.sum_target, params.move_pct * Decimal::ONE_HUNDRED, params.window_min
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
    pub async fn tick(&self, watcher: &MarketWatcher) -> Result<()> {
        if !self.is_enabled().await {
            return Ok(());
        }

        // Check if round changed
        let current_market = watcher.get_current_market().await;
        let current_slug = current_market.as_ref().map(|m| m.slug.clone());

        {
            let mut round = self.current_round.write().await;
            if *round != current_slug {
                // Round changed - abandon current cycle
                if round.is_some() && self.get_state().await != StrategyState::Watching {
                    self.log("Round changed - abandoning cycle").await;
                    self.reset_cycle().await;
                }
                *round = current_slug.clone();
            }
        }

        // Process based on current state
        let state = self.get_state().await;

        match state {
            StrategyState::Watching => {
                self.watch_for_dump(watcher).await?;
            }
            StrategyState::WaitingForHedge { leg1_side, leg1_price } => {
                self.watch_for_hedge(watcher, leg1_side, leg1_price).await?;
            }
            StrategyState::Completed | StrategyState::Abandoned => {
                // Reset for next cycle
                self.reset_cycle().await;
            }
        }

        Ok(())
    }

    /// Watch for price dump during window
    async fn watch_for_dump(&self, watcher: &MarketWatcher) -> Result<()> {
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
            if let Err(e) = self.execute_leg1(watcher, side).await {
                self.log_error(&format!("Leg 1 failed: {}", e)).await;
            }
        }

        Ok(())
    }

    /// Execute Leg 1 - buy the side that dumped
    async fn execute_leg1(&self, watcher: &MarketWatcher, side: Side) -> Result<()> {
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

        // Place order (if authenticated)
        if watcher.client().is_authenticated() {
            let result = watcher.client()
                .buy_at_ask(token_id, shares)
                .await?;

            let mut leg1 = self.leg1_result.write().await;
            *leg1 = Some(result);
        } else {
            self.log("(Simulated - not authenticated)").await;
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

            // Check hedge condition
            if sum <= sum_target {
                self.log(&format!(
                    "Hedge condition met! {:.4} + {:.4} = {:.4} <= {:.4}",
                    leg1_price, ask, sum, sum_target
                )).await;

                if let Err(e) = self.execute_leg2(watcher, opposite_side, ask).await {
                    self.log_error(&format!("Leg 2 failed: {}", e)).await;
                }
            }
        }

        Ok(())
    }

    /// Execute Leg 2 - hedge by buying opposite side
    async fn execute_leg2(&self, watcher: &MarketWatcher, side: Side, price: Decimal) -> Result<()> {
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

        // Place order (if authenticated)
        if watcher.client().is_authenticated() {
            let result = watcher.client()
                .buy_at_ask(token_id, shares)
                .await?;

            let mut leg2 = self.leg2_result.write().await;
            *leg2 = Some(result);
        } else {
            self.log("(Simulated - not authenticated)").await;
        }

        // Calculate profit
        let leg1 = self.leg1_result.read().await;
        if let Some(l1) = leg1.as_ref() {
            let total_cost = l1.price * shares + price * shares;
            let payout = shares; // $1 per share pair
            let profit = payout - total_cost;
            let profit_pct = (profit / total_cost) * Decimal::ONE_HUNDRED;

            self.log(&format!(
                "CYCLE COMPLETE! Cost: ${:.4}, Payout: ${:.4}, Profit: ${:.4} ({:.2}%)",
                total_cost, payout, profit, profit_pct
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

        let mut leg1 = self.leg1_result.write().await;
        *leg1 = None;

        let mut leg2 = self.leg2_result.write().await;
        *leg2 = None;
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
