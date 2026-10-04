use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::{Captures, Regex};

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new("bottom", "Bottom", OptionKind::Bool(true)).describe(
        "Ensures one blank line after headings (when disabled it does not remove blank lines after headings)",
    ),
    OptionSpec::new(
        "emptyLineAfterYaml",
        "Empty line between YAML and header",
        OptionKind::Bool(true),
    )
    .describe("Keep the empty line between the YAML frontmatter and header"),
];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "heading-blank-lines"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Heading blank lines"
    }

    fn description(&self) -> &'static str {
        "All headings have one blank line both before and after (except where the heading is at the beginning or end of the document)."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        static TOP_ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^([^#\n][^\n]+)\n+(#+\s.*)"));
        static AROUND: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(#+\s.*)"));
        static BEFORE: LazyLock<Regex> = LazyLock::new(|| re(r"\n+(#+\s.*)"));
        static AFTER: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)(^#+\s.*)\n+"));
        static FIRST: LazyLock<Regex> = LazyLock::new(|| re(r"\A\n+(#+\s.*)"));
        static LAST: LazyLock<Regex> = LazyLock::new(|| re(r"(#+\s.*)\n+\z"));
        static AFTER_YAML: LazyLock<Regex> = LazyLock::new(|| re(r"\A(---\n---)\n+(#+\s.*)"));

        let projection = ctx.projection(&[
            Ignore::Code,
            Ignore::Math,
            Ignore::Yaml,
            Ignore::Link,
            Ignore::WikiLink,
        ]);
        // The passes run in order: later ones tidy the blank lines earlier
        // ones add.
        let mut text = projection.text.clone();
        let group = |caps: &Captures, i: usize| caps.get(i).map_or("", |m| m.as_str()).to_owned();
        if !options.bool("bottom") {
            text = TOP_ONLY
                .replace_all(&text, |c: &Captures| {
                    format!("{}\n\n{}", group(c, 1), group(c, 2))
                })
                .into_owned();
        } else {
            text = AROUND
                .replace_all(&text, |c: &Captures| format!("\n\n{}\n\n", group(c, 1)))
                .into_owned();
            text = BEFORE
                .replace_all(&text, |c: &Captures| format!("\n\n{}", group(c, 1)))
                .into_owned();
            text = AFTER
                .replace_all(&text, |c: &Captures| format!("{}\n\n", group(c, 1)))
                .into_owned();
        }
        text = FIRST
            .replace(&text, |c: &Captures| group(c, 1))
            .into_owned();
        text = LAST.replace(&text, |c: &Captures| group(c, 1)).into_owned();
        if !options.bool("emptyLineAfterYaml") {
            text = AFTER_YAML
                .replace(&text, |c: &Captures| {
                    format!("{}\n{}", group(c, 1), group(c, 2))
                })
                .into_owned();
        }
        projection.edits_to(&text)
    }
}
