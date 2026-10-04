use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, Options, Stage};
use crate::syntax;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "add-blank-line-after-yaml"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Add blank line after YAML"
    }

    fn description(&self) -> &'static str {
        "Adds a blank line after the YAML block if it does not end the current file or it is not already followed by at least 1 blank line"
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        let text = ctx.text;
        let Some((range, _)) = syntax::yaml(text) else {
            return Vec::new();
        };
        let end = range.end;
        if end + 1 >= text.len()
            || text.trim_end() == text[range].trim_end()
            || text.as_bytes()[end + 1] == b'\n'
        {
            return Vec::new();
        }
        vec![TextEdit::insert(end, "\n")]
    }
}
