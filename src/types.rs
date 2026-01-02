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
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        let now = Utc::now();
        now >= self.start_time && now < self.end_time
    }
}

/// A single price level in the order book.
///
/// Represents one row in the book with price and available size.
#[derive(Debug, Clone)]
pub struct OrderBookLevel {
    /// Price at this level
    pub price: Decimal,
    /// Size available at this price
    pub size: Decimal,
}

/// Current state of the order book for a token.
///
/// Tracks multiple bid/ask levels (up to 3 levels for depth display),
/// updated in real-time via WebSocket connection.
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
    /// Top ask levels (best ask first, up to 3 levels)
    pub ask_levels: Vec<OrderBookLevel>,
    /// Top bid levels (best bid first, up to 3 levels)
    pub bid_levels: Vec<OrderBookLevel>,
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
///     |
///     v
///   (wait for next round)
/// ```
///
/// # State Transitions
///
/// - `Watching` -> `WaitingForHedge`: When a price dump is detected and Leg 1 executed
/// - `WaitingForHedge` -> `Completed`: When hedge condition met and Leg 2 executed
/// - `Completed` -> `Watching`: Automatic reset for next cycle
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
/// # Time Decay
///
/// The sum_target uses options-like theta decay in the last 5 minutes:
/// - First 10 minutes: No decay, uses strict sum_target
/// - Last 5 minutes: Exponential decay toward 0.99 (1% minimum profit)
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
    /// In the last 5 minutes, this decays toward 0.99 (theta decay).
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
    /// Maximum number of cycles (Leg1+Leg2 pairs) per round.
    /// Set to 1 for conservative, higher for more aggressive.
    /// Default: 1
    pub max_cycles: u32,
}

impl Default for AutoParams {
    fn default() -> Self {
        Self {
            shares: Decimal::new(10, 0),      // 10 shares
            sum_target: Decimal::new(95, 2),  // 0.95
            move_pct: Decimal::new(15, 2),    // 0.15 (15%)
            window_min: 2,                    // 2 minutes
            max_cycles: 1,                    // 1 cycle per round
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // ============================================================================
    // Side Tests
    // ============================================================================

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Up.opposite(), Side::Down);
        assert_eq!(Side::Down.opposite(), Side::Up);
    }

    #[test]
    fn test_side_opposite_is_reflexive() {
        assert_eq!(Side::Up.opposite().opposite(), Side::Up);
        assert_eq!(Side::Down.opposite().opposite(), Side::Down);
    }

    #[test]
    fn test_side_as_str() {
        assert_eq!(Side::Up.as_str(), "UP");
        assert_eq!(Side::Down.as_str(), "DOWN");
    }

    #[test]
    fn test_side_display() {
        assert_eq!(format!("{}", Side::Up), "UP");
        assert_eq!(format!("{}", Side::Down), "DOWN");
    }

    #[test]
    fn test_side_equality() {
        assert_eq!(Side::Up, Side::Up);
        assert_eq!(Side::Down, Side::Down);
        assert_ne!(Side::Up, Side::Down);
    }

    // ============================================================================
    // OrderType Tests
    // ============================================================================

    #[test]
    fn test_order_type_default() {
        assert_eq!(OrderType::default(), OrderType::Gtc);
    }

    #[test]
    fn test_order_type_variants() {
        let gtc = OrderType::Gtc;
        let fok = OrderType::Fok;
        let ioc = OrderType::Ioc;

        assert_ne!(gtc, fok);
        assert_ne!(fok, ioc);
        assert_ne!(gtc, ioc);
    }

    // ============================================================================
    // MarketInfo Tests
    // ============================================================================

    fn create_test_market(start_offset_secs: i64, end_offset_secs: i64) -> MarketInfo {
        let now = Utc::now();
        MarketInfo {
            condition_id: "test-condition".to_string(),
            question_id: "test-question".to_string(),
            slug: "btc-updown-15m-test".to_string(),
            up_token_id: "up-token-123".to_string(),
            down_token_id: "down-token-456".to_string(),
            start_time: now + Duration::seconds(start_offset_secs),
            end_time: now + Duration::seconds(end_offset_secs),
        }
    }

    #[test]
    fn test_market_info_seconds_remaining_future() {
        let market = create_test_market(-60, 300); // started 1 min ago, ends in 5 min
        let remaining = market.seconds_remaining();
        // Should be approximately 300 seconds (allowing for test execution time)
        assert!(remaining > 295 && remaining <= 300);
    }

    #[test]
    fn test_market_info_seconds_remaining_past() {
        let market = create_test_market(-600, -60); // ended 1 min ago
        let remaining = market.seconds_remaining();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn test_market_info_is_active_true() {
        let market = create_test_market(-60, 300); // started 1 min ago, ends in 5 min
        assert!(market.is_active());
    }

    #[test]
    fn test_market_info_is_active_not_started() {
        let market = create_test_market(60, 360); // starts in 1 min
        assert!(!market.is_active());
    }

    #[test]
    fn test_market_info_is_active_ended() {
        let market = create_test_market(-600, -60); // ended 1 min ago
        assert!(!market.is_active());
    }

    // ============================================================================
    // OrderBook Tests
    // ============================================================================

    #[test]
    fn test_order_book_default() {
        let book = OrderBook::default();
        assert!(book.best_bid.is_none());
        assert!(book.best_ask.is_none());
        assert!(book.bid_size.is_none());
        assert!(book.ask_size.is_none());
    }

    #[test]
    fn test_order_book_with_values() {
        let book = OrderBook {
            best_bid: Some(Decimal::new(45, 2)),  // 0.45
            best_ask: Some(Decimal::new(50, 2)),  // 0.50
            bid_size: Some(Decimal::new(100, 0)), // 100 shares
            ask_size: Some(Decimal::new(150, 0)), // 150 shares
            ask_levels: vec![
                OrderBookLevel { price: Decimal::new(50, 2), size: Decimal::new(150, 0) },
            ],
            bid_levels: vec![
                OrderBookLevel { price: Decimal::new(45, 2), size: Decimal::new(100, 0) },
            ],
        };

        assert_eq!(book.best_bid, Some(Decimal::new(45, 2)));
        assert_eq!(book.best_ask, Some(Decimal::new(50, 2)));
        assert_eq!(book.ask_levels.len(), 1);
        assert_eq!(book.bid_levels.len(), 1);
    }

    // ============================================================================
    // StrategyState Tests
    // ============================================================================

    #[test]
    fn test_strategy_state_default() {
        assert_eq!(StrategyState::default(), StrategyState::Watching);
    }

    #[test]
    fn test_strategy_state_waiting_for_hedge() {
        let state = StrategyState::WaitingForHedge {
            leg1_side: Side::Up,
            leg1_price: Decimal::new(40, 2), // 0.40
        };

        if let StrategyState::WaitingForHedge { leg1_side, leg1_price } = state {
            assert_eq!(leg1_side, Side::Up);
            assert_eq!(leg1_price, Decimal::new(40, 2));
        } else {
            panic!("Expected WaitingForHedge state");
        }
    }

    #[test]
    fn test_strategy_state_equality() {
        assert_eq!(StrategyState::Watching, StrategyState::Watching);
        assert_eq!(StrategyState::Completed, StrategyState::Completed);

        let state1 = StrategyState::WaitingForHedge {
            leg1_side: Side::Up,
            leg1_price: Decimal::new(40, 2),
        };
        let state2 = StrategyState::WaitingForHedge {
            leg1_side: Side::Up,
            leg1_price: Decimal::new(40, 2),
        };
        let state3 = StrategyState::WaitingForHedge {
            leg1_side: Side::Down,
            leg1_price: Decimal::new(40, 2),
        };

        assert_eq!(state1, state2);
        assert_ne!(state1, state3);
    }

    // ============================================================================
    // AutoParams Tests
    // ============================================================================

    #[test]
    fn test_auto_params_default() {
        let params = AutoParams::default();

        assert_eq!(params.shares, Decimal::new(10, 0));
        assert_eq!(params.sum_target, Decimal::new(95, 2));
        assert_eq!(params.move_pct, Decimal::new(15, 2));
        assert_eq!(params.window_min, 2);
        assert_eq!(params.max_cycles, 1);
    }

    #[test]
    fn test_auto_params_custom() {
        let params = AutoParams {
            shares: Decimal::new(20, 0),       // 20 shares
            sum_target: Decimal::new(92, 2),   // 0.92
            move_pct: Decimal::new(10, 2),     // 0.10 (10%)
            window_min: 4,                     // 4 minutes
            max_cycles: 3,                     // 3 cycles
        };

        assert_eq!(params.shares, Decimal::new(20, 0));
        assert_eq!(params.sum_target, Decimal::new(92, 2));
        assert_eq!(params.move_pct, Decimal::new(10, 2));
        assert_eq!(params.window_min, 4);
        assert_eq!(params.max_cycles, 3);
    }

    #[test]
    fn test_auto_params_profit_calculation() {
        // Verify the profit calculation logic described in docs
        let params = AutoParams::default();

        // Scenario: DOWN drops to 0.35, UP falls to 0.56
        let leg1_price = Decimal::new(35, 2); // 0.35
        let leg2_price = Decimal::new(56, 2); // 0.56
        let sum = leg1_price + leg2_price;    // 0.91

        // Should be within sum_target
        assert!(sum <= params.sum_target); // 0.91 <= 0.95

        // Calculate profit
        let total_cost = (leg1_price + leg2_price) * params.shares; // $9.10
        let payout = params.shares; // $10.00 (one side always wins)
        let profit = payout - total_cost; // $0.90

        assert_eq!(total_cost, Decimal::new(910, 2));
        assert_eq!(profit, Decimal::new(90, 2));
    }

    // ============================================================================
    // TradeResult & TradeStatus Tests
    // ============================================================================

    #[test]
    fn test_trade_status_variants() {
        assert_ne!(TradeStatus::Pending, TradeStatus::Filled);
        assert_ne!(TradeStatus::PartiallyFilled, TradeStatus::Cancelled);
        assert_ne!(TradeStatus::Filled, TradeStatus::Failed);
    }

    #[test]
    fn test_trade_result() {
        let result = TradeResult {
            order_id: "order-123".to_string(),
            token_id: "token-456".to_string(),
            side: BuySell::Buy,
            price: Decimal::new(50, 2),
            size: Decimal::new(10, 0),
            filled_size: Decimal::new(10, 0),
            status: TradeStatus::Filled,
            timestamp: Utc::now(),
        };

        assert_eq!(result.order_id, "order-123");
        assert_eq!(result.side, BuySell::Buy);
        assert_eq!(result.size, result.filled_size); // Fully filled
        assert_eq!(result.status, TradeStatus::Filled);
    }

    #[test]
    fn test_trade_result_partial_fill() {
        let result = TradeResult {
            order_id: "order-789".to_string(),
            token_id: "token-abc".to_string(),
            side: BuySell::Buy,
            price: Decimal::new(45, 2),
            size: Decimal::new(100, 0),
            filled_size: Decimal::new(50, 0), // Only half filled
            status: TradeStatus::PartiallyFilled,
            timestamp: Utc::now(),
        };

        assert!(result.filled_size < result.size);
        assert_eq!(result.status, TradeStatus::PartiallyFilled);
    }

    // ============================================================================
    // PriceEntry Tests
    // ============================================================================

    #[test]
    fn test_price_entry() {
        let entry = PriceEntry {
            timestamp: Utc::now(),
            up_ask: Decimal::new(52, 2),   // 0.52
            down_ask: Decimal::new(48, 2), // 0.48
        };

        // Sum should be 1.00 (perfectly balanced market)
        assert_eq!(entry.up_ask + entry.down_ask, Decimal::ONE);
    }

    #[test]
    fn test_price_entry_arbitrage_opportunity() {
        let entry = PriceEntry {
            timestamp: Utc::now(),
            up_ask: Decimal::new(45, 2),   // 0.45
            down_ask: Decimal::new(50, 2), // 0.50
        };

        let sum = entry.up_ask + entry.down_ask;
        // Sum < 1.00 means arbitrage opportunity exists
        assert!(sum < Decimal::ONE);
        assert_eq!(sum, Decimal::new(95, 2)); // 0.95
    }

    // ============================================================================
    // PriceSnapshot Tests
    // ============================================================================

    #[test]
    fn test_price_snapshot() {
        let snapshot = PriceSnapshot {
            timestamp: Utc::now(),
            round_slug: "btc-updown-15m-1234567890".to_string(),
            seconds_remaining: 450,
            up_token_id: "up-token".to_string(),
            down_token_id: "down-token".to_string(),
            up_best_ask: Decimal::new(55, 2),
            up_best_bid: Decimal::new(53, 2),
            down_best_ask: Decimal::new(46, 2),
            down_best_bid: Decimal::new(44, 2),
        };

        // Verify spread exists (ask > bid)
        assert!(snapshot.up_best_ask > snapshot.up_best_bid);
        assert!(snapshot.down_best_ask > snapshot.down_best_bid);

        // Verify reasonable market (UP + DOWN close to 1)
        let mid_sum = (snapshot.up_best_ask + snapshot.up_best_bid) / Decimal::new(2, 0)
            + (snapshot.down_best_ask + snapshot.down_best_bid) / Decimal::new(2, 0);
        assert!(mid_sum < Decimal::new(105, 2)); // Should be close to 1.00
    }
}
