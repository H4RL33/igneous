//! Working on frontmatter as text, the way obsidian-linter's YAML rules do:
//! finding a key's value in the source, replacing it, and formatting arrays
//! and quoted strings. Ported from obsidian-linter's `utils/yaml.ts`.
//!
//! The functions taking `yaml` work on the whole block, fences included.

use igneous_markdown::frontmatter::{self, Value};

use crate::Span;
use crate::syntax;

pub const TAG_KEYS: [&str; 2] = ["tag", "tags"];
pub const ALIAS_KEYS: [&str; 2] = ["alias", "aliases"];

/// Replaces the frontmatter block with what `f` makes of it. Returns `None`
/// if there's no frontmatter.
pub fn format_yaml(text: &str, f: impl FnOnce(&str) -> String) -> Option<String> {
    let (range, _) = syntax::yaml(text)?;
    let new = f(&text[range.clone()]);
    Some(format!(
        "{}{}{}",
        &text[..range.start],
        new,
        &text[range.end..]
    ))
}

/// The top-level keys of a frontmatter block, with their parsed values.
/// `None` if the YAML isn't valid.
pub fn load(yaml: &str) -> Option<Vec<(String, Value)>> {
    // obsidian-linter replaces tabs starting a line so YAML with tab
    // indentation still loads.
    let normalised = tab_indent_to_spaces(yaml);
    let block = if normalised.ends_with('\n') {
        normalised
    } else {
        format!("{normalised}\n")
    };
    let fm = frontmatter::parse(&block)?;
    if fm.error.is_some() {
        return None;
    }
    Some(fm.entries.into_iter().map(|e| (e.key, e.value)).collect())
}

fn tab_indent_to_spaces(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    let mut rest = yaml;
    while let Some(i) = rest.find("\n\t") {
        out.push_str(&rest[..i]);
        out.push_str("\n  ");
        rest = rest[i + 1..].trim_start_matches('\t');
    }
    out.push_str(rest);
    out
}

/// Where a key and its value are in a frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    line_start: usize,
    key: Span,
    /// From after the colon (and spaces after it) to the end of the value.
    value: Span,
    /// Through the newline ending the value.
    end: usize,
}

/// Finds `key`, at the top level or (as obsidian-linter does) inside nested
/// mappings, whichever comes first.
fn find_section(yaml: &str, key: &str) -> Option<Section> {
    let wanted = unquote_key(key.trim());
    let lines = lines_of(yaml);
    // (indent, is a mapping key) for each enclosing line.
    let mut ancestors: Vec<(usize, bool)> = Vec::new();
    let mut block_scalar: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        let content = &yaml[line.clone()];
        if (index == 0 || index + 1 == lines.len()) && content.trim_end() == "---" {
            continue;
        }
        let trimmed = content.trim_start_matches([' ', '\t']);
        let indent = content.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(scalar_indent) = block_scalar {
            if indent > scalar_indent {
                continue;
            }
            block_scalar = None;
        }
        while ancestors.last().is_some_and(|(i, _)| *i >= indent) {
            ancestors.pop();
        }
        if trimmed.starts_with("- ") || trimmed == "-" {
            ancestors.push((indent, false));
            continue;
        }
        let Some(colon) = key_colon(trimmed) else {
            continue;
        };
        let reachable = ancestors.iter().all(|(_, is_key)| *is_key);
        let key_start = line.start + indent;
        let key_text = trimmed[..colon].trim_end();
        let after = trimmed[colon + 1..].trim_start_matches([' ', '\t']);
        if is_block_scalar_indicator(after) {
            block_scalar = Some(indent);
        }
        ancestors.push((indent, true));
        if !reachable || unquote_key(key_text) != wanted {
            continue;
        }
        let colon_at = key_start + colon;
        let value_start = line.start + content.len() - after.len();
        let mut value_end = trim_end_ws(yaml, value_start..line.end).end;
        // Following lines belong to the value while they're indented more
        // than the key, or are list items at the key's own indentation.
        let inline = !after.trim().is_empty() && !is_block_scalar_indicator(after);
        for next in &lines[index + 1..] {
            let next_content = &yaml[next.clone()];
            if next.end == yaml.len() && next_content.trim_end() == "---" {
                break;
            }
            let next_trimmed = next_content.trim_start_matches([' ', '\t']);
            if next_trimmed.is_empty() {
                continue;
            }
            let next_indent = next_content.len() - next_trimmed.len();
            let list_item = next_trimmed.starts_with("- ") || next_trimmed == "-";
            if next_indent > indent || (!inline && next_indent == indent && list_item) {
                value_end = trim_end_ws(yaml, next.clone()).end;
            } else {
                break;
            }
        }
        let value_end = value_end.max(value_start);
        let mut end = value_end;
        while matches!(yaml.as_bytes().get(end), Some(b' ' | b'\t')) {
            end += 1;
        }
        if yaml.as_bytes().get(end) == Some(&b'\n') {
            end += 1;
        }
        return Some(Section {
            line_start: line.start,
            key: key_start..colon_at,
            value: value_start..value_end,
            end,
        });
    }
    None
}

fn lines_of(text: &str) -> Vec<Span> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            lines.push(start..i);
            start = i + 1;
        }
    }
    lines.push(start..text.len());
    lines
}

fn trim_end_ws(text: &str, range: Span) -> Span {
    let trimmed = text[range.clone()].trim_end_matches([' ', '\t', '\r']);
    range.start..range.start + trimmed.len()
}

/// The colon ending a mapping key at the start of `line`, skipping colons
/// inside a quoted key.
fn key_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => {
                if q == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                    i += 1;
                } else if q == b'"' && i > 0 && bytes[i - 1] == b'\\' {
                } else {
                    quote = None;
                }
            }
            Some(_) => {}
            None if (b == b'"' || b == b'\'') && i == 0 => quote = Some(b),
            None if b == b':' => {
                let next = bytes.get(i + 1);
                if next.is_none_or(|n| *n == b' ' || *n == b'\t') {
                    return (i > 0).then_some(i);
                }
            }
            None if b == b'#' && i > 0 && bytes[i - 1] == b' ' => return None,
            None if i == 0 && matches!(b, b'[' | b'{' | b'&' | b'*' | b'!' | b'|' | b'>') => {
                return None;
            }
            None => {}
        }
        i += 1;
    }
    None
}

fn unquote_key(key: &str) -> &str {
    if key.len() >= 2
        && ((key.starts_with('"') && key.ends_with('"'))
            || (key.starts_with('\'') && key.ends_with('\'')))
    {
        &key[1..key.len() - 1]
    } else {
        key
    }
}

pub fn is_block_scalar_indicator(value: &str) -> bool {
    let value = value.trim_end();
    let mut chars = value.chars().peekable();
    if !matches!(chars.next(), Some('|' | '>')) {
        return false;
    }
    if chars.peek().is_some_and(|c| ('1'..='9').contains(c)) {
        chars.next();
    }
    if chars.peek().is_some_and(|c| *c == '+' || *c == '-') {
        chars.next();
    }
    let rest: String = chars.collect();
    rest.is_empty() || {
        let trimmed = rest.trim_start_matches([' ', '\t']);
        trimmed.len() < rest.len() && trimmed.starts_with('#')
    }
}

/// The source of `key`'s value, from after the colon. A block value starts
/// with the newline before it.
pub fn section_value(yaml: &str, key: &str) -> Option<String> {
    find_section(yaml, key).map(|s| yaml[s.value].to_owned())
}

/// Replaces `key`'s value with `raw` (which includes any leading space or
/// newline), keeping the key as written. A missing key is appended.
pub fn set_section(yaml: &str, key: &str, raw: &str) -> String {
    match find_section(yaml, key) {
        Some(section) => {
            let indent = &yaml[section.line_start..section.key.start];
            let original_key = yaml[section.key.clone()].trim_end();
            format!(
                "{}{indent}{original_key}:{raw}\n{}",
                &yaml[..section.line_start],
                &yaml[section.end..]
            )
        }
        None => format!("{yaml}{key}:{raw}\n"),
    }
}

pub fn is_value_escaped_already(value: &str) -> bool {
    value.len() > 1
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
}

/// Quotes `value` if it has a colon followed by a space or a quote in it (or
/// always, with `force`), unless it's already quoted or has both kinds of
/// quote.
fn basic_escape(value: &str, quote: char, force: bool) -> String {
    if is_value_escaped_already(value) {
        return value.to_owned();
    }
    let single = value.contains('\'');
    let double = value.contains('"');
    let colon = value.contains(": ");
    if (!single && !double && !colon && !force) || (single && double) {
        return value.to_owned();
    }
    if single {
        format!("\"{value}\"")
    } else if double {
        format!("'{value}'")
    } else {
        format!("{quote}{value}{quote}")
    }
}

/// Quotes `value` when it needs it (or with `force`). Unless validation is
/// skipped, the result is checked to read back as `value`, falling back to a
/// properly escaped string. obsidian-linter's
/// `escapeStringIfNecessaryAndPossible`.
pub fn escape(value: &str, quote: char, force: bool, skip_validation: bool) -> String {
    let basic = basic_escape(value, quote, force);
    if skip_validation || parse_string(&basic).as_deref() == Some(value) {
        return basic;
    }
    let other = if quote == '"' { '\'' } else { '"' };
    let with_default = stringify(value, quote, force);
    let with_other = stringify(value, other, force);
    if with_other == value || with_other.len() < with_default.len() {
        with_other
    } else {
        with_default
    }
}

/// Reads a single YAML scalar, if it's a string.
fn parse_string(scalar: &str) -> Option<String> {
    if scalar.contains('\n') {
        return None;
    }
    let block = format!("---\nk: {scalar}\n---\n");
    let fm = frontmatter::parse(&block)?;
    if fm.error.is_some() {
        return None;
    }
    match fm.entries.into_iter().next()?.value {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// Writes `value` as a YAML string, plain if that's safe and not forced.
fn stringify(value: &str, quote: char, force: bool) -> String {
    if !force && plain_is_safe(value) {
        return value.to_owned();
    }
    let needs_double = value.chars().any(|c| c.is_control() && c != '\t');
    if quote == '\'' && !needs_double {
        return format!("'{}'", value.replace('\'', "''"));
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn plain_is_safe(value: &str) -> bool {
    if value.is_empty() || value.trim() != value || value.contains('\n') {
        return false;
    }
    let first = value.chars().next().unwrap();
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        let second = value.chars().nth(1);
        if !(matches!(first, '-' | '?' | ':') && second.is_some_and(|c| !c.is_whitespace())) {
            return false;
        }
    }
    if value.contains(": ") || value.contains(" #") || value.ends_with(':') {
        return false;
    }
    parse_string(value).as_deref() == Some(value)
}

/// JavaScript's `!isNaN(s) && !isNaN(parseFloat(s))`.
pub fn is_numeric(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    let unsigned = t.trim_start_matches(['+', '-']);
    if unsigned == "Infinity" {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0o") || lower.starts_with("0b") {
        let radix = match &lower[1..2] {
            "x" => 16,
            "o" => 8,
            _ => 2,
        };
        return t.len() > 2 && u64::from_str_radix(&t[2..], radix).is_ok();
    }
    if unsigned
        .bytes()
        .any(|b| b.is_ascii_alphabetic() && b != b'e' && b != b'E')
    {
        return false;
    }
    t.parse::<f64>().is_ok()
}

/// Array formats from obsidian-linter's common styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayFormat {
    SingleLine,
    MultiLine,
    SingleStringToSingleLine,
    SingleStringToMultiLine,
    SingleStringCommaDelimited,
    SingleStringSpaceDelimited,
    SingleLineSpaceDelimited,
}

impl ArrayFormat {
    pub fn parse(name: &str) -> ArrayFormat {
        match name {
            "multi-line" => ArrayFormat::MultiLine,
            "single string to single-line" => ArrayFormat::SingleStringToSingleLine,
            "single string to multi-line" => ArrayFormat::SingleStringToMultiLine,
            "single string comma delimited" => ArrayFormat::SingleStringCommaDelimited,
            "single string space delimited" => ArrayFormat::SingleStringSpaceDelimited,
            "single-line space delimited" => ArrayFormat::SingleLineSpaceDelimited,
            _ => ArrayFormat::SingleLine,
        }
    }
}

/// A key's value split into array items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Items {
    /// Not an array: a single string.
    One(String),
    Many(Vec<String>),
}

impl Items {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Items::One(s) => vec![s],
            Items::Many(v) => v,
        }
    }
}

/// Splits a single-line (`[a, b]`) or multi-line (`- a`) array into its
/// items; anything else is returned whole. `None` for an empty value.
/// obsidian-linter's `splitValueIfSingleOrMultilineArray`.
pub fn split_array(value: Option<&str>) -> Option<Items> {
    let value = value?.trim_end();
    if value.is_empty() {
        return None;
    }
    if let Some(inner) = value.strip_prefix('[') {
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        if inner.is_empty() {
            return None;
        }
        let items = split_delimited(inner, ',')
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        return Some(Items::Many(items));
    }
    if value.contains('\n') {
        static ITEM: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"[ \t]*\n[ \t]*-[ \t]*").unwrap());
        let items: Vec<String> = ITEM
            .split(value)
            .skip(1)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        return (!items.is_empty()).then_some(Items::Many(items));
    }
    Some(Items::One(value.to_owned()))
}

/// Splits on `delimiter`, keeping quoted parts together and trimming each
/// item. obsidian-linter's `convertYAMLStringToArray`.
pub fn split_delimited(value: &str, delimiter: char) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = value.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == delimiter {
            items.push(current.trim().to_owned());
            current.clear();
        } else if c == '"' || c == '\'' {
            match value[i + 1..].find(c) {
                Some(j) => {
                    current.push_str(&value[i..i + 1 + j + 1]);
                    // Skip to the closing quote.
                    for (k, _) in chars.by_ref() {
                        if k == i + 1 + j {
                            break;
                        }
                    }
                }
                None => current.push(c),
            }
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_owned());
    }
    items
}

/// Tag values: an array as is, otherwise split on commas or spaces.
pub fn tag_items(items: Option<Items>) -> Vec<String> {
    match items {
        None => Vec::new(),
        Some(Items::Many(v)) => v.into_iter().map(|s| s.trim().to_owned()).collect(),
        Some(Items::One(s)) => {
            let delimiter = if s.contains(',') { ',' } else { ' ' };
            split_delimited(&s, delimiter)
                .into_iter()
                .map(|s| s.trim().to_owned())
                .collect()
        }
    }
}

/// Alias values: an array as is, otherwise split on commas.
pub fn alias_items(items: Option<Items>) -> Vec<String> {
    match items {
        None => Vec::new(),
        Some(Items::Many(v)) => v,
        Some(Items::One(s)) => split_delimited(&s, ','),
    }
}

/// Writes array items in `format`, as the text following the key's colon.
/// obsidian-linter's `formatYamlArrayValue`.
pub fn format_array(
    mut values: Vec<String>,
    format: ArrayFormat,
    quote: char,
    unescape_multi_line: bool,
    escape_numbers: bool,
) -> String {
    use ArrayFormat::*;
    if values.is_empty() {
        return match format {
            SingleLine | SingleLineSpaceDelimited | MultiLine => " []".into(),
            _ => " ".into(),
        };
    }
    let unescape = unescape_multi_line
        && (format == MultiLine || (format == SingleStringToMultiLine && values.len() > 1));
    if escape_numbers || unescape {
        for value in values.iter_mut() {
            let escaped = is_value_escaped_already(value);
            let inner = if escaped {
                value[1..value.len() - 1].to_owned()
            } else {
                value.clone()
            };
            let must_escape = escape_numbers && is_numeric(&inner);
            if escaped && must_escape {
                continue;
            }
            if must_escape || (escaped && unescape) {
                *value = escape(&inner, quote, must_escape, false);
            }
        }
    }
    let escape_commas = |values: &mut Vec<String>| {
        for value in values.iter_mut() {
            if value.contains(',') && !is_value_escaped_already(value) {
                *value = escape(value, quote, true, false);
            }
        }
    };
    let single_line = |values: &[String]| format!("[{}]", values.join(", "));
    match format {
        SingleStringToSingleLine | SingleLine => {
            if format == SingleStringToSingleLine && values.len() == 1 {
                return format!(" {}", values[0]);
            }
            escape_commas(&mut values);
            format!(" {}", single_line(&values))
        }
        SingleStringToMultiLine | MultiLine => {
            if format == SingleStringToMultiLine && values.len() == 1 {
                return format!(" {}", values[0]);
            }
            format!("\n  - {}", values.join("\n  - "))
        }
        SingleStringSpaceDelimited => format!(" {}", values.join(" ")),
        SingleStringCommaDelimited => {
            escape_commas(&mut values);
            format!(" {}", values.join(", "))
        }
        SingleLineSpaceDelimited => {
            if values.len() == 1 {
                return format!(" {}", values[0]);
            }
            format!(" {}", single_line(&values).replace(", ", " "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "---\ntitle: Note  # a comment\ntags:\n  - one\n  - two\nnested:\n  inner: x\nlist:\n- a\n- b\nempty:\n---";

    #[test]
    fn finds_values() {
        assert_eq!(
            section_value(YAML, "title").as_deref(),
            Some("Note  # a comment")
        );
        assert_eq!(
            section_value(YAML, "tags").as_deref(),
            Some("\n  - one\n  - two")
        );
        assert_eq!(section_value(YAML, "inner").as_deref(), Some("x"));
        assert_eq!(section_value(YAML, "list").as_deref(), Some("\n- a\n- b"));
        assert_eq!(section_value(YAML, "empty").as_deref(), Some(""));
        assert_eq!(section_value(YAML, "missing"), None);
    }

    #[test]
    fn sets_values() {
        let out = set_section(YAML, "tags", " [one, two]");
        assert!(out.contains("tags: [one, two]\nnested:"), "{out}");
        let out = set_section("---\n\"quoted\": x\n---", "quoted", " y");
        assert_eq!(out, "---\n\"quoted\": y\n---");
    }

    #[test]
    fn arrays() {
        assert_eq!(
            split_array(Some("[a, \"b, c\", d]")),
            Some(Items::Many(vec!["a".into(), "\"b, c\"".into(), "d".into()]))
        );
        assert_eq!(
            split_array(Some("\n  - a\n  - b")),
            Some(Items::Many(vec!["a".into(), "b".into()]))
        );
        assert_eq!(split_array(Some("[]")), None);
        let values = vec!["a".to_owned(), "b, c".to_owned()];
        assert_eq!(
            format_array(values.clone(), ArrayFormat::SingleLine, '"', false, false),
            " [a, \"b, c\"]"
        );
        assert_eq!(
            format_array(values, ArrayFormat::MultiLine, '"', false, false),
            "\n  - a\n  - b, c"
        );
        assert_eq!(
            format_array(
                vec!["123".into()],
                ArrayFormat::SingleLine,
                '"',
                false,
                true
            ),
            " [\"123\"]"
        );
    }

    #[test]
    fn escaping() {
        assert_eq!(escape("a: b", '"', false, true), "\"a: b\"");
        assert_eq!(escape("it's", '"', false, true), "\"it's\"");
        assert_eq!(escape("plain", '"', false, true), "plain");
        assert_eq!(escape("false", '"', true, false), "\"false\"");
        assert!(is_numeric("12.5") && is_numeric(" 7 ") && !is_numeric("abc") && !is_numeric(""));
    }
}
