//! Paper trading module - simulates trades without real orders

#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::PaperTradingConfig;
use crate::types::{BuySell, Side, TradeResult, TradeStatus};

/// A simulated position in a token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub token_id: String,
    pub side: Side,
    pub shares: Decimal,
    pub avg_entry_price: Decimal,
    pub current_value: Decimal,
    pub unrealized_pnl: Decimal,
}

/// A paper trade record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTrade {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub token_id: String,
    pub side: Side,
    pub buy_sell: BuySell,
    pub shares: Decimal,
    pub price: Decimal,
    pub fee: Decimal,
    pub slippage: Decimal,
    pub total_cost: Decimal,
    pub balance_after: Decimal,
    pub round_slug: String,
}

/// Summary of paper trading performance
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaperTradingStats {
    pub total_trades: u32,
    pub winning_trades: u32,
    pub losing_trades: u32,
    pub total_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub unrealized_pnl: Decimal,
    pub total_fees: Decimal,
    pub total_slippage: Decimal,
    pub best_trade: Decimal,
    pub worst_trade: Decimal,
    pub cycles_completed: u32,
    pub cycles_abandoned: u32,
}

/// Paper trading engine
pub struct PaperTrader {
    config: PaperTradingConfig,
    balance: Arc<RwLock<Decimal>>,
    starting_balance: Decimal,
    positions: Arc<RwLock<HashMap<String, Position>>>,
    trades: Arc<RwLock<Vec<PaperTrade>>>,
    stats: Arc<RwLock<PaperTradingStats>>,
    log_writer: Option<Arc<RwLock<BufWriter<File>>>>,
}

impl PaperTrader {
    /// Create a new paper trader
    pub fn new(config: PaperTradingConfig) -> Result<Self> {
        let starting_balance = config.starting_balance;

        // Open log file for appending
        let log_writer = if !config.log_file.is_empty() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config.log_file)
                .context("Failed to open paper trades log file")?;
            Some(Arc::new(RwLock::new(BufWriter::new(file))))
        } else {
            None
        };

        Ok(Self {
            config,
            balance: Arc::new(RwLock::new(starting_balance)),
            starting_balance,
            positions: Arc::new(RwLock::new(HashMap::new())),
            trades: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(PaperTradingStats::default())),
            log_writer,
        })
    }

    /// Check if paper trading is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get current balance
    pub async fn get_balance(&self) -> Decimal {
        *self.balance.read().await
    }

    /// Get starting balance
    pub fn get_starting_balance(&self) -> Decimal {
        self.starting_balance
    }

    /// Get all positions
    pub async fn get_positions(&self) -> HashMap<String, Position> {
        self.positions.read().await.clone()
    }

    /// Get position for a specific token
    pub async fn get_position(&self, token_id: &str) -> Option<Position> {
        self.positions.read().await.get(token_id).cloned()
    }

    /// Get trading stats
    pub async fn get_stats(&self) -> PaperTradingStats {
        self.stats.read().await.clone()
    }

    /// Get recent trades
    pub async fn get_recent_trades(&self, count: usize) -> Vec<PaperTrade> {
        let trades = self.trades.read().await;
        trades.iter().rev().take(count).cloned().collect()
    }

    /// Get total PnL (realized + unrealized)
    pub async fn get_total_pnl(&self) -> Decimal {
        let balance = self.get_balance().await;
        let positions = self.positions.read().await;

        let positions_value: Decimal = positions.values()
            .map(|p| p.current_value)
            .sum();

        (balance + positions_value) - self.starting_balance
    }

    /// Simulate buying shares
    pub async fn buy(
        &self,
        token_id: &str,
        side: Side,
        shares: Decimal,
        market_price: Decimal,
        round_slug: &str,
    ) -> Result<TradeResult> {
        // Calculate price with slippage (buying at slightly higher price)
        let slippage_amount = market_price * self.config.slippage;
        let execution_price = market_price + slippage_amount;

        // Calculate cost and fee
        let gross_cost = execution_price * shares;
        let fee = gross_cost * self.config.fee_rate;
        let total_cost = gross_cost + fee;

        // Check if we have enough balance
        let mut balance = self.balance.write().await;
        if *balance < total_cost {
            anyhow::bail!(
                "Insufficient balance: need ${:.2}, have ${:.2}",
                total_cost,
                *balance
            );
        }

        // Deduct from balance
        *balance -= total_cost;
        let balance_after = *balance;
        drop(balance);

        // Update position
        let mut positions = self.positions.write().await;
        let position = positions.entry(token_id.to_string()).or_insert(Position {
            token_id: token_id.to_string(),
            side,
            shares: Decimal::ZERO,
            avg_entry_price: Decimal::ZERO,
            current_value: Decimal::ZERO,
            unrealized_pnl: Decimal::ZERO,
        });

        // Calculate new average entry price
        let old_value = position.shares * position.avg_entry_price;
        let new_value = shares * execution_price;
        position.shares += shares;
        if position.shares > Decimal::ZERO {
            position.avg_entry_price = (old_value + new_value) / position.shares;
        }
        position.current_value = position.shares * execution_price;
        drop(positions);

        // Create trade record
        let trade = PaperTrade {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            token_id: token_id.to_string(),
            side,
            buy_sell: BuySell::Buy,
            shares,
            price: execution_price,
            fee,
            slippage: slippage_amount * shares,
            total_cost,
            balance_after,
            round_slug: round_slug.to_string(),
        };

        // Log the trade
        self.log_trade(&trade).await?;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_trades += 1;
        stats.total_fees += fee;
        stats.total_slippage += slippage_amount * shares;
        drop(stats);

        // Store trade
        let mut trades = self.trades.write().await;
        trades.push(trade.clone());
        drop(trades);

        Ok(TradeResult {
            order_id: trade.id,
            token_id: token_id.to_string(),
            side: BuySell::Buy,
            price: execution_price,
            size: shares,
            filled_size: shares,
            status: TradeStatus::Filled,
            timestamp: Utc::now(),
        })
    }

    /// Simulate selling shares (or settling a position)
    pub async fn sell(
        &self,
        token_id: &str,
        shares: Decimal,
        market_price: Decimal,
        round_slug: &str,
    ) -> Result<TradeResult> {
        // Check if we have the position
        let mut positions = self.positions.write().await;
        let position = positions.get_mut(token_id)
            .context("No position to sell")?;

        if position.shares < shares {
            anyhow::bail!(
                "Insufficient shares: need {}, have {}",
                shares,
                position.shares
            );
        }

        let side = position.side;
        let entry_price = position.avg_entry_price;

        // Calculate price with slippage (selling at slightly lower price)
        let slippage_amount = market_price * self.config.slippage;
        let execution_price = market_price - slippage_amount;

        // Calculate proceeds and fee
        let gross_proceeds = execution_price * shares;
        let fee = gross_proceeds * self.config.fee_rate;
        let net_proceeds = gross_proceeds - fee;

        // Calculate realized PnL for this trade
        let cost_basis = entry_price * shares;
        let realized_pnl = net_proceeds - cost_basis;

        // Update position
        position.shares -= shares;
        if position.shares == Decimal::ZERO {
            positions.remove(token_id);
        } else {
            position.current_value = position.shares * execution_price;
        }
        drop(positions);

        // Add to balance
        let mut balance = self.balance.write().await;
        *balance += net_proceeds;
        let balance_after = *balance;
        drop(balance);

        // Create trade record
        let trade = PaperTrade {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            token_id: token_id.to_string(),
            side,
            buy_sell: BuySell::Sell,
            shares,
            price: execution_price,
            fee,
            slippage: slippage_amount * shares,
            total_cost: net_proceeds, // For sells, this is proceeds
            balance_after,
            round_slug: round_slug.to_string(),
        };

        // Log the trade
        self.log_trade(&trade).await?;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_trades += 1;
        stats.total_fees += fee;
        stats.total_slippage += slippage_amount * shares;
        stats.realized_pnl += realized_pnl;

        if realized_pnl > Decimal::ZERO {
            stats.winning_trades += 1;
            if realized_pnl > stats.best_trade {
                stats.best_trade = realized_pnl;
            }
        } else if realized_pnl < Decimal::ZERO {
            stats.losing_trades += 1;
            if realized_pnl < stats.worst_trade {
                stats.worst_trade = realized_pnl;
            }
        }
        drop(stats);

        // Store trade
        let mut trades = self.trades.write().await;
        trades.push(trade.clone());
        drop(trades);

        Ok(TradeResult {
            order_id: trade.id,
            token_id: token_id.to_string(),
            side: BuySell::Sell,
            price: execution_price,
            size: shares,
            filled_size: shares,
            status: TradeStatus::Filled,
            timestamp: Utc::now(),
        })
    }

    /// Settle a completed round (one side wins, the other loses)
    /// winning_side: The side that won (UP or DOWN)
    pub async fn settle_round(
        &self,
        up_token_id: &str,
        down_token_id: &str,
        winning_side: Side,
        round_slug: &str,
    ) -> Result<Decimal> {
        let mut total_pnl = Decimal::ZERO;
        let mut positions = self.positions.write().await;

        // Process winning position
        let winning_token = match winning_side {
            Side::Up => up_token_id,
            Side::Down => down_token_id,
        };

        if let Some(pos) = positions.remove(winning_token) {
            // Winning position pays out $1 per share
            let payout = pos.shares;
            let cost_basis = pos.shares * pos.avg_entry_price;
            let pnl = payout - cost_basis;
            total_pnl += pnl;

            let mut balance = self.balance.write().await;
            *balance += payout;
            drop(balance);

            tracing::info!(
                "[PAPER] Round settled - {} WON: {} shares at ${:.4} avg, payout ${:.2}, PnL ${:.2}",
                winning_side,
                pos.shares,
                pos.avg_entry_price,
                payout,
                pnl
            );
        }

        // Process losing position
        let losing_token = match winning_side {
            Side::Up => down_token_id,
            Side::Down => up_token_id,
        };

        if let Some(pos) = positions.remove(losing_token) {
            // Losing position is worth $0
            let cost_basis = pos.shares * pos.avg_entry_price;
            let pnl = -cost_basis;
            total_pnl += pnl;

            tracing::info!(
                "[PAPER] Round settled - {} LOST: {} shares at ${:.4} avg, lost ${:.2}",
                pos.side,
                pos.shares,
                pos.avg_entry_price,
                cost_basis
            );
        }

        drop(positions);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.realized_pnl += total_pnl;
        stats.cycles_completed += 1;

        if total_pnl > Decimal::ZERO {
            stats.winning_trades += 1;
        } else if total_pnl < Decimal::ZERO {
            stats.losing_trades += 1;
        }
        drop(stats);

        // Log settlement
        let settlement_log = serde_json::json!({
            "type": "settlement",
            "timestamp": Utc::now().to_rfc3339(),
            "round_slug": round_slug,
            "winning_side": winning_side.as_str(),
            "pnl": total_pnl.to_string(),
            "balance_after": self.get_balance().await.to_string(),
        });

        if let Some(writer) = &self.log_writer {
            let mut w = writer.write().await;
            let _ = writeln!(w, "{}", settlement_log);
            let _ = w.flush();
        }

        Ok(total_pnl)
    }

    /// Mark a cycle as abandoned (round changed before completion)
    pub async fn abandon_cycle(&self, round_slug: &str) {
        let mut stats = self.stats.write().await;
        stats.cycles_abandoned += 1;
        drop(stats);

        tracing::warn!("[PAPER] Cycle abandoned for round: {}", round_slug);
    }

    /// Update position values with current market prices
    pub async fn update_prices(&self, up_price: Option<Decimal>, down_price: Option<Decimal>) {
        let mut positions = self.positions.write().await;
        let mut unrealized_pnl = Decimal::ZERO;

        for position in positions.values_mut() {
            let current_price = match position.side {
                Side::Up => up_price,
                Side::Down => down_price,
            };

            if let Some(price) = current_price {
                position.current_value = position.shares * price;
                position.unrealized_pnl = position.current_value
                    - (position.shares * position.avg_entry_price);
                unrealized_pnl += position.unrealized_pnl;
            }
        }
        drop(positions);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.unrealized_pnl = unrealized_pnl;
        stats.total_pnl = stats.realized_pnl + unrealized_pnl;
    }

    /// Log a trade to the file
    async fn log_trade(&self, trade: &PaperTrade) -> Result<()> {
        if let Some(writer) = &self.log_writer {
            let json = serde_json::to_string(trade)?;
            let mut w = writer.write().await;
            writeln!(w, "{}", json)?;
            w.flush()?;
        }

        tracing::info!(
            "[PAPER] {} {} {} shares of {} at ${:.4} (fee: ${:.4}, slippage: ${:.4})",
            trade.buy_sell.to_string().to_uppercase(),
            trade.side,
            trade.shares,
            trade.token_id,
            trade.price,
            trade.fee,
            trade.slippage
        );

        Ok(())
    }

    /// Reset paper trading (clear positions, reset balance)
    pub async fn reset(&self) {
        let mut balance = self.balance.write().await;
        *balance = self.starting_balance;
        drop(balance);

        let mut positions = self.positions.write().await;
        positions.clear();
        drop(positions);

        let mut trades = self.trades.write().await;
        trades.clear();
        drop(trades);

        let mut stats = self.stats.write().await;
        *stats = PaperTradingStats::default();
        drop(stats);

        tracing::info!("[PAPER] Paper trading reset. Balance: ${}", self.starting_balance);
    }

    /// Flush log file
    pub async fn flush(&self) -> Result<()> {
        if let Some(writer) = &self.log_writer {
            let mut w = writer.write().await;
            w.flush()?;
        }
        Ok(())
    }
}

impl BuySell {
    fn to_string(&self) -> &'static str {
        match self {
            BuySell::Buy => "buy",
            BuySell::Sell => "sell",
        }
    }
}
