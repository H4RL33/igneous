use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::text::edits_between;
use crate::yaml::{escape, format_yaml, is_value_escaped_already, section_value, set_section};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new("forceYamlEscape", "Force YAML escape on keys", OptionKind::List).describe(
        "Uses the YAML escape character on the specified YAML keys if it is not already escaped. Do not use on YAML arrays.",
    ),
    OptionSpec::new("defaultEscapeCharacter", "YAML quote character", OptionKind::Text("\"")).shared(),
];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "force-yaml-escape"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Force YAML escape"
    }

    fn description(&self) -> &'static str {
        "Escapes the values for the specified YAML keys."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let quote = options
            .str("defaultEscapeCharacter")
            .chars()
            .next()
            .unwrap_or('"');
        let keys = options.list("forceYamlEscape");
        if keys.is_empty() {
            return Vec::new();
        }
        let Some(new) = format_yaml(ctx.text, |yaml| {
            let mut yaml = yaml.to_owned();
            for key in &keys {
                let Some(value) = section_value(&yaml, key) else {
                    continue;
                };
                if value.contains('\n')
                    || value.starts_with(" [")
                    || is_value_escaped_already(&value)
                {
                    continue;
                }
                let escaped = escape(&value, quote, true, false);
                yaml = set_section(&yaml, key, &format!(" {escaped}"));
            }
            yaml
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}
