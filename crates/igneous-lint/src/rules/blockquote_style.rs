use std::sync::LazyLock;

use igneous_markdown::TextEdit;
use regex::Regex;

use crate::protect::{Ignore, RangeSet};
use crate::regexes::STARTS_WITH_LIST_MARKER;
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::syntax::Element;
use crate::text::{edits_between, is_ws, line_prefix};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "style",
    "Style",
    OptionKind::Choice {
        choices: &["space", "no space"],
        default: "space",
    },
)
.describe("The style used on blockquote indicators")];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "blockquote-style"
    }

    fn category(&self) -> Category {
        Category::Content
    }

    fn name(&self) -> &'static str {
        "Blockquote style"
    }

    fn description(&self) -> &'static str {
        "Makes sure the blockquote style is consistent."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::Last
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let space = options.str("style") != "no space";
        let text = ctx.text;
        let protected = ctx.ignoring(&[Ignore::Html, Ignore::Code, Ignore::Math]);
        let code_and_math = RangeSet::new(
            ctx.regions(Ignore::Code)
                .into_iter()
                .chain(ctx.regions(Ignore::Math))
                .collect(),
        );
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let line_of = |pos: usize| line_starts.partition_point(|s| *s <= pos) - 1;

        // Innermost (latest) blockquotes first, as obsidian-linter does. Only
        // the markers at the start of lines change, so lines keep their
        // places.
        for position in ctx.positions(Element::Blockquote) {
            let mut end = position.end;
            while end + 1 < text.len() && text.as_bytes()[end] != b'\n' {
                end += 1;
            }
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let quote = &text[position.start..end];
            let decisions = prepare(text, quote, position.start, &protected, &code_and_math);
            let first = line_of(position.start);
            let last = first + quote.matches('\n').count();
            let first_column = position.start - line_starts[first];
            let original_last_len = lines_len(text, &line_starts, last);
            let tail = original_last_len - (end - line_starts[last]).min(original_last_len);
            for (k, index) in (first..=last).enumerate() {
                let Some(&(skip, list_marker, has_content)) = decisions.get(k) else {
                    break;
                };
                let line = &lines[index];
                let seg_start = if k == 0 { first_column } else { 0 };
                let seg_end = if index == last {
                    line.len().saturating_sub(tail)
                } else {
                    line.len()
                };
                if seg_start > seg_end || skip {
                    continue;
                }
                let segment = &line[seg_start..seg_end];
                let scan_end = if index == last {
                    segment.len().saturating_sub(1)
                } else {
                    segment.len()
                };
                let prefix_len = segment.as_bytes()[..scan_end]
                    .iter()
                    .take_while(|b| is_ws(**b) || **b == b'>')
                    .count();
                let prefix = &segment[..prefix_len];
                let updated = if space {
                    add_space(prefix, list_marker, has_content)
                } else {
                    remove_space(prefix, list_marker)
                };
                if updated != prefix {
                    let mut new_line = String::with_capacity(line.len() + 2);
                    new_line.push_str(&line[..seg_start]);
                    new_line.push_str(&updated);
                    new_line.push_str(&line[seg_start + prefix_len..]);
                    lines[index] = new_line;
                }
            }
        }
        edits_between(text, &lines.join("\n"))
    }
}

fn lines_len(text: &str, starts: &[usize], index: usize) -> usize {
    let start = starts[index];
    let end = starts.get(index + 1).map_or(text.len(), |next| next - 1);
    end - start
}

/// For each line of a blockquote: whether to leave it alone, whether it holds
/// a list item, and whether it has content.
fn prepare(
    text: &str,
    quote: &str,
    offset: usize,
    protected: &RangeSet,
    code_and_math: &RangeSet,
) -> Vec<(bool, bool, bool)> {
    let mut out = Vec::new();
    let mut current = 0usize;
    loop {
        let (next_newline, done) = match quote[current.min(quote.len())..].find('\n') {
            Some(i) => (current + i, false),
            None => (quote.len().saturating_sub(1), true),
        };
        let (start_of_line, index) = line_prefix(quote, next_newline as isize - 1);
        let line_start = (offset as isize + index + 1) as usize;
        let content_start = line_start + start_of_line.len();
        let line_end = offset + next_newline + usize::from(done);
        let skip = code_and_math.is_protected(&(line_start..line_end))
            || protected.is_protected(&(line_start..content_start));
        let rest = redact(text, protected, content_start.min(line_end), line_end);
        out.push((
            skip,
            STARTS_WITH_LIST_MARKER.is_match(&rest),
            !rest.trim().is_empty(),
        ));
        if done {
            break;
        }
        current = next_newline + 1;
    }
    out
}

/// The text between `start` and `end` with protected regions replaced by a
/// placeholder, for decisions only.
fn redact(text: &str, protected: &RangeSet, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut cursor = start;
    for region in protected.overlapping(&(start..end)) {
        let from = region.start.max(start);
        let to = region.end.min(end);
        if from > cursor {
            out.push_str(&text[cursor..from]);
        }
        out.push_str("{PROTECTED}");
        cursor = to;
    }
    if cursor < end {
        out.push_str(&text[cursor..end]);
    }
    out
}

fn remove_space(prefix: &str, list_marker: bool) -> String {
    static BETWEEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">[ \t]+>").unwrap());
    static AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">[ \t]+").unwrap());
    if list_marker {
        BETWEEN.replace_all(prefix, ">>").into_owned()
    } else {
        AFTER.replace_all(prefix, ">").into_owned()
    }
}

fn add_space(prefix: &str, list_marker: bool, has_content: bool) -> String {
    static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">([^ >])").unwrap());
    static BARE_OR_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">([^ ]|\z)").unwrap());
    static DOUBLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">>").unwrap());
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+\z").unwrap());
    static WIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">(?:[ \t]{2,}|\t+)").unwrap());
    // An empty blockquote line gets no space after its marker; adding one
    // would leave trailing whitespace for `trailing-spaces` to remove again.
    if !has_content {
        let out = BARE.replace_all(prefix, "> $1");
        let out = DOUBLE.replace_all(&out, "> >");
        return TRAILING.replace(&out, "").into_owned();
    }
    let out = BARE_OR_END.replace_all(prefix, "> $1");
    let out = DOUBLE.replace_all(&out, "> >").into_owned();
    if list_marker {
        return out;
    }
    WIDE.replace_all(&out, "> ").into_owned()
}
