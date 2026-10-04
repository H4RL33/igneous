//! Turning themes into GtkSourceView style schemes.
//!
//! Each theme variant is written as a scheme file under
//! `$XDG_CACHE_HOME/igneous/style-schemes` and loaded from there. The files
//! are a cache: they're cleared the first time a scheme is needed.

use std::path::PathBuf;
use std::sync::Once;

use gtk::glib;

use crate::theme::{Color, Theme, chrome_scheme_xml, scheme_xml};

fn schemes_dir() -> PathBuf {
    glib::user_cache_dir().join("igneous").join("style-schemes")
}

/// The desktop's accent colour, for themes whose roles use `accent`.
pub fn system_accent() -> Color {
    let rgba = adw::StyleManager::default().accent_color_rgba();
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::rgb(
        channel(rgba.red()),
        channel(rgba.green()),
        channel(rgba.blue()),
    )
}

/// The style scheme for `theme` on a light or dark desktop. Returns `None`
/// (after logging why) if the scheme can't be written or loaded.
pub fn style_scheme(theme: &Theme, dark: bool) -> Option<sourceview::StyleScheme> {
    load(theme, scheme_xml(theme, dark, system_accent()))
}

/// The scheme Live Preview uses: the editor's colours without syntax styles.
pub fn live_style_scheme(theme: &Theme, dark: bool) -> Option<sourceview::StyleScheme> {
    load(theme, chrome_scheme_xml(theme, dark, system_accent()))
}

fn load(theme: &Theme, (id, xml): (String, String)) -> Option<sourceview::StyleScheme> {
    crate::init();
    static PREPARE: Once = Once::new();
    let dir = schemes_dir();
    PREPARE.call_once(|| {
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%e, "can't create the style scheme cache");
        }
        sourceview::StyleSchemeManager::default().append_search_path(&dir.to_string_lossy());
    });

    let manager = sourceview::StyleSchemeManager::default();
    if let Some(scheme) = manager.scheme(&id) {
        return Some(scheme);
    }
    if let Err(e) = std::fs::write(dir.join(format!("{id}.xml")), xml) {
        tracing::warn!(%e, theme = %theme.id, "can't write the style scheme");
        return None;
    }
    manager.force_rescan();
    let scheme = manager.scheme(&id);
    if scheme.is_none() {
        tracing::warn!(theme = %theme.id, "GtkSourceView didn't accept the generated scheme");
    }
    scheme
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtin;

    #[gtk::test]
    fn every_built_in_theme_loads_in_gtksourceview() {
        for theme in builtin() {
            for dark in [false, true] {
                let scheme = style_scheme(&theme, dark)
                    .unwrap_or_else(|| panic!("{} (dark: {dark}) didn't load", theme.id));
                assert!(scheme.id().starts_with(&format!("igneous-{}-", theme.id)));
                assert_eq!(scheme.name().as_str(), theme.display_name(dark));
                let text = scheme.style("text").expect("text style");
                assert!(text.background().is_some() || text.foreground().is_some());
            }
        }
    }
}
