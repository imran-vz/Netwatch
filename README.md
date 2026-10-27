# Utilify

macOS menu-bar utilities: a monitor for network activity and storage, keep
awake, fan control and temperatures, and a data workbench that ports [CyberChef](https://github.com/gchq/CyberChef)'s
operations to native Rust. (Utilify was called Netwatch; on first launch it
moves the old `dev.netwatch.app` data directory to `dev.utilify.app`.)

- **Menu-bar icon**, redrawn every tick (0.5 / 1 / 2 / 5 s): current download ▼
  and upload ▲ rates (`9.2MB/s`, fixed width so the item doesn't jitter), a
  bar graph of the last 24 download samples, and a storage bar under it. In the bar, the translucent
  track is total capacity, the solid fill is used space, and the rest is free.
  The icon is an SVG rendered with resvg and shown as a template image, so it
  follows the light/dark menu bar. It can also show a temperature (any
  sensor; CPU Core Average by default) and the fastest fan's speed, as
  `80°C` over `5775rpm`, alongside the rates or instead of them (Settings
  tab, or the menu's "Show Temperature/Fan Speed in Menu Bar").
- **Left click** opens a dashboard in six tabs: **Overview** (the 2-minute
  rate graph, current rates, storage, keep awake); **Network** (link capacity and use,
  session totals, and an on-demand speed test with latency idle vs under
  load); **Storage** (used space over the last 30 days); **Awake** (keep-awake
  sessions); **Fans** (fan control and temperature sensors, below);
  **Settings** (refresh interval, rates, temperature and fan speed in the
  menu bar, storage colour thresholds, Quit).
- **Storage colours**: the storage bar is green below 50% used, yellow from
  50%, red from 80% (both adjustable in Settings). Crossing into red posts a
  notification (on by default; only from the bundled app, as macOS needs a
  bundle identity). It fires once per crossing and re-arms after usage
  drops 2 points below the red line; starting up already in the red doesn't
  notify. The look comes from
  `design/dashboard-mock.html`.
- **Keep awake**, with the options from
  [Amphetamine](https://apps.apple.com/app/amphetamine/id937984704)'s menu,
  under the menu's Keep Awake submenu and the dashboard's Awake tab:
  - Start a session: Indefinitely (⌘I), Minutes (5–45), Hours (1–12), a
    custom length or "until 18:30" (Awake tab), while an app runs (pick from
    the running apps; ends when it quits), while a file downloads (any
    `.download`/`.crdownload`/`.part` in Downloads; ends a minute after the
    last one goes; ⌘F), or while the network is busy (ends after 2 minutes
    under 50 KB/s). End Current Session is ⌘X.
  - Session details: time remaining and how it started (manual or at launch).
  - Options: allow display sleep, allow system sleep when the display is
    closed (blocking it only works on AC power; macOS ignores it on battery),
    and allow the screen saver after N minutes without input (blocking display
    sleep also blocks the screen saver, so Utilify starts it).
  - Quick settings: start a session at launch, notify when a session ends,
    launch Utilify at login (macOS 13+, bundled app only).
  - While a session runs, a pill shows left of the menu-bar icon. The total
    time kept awake is stored across launches (`awake.json`). Sessions hold
    IOKit power assertions, which macOS drops if Utilify quits or crashes;
    check them with `pmset -g assertions`.
- **Fans**, like [Macs Fan Control](https://crystalidea.com/macs-fan-control),
  in the dashboard's Fans tab and the menu's Fans submenu:
  - Each fan ("Left side"/"Right side" on two-fan Macs, else "Fan N") with
    its current, minimum and maximum speed, and its control: **Auto** (the
    system's own), **Constant** (a speed in 100 rpm steps), or **Sensor**
    (pick a sensor and two temperatures: the fan stays at its minimum up to
    the first and reaches its maximum at the second, linear between). Curves
    only lower the fan once the sensor is 2 °C below the reading that set the
    speed, and move it at most 1000 rpm/s up and 250 rpm/s down, so it
    doesn't hunt. A curve whose sensor disappears falls back to automatic.
  - Presets: **Automatic** (every fan automatic), **Full blast** (every fan
    at its maximum), and your own: Save the current controls under a name,
    rename (pencil) or delete them. Editing a fan inside a preset edits the
    preset; otherwise the edit becomes the "Custom" set. The menu lists the
    fans with their control and the presets with a check on the active one,
    plus Show Fans…. Kept in `<Application Support>/dev.utilify.app/fans.json`.
  - Every temperature sensor, grouped (CPU, GPU, SoC, battery, storage,
    ...), refreshed every 2 s.
  - Setting speeds needs the fan helper (see Install). Without it, or for a
    non-admin user, the tab shows a banner with what to do, and the controls
    stay disabled; speeds and temperatures still show. It also warns when
    another tool (e.g. Macs Fan Control) holds the fans, and shows errors the
    helper reports. Quitting Utilify, or choosing Automatic, sends every fan
    back to automatic; if Utilify crashes, the helper does it when the
    connection drops. Intel Macs are read-only.
- **Workbench** (⌘B in the menu, or the chef's hat in the dashboard header)
  opens a full window in CyberChef's layout: operations on the left (search,
  favourites, CyberChef's categories), the recipe in the middle, input over
  output on the right. Click or drag operations into the recipe; each step can
  be disabled, given a breakpoint (Step runs on from it), removed or dragged
  into place. Auto bake reruns the recipe on every change, on a background
  thread; ⌘↩ bakes by hand. Input is typed text or a file (Open file, or drop
  one); output copies, saves, or becomes the next input. Paste takes a recipe
  as CyberChef JSON, its `Op_Name(args)` chef format or a CyberChef URL (with
  its input); Copy gives chef format. The recipe, input (up to 256 KB), auto
  bake and favourites are kept in `<Application Support>/dev.utilify.app/workbench.json`.
  So far CyberChef's "Data format" category is ported; the list shows the
  rest as "soon". The codecs come from maintained libraries and the
  utilify-codecs crates (see `crates/utilify-chef/DEPENDENCIES.md`).
  While the window is open Utilify shows in the Dock.
- **Right click** opens the menu: Open Dashboard, Open Workbench, Keep Awake,
  Fans, the refresh interval, "Show Speeds / Temperature / Fan Speed in Menu
  Bar" toggles, and Quit. Hovering the item
  shows the current rates, free space and any keep-awake session.

## Install

```sh
./install.sh
```

Builds the current source and installs it, so re-run it after pulling or
editing. It needs Rust (https://rustup.rs) and a checkout of
[utilify-codecs](https://github.com/imran-vz/utilify-codecs) next to this
one (`../utilify-codecs`), which the workbench uses by path. The script installs
`cargo-packager` if it's missing, builds `Utilify.app`, puts
it in `/Applications` (or `~/Applications` if that isn't writable; override
with `UTILIFY_INSTALL_DIR`), and launches it (skip that with
`UTILIFY_NO_LAUNCH=1`).

On Apple Silicon it also installs the **fan helper**, which lets Utilify set
fan speeds. Reading fans and temperatures works for any user, but writing to
the SMC (the chip that runs the fans) needs root, so `utilify-fanhelper` runs
as a root LaunchDaemon: `/Library/PrivilegedHelperTools/dev.utilify.fanhelper`,
loaded by `/Library/LaunchDaemons/dev.utilify.fanhelper.plist`, logging to
`/var/log/utilify-fanhelper.log`. The script asks for your password once
(`sudo`) to put it there. The helper only accepts admin users (its socket,
`/var/run/dev.utilify.fanhelper.sock`, is `root:admin` 0660), only changes
fan modes and target speeds (`Ftst`, `F{n}Md`, `F{n}Tg`), clamps speeds to
each fan's range, and returns the fans to automatic when Utilify disconnects
or goes 15 s without a request, when it's stopped, and after a crash. Skip it
with `UTILIFY_NO_FAN_HELPER=1`; remove it with
`scripts/uninstall-fan-helper.sh`, which stops it (fans back to automatic)
and deletes those files. Intel Macs skip it: fans are read-only there.

## Build and run

```sh
cargo run -p utilify-app            # debug, unbundled
cargo packager --release -p utilify-app --formats app   # Utilify.app (LSUIElement, no Dock icon)
```

Debug builds open the workbench at launch when `UTILIFY_DEV_OPEN_WORKBENCH=1`
is set, and the dashboard when `UTILIFY_DEV_OPEN_DASHBOARD=1`
is set; add `UTILIFY_DEV_TAB=network` (or `storage`, `awake`, `fans`, `settings`) to pick the
tab, `UTILIFY_DEV_SPEED_TEST=1` to start a speed test, `UTILIFY_DEV_AWAKE=1` to
start a 30-minute keep-awake session, and
`UTILIFY_DEV_PIN=1` to keep it open when focus moves away.
`UTILIFY_DEV_QUIT_AFTER=10` quits through the normal path after 10 s.
To try fan control without touching the real fans, run the helper with its
in-memory SMC and point a debug build at it:

```sh
cargo run -p utilify-fanhelper -- --fake --no-root-check --socket /tmp/fans.sock &
UTILIFY_DEV_FAN_SOCKET=/tmp/fans.sock UTILIFY_DEV_OPEN_DASHBOARD=1 UTILIFY_DEV_TAB=fans cargo run -p utilify-app
```

`cargo test -p utilify-core real_helper -- --ignored` (after building
`utilify-fanhelper`) runs the fan controller against that fake helper. `cargo run -p utilify-core --example render_icon -- /tmp/out 8`
samples for 8 s and writes the menu-bar icon as PNGs (`UTILIFY_AWAKE=1` adds
the keep-awake pill, `UTILIFY_FANS=1` the temperature and fan readout).

The app icon's source is `assets/icons/app/utilify.svg`. After editing it, run
`cargo run -p utilify-core --example render_app_icon` to regenerate
`utilify.png` and `utilify.icns`, which the packager bundles.

## Layout

It uses the same stack as [Guidy](https://github.com/imran-vz/Guidy): a Rust workspace, GPUI through
`gpui-kit = "=0.7.3"`, `tray-icon` for the status item, objc2 for AppKit, and
Guidy's Zed One theme. The dashboard uses Geist / Geist Mono; the menu-bar
text uses Lilex.

```
crates/
├─ utilify-core/   No UI and no gpui dependency.
│    net.rs       sysctl(NET_RT_IFLIST2): 64-bit byte counters + link speed per
│                 interface; NetMeter turns readings into rates. Only physical
│                 `enN` ports are counted (VPN `utun` traffic also crosses them).
│    disk.rs      startup volume: Finder's "important usage" capacity, statvfs fallback
│    disk_history.rs  used space every 15 min, kept 30 days
│                 (<Application Support>/dev.utilify.app/disk_history.json)
│    awake.rs     keep-awake sessions, IOKit power assertions, idle time, and the
│                 awake total (<Application Support>/dev.utilify.app/awake.json)
│    speedtest.rs runs macOS's `networkQuality -c` (macOS 12+), with a watchdog
│    sensors.rs   named temperature sensors (HID + SMC), Apple Silicon
│    fans.rs      fan controls, curves with hysteresis, presets
│                 (<Application Support>/dev.utilify.app/fans.json)
│    fans/        client.rs (the helper's socket), controller.rs (one tick:
│                 targets, only-changed SETs, PINGs, AUTO ALL), service.rs
│                 (the 2 s background thread)
│    monitor.rs   one tick: rates, rolling history, session totals, cached disk
│    icon.rs      menu-bar SVG + resvg rasterizer
│    format.rs, history.rs, settings.rs (<Application Support>/dev.utilify.app/settings.json)
├─ utilify-smc/    The SMC: keys, typed values, fans, temperatures (IOKit, no root
│                 needed to read), and the fan helper's wire protocol.
│                 `cargo run -p utilify-smc --example fans` prints fans and all T* keys.
├─ utilify-fanhelper/  binary `utilify-fanhelper`, the root LaunchDaemon that sets
│                 fan speeds. `--socket <path> --no-root-check` (and `--fake` for an
│                 in-memory SMC) run it as a normal user for testing.
├─ utilify-chef/   The workbench engine. No UI and no gpui dependency.
│    dish.rs      the value between operations, with CyberChef's text/bytes rules
│    args.rs      argument kinds (CyberChef's), values and typed access
│    recipe.rs    steps, baking, breakpoints; CyberChef JSON, chef format, URLs
│    ops/         the operations, one module per CyberChef category
│    catalog.rs   CyberChef's full category list, ported or not
│    session.rs   the workbench state kept across launches
│    tests/cyberchef_vectors.rs  CyberChef's own operation tests, run on the port
└─ utilify-app/    binary `utilify`
     lib.rs       startup (Accessory policy), sampler timer, tray event loop
     tray.rs      status item, menu, icon updates (main thread only)
     dashboard.rs GPUI popover: the six tabs and window open/close
     dashboard/awake.rs  the Awake tab and Overview's keep-awake control
     dashboard/fans.rs   the Fans tab: banners, presets, fans, sensors
     system.rs    running apps (NSWorkspace) and launch at login (SMAppService)
     ui.rs        the dashboard's dark palette and building blocks
     placement.rs dashboard position under the clicked status item
     workbench.rs the workbench window: state, baking, persistence, header
     workbench/   operations list, recipe steps, input/output, widgets
```

### Porting CyberChef operations

Names, arguments and defaults match CyberChef, so its recipes load unchanged,
but each operation follows its format's specification rather than CyberChef:
where CyberChef deviates from the spec, the port doesn't copy it.
`crates/utilify-chef/tests/vectors/cyberchef.json` holds CyberChef's operation
tests (exported by `scripts/cyberchef/export.mjs`); every vector whose
operations are all ported must pass, apart from the places CyberChef deviates from
the spec, listed in `known_differences.txt`. `VECTORS=Base64 cargo test -p utilify-chef --test
cyberchef_vectors -- --nocapture` runs a subset. For behaviour the vectors
don't cover, `scripts/cyberchef/oracle.mjs` bakes recipes with real CyberChef;
see `scripts/cyberchef/README.md`. Ported operations follow CyberChef
(Apache-2.0, Crown Copyright). `crates/utilify-chef/PORTING.md` has the
port's status and the rule for dependencies: maintained, well-used libraries
for encodings, formats and crypto, with our code as the CyberChef adapter.

### Notes

- **Link capacity** is the driver-reported `ifi_baudrate`. For Ethernet that's
  the negotiated speed. For Wi-Fi it's the current PHY rate, which moves with
  signal quality. Usage = the busier direction ÷ capacity, since links are
  full duplex.
- The "primary" interface is the active `enN` port that has carried the most
  traffic since boot.
