use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::rules::emphasis_style::consistent_delimiters;
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
.describe("The style used to denote strong/bolded content")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "strong-style"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Strong style"
    }

    fn description(&self) -> &'static str {
        "Makes sure the strong style is consistent."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        consistent_delimiters(ctx, options.str("style"), Element::Strong)
    }
}
