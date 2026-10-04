//! Folding headings and callouts in Live Preview and Reading.
//!
//! A folded heading hides its section (up to the next heading of the same
//! or a higher level); a folded callout hides everything after its first
//! line. Folded text is hidden the same way as syntax, so the buffer never
//! changes. Callouts written `[!note]-` start folded, as in Obsidian. Moving
//! the cursor into folded text unfolds it.

use std::collections::BTreeSet;
use std::ops::Range;

use igneous_markdown::{Document, Fold};

/// A heading or callout that can fold, by where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foldable {
    pub start: usize,
    /// What folding hides.
    pub hidden: Range<usize>,
}

/// Everything in `doc` that can fold. Headings only fold when
/// `headings` is on.
pub fn foldables(doc: &Document, text: &str, headings: bool) -> Vec<Foldable> {
    let mut out = Vec::new();
    if headings {
        for (i, heading) in doc.headings.iter().enumerate() {
            let line_end = igneous_markdown::text::line_end(text, heading.range.start);
            let end = doc.headings[i + 1..]
                .iter()
                .find(|h| h.level <= heading.level)
                .map_or(text.len(), |h| h.range.start);
            // Keep the newline before the next heading, so it stays on its
            // own line.
            let end = trim_end_newline(text, end);
            if end > line_end {
                out.push(Foldable {
                    start: heading.range.start,
                    hidden: line_end..end,
                });
            }
        }
    }
    for callout in doc.callouts.iter().filter(|c| c.fold.is_some()) {
        let line_end = igneous_markdown::text::line_end(text, callout.range.start);
        let end = trim_end_newline(text, callout.range.end);
        if end > line_end {
            out.push(Foldable {
                start: callout.range.start,
                hidden: line_end..end,
            });
        }
    }
    out.sort_by_key(|f| f.start);
    out
}

fn trim_end_newline(text: &str, end: usize) -> usize {
    let end = end.min(text.len());
    if end > 0 && text.as_bytes()[end - 1] == b'\n' {
        end - 1
    } else {
        end
    }
}

/// Callouts written to start folded (`[!type]-`).
pub fn folded_by_default(doc: &Document) -> BTreeSet<usize> {
    doc.callouts
        .iter()
        .filter(|c| c.fold == Some(Fold::Closed))
        .map(|c| c.range.start)
        .collect()
}

/// Moves fold positions to follow an edit: `deleted` bytes at `pos`
/// replaced by `inserted`. Folds inside deleted text are dropped.
pub fn shift(folded: &mut BTreeSet<usize>, pos: usize, deleted: usize, inserted: usize) {
    *folded = folded
        .iter()
        .filter_map(|&start| {
            if start < pos {
                Some(start)
            } else if start >= pos + deleted {
                Some(start - deleted + inserted)
            } else {
                None
            }
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use igneous_markdown::parse;

    #[test]
    fn heading_sections_and_callouts() {
        let text = "# A\none\n## B\ntwo\n# C\nthree\n\n> [!note]- Closed\n> hidden\n";
        let doc = parse(text);
        let found = foldables(&doc, text, true);
        let hidden: Vec<&str> = found.iter().map(|f| &text[f.hidden.clone()]).collect();
        assert_eq!(
            hidden,
            [
                "\none\n## B\ntwo",
                "\ntwo",
                "\nthree\n\n> [!note]- Closed\n> hidden",
                "\n> hidden"
            ]
        );
        assert_eq!(folded_by_default(&doc).len(), 1);
        assert_eq!(foldables(&doc, text, false).len(), 1);
    }

    #[test]
    fn folds_follow_edits() {
        let mut folded: BTreeSet<usize> = [10, 20, 30].into();
        shift(&mut folded, 15, 10, 2);
        assert_eq!(folded, [10, 22].into());
    }
}
