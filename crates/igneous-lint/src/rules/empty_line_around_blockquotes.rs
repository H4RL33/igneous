use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, Options};
use crate::rules::empty_line_around_code_fences::projected_positions;
use crate::syntax::Element;
use crate::text::ensure_empty_lines_around;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "empty-line-around-blockquotes"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Empty line around blockquotes"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is an empty line around blockquotes unless they start or end a document. An empty line is either one less level of nesting for blockquotes or a newline character."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[]);
        let mut text = projection.text.clone();
        for range in projected_positions(ctx, &projection, Element::Blockquote) {
            // Nested blockquotes can move content, so go on to the end of the line.
            let bytes = text.as_bytes();
            let mut end = range.end;
            while end + 1 < text.len() && bytes[end] != b'\n' {
                end += 1;
            }
            text = ensure_empty_lines_around(&text, range.start, end, true);
        }
        projection.edits_to(&text)
    }
}
