//! Rasterize `assets/icons/app/utilify.svg` into the bundle icons:
//! `utilify.png` (1024px) and `utilify.icns` (via `iconutil`).
//!
//! `cargo run -p utilify-core --example render_app_icon`
//!
//! With `-- <in.svg> <out.png>` it renders just that file instead, which is
//! handy for previewing design variants.

use resvg::{tiny_skia, usvg};
use std::path::Path;
use std::process::Command;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [input, output] = args.as_slice() {
        return preview(Path::new(input), Path::new(output));
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons/app");
    let svg = std::fs::read_to_string(dir.join("utilify.svg"))?;
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())?;

    let render = |px: u32, path: &Path| -> anyhow::Result<()> {
        let scale = px as f32 / tree.size().width();
        let mut pixmap = tiny_skia::Pixmap::new(px, px).unwrap();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap.save_png(path)?;
        Ok(())
    };

    render(1024, &dir.join("utilify.png"))?;

    let iconset = std::env::temp_dir().join("utilify.iconset");
    let _ = std::fs::remove_dir_all(&iconset);
    std::fs::create_dir_all(&iconset)?;
    for base in [16, 32, 128, 256, 512] {
        render(base, &iconset.join(format!("icon_{base}x{base}.png")))?;
        render(
            base * 2,
            &iconset.join(format!("icon_{base}x{base}@2x.png")),
        )?;
    }
    let status = Command::new("iconutil")
        .args(["-c", "icns", "-o"])
        .arg(dir.join("utilify.icns"))
        .arg(&iconset)
        .status()?;
    anyhow::ensure!(status.success(), "iconutil failed: {status}");
    println!("wrote {}", dir.display());
    Ok(())
}

fn preview(input: &Path, output: &Path) -> anyhow::Result<()> {
    let mut opt = usvg::Options {
        resources_dir: input.parent().map(Path::to_path_buf),
        ..Default::default()
    };
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(&std::fs::read_to_string(input)?, &opt)?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.save_png(output)?;
    Ok(())
}
