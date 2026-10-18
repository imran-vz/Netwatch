# Netwatch

A macOS menu-bar monitor for network activity and storage.

- **Menu-bar icon**, redrawn every tick (0.5 / 1 / 2 / 5 s): current download ▼
  and upload ▲ rates (`9.2MB/s`, fixed width so the item doesn't jitter), a
  bar graph of the last 24 download samples, and a storage bar under it. In the bar, the translucent
  track is total capacity, the solid fill is used space, and the rest is free.
  The icon is an SVG rendered with resvg and shown as a template image, so it
  follows the light/dark menu bar.
- **Left click** opens a dashboard in four tabs: **Overview** (the 2-minute
  rate graph, current rates, storage); **Network** (link capacity and use,
  session totals, and an on-demand speed test with latency idle vs under
  load); **Storage** (used space over the last 30 days); **Settings** (refresh
  interval, rates in the menu bar, storage colour thresholds, Quit).
- **Storage colours**: the storage bar is green below 50% used, yellow from
  50%, red from 80% (both adjustable in Settings). Crossing into red posts a
  notification (on by default; only from the bundled app, as macOS needs a
  bundle identity). It fires once per crossing and re-arms after usage
  drops 2 points below the red line; starting up already in the red doesn't
  notify. The look comes from
  `design/dashboard-mock.html`.
- **Right click** opens the menu: Open Dashboard, the refresh interval, a
  "Show Speeds in Menu Bar" toggle, and Quit. Hovering the item shows the
  current rates and free space.

## Install

```sh
./install.sh
```

Builds the current source and installs it, so re-run it after pulling or
editing. It needs Rust (https://rustup.rs). The script installs
`cargo-packager` if it's missing, builds `Netwatch.app`, puts
it in `/Applications` (or `~/Applications` if that isn't writable; override
with `NETWATCH_INSTALL_DIR`), and launches it (skip that with
`NETWATCH_NO_LAUNCH=1`).

## Build and run

```sh
cargo run -p netwatch-app            # debug, unbundled
cargo packager --release -p netwatch-app --formats app   # Netwatch.app (LSUIElement, no Dock icon)
```

Debug builds open the dashboard at launch when `NETWATCH_DEV_OPEN_DASHBOARD=1`
is set; add `NETWATCH_DEV_TAB=network` (or `storage`, `settings`) to pick the
tab, `NETWATCH_DEV_SPEED_TEST=1` to start a speed test, and
`NETWATCH_DEV_PIN=1` to keep it open when focus moves away. `cargo run -p netwatch-core --example render_icon -- /tmp/out 8`
samples for 8 s and writes the menu-bar icon as PNGs.

The app icon's source is `assets/icons/app/netwatch.svg`. After editing it, run
`cargo run -p netwatch-core --example render_app_icon` to regenerate
`netwatch.png` and `netwatch.icns`, which the packager bundles.

## Layout

It uses the same stack as [Guidy](https://github.com/imran-vz/Guidy): a Rust workspace, GPUI through
`gpui-kit = "=0.7.3"`, `tray-icon` for the status item, objc2 for AppKit, and
Guidy's Zed One theme. The dashboard uses Geist / Geist Mono; the menu-bar
text uses Lilex.

```
crates/
├─ netwatch-core/   No UI and no gpui dependency.
│    net.rs       sysctl(NET_RT_IFLIST2): 64-bit byte counters + link speed per
│                 interface; NetMeter turns readings into rates. Only physical
│                 `enN` ports are counted (VPN `utun` traffic also crosses them).
│    disk.rs      startup volume: Finder's "important usage" capacity, statvfs fallback
│    disk_history.rs  used space every 15 min, kept 30 days
│                 (<Application Support>/dev.netwatch.app/disk_history.json)
│    speedtest.rs runs macOS's `networkQuality -c` (macOS 12+), with a watchdog
│    monitor.rs   one tick: rates, rolling history, session totals, cached disk
│    icon.rs      menu-bar SVG + resvg rasterizer
│    format.rs, history.rs, settings.rs (<Application Support>/dev.netwatch.app/settings.json)
└─ netwatch-app/    binary `netwatch`
     lib.rs       startup (Accessory policy), sampler timer, tray event loop
     tray.rs      status item, menu, icon updates (main thread only)
     dashboard.rs GPUI popover: the four tabs and window open/close
     ui.rs        the dashboard's dark palette and building blocks
     placement.rs dashboard position under the clicked status item
```

### Notes

- **Link capacity** is the driver-reported `ifi_baudrate`. For Ethernet that's
  the negotiated speed. For Wi-Fi it's the current PHY rate, which moves with
  signal quality. Usage = the busier direction ÷ capacity, since links are
  full duplex.
- The "primary" interface is the active `enN` port that has carried the most
  traffic since boot.
