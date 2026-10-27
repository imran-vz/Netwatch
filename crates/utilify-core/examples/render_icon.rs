//! Sample the real counters for a few seconds and write the menu-bar icon as
//! PNGs (template black, plus previews on light and dark menu bars).
//!
//! `cargo run -p utilify-core --example render_icon -- [out_dir] [seconds]`
//! (`UTILIFY_AWAKE=1` adds the keep-awake pill, `UTILIFY_FANS=1` the
//! temperature and fan readout)

use resvg::tiny_skia::{Color, IntSize, Pixmap};
use std::time::{Duration, Instant};
use utilify_core::icon::{IconRenderer, Readout, TrayContent};
use utilify_core::monitor::Monitor;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    std::fs::create_dir_all(&out)?;

    let mut m = Monitor::default();
    for _ in 0..=secs * 2 {
        m.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(500));
    }
    let font = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/lilex/Lilex-SemiBold.ttf"
    ))?;
    let r = m.rates();
    println!(
        "down {:.0} B/s, up {:.0} B/s, link {:?}, disk {:?}",
        r.down,
        r.up,
        r.link,
        m.disk()
    );
    let points = m.history().tail(utilify_core::icon::SLOTS);
    let content = TrayContent {
        points: &points,
        rates: Some((r.down, r.up)),
        disk_used: m.disk().map(|d| d.used_fraction()),
        // UTILIFY_AWAKE=1 previews the keep-awake pill.
        awake: std::env::var_os("UTILIFY_AWAKE").is_some(),
        // UTILIFY_FANS=1 adds the readout: CPU average and fastest fan.
        readout: if std::env::var_os("UTILIFY_FANS").is_some() {
            let fans = utilify_smc::Smc::open()
                .map(|smc| utilify_core::fans::read(&smc).fans)
                .unwrap_or_default();
            let rpm = fans.iter().map(|f| f.current).reduce(f32::max);
            Readout {
                celsius: Some(utilify_core::sensors::cpu_average(
                    &utilify_core::sensors::read_all(),
                )),
                rpm: Some(rpm),
            }
        } else {
            Readout::default()
        },
    };
    let bmp = IconRenderer::new([font]).tray_icon(&content, 4.0)?;
    let size = IntSize::from_wh(bmp.width, bmp.height).unwrap();

    // Previews: tint the template like the menu bar does.
    for (name, bg, ink) in [
        ("light", [236, 236, 236], [0, 0, 0]),
        ("dark", [40, 40, 44], [255, 255, 255]),
    ] {
        let mut px = Pixmap::new(bmp.width + 16, bmp.height + 8).unwrap();
        px.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
        let data = px.data_mut();
        let stride = (bmp.width + 16) as usize * 4;
        for (i, c) in bmp.rgba.chunks(4).enumerate() {
            let (x, y) = (i % bmp.width as usize + 8, i / bmp.width as usize + 4);
            let a = c[3] as u32;
            let o = y * stride + x * 4;
            for k in 0..3 {
                data[o + k] = ((ink[k] * a + bg[k] as u32 * (255 - a)) / 255) as u8;
            }
        }
        px.save_png(out.join(format!("icon-{name}.png")))?;
    }
    let mut premul = bmp.rgba.clone();
    for c in premul.chunks_mut(4) {
        let a = c[3] as u32;
        for v in &mut c[..3] {
            *v = (*v as u32 * a / 255) as u8;
        }
    }
    Pixmap::from_vec(premul, size)
        .unwrap()
        .save_png(out.join("icon-template.png"))?;
    Ok(())
}
