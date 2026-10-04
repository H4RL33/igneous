//! Text helpers shared by the rules: diffs, replacements, and ports of
//! obsidian-linter's string utilities.
//!
//! The ports keep obsidian-linter's character-by-character logic, working on
//! bytes. Every character they look for (`\n`, `>`, spaces and tabs) is ASCII,
//! and the bytes of other characters are never mistaken for one, so offsets
//! they return always fall on character boundaries.

use std::time::{Duration, Instant};

use igneous_markdown::TextEdit;
use similar::{Algorithm, DiffOp};

use crate::Span;

/// The edits that turn `before` into `after`, from a character diff.
pub fn edits_between(before: &str, after: &str) -> Vec<TextEdit> {
    if before == after {
        return Vec::new();
    }
    // Trim the common ends first: rules usually change little of a note.
    let prefix = before
        .char_indices()
        .zip(after.chars())
        .find(|((_, a), b)| a != b)
        .map_or(before.len().min(after.len()), |((i, _), _)| i);
    let prefix = floor_boundary(after, prefix.min(after.len())).min(prefix);
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let old = &before[prefix..before.len() - suffix];
    let new = &after[prefix..after.len() - suffix];
    if old.is_empty() || new.is_empty() {
        return vec![TextEdit::replace(
            prefix..prefix + old.len(),
            new.to_owned(),
        )];
    }

    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let old_offsets = char_offsets(old);
    let new_offsets = char_offsets(new);
    let deadline = Instant::now() + Duration::from_millis(500);
    let ops = similar::capture_diff_slices_deadline(
        Algorithm::Myers,
        &old_chars,
        &new_chars,
        Some(deadline),
    );

    let mut edits: Vec<TextEdit> = Vec::new();
    let mut pending: Option<(Span, Span)> = None;
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {
                if let Some((o, n)) = pending.take() {
                    edits.push(TextEdit::replace(
                        prefix + old_offsets[o.start]..prefix + old_offsets[o.end],
                        &new[new_offsets[n.start]..new_offsets[n.end]],
                    ));
                }
            }
            _ => {
                let (o, n) = (op.old_range(), op.new_range());
                pending = Some(match pending.take() {
                    Some((po, pn)) => (po.start..o.end, pn.start..n.end),
                    None => (o, n),
                });
            }
        }
    }
    if let Some((o, n)) = pending {
        edits.push(TextEdit::replace(
            prefix + old_offsets[o.start]..prefix + old_offsets[o.end],
            &new[new_offsets[n.start]..new_offsets[n.end]],
        ));
    }
    edits
}

/// Byte offset of each character, plus the length at the end.
fn char_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect()
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Applies sorted, non-overlapping replacements.
pub fn replace_ranges(text: &str, replacements: &[(Span, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, value) in replacements {
        debug_assert!(range.start >= cursor, "overlapping replacements");
        out.push_str(&text[cursor..range.start]);
        out.push_str(value);
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Sorts replacements and drops any overlapping an earlier one.
pub fn non_overlapping(mut replacements: Vec<(Span, String)>) -> Vec<(Span, String)> {
    replacements.sort_by_key(|(r, _)| (r.start, r.end));
    let mut out: Vec<(Span, String)> = Vec::with_capacity(replacements.len());
    for (range, value) in replacements {
        if out.last().is_some_and(|(last, _)| range.start < last.end) {
            continue;
        }
        out.push((range, value));
    }
    out
}

/// Turns replacements into edits.
pub fn to_edits(replacements: Vec<(Span, String)>) -> Vec<TextEdit> {
    replacements
        .into_iter()
        .map(|(range, value)| TextEdit::replace(range, value))
        .collect()
}

pub fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// The byte at `i`, or `None` outside the text (JavaScript's `charAt` gives
/// `''` there).
fn at(text: &[u8], i: isize) -> Option<u8> {
    usize::try_from(i).ok().and_then(|i| text.get(i).copied())
}

/// Whitespace in JavaScript's `char.trim() === ''` sense, where a character
/// outside the text counts too.
fn blank(text: &[u8], i: isize) -> bool {
    at(text, i).is_none_or(is_ws)
}

/// Counts occurrences, overlapping ones included, as obsidian-linter's
/// `countInstances` does.
pub fn count_instances(text: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut from = 0;
    while let Some(i) = text[from..].find(needle) {
        count += 1;
        let at = from + i;
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    count
}

/// Start of the line holding `index`.
pub fn start_of_line(text: &str, index: usize) -> usize {
    text[..index].rfind('\n').map_or(0, |i| i + 1)
}

/// Walks back from `start` collecting the whitespace and `>` characters that
/// begin its line. Returns them and the offset of the line's preceding
/// newline (`-1` at the start of the text). obsidian-linter's
/// `getStartOfLineWhitespaceOrBlockquoteLevel`.
pub fn line_prefix(text: &str, start: isize) -> (String, isize) {
    if start == 0 {
        return (String::new(), 0);
    }
    let bytes = text.as_bytes();
    let mut prefix_start: Option<usize> = None;
    let mut prefix_end: Option<usize> = None;
    let mut index = start;
    while index >= 0 {
        match at(bytes, index) {
            Some(b'\n') => break,
            None => {}
            Some(b) if is_ws(b) || b == b'>' => {
                let i = index as usize;
                prefix_start = Some(i);
                prefix_end.get_or_insert(i + 1);
            }
            Some(_) => {
                prefix_start = None;
                prefix_end = None;
            }
        }
        index -= 1;
    }
    let prefix = match (prefix_start, prefix_end) {
        (Some(s), Some(e)) => text[s..e].to_owned(),
        _ => String::new(),
    };
    (prefix, index)
}

fn empty_line(line: &str) -> String {
    let (prefix, _) = line_prefix(line, line.len() as isize);
    format!("\n{}", prefix.trim())
}

fn empty_line_after_blockquote(next_line: &str, is_callout: bool, level: usize) -> String {
    let potential = empty_line(next_line);
    let next_level = count_instances(&potential, ">");
    let callout = is_callout || crate::regexes::CALLOUT.is_match(next_line);
    if (callout && level == next_level) || level < next_level {
        let cut = potential.rfind('>').unwrap_or(0);
        return potential[..cut].to_owned();
    }
    potential
}

fn single_empty_line_before(text: &str, start_of_content: isize) -> String {
    if start_of_content == 0 {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut index = start_of_content;
    let mut start_of_new = start_of_content;
    while index >= 0 {
        if !blank(bytes, index) {
            break;
        } else if at(bytes, index) == Some(b'\n') {
            start_of_new = index;
        }
        index -= 1;
    }
    if index < 0 || start_of_new == 0 {
        return substring_from(text, start_of_content + 1);
    }
    format!(
        "{}\n{}",
        &text[..start_of_new as usize],
        substring_from(text, start_of_content)
    )
}

fn single_empty_line_before_blockquote(
    text: &str,
    start_of_line: &str,
    start_of_content: isize,
    is_callout: bool,
    around_blockquotes: bool,
) -> String {
    if start_of_content == 0 {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let nesting = start_of_line.matches('>').count();
    let mut index = start_of_content;
    let mut start_of_new = start_of_content;
    let mut found_blank_line = false;
    let mut previous: Option<u8> = None;
    while index >= 0 {
        let current = at(bytes, index);
        if !blank(bytes, index) && current != Some(b'>') {
            break;
        } else if current == Some(b'>') {
            if found_blank_line {
                break;
            }
        } else if current == Some(b'\n') {
            start_of_new = index;
            if previous == Some(b'\n') {
                found_blank_line = true;
            }
        }
        index -= 1;
        previous = current;
    }
    if index < 0 || start_of_new == 0 {
        return substring_from(text, start_of_content + 1);
    }
    let starting_empty = substring(text, start_of_new, start_of_content);
    if starting_empty == "\n" || starting_empty.starts_with("\n\n") {
        return format!(
            "{}\n{}",
            substring(text, 0, start_of_new),
            substring_from(text, start_of_content)
        );
    }
    let last_newline = last_index_of_newline(text, start_of_new - 1);
    let prior_line = if last_newline == -1 {
        substring(text, 0, start_of_new)
    } else {
        substring(text, last_newline, start_of_new)
    };
    let end_of_first_line = index_of_newline(text, start_of_content + 1);
    let first_line = if end_of_first_line == -1 {
        substring_from(text, start_of_content)
    } else {
        substring(text, start_of_content, end_of_first_line).to_owned()
    };
    let empty = if around_blockquotes {
        empty_line_after_blockquote(prior_line, is_callout, nesting)
    } else if count_instances(prior_line, ">") != 0
        && !crate::regexes::CALLOUT.is_match(prior_line)
        && (crate::regexes::CODE_BLOCK_BLOCKQUOTE.is_match(prior_line)
            || crate::regexes::CODE_BLOCK_BLOCKQUOTE.is_match(&first_line))
    {
        substring(text, start_of_new, start_of_content)
            .trim_end()
            .to_owned()
    } else {
        empty_line(prior_line)
    };
    format!(
        "{}{}{}",
        substring(text, 0, start_of_new),
        empty,
        substring_from(text, start_of_content)
    )
}

fn single_empty_line_after(text: &str, end_of_content: usize) -> String {
    if end_of_content + 1 >= text.len() {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut index = end_of_content;
    let mut end_of_new = end_of_content;
    let mut first_newline = true;
    while index < text.len() {
        let current = bytes[index];
        if !is_ws(current) {
            break;
        } else if current == b'\n' {
            if first_newline {
                first_newline = false;
            } else {
                end_of_new = index;
            }
        }
        index += 1;
    }
    if index == text.len() || end_of_new + 1 == text.len() {
        return text[..end_of_content].to_owned();
    }
    format!("{}\n{}", &text[..end_of_content], &text[end_of_new..])
}

fn is_empty_blockquote_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.bytes().all(|b| b == b'>')
}

fn single_empty_line_after_blockquote(
    text: &str,
    start_of_line: &str,
    end_of_content: usize,
    is_callout: bool,
    around_blockquotes: bool,
) -> String {
    let len = text.len() as isize;
    let mut end_of_content = end_of_content as isize;
    if end_of_content >= len - 1 {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    // A `>` at the end means the element took in part of an empty blockquote
    // line after it (obsidian-linter issue 1367).
    if at(bytes, end_of_content) == Some(b'>') && !around_blockquotes {
        let end_of_previous = end_of_content - 1;
        if at(bytes, end_of_previous) == Some(b'\n') {
            let start_of_previous = last_index_of_newline(text, end_of_previous - 1) + 1;
            if is_empty_blockquote_line(substring(text, start_of_previous, end_of_previous)) {
                end_of_content = (start_of_previous - 1).max(0);
            }
        }
    }

    let nesting = start_of_line.matches('>').count();
    let mut index = end_of_content;
    let mut end_of_new = end_of_content;
    let mut first_newline = true;
    let mut found_blank_line = false;
    let mut previous: Option<u8> = None;
    let mut first_char = true;
    let pre_ending = at(bytes, end_of_content - 1);
    while index < len {
        let current = at(bytes, index);
        if !blank(bytes, index) && current != Some(b'>') {
            break;
        } else if current == Some(b'>') {
            if found_blank_line {
                break;
            }
        } else if current == Some(b'\n') {
            if first_newline {
                first_newline = false;
            } else {
                end_of_new = index;
            }
            if previous == Some(b'\n') {
                found_blank_line = true;
            }
        }
        index += 1;
        previous = current;
        if first_char && current == Some(b'\n') && around_blockquotes && pre_ending == Some(b'\n') {
            end_of_new = index;
            break;
        }
        first_char = false;
    }
    if index == len || end_of_new == len - 1 {
        return substring(text, 0, end_of_content).to_owned();
    }
    let ending_empty = substring(text, end_of_content, end_of_new);
    if ending_empty == "\n" || ending_empty.ends_with("\n\n") {
        return format!(
            "{}\n{}",
            substring(text, 0, end_of_content),
            substring_from(text, end_of_new)
        );
    }
    let second_newline = index_of_newline(text, end_of_new + 1);
    let next_line = if second_newline == -1 {
        substring_from(text, end_of_new)
    } else {
        substring(text, end_of_new + 1, second_newline).to_owned()
    };
    let end_of_last_line = last_index_of_newline(text, end_of_content - 1);
    let last_line = if end_of_last_line == -1 {
        substring(text, 0, end_of_new)
    } else {
        substring(text, end_of_last_line + 1, end_of_content)
    };
    let with_indicators = if next_line.contains('>') {
        start_of_line
    } else {
        next_line.as_str()
    };
    let empty = if around_blockquotes {
        empty_line_after_blockquote(with_indicators, is_callout, nesting)
    } else if (crate::regexes::CODE_BLOCK_BLOCKQUOTE.is_match(&next_line)
        || crate::regexes::CODE_BLOCK_BLOCKQUOTE.is_match(last_line))
        && end_of_content != end_of_new
    {
        substring(text, end_of_content, end_of_new)
            .trim_end()
            .to_owned()
    } else {
        empty_line(with_indicators)
    };
    format!(
        "{}{}{}",
        substring(text, 0, end_of_content),
        empty,
        substring_from(text, end_of_new)
    )
}

fn start_of_first_non_empty_line(text: &str, current_start: isize, level: usize) -> isize {
    let bytes = text.as_bytes();
    let mut actual = current_start;
    let mut index = current_start + 1;
    let mut found = false;
    let mut current_level = 0;
    while (index as usize) < text.len() {
        let current = bytes[index as usize];
        if !is_ws(current) && current != b'>' {
            found = true;
            break;
        } else if current == b'\n' {
            if current_level != level {
                break;
            }
            current_level = 0;
            actual = index;
        } else if current == b'>' {
            current_level += 1;
        }
        index += 1;
    }
    if found { actual } else { current_start }
}

fn end_of_last_non_empty_line(text: &str, current_end: usize, level: usize) -> usize {
    let bytes = text.as_bytes();
    let mut actual = current_end;
    let mut index = current_end as isize - 1;
    let mut found = false;
    let mut current_level = 0;
    while index >= 0 {
        let current = bytes[index as usize];
        if !is_ws(current) && current != b'>' {
            found = true;
            break;
        } else if current == b'\n' {
            if current_level != level {
                break;
            }
            current_level = 0;
            actual = index as usize;
        } else if current == b'>' {
            current_level += 1;
        }
        index -= 1;
    }
    if found { actual } else { current_end }
}

/// Makes sure the content between `start` and `end` has an empty line before
/// and after it, unless it starts or ends the text. Inside blockquotes the
/// empty line keeps the right `>` markers. obsidian-linter's
/// `makeSureContentHasEmptyLinesAddedBeforeAndAfter`.
pub fn ensure_empty_lines_around(
    text: &str,
    start: usize,
    end: usize,
    around_blockquotes: bool,
) -> String {
    let (start_of_line, start_of_line_index) = line_prefix(text, start as isize);
    if !start_of_line.trim().is_empty() {
        let is_callout = crate::regexes::CALLOUT.is_match(&text[start..end.min(text.len())]);
        let level = count_instances(&start_of_line, ">");
        let new_end = end_of_last_non_empty_line(text, end, level);
        let new_text = single_empty_line_after_blockquote(
            text,
            &start_of_line,
            new_end,
            is_callout,
            around_blockquotes,
        );
        let start_index = start_of_first_non_empty_line(&new_text, start_of_line_index, level);
        return single_empty_line_before_blockquote(
            &new_text,
            &start_of_line,
            start_index,
            is_callout,
            around_blockquotes,
        );
    }
    let new_text = single_empty_line_after(text, end);
    single_empty_line_before(&new_text, start_of_line_index)
}

/// JavaScript's `text.substring(i)`.
pub fn substring_from(text: &str, i: isize) -> String {
    let i = i.clamp(0, text.len() as isize) as usize;
    text[floor_boundary(text, i)..].to_owned()
}

/// JavaScript's `text.substring(a, b)`: clamped, and swapped if reversed.
pub fn substring(text: &str, a: isize, b: isize) -> &str {
    let len = text.len() as isize;
    let (a, b) = (a.clamp(0, len) as usize, b.clamp(0, len) as usize);
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    &text[floor_boundary(text, a)..floor_boundary(text, b)]
}

/// JavaScript's `text.lastIndexOf('\n', from)`.
pub fn last_index_of_newline(text: &str, from: isize) -> isize {
    if text.is_empty() {
        return -1;
    }
    let from = from.clamp(0, text.len() as isize - 1) as usize;
    text[..=from].rfind('\n').map_or(-1, |i| i as isize)
}

/// JavaScript's `text.indexOf('\n', from)`.
pub fn index_of_newline(text: &str, from: isize) -> isize {
    let from = from.clamp(0, text.len() as isize) as usize;
    text[from..].find('\n').map_or(-1, |i| (from + i) as isize)
}

/// Applies `edits` to a copy of `text`.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    igneous_markdown::edit::apply(text, edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(before: &str, after: &str) {
        let edits = edits_between(before, after);
        assert_eq!(apply(before, &edits), after, "{before:?} → {after:?}");
    }

    #[test]
    fn diffs_round_trip() {
        check("", "abc");
        check("abc", "");
        check("hello world", "hello brave world");
        check("a\n\n\nb", "a\n\nb");
        check("ünïcödé text", "ünïcode text!");
        check("# Title\n\ntext  \n", "# Title\n\ntext\n");
        assert!(edits_between("same", "same").is_empty());
        let edits = edits_between("one two three", "one 2 three");
        assert_eq!(edits, vec![TextEdit::replace(4..7, "2")]);
    }

    #[test]
    fn line_prefixes() {
        assert_eq!(line_prefix("a\n> > b", 5), ("> > ".into(), 1));
        assert_eq!(line_prefix("text", 3), (String::new(), -1));
        assert_eq!(line_prefix("  x", 1), ("  ".into(), -1));
    }

    #[test]
    fn empty_lines_around_content() {
        let text = "before\n```\ncode\n```\nafter";
        assert_eq!(
            ensure_empty_lines_around(text, 7, 19, false),
            "before\n\n```\ncode\n```\n\nafter"
        );
        let text = "a\n\n\n\n---\n\n\n\nb";
        assert_eq!(
            ensure_empty_lines_around(text, 5, 8, false),
            "a\n\n---\n\nb"
        );
    }
}
