//! Market monitoring and tracking.
//!
//! This module provides the [`MarketWatcher`] which:
//! - Discovers active BTC 15-minute UP/DOWN markets
//! - Tracks the current round and handles round transitions
//! - Streams real-time prices via WebSocket
//! - Detects price dumps for strategy triggering
//! - Maintains price history for dump detection

mod watcher;

pub use watcher::MarketWatcher;
