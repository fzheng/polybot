//! Polymarket REST API client

#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MarketData {
    condition_id: String,
    question_id: String,
    question: String,
    slug: String,
    tokens: Vec<TokenData>,
    end_date_iso: Option<String>,
    game_start_time: Option<String>,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    closed: bool,
}

#[derive(Debug, Deserialize)]
struct TokenData {
    token_id: String,
    outcome: String,
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

impl PolymarketClient {
    /// Create a new client
    pub fn new(config: &Config) -> Self {
        Self {
            client: Client::new(),
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
        // Search for active BTC 15-minute markets
        let url = format!(
            "{}/markets?active=true&closed=false&limit=100",
            self.gamma_endpoint
        );

        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to get markets: {} - {}", status, body);
        }

        let markets: Vec<MarketData> = resp.json().await?;

        // Find BTC 15-minute UP/DOWN market
        for market in markets {
            let slug_lower = market.slug.to_lowercase();
            if slug_lower.contains("bitcoin") && slug_lower.contains("15") {
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
        let mut up_token_id = None;
        let mut down_token_id = None;

        for token in &market.tokens {
            let outcome_lower = token.outcome.to_lowercase();
            if outcome_lower.contains("up") || outcome_lower == "yes" {
                up_token_id = Some(token.token_id.clone());
            } else if outcome_lower.contains("down") || outcome_lower == "no" {
                down_token_id = Some(token.token_id.clone());
            }
        }

        let (up_token_id, down_token_id) = match (up_token_id, down_token_id) {
            (Some(up), Some(down)) => (up, down),
            _ => return None,
        };

        // Parse end time
        let end_time = market.end_date_iso.as_ref()
            .or(market.game_start_time.as_ref())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(15));

        // Estimate start time (15 minutes before end for 15-min markets)
        let start_time = end_time - chrono::Duration::minutes(15);

        Some(MarketInfo {
            condition_id: market.condition_id.clone(),
            question_id: market.question_id.clone(),
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
}
