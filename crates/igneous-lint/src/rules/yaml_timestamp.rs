use igneous_markdown::TextEdit;
use jiff::Zoned;
use jiff::tz::TimeZone;
use regex::Regex;

use crate::moment;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::syntax;
use crate::text::edits_between;
use crate::yaml::format_yaml;

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new("dateCreated", "Date created", OptionKind::Bool(true))
        .describe("Insert the file creation date"),
    OptionSpec::new(
        "dateCreatedKey",
        "Date created key",
        OptionKind::Text("date created"),
    )
    .describe("Which YAML key to use for creation date"),
    OptionSpec::new(
        "dateCreatedSourceOfTruth",
        "Date created source of truth",
        OptionKind::Choice {
            choices: &["file system", "frontmatter"],
            default: "file system",
        },
    )
    .describe("Specifies where to get the date created value from if it is already present in the frontmatter."),
    OptionSpec::new("dateModified", "Date modified", OptionKind::Bool(true))
        .describe("Insert the date the file was last modified"),
    OptionSpec::new(
        "dateModifiedKey",
        "Date modified key",
        OptionKind::Text("date modified"),
    )
    .describe("Which YAML key to use for modification date"),
    OptionSpec::new(
        "dateModifiedSourceOfTruth",
        "Date modified source of truth",
        OptionKind::Choice {
            choices: &["file system", "user or Linter edits"],
            default: "file system",
        },
    )
    .describe("Specifies what way should be used to determine when the date modified should be updated if it is already present in the frontmatter."),
    OptionSpec::new(
        "format",
        "Format",
        OptionKind::Text("dddd, MMMM Do YYYY, h:mm:ss a"),
    )
    .describe("Moment date format to use"),
    OptionSpec::new("convertToUTC", "Convert local time to UTC", OptionKind::Bool(false))
        .describe("Uses UTC equivalent for saved dates instead of local time"),
];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "yaml-timestamp"
    }

    fn category(&self) -> Category {
        Category::Yaml
    }

    fn name(&self) -> &'static str {
        "YAML timestamp"
    }

    fn description(&self) -> &'static str {
        "Keep track of the date the file was last edited in the YAML front matter. Gets dates from file metadata."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let utc = options.bool("convertToUTC");
        let tz = if utc {
            TimeZone::UTC
        } else {
            ctx.now.time_zone().clone()
        };
        let stamp = Stamp {
            format: options.str("format").trim_end().to_owned(),
            tz: tz.clone(),
            now: ctx.now.with_time_zone(tz.clone()),
            created: ctx
                .file
                .created
                .as_ref()
                .unwrap_or(ctx.now)
                .with_time_zone(tz.clone()),
            modified: ctx
                .file
                .modified
                .as_ref()
                .unwrap_or(ctx.now)
                .with_time_zone(tz),
        };
        let mut modified = ctx.already_modified;
        let text = if syntax::yaml(ctx.text).is_none() {
            modified = true;
            format!("---\n---\n{}", ctx.text)
        } else {
            ctx.text.to_owned()
        };
        let Some(new) = format_yaml(&text, |yaml| {
            let mut yaml = yaml.to_owned();
            if options.bool("dateCreated") {
                let (new, changed) = stamp.created(
                    &yaml,
                    options.str("dateCreatedKey"),
                    options.str("dateCreatedSourceOfTruth") == "frontmatter",
                );
                yaml = new;
                modified |= changed;
            }
            if options.bool("dateModified") {
                yaml = stamp.modified(
                    &yaml,
                    options.str("dateModifiedKey"),
                    modified,
                    options.str("dateModifiedSourceOfTruth") != "user or Linter edits",
                );
            }
            yaml
        }) else {
            return Vec::new();
        };
        edits_between(ctx.text, &new)
    }
}

struct Stamp {
    format: String,
    tz: TimeZone,
    now: Zoned,
    created: Zoned,
    modified: Zoned,
}

/// `\n<key>: <value>\n` and `\n<key>:\n`.
fn key_patterns(key: &str) -> (Regex, Regex) {
    let key = regex::escape(key);
    (
        Regex::new(&format!(r"\n{key}: [^\n]+\n")).unwrap(),
        Regex::new(&format!(r"\n{key}:[ \t]*\n")).unwrap(),
    )
}

/// The value in a `\n<key>: <value>\n` match.
fn value_of(yaml: &str, pattern: &Regex, key: &str) -> Option<String> {
    let found = pattern.find(yaml)?.as_str();
    Some(found.replacen(&format!("{key}:"), "", 1).trim().to_owned())
}

/// Adds `line` before the closing fence.
fn append(yaml: &str, line: &str) -> String {
    let at = yaml.find("\n---").unwrap_or(yaml.len());
    format!("{}{line}{}", &yaml[..at], &yaml[at..])
}

impl Stamp {
    fn created(&self, yaml: &str, key: &str, from_frontmatter: bool) -> (String, bool) {
        let (with_value, empty) = key_patterns(key);
        let line = format!("\n{key}: {}", moment::format(&self.created, &self.format));
        if !with_value.is_match(yaml) {
            if empty.is_match(yaml) {
                return (
                    empty
                        .replacen(yaml, 1, regex::NoExpand(&format!("{line}\n")))
                        .into_owned(),
                    true,
                );
            }
            return (append(yaml, &line), true);
        }
        let Some(current) = value_of(yaml, &with_value, key) else {
            return (yaml.to_owned(), false);
        };
        if from_frontmatter {
            let Some(guessed) = moment::guess_format(&current, &self.format, &self.tz) else {
                return (yaml.to_owned(), false);
            };
            // Rewrite the date in the configured format.
            let Some(time) = moment::parse(&current, guessed, &self.tz) else {
                return (yaml.to_owned(), false);
            };
            let rewritten = moment::format(&time.with_time_zone(self.tz.clone()), &self.format);
            if moment::parse(&rewritten, &self.format, &self.tz).is_none() || rewritten == current {
                return (yaml.to_owned(), false);
            }
            let line = format!("\n{key}: {rewritten}\n");
            return (
                with_value
                    .replacen(yaml, 1, regex::NoExpand(&line))
                    .into_owned(),
                true,
            );
        }
        let valid = if self.format.is_empty() {
            moment::parse(&current, &self.format, &self.tz).is_some()
                || moment::parse_iso(&current, &self.tz).is_some()
        } else {
            moment::matches(&current, &self.format, &self.tz).is_some()
        };
        if valid {
            return (yaml.to_owned(), false);
        }
        (
            with_value
                .replacen(yaml, 1, regex::NoExpand(&format!("{line}\n")))
                .into_owned(),
            true,
        )
    }

    fn modified(&self, yaml: &str, key: &str, changed: bool, check_file: bool) -> String {
        let (with_value, empty) = key_patterns(key);
        let line = format!("\n{key}: {}\n", moment::format(&self.now, &self.format));
        if with_value.is_match(yaml) {
            let current = value_of(yaml, &with_value, key).unwrap_or_default();
            let parsed = moment::matches(&current, &self.format, &self.tz);
            let stale = match &parsed {
                None => true,
                Some(time) => {
                    check_file && {
                        let file = moment::format(&self.modified, &self.format);
                        moment::parse(&file, &self.format, &self.tz).is_some_and(|file| {
                            (time.timestamp().as_second() - file.timestamp().as_second()).abs() > 5
                        })
                    }
                }
            };
            if changed || stale {
                return with_value
                    .replacen(yaml, 1, regex::NoExpand(&line))
                    .into_owned();
            }
            return yaml.to_owned();
        }
        if empty.is_match(yaml) {
            return empty.replacen(yaml, 1, regex::NoExpand(&line)).into_owned();
        }
        append(yaml, line.trim_end_matches('\n'))
    }
}
