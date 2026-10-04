use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::regexes::MULTIPLE_BLANK_LINES;
use crate::rule::{Category, LintCtx, Options, Stage};

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "consecutive-blank-lines"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Consecutive blank lines"
    }

    fn description(&self) -> &'static str {
        "There should be at most one consecutive blank line."
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        MULTIPLE_BLANK_LINES
            .find_iter(&projection.text)
            .filter_map(|m| projection.edit_to_source(&m.range()))
            .map(|range| TextEdit::replace(range, "\n\n"))
            .collect()
    }
}
