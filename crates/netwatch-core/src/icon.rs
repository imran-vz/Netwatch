//! The menu-bar icon: optional rate text, then a bar graph of recent
//! download rates over a storage bar, built as SVG and rasterized with
//! resvg every tick.
//!
//! ```text
//!  ▼ 9.2MB/s   ▅▇▆▇▅▅▄▃▂▃▅▆   download, one bar per sample
//!  ▲ 1.4MB/s   ██████░░░░░░   storage: solid = used, translucent = free
//! ```
//!
//! A keep-awake session adds a small pill on the left.
//!
//! The image is drawn in black with varying alpha and shown as a macOS
//! *template* image, so the menu bar tints it for light and dark mode.

use crate::history::{self, RatePoint};
use std::fmt::Write as _;

/// Height macOS gives status-item images (tray-icon scales to 18 pt).
pub const HEIGHT_PT: f32 = 18.0;
/// Samples visible in the menu-bar graph, one bar each.
pub const SLOTS: usize = 24;
/// Bar spacing and width.
const BAR_PITCH: f32 = 2.0;
const BAR_W: f32 = 1.5;
/// Graph width: one pitch per bar, minus the gap after the last.
pub const GRAPH_WIDTH_PT: f32 = SLOTS as f32 * BAR_PITCH - (BAR_PITCH - BAR_W);
/// Width of the rate text column, when shown, including the gap before the
/// graph. Fits the arrows and seven monospace characters (`9.2MB/s`).
pub const TEXT_WIDTH_PT: f32 = 47.0;
/// Width of the keep-awake pill column, when shown, including its gap.
pub const AWAKE_WIDTH_PT: f32 = 8.0;
/// Monospace family used for the rate text (embedded by the app).
pub const FONT_FAMILY: &str = "Lilex";
/// Below this peak the graph stays low, so idle chatter (a few KB/s) doesn't
/// fill the icon.
pub const MIN_PEAK: f64 = 16_000.0;

/// Bars stand on this line and reach up to `BARS_TOP`.
const BARS_BOTTOM: f32 = 11.5;
const BARS_TOP: f32 = 2.0;
/// Quiet or not-yet-sampled slots still show a stub, so the graph reads as
/// a graph.
const STUB_H: f32 = 0.75;
const BAR_Y: f32 = 14.5;
const BAR_H: f32 = 3.0;
const FONT_PT: f32 = 8.5;

/// Heights (0..=1) of the newest [`SLOTS`] download samples, oldest first,
/// scaled to their peak (at least `min_peak`). Slots not yet sampled are
/// `None`, so a fresh graph fills in from the right.
pub fn bar_heights(points: &[RatePoint], min_peak: f64) -> Vec<Option<f32>> {
    let pts = &points[points.len().saturating_sub(SLOTS)..];
    let peak = pts.iter().fold(min_peak, |m, p| m.max(p.down));
    history::bars(pts, SLOTS, SLOTS)
        .into_iter()
        .map(|b| b.map(|p| (p.down / peak).clamp(0.0, 1.0) as f32))
        .collect()
}

/// What the menu-bar icon shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrayContent<'a> {
    pub points: &'a [RatePoint],
    /// Current (download, upload) in bytes/s; `None` hides the text column.
    pub rates: Option<(f64, f64)>,
    /// Occupied fraction of the startup volume; `None` hides the bar.
    pub disk_used: Option<f64>,
    /// A keep-awake session is running: show the pill.
    pub awake: bool,
}

impl TrayContent<'_> {
    pub fn width(&self) -> f32 {
        GRAPH_WIDTH_PT
            + if self.rates.is_some() {
                TEXT_WIDTH_PT
            } else {
                0.0
            }
            + if self.awake { AWAKE_WIDTH_PT } else { 0.0 }
    }
}

fn text_column(s: &mut String, down: f64, up: f64) {
    // Arrows are drawn, not typed, so they don't depend on font coverage:
    // ▼ (received) on the download line, ▲ (sent) on the upload line.
    let _ = write!(
        s,
        r#"<path d="M0.5 3 L5.5 3 L3 6.5 Z"/><path d="M0.5 15.5 L5.5 15.5 L3 12 Z" fill-opacity="0.75"/>"#
    );
    for (y, v, opacity) in [(7.5, down, 1.0), (16.5, up, 0.75)] {
        let _ = write!(
            s,
            r#"<text x="{x}" y="{y}" font-family="{FONT_FAMILY}" font-size="{FONT_PT}" text-anchor="end" fill-opacity="{opacity}" xml:space="preserve">{t}</text>"#,
            x = TEXT_WIDTH_PT - 4.0,
            t = crate::format::rate_menu(v)
        );
    }
}

/// SVG for the menu-bar icon, in points (`content.width()` x `HEIGHT_PT`).
pub fn tray_svg(content: &TrayContent) -> String {
    let width = content.width();
    let gx = width - GRAPH_WIDTH_PT;
    let mut s = String::with_capacity(4096);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{HEIGHT_PT}" viewBox="0 0 {width} {HEIGHT_PT}" fill="black">"#
    );
    if content.awake {
        // A capsule standing upright: solid top, translucent bottom, a gap
        // between (template images are black only, so no white stripe).
        s.push_str(
            r#"<clipPath id="p"><rect x="0.75" y="3.5" width="5" height="11" rx="2.5"/></clipPath><g clip-path="url(#p)"><rect x="0.75" y="3.5" width="5" height="5.1"/><rect x="0.75" y="9.4" width="5" height="5.1" fill-opacity="0.55"/></g>"#,
        );
    }
    if let Some((down, up)) = content.rates {
        let tx = if content.awake { AWAKE_WIDTH_PT } else { 0.0 };
        let _ = write!(s, r#"<g transform="translate({tx} 0)">"#);
        text_column(&mut s, down, up);
        s.push_str("</g>");
    }
    let full = BARS_BOTTOM - BARS_TOP;
    for (i, h) in bar_heights(content.points, MIN_PEAK)
        .into_iter()
        .enumerate()
    {
        let x = gx + i as f32 * BAR_PITCH;
        let (h, opacity) = match h {
            Some(f) if f * full > STUB_H => (f * full, 1.0),
            _ => (STUB_H, 0.35),
        };
        let _ = write!(
            s,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{BAR_W}" height="{h:.2}" fill-opacity="{opacity}"/>"#,
            y = BARS_BOTTOM - h
        );
    }
    if let Some(f) = content.disk_used {
        let f = f.clamp(0.0, 1.0) as f32;
        let (x, w, r) = (gx, GRAPH_WIDTH_PT, BAR_H / 2.0);
        let _ = write!(
            s,
            r#"<rect x="{x}" y="{BAR_Y}" width="{w}" height="{BAR_H}" rx="{r}" fill-opacity="0.3"/>"#
        );
        // Clip the used part to the rounded track so its right end is square.
        let _ = write!(
            s,
            r#"<clipPath id="t"><rect x="{x}" y="{BAR_Y}" width="{w}" height="{BAR_H}" rx="{r}"/></clipPath><rect x="{x}" y="{BAR_Y}" width="{u:.2}" height="{BAR_H}" clip-path="url(#t)"/>"#,
            u = w * f
        );
    }
    s.push_str("</svg>");
    s
}

/// A straight (non-premultiplied) RGBA bitmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Rasterizes icon SVGs. Holds the font database, which is costly to build,
/// so create one and reuse it.
pub struct IconRenderer {
    options: resvg::usvg::Options<'static>,
}

impl IconRenderer {
    /// `fonts`: font files to make available to `<text>` (the rate column
    /// expects [`FONT_FAMILY`]). System fonts are not loaded.
    pub fn new(fonts: impl IntoIterator<Item = Vec<u8>>) -> Self {
        let mut options = resvg::usvg::Options::default();
        let db = options.fontdb_mut();
        for font in fonts {
            db.load_font_data(font);
        }
        Self { options }
    }

    /// Rasterize an SVG to `scale` pixels per SVG unit.
    pub fn rasterize(&self, svg: &str, scale: f32) -> anyhow::Result<Bitmap> {
        use resvg::{tiny_skia, usvg};
        let tree = usvg::Tree::from_str(svg, &self.options)?;
        let size = tree.size();
        let width = (size.width() * scale).round() as u32;
        let height = (size.height() * scale).round() as u32;
        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| anyhow::anyhow!("empty icon size {width}x{height}"))?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let rgba = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        Ok(Bitmap {
            width,
            height,
            rgba,
        })
    }

    /// The menu-bar icon at `scale` (2.0 for Retina).
    pub fn tray_icon(&self, content: &TrayContent, scale: f32) -> anyhow::Result<Bitmap> {
        self.rasterize(&tray_svg(content), scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(down: f64, up: f64) -> RatePoint {
        RatePoint { down, up }
    }

    #[test]
    fn bars_are_right_aligned_and_scaled_to_the_peak() {
        let h = bar_heights(&[p(50.0, 0.0), p(100.0, 900.0)], 1.0);
        assert_eq!(h.len(), SLOTS);
        assert!(h[..SLOTS - 2].iter().all(Option::is_none));
        // Upload doesn't count towards the download bars' scale.
        assert_eq!(h[SLOTS - 2..], [Some(0.5), Some(1.0)]);
    }

    #[test]
    fn quiet_traffic_stays_low_and_old_points_scroll_off() {
        let pts: Vec<_> = (0..40).map(|i| p(i as f64, 0.0)).collect();
        let h = bar_heights(&pts, 1_000.0);
        assert_eq!(h.len(), SLOTS);
        assert_eq!(h[0], Some(16.0 / 1_000.0));
        assert!(h.iter().all(|h| h.unwrap() < 0.05));
    }

    #[test]
    fn empty_history_draws_only_stubs() {
        let svg = tray_svg(&TrayContent::default());
        assert_eq!(svg.matches(r#"fill-opacity="0.35""#).count(), SLOTS);
        assert!(!svg.contains("clipPath"));
    }

    #[test]
    fn storage_bar_fills_the_used_share() {
        let svg = tray_svg(&TrayContent {
            disk_used: Some(0.5),
            ..Default::default()
        });
        assert!(
            svg.contains(&format!(r#"width="{:.2}""#, GRAPH_WIDTH_PT / 2.0)),
            "{svg}"
        );
        assert!(svg.contains(r#"fill-opacity="0.3""#));
    }

    #[test]
    fn awake_pill_sits_left_of_the_text() {
        let content = TrayContent {
            rates: Some((0.0, 0.0)),
            awake: true,
            ..Default::default()
        };
        assert_eq!(
            content.width(),
            AWAKE_WIDTH_PT + TEXT_WIDTH_PT + GRAPH_WIDTH_PT
        );
        let svg = tray_svg(&content);
        assert!(svg.contains(r#"clip-path="url(#p)""#));
        assert!(svg.contains(&format!(r#"translate({AWAKE_WIDTH_PT} 0)"#)));
        let bmp = IconRenderer::new([]).tray_icon(&content, 2.0).unwrap();
        assert_eq!(alpha_at(&bmp, 6, 10), 255, "solid top half");
        assert!(bmp.rgba.chunks(4).all(|c| c[3] == 0 || c[..3] == [0, 0, 0]));
    }

    fn lilex() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/lilex/Lilex-Regular.ttf"
        );
        std::fs::read(path).expect("Lilex font in assets")
    }

    fn alpha_at(bmp: &Bitmap, x: u32, y: u32) -> u8 {
        bmp.rgba[((y * bmp.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn rasterizes_at_retina_scale() {
        let pts: Vec<_> = (0..40).map(|i| p((i * 1000) as f64, 500.0)).collect();
        let content = TrayContent {
            points: &pts,
            rates: None,
            disk_used: Some(0.75),
            awake: false,
        };
        let bmp = IconRenderer::new([]).tray_icon(&content, 2.0).unwrap();
        let w = (GRAPH_WIDTH_PT * 2.0).round() as u32;
        assert_eq!((bmp.width, bmp.height), (w, 36));
        assert_eq!(bmp.rgba.len(), (w * 36 * 4) as usize);
        // Storage bar: solid at 25% across, translucent at 90%.
        let bar_y = ((BAR_Y + BAR_H / 2.0) * 2.0) as u32;
        assert_eq!(alpha_at(&bmp, w / 4, bar_y), 255);
        let free = alpha_at(&bmp, w * 9 / 10, bar_y);
        assert!((60..110).contains(&free), "free alpha {free}");
        // Newest (largest) download sample reaches near the top-right.
        assert!(alpha_at(&bmp, w - 2, (BARS_TOP * 2.0) as u32 + 1) > 200);
        // Template image: every pixel is black.
        assert!(bmp.rgba.chunks(4).all(|c| c[3] == 0 || c[..3] == [0, 0, 0]));
    }

    #[test]
    fn rate_text_widens_the_icon_and_renders_glyphs() {
        let content = TrayContent {
            points: &[],
            rates: Some((1_200_000.0, 30_000.0)),
            disk_used: None,
            awake: false,
        };
        let svg = tray_svg(&content);
        assert!(
            svg.contains(">1.2MB/s</text>") && svg.contains("> 30KB/s</text>"),
            "{svg}"
        );
        let bmp = IconRenderer::new([lilex()])
            .tray_icon(&content, 2.0)
            .unwrap();
        assert_eq!(
            bmp.width,
            ((GRAPH_WIDTH_PT + TEXT_WIDTH_PT) * 2.0).round() as u32
        );
        // Some ink in the text area (right of the arrows' column).
        let text_px = (14..90)
            .flat_map(|x| (0..36).map(move |y| (x, y)))
            .filter(|&(x, y)| alpha_at(&bmp, x, y) > 128)
            .count();
        assert!(text_px > 40, "only {text_px} text pixels");
    }
}
