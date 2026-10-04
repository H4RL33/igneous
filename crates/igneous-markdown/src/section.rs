//! The part of a note a link's subpath points at: a heading's section
//! (`#Heading#Subheading`) or a block (`#^block-id`).

use crate::link::Subpath;
use crate::parse::{Document, NodeKind};
use crate::{Span, text};

/// The range `subpath` refers to in `text`, or `None` if it doesn't exist.
///
/// - A heading path picks the last heading, which must come after the
///   earlier ones, compared loosely (case and surrounding spaces). Its
///   section runs to the next heading of the same or a higher level.
/// - A block ID picks the block (paragraph, list item, quote…) that ends
///   with `^id`, without the ID itself.
pub fn subpath_range(doc: &Document, text: &str, subpath: &Subpath) -> Option<Span> {
    match subpath {
        Subpath::Heading(path) => {
            let mut from = 0;
            let mut found = None;
            for wanted in path {
                let wanted = loose(wanted);
                let (i, heading) = doc
                    .headings
                    .iter()
                    .enumerate()
                    .find(|(_, h)| h.range.start >= from && loose(&h.text) == wanted)?;
                from = heading.range.end;
                found = Some(i);
            }
            let i = found?;
            let heading = &doc.headings[i];
            let end = doc.headings[i + 1..]
                .iter()
                .find(|h| h.level <= heading.level)
                .map_or(text.len(), |h| h.range.start);
            Some(heading.range.start..end)
        }
        Subpath::Block(id) => {
            let block = doc
                .block_ids
                .iter()
                .find(|b| b.id.eq_ignore_ascii_case(id))?;
            // The innermost block-level node holding the ID.
            let node = doc
                .nodes
                .iter()
                .filter(|n| {
                    matches!(
                        n.kind,
                        NodeKind::Paragraph
                            | NodeKind::ListItem { .. }
                            | NodeKind::Quote
                            | NodeKind::Callout(_)
                            | NodeKind::Table
                            | NodeKind::CodeBlock { .. }
                            | NodeKind::Heading { .. }
                    ) && n.range.start <= block.range.start
                        && block.range.end <= n.range.end
                })
                .max_by_key(|n| n.depth);
            let start = match node {
                Some(node) => node.range.start,
                None => text::line_start(text, block.range.start),
            };
            let end = text[..block.range.start].trim_end().len();
            Some(start..end.max(start))
        }
    }
}

fn loose(s: &str) -> String {
    s.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const NOTE: &str = "# Top\nintro\n## Plans\nplan text\n### Detail\ndeep\n## Other\nmore ^abc\n\n- item\n- second ^xyz\n";

    fn section(subpath: Subpath) -> Option<&'static str> {
        let doc = parse(NOTE);
        subpath_range(&doc, NOTE, &subpath).map(|r| &NOTE[r])
    }

    #[test]
    fn heading_sections() {
        assert_eq!(
            section(Subpath::Heading(vec!["plans".into()])),
            Some("## Plans\nplan text\n### Detail\ndeep\n")
        );
        assert_eq!(
            section(Subpath::Heading(vec!["Plans".into(), "Detail".into()])),
            Some("### Detail\ndeep\n")
        );
        assert_eq!(section(Subpath::Heading(vec!["Missing".into()])), None);
        // A later heading can't come before an earlier one in the path.
        assert_eq!(
            section(Subpath::Heading(vec!["Other".into(), "Detail".into()])),
            None
        );
    }

    #[test]
    fn blocks() {
        assert_eq!(section(Subpath::Block("abc".into())), Some("more"));
        assert_eq!(section(Subpath::Block("xyz".into())), Some("- second"));
        assert_eq!(section(Subpath::Block("nope".into())), None);
    }
}
