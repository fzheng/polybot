//! Market watcher for BTC 15-minute UP/DOWN rounds

#![allow(dead_code)]

use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::api::{PolymarketClient, PriceStream, PriceUpdate};
use crate::config::Config;
use crate::types::{MarketInfo, OrderBook, PriceEntry, PriceSnapshot, Side};

/// Market watcher that tracks the current BTC 15-minute round
pub struct MarketWatcher {
    client: PolymarketClient,
    price_stream: PriceStream,
    current_market: Arc<RwLock<Option<MarketInfo>>>,
    config: Config,
}

impl MarketWatcher {
    /// Create a new market watcher
    pub fn new(config: Config) -> Self {
        let client = PolymarketClient::new(&config);
        let price_stream = PriceStream::new(&config.api.ws_endpoint);

        Self {
            client,
            price_stream,
            current_market: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// Initialize and start watching
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("Searching for active BTC markets...");

        // Find current BTC market
        self.refresh_market().await?;

        // Start price stream if we have a market
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            tracing::info!("Starting price stream for market: {}", m.slug);
            tracing::info!("UP token ID: {}", m.up_token_id);
            tracing::info!("DOWN token ID: {}", m.down_token_id);
            let tokens = vec![m.up_token_id.clone(), m.down_token_id.clone()];
            self.price_stream.start(tokens).await?;
        } else {
            tracing::warn!("No active BTC 15-minute market found - will retry on tick");
        }

        Ok(())
    }

    /// Refresh the current market (find new round if needed)
    pub async fn refresh_market(&mut self) -> Result<bool> {
        // Check if current market is still valid (active OR hasn't started yet but not ended)
        // This allows us to switch to the next market early and stay on it
        {
            let market = self.current_market.read().await;
            if let Some(m) = market.as_ref() {
                let now = Utc::now();
                // Keep current market if it hasn't ended yet (even if it hasn't started)
                if now < m.end_time {
                    return Ok(false); // No change needed
                }
            }
        }

        // Find new market
        let new_market = self.client.get_btc_market().await?;

        if let Some(m) = &new_market {
            tracing::info!(
                "Found BTC market: {} (ends at {})",
                m.slug,
                m.end_time.format("%H:%M:%S")
            );

            // Update price stream with new tokens
            let tokens = vec![m.up_token_id.clone(), m.down_token_id.clone()];
            self.price_stream.start(tokens).await?;
        }

        let mut current = self.current_market.write().await;
        let changed = new_market.is_some() != current.is_some()
            || new_market.as_ref().map(|m| &m.slug) != current.as_ref().map(|m| &m.slug);

        *current = new_market;

        Ok(changed)
    }

    /// Get current market info
    pub async fn get_current_market(&self) -> Option<MarketInfo> {
        self.current_market.read().await.clone()
    }

    /// Get seconds remaining in current round
    pub async fn get_seconds_remaining(&self) -> Option<i64> {
        self.current_market.read().await.as_ref().map(|m| m.seconds_remaining())
    }

    /// Get current UP price (best ask)
    pub async fn get_up_price(&self) -> Option<Decimal> {
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            self.price_stream.get_best_ask(&m.up_token_id).await
        } else {
            None
        }
    }

    /// Get current DOWN price (best ask)
    pub async fn get_down_price(&self) -> Option<Decimal> {
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            self.price_stream.get_best_ask(&m.down_token_id).await
        } else {
            None
        }
    }

    /// Get UP order book
    pub async fn get_up_book(&self) -> Option<OrderBook> {
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            self.price_stream.get_order_book(&m.up_token_id).await
        } else {
            None
        }
    }

    /// Get DOWN order book
    pub async fn get_down_book(&self) -> Option<OrderBook> {
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            self.price_stream.get_order_book(&m.down_token_id).await
        } else {
            None
        }
    }

    /// Get current price snapshot for recording
    pub async fn get_snapshot(&self) -> Option<PriceSnapshot> {
        let market = self.current_market.read().await;
        let m = market.as_ref()?;

        let up_book = self.price_stream.get_order_book(&m.up_token_id).await?;
        let down_book = self.price_stream.get_order_book(&m.down_token_id).await?;

        Some(PriceSnapshot {
            timestamp: Utc::now(),
            round_slug: m.slug.clone(),
            seconds_remaining: m.seconds_remaining() as u32,
            up_token_id: m.up_token_id.clone(),
            down_token_id: m.down_token_id.clone(),
            up_best_ask: up_book.best_ask.unwrap_or(Decimal::ZERO),
            up_best_bid: up_book.best_bid.unwrap_or(Decimal::ZERO),
            down_best_ask: down_book.best_ask.unwrap_or(Decimal::ZERO),
            down_best_bid: down_book.best_bid.unwrap_or(Decimal::ZERO),
        })
    }

    /// Get price history for dump detection
    pub async fn get_price_history(&self, seconds: u64) -> Vec<PriceEntry> {
        self.price_stream.get_price_history(seconds).await
    }

    /// Detect price dump in the given time window
    /// Returns (side_that_dumped, dump_percentage) if dump detected
    pub async fn detect_dump(&self, window_secs: u64, threshold_pct: Decimal) -> Option<(Side, Decimal)> {
        let history = self.get_price_history(window_secs).await;

        if history.len() < 2 {
            return None;
        }

        let first = history.first()?;
        let last = history.last()?;

        // Calculate UP price change
        if first.up_ask > Decimal::ZERO {
            let up_change = (first.up_ask - last.up_ask) / first.up_ask;
            if up_change >= threshold_pct {
                return Some((Side::Up, up_change));
            }
        }

        // Calculate DOWN price change
        if first.down_ask > Decimal::ZERO {
            let down_change = (first.down_ask - last.down_ask) / first.down_ask;
            if down_change >= threshold_pct {
                return Some((Side::Down, down_change));
            }
        }

        None
    }

    /// Check if we're within the allowed window for Leg 1
    pub async fn is_within_window(&self, window_min: u32) -> bool {
        let market = self.current_market.read().await;
        if let Some(m) = market.as_ref() {
            let elapsed = (Utc::now() - m.start_time).num_minutes();
            elapsed < window_min as i64
        } else {
            false
        }
    }

    /// Get the next market (the one that starts when current ends)
    /// Used for early switching after hedge completion
    pub async fn get_next_market(&self) -> Option<MarketInfo> {
        match self.client.get_next_btc_market().await {
            Ok(market) => market,
            Err(e) => {
                tracing::warn!("Failed to fetch next market: {}", e);
                None
            }
        }
    }

    /// Switch to the next market immediately (for early entry after hedge completion)
    /// Returns true if successfully switched to next market
    pub async fn switch_to_next_market(&mut self) -> bool {
        match self.client.get_next_btc_market().await {
            Ok(Some(next_market)) => {
                tracing::info!(
                    "Switching to next market: {} (starts at {})",
                    next_market.slug,
                    next_market.start_time.format("%H:%M:%S")
                );

                // Update price stream with new tokens
                let tokens = vec![
                    next_market.up_token_id.clone(),
                    next_market.down_token_id.clone(),
                ];
                if let Err(e) = self.price_stream.start(tokens).await {
                    tracing::error!("Failed to start price stream for next market: {}", e);
                    return false;
                }

                // Update current market
                let mut current = self.current_market.write().await;
                *current = Some(next_market);
                true
            }
            Ok(None) => {
                tracing::debug!("Next market not yet available");
                false
            }
            Err(e) => {
                tracing::warn!("Failed to fetch next market: {}", e);
                false
            }
        }
    }

    /// Get the API client for trading
    pub fn client(&self) -> &PolymarketClient {
        &self.client
    }

    /// Get mutable client for authentication
    pub fn client_mut(&mut self) -> &mut PolymarketClient {
        &mut self.client
    }

    /// Subscribe to price updates
    pub fn subscribe_prices(&self) -> tokio::sync::broadcast::Receiver<PriceUpdate> {
        self.price_stream.subscribe()
    }

    /// Stop watching
    pub async fn stop(&self) {
        self.price_stream.stop().await;
    }
}
