use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, Options};
use crate::syntax;
use crate::text::ensure_empty_lines_around;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "empty-line-around-tables"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Empty line around tables"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is an empty line around github flavored tables unless they start or end a document."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[
            Ignore::Yaml,
            Ignore::Code,
            Ignore::Math,
            Ignore::InlineMath,
            Ignore::WikiLink,
            Ignore::Link,
        ]);
        // Tables are found by their text alone, in the projection.
        let mut text = projection.text.clone();
        for table in syntax::tables(&projection.text) {
            text = ensure_empty_lines_around(&text, table.start, table.end, false);
        }
        projection.edits_to(&text)
    }
}
