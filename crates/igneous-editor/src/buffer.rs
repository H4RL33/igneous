//! Buffers for notes.

use adw::prelude::*;
use sourceview::prelude::*;

pub const LANGUAGE_ID: &str = "igneous-markdown";

/// A buffer with Markdown highlighting whose colours follow the system's
/// light or dark style.
pub fn new_buffer() -> sourceview::Buffer {
    crate::init();
    let buffer = sourceview::Buffer::new(None);
    buffer.set_language(
        sourceview::LanguageManager::default()
            .language(LANGUAGE_ID)
            .as_ref(),
    );
    buffer.set_highlight_matching_brackets(false);

    let style = adw::StyleManager::default();
    apply_scheme(&buffer, style.is_dark());
    let weak = buffer.downgrade();
    let handler = style.connect_dark_notify(move |style| {
        if let Some(buffer) = weak.upgrade() {
            apply_scheme(&buffer, style.is_dark());
        }
    });
    // Disconnect when the buffer goes away.
    let style_weak = style.downgrade();
    let handler = std::cell::Cell::new(Some(handler));
    buffer.add_weak_ref_notify_local(move || {
        if let (Some(style), Some(handler)) = (style_weak.upgrade(), handler.take()) {
            style.disconnect(handler);
        }
    });
    buffer
}

fn apply_scheme(buffer: &sourceview::Buffer, dark: bool) {
    let id = if dark { "Adwaita-dark" } else { "Adwaita" };
    buffer.set_style_scheme(
        sourceview::StyleSchemeManager::default()
            .scheme(id)
            .as_ref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk::test]
    fn highlights_obsidian_syntax() {
        let buffer = new_buffer();
        assert_eq!(
            buffer.language().map(|l| l.id().to_string()).as_deref(),
            Some(LANGUAGE_ID)
        );
        let text = "---\ntags: [a]\n---\nSee [[Note]] and #tag and ==hi== and %%secret%%\n\n---\n\nnot frontmatter\n";
        buffer.set_text(text);
        buffer.ensure_highlight(&buffer.start_iter(), &buffer.end_iter());
        let iter = |needle: &str| buffer.iter_at_offset(text.find(needle).unwrap() as i32 + 1);
        // Style tags are anonymous; count them against plain text.
        let styled = |needle: &str| iter(needle).tags().len() > iter("See").tags().len();
        let class = |needle: &str, class: &str| buffer.iter_has_context_class(&iter(needle), class);
        for needle in ["tags", "[[Note", "#tag", "==hi", "%%secret"] {
            assert!(styled(needle), "{needle} is not highlighted");
        }
        assert!(class("%%secret", "comment"));
        assert!(class("[[Note", "no-spell-check"));
        assert!(class("tags", "no-spell-check"));
        // A later `---` pair is a rule, not frontmatter.
        assert!(!styled("not frontmatter"));
        assert!(!class("not frontmatter", "no-spell-check"));
    }
}
