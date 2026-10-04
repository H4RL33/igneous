use igneous_markdown::TextEdit;
use regex::Regex;

use crate::Span;
use crate::protect::Ignore;
use crate::regexes::{EMPTY_LINE_MATH_BLOCKQUOTE, STARTS_WITH_BLOCKQUOTE};
use crate::rule::{Category, LintCtx, OptionKind, OptionSpec, Options, Stage};
use crate::rules::empty_line_around_code_fences::projected_positions;
use crate::rules::empty_line_around_math_blocks::{dollar_count, inline_math_after};
use crate::syntax::Element;
use crate::text::{count_instances, line_prefix, start_of_line};

pub struct Rule;

static OPTIONS: &[OptionSpec] = &[OptionSpec::new(
    "minimumNumberOfDollarSignsToBeAMathBlock",
    "Number of dollar signs to indicate a math block",
    OptionKind::Number(2.0),
)
.shared()];

impl crate::rule::Rule for Rule {
    fn id(&self) -> &'static str {
        "move-math-block-indicators-to-their-own-line"
    }

    fn category(&self) -> Category {
        Category::Spacing
    }

    fn name(&self) -> &'static str {
        "Move math block indicators to their own line"
    }

    fn description(&self) -> &'static str {
        "Move all starting and ending math block indicators to their own lines, using the number of dollar signs that indicates a math block to recognise single-line math."
    }

    fn options(&self) -> &'static [OptionSpec] {
        OPTIONS
    }

    fn stage(&self) -> Stage {
        Stage::First
    }

    fn edits_math(&self) -> bool {
        true
    }

    fn fix(&self, ctx: &LintCtx, options: &Options) -> Vec<TextEdit> {
        let n = dollar_count(options);
        let opening = Regex::new(&format!(r"\A(\${{{n},}})(\n*)")).unwrap();
        let closing = Regex::new(&format!(r"(\n*)(\${{{n},}})([^\$]*)\z")).unwrap();
        let projection = ctx.projection(&[Ignore::Code, Ignore::InlineCode]);
        let mut text = projection.text.clone();
        for range in projected_positions(ctx, &projection, Element::Math) {
            let block = text[range.clone()].to_owned();
            // Overlapping ranges are rewritten one after another, as in
            // obsidian-linter.
            for part in split_blocks(&block, n, range.start) {
                text = own_lines(&text, part, &opening, &closing);
            }
        }
        let dollars = "$".repeat(n);
        for range in inline_math_after(ctx, &projection, &text) {
            if !text[range.clone()].starts_with(&dollars) {
                continue;
            }
            text = own_lines(&text, range, &opening, &closing);
        }
        projection.edits_to(&text)
    }
}

/// A math block holding several `$$` pairs, split into one range per pair.
/// obsidian-linter's `breakMathBlockIntoMultipleBlocksIfNeedBe`.
fn split_blocks(block: &str, n: usize, offset: usize) -> Vec<Span> {
    let mut indicator = "$".repeat(n);
    let mut end_of_opening = n;
    while block.as_bytes().get(end_of_opening) == Some(&b'$') {
        indicator.push('$');
        end_of_opening += 1;
    }
    let find = |from: usize| {
        block
            .get(from..)
            .and_then(|b| b.find(&indicator))
            .map(|i| from + i)
    };
    let mut count = count_instances(block, &indicator);
    let mut out: Vec<Span> = Vec::new();
    if count <= 1 {
        return out;
    }
    if count == 2 {
        out.push(offset..offset + block.len());
        return out;
    }
    if count == 3 {
        let second = find(indicator.len()).unwrap_or(0);
        out.insert(0, offset..offset + second + indicator.len());
    }
    if count % 2 == 1 {
        count -= 1;
    }
    let mut start = offset;
    let mut search = indicator.len();
    while count > 2 {
        let end = find(search).unwrap_or(block.len()) + indicator.len();
        out.insert(0, start..offset + end);
        start = offset + end + 1;
        search = end + 1;
        count -= 2;
    }
    let last_start = find(search).unwrap_or(0);
    out.insert(0, offset + last_start..offset + block.len());
    out
}

/// Puts the opening and closing `$$` of the math in `range` on lines of their
/// own. obsidian-linter's `addBlankLinesAroundStartAndStopMathIndicators`.
fn own_lines(text: &str, range: Span, opening: &Regex, closing: &Regex) -> String {
    let (start, end) = (range.start.min(text.len()), range.end.min(text.len()));
    if start > end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        return text.to_owned();
    }
    let line_before = &text[start_of_line(text, start)..start];
    let (line_start, _) = line_prefix(line_before, line_before.len() as isize);
    let ending_line = &text[start_of_line(text, end)..end];
    let in_quote = STARTS_WITH_BLOCKQUOTE.is_match(line_before.trim());
    let mut newline_added = false;

    let block = &text[start..end];
    let block = opening.replace(block, |caps: &regex::Captures| {
        let mut out = String::new();
        if !in_quote && !line_before.trim().is_empty() {
            out.push('\n');
            newline_added = true;
        } else if in_quote && !EMPTY_LINE_MATH_BLOCKQUOTE.is_match(line_before) {
            out.push('\n');
            out.push_str(&line_start);
            newline_added = true;
        }
        out.push_str(&caps[1]);
        out.push('\n');
        if caps[2].is_empty() && in_quote {
            out.push_str(&line_start);
        }
        out
    });
    let block = closing.replace(&block, |caps: &regex::Captures| {
        let newlines_empty = caps[1].is_empty();
        if newlines_empty && in_quote && EMPTY_LINE_MATH_BLOCKQUOTE.is_match(ending_line.trim()) {
            return caps[0].to_owned();
        }
        if newlines_empty && in_quote {
            return format!("\n{}{}{}", line_start, &caps[2], &caps[3]);
        }
        format!("\n{}{}", &caps[2], &caps[3])
    });

    // Drop whitespace left before an opening moved to its own line.
    let mut cut = start;
    if newline_added {
        while cut > 0 && matches!(text.as_bytes()[cut - 1], b' ' | b'\t') {
            cut -= 1;
        }
    }
    format!("{}{}{}", &text[..cut], block, &text[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_blocks() {
        assert_eq!(split_blocks("$$a$$", 2, 10), vec![10..15]);
        assert!(split_blocks("$$a", 2, 0).is_empty());
        assert_eq!(count_instances("$$a$$b$$c$$", "$$"), 4);
        assert_eq!(split_blocks("$$a$$b$$c$$", 2, 0), vec![6..11, 0..5]);
    }
}
