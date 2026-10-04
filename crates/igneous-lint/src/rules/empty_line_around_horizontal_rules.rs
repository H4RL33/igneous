use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, Options};
use crate::rules::empty_line_around_code_fences::projected_positions;
use crate::syntax::Element;
use crate::text::ensure_empty_lines_around;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "empty-line-around-horizontal-rules"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Empty line around horizontal rules"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is an empty line around horizontal rules unless they start or end a document."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[]);
        let mut text = projection.text.clone();
        for range in projected_positions(ctx, &projection, Element::HorizontalRule) {
            text = ensure_empty_lines_around(&text, range.start, range.end, false);
        }
        projection.edits_to(&text)
    }
}
