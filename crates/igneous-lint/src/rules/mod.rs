//! The rules, one module each, named after obsidian-linter's rule IDs.

use regex::{Captures, Regex};

use crate::Span;
use crate::protect::RangeSet;
use crate::rule::Rule;

mod add_blank_line_after_yaml;
mod blockquote_style;
mod compact_yaml;
mod consecutive_blank_lines;
mod dedupe_yaml_array_values;
mod emphasis_style;
mod empty_line_around_blockquotes;
mod empty_line_around_code_fences;
mod empty_line_around_horizontal_rules;
mod empty_line_around_math_blocks;
mod empty_line_around_tables;
mod escape_yaml_special_characters;
mod force_yaml_escape;
mod format_tags_in_yaml;
mod format_yaml_array;
mod header_increment;
mod heading_blank_lines;
mod headings_start_line;
mod line_break_at_document_end;
mod move_math_block_indicators_to_their_own_line;
mod ordered_list_style;
mod remove_consecutive_list_markers;
mod remove_empty_list_markers;
mod remove_hyphenated_line_breaks;
mod remove_multiple_spaces;
mod remove_trailing_punctuation_in_heading;
mod space_after_list_markers;
mod strong_style;
mod trailing_spaces;
mod unordered_list_style;
mod yaml_timestamp;

static ALL: &[&dyn Rule] = &[
    &add_blank_line_after_yaml::Rule,
    &compact_yaml::Rule,
    &dedupe_yaml_array_values::Rule,
    &escape_yaml_special_characters::Rule,
    &force_yaml_escape::Rule,
    &format_tags_in_yaml::Rule,
    &format_yaml_array::Rule,
    &yaml_timestamp::Rule,
    &header_increment::Rule,
    &headings_start_line::Rule,
    &remove_trailing_punctuation_in_heading::Rule,
    &blockquote_style::Rule,
    &emphasis_style::Rule,
    &ordered_list_style::Rule,
    &remove_consecutive_list_markers::Rule,
    &remove_empty_list_markers::Rule,
    &remove_hyphenated_line_breaks::Rule,
    &remove_multiple_spaces::Rule,
    &strong_style::Rule,
    &unordered_list_style::Rule,
    &consecutive_blank_lines::Rule,
    &empty_line_around_blockquotes::Rule,
    &empty_line_around_code_fences::Rule,
    &empty_line_around_horizontal_rules::Rule,
    &empty_line_around_math_blocks::Rule,
    &empty_line_around_tables::Rule,
    &heading_blank_lines::Rule,
    &line_break_at_document_end::Rule,
    &move_math_block_indicators_to_their_own_line::Rule,
    &space_after_list_markers::Rule,
    &trailing_spaces::Rule,
];

/// Every rule Igneous has.
pub fn all() -> &'static [&'static dyn Rule] {
    ALL
}

/// The rule with this ID.
pub fn get(id: &str) -> Option<&'static dyn Rule> {
    ALL.iter().copied().find(|r| r.id() == id)
}

/// Regex matches turned into replacements, skipping matches whose guard
/// range is protected. obsidian-linter's `collectUnprotectedRegexReplacements`;
/// `offset` places a match in `haystack` within the note.
pub(crate) fn unprotected_matches(
    haystack: &str,
    offset: usize,
    regex: &Regex,
    protected: &RangeSet,
    mut edit: impl FnMut(&Captures, usize) -> (Span, String),
    mut guard: impl FnMut(&Captures, usize) -> Span,
) -> Vec<(Span, String)> {
    let mut out = Vec::new();
    for caps in regex.captures_iter(haystack) {
        let start = offset + caps.get(0).unwrap().start();
        if !protected.is_protected(&guard(&caps, start)) {
            out.push(edit(&caps, start));
        }
    }
    out
}

/// The length of a capture group, or 0 if it didn't take part.
pub(crate) fn group_len(caps: &Captures, i: usize) -> usize {
    caps.get(i).map_or(0, |m| m.len())
}
