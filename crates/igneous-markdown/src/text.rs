//! Small helpers for working with lines in a note.

use crate::Span;

/// Start of the line containing `pos`.
pub fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// End of the line containing `pos`, excluding the newline.
pub fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i)
}

/// The whole lines covering `range`, excluding the final newline.
pub fn line_span(text: &str, range: &Span) -> Span {
    let end = if range.end > range.start && text.as_bytes().get(range.end - 1) == Some(&b'\n') {
        range.end - 1
    } else {
        range.end
    };
    line_start(text, range.start)..line_end(text, end.max(range.start))
}

/// `range` with trailing newlines removed.
pub fn trim_newlines(text: &str, range: &Span) -> Span {
    let mut end = range.end;
    while end > range.start && matches!(text.as_bytes()[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    range.start..end
}

/// The lines intersecting `range`, ends excluding newlines. The first line
/// starts at `range.start`; a newline at the very end of `range` doesn't start
/// another line.
pub fn lines_in(text: &str, range: &Span) -> Vec<Span> {
    let end = range.end.min(text.len());
    let mut lines = Vec::new();
    let mut pos = range.start;
    loop {
        let le = line_end(text, pos);
        lines.push(pos..le);
        if le >= end {
            break;
        }
        pos = le + 1;
        if pos >= end {
            break;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_helpers() {
        let text = "ab\ncd\n\nef";
        assert_eq!(line_start(text, 4), 3);
        assert_eq!(line_end(text, 4), 5);
        assert_eq!(line_span(text, &(4..6)), 3..5);
        assert_eq!(trim_newlines(text, &(3..7)), 3..5);
        assert_eq!(
            lines_in(text, &(0..text.len())),
            vec![0..2, 3..5, 6..6, 7..9]
        );
        assert_eq!(lines_in(text, &(3..6)), vec![3..5]);
        assert_eq!(lines_in(text, &(1..1)), vec![1..2]);
    }
}
