use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::syntax::Element;

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "style",
    "Style",
    OptionKind::Choice {
        choices: &["consistent", "asterisk", "underscore"],
        default: "consistent",
    },
)
.describe("The style used to denote emphasized content")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "emphasis-style"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Emphasis style"
    }

    fn description(&self) -> &'static str {
        "Makes sure the emphasis style is consistent."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        consistent_delimiters(ctx, options.str("style"), Element::Emphasis)
    }
}

/// Rewrites the delimiters of every emphasis (or strong) span in one style.
/// obsidian-linter's `makeEmphasisOrBoldConsistent`.
pub(crate) fn consistent_delimiters(ctx: &LintCtx, style: &str, element: Element) -> Vec<TextEdit> {
    let width = if element == Element::Strong { 2 } else { 1 };
    let protected = ctx.ignoring(&[
        Ignore::Code,
        Ignore::Math,
        Ignore::Yaml,
        Ignore::Link,
        Ignore::WikiLink,
        Ignore::Tag,
        Ignore::InlineMath,
    ]);
    let text = ctx.text;
    // Only the delimiters change, so a span may enclose protected text, but
    // its delimiters mustn't be protected.
    let positions: Vec<_> = ctx
        .positions(element)
        .into_iter()
        .filter(|p| {
            p.end - p.start >= 2 * width
                && !protected.is_protected(&(p.start..p.start + width))
                && !protected.is_protected(&(p.end - width..p.end))
        })
        .collect();
    let Some(first) = positions.last() else {
        return Vec::new();
    };
    let marker = match style {
        "underscore" => "_",
        "asterisk" => "*",
        _ => &text[first.start..first.start + 1],
    };
    let marker = marker.repeat(width);
    let mut edits = Vec::new();
    for p in &positions {
        edits.push(TextEdit::replace(p.start..p.start + width, marker.clone()));
        edits.push(TextEdit::replace(p.end - width..p.end, marker.clone()));
    }
    edits
}
