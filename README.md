# PolyBot - Polymarket Trading Bot

An automated trading bot for Polymarket's BTC 15-minute UP/DOWN prediction markets, implementing a two-leg arbitrage strategy.

## Strategy Overview

The bot implements a two-leg arbitrage strategy for BTC 15-minute UP/DOWN markets. The strategy runs automatically when the bot starts - paper trading mode protects you from real orders until you explicitly switch to live mode.

### How It Works

1. **Leg 1 (Buy the Dump)**: During the first N minutes of a round, watch for rapid price drops. If either UP or DOWN drops by at least X% in ~3 seconds, buy that side.

2. **Leg 2 (Hedge)**: After Leg 1, wait for the opposite side's price to satisfy: `leg1_price + opposite_ask <= sum_target`

3. **Profit**: When both legs complete, you hold equal shares on both sides. Since one side always wins, you get $1 payout per share pair while having paid less than $1 total.

### Example

```
Strategy detects a 17% dump in DOWN over 3 seconds
- Bot buys 10 DOWN at $0.35 (Leg 1)
- UP ask drops to $0.56, check: 0.35 + 0.56 = 0.91 <= 0.95
- Bot buys 10 UP at $0.56 (Leg 2)
- Total cost: $9.10, Guaranteed payout: $10.00
- Profit: $0.90 (9.9%)
```

## Installation

### Prerequisites

1. **Install Rust** (if not installed):
   ```bash
   # Windows (PowerShell)
   winget install Rustlang.Rust.MSVC

   # Or download from https://rustup.rs
   ```

2. **Clone and build**:
   ```bash
   git clone https://github.com/yourusername/polybot.git
   cd polybot
   cargo build --release
   ```

### Polymarket Wallet Setup (for Live Trading)

1. Go to [Polymarket](https://polymarket.com) and log in
2. Click on "Cash" in the top right
3. Click the three dots menu
4. Select "Export Private Key"
5. Copy your private key

6. Create a `.env` file:
   ```bash
   cp .env.example .env
   ```

7. Add your private key to `.env`:
   ```
   PK=your_private_key_here
   ```

**WARNING**: Never share your private key or commit `.env` to version control!

## Usage

### Start the Bot

```bash
# Normal mode (paper trading enabled by default - safe!)
cargo run --release

# Record-only mode (no trading, just collect data)
cargo run --release -- --record-only

# Verbose logging to file (for debugging)
cargo run --release -- --verbose --log-file polybot.log
```

### Commands

Once the bot is running, use these commands:

| Command | Description |
|---------|-------------|
| `help` | Show all commands |
| `status` | Show current market and prices |
| `buy up <usd>` | Buy UP shares for USD amount |
| `buy down <usd>` | Buy DOWN shares for USD amount |
| `buyshares up <n>` | Buy N UP shares at best ask |
| `buyshares down <n>` | Buy N DOWN shares at best ask |
| `params` | Show current strategy parameters |
| `params <shares> [sum] [move] [window]` | Update strategy parameters |
| `mode` | Show current trading mode |
| `mode paper` | Switch to paper trading |
| `mode live` | Switch to live trading (requires confirmation) |
| `logs` | Show strategy execution logs |
| `balance` (or `bal`) | Show paper trading balance |
| `positions` (or `pos`) | Show open positions |
| `pnl` | Show profit/loss summary |
| `trades` | Show recent trades |
| `reset` | Reset paper trading balance/history |
| `clear` | Clear message log |
| `quit` | Exit the bot |

### Strategy Parameters

Adjust the strategy parameters at runtime:

```
params <shares> [sum=0.95] [move=0.15] [window=2]
```

- **shares**: Number of shares to buy for each leg
- **sum**: Hedge threshold (default 0.95 = 5% minimum profit)
- **move**: Dump threshold percentage (default 0.15 = 15%)
- **window**: Minutes from round start to watch for Leg 1 (default 2)

### Examples

```bash
# Conservative: 10 shares, 5% profit target, 15% dump, 2 min window
params 10 0.95 0.15 2

# Aggressive: 20 shares, 8% profit target, 10% dump, 4 min window
params 20 0.92 0.10 4

# Very conservative: 5 shares, 3% profit target, 20% dump, 1 min window
params 5 0.97 0.20 1
```

### Trading Modes

The bot supports two trading modes:

- **PAPER** (default): Simulates trades without placing real orders. Safe for testing.
- **LIVE**: Places real orders on Polymarket. Requires confirmation to enable.

Switch modes using:
```
mode paper   # Switch to paper trading (no real orders)
mode live    # Switch to live trading (requires 'yes' confirmation)
```

## Configuration

Edit `config.toml` to customize settings:

```toml
[api]
clob_endpoint = "https://clob.polymarket.com"
ws_endpoint = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
gamma_endpoint = "https://gamma-api.polymarket.com"

[trading]
default_shares = 10
default_sum_target = 0.95
default_move_pct = 0.15
default_window_min = 2
dump_window_secs = 3

[recording]
enabled = true
data_dir = "data"
interval_ms = 1000

[paper_trading]
enabled = true           # Safe by default - no real orders
starting_balance = 1000  # Starting balance in USD
fee_rate = 0.005         # Simulated fee (0.5%)
slippage = 0.02          # Simulated slippage (2%)
```

## Data Recording

The bot records price snapshots for backtesting:

- Data is stored in `data/prices_YYYY-MM-DD.jsonl`
- Each line is a JSON object with timestamp, prices, and market info
- Use this data to test different parameter combinations

## Risk Warning

**This bot involves financial risk:**

- Past performance does not guarantee future results
- The strategy assumes price reversals happen frequently enough to profit
- Market conditions can change, making the strategy less effective
- You can lose money if Leg 2 never triggers before the round ends
- Always test with paper trading first (enabled by default)
- Paper trading is enabled by default for your safety

## Architecture

```
src/
├── main.rs          # Entry point and CLI
├── types.rs         # Core data structures
├── config.rs        # Configuration management
├── api/
│   ├── client.rs    # REST API client
│   └── websocket.rs # WebSocket price streaming
├── market/
│   └── watcher.rs   # Market monitoring
├── strategy/
│   └── auto.rs      # Two-leg strategy implementation
├── paper/
│   └── mod.rs       # Paper trading simulation
├── recorder/
│   └── mod.rs       # Data recording for backtesting
└── terminal/
    ├── app.rs       # Application logic
    └── ui.rs        # Terminal UI rendering
```

## API Reference

The bot uses:
- [Polymarket CLOB API](https://docs.polymarket.com/developers/CLOB/introduction) for trading
- [Gamma API](https://gamma-api.polymarket.com) for market metadata
- WebSocket for real-time price updates

## License

MIT License - See [LICENSE](LICENSE) for details.

## Disclaimer

This software is for educational purposes only. Trading on prediction markets involves substantial risk of loss. The authors are not responsible for any financial losses incurred through use of this software.
