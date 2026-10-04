use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, Options};
use crate::rules::unprotected_matches;
use crate::text::to_edits;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "remove-hyphenated-line-breaks"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Remove hyphenated line breaks"
    }

    fn description(&self) -> &'static str {
        "Removes hyphenated line breaks. Useful when pasting text from textbooks."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        // JavaScript's `\b` only knows ASCII word characters.
        static HYPHEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?-u:\b)[-‐] (?-u:\b)").unwrap());
        let protected = ctx.ignoring(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        let whole = |caps: &regex::Captures, start: usize| start..start + caps[0].len();
        to_edits(unprotected_matches(
            ctx.text,
            0,
            &HYPHEN,
            &protected,
            |caps, start| (whole(caps, start), String::new()),
            whole,
        ))
    }
}
