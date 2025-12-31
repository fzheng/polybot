//! Core types used throughout the PolyBot trading application.
//!
//! This module defines the fundamental data structures used across the trading system,
//! including market sides, order types, price data, and strategy state management.
//!
//! # Key Types
//!
//! - [`Side`] - Represents UP or DOWN market positions
//! - [`OrderType`] - Order execution types (GTC, FOK, IOC)
//! - [`MarketInfo`] - BTC 15-minute market metadata
//! - [`OrderBook`] - Current bid/ask state for a token
//! - [`StrategyState`] - State machine for the two-leg trading strategy
//! - [`AutoParams`] - Configurable parameters for automated trading
//!
//! # Trading Model
//!
//! Polymarket BTC 15-minute markets are binary prediction markets where:
//! - Each round lasts 15 minutes
//! - Users can buy UP or DOWN tokens
//! - At the end, the winning side pays $1 per share, the losing side pays $0
//! - The strategy exploits price inefficiencies when UP + DOWN < $1

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// ============================================================================
// Market Sides
// ============================================================================

/// Side of a trade in BTC UP/DOWN prediction markets.
///
/// In Polymarket BTC 15-minute markets, users bet on whether BTC price
/// will go UP or DOWN within the 15-minute window. One side always wins
/// (pays $1 per share) and the other loses (pays $0).
///
/// # Examples
///
/// ```ignore
/// let side = Side::Up;
/// let opposite = side.opposite(); // Side::Down
/// println!("{}", side); // "UP"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    /// Betting that BTC price will increase
    Up,
    /// Betting that BTC price will decrease
    Down,
}

impl Side {
    /// Returns the opposite side.
    ///
    /// Used when executing the hedge leg of the two-leg strategy.
    #[inline]
    pub fn opposite(&self) -> Self {
        match self {
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }

    /// Returns a static string representation.
    #[inline]
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

// ============================================================================
// Order Types
// ============================================================================

/// Order execution type for trading on the CLOB.
///
/// Determines how the order is processed by the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good Till Cancelled - stays in order book until filled or cancelled.
    /// This is the default order type for the strategy.
    Gtc,
    /// Fill Or Kill - must fill entirely immediately or cancel.
    /// Used when you need all-or-nothing execution.
    Fok,
    /// Immediate Or Cancel - fill what's possible immediately, cancel rest.
    /// Useful for partial fills when speed matters.
    Ioc,
}

impl Default for OrderType {
    fn default() -> Self {
        Self::Gtc
    }
}

/// Direction of a trade (buying or selling shares).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BuySell {
    /// Buying shares (taking from asks)
    Buy,
    /// Selling shares (taking from bids)
    Sell,
}

// ============================================================================
// Market Data
// ============================================================================

/// A snapshot of market prices at a specific point in time.
///
/// Used for recording price data for backtesting and analysis.
/// Captured at regular intervals (configurable, default 1 second).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    /// When this snapshot was taken
    pub timestamp: DateTime<Utc>,
    /// Market identifier (e.g., "btc-updown-15m-1234567890")
    pub round_slug: String,
    /// Seconds until round ends
    pub seconds_remaining: u32,
    /// Token ID for UP outcome
    pub up_token_id: String,
    /// Token ID for DOWN outcome
    pub down_token_id: String,
    /// Best ask price for UP (price to buy)
    pub up_best_ask: Decimal,
    /// Best bid price for UP (price to sell)
    pub up_best_bid: Decimal,
    /// Best ask price for DOWN (price to buy)
    pub down_best_ask: Decimal,
    /// Best bid price for DOWN (price to sell)
    pub down_best_bid: Decimal,
}

/// Market information for a BTC 15-minute UP/DOWN round.
///
/// Contains all metadata needed to trade on a specific round,
/// including token IDs and timing information.
///
/// # Slug Format
///
/// Market slugs follow the pattern: `btc-updown-15m-{unix_timestamp}`
/// where the timestamp is the START time of the round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    /// Unique identifier for the market condition
    pub condition_id: String,
    /// Question ID for the market
    pub question_id: String,
    /// Human-readable market slug (e.g., "btc-updown-15m-1767246300")
    pub slug: String,
    /// Token ID for UP outcome - used for API calls
    pub up_token_id: String,
    /// Token ID for DOWN outcome - used for API calls
    pub down_token_id: String,
    /// When the round ends (resolution time)
    pub end_time: DateTime<Utc>,
    /// When the round started
    pub start_time: DateTime<Utc>,
}

impl MarketInfo {
    /// Returns seconds remaining until round ends.
    ///
    /// Returns 0 if the round has already ended.
    #[inline]
    pub fn seconds_remaining(&self) -> i64 {
        (self.end_time - Utc::now()).num_seconds().max(0)
    }

    /// Checks if the market is currently active.
    ///
    /// A market is active if the current time is between start and end.
    #[inline]
    pub fn is_active(&self) -> bool {
        let now = Utc::now();
        now >= self.start_time && now < self.end_time
    }
}

/// Current state of the order book for a token.
///
/// Tracks the best bid/ask prices and sizes, updated in real-time
/// via WebSocket connection.
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// Best (highest) bid price - what buyers are willing to pay
    pub best_bid: Option<Decimal>,
    /// Best (lowest) ask price - what sellers are asking
    pub best_ask: Option<Decimal>,
    /// Size available at best bid
    pub bid_size: Option<Decimal>,
    /// Size available at best ask
    pub ask_size: Option<Decimal>,
}

impl OrderBook {
    /// Calculates the bid-ask spread.
    ///
    /// Returns `None` if either bid or ask is unavailable.
    /// A smaller spread indicates more liquid market.
    #[inline]
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Calculates the mid-price (average of bid and ask).
    ///
    /// Returns `None` if either bid or ask is unavailable.
    #[inline]
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::TWO),
            _ => None,
        }
    }
}

// ============================================================================
// Orders and Trades
// ============================================================================

/// An order placed on the exchange.
///
/// Represents a limit order in the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Unique order identifier from the exchange
    pub id: String,
    /// Token being traded
    pub token_id: String,
    /// Whether this is a buy or sell order
    pub side: BuySell,
    /// Limit price for the order
    pub price: Decimal,
    /// Number of shares
    pub size: Decimal,
    /// Order type (GTC, FOK, IOC)
    pub order_type: OrderType,
    /// When the order was created
    pub created_at: DateTime<Utc>,
}

/// Result of a trade execution.
///
/// Contains details about how an order was filled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    /// Order ID assigned by the exchange
    pub order_id: String,
    /// Token that was traded
    pub token_id: String,
    /// Direction of the trade
    pub side: BuySell,
    /// Execution price
    pub price: Decimal,
    /// Requested size
    pub size: Decimal,
    /// Actually filled size (may be less than requested)
    pub filled_size: Decimal,
    /// Current status of the order
    pub status: TradeStatus,
    /// When the trade was executed
    pub timestamp: DateTime<Utc>,
}

/// Status of a trade/order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeStatus {
    /// Order is pending/live in the book
    Pending,
    /// Order is partially filled
    PartiallyFilled,
    /// Order is completely filled
    Filled,
    /// Order was cancelled
    Cancelled,
    /// Order failed to execute
    Failed,
}

// ============================================================================
// Strategy Types
// ============================================================================

/// State machine for the two-leg arbitrage strategy.
///
/// The strategy progresses through these states:
///
/// ```text
/// Watching -> WaitingForHedge -> Completed
///     |             |
///     v             v
///   (wait)      Abandoned (on round change)
/// ```
///
/// # State Transitions
///
/// - `Watching` -> `WaitingForHedge`: When a price dump is detected and Leg 1 executed
/// - `WaitingForHedge` -> `Completed`: When hedge condition met and Leg 2 executed
/// - Any state -> `Abandoned`: When the round changes before completion
/// - `Completed`/`Abandoned` -> `Watching`: Automatic reset for next cycle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyState {
    /// Watching for a price dump during the allowed window.
    /// This is the initial state at the start of each round.
    Watching,
    /// Leg 1 executed, waiting for hedge opportunity.
    /// Stores the side bought and price paid for hedge calculation.
    WaitingForHedge {
        /// Side that was bought in Leg 1
        leg1_side: Side,
        /// Price paid in Leg 1
        leg1_price: Decimal,
    },
    /// Both legs completed successfully. Cycle finished.
    Completed,
    /// Cycle abandoned before completion (e.g., round changed).
    Abandoned,
}

impl Default for StrategyState {
    fn default() -> Self {
        Self::Watching
    }
}

/// Parameters for the automated two-leg trading strategy.
///
/// These parameters control when and how the strategy executes trades.
///
/// # Strategy Logic
///
/// 1. During the first `window_min` minutes of a round
/// 2. Watch for a price drop of at least `move_pct` in either side
/// 3. When detected, buy `shares` of the dumped side (Leg 1)
/// 4. Wait for hedge condition: `leg1_price + opposite_ask <= sum_target`
/// 5. When condition met, buy `shares` of the opposite side (Leg 2)
/// 6. Guaranteed profit = `$1 * shares - total_cost`
///
/// # Example
///
/// With default params (shares=10, sum=0.95, move=15%, window=2min):
/// - If DOWN drops 17% to $0.35 in 3 seconds -> Buy 10 DOWN @ $0.35
/// - If UP ask falls to $0.56 (0.35 + 0.56 = 0.91 <= 0.95) -> Buy 10 UP @ $0.56
/// - Total cost: $9.10, Guaranteed payout: $10.00, Profit: $0.90 (9.9%)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoParams {
    /// Number of shares to buy for each leg.
    /// Both legs use the same share count to ensure balanced hedge.
    pub shares: Decimal,
    /// Sum threshold for hedge condition.
    /// Leg 2 triggers when: `leg1_price + opposite_ask <= sum_target`.
    /// Lower values = higher profit but fewer opportunities.
    /// Default: 0.95 (5% minimum profit margin)
    pub sum_target: Decimal,
    /// Dump threshold as a decimal percentage.
    /// Leg 1 triggers when price drops by at least this percentage.
    /// Example: 0.15 = 15% drop required.
    /// Default: 0.15
    pub move_pct: Decimal,
    /// Minutes from round start during which Leg 1 is allowed.
    /// After this window, the strategy waits for the next round.
    /// Default: 2 minutes
    pub window_min: u32,
}

impl Default for AutoParams {
    fn default() -> Self {
        Self {
            shares: Decimal::new(10, 0),     // 10 shares
            sum_target: Decimal::new(95, 2), // 0.95
            move_pct: Decimal::new(15, 2),   // 0.15 (15%)
            window_min: 2,                   // 2 minutes
        }
    }
}

/// Price history entry for dump detection.
///
/// Used to track recent price movements and detect rapid drops
/// that trigger Leg 1 of the strategy.
#[derive(Debug, Clone)]
pub struct PriceEntry {
    /// When this price was observed
    pub timestamp: DateTime<Utc>,
    /// Best ask price for UP at this time
    pub up_ask: Decimal,
    /// Best ask price for DOWN at this time
    pub down_ask: Decimal,
}
