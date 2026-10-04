use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, Options};

pub struct Rule;

const MARKER: &str = r"^\s*(>\s*)*(-|\*|\+|\d+[.)]|- (\[(.)\]))\s*?$";

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "remove-empty-list-markers"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Remove empty list markers"
    }

    fn description(&self) -> &'static str {
        "Removes empty list markers, i.e. list items without content."
    }

    fn fix(&self, ctx: &LintCtx, _: &Options) -> Vec<TextEdit> {
        static FOLLOWED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(&format!(r"(?m){MARKER}\n")).unwrap());
        static PRECEDED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(&format!(r"(?m)\n{MARKER}")).unwrap());
        static ALONE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(&format!("(?m){MARKER}")).unwrap());
        let projection = ctx.projection(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
            Ignore::Tag,
        ]);
        let text = FOLLOWED.replace_all(&projection.text, "");
        let text = PRECEDED.replace_all(&text, "");
        let text = ALONE.replace_all(&text, "");
        projection.edits_to(&text)
    }
}
