//! Live trading module using the official Polymarket SDK.
//!
//! This module provides a wrapper around the `polymarket-client-sdk` for
//! executing real orders on Polymarket. It handles:
//!
//! - **Order signing**: Uses EIP-712 typed data signing via the SDK
//! - **FOK orders**: Fill-or-Kill orders for guaranteed immediate fills
//! - **GTC orders**: Good-Till-Cancelled limit orders that rest on the book
//! - **Fill verification**: Tracks actual filled amounts and transaction hashes
//!
//! # Architecture
//!
//! The `LiveTrader` wraps the SDK's `Client` with a simplified interface:
//!
//! ```text
//! LiveTrader
//!   ├── Lazy authentication (on first order)
//!   ├── Order building (limit_order builder pattern)
//!   ├── Order signing (EIP-712 via signer)
//!   └── Order posting & response parsing
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // Create trader with private key
//! let trader = LiveTrader::new(private_key).await?;
//!
//! // Place a Fill-or-Kill buy order
//! let result = trader.buy_fok(token_id, size, price).await?;
//!
//! // Check if order filled
//! if result.filled {
//!     println!("Filled {} shares", result.filled_size);
//! }
//! ```
//!
//! # Order Types
//!
//! - **FOK (Fill-or-Kill)**: Must fill entirely and immediately, or is rejected.
//!   Best for the arbitrage strategy where we need certainty of execution.
//!
//! - **GTC (Good-Till-Cancelled)**: Rests on the order book until filled or cancelled.
//!   Useful for less time-sensitive orders.

use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use anyhow::{Context, Result};
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::clob::types::{OrderType as SdkOrderType, Side as SdkSide};
use polymarket_client_sdk::clob::{Client, Config};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Constants
// ============================================================================

/// Polymarket CLOB API endpoint for mainnet
const CLOB_ENDPOINT: &str = "https://clob.polymarket.com";

/// Polygon mainnet chain ID
const POLYGON_CHAIN_ID: u64 = 137;

// ============================================================================
// Type Aliases
// ============================================================================

/// Type alias for the authenticated Polymarket client.
/// This simplifies the complex generic type from the SDK.
type AuthenticatedClient = Client<Authenticated<Normal>>;

// ============================================================================
// Result Types
// ============================================================================

/// Result of a live trade execution.
///
/// Contains all information about an order's execution status,
/// including fill amounts and any error messages.
#[derive(Debug, Clone)]
pub struct LiveTradeResult {
    /// Unique order ID assigned by the exchange
    pub order_id: String,

    /// Whether the order was fully filled (for FOK) or partially filled
    pub filled: bool,

    /// Actual filled size in shares (may be 0 if FOK was rejected)
    pub filled_size: Decimal,

    /// Originally requested size
    pub requested_size: Decimal,

    /// Price at which the order was placed
    pub price: Decimal,

    /// Blockchain transaction hashes if the order was filled on-chain
    #[allow(dead_code)]
    pub tx_hashes: Vec<String>,

    /// Error message from the exchange, if any
    pub error: Option<String>,
}

/// Current status of an order.
///
/// Used for tracking GTC orders that may not fill immediately.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OrderStatus {
    /// The order's unique identifier
    pub order_id: String,

    /// Amount that has been filled so far
    pub filled_size: Decimal,

    /// Original order size
    pub original_size: Decimal,

    /// Order price
    pub price: Decimal,

    /// Whether the order is still active in the book
    pub is_open: bool,

    /// Whether the order has been completely filled
    pub is_filled: bool,
}

// ============================================================================
// LiveTrader Implementation
// ============================================================================

/// Live trader using the official Polymarket SDK.
///
/// Provides a high-level interface for executing orders on Polymarket's
/// Central Limit Order Book (CLOB). Handles authentication, order building,
/// signing, and response parsing.
///
/// # Thread Safety
///
/// `LiveTrader` is designed to be shared across async tasks. The internal
/// authenticated client is protected by `RwLock` for safe concurrent access.
///
/// # Authentication
///
/// Authentication with the Polymarket API happens lazily on the first order.
/// This allows creating the trader early without network calls, and handles
/// cases where the API may be temporarily unavailable.
pub struct LiveTrader {
    /// Hex-encoded private key (without 0x prefix)
    #[allow(dead_code)]
    private_key: String,

    /// The authenticated SDK client, initialized lazily on first use.
    /// Uses Arc<RwLock> for thread-safe interior mutability.
    auth_client: Arc<RwLock<Option<AuthenticatedClient>>>,

    /// Signer instance for signing orders.
    /// Created upfront to validate the private key format.
    signer: PrivateKeySigner,
}

impl std::fmt::Debug for LiveTrader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Check if initialized without blocking
        let initialized = self
            .auth_client
            .try_read()
            .map(|guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>>| guard.is_some())
            .unwrap_or(false);

        f.debug_struct("LiveTrader")
            .field("wallet", &self.wallet_address().unwrap_or_default())
            .field("initialized", &initialized)
            .finish()
    }
}

impl LiveTrader {
    /// Create a new live trader from a private key.
    ///
    /// # Arguments
    ///
    /// * `private_key` - Hex-encoded private key, with or without "0x" prefix.
    ///                   Must be exactly 64 hex characters (32 bytes).
    ///
    /// # Returns
    ///
    /// A `LiveTrader` instance ready for order execution. Note that actual
    /// API authentication happens lazily on the first order.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The private key is not exactly 64 hex characters
    /// - The private key contains invalid hex characters
    /// - The private key is not a valid secp256k1 key
    ///
    /// # Example
    ///
    /// ```ignore
    /// let trader = LiveTrader::new("0x1234...abcd").await?;
    /// println!("Wallet: {}", trader.wallet_address()?);
    /// ```
    pub async fn new(private_key: &str) -> Result<Self> {
        // Normalize the private key by removing 0x prefix if present
        let pk = private_key.strip_prefix("0x").unwrap_or(private_key);

        // Validate key length (32 bytes = 64 hex chars)
        if pk.len() != 64 {
            anyhow::bail!(
                "Invalid private key length: expected 64 hex chars, got {}",
                pk.len()
            );
        }

        // Validate hex encoding
        hex::decode(pk).context("Invalid private key: not valid hex")?;

        // Create the signer with Polygon chain ID for proper EIP-712 signing
        let signer = PrivateKeySigner::from_str(pk)
            .context("Failed to create signer from private key")?
            .with_chain_id(Some(POLYGON_CHAIN_ID));

        tracing::info!(
            "LiveTrader created for wallet {} (authentication will happen on first order)",
            signer.address()
        );

        Ok(Self {
            private_key: pk.to_string(),
            auth_client: Arc::new(RwLock::new(None)),
            signer,
        })
    }

    /// Ensure the client is authenticated with the Polymarket API.
    ///
    /// This method is called automatically before any order operation.
    /// It's idempotent - if already authenticated, it returns immediately.
    ///
    /// # Authentication Flow
    ///
    /// 1. Create an unauthenticated client pointing to the CLOB endpoint
    /// 2. Use the authentication builder with our signer
    /// 3. Call authenticate() to complete the handshake
    /// 4. Store the authenticated client for future use
    async fn ensure_authenticated(&self) -> Result<()> {
        // Fast path: already authenticated (read lock only)
        {
            let guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>> =
                self.auth_client.read().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        // Slow path: need to authenticate (write lock)
        let mut client_guard: tokio::sync::RwLockWriteGuard<'_, Option<AuthenticatedClient>> =
            self.auth_client.write().await;

        // Double-check after acquiring write lock (another task may have authenticated)
        if client_guard.is_some() {
            return Ok(());
        }

        tracing::info!("Authenticating with Polymarket API...");

        // Create unauthenticated client with default config
        let client = Client::new(CLOB_ENDPOINT, Config::default())
            .context("Failed to create Polymarket client")?;

        // Authenticate using EIP-712 signature
        let auth_client: AuthenticatedClient = client
            .authentication_builder(&self.signer)
            .authenticate()
            .await
            .context("Failed to authenticate with Polymarket API")?;

        tracing::info!("Successfully authenticated with Polymarket");

        *client_guard = Some(auth_client);
        Ok(())
    }

    /// Place a Fill-or-Kill buy order.
    ///
    /// FOK orders must fill completely and immediately, or they are rejected.
    /// This is ideal for the arbitrage strategy where we need certainty
    /// that both legs execute at the expected prices.
    ///
    /// # Arguments
    ///
    /// * `token_id` - The outcome token to buy (UP or DOWN token ID)
    /// * `size` - Number of shares to buy
    /// * `price` - Maximum price per share (order will fill at this or better)
    ///
    /// # Returns
    ///
    /// A `LiveTradeResult` containing:
    /// - `filled`: true if the order was completely filled
    /// - `filled_size`: the actual number of shares bought
    /// - `tx_hashes`: blockchain transaction hashes if filled
    /// - `error`: error message if the order was rejected
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = trader.buy_fok(up_token_id, dec!(10), dec!(0.45)).await?;
    /// if result.filled {
    ///     println!("Bought {} shares at ${}", result.filled_size, result.price);
    /// }
    /// ```
    pub async fn buy_fok(
        &self,
        token_id: &str,
        size: Decimal,
        price: Decimal,
    ) -> Result<LiveTradeResult> {
        self.place_order(token_id, SdkSide::Buy, size, price, SdkOrderType::FOK)
            .await
    }

    /// Place a Good-Till-Cancelled limit buy order.
    ///
    /// GTC orders rest on the order book until they are filled or cancelled.
    /// Use this for less time-sensitive orders where you want a specific price.
    ///
    /// # Arguments
    ///
    /// * `token_id` - The outcome token to buy
    /// * `size` - Number of shares to buy
    /// * `price` - Limit price (order will only fill at this price or better)
    ///
    /// # Returns
    ///
    /// A `LiveTradeResult` with the order ID for tracking. The order may not
    /// be immediately filled - check `filled` and use `cancel_order` if needed.
    #[allow(dead_code)]
    pub async fn buy_limit(
        &self,
        token_id: &str,
        size: Decimal,
        price: Decimal,
    ) -> Result<LiveTradeResult> {
        self.place_order(token_id, SdkSide::Buy, size, price, SdkOrderType::GTC)
            .await
    }

    /// Place a Fill-or-Kill sell order.
    ///
    /// # Arguments
    ///
    /// * `token_id` - The outcome token to sell
    /// * `size` - Number of shares to sell
    /// * `price` - Minimum price to accept per share
    #[allow(dead_code)]
    pub async fn sell_fok(
        &self,
        token_id: &str,
        size: Decimal,
        price: Decimal,
    ) -> Result<LiveTradeResult> {
        self.place_order(token_id, SdkSide::Sell, size, price, SdkOrderType::FOK)
            .await
    }

    /// Place a Good-Till-Cancelled limit sell order.
    ///
    /// # Arguments
    ///
    /// * `token_id` - The outcome token to sell
    /// * `size` - Number of shares to sell
    /// * `price` - Limit price (minimum price to accept)
    #[allow(dead_code)]
    pub async fn sell_limit(
        &self,
        token_id: &str,
        size: Decimal,
        price: Decimal,
    ) -> Result<LiveTradeResult> {
        self.place_order(token_id, SdkSide::Sell, size, price, SdkOrderType::GTC)
            .await
    }

    /// Internal method to place an order with the SDK.
    ///
    /// This handles the complete order flow:
    /// 1. Ensure authentication
    /// 2. Build the order using the SDK's builder pattern
    /// 3. Sign the order with our private key
    /// 4. Post to the exchange
    /// 5. Parse and return the response
    async fn place_order(
        &self,
        token_id: &str,
        side: SdkSide,
        size: Decimal,
        price: Decimal,
        order_type: SdkOrderType,
    ) -> Result<LiveTradeResult> {
        // Step 1: Ensure we're authenticated
        self.ensure_authenticated().await?;

        // Step 2: Get the authenticated client
        let client_guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>> =
            self.auth_client.read().await;
        let client: &AuthenticatedClient = client_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client not authenticated"))?;

        // Log the order details
        let side_str = match side {
            SdkSide::Buy => "buy",
            SdkSide::Sell => "sell",
            _ => "unknown",
        };

        tracing::info!(
            "Placing {:?} {} order: {} shares of {} at ${}",
            order_type,
            side_str,
            size,
            token_id,
            price
        );

        // Step 3: Build the order using the SDK's fluent builder
        let order = client
            .limit_order()
            .token_id(token_id)
            .side(side)
            .price(price)
            .size(size)
            .order_type(order_type)
            .build()
            .await
            .context("Failed to build order")?;

        // Step 4: Sign the order with EIP-712
        let signed_order = client
            .sign(&self.signer, order)
            .await
            .context("Failed to sign order")?;

        // Step 5: Post to the exchange
        let response = client
            .post_order(signed_order)
            .await
            .context("Failed to post order")?;

        // Step 6: Parse the response
        // For buys, taking_amount is what we received (shares)
        // For sells, making_amount is what we gave (shares)
        let filled_size = match side {
            SdkSide::Buy => response.taking_amount,
            SdkSide::Sell => response.making_amount,
            _ => Decimal::ZERO,
        };

        let filled = response.success && filled_size > Decimal::ZERO;

        let result = LiveTradeResult {
            order_id: response.order_id.clone(),
            filled,
            filled_size,
            requested_size: size,
            price,
            tx_hashes: response.transaction_hashes.clone(),
            error: response.error_msg.clone(),
        };

        // Log the result
        if filled {
            tracing::info!(
                "Order {} filled: {} shares at ${} (tx: {:?})",
                response.order_id,
                filled_size,
                price,
                response.transaction_hashes
            );
        } else if let Some(ref err) = response.error_msg {
            tracing::warn!("Order {} failed: {}", response.order_id, err);
        } else {
            tracing::info!(
                "Order {} placed (GTC, awaiting fill)",
                response.order_id
            );
        }

        Ok(result)
    }

    /// Cancel a specific order by ID.
    ///
    /// # Arguments
    ///
    /// * `order_id` - The order ID returned from a previous order placement
    ///
    /// # Errors
    ///
    /// Returns an error if the order doesn't exist or has already been filled.
    #[allow(dead_code)]
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        self.ensure_authenticated().await?;

        let client_guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>> =
            self.auth_client.read().await;
        let client: &AuthenticatedClient = client_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client not authenticated"))?;

        tracing::info!("Cancelling order {}", order_id);

        client
            .cancel_order(order_id)
            .await
            .context("Failed to cancel order")?;

        tracing::info!("Order {} cancelled successfully", order_id);
        Ok(())
    }

    /// Cancel all open orders.
    ///
    /// This is a safety mechanism to clear all pending orders,
    /// useful when shutting down or in error recovery scenarios.
    #[allow(dead_code)]
    pub async fn cancel_all_orders(&self) -> Result<()> {
        self.ensure_authenticated().await?;

        let client_guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>> =
            self.auth_client.read().await;
        let client: &AuthenticatedClient = client_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Client not authenticated"))?;

        tracing::info!("Cancelling all open orders");

        client
            .cancel_all_orders()
            .await
            .context("Failed to cancel all orders")?;

        tracing::info!("All orders cancelled successfully");
        Ok(())
    }

    /// Check if the trader is authenticated with the Polymarket API.
    ///
    /// Returns true if authentication has completed successfully.
    /// Note that authentication happens lazily on first order.
    #[allow(dead_code)]
    pub async fn is_initialized(&self) -> bool {
        let guard: tokio::sync::RwLockReadGuard<'_, Option<AuthenticatedClient>> =
            self.auth_client.read().await;
        guard.is_some()
    }

    /// Get the Ethereum wallet address for this trader.
    ///
    /// This is the address derived from the private key, which is used
    /// for signing orders and receiving funds.
    ///
    /// # Returns
    ///
    /// The checksummed Ethereum address as a string (e.g., "0x1234...abcd")
    pub fn wallet_address(&self) -> Result<String> {
        Ok(format!("{:?}", self.signer.address()))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // LiveTradeResult Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_live_trade_result_filled() {
        let result = LiveTradeResult {
            order_id: "test-123".to_string(),
            filled: true,
            filled_size: Decimal::new(10, 0),
            requested_size: Decimal::new(10, 0),
            price: Decimal::new(50, 2), // 0.50
            tx_hashes: vec!["0xabc".to_string()],
            error: None,
        };

        assert!(result.filled);
        assert_eq!(result.filled_size, result.requested_size);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_live_trade_result_rejected() {
        let result = LiveTradeResult {
            order_id: "test-456".to_string(),
            filled: false,
            filled_size: Decimal::ZERO,
            requested_size: Decimal::new(10, 0),
            price: Decimal::new(50, 2),
            tx_hashes: vec![],
            error: Some("Insufficient liquidity".to_string()),
        };

        assert!(!result.filled);
        assert_eq!(result.filled_size, Decimal::ZERO);
        assert!(result.error.is_some());
    }

    // ------------------------------------------------------------------------
    // OrderStatus Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_order_status() {
        let status = OrderStatus {
            order_id: "order-789".to_string(),
            filled_size: Decimal::new(5, 0),
            original_size: Decimal::new(10, 0),
            price: Decimal::new(45, 2), // 0.45
            is_open: true,
            is_filled: false,
        };

        assert!(status.is_open);
        assert!(!status.is_filled);
        assert!(status.filled_size < status.original_size);
    }

    // ------------------------------------------------------------------------
    // LiveTrader Creation Tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_live_trader_creation_valid_key() {
        // Test with a valid 64-char hex key (not a real key!)
        let test_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = LiveTrader::new(test_key).await;
        assert!(result.is_ok());

        let trader = result.unwrap();
        // Not initialized until first order triggers authentication
        assert!(!trader.is_initialized().await);
    }

    #[tokio::test]
    async fn test_live_trader_creation_with_0x_prefix() {
        // Test that 0x prefix is handled correctly
        let test_key = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = LiveTrader::new(test_key).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_live_trader_creation_invalid_key_length() {
        // Test rejection of short keys
        let short_key = "0123456789abcdef";
        let result = LiveTrader::new(short_key).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid private key length"));
    }

    #[tokio::test]
    async fn test_live_trader_creation_invalid_hex() {
        // Test rejection of invalid hex characters
        let invalid_key = "xyz3456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = LiveTrader::new(invalid_key).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not valid hex"));
    }

    // ------------------------------------------------------------------------
    // Wallet Address Tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_wallet_address() {
        let test_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let trader = LiveTrader::new(test_key).await.unwrap();

        let address = trader.wallet_address();
        assert!(address.is_ok());

        let addr = address.unwrap();
        // Ethereum address format: 0x + 40 hex chars
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }
}
