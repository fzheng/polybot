//! PolyBot - Automated Polymarket Trading Bot
//!
//! A trading bot for Polymarket's BTC 15-minute UP/DOWN prediction markets,
//! implementing a two-leg arbitrage strategy.
//!
//! # Quick Start
//!
//! ```bash
//! # Paper trading mode (default, safe)
//! cargo run --release
//!
//! # With verbose logging to file
//! cargo run --release -- --verbose --log-file polybot.log
//! ```
//!
//! # Architecture
//!
//! - [`api`] - Polymarket REST and WebSocket clients
//! - [`config`] - Configuration management (TOML + env vars)
//! - [`market`] - Market monitoring and round tracking
//! - [`strategy`] - Two-leg arbitrage strategy
//! - [`paper`] - Paper trading simulation
//! - [`recorder`] - Price data recording for backtesting
//! - [`terminal`] - Terminal UI and command handling
//! - [`types`] - Core data structures

mod api;
mod config;
mod market;
mod paper;
mod recorder;
mod strategy;
mod terminal;
mod types;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;
use crate::terminal::App;

/// Command-line arguments for PolyBot.
#[derive(Parser, Debug)]
#[command(name = "polybot")]
#[command(about = "Automated Polymarket trading bot for BTC 15-minute UP/DOWN markets")]
#[command(version)]
struct Args {
    /// Enable verbose (debug) logging.
    /// Use with --log-file to capture detailed logs.
    #[arg(short, long)]
    verbose: bool,

    /// Path to configuration file.
    /// Defaults to config.toml in current directory.
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Log to file instead of suppressing logs.
    /// Console logging is disabled by default because it
    /// interferes with the terminal UI.
    #[arg(long)]
    log_file: Option<String>,
}

/// Application entry point.
///
/// Initializes logging, loads configuration, and runs the terminal application.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize logging
    // Console logging is disabled by default (interferes with TUI)
    // Use --log-file to enable file logging for debugging
    let log_level = if args.verbose { "debug" } else { "info" };

    if let Some(log_path) = &args.log_file {
        // Log to file for debugging
        let file = std::fs::File::create(log_path)?;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| format!("polybot={}", log_level).into()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(file),
            )
            .init();
    } else {
        // No console logging - it interferes with the TUI
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| format!("polybot={}", log_level).into()),
            )
            .init();
    }

    // Load configuration from file and environment
    let config = Config::load(&args.config)?;

    // Run the terminal application
    let mut app = App::new(config).await?;
    app.run().await?;

    Ok(())
}
