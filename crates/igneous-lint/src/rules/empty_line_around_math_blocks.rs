use igneous_markdown::TextEdit;

use crate::Span;
use crate::protect::{Ignore, Projection};
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::rules::empty_line_around_code_fences::projected_positions;
use crate::syntax::Element;
use crate::text::{edits_between, ensure_empty_lines_around};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "minimumNumberOfDollarSignsToBeAMathBlock",
    "Number of dollar signs to indicate a math block",
    OptionKind::Number(2.0),
)
.shared()];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "empty-line-around-math-blocks"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Empty line around math blocks"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is an empty line around math blocks, using the number of dollar signs that indicates a math block to recognise single-line math."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let dollars = "$".repeat(dollar_count(options));
        let projection = ctx.projection(&[Ignore::Yaml, Ignore::Code]);
        let mut text = projection.text.clone();
        for range in projected_positions(ctx, &projection, Element::Math) {
            text = ensure_empty_lines_around(&text, range.start, range.end, false);
        }
        for range in inline_math_after(ctx, &projection, &text) {
            if !text[range.clone()].starts_with(&dollars) {
                continue;
            }
            text = ensure_empty_lines_around(&text, range.start, range.end, false);
        }
        projection.edits_to(&text)
    }
}

pub(crate) fn dollar_count(options: &Options) -> usize {
    let n = options.number("minimumNumberOfDollarSignsToBeAMathBlock");
    if n >= 1.0 { n as usize } else { 2 }
}

/// Inline math positions in the projection, moved past the changes already
/// made to it (rather than parsing the changed text again).
pub(crate) fn inline_math_after(
    ctx: &LintCtx,
    projection: &Projection,
    changed: &str,
) -> Vec<Span> {
    let edits = edits_between(&projection.text, changed);
    let shift = |offset: usize| -> Option<usize> {
        let mut delta: isize = 0;
        for edit in &edits {
            if edit.range.start > offset {
                break;
            }
            if edit.range.end > offset {
                return None;
            }
            delta += edit.insert.len() as isize - edit.range.len() as isize;
        }
        Some((offset as isize + delta) as usize)
    };
    projected_positions(ctx, projection, Element::InlineMath)
        .into_iter()
        .filter_map(|r| Some(shift(r.start)?..shift(r.end)?))
        .filter(|r| r.end <= changed.len())
        .collect()
}
