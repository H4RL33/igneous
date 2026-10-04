use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::regexes::ALL_HEADERS;
use crate::rule::{Category, LintCtx, Options};
use crate::rules::{group_len, unprotected_matches};
use crate::text::to_edits;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "headings-start-line"
    }

    fn category(&self) -> Category {
        Category::Heading
    }

    fn name(&self) -> &'static str {
        "Headings start line"
    }

    fn description(&self) -> &'static str {
        "Headings that do not start a line will have their preceding whitespace removed to make sure they get recognized as headers."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let protected = ctx.ignoring(&[Ignore::Code, Ignore::Math, Ignore::Yaml]);
        to_edits(unprotected_matches(
            ctx.text,
            0,
            &ALL_HEADERS,
            &protected,
            |caps, start| (start..start + group_len(caps, 1), String::new()),
            // The hashes must be visible; the heading's text may be protected.
            |caps, start| {
                start..start + group_len(caps, 1) + group_len(caps, 2) + group_len(caps, 3)
            },
        ))
    }
}
