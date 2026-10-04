use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::regexes::{ALL_HEADERS, HTML_ENTITY_AT_END};
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "punctuationToRemove",
    "Trailing punctuation",
    OptionKind::Text(".,;:!。，；：！"),
)
.describe("The trailing punctuation to remove from the headings in the file.")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "remove-trailing-punctuation-in-heading"
    }

    fn category(&self) -> Category {
        Category::Heading
    }

    fn name(&self) -> &'static str {
        "Remove trailing punctuation in heading"
    }

    fn description(&self) -> &'static str {
        "Removes the specified punctuation from the end of headings making sure to ignore the semicolon at the end of HTML entity references."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let punctuation: Vec<char> = options.str("punctuationToRemove").chars().collect();
        let projection = ctx.projection(&[Ignore::Code, Ignore::Math, Ignore::Yaml]);
        let mut edits = Vec::new();
        for caps in ALL_HEADERS.captures_iter(&projection.text) {
            let heading = caps.get(4).map_or("", |m| m.as_str());
            if heading.is_empty() || HTML_ENTITY_AT_END.is_match(heading) {
                continue;
            }
            let trimmed = heading.trim_end();
            let kept = trimmed.trim_end_matches(punctuation.as_slice());
            if kept.len() == trimmed.len() {
                continue;
            }
            let text_start = caps.get(4).unwrap().start();
            let range = text_start + kept.len()..text_start + trimmed.len();
            if let Some(range) = projection.edit_to_source(&range) {
                edits.push(TextEdit::delete(range));
            }
        }
        edits
    }
}
