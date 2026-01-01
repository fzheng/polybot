//! Configuration management for PolyBot.
//!
//! This module handles loading, parsing, and managing configuration from:
//! - TOML configuration file (`config.toml`)
//! - Environment variables (for sensitive data like private keys)
//!
//! # Configuration Priority
//!
//! 1. Environment variables (highest priority)
//! 2. Config file values
//! 3. Default values (lowest priority)
//!
//! # Example Config File
//!
//! ```toml
//! [api]
//! clob_endpoint = "https://clob.polymarket.com"
//! ws_endpoint = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
//!
//! [trading]
//! default_shares = 10
//! default_sum_target = 0.95
//! default_move_pct = 0.15
//! default_window_min = 2
//!
//! [paper_trading]
//! enabled = true
//! starting_balance = 1000
//! ```
//!
//! # Environment Variables
//!
//! - `PK` or `POLYMARKET_PRIVATE_KEY`: Wallet private key
//! - `POLYMARKET_FUNDER`: Funder address for proxy wallets
//! - `POLYMARKET_SIGNATURE_TYPE`: Signature type (0=EOA, 1=Magic, 2=Proxy)

use anyhow::{Context, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ============================================================================
// Main Configuration
// ============================================================================

/// Root application configuration.
///
/// Contains all configuration sections for the trading bot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Polymarket API connection settings
    pub api: ApiConfig,

    /// Trading strategy parameters
    pub trading: TradingConfig,

    /// Price data recording settings
    pub recording: RecordingConfig,

    /// Paper trading simulation settings
    #[serde(default)]
    pub paper_trading: PaperTradingConfig,
}

// ============================================================================
// API Configuration
// ============================================================================

/// API connection and authentication configuration.
///
/// Contains endpoints and credentials for connecting to Polymarket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// CLOB (Central Limit Order Book) API endpoint.
    /// Used for placing orders and fetching order books.
    #[serde(default = "default_clob_endpoint")]
    pub clob_endpoint: String,

    /// WebSocket endpoint for real-time price streaming.
    /// Receives order book updates in real-time.
    #[serde(default = "default_ws_endpoint")]
    pub ws_endpoint: String,

    /// Gamma API endpoint for market metadata.
    /// Used to discover active BTC 15-minute markets.
    #[serde(default = "default_gamma_endpoint")]
    pub gamma_endpoint: String,

    /// Wallet private key for signing transactions.
    /// Can be loaded from `PK` or `POLYMARKET_PRIVATE_KEY` env vars.
    /// **SECURITY**: Never commit this to version control!
    #[serde(default)]
    pub private_key: Option<String>,

    /// Funder address for proxy wallet setups.
    /// Used with Safe/Gnosis multi-sig wallets.
    #[serde(default)]
    pub funder_address: Option<String>,

    /// Signature type for authentication.
    /// - 0: EOA (Externally Owned Account) - standard wallet
    /// - 1: Magic/Email login
    /// - 2: Proxy/Safe wallet
    #[serde(default)]
    pub signature_type: u8,
}

// ============================================================================
// Trading Configuration
// ============================================================================

/// Trading strategy default parameters.
///
/// These values are used when the bot starts and can be
/// overridden at runtime using the `params` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    /// Default number of shares per trade leg.
    #[serde(default = "default_shares")]
    pub default_shares: Decimal,

    /// Default sum target for hedge condition.
    /// Leg 2 triggers when: `leg1_price + opposite_ask <= sum_target`
    /// Uses theta decay in last 5 minutes (decays toward 0.99).
    #[serde(default = "default_sum_target")]
    pub default_sum_target: Decimal,

    /// Default dump threshold percentage (as decimal).
    /// Example: 0.15 = 15% price drop required for Leg 1.
    #[serde(default = "default_move_pct")]
    pub default_move_pct: Decimal,

    /// Default window in minutes for Leg 1 detection.
    /// Strategy only watches for dumps during this initial window.
    #[serde(default = "default_window_min")]
    pub default_window_min: u32,

    /// Maximum cycles per round (Leg1+Leg2 pairs).
    /// Set to 1 for conservative, higher for more aggressive.
    #[serde(default = "default_max_cycles")]
    pub default_max_cycles: u32,

    /// Dump detection window in seconds.
    /// How far back to look for price drops.
    #[serde(default = "default_dump_window_secs")]
    pub dump_window_secs: u64,

    /// Market slug pattern for BTC UP/DOWN markets.
    /// Used to filter which markets to trade.
    #[serde(default = "default_market_pattern")]
    pub market_pattern: String,
}

// ============================================================================
// Recording Configuration
// ============================================================================

/// Price data recording configuration.
///
/// Controls how market data is saved for backtesting and analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Enable/disable price recording.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Directory to store recorded price data.
    /// Files are named `prices_YYYY-MM-DD.jsonl`.
    #[serde(default = "default_data_dir")]
    pub data_dir: String,

    /// Recording interval in milliseconds.
    /// How often to capture price snapshots.
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
}

// ============================================================================
// Paper Trading Configuration
// ============================================================================

/// Paper trading (simulation) configuration.
///
/// Paper trading allows testing strategies without risking real money.
/// Simulates order execution with configurable fees and slippage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTradingConfig {
    /// Enable paper trading mode.
    /// When enabled, no real orders are placed.
    /// **Enabled by default for safety!**
    #[serde(default = "default_paper_enabled")]
    pub enabled: bool,

    /// Starting balance in USD for paper trading.
    #[serde(default = "default_starting_balance")]
    pub starting_balance: Decimal,

    /// Log file for paper trades (JSONL format).
    /// Each trade is appended as a JSON object on a new line.
    #[serde(default = "default_paper_log_file")]
    pub log_file: String,

    /// Simulated fee rate as a decimal.
    /// Example: 0.005 = 0.5% fee per trade.
    #[serde(default = "default_fee_rate")]
    pub fee_rate: Decimal,

    /// Simulated slippage rate as a decimal.
    /// Example: 0.02 = 2% price slippage on execution.
    #[serde(default = "default_slippage")]
    pub slippage: Decimal,
}

// ============================================================================
// Default Value Functions
// ============================================================================

fn default_clob_endpoint() -> String {
    "https://clob.polymarket.com".to_string()
}

fn default_ws_endpoint() -> String {
    "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string()
}

fn default_gamma_endpoint() -> String {
    "https://gamma-api.polymarket.com".to_string()
}

fn default_shares() -> Decimal {
    Decimal::new(10, 0)
}

fn default_sum_target() -> Decimal {
    Decimal::new(95, 2) // 0.95
}

fn default_move_pct() -> Decimal {
    Decimal::new(15, 2) // 0.15
}

fn default_window_min() -> u32 {
    2
}

fn default_max_cycles() -> u32 {
    1
}

fn default_dump_window_secs() -> u64 {
    3
}

fn default_market_pattern() -> String {
    "bitcoin-15-minute".to_string()
}

fn default_enabled() -> bool {
    true
}

fn default_data_dir() -> String {
    "data".to_string()
}

fn default_interval_ms() -> u64 {
    1000 // 1 second
}

fn default_paper_enabled() -> bool {
    true // Paper trading enabled by default for safety
}

fn default_starting_balance() -> Decimal {
    Decimal::new(1000, 0) // $1000
}

fn default_paper_log_file() -> String {
    "paper_trades.jsonl".to_string()
}

fn default_fee_rate() -> Decimal {
    Decimal::new(5, 3) // 0.005 = 0.5%
}

fn default_slippage() -> Decimal {
    Decimal::new(2, 2) // 0.02 = 2%
}

// ============================================================================
// Default Implementations
// ============================================================================

impl Default for PaperTradingConfig {
    fn default() -> Self {
        Self {
            enabled: default_paper_enabled(),
            starting_balance: default_starting_balance(),
            log_file: default_paper_log_file(),
            fee_rate: default_fee_rate(),
            slippage: default_slippage(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig {
                clob_endpoint: default_clob_endpoint(),
                ws_endpoint: default_ws_endpoint(),
                gamma_endpoint: default_gamma_endpoint(),
                private_key: None,
                funder_address: None,
                signature_type: 0,
            },
            trading: TradingConfig {
                default_shares: default_shares(),
                default_sum_target: default_sum_target(),
                default_move_pct: default_move_pct(),
                default_window_min: default_window_min(),
                default_max_cycles: default_max_cycles(),
                dump_window_secs: default_dump_window_secs(),
                market_pattern: default_market_pattern(),
            },
            recording: RecordingConfig {
                enabled: default_enabled(),
                data_dir: default_data_dir(),
                interval_ms: default_interval_ms(),
            },
            paper_trading: PaperTradingConfig::default(),
        }
    }
}

// ============================================================================
// Config Implementation
// ============================================================================

impl Config {
    /// Load configuration from file with environment variable overrides.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the TOML configuration file
    ///
    /// # Returns
    ///
    /// The loaded configuration, or defaults if file doesn't exist.
    ///
    /// # Environment Variables
    ///
    /// These override file values:
    /// - `PK` or `POLYMARKET_PRIVATE_KEY`: Wallet private key
    /// - `POLYMARKET_FUNDER`: Funder address
    /// - `POLYMARKET_SIGNATURE_TYPE`: Signature type
    pub fn load(path: &str) -> Result<Self> {
        let path = Path::new(path);

        if path.exists() {
            let content = std::fs::read_to_string(path)
                .context("Failed to read config file")?;
            let mut config: Config = toml::from_str(&content)
                .context("Failed to parse config file")?;

            // Override with environment variables if set
            config.load_env_overrides();
            Ok(config)
        } else {
            // Use defaults and env vars
            let mut config = Config::default();
            config.load_env_overrides();
            Ok(config)
        }
    }

    /// Load sensitive configuration from environment variables.
    ///
    /// Checks for private key and other auth-related env vars.
    fn load_env_overrides(&mut self) {
        // Private key (try both env var names)
        if let Ok(pk) = std::env::var("PK") {
            self.api.private_key = Some(pk);
        }
        if let Ok(pk) = std::env::var("POLYMARKET_PRIVATE_KEY") {
            self.api.private_key = Some(pk);
        }
        // Funder address for proxy wallets
        if let Ok(funder) = std::env::var("POLYMARKET_FUNDER") {
            self.api.funder_address = Some(funder);
        }
        // Signature type
        if let Ok(sig_type) = std::env::var("POLYMARKET_SIGNATURE_TYPE") {
            if let Ok(v) = sig_type.parse() {
                self.api.signature_type = v;
            }
        }
    }

}
