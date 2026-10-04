use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::Span;
use crate::regexes::MULTIPLE_BLANK_LINES;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::text::edits_between;
use crate::yaml::{format_yaml, is_block_scalar_indicator};

pub struct Rule;

static OPTIONS: &[OptionSpec] =
    &[
        OptionSpec::new("innerNewLines", "Inner new lines", OptionKind::Bool(false))
            .describe("Remove new lines that are not at the start or the end of the YAML"),
    ];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "compact-yaml"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Compact YAML"
    }

    fn description(&self) -> &'static str {
        "Removes leading and trailing blank lines in the YAML front matter."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn edits_frontmatter(&self) -> bool {
        true
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        static START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A---\n+").unwrap());
        static END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n+---").unwrap());
        let inner = options.bool("innerNewLines");
        let Some(new) = format_yaml(ctx.text, |yaml| {
            let yaml = START.replace(yaml, "---\n");
            let yaml = END.replace(&yaml, "\n---").into_owned();
            if inner {
                remove_blank_lines_outside_block_scalars(&yaml)
            } else {
                yaml
            }
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}

/// Collapses runs of blank lines, except inside `|` and `>` block scalars.
fn remove_blank_lines_outside_block_scalars(yaml: &str) -> String {
    let scalars = block_scalars(yaml);
    MULTIPLE_BLANK_LINES
        .replace_all(yaml, |caps: &regex::Captures| {
            let m = caps.get(0).unwrap();
            if scalars
                .iter()
                .any(|s| m.start() < s.end && m.end() > s.start)
            {
                m.as_str().to_owned()
            } else {
                "\n".to_owned()
            }
        })
        .into_owned()
}

/// The bodies of block scalars: from the indicator to the last line indented
/// more than its key.
fn block_scalars(yaml: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut offset = 0;
    let lines: Vec<&str> = yaml.split('\n').collect();
    let mut starts = Vec::with_capacity(lines.len());
    for line in &lines {
        starts.push(offset);
        offset += line.len() + 1;
    }
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        let value = line
            .split_once(": ")
            .map(|(_, v)| v)
            .or_else(|| line.trim_end().strip_suffix(':').map(|_| ""))
            .unwrap_or(line.trim_start().strip_prefix("- ").unwrap_or(""));
        if is_block_scalar_indicator(value.trim()) {
            let start = starts[i] + line.len();
            let mut end = start;
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j];
                let next_indent = next.len() - next.trim_start().len();
                if !next.trim().is_empty() && next_indent <= indent {
                    break;
                }
                if !next.trim().is_empty() {
                    end = starts[j] + next.len();
                }
                j += 1;
            }
            out.push(start..end);
            i = j;
            continue;
        }
        i += 1;
    }
    out
}
