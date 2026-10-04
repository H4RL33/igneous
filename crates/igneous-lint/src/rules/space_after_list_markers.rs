use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, Options};
use crate::rules::{group_len, unprotected_matches};
use crate::text::to_edits;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "space-after-list-markers"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Space after list markers"
    }

    fn description(&self) -> &'static str {
        "There should be a single space after list markers and checkboxes."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        static MARKER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(\s*\d+\.|\s*[-+*])[^\S\r\n]+").unwrap());
        static CHECKBOX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(\s*[-+*]\s+\[[ xX]\])[^\S\r\n]+").unwrap());
        let protected = ctx.ignoring(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        let edit = |caps: &regex::Captures, start: usize| {
            (
                start + group_len(caps, 1)..start + caps[0].len(),
                " ".to_owned(),
            )
        };
        // Placeholders can't stand in for a marker or a checkbox.
        let guard = |caps: &regex::Captures, start: usize| start..start + caps[0].len();
        let mut replacements = unprotected_matches(ctx.text, 0, &MARKER, &protected, edit, guard);
        replacements.extend(unprotected_matches(
            ctx.text, 0, &CHECKBOX, &protected, edit, guard,
        ));
        to_edits(crate::text::non_overlapping(replacements))
    }
}
