use igneous_markdown::TextEdit;

use crate::protect::Ignore;
use crate::regexes::ALL_HEADERS;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "startAtH2",
    "Start header increment at heading level 2",
    OptionKind::Bool(false),
)
.describe("Makes heading level 2 the minimum heading level in a file for header increment and shifts all headings accordingly so they increment starting with a level 2 heading.")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "header-increment"
    }

    fn category(&self) -> Category {
        Category::Heading
    }

    fn name(&self) -> &'static str {
        "Header increment"
    }

    fn description(&self) -> &'static str {
        "Heading levels should only increment by one level at a time"
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let projection = ctx.projection(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        let minimum = if options.bool("startAtH2") { 2 } else { 1 };
        const HIGHEST: usize = 6;
        // For each level found in the note, the level it becomes (0: unmapped).
        let mut levels = [0usize; HIGHEST];
        // The levels that started a new nesting depth, outermost first.
        let mut starts: Vec<usize> = Vec::new();
        let mut last = 0;
        let mut edits = Vec::new();
        for caps in ALL_HEADERS.captures_iter(&projection.text) {
            let start = caps.get(0).unwrap().start() + caps[1].len();
            let Some(range) = projection.edit_to_source(&(start..start + caps[2].len())) else {
                continue;
            };
            let level = caps[2].len().min(HIGHEST);
            if level < last {
                let mut remove_to = HIGHEST;
                while starts.last().is_some_and(|s| level <= *s) {
                    remove_to = starts.pop().unwrap();
                }
                remove_to = if starts.is_empty() { 0 } else { remove_to - 1 };
                for slot in levels.iter_mut().skip(remove_to) {
                    *slot = 0;
                }
            }
            if levels[level - 1] == 0 {
                let new_level = (starts.len() + minimum).min(HIGHEST);
                for slot in levels.iter_mut().take(level - 1).skip(last) {
                    *slot = new_level - 1;
                }
                starts.push(level);
                levels[level - 1] = new_level;
            }
            last = level;
            edits.push(TextEdit::replace(range, "#".repeat(levels[level - 1])));
        }
        edits
    }
}
