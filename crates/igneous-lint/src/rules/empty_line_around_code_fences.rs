use igneous_markdown::TextEdit;

use crate::Span;
use crate::protect::Projection;
use crate::rule::{Category, LintCtx, Options};
use crate::syntax::Element;
use crate::text::ensure_empty_lines_around;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "empty-line-around-code-fences"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Empty line around code fences"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is an empty line around code fences unless they start or end a document."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[]);
        let mut text = projection.text.clone();
        for range in projected_positions(ctx, &projection, Element::Code) {
            let block = &text[range.clone()];
            if !block.starts_with("```") && !block.starts_with("~~~") {
                continue;
            }
            text = ensure_empty_lines_around(&text, range.start, range.end, false);
        }
        projection.edits_to(&text)
    }
}

/// Positions of `element` in the note, moved into the projection. Elements
/// starting inside a placeholder are skipped. Last first.
pub(crate) fn projected_positions(
    ctx: &LintCtx,
    projection: &Projection,
    element: Element,
) -> Vec<Span> {
    ctx.positions(element)
        .into_iter()
        .filter_map(|p| {
            let start = projection.source_to_projection(p.start)?;
            let end = projection.source_to_projection(p.end)?;
            (!projection.is_token(start)).then_some(start..end)
        })
        .collect()
}
