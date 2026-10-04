use igneous_markdown::TextEdit;

use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::rules::dedupe_yaml_array_values::is_plain_array;
use crate::text::edits_between;
use crate::yaml::{
    ALIAS_KEYS, ArrayFormat, Items, TAG_KEYS, alias_items, format_array, format_yaml, load,
    section_value, set_section, split_array, tag_items,
};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new(
        "formatAliasKey",
        "Format YAML aliases section",
        OptionKind::Bool(true),
    )
    .describe("Turns on formatting for the YAML aliases section."),
    OptionSpec::new("formatTagKey", "Format YAML tags section", OptionKind::Bool(true))
        .describe("Turns on formatting for the YAML tags section."),
    OptionSpec::new(
        "formatArrayKeys",
        "Format YAML array sections",
        OptionKind::Bool(true),
    )
    .describe("Turns on formatting for regular YAML arrays"),
    OptionSpec::new(
        "forceSingleLineArrayStyle",
        "Force key values to be single-line arrays",
        OptionKind::List,
    )
    .describe("Forces the YAML array keys to be in single-line format (leave empty to disable this option)"),
    OptionSpec::new(
        "forceMultiLineArrayStyle",
        "Force key values to be multi-line arrays",
        OptionKind::List,
    )
    .describe("Forces the YAML array keys to be in multi-line format (leave empty to disable this option)"),
    OptionSpec::new("aliasArrayStyle", "Alias array style", OptionKind::Text("single-line")).shared(),
    OptionSpec::new("tagArrayStyle", "Tag array style", OptionKind::Text("single-line")).shared(),
    OptionSpec::new("defaultArrayStyle", "Array style", OptionKind::Text("single-line")).shared(),
    OptionSpec::new("defaultEscapeCharacter", "YAML quote character", OptionKind::Text("\"")).shared(),
    OptionSpec::new(
        "removeUnnecessaryEscapeCharsForMultiLineArrays",
        "Remove unnecessary quotes in multi-line arrays",
        OptionKind::Bool(false),
    )
    .shared(),
];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "format-yaml-array"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "Format YAML array"
    }

    fn description(&self) -> &'static str {
        "Allows for the formatting of regular YAML arrays as either multi-line or single-line, and of tags and aliases in Obsidian's own formats."
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
        let single = options.list("forceSingleLineArrayStyle");
        let multi = options.list("forceMultiLineArrayStyle");
        let Some(new) = format_yaml(ctx.text, |yaml| {
            let Some(entries) = load(yaml) else {
                return yaml.to_owned();
            };
            let has = |key: &str| entries.iter().any(|(k, _)| k == key);
            let mut yaml = yaml.to_owned();
            if options.bool("formatAliasKey")
                && let Some(key) = ALIAS_KEYS.into_iter().find(|k| has(k))
            {
                let items = split_array(section_value(&yaml, key).as_deref());
                let value = format_array(
                    alias_items(items),
                    ArrayFormat::parse(options.str("aliasArrayStyle")),
                    quote,
                    unescape,
                    true,
                );
                yaml = set_section(&yaml, key, &value);
            }
            if options.bool("formatTagKey")
                && let Some(key) = TAG_KEYS.into_iter().find(|k| has(k))
            {
                let items = split_array(section_value(&yaml, key).as_deref());
                let value = format_array(
                    tag_items(items),
                    ArrayFormat::parse(options.str("tagArrayStyle")),
                    quote,
                    unescape,
                    false,
                );
                yaml = set_section(&yaml, key, &value);
            }
            let reformat = |yaml: &str, key: &str, format: ArrayFormat| {
                let items = split_array(section_value(yaml, key).as_deref())
                    .map(Items::into_vec)
                    .unwrap_or_default();
                set_section(
                    yaml,
                    key,
                    &format_array(items, format, quote, unescape, false),
                )
            };
            if options.bool("formatArrayKeys") {
                let format = ArrayFormat::parse(options.str("defaultArrayStyle"));
                for (key, value) in &entries {
                    if ALIAS_KEYS.contains(&key.as_str())
                        || TAG_KEYS.contains(&key.as_str())
                        || single.contains(key)
                        || multi.contains(key)
                        || !is_plain_array(value)
                    {
                        continue;
                    }
                    yaml = reformat(&yaml, key, format);
                }
            }
            for key in single.iter().filter(|k| has(k)) {
                yaml = reformat(&yaml, key, ArrayFormat::SingleLine);
            }
            for key in multi.iter().filter(|k| has(k)) {
                yaml = reformat(&yaml, key, ArrayFormat::MultiLine);
            }
            yaml
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}
