use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::Span;
use crate::protect::{Ignore, RangeSet};
use crate::regexes::CHECKLIST_BOX_STARTS_TEXT;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::rules::{group_len, unprotected_matches};
use crate::syntax;
use crate::text::is_ws;

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "twoSpaceLineBreak",
    "Two space linebreak",
    OptionKind::Bool(false),
)
.describe("Ignore two spaces followed by a line break (\"Two Space Rule\").")];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "trailing-spaces"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Trailing spaces"
    }

    fn description(&self) -> &'static str {
        "Removes extra spaces after every line."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        static PLAIN: LazyLock<[Regex; 1]> = LazyLock::new(|| [re(r"(?m)[ \t]+$")]);
        static TWO_SPACE: LazyLock<[Regex; 3]> = LazyLock::new(|| {
            [
                re(r"(?m)(\S)[ \t]$"),
                re(r"(?m)(\S)[ \t]{3,}$"),
                re(r"(?m)(\S)( ?\t\t? ?)$"),
            ]
        });
        static EMPTY_PLAIN: LazyLock<[Regex; 1]> = LazyLock::new(|| [re(r"(?m)^[ \t]+$")]);
        static EMPTY_TWO_SPACE: LazyLock<[Regex; 2]> =
            LazyLock::new(|| [re(r"(?m)^[ \t]$"), re(r"(?m)^[ \t]{3,}$")]);

        let two_space = options.bool("twoSpaceLineBreak");
        let expressions: &[Regex] = if two_space { &*TWO_SPACE } else { &*PLAIN };
        let empty_expressions: &[Regex] = if two_space {
            &*EMPTY_TWO_SPACE
        } else {
            &*EMPTY_PLAIN
        };
        let text = ctx.text;
        let protected = ctx.ignoring(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        let outside_lists = protected.union(&RangeSet::new(ctx.regions(Ignore::List)));
        let remove = |caps: &regex::Captures, start: usize| -> Span {
            let keep = if two_space { group_len(caps, 1) } else { 0 };
            start + keep..start + caps[0].len()
        };

        let mut deletions: Vec<Span> = Vec::new();
        for expression in expressions {
            deletions.extend(
                unprotected_matches(
                    text,
                    0,
                    expression,
                    &outside_lists,
                    |c, s| (remove(c, s), String::new()),
                    remove,
                )
                .into_iter()
                .map(|(r, _)| r),
            );
        }

        // Inside list items, start after the marker so an empty item keeps
        // the space after its marker.
        let bytes = text.as_bytes();
        for (position, empty) in syntax::list_item_texts(text, ctx.doc, true) {
            let mut start = position.start;
            if empty {
                while start < position.end && !is_ws(bytes[start]) {
                    start += 1;
                }
                if start < position.end {
                    start += 1;
                }
            } else {
                while start > 0 && is_ws(bytes[start - 1]) {
                    start -= 1;
                }
                if start == 0 || !is_ws(bytes[start - 1]) {
                    start += 1;
                }
            }
            let start = start.min(position.end);
            let start = if CHECKLIST_BOX_STARTS_TEXT.is_match(&text[start..position.end]) {
                (start + 4).min(position.end)
            } else {
                start
            };
            for expression in expressions {
                deletions.extend(
                    unprotected_matches(
                        &text[start..position.end],
                        start,
                        expression,
                        &protected,
                        |c, s| (remove(c, s), String::new()),
                        remove,
                    )
                    .into_iter()
                    .map(|(r, _)| r),
                );
            }
        }

        // Lines holding only whitespace, lists included.
        let whole = |caps: &regex::Captures, start: usize| start..start + caps[0].len();
        for expression in empty_expressions {
            deletions.extend(
                unprotected_matches(
                    text,
                    0,
                    expression,
                    &protected,
                    |c, s| (whole(c, s), String::new()),
                    whole,
                )
                .into_iter()
                .map(|(r, _)| r),
            );
        }

        // The passes can select the same whitespace; merge their deletions.
        let merged = RangeSet::new(deletions);
        merged
            .ranges()
            .iter()
            .map(|r| TextEdit::delete(r.clone()))
            .collect()
    }
}
