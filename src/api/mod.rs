//! Polymarket API integration.
//!
//! This module provides REST and WebSocket clients for interacting
//! with Polymarket's trading infrastructure:
//!
//! - [`PolymarketClient`] - REST API client for market data and order placement
//! - [`PriceStream`] - WebSocket client for real-time price updates
//! - [`PriceUpdate`] - Price update events from the WebSocket
//!
//! # API Endpoints
//!
//! - **CLOB API**: `https://clob.polymarket.com` - Order placement and book data
//! - **Gamma API**: `https://gamma-api.polymarket.com` - Market metadata
//! - **WebSocket**: `wss://ws-subscriptions-clob.polymarket.com/ws/market` - Real-time updates

mod client;
mod websocket;

pub use client::PolymarketClient;
pub use websocket::{PriceStream, PriceUpdate};
