//! Where Markdown elements are, in the shape obsidian-linter's rules expect.
//!
//! obsidian-linter asks an mdast tree for element positions. Igneous parses
//! with pulldown-cmark (through `igneous-markdown`), whose block ranges also
//! take in the newline after a block; these helpers trim them to mdast's
//! extent. Positions come back sorted by start, last first, as
//! obsidian-linter's `getPositions` returns them: rules that rewrite text in a
//! loop rely on working from the end.

use igneous_markdown::{Document, LinkKind, NodeKind};

use crate::Span;
use crate::regexes;
use crate::text::is_ws;

/// The element types rules ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Code,
    InlineCode,
    Math,
    InlineMath,
    Html,
    List,
    ListItem,
    Blockquote,
    HorizontalRule,
    Emphasis,
    Strong,
    Image,
    Link,
}

/// Positions of `element`, last first.
pub fn positions(text: &str, doc: &Document, element: Element) -> Vec<Span> {
    if matches!(element, Element::Math | Element::InlineMath) {
        return math_positions(text, doc, element);
    }
    let mut out: Vec<Span> = doc
        .nodes
        .iter()
        .filter_map(|node| {
            let range = node.range.clone();
            let keep = match (&node.kind, element) {
                (NodeKind::CodeBlock { .. }, Element::Code) => Some(trim_block(text, range)),
                (NodeKind::InlineCode, Element::InlineCode) => Some(range),
                (NodeKind::HtmlBlock, Element::Html) => Some(trim_block(text, range)),
                (NodeKind::InlineHtml, Element::Html) => Some(range),
                (NodeKind::List { .. }, Element::List) => {
                    Some(skip_indent(text, trim_block(text, range)))
                }
                (NodeKind::ListItem { .. }, Element::ListItem) => {
                    Some(skip_indent(text, trim_block(text, range)))
                }
                (NodeKind::Quote | NodeKind::Callout(_), Element::Blockquote) => {
                    Some(skip_indent(text, trim_block(text, range)))
                }
                (NodeKind::Rule, Element::HorizontalRule) => Some(trim_block(text, range)),
                (NodeKind::Emphasis, Element::Emphasis) => Some(range),
                (NodeKind::Strong, Element::Strong) => Some(range),
                (NodeKind::Embed(i), Element::Image) => {
                    (doc.links[*i].kind == LinkKind::Markdown).then_some(range)
                }
                (NodeKind::Link(i), Element::Link) => (doc.links[*i].kind == LinkKind::Markdown
                    && regexes::GENERIC_LINK.is_match(&text[range.clone()]))
                .then_some(range),
                _ => None,
            };
            keep.filter(|r| r.start < r.end)
        })
        .collect();
    // Stable, so a parent stays ahead of a child starting at the same place.
    out.sort_by_key(|r| std::cmp::Reverse(r.start));
    out
}

/// A range starting after any spaces or tabs at its start.
fn skip_indent(text: &str, range: Span) -> Span {
    let skipped = text[range.clone()]
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    (range.start + skipped).min(range.end)..range.end
}

/// Math, found the way obsidian-linter's parser (micromark) finds it: a block
/// is a `$$` fence opening a line (with no `$` after it) through a closing
/// fence; inline math is a run of `$` through the next run of the same length.
/// pulldown-cmark is stricter about inline math, so both are combined.
fn math_positions(text: &str, doc: &Document, element: Element) -> Vec<Span> {
    let code: Vec<Span> = doc
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::CodeBlock { .. } | NodeKind::InlineCode))
        .map(|n| n.range.clone())
        .collect();
    let in_code = |pos: usize| code.iter().any(|c| c.start <= pos && pos < c.end);
    let blocks = math_blocks(text, &in_code);
    let mut out = match element {
        Element::Math => blocks,
        _ => {
            let in_block = |pos: usize| blocks.iter().any(|b| b.start <= pos && pos < b.end);
            let mut inline = inline_math(text, &|pos| in_code(pos) || in_block(pos));
            for node in &doc.nodes {
                if let NodeKind::Math { .. } = node.kind {
                    let r = node.range.clone();
                    let covered = blocks
                        .iter()
                        .chain(inline.iter())
                        .any(|b| b.start < r.end && r.start < b.end);
                    if !covered {
                        inline.push(r);
                    }
                }
            }
            inline
        }
    };
    out.sort_by_key(|r| std::cmp::Reverse(r.start));
    out
}

fn math_blocks(text: &str, in_code: &dyn Fn(usize) -> bool) -> Vec<Span> {
    let mut lines = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        lines.push(start..start + line.len());
        start += line.len() + 1;
    }
    let prefix = |line: &Span| {
        text[line.clone()]
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t' | b'>'))
            .count()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        let open = line.start + prefix(line);
        let rest = &text[open..line.end];
        let n = rest.bytes().take_while(|b| *b == b'$').count();
        if n < 2 || rest[n..].contains('$') || in_code(open) {
            i += 1;
            continue;
        }
        let mut end = text.trim_end().len().max(open + n);
        let mut j = i + 1;
        while j < lines.len() {
            let candidate = &lines[j];
            let at = candidate.start + prefix(candidate);
            let body = &text[at..candidate.end];
            let m = body.bytes().take_while(|b| *b == b'$').count();
            if m >= n && body[m..].trim().is_empty() {
                end = at + m;
                break;
            }
            j += 1;
        }
        out.push(open..end);
        i = j + 1;
    }
    out
}

fn inline_math(text: &str, excluded: &dyn Fn(usize) -> bool) -> Vec<Span> {
    let bytes = text.as_bytes();
    let escaped = |pos: usize| {
        bytes[..pos]
            .iter()
            .rev()
            .take_while(|b| **b == b'\\')
            .count()
            % 2
            == 1
    };
    let run_at = |pos: usize| bytes[pos..].iter().take_while(|b| **b == b'$').count();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = text[pos..].find('$') {
        let open = pos + i;
        let n = run_at(open);
        if escaped(open) || excluded(open) {
            pos = open + n;
            continue;
        }
        // The next run of exactly `n`, within the paragraph.
        let mut search = open + n;
        let mut close = None;
        while let Some(j) = text[search..].find('$') {
            let at = search + j;
            if text[open + n..at].contains("\n\n") || excluded(at) {
                break;
            }
            let m = run_at(at);
            if m == n && !escaped(at) {
                close = Some(at + m);
                break;
            }
            search = at + m;
        }
        match close {
            Some(end) if end > open + 2 * n => {
                out.push(open..end);
                pos = end;
            }
            _ => pos = open + n,
        }
    }
    out
}

/// A block's range without the line ending pulldown-cmark includes.
fn trim_block(text: &str, range: Span) -> Span {
    let bytes = text.as_bytes();
    let mut end = range.end.min(text.len());
    while end > range.start && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    range.start..end
}

/// The text of each list item (mdast's paragraphs directly inside list
/// items), last first. With `include_empty`, items with no content are
/// included too, as their whole range, marked `true`.
pub fn list_item_texts(text: &str, doc: &Document, include_empty: bool) -> Vec<(Span, bool)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (index, item) in doc.nodes.iter().enumerate() {
        if !matches!(item.kind, NodeKind::ListItem { .. }) {
            continue;
        }
        let range = trim_block(text, item.range.clone());
        let children: Vec<&igneous_markdown::Node> = doc.nodes[index + 1..]
            .iter()
            .take_while(|n| n.range.start < item.range.end)
            .filter(|n| n.depth == item.depth + 1)
            .collect();
        let paragraphs: Vec<Span> = children
            .iter()
            .filter(|n| n.kind == NodeKind::Paragraph)
            .map(|n| trim_trailing_ws(text, n.range.clone()))
            .collect();
        if !paragraphs.is_empty() {
            out.extend(paragraphs.into_iter().map(|p| (p, false)));
            continue;
        }
        // A tight item: its text runs from after the marker to the first
        // block inside it.
        let mut start = range.start;
        while start < range.end && !is_ws(bytes[start]) {
            start += 1;
        }
        while start < range.end && matches!(bytes[start], b' ' | b'\t') {
            start += 1;
        }
        let first_block = children.iter().find(|n| is_block(&n.kind));
        let end = first_block.map_or(range.end, |n| n.range.start);
        let content = trim_trailing_ws(text, start..end.max(start));
        if !content.is_empty() {
            out.push((content, false));
        } else if first_block.is_none() && include_empty {
            out.push((range, true));
        }
    }
    out.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    out
}

fn is_block(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::List { .. }
            | NodeKind::CodeBlock { .. }
            | NodeKind::Quote
            | NodeKind::Callout(_)
            | NodeKind::Heading { .. }
            | NodeKind::Table
            | NodeKind::HtmlBlock
            | NodeKind::Rule
            | NodeKind::Paragraph
    )
}

fn line_start_of(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// `range` up to the end of its last line with content, keeping the spaces
/// at the end of that line (mdast's paragraphs keep them).
fn trim_trailing_ws(text: &str, range: Span) -> Span {
    let bytes = text.as_bytes();
    let mut end = range.end.min(text.len());
    while end > range.start && is_ws(bytes[end - 1]) {
        end -= 1;
    }
    if end > range.start {
        while end < text.len() && matches!(bytes[end], b' ' | b'\t') {
            end += 1;
        }
    }
    range.start..end
}

/// `#tags`, without the whitespace in front of them.
pub fn tags(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(m) = regexes::TAG_WITH_LEADING_WHITESPACE.captures_at(text, pos) {
        let whole = m.get(0).unwrap();
        let tag = m.get(2).unwrap();
        pos = whole.end().max(pos + 1);
        while !text.is_char_boundary(pos) && pos < text.len() {
            pos += 1;
        }
        // JavaScript's lookahead: the tag needs a character that isn't a digit.
        if tag.as_str()[1..].bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        out.push(tag.range());
        if pos >= text.len() {
            break;
        }
    }
    out
}

/// obsidian-linter's frontmatter: `---` on the first line through the next
/// `---` line. Returns the whole block (without the newline after it) and its
/// inside, which includes the newline before the closing fence.
pub fn yaml(text: &str) -> Option<(Span, Span)> {
    if !text.starts_with("---\n") {
        return None;
    }
    let after = |p: usize| p == text.len() || text.as_bytes()[p] == b'\n';
    if text[4..].starts_with("---") {
        return after(7).then_some((0..7, 4..4));
    }
    let mut search = 4;
    while let Some(i) = text[search..].find("\n---") {
        let p = search + i;
        if after(p + 4) {
            return Some((0..p + 4, 4..p + 1));
        }
        search = p + 1;
    }
    None
}

/// `<!-- linter-disable -->` regions, through the next `linter-enable` (or the
/// end of the text).
pub fn custom_ignore(text: &str) -> Vec<Span> {
    let starts: Vec<usize> = regexes::CUSTOM_IGNORE_START
        .find_iter(text)
        .map(|m| m.start())
        .collect();
    if starts.is_empty() {
        return Vec::new();
    }
    let ends: Vec<usize> = regexes::CUSTOM_IGNORE_END
        .find_iter(text)
        .map(|m| m.end())
        .collect();
    let end_starts: Vec<usize> = regexes::CUSTOM_IGNORE_END
        .find_iter(text)
        .map(|m| m.start())
        .collect();
    let mut out = Vec::new();
    let mut next_end = 0;
    for start in starts {
        while next_end < end_starts.len() && end_starts[next_end] <= start {
            next_end += 1;
        }
        let end = ends
            .get(next_end)
            .copied()
            .unwrap_or(text.len().saturating_sub(1));
        out.push(start..end.max(start));
    }
    out
}

/// GitHub tables, found the way obsidian-linter finds them: from separator
/// rows, without parsing. obsidian-linter's `getAllTablesInText`, last first.
pub fn tables(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for m in regexes::TABLE_SEPARATOR.find_iter(text) {
        let line_start = line_start_of(text, m.start());
        if line_start == 0 {
            continue;
        }
        let previous_start = line_start_of(text, line_start - 1);
        let separator = m.as_str();
        let row = &text[line_start..m.end()];
        if invalid_separator_row(row, separator) {
            continue;
        }
        let mut start = previous_start;
        let mut first_line = text[previous_start..line_start - 1].to_owned();
        if !separator.contains('|') && !first_line.contains('|') {
            continue;
        }
        if let Some(pipe) = regexes::TABLE_STARTING_PIPE.find(&first_line) {
            start += pipe.len() - 1;
            first_line.replace_range(pipe.range(), "");
        }
        let mut delimiter_line = regexes::TABLE_STARTING_PIPE
            .replace(separator, "")
            .into_owned();
        if first_line.ends_with('|') {
            first_line.pop();
        }
        if delimiter_line.ends_with('|') {
            delimiter_line.pop();
        }
        if count_delimiters(&first_line) != count_delimiters(&delimiter_line) {
            continue;
        }
        if previous_start != 0 {
            let two_before = line_start_of(text, previous_start - 1);
            let line = &text[two_before..previous_start - 1];
            if line.starts_with('|') || line.ends_with('|') {
                continue;
            }
        }
        let mut end = m.end();
        if end + 1 >= text.len() {
            out.push(start..text.len());
            continue;
        }
        for line in text[end + 1..].split('\n') {
            if !regexes::TABLE_ROW.is_match(line) {
                break;
            }
            end += line.len() + 1;
        }
        out.push(start..end.min(text.len()));
    }
    out.reverse();
    out
}

fn invalid_separator_row(row: &str, separator: &str) -> bool {
    if row.trim().is_empty() || separator.contains("||") {
        return true;
    }
    let rest = row.replacen(separator, "", 1);
    rest.chars().any(|c| !c.is_whitespace() && c != '>')
}

fn count_delimiters(line: &str) -> usize {
    let mut escaped = false;
    let mut run = 0;
    let mut count = 0;
    for c in line.chars() {
        if c == '\\' {
            run += 1;
            escaped = run % 2 == 1;
        } else {
            run = 0;
            if c == '|' && !escaped {
                count += 1;
            }
            escaped = false;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_like_obsidian_linter() {
        assert_eq!(yaml("---\nkey: value\n---\nbody"), Some((0..18, 4..15)));
        assert_eq!(yaml("---\n---"), Some((0..7, 4..4)));
        assert_eq!(yaml("---\n\n---\n"), Some((0..8, 4..5)));
        assert_eq!(yaml("---\nkey: value\n----\n"), None);
        assert_eq!(yaml("text"), None);
    }

    #[test]
    fn tags_need_a_non_digit() {
        let text = "#tag and #123 and x#no and #12a";
        let found: Vec<&str> = tags(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["#tag", "#12a"]);
    }

    #[test]
    fn finds_tables() {
        let text = "intro\n| a | b |\n| - | - |\n| 1 | 2 |\n\nafter";
        let found = tables(text);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].clone()], "| a | b |\n| - | - |\n| 1 | 2 |");
    }

    #[test]
    fn list_item_text() {
        let text = "- one\n- two\n  - three\n-\n";
        let doc = igneous_markdown::parse(text);
        let found: Vec<(&str, bool)> = list_item_texts(text, &doc, true)
            .into_iter()
            .map(|(r, empty)| (&text[r], empty))
            .collect();
        assert_eq!(
            found,
            [
                ("-", true),
                ("three", false),
                ("two", false),
                ("one", false)
            ]
        );
    }
}
