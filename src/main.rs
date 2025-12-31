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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("polybot={}", log_level).into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // Load environment variables
    dotenvy::dotenv().ok();

    // Load configuration
    let config = Config::load(&args.config)?;

    // Run the terminal application
    let mut app = App::new(config, args.record_only).await?;
    app.run().await?;

    Ok(())
}
