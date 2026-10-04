//! Buffers for notes.

use sourceview::prelude::*;

pub const LANGUAGE_ID: &str = "igneous-markdown";

/// A buffer with Markdown highlighting. Its colours come from whichever
/// editor theme its owner applies (see [`crate::style_scheme`]).
pub fn new_buffer() -> sourceview::Buffer {
    crate::init();
    let buffer = sourceview::Buffer::new(None);
    buffer.set_language(
        sourceview::LanguageManager::default()
            .language(LANGUAGE_ID)
            .as_ref(),
    );
    buffer.set_highlight_matching_brackets(false);
    buffer
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
