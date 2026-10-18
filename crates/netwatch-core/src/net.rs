//! Network interface counters.
//!
//! macOS: `sysctl(NET_RT_IFLIST2)` returns one `if_msghdr2` per interface
//! with 64-bit byte counters (`getifaddrs` only has 32-bit ones, which wrap
//! every 4 GB) and the driver-reported link speed (`ifi_baudrate`).

use std::collections::HashMap;
use std::time::Instant;

/// One interface's cumulative counters, as read from the OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceCounters {
    pub name: String,
    pub up: bool,
    pub running: bool,
    pub loopback: bool,
    /// Bytes received since boot.
    pub rx_bytes: u64,
    /// Bytes sent since boot.
    pub tx_bytes: u64,
    /// Link speed in bits per second, as reported by the driver (0 if unknown).
    pub baudrate: u64,
}

impl InterfaceCounters {
    /// Physical Ethernet and Wi-Fi ports are `enN` on macOS. Counting only
    /// those avoids double counting: VPN tunnels (`utunN`) carry traffic that
    /// also crosses `enN`, and `awdl`/`llw`/`bridge`/`anpi` are side links.
    pub fn is_physical(&self) -> bool {
        !self.loopback
            && self
                .name
                .strip_prefix("en")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }

    pub fn is_active(&self) -> bool {
        self.up && self.running
    }
}

/// The interface treated as "the" connection, with its link capacity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    /// Bits per second; 0 when the driver doesn't report it.
    pub capacity_bps: u64,
}

/// Throughput over one sampling interval.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetRates {
    /// Bytes per second received.
    pub down: f64,
    /// Bytes per second sent.
    pub up: f64,
    /// Bytes received during the interval.
    pub down_bytes: u64,
    /// Bytes sent during the interval.
    pub up_bytes: u64,
    pub link: Option<Link>,
}

impl NetRates {
    /// Fraction of link capacity in use, by the busier direction (links are
    /// full duplex, so each direction has the whole capacity).
    pub fn utilization(&self) -> Option<f64> {
        let cap = self.link.as_ref()?.capacity_bps;
        (cap > 0).then(|| (self.down.max(self.up) * 8.0 / cap as f64).clamp(0.0, 1.0))
    }
}

/// (received, sent) bytes per interface name.
type Totals = HashMap<String, (u64, u64)>;

/// Turns successive counter readings into rates.
#[derive(Default)]
pub struct NetMeter {
    prev: Option<(Instant, Totals)>,
}

impl NetMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rates since the previous call. The first call only records a baseline
    /// and reports zero.
    pub fn sample(&mut self, now: Instant, ifaces: &[InterfaceCounters]) -> NetRates {
        let counted: Totals = ifaces
            .iter()
            .filter(|i| i.is_physical())
            .map(|i| (i.name.clone(), (i.rx_bytes, i.tx_bytes)))
            .collect();

        let mut rates = NetRates {
            link: primary_link(ifaces),
            ..Default::default()
        };
        if let Some((then, prev)) = &self.prev {
            let secs = now.saturating_duration_since(*then).as_secs_f64();
            for (name, (rx, tx)) in &counted {
                // An interface that just appeared has no baseline; a counter
                // that went backwards was reset (interface re-created).
                if let Some((prx, ptx)) = prev.get(name) {
                    rates.down_bytes += rx.checked_sub(*prx).unwrap_or(0);
                    rates.up_bytes += tx.checked_sub(*ptx).unwrap_or(0);
                }
            }
            if secs > 0.0 {
                rates.down = rates.down_bytes as f64 / secs;
                rates.up = rates.up_bytes as f64 / secs;
            }
        }
        self.prev = Some((now, counted));
        rates
    }
}

/// The active physical interface that has carried the most traffic since
/// boot. Stable from tick to tick, and on a Mac with both Ethernet and Wi-Fi
/// up it picks the one actually in use.
pub fn primary_link(ifaces: &[InterfaceCounters]) -> Option<Link> {
    ifaces
        .iter()
        .filter(|i| i.is_physical() && i.is_active())
        .max_by_key(|i| {
            (
                i.rx_bytes.saturating_add(i.tx_bytes),
                std::cmp::Reverse(i.name.clone()),
            )
        })
        .map(|i| Link {
            name: i.name.clone(),
            capacity_bps: i.baudrate,
        })
}

/// Read every interface's counters.
#[cfg(target_os = "macos")]
pub fn read_interfaces() -> std::io::Result<Vec<InterfaceCounters>> {
    use std::io;
    use std::mem::size_of;

    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    // The table can grow between the size query and the read; retry.
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..4 {
        let mut len: libc::size_t = 0;
        // SAFETY: a size query with a null buffer.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        buf.resize(len + len / 8, 0);
        let mut len = buf.len();
        // SAFETY: `buf` is `len` bytes long.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 {
            buf.truncate(len);
            break;
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ENOMEM) {
            return Err(err);
        }
        buf.clear();
    }

    let mut out = Vec::new();
    let mut offset = 0;
    while offset + size_of::<libc::if_msghdr>() <= buf.len() {
        // SAFETY: bounds checked above; read_unaligned because the kernel
        // only aligns messages to 4 bytes.
        let hdr: libc::if_msghdr =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(offset).cast()) };
        let msglen = hdr.ifm_msglen as usize;
        if msglen == 0 || offset + msglen > buf.len() {
            break;
        }
        if hdr.ifm_type as i32 == libc::RTM_IFINFO2 && msglen >= size_of::<libc::if_msghdr2>() {
            // SAFETY: the message is at least an if_msghdr2.
            let ifm: libc::if_msghdr2 =
                unsafe { std::ptr::read_unaligned(buf.as_ptr().add(offset).cast()) };
            let flags = ifm.ifm_flags;
            out.push(InterfaceCounters {
                name: interface_name(ifm.ifm_index as u32)
                    .unwrap_or_else(|| format!("if{}", ifm.ifm_index)),
                up: flags & libc::IFF_UP != 0,
                running: flags & libc::IFF_RUNNING != 0,
                loopback: flags & libc::IFF_LOOPBACK != 0,
                rx_bytes: ifm.ifm_data.ifi_ibytes,
                tx_bytes: ifm.ifm_data.ifi_obytes,
                baudrate: ifm.ifm_data.ifi_baudrate,
            });
        }
        offset += msglen;
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn interface_name(index: u32) -> Option<String> {
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: `name` is IF_NAMESIZE bytes, as if_indextoname requires.
    let p = unsafe { libc::if_indextoname(index, name.as_mut_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: if_indextoname wrote a NUL-terminated string.
    let s = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
    Some(s.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn read_interfaces() -> std::io::Result<Vec<InterfaceCounters>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "interface counters are only implemented on macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn iface(name: &str, rx: u64, tx: u64) -> InterfaceCounters {
        InterfaceCounters {
            name: name.into(),
            up: true,
            running: true,
            loopback: name.starts_with("lo"),
            rx_bytes: rx,
            tx_bytes: tx,
            baudrate: 1_000_000_000,
        }
    }

    #[test]
    fn only_en_ports_are_physical() {
        for name in ["en0", "en1", "en12"] {
            assert!(iface(name, 0, 0).is_physical(), "{name}");
        }
        for name in [
            "lo0", "utun3", "awdl0", "llw0", "bridge0", "anpi0", "en", "enx",
        ] {
            assert!(!iface(name, 0, 0).is_physical(), "{name}");
        }
    }

    #[test]
    fn first_sample_is_a_baseline() {
        let mut m = NetMeter::new();
        let r = m.sample(Instant::now(), &[iface("en0", 5_000, 1_000)]);
        assert_eq!((r.down, r.up), (0.0, 0.0));
    }

    #[test]
    fn rates_sum_physical_ports_and_ignore_tunnels() {
        let mut m = NetMeter::new();
        let t0 = Instant::now();
        m.sample(
            t0,
            &[iface("en0", 0, 0), iface("en1", 0, 0), iface("utun0", 0, 0)],
        );
        let r = m.sample(
            t0 + Duration::from_secs(2),
            &[
                iface("en0", 2_000, 400),
                iface("en1", 2_000, 0),
                iface("utun0", 9_999, 9_999),
            ],
        );
        assert_eq!(r.down, 2_000.0);
        assert_eq!(r.up, 200.0);
        assert_eq!((r.down_bytes, r.up_bytes), (4_000, 400));
    }

    #[test]
    fn counter_reset_and_new_interfaces_count_zero() {
        let mut m = NetMeter::new();
        let t0 = Instant::now();
        m.sample(t0, &[iface("en0", 10_000, 10_000)]);
        let r = m.sample(
            t0 + Duration::from_secs(1),
            &[iface("en0", 50, 50), iface("en5", 1_000_000, 1_000_000)],
        );
        assert_eq!((r.down, r.up), (0.0, 0.0));
    }

    #[test]
    fn primary_is_the_busiest_active_port() {
        let mut wifi = iface("en0", 100, 100);
        wifi.baudrate = 866_000_000;
        let mut down = iface("en1", 1 << 40, 0);
        down.running = false;
        let ether = iface("en7", 1_000, 1_000);
        let link = primary_link(&[wifi, down, ether, iface("lo0", 1 << 50, 0)]).unwrap();
        assert_eq!(link.name, "en7");
        assert_eq!(link.capacity_bps, 1_000_000_000);
        assert!(primary_link(&[iface("lo0", 1, 1)]).is_none());
    }

    #[test]
    fn utilization_uses_the_busier_direction() {
        let r = NetRates {
            down: 12_500_000.0, // 100 Mbps
            up: 1_000.0,
            link: Some(Link {
                name: "en0".into(),
                capacity_bps: 1_000_000_000,
            }),
            ..Default::default()
        };
        assert!((r.utilization().unwrap() - 0.1).abs() < 1e-9);
        let unknown = NetRates {
            link: Some(Link {
                name: "en0".into(),
                capacity_bps: 0,
            }),
            ..r
        };
        assert_eq!(unknown.utilization(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_real_interfaces() {
        let ifaces = read_interfaces().unwrap();
        assert!(ifaces.iter().any(|i| i.loopback), "{ifaces:?}");
    }
}
