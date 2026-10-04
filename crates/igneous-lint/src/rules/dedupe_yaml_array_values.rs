use igneous_markdown::TextEdit;
use igneous_markdown::frontmatter::Value;

use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::text::edits_between;
use crate::yaml::{
    ALIAS_KEYS, ArrayFormat, Items, TAG_KEYS, alias_items, format_array, format_yaml, load,
    section_value, set_section, split_array, tag_items,
};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new(
        "dedupeAliasKey",
        "Dedupe YAML aliases section",
        OptionKind::Bool(true),
    )
    .describe("Turns on removing duplicate aliases."),
    OptionSpec::new(
        "dedupeTagKey",
        "Dedupe YAML tags section",
        OptionKind::Bool(true),
    )
    .describe("Turns on removing duplicate tags."),
    OptionSpec::new(
        "dedupeArrayKeys",
        "Dedupe YAML array sections",
        OptionKind::Bool(true),
    )
    .describe("Turns on removing duplicate values for regular YAML arrays"),
    OptionSpec::new(
        "ignoreDedupeArrayKeys",
        "YAML keys to ignore for Dedupe YAML array values",
        OptionKind::List,
    )
    .describe("YAML keys whose duplicate values are kept."),
    OptionSpec::new(
        "aliasArrayStyle",
        "Alias array style",
        OptionKind::Text("single-line"),
    )
    .shared(),
    OptionSpec::new(
        "tagArrayStyle",
        "Tag array style",
        OptionKind::Text("single-line"),
    )
    .shared(),
    OptionSpec::new(
        "defaultEscapeCharacter",
        "YAML quote character",
        OptionKind::Text("\""),
    )
    .shared(),
    OptionSpec::new(
        "removeUnnecessaryEscapeCharsForMultiLineArrays",
        "Remove unnecessary quotes in multi-line arrays",
        OptionKind::Bool(false),
    )
    .shared(),
];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "dedupe-yaml-array-values"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Dedupe YAML array values"
    }

    fn description(&self) -> &'static str {
        "Removes duplicate array values in a case sensitive manner."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let quote = options
            .str("defaultEscapeCharacter")
            .chars()
            .next()
            .unwrap_or('"');
        let unescape = options.bool("removeUnnecessaryEscapeCharsForMultiLineArrays");
        let Some(new) = format_yaml(ctx.text, |yaml| {
            let Some(entries) = load(yaml) else {
                return yaml.to_owned();
            };
            let has = |key: &str| entries.iter().any(|(k, _)| k == key);
            let mut yaml = yaml.to_owned();
            if options.bool("dedupeAliasKey")
                && let Some(key) = ALIAS_KEYS.into_iter().find(|k| has(k))
            {
                let items = unique(split_array(section_value(&yaml, key).as_deref()));
                let value = format_array(
                    alias_items(items),
                    ArrayFormat::parse(options.str("aliasArrayStyle")),
                    quote,
                    unescape,
                    true,
                );
                yaml = set_section(&yaml, key, &value);
            }
            if options.bool("dedupeTagKey")
                && let Some(key) = TAG_KEYS.into_iter().find(|k| has(k))
            {
                let items = unique(split_array(section_value(&yaml, key).as_deref()));
                let value = format_array(
                    tag_items(items),
                    ArrayFormat::parse(options.str("tagArrayStyle")),
                    quote,
                    unescape,
                    false,
                );
                yaml = set_section(&yaml, key, &value);
            }
            if options.bool("dedupeArrayKeys") {
                let ignored = options.list("ignoreDedupeArrayKeys");
                for (key, value) in &entries {
                    if ALIAS_KEYS.contains(&key.as_str())
                        || TAG_KEYS.contains(&key.as_str())
                        || ignored.contains(key)
                        || !is_plain_array(value)
                    {
                        continue;
                    }
                    let Some(current) = section_value(&yaml, key) else {
                        continue;
                    };
                    let format = if current.contains('\n') {
                        ArrayFormat::MultiLine
                    } else {
                        ArrayFormat::SingleLine
                    };
                    let items = unique(split_array(Some(&current)))
                        .map(Items::into_vec)
                        .unwrap_or_default();
                    let value = format_array(items, format, quote, unescape, false);
                    yaml = set_section(&yaml, key, &value);
                }
            }
            yaml
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}

/// An array whose first item (if any) isn't an object.
pub(crate) fn is_plain_array(value: &Value) -> bool {
    match value {
        Value::List(items) => !matches!(items.first(), Some(Value::Map(_))),
        _ => false,
    }
}

/// Drops repeated items, comparing them without their quotes.
fn unique(items: Option<Items>) -> Option<Items> {
    match items {
        Some(Items::Many(values)) if values.len() > 1 => {
            let mut seen: Vec<String> = Vec::new();
            let mut out = Vec::new();
            for value in values {
                let normalised = unquote(&value).to_owned();
                if seen.contains(&normalised) {
                    continue;
                }
                seen.push(normalised);
                out.push(value);
            }
            Some(Items::Many(out))
        }
        other => other,
    }
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}
