//! Text edits: the only way Igneous changes a note.

use crate::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Span,
    pub insert: String,
}

impl TextEdit {
    pub fn replace(range: Span, insert: impl Into<String>) -> Self {
        Self {
            range,
            insert: insert.into(),
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::replace(at..at, text)
    }

    pub fn delete(range: Span) -> Self {
        Self::replace(range, "")
    }
}

/// Applies non-overlapping edits, given in any order.
///
/// # Panics
/// If two edits overlap or an edit lies outside `text` or off a character
/// boundary.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.range.start, e.range.end));
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for edit in sorted {
        assert!(edit.range.start >= pos, "overlapping edits");
        out.push_str(&text[pos..edit.range.start]);
        out.push_str(&edit.insert);
        pos = edit.range.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// The edits that undo `edits`, to be applied to `apply(original, edits)`.
pub fn invert(original: &str, edits: &[TextEdit]) -> Vec<TextEdit> {
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.range.start, e.range.end));
    let mut delta: isize = 0;
    sorted
        .into_iter()
        .map(|edit| {
            let start = (edit.range.start as isize + delta) as usize;
            delta += edit.insert.len() as isize - edit.range.len() as isize;
            TextEdit::replace(
                start..start + edit.insert.len(),
                &original[edit.range.clone()],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_and_invert() {
        let text = "hello brave new world";
        let edits = [
            TextEdit::replace(16..21, "planet"),
            TextEdit::delete(5..11),
            TextEdit::insert(0, ">> "),
        ];
        let out = apply(text, &edits);
        assert_eq!(out, ">> hello new planet");
        assert_eq!(apply(&out, &invert(text, &edits)), text);
    }

    #[test]
    #[should_panic(expected = "overlapping")]
    fn rejects_overlap() {
        apply("abcdef", &[TextEdit::delete(0..3), TextEdit::delete(2..4)]);
    }
}
