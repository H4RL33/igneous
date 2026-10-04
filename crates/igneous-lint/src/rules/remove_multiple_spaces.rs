use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::Span;
use crate::protect::{Ignore, RangeSet};
use crate::regexes::CHECKLIST_BOX_STARTS_TEXT;
use crate::rule::{Category, LintCtx, Options};
use crate::syntax;
use crate::text::{is_ws, to_edits};

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "remove-multiple-spaces"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Remove multiple spaces"
    }

    fn description(&self) -> &'static str {
        "Removes two or more consecutive spaces. Ignores spaces at the beginning and ending of the line."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let text = ctx.text;
        let protected = ctx.ignoring(&[
            Ignore::Code,
            Ignore::InlineCode,
            Ignore::Math,
            Ignore::InlineMath,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
            Ignore::Table,
            Ignore::Image,
        ]);
        // Lists are handled item by item below, so the spaces after their
        // markers are kept.
        let outside_lists = protected.union(&RangeSet::new(ctx.regions(Ignore::List)));
        let mut replacements = Vec::new();
        runs(text, 0, &outside_lists, true, &mut replacements);
        for (position, _) in syntax::list_item_texts(text, ctx.doc, false) {
            let bytes = text.as_bytes();
            let mut start = position.start;
            while start > 0 && is_ws(bytes[start - 1]) {
                start -= 1;
            }
            if start == 0 || !is_ws(bytes[start - 1]) {
                start += 1;
            }
            let start = start.min(position.end);
            if CHECKLIST_BOX_STARTS_TEXT.is_match(&text[start..position.end]) {
                let skip = (start + 4).min(position.end);
                runs(
                    &text[skip..position.end],
                    skip,
                    &protected,
                    false,
                    &mut replacements,
                );
            } else {
                runs(
                    &text[start..position.end],
                    start,
                    &protected,
                    false,
                    &mut replacements,
                );
            }
        }
        replacements.sort_by_key(|(r, _)| r.start);
        replacements.dedup_by(|a, b| a.0.start < b.0.end);
        to_edits(replacements)
    }
}

/// Runs of two or more spaces between non-space characters, as edits to one
/// space. Unlike obsidian-linter's expression, the character after a run can
/// also start the next one, so a single pass catches every run.
fn runs(
    haystack: &str,
    offset: usize,
    protected: &RangeSet,
    skip_quote_markers: bool,
    out: &mut Vec<(Span, String)>,
) {
    static RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\s]( ){2,}").unwrap());
    let bytes = haystack.as_bytes();
    let mut pos = 0;
    while let Some(m) = RUN.find_at(haystack, pos) {
        let first_len = haystack[m.start()..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        let spaces = m.start() + first_len..m.end();
        let followed = haystack[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace());
        let quote_marker = skip_quote_markers
            && bytes[m.start()] == b'>'
            && (m.start() == 0 || bytes[m.start() - 1] == b'\n');
        if !followed || quote_marker {
            pos = spaces.start;
            continue;
        }
        let range = offset + spaces.start..offset + spaces.end;
        if !protected.is_protected(&range) {
            out.push((range, " ".to_owned()));
        }
        pos = m.end();
    }
}
