//! The menu-bar icon: optional rate text, then a scrolling network graph
//! over a storage bar, built as SVG and rasterized with resvg every tick.
//!
//! ```text
//!  ▼ 1.2M   ▁▂▅▇▅▃▁▁▂▃   download, growing up from the midline
//!            ───────────
//!  ▲  30K   ▁▁▂▁ ▁▁▂▁   upload, growing down (fainter)
//!
//!            ██████░░░░░  storage: solid = used, translucent = free
//! ```
//!
//! The image is drawn in black with varying alpha and shown as a macOS
//! *template* image, so the menu bar tints it for light and dark mode.
//!
//! [`mirrored_graph`] is the shared geometry; the dashboard draws the same
//! shape at a larger size.

use crate::history::{History, RatePoint};
use std::fmt::Write as _;

/// Height macOS gives status-item images (tray-icon scales to 18 pt).
pub const HEIGHT_PT: f32 = 18.0;
/// Graph width; one sample per point.
pub const GRAPH_WIDTH_PT: f32 = 30.0;
/// Width of the rate text column, when shown.
pub const TEXT_WIDTH_PT: f32 = 29.0;
/// Monospace family used for the rate text (embedded by the app).
pub const FONT_FAMILY: &str = "Lilex";
/// Samples visible in the menu-bar graph.
pub const SLOTS: usize = 30;
/// Below this peak the graph stays flat-ish, so idle chatter (a few KB/s)
/// doesn't fill the icon.
pub const MIN_PEAK: f64 = 16_000.0;

const MID: f32 = 7.0;
const DOWN_H: f32 = 6.5;
const UP_H: f32 = 5.0;
const BAR_Y: f32 = 14.5;
const BAR_H: f32 = 3.0;
const FONT_PT: f32 = 8.5;

/// Polygons for a two-sided area graph.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphGeometry {
    /// Closed polygon above the midline (download).
    pub down: Vec<(f32, f32)>,
    /// Closed polygon below the midline (upload).
    pub up: Vec<(f32, f32)>,
    /// Rate (bytes/s) that maps to full height.
    pub peak: f64,
}

/// Layout for [`mirrored_graph`]: `x`..`x + width` horizontally, with
/// download growing up from `mid` by at most `down_h` and upload growing
/// down by at most `up_h`.
#[derive(Clone, Copy, Debug)]
pub struct GraphFrame {
    pub x: f32,
    pub width: f32,
    pub mid: f32,
    pub down_h: f32,
    pub up_h: f32,
}

/// Map the newest `slots` points onto `frame`, newest at the right edge,
/// with a shared scale for both directions.
pub fn mirrored_graph(
    points: &[RatePoint],
    slots: usize,
    frame: GraphFrame,
    min_peak: f64,
) -> GraphGeometry {
    let slots = slots.max(2);
    let pts = &points[points.len().saturating_sub(slots)..];
    let peak = History::peak(pts).max(min_peak);
    let step = frame.width / (slots - 1) as f32;
    // Right-align: the newest sample sits on the right edge.
    let x_of = |i: usize| frame.x + frame.width - (pts.len() - 1 - i) as f32 * step;

    let mut down = Vec::with_capacity(pts.len() + 2);
    let mut up = Vec::with_capacity(pts.len() + 2);
    if let (Some(_), Some(_)) = (pts.first(), pts.last()) {
        let (x0, x1) = (x_of(0), x_of(pts.len() - 1));
        down.push((x0, frame.mid));
        up.push((x0, frame.mid));
        for (i, p) in pts.iter().enumerate() {
            let x = x_of(i);
            down.push((
                x,
                frame.mid - frame.down_h * (p.down / peak).min(1.0) as f32,
            ));
            up.push((x, frame.mid + frame.up_h * (p.up / peak).min(1.0) as f32));
        }
        down.push((x1, frame.mid));
        up.push((x1, frame.mid));
    }
    GraphGeometry { down, up, peak }
}

fn polygon(out: &mut String, pts: &[(f32, f32)], opacity: f32) {
    if pts.len() < 3 {
        return;
    }
    out.push_str("<path d=\"");
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(out, "{}{x:.2} {y:.2}", if i == 0 { "M" } else { " L" });
    }
    let _ = write!(out, " Z\" fill-opacity=\"{opacity}\"/>");
}

/// What the menu-bar icon shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrayContent<'a> {
    pub points: &'a [RatePoint],
    /// Current (download, upload) in bytes/s; `None` hides the text column.
    pub rates: Option<(f64, f64)>,
    /// Occupied fraction of the startup volume; `None` hides the bar.
    pub disk_used: Option<f64>,
}

impl TrayContent<'_> {
    pub fn width(&self) -> f32 {
        GRAPH_WIDTH_PT
            + if self.rates.is_some() {
                TEXT_WIDTH_PT
            } else {
                0.0
            }
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
            x = TEXT_WIDTH_PT - 2.0,
            t = crate::format::rate_compact(v)
        );
    }
}

/// SVG for the menu-bar icon, in points (`content.width()` x `HEIGHT_PT`).
pub fn tray_svg(content: &TrayContent) -> String {
    let width = content.width();
    let gx = width - GRAPH_WIDTH_PT;
    let g = mirrored_graph(
        content.points,
        SLOTS,
        GraphFrame {
            x: gx + 0.5,
            width: GRAPH_WIDTH_PT - 1.0,
            mid: MID,
            down_h: DOWN_H,
            up_h: UP_H,
        },
        MIN_PEAK,
    );
    let mut s = String::with_capacity(2048);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{HEIGHT_PT}" viewBox="0 0 {width} {HEIGHT_PT}" fill="black">"#
    );
    if let Some((down, up)) = content.rates {
        text_column(&mut s, down, up);
    }
    // Midline, so a quiet network still reads as a graph.
    let _ = write!(
        s,
        r#"<rect x="{gx}" y="{}" width="{GRAPH_WIDTH_PT}" height="0.75" fill-opacity="0.35"/>"#,
        MID - 0.375
    );
    polygon(&mut s, &g.down, 1.0);
    polygon(&mut s, &g.up, 0.6);
    if let Some(f) = content.disk_used {
        let f = f.clamp(0.0, 1.0) as f32;
        let (x, w, r) = (gx + 0.5, GRAPH_WIDTH_PT - 1.0, BAR_H / 2.0);
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

    const FRAME: GraphFrame = GraphFrame {
        x: 0.0,
        width: 10.0,
        mid: 10.0,
        down_h: 10.0,
        up_h: 5.0,
    };

    fn p(down: f64, up: f64) -> RatePoint {
        RatePoint { down, up }
    }

    #[test]
    fn graph_is_right_aligned_and_scaled_to_the_peak() {
        let g = mirrored_graph(&[p(50.0, 0.0), p(100.0, 100.0)], 11, FRAME, 1.0);
        assert_eq!(g.peak, 100.0);
        // Closing points on the midline, samples one step apart at the right.
        assert_eq!(g.down, [(9.0, 10.0), (9.0, 5.0), (10.0, 0.0), (10.0, 10.0)]);
        assert_eq!(g.up, [(9.0, 10.0), (9.0, 10.0), (10.0, 15.0), (10.0, 10.0)]);
    }

    #[test]
    fn quiet_traffic_stays_low_and_old_points_scroll_off() {
        let pts: Vec<_> = (0..20).map(|i| p(i as f64, 0.0)).collect();
        let g = mirrored_graph(&pts, 11, FRAME, 1_000.0);
        assert_eq!(g.peak, 1_000.0);
        assert_eq!(g.down.len(), 11 + 2);
        assert_eq!(g.down[1].0, 0.0);
        assert!(g.down.iter().all(|&(_, y)| y > 9.0));
    }

    #[test]
    fn empty_history_draws_nothing() {
        let g = mirrored_graph(&[], 11, FRAME, 1.0);
        assert!(g.down.is_empty() && g.up.is_empty());
        let svg = tray_svg(&TrayContent::default());
        assert!(!svg.contains("<path"));
        assert!(!svg.contains("clipPath"));
    }

    #[test]
    fn storage_bar_fills_the_used_share() {
        let svg = tray_svg(&TrayContent {
            disk_used: Some(0.5),
            ..Default::default()
        });
        assert!(svg.contains(r#"width="14.50""#), "{svg}");
        assert!(svg.contains(r#"fill-opacity="0.3""#));
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
        };
        let bmp = IconRenderer::new([]).tray_icon(&content, 2.0).unwrap();
        assert_eq!((bmp.width, bmp.height), (60, 36));
        assert_eq!(bmp.rgba.len(), 60 * 36 * 4);
        // Storage bar: solid at 25% across, translucent at 90%.
        let bar_y = ((BAR_Y + BAR_H / 2.0) * 2.0) as u32;
        assert_eq!(alpha_at(&bmp, 15, bar_y), 255);
        let free = alpha_at(&bmp, 54, bar_y);
        assert!((60..110).contains(&free), "free alpha {free}");
        // Newest (largest) download sample reaches near the top-right.
        assert!(alpha_at(&bmp, 57, 3) > 200);
        // Template image: every pixel is black.
        assert!(bmp.rgba.chunks(4).all(|c| c[3] == 0 || c[..3] == [0, 0, 0]));
    }

    #[test]
    fn rate_text_widens_the_icon_and_renders_glyphs() {
        let content = TrayContent {
            points: &[],
            rates: Some((1_200_000.0, 30_000.0)),
            disk_used: None,
        };
        let svg = tray_svg(&content);
        assert!(
            svg.contains(">1.2M</text>") && svg.contains("> 30K</text>"),
            "{svg}"
        );
        let bmp = IconRenderer::new([lilex()])
            .tray_icon(&content, 2.0)
            .unwrap();
        assert_eq!(bmp.width, ((GRAPH_WIDTH_PT + TEXT_WIDTH_PT) * 2.0) as u32);
        // Some ink in the text area (left of the arrows' column is the text).
        let text_px = (14..60)
            .flat_map(|x| (0..36).map(move |y| (x, y)))
            .filter(|&(x, y)| alpha_at(&bmp, x, y) > 128)
            .count();
        assert!(text_px > 40, "only {text_px} text pixels");
    }
}
