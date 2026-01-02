//! Polymarket REST API client

#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Timelike, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::Config;
use crate::types::{BuySell, MarketInfo, OrderType, TradeResult, TradeStatus};

/// Polymarket API client
pub struct PolymarketClient {
    client: Client,
    clob_endpoint: String,
    gamma_endpoint: String,
    api_key: Option<String>,
    api_secret: Option<String>,
    api_passphrase: Option<String>,
    #[allow(dead_code)]
    private_key: Option<String>,
    #[allow(dead_code)]
    funder: Option<String>,
    #[allow(dead_code)]
    signature_type: u8,
}

/// Response from markets endpoint
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MarketsResponse {
    data: Vec<MarketData>,
    #[serde(default)]
    next_cursor: Option<String>,
}

/// Event data from the events endpoint
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EventData {
    slug: String,
    title: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    markets: Vec<MarketData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MarketData {
    #[serde(rename = "conditionId")]
    condition_id: String,
    #[serde(rename = "questionID")]
    question_id: Option<String>,
    question: String,
    slug: String,
    /// Token IDs as JSON array string e.g. "[\"abc\", \"def\"]"
    #[serde(rename = "clobTokenIds", default)]
    clob_token_ids: Option<String>,
    /// Outcomes as JSON array string e.g. "[\"Yes\", \"No\"]"
    #[serde(default)]
    outcomes: Option<String>,
    /// Outcome prices as JSON array string e.g. "[\"0\", \"1\"]" - winner has price "1"
    #[serde(rename = "outcomePrices", default)]
    outcome_prices: Option<String>,
    #[serde(rename = "endDateIso")]
    end_date_iso: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
    #[serde(rename = "startDateIso")]
    start_date_iso: Option<String>,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    closed: bool,
}

/// Order book response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OrderBookResponse {
    pub market: String,
    pub asset_id: String,
    pub bids: Vec<OrderBookLevel>,
    pub asks: Vec<OrderBookLevel>,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OrderBookLevel {
    pub price: String,
    pub size: String,
}

/// Price response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PriceResponse {
    pub mid: Option<String>,
    pub bid: Option<String>,
    pub ask: Option<String>,
}

/// Order request
#[derive(Debug, Serialize)]
struct OrderRequest {
    token_id: String,
    price: String,
    size: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<String>,
}

/// Order response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OrderResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    size_matched: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    error_msg: Option<String>,
}

/// Token position from the API
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TokenPosition {
    /// The token ID
    #[serde(rename = "asset_id", default)]
    pub token_id: String,
    /// Number of shares held
    #[serde(default)]
    pub size: Decimal,
    /// Average entry price
    #[serde(rename = "avg_price", default)]
    pub avg_price: Decimal,
    /// Current market price
    #[serde(rename = "market_price", default)]
    pub market_price: Option<Decimal>,
    /// Unrealized P&L
    #[serde(rename = "unrealized_pnl", default)]
    pub unrealized_pnl: Option<Decimal>,
}

impl PolymarketClient {
    /// Create a new client
    pub fn new(config: &Config) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            clob_endpoint: config.api.clob_endpoint.clone(),
            gamma_endpoint: config.api.gamma_endpoint.clone(),
            api_key: None,
            api_secret: None,
            api_passphrase: None,
            private_key: config.api.private_key.clone(),
            funder: config.api.funder_address.clone(),
            signature_type: config.api.signature_type,
        }
    }

    /// Check if API is reachable
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/", self.clob_endpoint);
        let resp = self.client.get(&url).send().await?;
        Ok(resp.status().is_success())
    }

    /// Authenticate and derive API credentials
    pub async fn authenticate(&mut self) -> Result<()> {
        let _private_key = self.private_key.as_ref()
            .context("Private key not configured")?;

        // For now, we'll use a simplified auth flow
        // In production, this would use EIP-712 signing
        tracing::info!("Authenticating with Polymarket API...");

        // Derive API key endpoint
        let _url = format!("{}/auth/derive-api-key", self.clob_endpoint);

        // Note: Full implementation would sign with the private key
        // This is a placeholder for the authentication flow
        tracing::warn!("Full authentication not yet implemented - using read-only mode");

        Ok(())
    }

    /// Get current BTC 15-minute UP/DOWN market
    pub async fn get_btc_market(&self) -> Result<Option<MarketInfo>> {
        // Calculate the expected slug based on current time
        // BTC 15-min markets run from :00-:15, :15-:30, :30-:45, :45-:00
        // The slug contains the Unix timestamp of the START time
        // So at 6:04 UTC, we look for the market that started at 6:00 UTC
        let now = Utc::now();
        let current_minute = now.minute();

        // Find the most recent 15-minute boundary (start time of current round)
        let minutes_since_boundary = current_minute % 15;
        let start_time = now - chrono::Duration::minutes(minutes_since_boundary as i64);
        // Truncate to exact minute
        let start_time = start_time
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        let end_time = start_time + chrono::Duration::minutes(15);

        let start_timestamp = start_time.timestamp();
        let expected_slug = format!("btc-updown-15m-{}", start_timestamp);

        tracing::debug!(
            "Current time: {}, expecting market slug: {} (starts at {}, ends at {})",
            now.format("%H:%M:%S"),
            expected_slug,
            start_time.format("%H:%M:%S"),
            end_time.format("%H:%M:%S")
        );

        // Query directly for this specific market
        let url = format!(
            "{}/events?slug={}",
            self.gamma_endpoint,
            expected_slug
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            tracing::debug!("Market {} not found, trying search", expected_slug);
            return Ok(None);
        }

        let events: Vec<EventData> = resp.json().await?;

        if let Some(event) = events.into_iter().next() {
            if let Some(market) = event.markets.into_iter().next() {
                if let Some(market_info) = self.parse_btc_market(&market) {
                    tracing::info!(
                        "Found market: {} (ends at {}, {} seconds remaining)",
                        market_info.slug,
                        market_info.end_time.format("%H:%M:%S"),
                        market_info.seconds_remaining()
                    );
                    return Ok(Some(market_info));
                }
            }
        }

        tracing::debug!("No active BTC 15-minute market found for slug {}", expected_slug);
        Ok(None)
    }

    /// Get the next BTC 15-minute UP/DOWN market (the one starting after current round ends)
    /// This is used for early switching after completing an arbitrage cycle
    pub async fn get_next_btc_market(&self) -> Result<Option<MarketInfo>> {
        let now = Utc::now();
        let current_minute = now.minute();

        // Find the next 15-minute boundary (start time of next round)
        let minutes_until_boundary = 15 - (current_minute % 15);
        let next_start_time = now + chrono::Duration::minutes(minutes_until_boundary as i64);
        // Truncate to exact minute
        let next_start_time = next_start_time
            .with_second(0).unwrap()
            .with_nanosecond(0).unwrap();
        let next_end_time = next_start_time + chrono::Duration::minutes(15);

        let next_start_timestamp = next_start_time.timestamp();
        let expected_slug = format!("btc-updown-15m-{}", next_start_timestamp);

        tracing::debug!(
            "Looking for next market slug: {} (starts at {}, ends at {})",
            expected_slug,
            next_start_time.format("%H:%M:%S"),
            next_end_time.format("%H:%M:%S")
        );

        // Query directly for this specific market
        let url = format!(
            "{}/events?slug={}",
            self.gamma_endpoint,
            expected_slug
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            tracing::debug!("Next market {} not found yet", expected_slug);
            return Ok(None);
        }

        let events: Vec<EventData> = resp.json().await?;

        if let Some(event) = events.into_iter().next() {
            if let Some(market) = event.markets.into_iter().next() {
                if let Some(market_info) = self.parse_btc_market(&market) {
                    tracing::info!(
                        "Found next market: {} (starts at {}, ends at {})",
                        market_info.slug,
                        market_info.start_time.format("%H:%M:%S"),
                        market_info.end_time.format("%H:%M:%S")
                    );
                    return Ok(Some(market_info));
                }
            }
        }

        tracing::debug!("No next BTC 15-minute market found for slug {}", expected_slug);
        Ok(None)
    }

    /// Get market info by slug (for settlement of past rounds)
    pub async fn get_market_info_by_slug(&self, slug: &str) -> Result<Option<MarketInfo>> {
        let url = format!(
            "{}/events?slug={}",
            self.gamma_endpoint,
            slug
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let events: Vec<EventData> = resp.json().await?;

        if let Some(event) = events.into_iter().next() {
            if let Some(market) = event.markets.into_iter().next() {
                if let Some(market_info) = self.parse_btc_market(&market) {
                    return Ok(Some(market_info));
                }
            }
        }

        Ok(None)
    }

    /// Search for markets by text query
    pub async fn search_markets(&self, query: &str) -> Result<Vec<MarketInfo>> {
        let url = format!(
            "{}/markets?active=true&closed=false&_q={}",
            self.gamma_endpoint,
            urlencoding::encode(query)
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to search markets: {}", resp.status());
        }

        let markets: Vec<MarketData> = resp.json().await?;

        let mut results = Vec::new();
        for market in markets {
            if let Some(info) = self.parse_btc_market(&market) {
                results.push(info);
            }
        }

        Ok(results)
    }

    fn parse_btc_market(&self, market: &MarketData) -> Option<MarketInfo> {
        // Parse token IDs from JSON string (e.g. "[\"abc\", \"def\"]")
        let token_ids: Vec<String> = market.clob_token_ids.as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // Parse outcomes from JSON string (e.g. "[\"Yes\", \"No\"]" or "[\"Up\", \"Down\"]")
        let outcomes: Vec<String> = market.outcomes.as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        if token_ids.len() != 2 || outcomes.len() != 2 {
            return None;
        }

        // Match outcomes to tokens
        let mut up_token_id = None;
        let mut down_token_id = None;

        for (i, outcome) in outcomes.iter().enumerate() {
            let outcome_lower = outcome.to_lowercase();
            if outcome_lower.contains("up") || outcome_lower == "yes" {
                up_token_id = Some(token_ids[i].clone());
            } else if outcome_lower.contains("down") || outcome_lower == "no" {
                down_token_id = Some(token_ids[i].clone());
            }
        }

        let (up_token_id, down_token_id) = match (up_token_id, down_token_id) {
            (Some(up), Some(down)) => (up, down),
            _ => return None,
        };

        // Parse start time from slug timestamp (slug format: btc-updown-15m-{start_timestamp})
        let start_time = market.start_date_iso.as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| {
                // Extract Unix timestamp from slug like "btc-updown-15m-1767246300"
                // This timestamp represents the START time of the 15-minute window
                market.slug.split('-').last()
                    .and_then(|ts| ts.parse::<i64>().ok())
                    .and_then(|ts| DateTime::from_timestamp(ts, 0))
                    .map(|dt| dt.with_timezone(&Utc))
            })
            .unwrap_or_else(|| Utc::now());

        // End time is 15 minutes after start time
        let end_time = market.end_date_iso.as_ref()
            .or(market.end_date.as_ref())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| start_time + chrono::Duration::minutes(15));

        Some(MarketInfo {
            condition_id: market.condition_id.clone(),
            question_id: market.question_id.clone().unwrap_or_default(),
            slug: market.slug.clone(),
            up_token_id,
            down_token_id,
            end_time,
            start_time,
        })
    }

    /// Get order book for a token
    pub async fn get_order_book(&self, token_id: &str) -> Result<OrderBookResponse> {
        let url = format!("{}/book?token_id={}", self.clob_endpoint, token_id);

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to get order book: {}", resp.status());
        }

        resp.json().await.context("Failed to parse order book")
    }

    /// Get best bid/ask prices for a token
    pub async fn get_price(&self, token_id: &str) -> Result<(Option<Decimal>, Option<Decimal>)> {
        let book = self.get_order_book(token_id).await?;

        let best_bid = book.bids.first()
            .and_then(|l| l.price.parse().ok());
        let best_ask = book.asks.first()
            .and_then(|l| l.price.parse().ok());

        Ok((best_bid, best_ask))
    }

    /// Get prices for multiple tokens
    pub async fn get_prices(&self, token_ids: &[&str]) -> Result<HashMap<String, (Option<Decimal>, Option<Decimal>)>> {
        let mut prices = HashMap::new();

        // Fetch prices concurrently
        let futures: Vec<_> = token_ids.iter()
            .map(|id| self.get_price(id))
            .collect();

        let results = futures::future::join_all(futures).await;

        for (id, result) in token_ids.iter().zip(results) {
            if let Ok(price) = result {
                prices.insert(id.to_string(), price);
            }
        }

        Ok(prices)
    }

    /// Place a limit order
    pub async fn place_limit_order(
        &self,
        token_id: &str,
        side: BuySell,
        price: Decimal,
        size: Decimal,
        order_type: OrderType,
    ) -> Result<TradeResult> {
        if self.api_key.is_none() {
            anyhow::bail!("Not authenticated - cannot place orders");
        }

        let order_type_str = match order_type {
            OrderType::Gtc => "GTC",
            OrderType::Fok => "FOK",
            OrderType::Ioc => "IOC",
        };

        let side_str = match side {
            BuySell::Buy => "BUY",
            BuySell::Sell => "SELL",
        };

        let request = OrderRequest {
            token_id: token_id.to_string(),
            price: price.to_string(),
            size: size.to_string(),
            side: side_str.to_string(),
            order_type: order_type_str.to_string(),
            expiration: None,
        };

        let url = format!("{}/order", self.clob_endpoint);

        let resp = self.client
            .post(&url)
            .header("POLY-API-KEY", self.api_key.as_deref().unwrap_or(""))
            .header("POLY-SECRET", self.api_secret.as_deref().unwrap_or(""))
            .header("POLY-PASSPHRASE", self.api_passphrase.as_deref().unwrap_or(""))
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to place order: {}", body);
        }

        let order_resp: OrderResponse = resp.json().await?;

        let status = match order_resp.status.as_str() {
            "matched" | "filled" => TradeStatus::Filled,
            "partial" => TradeStatus::PartiallyFilled,
            "live" | "open" => TradeStatus::Pending,
            _ => TradeStatus::Failed,
        };

        Ok(TradeResult {
            order_id: order_resp.id,
            token_id: token_id.to_string(),
            side,
            price,
            size,
            filled_size: order_resp.size_matched.parse().unwrap_or(Decimal::ZERO),
            status,
            timestamp: Utc::now(),
        })
    }

    /// Place a market buy order (uses best ask price)
    pub async fn buy_at_ask(
        &self,
        token_id: &str,
        size: Decimal,
    ) -> Result<TradeResult> {
        let (_, best_ask) = self.get_price(token_id).await?;

        let price = best_ask.context("No ask price available")?;

        self.place_limit_order(token_id, BuySell::Buy, price, size, OrderType::Gtc).await
    }

    /// Buy shares worth a specific USD amount
    pub async fn buy_usd_amount(
        &self,
        token_id: &str,
        usd_amount: Decimal,
    ) -> Result<TradeResult> {
        let (_, best_ask) = self.get_price(token_id).await?;
        let price = best_ask.context("No ask price available")?;

        // Calculate number of shares
        let shares = usd_amount / price;

        self.buy_at_ask(token_id, shares).await
    }

    /// Cancel an order
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        if self.api_key.is_none() {
            anyhow::bail!("Not authenticated - cannot cancel orders");
        }

        let url = format!("{}/order/{}", self.clob_endpoint, order_id);

        let resp = self.client
            .delete(&url)
            .header("POLY-API-KEY", self.api_key.as_deref().unwrap_or(""))
            .header("POLY-SECRET", self.api_secret.as_deref().unwrap_or(""))
            .header("POLY-PASSPHRASE", self.api_passphrase.as_deref().unwrap_or(""))
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to cancel order: {}", body);
        }

        Ok(())
    }

    /// Check if client is authenticated
    pub fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    /// Get account balance (USDC.e)
    /// Returns the available USDC.e balance for trading
    pub async fn get_balance(&self) -> Result<Decimal> {
        if self.api_key.is_none() {
            anyhow::bail!("Not authenticated - cannot get balance");
        }

        let url = format!("{}/balance", self.clob_endpoint);

        let resp = self.client
            .get(&url)
            .header("POLY-API-KEY", self.api_key.as_deref().unwrap_or(""))
            .header("POLY-SECRET", self.api_secret.as_deref().unwrap_or(""))
            .header("POLY-PASSPHRASE", self.api_passphrase.as_deref().unwrap_or(""))
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get balance: {}", body);
        }

        // Response is like: { "balance": "1234.56" }
        let balance_resp: serde_json::Value = resp.json().await?;
        let balance_str = balance_resp["balance"]
            .as_str()
            .unwrap_or("0");

        balance_str.parse::<Decimal>()
            .context("Failed to parse balance")
    }

    /// Get all open positions
    /// Returns a list of token positions with sizes
    pub async fn get_positions(&self) -> Result<Vec<TokenPosition>> {
        if self.api_key.is_none() {
            anyhow::bail!("Not authenticated - cannot get positions");
        }

        let url = format!("{}/positions", self.clob_endpoint);

        let resp = self.client
            .get(&url)
            .header("POLY-API-KEY", self.api_key.as_deref().unwrap_or(""))
            .header("POLY-SECRET", self.api_secret.as_deref().unwrap_or(""))
            .header("POLY-PASSPHRASE", self.api_passphrase.as_deref().unwrap_or(""))
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get positions: {}", body);
        }

        let positions: Vec<TokenPosition> = resp.json().await?;
        Ok(positions)
    }

    /// Get market outcome (resolution) for a closed market
    /// Returns Some(Side) if the market is resolved, None if still open
    pub async fn get_market_outcome(&self, slug: &str) -> Result<Option<crate::types::Side>> {
        let url = format!("{}/events?slug={}", self.gamma_endpoint, slug);

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let events: Vec<EventData> = resp.json().await?;

        if let Some(event) = events.into_iter().next() {
            if let Some(market) = event.markets.into_iter().next() {
                // Check if market is closed
                if !market.closed {
                    return Ok(None);
                }

                // Parse outcomes and outcomePrices
                let outcomes: Vec<String> = market.outcomes.as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();

                // outcomePrices is like "[\"0\", \"1\"]" - the winning outcome has price "1"
                let outcome_prices: Vec<String> = market.outcome_prices.as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();

                if outcomes.len() != outcome_prices.len() || outcomes.is_empty() {
                    return Ok(None);
                }

                // Find which outcome has price "1" (the winner)
                for (i, price) in outcome_prices.iter().enumerate() {
                    if price == "1" {
                        let outcome = &outcomes[i];
                        let outcome_lower = outcome.to_lowercase();
                        if outcome_lower.contains("up") || outcome_lower == "yes" {
                            return Ok(Some(crate::types::Side::Up));
                        } else if outcome_lower.contains("down") || outcome_lower == "no" {
                            return Ok(Some(crate::types::Side::Down));
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}
