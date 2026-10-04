use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::Ignore;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options};
use crate::syntax::Element;
use crate::text::{edits_between, start_of_line};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[
    OptionSpec::new(
        "numberStyle",
        "Number style",
        OptionKind::Choice {
            choices: &["ascending", "lazy", "preserve"],
            default: "ascending",
        },
    )
    .describe("The number style used in ordered list markers"),
    OptionSpec::new(
        "listEndStyle",
        "Ordered list marker end style",
        OptionKind::Choice {
            choices: &[".", ")"],
            default: ".",
        },
    )
    .describe("The ending character of an ordered list marker"),
    OptionSpec::new("preserveStart", "Preserve starting number", OptionKind::Bool(false))
        .describe("Whether to preserve the starting number of an ordered list. This can be used to have an ordered list that has content in between the ordered list items."),
];

static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(( |\t|> )*)((\d+(\.|\)))|[-*+])([^\n]*)$").unwrap());

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "ordered-list-style"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Ordered list style"
    }

    fn description(&self) -> &'static str {
        "Makes sure that ordered lists follow the style specified. Note that 2 spaces or 1 tab is considered to be an indentation level."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let style = options.str("numberStyle");
        let end_style = match options.str("listEndStyle") {
            ")" => ")",
            _ => ".",
        };
        let preserve_start = options.bool("preserveStart");
        let protected = ctx.ignoring(&[Ignore::Code, Ignore::Math, Ignore::Yaml, Ignore::Tag]);
        let original = ctx.text;

        // Markers that are protected, by line: renumbering never changes
        // newlines, so lines stay put while nested lists change widths.
        let mut protected_lines = HashSet::new();
        for caps in LIST_ITEM.captures_iter(original) {
            let m = caps.get(0).unwrap();
            let marker_start = m.start() + caps.get(1).map_or(0, |g| g.len());
            if protected.is_protected(&(marker_start..marker_start + caps[3].len())) {
                protected_lines.insert(original[..m.start()].matches('\n').count());
            }
        }

        let mut text = original.to_owned();
        for position in ctx.positions(Element::List) {
            let start = start_of_line(&text, position.start.min(text.len()));
            let mut end = position.end.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let list = &text[start..end];
            let first_line = text[..start].matches('\n').count();
            let mut numbers: HashMap<usize, i64> = HashMap::new();
            let mut last_level: isize = -1;
            let mut out = String::with_capacity(list.len());
            let mut cursor = 0;
            for caps in LIST_ITEM.captures_iter(list) {
                let m = caps.get(0).unwrap();
                out.push_str(&list[cursor..m.start()]);
                cursor = m.end();
                let line = first_line + list[..m.start()].matches('\n').count();
                if protected_lines.contains(&line) {
                    out.push_str(m.as_str());
                    continue;
                }
                let indent = caps.get(1).map_or("", |g| g.as_str());
                let marker = &caps[3];
                let level = level_of(indent) as isize;
                if !marker.starts_with(|c: char| c.is_ascii_digit()) {
                    let highest = level.max(last_level);
                    forget(&mut numbers, level, highest);
                    out.push_str(m.as_str());
                    continue;
                }
                let written: i64 = caps[4].trim_end_matches(['.', ')']).parse().unwrap_or(1);
                let mut number = if style == "preserve" || preserve_start {
                    written
                } else {
                    1
                };
                let key = level as usize;
                if let Some(previous) = numbers.get(&key).copied() {
                    if style == "ascending" {
                        number = previous + 1;
                        numbers.insert(key, number);
                    } else if preserve_start {
                        number = previous;
                    }
                } else {
                    numbers.insert(key, number);
                }
                if last_level > level {
                    forget(&mut numbers, level, last_level);
                }
                last_level = level;
                out.push_str(indent);
                out.push_str(&number.to_string());
                out.push_str(end_style);
                out.push_str(caps.get(6).map_or("", |g| g.as_str()));
            }
            out.push_str(&list[cursor..]);
            text.replace_range(start..end, &out);
        }
        edits_between(original, &text)
    }
}

/// The nesting level of a list item: 2 spaces or a tab per level, counted
/// after the last blockquote marker.
fn level_of(indent: &str) -> usize {
    let after_quote = indent.rfind("> ").map_or(indent, |i| &indent[i + 2..]);
    let spaces = after_quote.replace('\t', "  ");
    spaces.matches(' ').count() / 2 + 1
}

/// Forgets the numbering of levels after `from`, through `to`.
fn forget(numbers: &mut HashMap<usize, i64>, from: isize, to: isize) {
    let mut i = to;
    while i > from {
        numbers.remove(&(i as usize));
        i -= 1;
    }
}
