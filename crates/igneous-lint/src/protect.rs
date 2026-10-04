//! The parts of a note a rule must leave alone, and the "projection" some
//! rules make their decisions on.
//!
//! A projection is the note with each protected region replaced by a short
//! placeholder token, so a rule's regular expressions can't see inside code or
//! links. What a rule changes in the projection is mapped back to the note, and
//! changes that would touch a placeholder are dropped. This mirrors
//! obsidian-linter's `ProtectedRanges` and `DocumentProjection`.

use crate::Span;
use crate::text::edits_between;
use igneous_markdown::TextEdit;

/// Sorted, non-overlapping byte ranges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RangeSet {
    ranges: Vec<Span>,
}

impl RangeSet {
    /// Collapses `ranges` into the smallest set covering the same bytes.
    pub fn new(mut ranges: Vec<Span>) -> Self {
        ranges.retain(|r| r.start < r.end);
        ranges.sort_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Span> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        Self { ranges: merged }
    }

    pub fn ranges(&self) -> &[Span] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn union(&self, other: &RangeSet) -> RangeSet {
        RangeSet::new(self.ranges.iter().chain(&other.ranges).cloned().collect())
    }

    /// Whether changing `range` would change a protected region. An empty
    /// range is an insertion: inserting against the edge of a region is
    /// allowed, inserting strictly inside one isn't.
    pub fn is_protected(&self, range: &Span) -> bool {
        // The first region that ends after the change starts.
        let first = self.ranges.partition_point(|r| r.end <= range.start);
        let Some(region) = self.ranges.get(first) else {
            return false;
        };
        if range.start == range.end {
            region.start < range.start && range.start < region.end
        } else {
            region.start < range.end
        }
    }

    /// The regions intersecting `range`.
    pub fn overlapping(&self, range: &Span) -> &[Span] {
        let first = self.ranges.partition_point(|r| r.end <= range.start);
        let last = self.ranges.partition_point(|r| r.start < range.end);
        &self.ranges[first..last.max(first)]
    }
}

/// What a rule may need hidden from it, named after obsidian-linter's ignore
/// types. Declared in obsidian-linter's canonical masking order, which decides
/// whose placeholder wins when regions of two types overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ignore {
    /// `<!-- linter-disable -->` … `<!-- linter-enable -->`.
    CustomIgnore,
    /// Templater's `<% … %>`.
    Templater,
    /// The frontmatter.
    Yaml,
    Table,
    /// Fenced and indented code blocks.
    Code,
    InlineCode,
    /// `$$` math blocks.
    Math,
    InlineMath,
    Html,
    List,
    /// `![alt](src)`
    Image,
    /// `[text](target)`
    Link,
    WikiLink,
    Tag,
}

impl Ignore {
    /// The placeholder a projection shows instead of the region.
    pub fn token(self) -> &'static str {
        match self {
            Ignore::CustomIgnore => "{CUSTOM_IGNORE_PLACEHOLDER0000000000000000}",
            Ignore::Templater => "{TEMPLATER_PLACEHOLDER0000000000000000}",
            Ignore::Yaml => "---\n---",
            Ignore::Table => "{TABLE_PLACEHOLDER0000000000000000}",
            Ignore::Code => "{CODE_BLOCK_PLACEHOLDER0000000000000000}",
            Ignore::InlineCode => "{INLINE_CODE_BLOCK_PLACEHOLDER0000000000000000}",
            Ignore::Math => "{MATH_PLACEHOLDER0000000000000000}",
            Ignore::InlineMath => "{INLINE_MATH_PLACEHOLDER0000000000000000}",
            Ignore::Html => "{HTML_PLACEHOLDER0000000000000000}",
            Ignore::List => "{LIST_PLACEHOLDER0000000000000000}",
            Ignore::Image => "{IMAGE_PLACEHOLDER0000000000000000}",
            Ignore::Link => "{REGULAR_LINK_PLACEHOLDER0000000000000000}",
            Ignore::WikiLink => "{WIKI_LINK_PLACEHOLDER0000000000000000}",
            Ignore::Tag => "#tag-placeholder0000000000000000",
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    source: Span,
    projected: Span,
}

/// A note with some regions replaced by placeholder tokens.
#[derive(Debug, Clone)]
pub struct Projection<'a> {
    source: &'a str,
    pub text: String,
    tokens: Vec<Token>,
}

impl<'a> Projection<'a> {
    /// `replacements` must be sorted and non-overlapping.
    pub fn new(source: &'a str, replacements: &[(Span, &'static str)]) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut tokens = Vec::with_capacity(replacements.len());
        let mut cursor = 0;
        for (range, token) in replacements {
            debug_assert!(range.start >= cursor && range.end > range.start);
            text.push_str(&source[cursor..range.start]);
            let start = text.len();
            text.push_str(token);
            tokens.push(Token {
                source: range.clone(),
                projected: start..text.len(),
            });
            cursor = range.end;
        }
        text.push_str(&source[cursor..]);
        Self {
            source,
            text,
            tokens,
        }
    }

    pub fn source(&self) -> &'a str {
        self.source
    }

    pub fn source_to_projection(&self, offset: usize) -> Option<usize> {
        map_offset(
            offset,
            self.source.len(),
            &self.tokens,
            |t| &t.source,
            |t| &t.projected,
        )
    }

    pub fn projection_to_source(&self, offset: usize) -> Option<usize> {
        map_offset(
            offset,
            self.text.len(),
            &self.tokens,
            |t| &t.projected,
            |t| &t.source,
        )
    }

    /// Whether `offset` in the projection lies inside a token.
    pub fn is_token(&self, offset: usize) -> bool {
        let index = self.tokens.partition_point(|t| t.projected.start <= offset);
        index > 0 && offset < self.tokens[index - 1].projected.end
    }

    /// Maps a change in the projection back to the note, or `None` if it
    /// would change a token.
    pub fn edit_to_source(&self, range: &Span) -> Option<Span> {
        if range.end < range.start {
            return None;
        }
        let start = self.projection_to_source(range.start)?;
        let end = self.projection_to_source(range.end)?;
        if range.start < range.end {
            // A token ending after the change starts, and starting before it ends.
            let mut index = self
                .tokens
                .partition_point(|t| t.projected.start <= range.end);
            if index > 0 && self.tokens[index - 1].projected.start == range.end {
                index -= 1;
            }
            if index > 0 && self.tokens[index - 1].projected.end > range.start {
                return None;
            }
        }
        Some(start..end)
    }

    /// The edits to the note that make the same changes as turning the
    /// projection into `projected`, minus any that would touch a token.
    pub fn edits_to(&self, projected: &str) -> Vec<TextEdit> {
        edits_between(&self.text, projected)
            .into_iter()
            .filter_map(|edit| {
                let range = self.edit_to_source(&edit.range)?;
                Some(TextEdit::replace(range, edit.insert))
            })
            .collect()
    }
}

fn map_offset(
    offset: usize,
    length: usize,
    tokens: &[Token],
    from: impl Fn(&Token) -> &Span,
    to: impl Fn(&Token) -> &Span,
) -> Option<usize> {
    if offset > length {
        return None;
    }
    let index = tokens.partition_point(|t| from(t).start <= offset);
    if index == 0 {
        return Some(offset);
    }
    let token = &tokens[index - 1];
    if offset == from(token).start {
        return Some(to(token).start);
    }
    if offset < from(token).end {
        return None;
    }
    Some(to(token).end + offset - from(token).end)
}

/// Builds a projection from regions of several types. Where regions overlap,
/// the enclosing one's token is kept, and among equal ranges the type that
/// comes first in [`Ignore`]'s order wins.
pub fn project<'a>(source: &'a str, regions: &[(Ignore, Vec<Span>)]) -> Projection<'a> {
    let mut candidates: Vec<(Span, Ignore)> = regions
        .iter()
        .flat_map(|(kind, ranges)| ranges.iter().map(move |r| (r.clone(), *kind)))
        .filter(|(r, _)| r.start < r.end)
        .collect();
    candidates.sort_by(|a, b| {
        a.0.start
            .cmp(&b.0.start)
            .then(b.0.end.cmp(&a.0.end))
            .then(a.1.cmp(&b.1))
    });
    let mut merged: Vec<(Span, &'static str)> = Vec::new();
    for (range, kind) in candidates {
        match merged.last_mut() {
            Some((previous, _)) if range.start < previous.end => {
                previous.end = previous.end.max(range.end)
            }
            _ => merged.push((range, kind.token())),
        }
    }
    Projection::new(source, &merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_protect() {
        let set = RangeSet::new(vec![5..10, 0..2, 8..12, 12..14, 20..20]);
        assert_eq!(set.ranges(), &[0..2, 5..14]);
        assert!(set.is_protected(&(1..1)));
        assert!(!set.is_protected(&(2..2)));
        assert!(!set.is_protected(&(5..5)));
        assert!(!set.is_protected(&(2..5)));
        assert!(set.is_protected(&(3..6)));
        assert!(!set.is_protected(&(14..20)));
        assert_eq!(set.overlapping(&(1..6)), &[0..2, 5..14]);
        assert_eq!(set.overlapping(&(2..5)), &[] as &[Span]);
    }

    #[test]
    fn projections_map_both_ways() {
        let source = "a `code` b";
        let p = project(
            source,
            &[(Ignore::InlineCode, std::iter::once(2..8).collect())],
        );
        assert_eq!(p.text, format!("a {} b", Ignore::InlineCode.token()));
        let token_end = 2 + Ignore::InlineCode.token().len();
        assert_eq!(p.projection_to_source(2), Some(2));
        assert_eq!(p.projection_to_source(3), None);
        assert_eq!(p.projection_to_source(token_end), Some(8));
        assert_eq!(p.source_to_projection(9), Some(token_end + 1));
        assert!(p.is_token(3));
        assert!(!p.is_token(token_end));
        assert_eq!(p.edit_to_source(&(0..1)), Some(0..1));
        assert_eq!(p.edit_to_source(&(1..3)), None);
        assert_eq!(p.edit_to_source(&(2..2)), Some(2..2));
        assert_eq!(p.edit_to_source(&(token_end..token_end + 1)), Some(8..9));
    }
}
