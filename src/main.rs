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

#[derive(Parser, Debug)]
#[command(name = "polybot")]
#[command(about = "Automated Polymarket trading bot for BTC 15-minute UP/DOWN markets")]
struct Args {
    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Path to config file
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Run in record-only mode (no trading, just data collection)
    #[arg(long)]
    record_only: bool,

    /// Log to file instead of hiding logs (for debugging)
    #[arg(long)]
    log_file: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load environment variables
    dotenvy::dotenv().ok();

    // Initialize logging - write to file if specified, otherwise disable console logging
    // (console logging interferes with the TUI)
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

    // Load configuration
    let config = Config::load(&args.config)?;

    // Run the terminal application
    let mut app = App::new(config, args.record_only).await?;
    app.run().await?;

    Ok(())
}
