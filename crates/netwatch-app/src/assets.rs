//! Embedded assets: fonts and theme, plus gpui-kit's Lucide icon set.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets"]
#[include = "fonts/**/*.ttf"]
#[include = "themes/*.json"]
pub struct Embedded;

/// App assets first, then gpui-kit's full Lucide catalog.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(f) = Embedded::get(path) {
            return Ok(Some(f.data));
        }
        match gpui_kit::assets::AllAssets.load(path) {
            Ok(v) => Ok(v),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut v = gpui_kit::assets::AllAssets.list(path).unwrap_or_default();
        v.extend(
            Embedded::iter()
                .filter(|p| p.starts_with(path))
                .map(|p| SharedString::from(p.to_string())),
        );
        Ok(v)
    }
}

pub fn text(path: &str) -> Option<String> {
    Embedded::get(path).map(|f| String::from_utf8_lossy(&f.data).into_owned())
}

pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    Embedded::iter()
        .filter(|p| p.starts_with("fonts/") && p.ends_with(".ttf"))
        .filter_map(|p| Embedded::get(&p).map(|f| f.data))
        .collect()
}

/// The monospace font the menu-bar icon renders its rate text with.
pub fn icon_font() -> Vec<u8> {
    Embedded::get("fonts/lilex/Lilex-Regular.ttf")
        .map(|f| f.data.into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_fonts_and_theme() {
        assert!(fonts().len() >= 3);
        assert!(!icon_font().is_empty());
        assert!(text("themes/zed-one.json").unwrap().contains("One Dark"));
    }

    #[test]
    fn falls_back_to_kit_icons() {
        assert!(AppAssets.load("icons/arrow-down.svg").unwrap().is_some());
        assert!(
            AppAssets
                .load("icons/definitely-missing.svg")
                .unwrap()
                .is_none()
        );
    }
}
