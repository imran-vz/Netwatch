# Netwatch

A macOS menu-bar monitor for network activity and storage.

- **Menu-bar icon**, redrawn every tick (0.5 / 1 / 2 / 5 s): current download ▼
  and upload ▲ rates, a scrolling graph of the last 30 samples (download above
  the midline, upload below), and a storage bar. In the bar, the translucent
  track is total capacity, the solid fill is used space, and the rest is free.
  The icon is an SVG rendered with resvg and shown as a template image, so it
  follows the light/dark menu bar.
- **Left click** opens a dashboard: rates, a 2-minute graph, link capacity and
  how much of it is in use, totals for this session, and storage.
- **Right click** opens the menu, which has the same numbers, the refresh
  interval, a "Show Speeds in Menu Bar" toggle, and Quit.

## Build and run

```sh
cargo run -p netwatch-app            # debug, unbundled
cargo packager --release -p netwatch-app --formats app   # Netwatch.app (LSUIElement, no Dock icon)
```

Debug builds open the dashboard at launch when `NETWATCH_DEV_OPEN_DASHBOARD=1`
is set. `cargo run -p netwatch-core --example render_icon -- /tmp/out 8`
samples for 8 s and writes the menu-bar icon as PNGs.

The app icon's source is `assets/icons/app/netwatch.svg`. After editing it, run
`cargo run -p netwatch-core --example render_app_icon` to regenerate
`netwatch.png` and `netwatch.icns`, which the packager bundles.

## Layout

It uses the same stack as [Guidy](https://github.com/imran-vz/Guidy): a Rust workspace, GPUI through
`gpui-kit = "=0.7.3"`, `tray-icon` for the status item, objc2 for AppKit, and
Guidy's Zed One theme and IBM Plex Sans / Lilex fonts.

```
crates/
├─ netwatch-core/   No UI and no gpui dependency.
│    net.rs       sysctl(NET_RT_IFLIST2): 64-bit byte counters + link speed per
│                 interface; NetMeter turns readings into rates. Only physical
│                 `enN` ports are counted (VPN `utun` traffic also crosses them).
│    disk.rs      startup volume: Finder's "important usage" capacity, statvfs fallback
│    monitor.rs   one tick: rates, rolling history, session totals, cached disk
│    icon.rs      menu-bar SVG + resvg rasterizer; `mirrored_graph` geometry is
│                 shared with the dashboard
│    format.rs, history.rs, settings.rs (<Application Support>/dev.netwatch.app/settings.json)
└─ netwatch-app/    binary `netwatch`
     lib.rs       startup (Accessory policy), sampler timer, tray event loop
     tray.rs      status item, menu, icon updates (main thread only)
     dashboard.rs GPUI popover: canvas graph, capacity meters
     placement.rs dashboard position under the clicked status item
```

### Notes

- **Link capacity** is the driver-reported `ifi_baudrate`. For Ethernet that's
  the negotiated speed. For Wi-Fi it's the current PHY rate, which moves with
  signal quality. Usage = the busier direction ÷ capacity, since links are
  full duplex.
- The "primary" interface is the active `enN` port that has carried the most
  traffic since boot.
