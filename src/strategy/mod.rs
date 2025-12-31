//! Trading strategy implementation.
//!
//! This module provides the [`AutoTrader`] which implements the
//! two-leg arbitrage strategy for BTC 15-minute UP/DOWN markets.
//!
//! # Strategy Overview
//!
//! 1. **Leg 1 (Buy the Dump)**: During the first N minutes, watch for
//!    rapid price drops. When detected, buy the dumped side.
//!
//! 2. **Leg 2 (Hedge)**: Wait for the opposite side's price to fall
//!    such that `leg1_price + opposite_ask <= sum_target`.
//!
//! 3. **Profit**: With both sides held, guaranteed payout is $1 per
//!    share pair, minus the total cost paid.

mod auto;

pub use auto::AutoTrader;
