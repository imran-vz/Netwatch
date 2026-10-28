//! Zed One Dark / One Light themes (shared with Guidy) and the UI fonts,
//! following the OS appearance. The dashboard and workbench draw with their
//! own palette ([`crate::ui::color`]), which follows it too.

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Window, WindowAppearance};

pub const UI_FONT: &str = "Geist";
pub const MONO_FONT: &str = "Geist Mono";
pub const DARK_THEME: &str = "One Dark";
pub const LIGHT_THEME: &str = "One Light";

/// Register fonts and the theme set; call once after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(crate::assets::fonts()) {
        log::warn!("Failed to register fonts: {e}");
    }
    let json = crate::assets::text("themes/zed-one.json").expect("theme json is embedded");
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(&json) {
        log::error!("Failed to load themes: {e}");
        return;
    }
    let registry = ThemeRegistry::global(cx);
    let dark = registry.themes().get(DARK_THEME).cloned();
    let light = registry.themes().get(LIGHT_THEME).cloned();
    Theme::update(cx, |t| {
        if let Some(d) = dark {
            t.dark_theme = d;
        }
        if let Some(l) = light {
            t.light_theme = l;
        }
    });
    follow_system(cx);
}

pub fn mode_for(appearance: WindowAppearance) -> ThemeMode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    }
}

/// Match the current OS appearance: gpui-kit's theme (text fields) and our
/// own palette ([`crate::ui::color`]).
pub fn follow_system(cx: &mut App) {
    let mut mode = mode_for(cx.window_appearance());
    // Debug builds: UTILIFY_DEV_APPEARANCE=light|dark overrides the OS, for
    // checking both palettes without changing the system setting.
    #[cfg(debug_assertions)]
    match std::env::var("UTILIFY_DEV_APPEARANCE").as_deref() {
        Ok("light") => mode = ThemeMode::Light,
        Ok("dark") => mode = ThemeMode::Dark,
        _ => {}
    }
    crate::ui::color::set_light(mode == ThemeMode::Light);
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None::<&mut Window>, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_os() {
        assert_eq!(mode_for(WindowAppearance::VibrantDark), ThemeMode::Dark);
        assert_eq!(mode_for(WindowAppearance::Light), ThemeMode::Light);
    }
}
