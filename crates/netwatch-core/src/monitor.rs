//! One sampling tick: network rates, disk usage, rolling history and session
//! totals.

use crate::disk::{self, DiskUsage};
use crate::history::{History, RatePoint};
use crate::net::{self, InterfaceCounters, NetMeter, NetRates};
use std::time::{Duration, Instant};

/// Disk usage changes slowly; re-reading it every tick is wasted work.
pub const DISK_REFRESH: Duration = Duration::from_secs(10);

/// Samples kept for the dashboard graph.
pub const HISTORY_LEN: usize = 120;

pub struct Monitor {
    meter: NetMeter,
    history: History,
    rates: NetRates,
    disk: Option<DiskUsage>,
    disk_read_at: Option<Instant>,
    session_down: u64,
    session_up: u64,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new(HISTORY_LEN)
    }
}

impl Monitor {
    pub fn new(history_len: usize) -> Self {
        Self {
            meter: NetMeter::new(),
            history: History::new(history_len),
            rates: NetRates::default(),
            disk: None,
            disk_read_at: None,
            session_down: 0,
            session_up: 0,
        }
    }

    /// Read the OS counters and advance one tick.
    pub fn tick(&mut self, now: Instant) {
        let ifaces = net::read_interfaces().unwrap_or_else(|e| {
            log::warn!("Reading interface counters failed: {e}");
            Vec::new()
        });
        self.tick_with(now, &ifaces, disk::read_root);
    }

    /// Advance one tick from given readings (`read_disk` is called only when
    /// the cached value is stale).
    pub fn tick_with(
        &mut self,
        now: Instant,
        ifaces: &[InterfaceCounters],
        read_disk: impl FnOnce() -> Option<DiskUsage>,
    ) {
        let first = self.disk_read_at.is_none();
        let rates = self.meter.sample(now, ifaces);
        // The first sample is only a baseline: no point to plot yet.
        if !first {
            self.history.push(RatePoint {
                down: rates.down,
                up: rates.up,
            });
            self.session_down += rates.down_bytes;
            self.session_up += rates.up_bytes;
        }
        self.rates = rates;

        let stale = self
            .disk_read_at
            .is_none_or(|t| now.saturating_duration_since(t) >= DISK_REFRESH);
        if stale {
            if let Some(d) = read_disk() {
                self.disk = Some(d);
            }
            self.disk_read_at = Some(now);
        }
    }

    pub fn rates(&self) -> &NetRates {
        &self.rates
    }

    pub fn disk(&self) -> Option<DiskUsage> {
        self.disk
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Bytes (received, sent) since the app started.
    pub fn session_totals(&self) -> (u64, u64) {
        (self.session_down, self.session_up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en0(rx: u64, tx: u64) -> Vec<InterfaceCounters> {
        vec![InterfaceCounters {
            name: "en0".into(),
            up: true,
            running: true,
            loopback: false,
            rx_bytes: rx,
            tx_bytes: tx,
            baudrate: 0,
        }]
    }

    fn disk(available: u64) -> Option<DiskUsage> {
        Some(DiskUsage {
            total: 100,
            available,
        })
    }

    #[test]
    fn accumulates_history_and_session_totals() {
        let mut m = Monitor::new(10);
        let t0 = Instant::now();
        m.tick_with(t0, &en0(1_000, 1_000), || disk(40));
        assert!(m.history().is_empty());
        m.tick_with(t0 + Duration::from_secs(1), &en0(3_000, 1_500), || disk(40));
        m.tick_with(t0 + Duration::from_secs(2), &en0(4_000, 1_500), || disk(40));
        assert_eq!(m.history().len(), 2);
        assert_eq!(m.rates().down, 1_000.0);
        assert_eq!(m.session_totals(), (3_000, 500));
    }

    #[test]
    fn disk_is_reread_only_when_stale() {
        let mut m = Monitor::new(10);
        let t0 = Instant::now();
        m.tick_with(t0, &[], || disk(40));
        m.tick_with(t0 + Duration::from_secs(1), &[], || panic!("read too soon"));
        assert_eq!(m.disk().unwrap().available, 40);
        m.tick_with(t0 + DISK_REFRESH, &[], || disk(30));
        assert_eq!(m.disk().unwrap().available, 30);
        // A failed read keeps the last good value.
        m.tick_with(t0 + DISK_REFRESH * 2, &[], || None);
        assert_eq!(m.disk().unwrap().available, 30);
    }
}
