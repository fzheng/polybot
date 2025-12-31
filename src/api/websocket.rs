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

use crate::types::{OrderBook, PriceEntry};

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

/// WebSocket message types
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum WsRequest {
    #[serde(rename = "subscribe")]
    Subscribe { channel: String, assets_ids: Vec<String> },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { channel: String, assets_ids: Vec<String> },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WsMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    channel: Option<String>,
    asset_id: Option<String>,
    data: Option<WsData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WsData {
    bids: Option<Vec<BookLevel>>,
    asks: Option<Vec<BookLevel>>,
    price: Option<String>,
    side: Option<String>,
    size: Option<String>,
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
        {
            let mut running = self.running.write().await;
            if *running {
                return Ok(());
            }
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
        let (ws_stream, _) = connect_async(ws_url)
            .await
            .context("Failed to connect to WebSocket")?;

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to price updates
        let subscribe_msg = WsRequest::Subscribe {
            channel: "book".to_string(),
            assets_ids: token_ids.to_vec(),
        };

        let msg = serde_json::to_string(&subscribe_msg)?;
        write.send(Message::Text(msg.into())).await?;

        tracing::info!("Subscribed to {} token(s)", token_ids.len());

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
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        if let Some(asset_id) = &ws_msg.asset_id {
                            if let Some(data) = &ws_msg.data {
                                let mut best_bid = None;
                                let mut best_ask = None;
                                let mut bid_size = None;
                                let mut ask_size = None;

                                if let Some(bids) = &data.bids {
                                    if let Some(top) = bids.first() {
                                        best_bid = top.price.parse().ok();
                                        bid_size = top.size.parse().ok();
                                    }
                                }

                                if let Some(asks) = &data.asks {
                                    if let Some(top) = asks.first() {
                                        best_ask = top.price.parse().ok();
                                        ask_size = top.size.parse().ok();
                                    }
                                }

                                // Update order book
                                {
                                    let mut books = order_books.write().await;
                                    let book = books.entry(asset_id.clone()).or_default();
                                    if best_bid.is_some() {
                                        book.best_bid = best_bid;
                                        book.bid_size = bid_size;
                                    }
                                    if best_ask.is_some() {
                                        book.best_ask = best_ask;
                                        book.ask_size = ask_size;
                                    }
                                }

                                // Send update
                                let update = PriceUpdate {
                                    token_id: asset_id.clone(),
                                    best_bid,
                                    best_ask,
                                    bid_size,
                                    ask_size,
                                    timestamp: Utc::now(),
                                };
                                let _ = update_tx.send(update);
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
