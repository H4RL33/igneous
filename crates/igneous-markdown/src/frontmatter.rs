//! YAML frontmatter ("properties") with exact source positions, so a single
//! property can be changed without rewriting the rest of the block.

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use crate::Span;
use crate::edit::TextEdit;
use crate::text::line_start;

#[derive(Debug, Clone, PartialEq)]
pub struct Frontmatter {
    /// The whole block: opening fence through the closing fence's newline.
    pub range: Span,
    /// The YAML between the fences.
    pub body: Span,
    pub entries: Vec<Entry>,
    /// Set when the YAML couldn't be read. `entries` is then empty.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub key_range: Span,
    pub value: Value,
    /// The text replaced when the value changes: from just after the colon to
    /// the end of the value. Comments after the value are outside it.
    pub value_range: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a list of strings, treating a single string as a
    /// one-item list (as Obsidian does for `tags: foo`).
    pub fn string_list(&self) -> Vec<String> {
        match self {
            Value::String(s) => vec![s.clone()],
            Value::List(items) => items
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Int(i) => Some(i.to_string()),
                    Value::Float(f) => Some(f.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("the frontmatter isn't valid YAML: {0}")]
    InvalidYaml(String),
}

/// Finds the frontmatter block: `---` on the very first line, closed by a
/// later `---` line. Returns `(range, body)`.
pub fn detect(text: &str) -> Option<(Span, Span)> {
    let first_end = text.find('\n').unwrap_or(text.len());
    if text[..first_end].trim_end() != "---" {
        return None;
    }
    let body_start = (first_end + 1).min(text.len());
    let mut pos = body_start;
    while pos < text.len() {
        let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
        if text[pos..end].trim_end() == "---" {
            let range_end = (end + 1).min(text.len());
            return Some((0..range_end, body_start..pos));
        }
        pos = end + 1;
    }
    None
}

pub fn parse(text: &str) -> Option<Frontmatter> {
    let (range, body) = detect(text)?;
    let yaml = &text[body.clone()];
    let mut fm = Frontmatter {
        range,
        body: body.clone(),
        entries: Vec::new(),
        error: None,
    };
    if yaml.trim().is_empty() {
        return Some(fm);
    }
    let docs = match MarkedYaml::load_from_str(yaml) {
        Ok(docs) => docs,
        Err(e) => {
            fm.error = Some(e.to_string());
            return Some(fm);
        }
    };
    let Some(doc) = docs.first() else {
        return Some(fm);
    };
    let offsets = CharOffsets::new(yaml, body.start);
    match &doc.data {
        YamlData::Mapping(map) => {
            for (key, value) in map {
                let key_range = offsets.span(key);
                let key_text = scalar_text(key).unwrap_or_default();
                let value_range = value_range(text, &key_range, value, &offsets, body.end);
                fm.entries.push(Entry {
                    key: key_text,
                    key_range,
                    value: convert(value),
                    value_range,
                });
            }
        }
        YamlData::Value(Scalar::Null) => {}
        _ => fm.error = Some("frontmatter must be a set of key: value pairs".into()),
    }
    Some(fm)
}

/// saphyr reports positions in characters; convert them to byte offsets in
/// the note.
struct CharOffsets {
    bytes: Vec<usize>,
}

impl CharOffsets {
    fn new(yaml: &str, base: usize) -> Self {
        let mut bytes: Vec<usize> = yaml.char_indices().map(|(b, _)| base + b).collect();
        bytes.push(base + yaml.len());
        Self { bytes }
    }

    fn at(&self, char_index: usize) -> usize {
        self.bytes[char_index.min(self.bytes.len() - 1)]
    }

    fn span(&self, node: &MarkedYaml) -> Span {
        self.at(node.span.start.index())..self.at(node.span.end.index())
    }
}

fn scalar_text(node: &MarkedYaml) -> Option<String> {
    match &node.data {
        YamlData::Value(scalar) => Some(match scalar {
            Scalar::Null => String::new(),
            Scalar::Boolean(b) => b.to_string(),
            Scalar::Integer(i) => i.to_string(),
            Scalar::FloatingPoint(f) => f.to_string(),
            Scalar::String(s) => s.to_string(),
        }),
        YamlData::Representation(s, _, _) => Some(s.to_string()),
        YamlData::Tagged(_, inner) => scalar_text(inner),
        _ => None,
    }
}

fn convert(node: &MarkedYaml) -> Value {
    match &node.data {
        // `key:` with nothing after it is null, not an empty string.
        YamlData::Value(Scalar::String(s)) if s.is_empty() && node.span.is_empty() => Value::Null,
        YamlData::Value(scalar) => match scalar {
            Scalar::Null => Value::Null,
            Scalar::Boolean(b) => Value::Bool(*b),
            Scalar::Integer(i) => Value::Int(*i),
            Scalar::FloatingPoint(f) => Value::Float(**f),
            Scalar::String(s) => Value::String(s.to_string()),
        },
        YamlData::Representation(s, _, _) => Value::String(s.to_string()),
        YamlData::Sequence(items) => Value::List(items.iter().map(convert).collect()),
        YamlData::Mapping(map) => Value::Map(
            map.iter()
                .map(|(k, v)| (scalar_text(k).unwrap_or_default(), convert(v)))
                .collect(),
        ),
        YamlData::Tagged(_, inner) => convert(inner),
        YamlData::Alias(_) | YamlData::BadValue => Value::Null,
    }
}

fn value_range(
    text: &str,
    key_range: &Span,
    value: &MarkedYaml,
    offsets: &CharOffsets,
    body_end: usize,
) -> Span {
    let bytes = text.as_bytes();
    // Find the colon after the key.
    let mut colon = key_range.end;
    while colon < body_end && matches!(bytes[colon], b' ' | b'\t') {
        colon += 1;
    }
    if bytes.get(colon) != Some(&b':') {
        return offsets.span(value);
    }
    let start = colon + 1;

    let span = offsets.span(value);
    if span.is_empty() {
        // `key:` with nothing after it: cover trailing spaces up to the end of
        // the line, unless a comment follows.
        let mut end = start;
        while end < body_end && matches!(bytes[end], b' ' | b'\t') {
            end += 1;
        }
        return if end >= body_end || bytes[end] == b'\n' {
            start..end
        } else {
            start..start
        };
    }

    let mut end = span.end;
    // Flow collections end just before their closing bracket.
    if matches!(bytes.get(span.start), Some(b'[' | b'{'))
        && matches!(bytes.get(end), Some(b']' | b'}'))
    {
        end += 1;
    }
    while end > start && matches!(bytes[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
        end -= 1;
    }
    start..end.max(start)
}

// --- editing ---------------------------------------------------------------

/// Sets `key` to `value`, adding the frontmatter block or the key if needed.
pub fn set(text: &str, key: &str, value: &Value) -> Result<Vec<TextEdit>, EditError> {
    let formatted = format_entry_value(value);
    let Some(fm) = parse(text) else {
        let block = format!("---\n{}:{formatted}\n---\n", format_key(key));
        return Ok(vec![TextEdit::insert(0, block)]);
    };
    if let Some(error) = fm.error {
        return Err(EditError::InvalidYaml(error));
    }
    Ok(match fm.entries.iter().find(|e| e.key == key) {
        Some(entry) => vec![TextEdit::replace(entry.value_range.clone(), formatted)],
        None => vec![TextEdit::insert(
            fm.body.end,
            format!("{}:{formatted}\n", format_key(key)),
        )],
    })
}

/// Removes `key` and its value, including its lines.
pub fn remove(text: &str, key: &str) -> Result<Vec<TextEdit>, EditError> {
    let Some(fm) = parse(text) else {
        return Ok(Vec::new());
    };
    if let Some(error) = fm.error {
        return Err(EditError::InvalidYaml(error));
    }
    Ok(fm
        .entries
        .iter()
        .find(|e| e.key == key)
        .map(|entry| {
            let start = line_start(text, entry.key_range.start);
            let end = text[entry.value_range.end..]
                .find('\n')
                .map_or(fm.body.end, |i| entry.value_range.end + i + 1)
                .min(fm.body.end);
            vec![TextEdit::delete(start..end)]
        })
        .unwrap_or_default())
}

/// Renames a key, keeping its value untouched.
pub fn rename_key(text: &str, from: &str, to: &str) -> Result<Vec<TextEdit>, EditError> {
    let Some(fm) = parse(text) else {
        return Ok(Vec::new());
    };
    if let Some(error) = fm.error {
        return Err(EditError::InvalidYaml(error));
    }
    Ok(fm
        .entries
        .iter()
        .find(|e| e.key == from)
        .map(|e| vec![TextEdit::replace(e.key_range.clone(), format_key(to))])
        .unwrap_or_default())
}

/// The text that follows `key:` for `value`, in Obsidian's style.
pub fn format_entry_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::List(items) if items.is_empty() => " []".into(),
        Value::List(items) => items
            .iter()
            .map(|item| format!("\n  - {}", format_scalar(item)))
            .collect(),
        Value::Map(entries) if entries.is_empty() => " {}".into(),
        Value::Map(entries) => entries
            .iter()
            .map(|(k, v)| format!("\n  {}: {}", format_key(k), format_scalar(v)))
            .collect(),
        scalar => format!(" {}", format_scalar(scalar)),
    }
}

fn format_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) if needs_quotes(s) => quote(s),
        Value::String(s) => s.clone(),
        Value::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(format_scalar)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(k, v)| format!("{}: {}", format_key(k), format_scalar(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_key(key: &str) -> String {
    if needs_quotes(key) {
        quote(key)
    } else {
        key.to_owned()
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether a string must be quoted to stay a string when read back.
fn needs_quotes(s: &str) -> bool {
    if s.is_empty() || s.trim() != s {
        return true;
    }
    if s.starts_with([
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ]) {
        return true;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') || s.contains(['\n', '\t']) {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off" | ".inf" | "-.inf" | ".nan"
    ) {
        return true;
    }
    looks_numeric(s)
}

fn looks_numeric(s: &str) -> bool {
    let t = s.trim_start_matches(['+', '-']);
    if t.starts_with("0x") || t.starts_with("0o") {
        return true;
    }
    s.parse::<f64>().is_ok() && !s.chars().any(|c| c.is_alphabetic() && c != 'e' && c != 'E')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply;

    const NOTE: &str = "---\ncreated: 2026-10-04\ntitle: Héllo wörld\ntags:\n  - rust\n  - gtk\naliases: [a, \"b c\"]\nempty:\ncount: 3 # comment\n---\n# Body\n";

    fn entry<'a>(fm: &'a Frontmatter, key: &str) -> &'a Entry {
        fm.entries.iter().find(|e| e.key == key).unwrap()
    }

    #[test]
    fn detects_fences() {
        assert_eq!(detect("---\na: 1\n---\nbody"), Some((0..13, 4..9)));
        assert_eq!(detect("---\n---\n"), Some((0..8, 4..4)));
        assert_eq!(detect("---\na: 1\n---"), Some((0..12, 4..9)));
        assert_eq!(detect("----\na: 1\n---\n"), None);
        assert_eq!(detect("text\n---\na: 1\n---\n"), None);
        assert_eq!(detect("---\nunclosed\n"), None);
    }

    #[test]
    fn parses_entries_with_byte_positions() {
        let fm = parse(NOTE).unwrap();
        assert_eq!(fm.error, None);
        let keys: Vec<_> = fm.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            ["created", "title", "tags", "aliases", "empty", "count"]
        );

        let title = entry(&fm, "title");
        assert_eq!(&NOTE[title.key_range.clone()], "title");
        assert_eq!(&NOTE[title.value_range.clone()], " Héllo wörld");
        assert_eq!(title.value, Value::String("Héllo wörld".into()));

        let tags = entry(&fm, "tags");
        assert_eq!(&NOTE[tags.value_range.clone()], "\n  - rust\n  - gtk");
        assert_eq!(tags.value.string_list(), ["rust", "gtk"]);

        let aliases = entry(&fm, "aliases");
        assert_eq!(&NOTE[aliases.value_range.clone()], " [a, \"b c\"]");

        let empty = entry(&fm, "empty");
        assert_eq!(empty.value, Value::Null);
        assert_eq!(empty.value_range.len(), 0);

        let count = entry(&fm, "count");
        assert_eq!(&NOTE[count.value_range.clone()], " 3");
        assert_eq!(count.value, Value::Int(3));

        assert_eq!(
            entry(&fm, "created").value,
            Value::String("2026-10-04".into())
        );
    }

    #[test]
    fn invalid_yaml_is_reported() {
        let fm = parse("---\na: [unclosed\n---\n").unwrap();
        assert!(fm.error.is_some());
        assert!(fm.entries.is_empty());
        assert!(set("---\na: [unclosed\n---\n", "b", &Value::Int(1)).is_err());

        let fm = parse("---\n- just\n- a list\n---\n").unwrap();
        assert!(fm.error.is_some());
    }

    #[test]
    fn set_changes_only_that_value() {
        let edits = set(NOTE, "title", &Value::String("New: title".into())).unwrap();
        let out = apply(NOTE, &edits);
        assert_eq!(
            out,
            NOTE.replace("title: Héllo wörld", "title: \"New: title\"")
        );

        let edits = set(NOTE, "count", &Value::Int(4)).unwrap();
        assert!(apply(NOTE, &edits).contains("count: 4 # comment\n"));

        let edits = set(NOTE, "empty", &Value::Bool(true)).unwrap();
        assert!(apply(NOTE, &edits).contains("\nempty: true\n"));

        let list = Value::List(vec![
            Value::String("x".into()),
            Value::String("[[Link]]".into()),
        ]);
        let edits = set(NOTE, "aliases", &list).unwrap();
        assert!(apply(NOTE, &edits).contains("aliases:\n  - x\n  - \"[[Link]]\"\nempty:"));

        let edits = set(NOTE, "tags", &Value::List(vec![])).unwrap();
        assert!(apply(NOTE, &edits).contains("tags: []\naliases"));
    }

    #[test]
    fn set_adds_keys_and_blocks() {
        let out = apply(
            NOTE,
            &set(NOTE, "status", &Value::String("draft".into())).unwrap(),
        );
        assert!(out.ends_with("count: 3 # comment\nstatus: draft\n---\n# Body\n"));

        let out = apply(
            "# Body\n",
            &set("# Body\n", "up", &Value::String("[[Home]]".into())).unwrap(),
        );
        assert_eq!(out, "---\nup: \"[[Home]]\"\n---\n# Body\n");

        let out = apply(
            "---\n---\nx",
            &set("---\n---\nx", "a", &Value::Int(1)).unwrap(),
        );
        assert_eq!(out, "---\na: 1\n---\nx");
    }

    #[test]
    fn remove_and_rename() {
        let out = apply(NOTE, &remove(NOTE, "tags").unwrap());
        assert!(out.contains("title: Héllo wörld\naliases:"), "{out}");
        let out = apply(NOTE, &remove(NOTE, "count").unwrap());
        assert!(out.ends_with("empty:\n---\n# Body\n"), "{out}");
        let out = apply(NOTE, &rename_key(NOTE, "created", "date created").unwrap());
        assert!(out.starts_with("---\ndate created: 2026-10-04\n"));
    }

    #[test]
    fn quoting_rules() {
        for s in [
            "", " x", "true", "No", "3", "-1.5", "0x1F", "[[Link]]", "#tag", "a: b", "x #y",
        ] {
            assert!(needs_quotes(s), "{s:?} should be quoted");
        }
        for s in ["hello", "2026-10-04", "a-b", "C++", "x:y", "e"] {
            assert!(!needs_quotes(s), "{s:?} should not be quoted");
        }
    }

    #[test]
    fn edits_round_trip_through_parse() {
        let values = [
            Value::String("He said \"hi\"".into()),
            Value::Float(2.5),
            Value::List(vec![Value::String("a b".into()), Value::Int(2)]),
            Value::Null,
        ];
        for value in values {
            let out = apply(NOTE, &set(NOTE, "title", &value).unwrap());
            let fm = parse(&out).unwrap();
            assert_eq!(fm.error, None, "{out}");
            assert_eq!(entry(&fm, "title").value, value, "{out}");
        }
    }
}
