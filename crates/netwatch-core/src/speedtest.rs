//! On-demand speed test, run with macOS's built-in `networkQuality` (macOS
//! 12+), which measures against Apple's CDN.

use serde::Deserialize;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const TOOL: &str = "/usr/bin/networkQuality";

/// Test length passed to the tool (`-M`).
pub const MAX_RUNTIME: Duration = Duration::from_secs(20);

/// Kill the tool if it hasn't finished by then: it can hang, for example
/// when the network drops mid-test.
pub const WATCHDOG: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, PartialEq)]
pub struct SpeedTestResult {
    /// Bits per second.
    pub down_bps: u64,
    /// Bits per second.
    pub up_bps: u64,
    /// Idle round-trip time, in milliseconds.
    pub idle_latency_ms: Option<f64>,
    /// Round trips per minute while the link is loaded; higher is better.
    pub responsiveness_rpm: Option<f64>,
    pub interface: Option<String>,
}

#[derive(Deserialize)]
struct Output {
    dl_throughput: f64,
    ul_throughput: f64,
    base_rtt: Option<f64>,
    responsiveness: Option<f64>,
    interface_name: Option<String>,
}

/// Parse the tool's `-c` (JSON) output.
pub fn parse(json: &[u8]) -> anyhow::Result<SpeedTestResult> {
    let o: Output = serde_json::from_slice(json)?;
    Ok(SpeedTestResult {
        down_bps: o.dl_throughput.max(0.0) as u64,
        up_bps: o.ul_throughput.max(0.0) as u64,
        idle_latency_ms: o.base_rtt,
        responsiveness_rpm: o.responsiveness,
        interface: o.interface_name,
    })
}

pub fn is_available() -> bool {
    std::path::Path::new(TOOL).exists()
}

/// Run a full test. Blocks for up to [`WATCHDOG`]; call it off the main
/// thread.
pub fn run() -> anyhow::Result<SpeedTestResult> {
    if !is_available() {
        anyhow::bail!("needs macOS 12 or later");
    }
    let mut child = Command::new(TOOL)
        .args(["-c", "-M", &MAX_RUNTIME.as_secs().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    // Drain stdout on a thread so a full pipe can't stall the tool.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= WATCHDOG {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("timed out");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let out = reader
        .join()
        .map_err(|_| anyhow::anyhow!("reader panicked"))??;
    if !status.success() {
        anyhow::bail!("networkQuality failed ({status})");
    }
    parse(&out).map_err(|e| anyhow::anyhow!("unexpected output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_output() {
        let json = br#"{
            "base_rtt" : 28.5,
            "dl_throughput" : 99963168,
            "ul_throughput" : 21000000.5,
            "responsiveness" : 338.28,
            "interface_name" : "en0",
            "il_h2_req_resp" : [28.0, 29.2],
            "other" : { "interface-type" : { "wifi" : 106 } }
        }"#;
        let r = parse(json).unwrap();
        assert_eq!(r.down_bps, 99_963_168);
        assert_eq!(r.up_bps, 21_000_000);
        assert_eq!(r.idle_latency_ms, Some(28.5));
        assert_eq!(r.responsiveness_rpm, Some(338.28));
        assert_eq!(r.interface.as_deref(), Some("en0"));
    }

    #[test]
    fn optional_fields_may_be_missing() {
        let r = parse(br#"{"dl_throughput": 1, "ul_throughput": 2}"#).unwrap();
        assert_eq!(r.idle_latency_ms, None);
        assert!(parse(b"not json").is_err());
        assert!(parse(br#"{"base_rtt": 1}"#).is_err());
    }
}
