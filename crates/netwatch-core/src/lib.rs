//! Netwatch backend. No UI and no gpui dependency.
//!
//! - [`net`]: per-interface byte counters and link speed, and the
//!   [`net::NetMeter`] that turns two readings into rates.
//! - [`disk`]: capacity and free space of the startup volume.
//! - [`monitor`]: one tick of sampling, plus the rolling history.
//! - [`icon`]: the menu-bar graph, built as SVG and rasterized with resvg.
//! - [`format`]: human-readable sizes and rates.
//! - [`settings`]: the persisted preferences.

pub mod disk;
pub mod format;
pub mod history;
pub mod icon;
pub mod monitor;
pub mod net;
pub mod settings;

/// Names the per-user data directory.
pub const APP_IDENTIFIER: &str = "dev.netwatch.app";
pub const PRODUCT_NAME: &str = "Netwatch";
