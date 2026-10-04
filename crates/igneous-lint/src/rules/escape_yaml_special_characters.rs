use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::text::edits_between;
use crate::yaml::{escape, format_yaml, is_block_scalar_indicator};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new(
        "tryToEscapeSingleLineArrays",
        "Try to escape single line arrays",
        OptionKind::Bool(false),
    )
    .describe("Tries to escape array values assuming that an array starts with \"[\", ends with \"]\", and has items that are delimited by \",\"."),
    OptionSpec::new("defaultEscapeCharacter", "YAML quote character", OptionKind::Text("\"")).shared(),
];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "escape-yaml-special-characters"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Escape YAML special characters"
    }

    fn description(&self) -> &'static str {
        "Escapes colons with a space after them (: ), single quotes ('), and double quotes (\") in YAML."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::First
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let quote = options
            .str("defaultEscapeCharacter")
            .chars()
            .next()
            .unwrap_or('"');
        let arrays = options.bool("tryToEscapeSingleLineArrays");
        let Some(new) = format_yaml(ctx.text, |yaml| escape_lines(yaml, quote, arrays)) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn escape_lines(yaml: &str, quote: char, arrays: bool) -> String {
    let mut lines: Vec<String> = yaml.split('\n').map(str::to_owned).collect();
    // The indentation of a key holding a block scalar, while inside it.
    let mut block_scalar: Option<usize> = None;
    for i in 0..lines.len() {
        let original = lines[i].clone();
        let indent = indentation(&original);
        let trimmed = original.trim();
        if let Some(scalar_indent) = block_scalar {
            if trimmed.is_empty() || indent > scalar_indent {
                continue;
            }
            block_scalar = None;
        }
        if trimmed.is_empty() {
            continue;
        }
        let colon = trimmed.find(':');
        let dash = trimmed.starts_with('-');
        let key_without_value = colon.is_none_or(|c| c + 1 >= trimmed.len());
        let item_without_value = dash && trimmed.len() < 2;
        if key_without_value && item_without_value {
            continue;
        }
        let mut value_start: isize = 1;
        if !dash {
            value_start += colon.map_or(-1, |c| c as isize);
        } else if let Some(c) = colon
            && i + 1 < lines.len()
        {
            // An item holding a mapping: `- key: value`.
            let expected = original.find('-').unwrap_or(0) + 1;
            if expected <= indentation(&lines[i + 1]) {
                value_start += c as isize;
            }
        }
        let value = trimmed.get(value_start as usize..).unwrap_or("").trim();
        if is_block_scalar_indicator(value) {
            block_scalar = Some(indent);
            continue;
        }
        if value.starts_with('[') {
            if !arrays || value.len() < 3 {
                continue;
            }
            let inner = &value[1..value.len() - 1];
            let items: Vec<String> = inner
                .split(',')
                .map(|item| {
                    let mut core = item.trim();
                    if let Some(rest) = core.strip_prefix('[') {
                        core = rest.trim_start();
                    }
                    if let Some(rest) = core.strip_suffix(']') {
                        core = rest.trim_end();
                    }
                    if core.is_empty() {
                        return item.to_owned();
                    }
                    item.replacen(core, &escape(core, quote, false, true), 1)
                })
                .collect();
            lines[i] = original.replacen(value, &format!("[{}]", items.join(",")), 1);
            continue;
        }
        if value.is_empty() {
            continue;
        }
        lines[i] = original.replacen(value, &escape(value, quote, false, true), 1);
    }
    lines.join("\n")
}
