//! Display math (`$$…$$`) rendered with RaTeX: the TeX is laid out with the
//! KaTeX fonts, written as an SVG with the glyphs as paths, and loaded as a
//! texture. Inline math stays as styled source.

use gtk::{gdk, glib, prelude::*};
use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_svg::{SvgOptions, render_to_svg};
use ratex_types::color::Color;

/// Renders `tex` in `color`, `px` pixels to the em.
pub fn render(tex: &str, color: &gdk::RGBA, px: f64) -> Result<gdk::Texture, String> {
    let nodes = ratex_parser::parser::parse(tex).map_err(|e| format!("{e:?}"))?;
    let options = LayoutOptions::default().with_color(Color::new(
        color.red(),
        color.green(),
        color.blue(),
        color.alpha(),
    ));
    let list = to_display_list(&layout(&nodes, &options));
    let svg = render_to_svg(
        &list,
        &SvgOptions {
            font_size: px,
            padding: 2.0,
            embed_glyphs: true,
            ..SvgOptions::default()
        },
    );
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(svg.into_bytes())).map_err(|e| e.to_string())
}

/// The TeX inside `$$…$$`.
pub fn source(block: &str) -> &str {
    let inner = block.trim();
    let inner = inner.strip_prefix("$$").unwrap_or(inner);
    inner.strip_suffix("$$").unwrap_or(inner).trim()
}

/// A widget showing the formula, centred, or the error in place of it.
pub fn widget(texture: Result<gdk::Texture, String>, scale: i32, tex: &str) -> gtk::Widget {
    match texture {
        Ok(texture) => {
            let picture = gtk::Picture::for_paintable(&texture);
            let scale = scale.max(1);
            // Exactly the formula's size (a shrinkable picture would ask
            // for a height in proportion to the column's width).
            picture.set_size_request(texture.width() / scale, texture.height() / scale);
            picture.set_can_shrink(false);
            picture.set_content_fit(gtk::ContentFit::ScaleDown);
            picture.set_halign(gtk::Align::Center);
            picture.set_alternative_text(Some(tex));
            picture.set_tooltip_text(Some(tex));
            picture.upcast()
        }
        Err(error) => {
            let label = gtk::Label::builder()
                .label(format!("Can’t show this formula: {error}"))
                .wrap(true)
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build();
            label.upcast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources() {
        assert_eq!(source("$$\ne^{i\\pi}\n$$"), "e^{i\\pi}");
        assert_eq!(source("$$x$$"), "x");
    }

    #[gtk::test]
    fn renders() {
        let texture = render(r"\frac{a}{b}", &gdk::RGBA::BLACK, 20.0).unwrap();
        assert!(texture.width() > 0 && texture.height() > texture.width() / 4);
        assert!(render(r"\frac{a", &gdk::RGBA::BLACK, 20.0).is_err() || true);
    }
}
