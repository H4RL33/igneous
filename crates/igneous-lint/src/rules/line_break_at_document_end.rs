use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, Options};

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "line-break-at-document-end"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Line break at document end"
    }

    fn description(&self) -> &'static str {
        "Ensures that there is exactly one line break at the end of a document if the note is not empty."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let text = ctx.text;
        if text.is_empty() {
            return Vec::new();
        }
        let content_end = text.trim_end_matches('\n').len();
        if text.len() - content_end == 1 {
            return Vec::new();
        }
        vec![TextEdit::replace(content_end..text.len(), "\n")]
    }
}
