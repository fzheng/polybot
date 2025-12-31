//! Core types used throughout the application

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Side of a trade (UP or DOWN in the BTC market)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Up,
    Down,
}

impl Side {
    pub fn opposite(&self) -> Self {
        match self {
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Up => "UP",
            Side::Down => "DOWN",
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Order type for trading
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good Till Cancelled - stays in orderbook until filled or cancelled
    Gtc,
    /// Fill Or Kill - must fill entirely or cancel
    Fok,
    /// Immediate Or Cancel - fill what's possible, cancel rest
    Ioc,
}

impl Default for OrderType {
    fn default() -> Self {
        Self::Gtc
    }
}

/// Buy or Sell
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BuySell {
    Buy,
    Sell,
}

/// Market price snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub timestamp: DateTime<Utc>,
    pub round_slug: String,
    pub seconds_remaining: u32,
    pub up_token_id: String,
    pub down_token_id: String,
    pub up_best_ask: Decimal,
    pub up_best_bid: Decimal,
    pub down_best_ask: Decimal,
    pub down_best_bid: Decimal,
}

/// Market information for a BTC 15-min round
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub condition_id: String,
    pub question_id: String,
    pub slug: String,
    pub up_token_id: String,
    pub down_token_id: String,
    pub end_time: DateTime<Utc>,
    pub start_time: DateTime<Utc>,
}

impl MarketInfo {
    pub fn seconds_remaining(&self) -> i64 {
        let now = Utc::now();
        (self.end_time - now).num_seconds().max(0)
    }

    pub fn is_active(&self) -> bool {
        let now = Utc::now();
        now >= self.start_time && now < self.end_time
    }
}

/// Current state of the order book for a token
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub bid_size: Option<Decimal>,
    pub ask_size: Option<Decimal>,
}

impl OrderBook {
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }
}

/// Order placed on the exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub token_id: String,
    pub side: BuySell,
    pub price: Decimal,
    pub size: Decimal,
    pub order_type: OrderType,
    pub created_at: DateTime<Utc>,
}

/// Trade execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    pub order_id: String,
    pub token_id: String,
    pub side: BuySell,
    pub price: Decimal,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub status: TradeStatus,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeStatus {
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Failed,
}

/// State of the auto trading strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyState {
    /// Watching for a price dump during the window
    Watching,
    /// Leg 1 executed, waiting for hedge opportunity
    WaitingForHedge { leg1_side: Side, leg1_price: Decimal },
    /// Both legs completed, cycle finished
    Completed,
    /// Cycle abandoned (e.g., round changed)
    Abandoned,
}

impl Default for StrategyState {
    fn default() -> Self {
        Self::Watching
    }
}

/// Parameters for auto trading mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoParams {
    /// Number of shares to buy for each leg
    pub shares: Decimal,
    /// Sum threshold for hedge (leg1_price + opposite_ask <= sum_target)
    pub sum_target: Decimal,
    /// Dump threshold as a percentage (0.15 = 15%)
    pub move_pct: Decimal,
    /// Minutes from round start during which Leg 1 is allowed
    pub window_min: u32,
}

impl Default for AutoParams {
    fn default() -> Self {
        Self {
            shares: Decimal::new(10, 0),          // 10 shares
            sum_target: Decimal::new(95, 2),      // 0.95
            move_pct: Decimal::new(15, 2),        // 0.15 (15%)
            window_min: 2,                         // 2 minutes
        }
    }
}

/// Price history entry for dump detection
#[derive(Debug, Clone)]
pub struct PriceEntry {
    pub timestamp: DateTime<Utc>,
    pub up_ask: Decimal,
    pub down_ask: Decimal,
}
