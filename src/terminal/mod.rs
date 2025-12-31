//! Terminal user interface.
//!
//! This module provides the terminal-based UI for PolyBot using `ratatui`.
//!
//! - [`App`] - Main application state machine and command handler
//! - `ui` - UI rendering functions (internal)
//!
//! # UI Layout
//!
//! ```text
//! ┌─ Prices ─────────────┬─ Status ─────────────┐
//! │ UP:   $0.51          │ Market: btc-updown...│
//! │ DOWN: $0.49          │ Time:   892s         │
//! │                      │ Mode:   PAPER        │
//! │ SUM:  $1.00          │ Bal:    $1000.00     │
//! └──────────────────────┴──────────────────────┘
//! ┌─ Log ───────────────────────────────────────┐
//! │ Connected!                                  │
//! │ New round: btc-updown-15m-1234567890        │
//! └─────────────────────────────────────────────┘
//! ┌─ Command (type 'help' for commands) ────────┐
//! │ >                                           │
//! └─────────────────────────────────────────────┘
//! ```

mod app;
mod ui;

pub use app::App;
