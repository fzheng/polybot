//! WebSocket price streaming

#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::types::{OrderBook, OrderBookLevel, PriceEntry};

/// Price update from WebSocket
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PriceUpdate {
    pub token_id: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub bid_size: Option<Decimal>,
    pub ask_size: Option<Decimal>,
    pub timestamp: chrono::DateTime<Utc>,
}

/// WebSocket subscription message (CLOB market channel format)
#[derive(Debug, Serialize)]
struct WsSubscription {
    assets_ids: Vec<String>,
    #[serde(rename = "type")]
    sub_type: String,
}

/// Incoming WebSocket message types
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WsMessage {
    event_type: Option<String>,
    asset_id: Option<String>,
    market: Option<String>,
    timestamp: Option<String>,
    hash: Option<String>,
    // For book events
    bids: Option<Vec<BookLevel>>,
    asks: Option<Vec<BookLevel>>,
    // For price_change events
    price_changes: Option<Vec<PriceChange>>,
    // For last_trade_price events
    price: Option<String>,
    side: Option<String>,
    size: Option<String>,
    // For best_bid_ask events
    best_bid: Option<String>,
    best_ask: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PriceChange {
    asset_id: String,
    price: Option<String>,
    size: Option<String>,
    side: Option<String>,
    hash: Option<String>,
    best_bid: Option<String>,
    best_ask: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BookLevel {
    price: String,
    size: String,
}

/// Price stream manager
pub struct PriceStream {
    ws_url: String,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
    price_history: Arc<RwLock<Vec<PriceEntry>>>,
    update_tx: broadcast::Sender<PriceUpdate>,
    subscribed_tokens: Arc<RwLock<Vec<String>>>,
    running: Arc<RwLock<bool>>,
}

impl PriceStream {
    /// Create a new price stream
    pub fn new(ws_url: &str) -> Self {
        let (update_tx, _) = broadcast::channel(1000);

        Self {
            ws_url: ws_url.to_string(),
            order_books: Arc::new(RwLock::new(HashMap::new())),
            price_history: Arc::new(RwLock::new(Vec::new())),
            update_tx,
            subscribed_tokens: Arc::new(RwLock::new(Vec::new())),
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Subscribe to price updates
    pub fn subscribe(&self) -> broadcast::Receiver<PriceUpdate> {
        self.update_tx.subscribe()
    }

    /// Get current order book for a token
    pub async fn get_order_book(&self, token_id: &str) -> Option<OrderBook> {
        let books = self.order_books.read().await;
        books.get(token_id).cloned()
    }

    /// Get best ask price for a token
    pub async fn get_best_ask(&self, token_id: &str) -> Option<Decimal> {
        let books = self.order_books.read().await;
        books.get(token_id).and_then(|b| b.best_ask)
    }

    /// Get best bid price for a token
    pub async fn get_best_bid(&self, token_id: &str) -> Option<Decimal> {
        let books = self.order_books.read().await;
        books.get(token_id).and_then(|b| b.best_bid)
    }

    /// Get recent price history
    pub async fn get_price_history(&self, seconds: u64) -> Vec<PriceEntry> {
        let history = self.price_history.read().await;
        let cutoff = Utc::now() - chrono::Duration::seconds(seconds as i64);

        history
            .iter()
            .filter(|e| e.timestamp >= cutoff)
            .cloned()
            .collect()
    }

    /// Start streaming prices for given tokens
    pub async fn start(&self, token_ids: Vec<String>) -> Result<()> {
        // Check if we need to restart with new tokens
        {
            let current_tokens = self.subscribed_tokens.read().await;
            let running = self.running.read().await;

            // If already running with the same tokens, no need to restart
            if *running && *current_tokens == token_ids {
                return Ok(());
            }
        }

        // Stop existing stream if running
        {
            let mut running = self.running.write().await;
            *running = false;
        }

        // Give the old task a moment to notice the stop signal
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Clear old order books and price history for fresh start
        {
            let mut books = self.order_books.write().await;
            books.clear();
        }
        {
            let mut history = self.price_history.write().await;
            history.clear();
        }

        // Update tokens and mark as running
        {
            let mut running = self.running.write().await;
            *running = true;
        }
        {
            let mut tokens = self.subscribed_tokens.write().await;
            *tokens = token_ids.clone();
        }

        let ws_url = self.ws_url.clone();
        let order_books = self.order_books.clone();
        let price_history = self.price_history.clone();
        let update_tx = self.update_tx.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            loop {
                {
                    let is_running = running.read().await;
                    if !*is_running {
                        break;
                    }
                }

                match Self::run_websocket(
                    &ws_url,
                    &token_ids,
                    order_books.clone(),
                    price_history.clone(),
                    update_tx.clone(),
                    running.clone(),
                ).await {
                    Ok(_) => {
                        tracing::info!("WebSocket closed, reconnecting...");
                    }
                    Err(e) => {
                        tracing::error!("WebSocket error: {}, reconnecting in 5s...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn run_websocket(
        ws_url: &str,
        token_ids: &[String],
        order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
        price_history: Arc<RwLock<Vec<PriceEntry>>>,
        update_tx: broadcast::Sender<PriceUpdate>,
        running: Arc<RwLock<bool>>,
    ) -> Result<()> {
        tracing::info!("Connecting to WebSocket: {}", ws_url);
        tracing::info!("Token IDs to subscribe: {:?}", token_ids);

        let (ws_stream, response) = connect_async(ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        tracing::info!("WebSocket connected! Response status: {:?}", response.status());

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to market data for each token
        // CLOB format: {"assets_ids":["token_id_1","token_id_2"],"type":"market"}
        let subscribe_msg = WsSubscription {
            assets_ids: token_ids.to_vec(),
            sub_type: "market".to_string(),
        };

        let msg = serde_json::to_string(&subscribe_msg)?;
        tracing::info!("Sending WebSocket subscription: {}", msg);
        write.send(Message::Text(msg.into())).await?;

        tracing::info!("Subscribed to {} token(s), waiting for messages...", token_ids.len());

        // Track last price entry for deduplication
        let mut last_up_ask: Option<Decimal> = None;
        let mut last_down_ask: Option<Decimal> = None;

        while let Some(msg) = read.next().await {
            {
                let is_running = running.read().await;
                if !*is_running {
                    break;
                }
            }

            match msg {
                Ok(Message::Text(text)) => {
                    // Log messages for debugging
                    tracing::debug!("WS received: {}", &text[..text.len().min(500)]);

                    // Try to parse as array first (server sends arrays of messages)
                    let messages: Vec<WsMessage> = if let Ok(arr) = serde_json::from_str::<Vec<WsMessage>>(&text) {
                        arr
                    } else if let Ok(single) = serde_json::from_str::<WsMessage>(&text) {
                        vec![single]
                    } else {
                        tracing::warn!("Failed to parse WS message: {}", &text[..text.len().min(100)]);
                        continue;
                    };

                    for ws_msg in messages {
                        let event_type = ws_msg.event_type.as_deref().unwrap_or("book");

                        match event_type {
                            "book" => {
                                // Full order book snapshot (initial or update)
                                if let Some(asset_id) = &ws_msg.asset_id {
                                    let mut best_bid = None;
                                    let mut best_ask = None;
                                    let mut bid_size = None;
                                    let mut ask_size = None;
                                    let mut ask_levels: Vec<OrderBookLevel> = Vec::new();
                                    let mut bid_levels: Vec<OrderBookLevel> = Vec::new();

                                    // Parse all bids, sort by price desc, take top 3
                                    if let Some(bids) = &ws_msg.bids {
                                        let mut parsed_bids: Vec<(Decimal, Decimal)> = bids.iter()
                                            .filter_map(|b| {
                                                let price = b.price.parse::<Decimal>().ok()?;
                                                let size = b.size.parse::<Decimal>().ok()?;
                                                Some((price, size))
                                            })
                                            .collect();
                                        // Sort by price descending (best bid = highest)
                                        parsed_bids.sort_by(|a, b| b.0.cmp(&a.0));

                                        // Take top 3 levels
                                        for (price, size) in parsed_bids.iter().take(3) {
                                            bid_levels.push(OrderBookLevel { price: *price, size: *size });
                                        }

                                        // Best bid is first
                                        if let Some(top) = parsed_bids.first() {
                                            best_bid = Some(top.0);
                                            bid_size = Some(top.1);
                                        }
                                    }

                                    // Parse all asks, sort by price asc, take top 3
                                    if let Some(asks) = &ws_msg.asks {
                                        let mut parsed_asks: Vec<(Decimal, Decimal)> = asks.iter()
                                            .filter_map(|a| {
                                                let price = a.price.parse::<Decimal>().ok()?;
                                                let size = a.size.parse::<Decimal>().ok()?;
                                                Some((price, size))
                                            })
                                            .collect();
                                        // Sort by price ascending (best ask = lowest)
                                        parsed_asks.sort_by(|a, b| a.0.cmp(&b.0));

                                        // Take top 3 levels
                                        for (price, size) in parsed_asks.iter().take(3) {
                                            ask_levels.push(OrderBookLevel { price: *price, size: *size });
                                        }

                                        // Best ask is first
                                        if let Some(top) = parsed_asks.first() {
                                            best_ask = Some(top.0);
                                            ask_size = Some(top.1);
                                        }
                                    }

                                    if best_bid.is_some() || best_ask.is_some() {
                                        tracing::info!("Book update for {}: bid={:?}, ask={:?}, ask_levels={}",
                                            &asset_id[..8], best_bid, best_ask, ask_levels.len());
                                        Self::update_book(
                                            &order_books, &update_tx, asset_id,
                                            best_bid, best_ask, bid_size, ask_size,
                                            ask_levels, bid_levels
                                        ).await;
                                    }
                                }
                            }
                            "price_change" => {
                                // Price change with best bid/ask (no depth levels)
                                if let Some(changes) = &ws_msg.price_changes {
                                    for change in changes {
                                        let best_bid = change.best_bid.as_ref()
                                            .and_then(|s| s.parse().ok());
                                        let best_ask = change.best_ask.as_ref()
                                            .and_then(|s| s.parse().ok());

                                        Self::update_book(
                                            &order_books, &update_tx, &change.asset_id,
                                            best_bid, best_ask, None, None,
                                            Vec::new(), Vec::new()
                                        ).await;
                                    }
                                }
                            }
                            "best_bid_ask" => {
                                // Direct best bid/ask update (no depth levels)
                                if let Some(asset_id) = &ws_msg.asset_id {
                                    let best_bid = ws_msg.best_bid.as_ref()
                                        .and_then(|s| s.parse().ok());
                                    let best_ask = ws_msg.best_ask.as_ref()
                                        .and_then(|s| s.parse().ok());

                                    Self::update_book(
                                        &order_books, &update_tx, asset_id,
                                        best_bid, best_ask, None, None,
                                        Vec::new(), Vec::new()
                                    ).await;
                                }
                            }
                            _ => {
                                // Ignore other event types
                                tracing::trace!("Ignoring event type: {}", event_type);
                            }
                        }
                    }
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => {
                    tracing::info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    tracing::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }

            // Record price history (every update for now)
            {
                let books = order_books.read().await;
                let up_ask = token_ids.get(0)
                    .and_then(|id| books.get(id))
                    .and_then(|b| b.best_ask);
                let down_ask = token_ids.get(1)
                    .and_then(|id| books.get(id))
                    .and_then(|b| b.best_ask);

                // Only record if we have both prices
                if let (Some(up), Some(down)) = (up_ask, down_ask) {
                    // Only record if prices changed
                    if last_up_ask != Some(up) || last_down_ask != Some(down) {
                        last_up_ask = Some(up);
                        last_down_ask = Some(down);

                        let mut history = price_history.write().await;
                        history.push(PriceEntry {
                            timestamp: Utc::now(),
                            up_ask: up,
                            down_ask: down,
                        });

                        // Keep only last 5 minutes of history
                        let cutoff = Utc::now() - chrono::Duration::minutes(5);
                        history.retain(|e| e.timestamp >= cutoff);
                    }
                }
            }
        }

        Ok(())
    }

    /// Helper to update order book and send notification
    async fn update_book(
        order_books: &Arc<RwLock<HashMap<String, OrderBook>>>,
        update_tx: &broadcast::Sender<PriceUpdate>,
        asset_id: &str,
        best_bid: Option<Decimal>,
        best_ask: Option<Decimal>,
        bid_size: Option<Decimal>,
        ask_size: Option<Decimal>,
        ask_levels: Vec<OrderBookLevel>,
        bid_levels: Vec<OrderBookLevel>,
    ) {
        // Update order book
        let (final_bid_size, final_ask_size) = {
            let mut books = order_books.write().await;
            let book = books.entry(asset_id.to_string()).or_default();

            // Update bid price and size (only update size if provided)
            if best_bid.is_some() {
                book.best_bid = best_bid;
                if bid_size.is_some() {
                    book.bid_size = bid_size;
                }
            }

            // Update ask price and size (only update size if provided)
            if best_ask.is_some() {
                book.best_ask = best_ask;
                if ask_size.is_some() {
                    book.ask_size = ask_size;
                }
            }

            // Update depth levels (only if provided - book events have full depth)
            if !ask_levels.is_empty() {
                book.ask_levels = ask_levels;
            }
            if !bid_levels.is_empty() {
                book.bid_levels = bid_levels;
            }

            (book.bid_size, book.ask_size)
        };

        // Send update notification with current sizes
        let update = PriceUpdate {
            token_id: asset_id.to_string(),
            best_bid,
            best_ask,
            bid_size: final_bid_size,
            ask_size: final_ask_size,
            timestamp: Utc::now(),
        };
        let _ = update_tx.send(update);
    }

    /// Stop the price stream
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    /// Check if stream is running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}
