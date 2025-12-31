//! Configuration management

#![allow(dead_code)]

use anyhow::{Context, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Polymarket API configuration
    pub api: ApiConfig,

    /// Trading configuration
    pub trading: TradingConfig,

    /// Recording configuration
    pub recording: RecordingConfig,

    /// Paper trading configuration
    #[serde(default)]
    pub paper_trading: PaperTradingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// CLOB API endpoint
    #[serde(default = "default_clob_endpoint")]
    pub clob_endpoint: String,

    /// WebSocket endpoint
    #[serde(default = "default_ws_endpoint")]
    pub ws_endpoint: String,

    /// Gamma API endpoint (for market metadata)
    #[serde(default = "default_gamma_endpoint")]
    pub gamma_endpoint: String,

    /// Private key (loaded from env var PK if not set)
    #[serde(default)]
    pub private_key: Option<String>,

    /// Funder address (for proxy wallets)
    #[serde(default)]
    pub funder_address: Option<String>,

    /// Signature type: 0 = EOA, 1 = Magic/Email, 2 = Proxy
    #[serde(default)]
    pub signature_type: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    /// Default number of shares per trade
    #[serde(default = "default_shares")]
    pub default_shares: Decimal,

    /// Default sum target for hedge condition
    #[serde(default = "default_sum_target")]
    pub default_sum_target: Decimal,

    /// Default dump threshold percentage
    #[serde(default = "default_move_pct")]
    pub default_move_pct: Decimal,

    /// Default window in minutes for Leg 1
    #[serde(default = "default_window_min")]
    pub default_window_min: u32,

    /// Dump detection window in seconds
    #[serde(default = "default_dump_window_secs")]
    pub dump_window_secs: u64,

    /// Market slug pattern for BTC UP/DOWN
    #[serde(default = "default_market_pattern")]
    pub market_pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Enable price recording
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Directory to store recorded data
    #[serde(default = "default_data_dir")]
    pub data_dir: String,

    /// Recording interval in milliseconds
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTradingConfig {
    /// Enable paper trading mode (no real orders)
    #[serde(default = "default_paper_enabled")]
    pub enabled: bool,

    /// Starting balance in USD for paper trading
    #[serde(default = "default_starting_balance")]
    pub starting_balance: Decimal,

    /// Log file for paper trades
    #[serde(default = "default_paper_log_file")]
    pub log_file: String,

    /// Simulated fee rate (0.005 = 0.5%)
    #[serde(default = "default_fee_rate")]
    pub fee_rate: Decimal,

    /// Simulated slippage rate (0.02 = 2%)
    #[serde(default = "default_slippage")]
    pub slippage: Decimal,
}

// Default value functions
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

impl Config {
    /// Load configuration from file
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

    /// Load overrides from environment variables
    fn load_env_overrides(&mut self) {
        if let Ok(pk) = std::env::var("PK") {
            self.api.private_key = Some(pk);
        }
        if let Ok(pk) = std::env::var("POLYMARKET_PRIVATE_KEY") {
            self.api.private_key = Some(pk);
        }
        if let Ok(funder) = std::env::var("POLYMARKET_FUNDER") {
            self.api.funder_address = Some(funder);
        }
        if let Ok(sig_type) = std::env::var("POLYMARKET_SIGNATURE_TYPE") {
            if let Ok(v) = sig_type.parse() {
                self.api.signature_type = v;
            }
        }
    }

    /// Get private key, returning error if not configured
    pub fn get_private_key(&self) -> Result<&str> {
        self.api
            .private_key
            .as_deref()
            .context("Private key not configured. Set PK or POLYMARKET_PRIVATE_KEY env var")
    }

    /// Save default config to file
    pub fn save_default(path: &str) -> Result<()> {
        let config = Config::default();
        let content = toml::to_string_pretty(&config)
            .context("Failed to serialize config")?;
        std::fs::write(path, content)
            .context("Failed to write config file")?;
        Ok(())
    }
}
