//! Netwatch backend. No UI and no gpui dependency.
//!
//! - [`awake`]: keep-awake sessions (power assertions), like Amphetamine.
//! - [`net`]: per-interface byte counters and link speed, and the
//!   [`net::NetMeter`] that turns two readings into rates.
//! - [`disk`]: capacity and free space of the startup volume.
//! - [`disk_history`]: storage use over time, persisted across launches.
//! - [`monitor`]: one tick of sampling, plus the rolling history.
//! - [`icon`]: the menu-bar graph, built as SVG and rasterized with resvg.
//! - [`format`]: human-readable sizes and rates.
//! - [`settings`]: the persisted preferences.
//! - [`speedtest`]: on-demand speed test via `networkQuality`.

pub mod awake;
pub mod disk;
pub mod disk_history;
pub mod format;
pub mod history;
pub mod icon;
pub mod monitor;
pub mod net;
pub mod settings;
pub mod speedtest;

/// Names the per-user data directory.
pub const APP_IDENTIFIER: &str = "dev.netwatch.app";
pub const PRODUCT_NAME: &str = "Netwatch";
