use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::rule::{Category, LintCtx, Options, Stage};
use crate::text::edits_between;
use crate::yaml::format_yaml;

pub struct Rule;

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "format-tags-in-yaml"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Format tags in YAML"
    }

    fn description(&self) -> &'static str {
        "Remove Hashtags from tags in the YAML frontmatter, as they make the tags there invalid."
    }

    fn stage(&self) -> Stage {
        Stage::First
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        static TAGS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?m)^(tags|tag):[ \t]*(\S.*|(?:(?:\n *- \S.*)|((?:\n *- *))*|(\n([ \t]+[^\n]*))*)*)\n",
            )
            .unwrap()
        });
        let Some(new) = format_yaml(ctx.text, |yaml| {
            TAGS.replacen(yaml, 1, |caps: &regex::Captures| caps[0].replace('#', ""))
                .into_owned()
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}
