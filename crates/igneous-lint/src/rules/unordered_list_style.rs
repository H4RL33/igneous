use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::syntax::Element;

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "listStyle",
    "List item style",
    OptionKind::Choice {
        choices: &["consistent", "-", "*", "+"],
        default: "consistent",
    },
)
.describe("The list item style to use in unordered lists")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "unordered-list-style"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Unordered list style"
    }

    fn description(&self) -> &'static str {
        "Makes sure that unordered lists follow the style specified."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        // Ordered items and `- [ ]` tasks keep their markers.
        static SKIPPED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^((\d+[.)])|(- \[[ x]\]))").unwrap());
        let text = ctx.text;
        let protected = ctx.ignoring(&[Ignore::Code, Ignore::Math, Ignore::Yaml, Ignore::Tag]);
        let positions: Vec<_> = ctx
            .positions(Element::ListItem)
            .into_iter()
            .filter(|p| !protected.is_protected(&(p.start..p.start + 1)))
            .collect();
        let mut style = options.str("listStyle").to_owned();
        if style == "consistent" || style.is_empty() {
            // The first item that isn't ordered or a task decides. As in
            // obsidian-linter, when that's the last item, nothing changes.
            let mut i = positions.len() as isize - 1;
            let mut found = None;
            while i >= 0 {
                let item = &text[positions[i as usize].clone()];
                i -= 1;
                if SKIPPED.is_match(item) {
                    continue;
                }
                found = item.chars().next();
                break;
            }
            match found {
                Some(c) if i != -1 => style = c.to_string(),
                _ => return Vec::new(),
            }
        }
        positions
            .iter()
            .filter(|p| !SKIPPED.is_match(&text[(*p).clone()]))
            .map(|p| TextEdit::replace(p.start..p.start + 1, style.clone()))
            .collect()
    }
}
