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
        "remove-consecutive-list-markers"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Remove consecutive list markers"
    }

    fn description(&self) -> &'static str {
        "Removes consecutive list markers. Useful when copy-pasting list items."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        static DOUBLE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^([ |\t]*)- - (\p{L})").unwrap());
        let protected = ctx.ignoring(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        to_edits(unprotected_matches(
            ctx.text,
            0,
            &DOUBLE,
            &protected,
            |caps, start| {
                let at = start + group_len(caps, 1) + 2;
                (at..at + 2, String::new())
            },
            |caps, start| start..start + caps.get(0).unwrap().len(),
        ))
    }
}
