//! Matching notes against a search.

use igneous_markdown::{Document, NodeKind, Span, Value};
use regex::{Regex, RegexBuilder};

use super::{Comparison, Operator, ParseError, Query, SearchOptions, parse};
use crate::NoteData;

/// A compiled search.
#[derive(Debug, Clone)]
pub struct Matcher {
    root: Node,
    reaches_files: bool,
}

/// Where a note matched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hit {
    /// Byte ranges in the note's text, sorted and merged.
    pub content: Vec<Span>,
    /// Byte ranges in the note's path, sorted and merged.
    pub path: Vec<Span>,
}

impl Hit {
    fn extend(&mut self, other: Hit) {
        self.content.extend(other.content);
        self.path.extend(other.path);
    }

    fn normalise(&mut self) {
        merge(&mut self.content);
        merge(&mut self.path);
    }
}

#[derive(Debug, Clone)]
enum Node {
    All,
    Text {
        re: Regex,
        target: Target,
    },
    Tag(TagPattern),
    Compare(Comparison, String),
    Null,
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Unit(Unit, Box<Node>),
    Property {
        /// Lower-cased.
        key: String,
        value: Option<Box<Node>>,
    },
}

/// What a word is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The note's text and, at the top level, its path.
    Any,
    Content,
    Name,
    Path,
    /// Tag names (`tag:`).
    Tag,
    /// A property's value.
    Value,
}

#[derive(Debug, Clone)]
enum TagPattern {
    /// Lower-cased, without `#`. Matches the tag and tags nested under it.
    Name(String),
    Regex(Regex),
}

impl TagPattern {
    fn matches(&self, tag: &str) -> bool {
        match self {
            Self::Name(name) => {
                let tag = tag.to_lowercase();
                tag == *name
                    || tag
                        .strip_prefix(name.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            }
            Self::Regex(re) => re.is_match(tag),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Line,
    Block,
    Section,
    Task(Option<bool>),
}

/// The part of a note a node is being matched in.
#[derive(Debug, Clone)]
struct Scope {
    range: Span,
    /// The whole file, where words also match the path.
    file: bool,
}

impl Matcher {
    pub fn new(query: &Query, options: SearchOptions) -> Result<Self, ParseError> {
        Ok(Self {
            root: compile(query, Target::Any, options.match_case)?,
            reaches_files: super::plan(query).reaches_files,
        })
    }

    /// Parses and compiles a search.
    pub fn parse(search: &str, options: SearchOptions) -> Result<Self, ParseError> {
        Self::new(&parse(search)?, options)
    }

    /// Where `note` matches, or `None` if it doesn't.
    pub fn matches(&self, note: &NoteData) -> Option<Hit> {
        if !note.is_note() && !self.reaches_files {
            return None;
        }
        let scope = Scope {
            range: 0..note.text.len(),
            file: true,
        };
        let mut hit = eval(&self.root, note, &scope)?;
        hit.normalise();
        Some(hit)
    }
}

fn compile(query: &Query, target: Target, match_case: bool) -> Result<Node, ParseError> {
    let literal = |text: &str| {
        RegexBuilder::new(&regex::escape(text))
            .case_insensitive(!match_case)
            .build()
            .expect("escaped text is a valid regex")
    };
    Ok(match query {
        Query::Empty => Node::All,
        Query::Word(text) | Query::Phrase(text) => match target {
            Target::Tag => Node::Tag(TagPattern::Name(
                text.trim_start_matches('#').to_lowercase(),
            )),
            _ => Node::Text {
                re: literal(text),
                target,
            },
        },
        Query::Regex(pattern) => {
            let re = RegexBuilder::new(pattern)
                .case_insensitive(!match_case)
                .build()
                .map_err(|e| ParseError {
                    message: format!("invalid regular expression /{pattern}/: {e}"),
                    offset: 0,
                })?;
            match target {
                Target::Tag => Node::Tag(TagPattern::Regex(re)),
                _ => Node::Text { re, target },
            }
        }
        Query::And(items) => Node::And(
            items
                .iter()
                .map(|q| compile(q, target, match_case))
                .collect::<Result<_, _>>()?,
        ),
        Query::Or(items) => Node::Or(
            items
                .iter()
                .map(|q| compile(q, target, match_case))
                .collect::<Result<_, _>>()?,
        ),
        Query::Not(inner) => Node::Not(Box::new(compile(inner, target, match_case)?)),
        Query::Op(op, inner) => {
            let within = |unit, inner: &Query| -> Result<Node, ParseError> {
                Ok(Node::Unit(
                    unit,
                    Box::new(compile(inner, Target::Content, match_case)?),
                ))
            };
            match op {
                Operator::File => compile(inner, Target::Name, match_case)?,
                Operator::Path => compile(inner, Target::Path, match_case)?,
                Operator::Content => compile(inner, Target::Content, match_case)?,
                Operator::Tag => compile(inner, Target::Tag, match_case)?,
                Operator::Line => within(Unit::Line, inner)?,
                Operator::Block => within(Unit::Block, inner)?,
                Operator::Section => within(Unit::Section, inner)?,
                Operator::Task => within(Unit::Task(None), inner)?,
                Operator::TaskTodo => within(Unit::Task(Some(false)), inner)?,
                Operator::TaskDone => within(Unit::Task(Some(true)), inner)?,
                Operator::MatchCase => compile(inner, target, true)?,
                Operator::IgnoreCase => compile(inner, target, false)?,
            }
        }
        Query::Property { key, value } => Node::Property {
            key: key.to_lowercase(),
            value: value
                .as_ref()
                .map(|v| compile(v, Target::Value, match_case).map(Box::new))
                .transpose()?,
        },
        Query::Compare(comparison, value) => Node::Compare(*comparison, value.clone()),
        Query::Null => Node::Null,
    })
}

fn eval(node: &Node, note: &NoteData, scope: &Scope) -> Option<Hit> {
    match node {
        Node::All => Some(Hit::default()),
        Node::Text { re, target } => {
            let path = note.path.as_str();
            let (found, hit) = match target {
                Target::Any => {
                    let (in_text, content) = find(re, &note.text, scope.range.clone());
                    let (in_path, path) = if scope.file && note.is_note() {
                        find(re, path, 0..path.len())
                    } else {
                        (false, Vec::new())
                    };
                    (in_text || in_path, Hit { content, path })
                }
                Target::Content => {
                    let (found, content) = find(re, &note.text, scope.range.clone());
                    (
                        found,
                        Hit {
                            content,
                            path: Vec::new(),
                        },
                    )
                }
                Target::Name => {
                    let start = path.len() - note.path.file_name().len();
                    let (found, path) = find(re, path, start..path.len());
                    (
                        found,
                        Hit {
                            content: Vec::new(),
                            path,
                        },
                    )
                }
                Target::Path => {
                    let (found, path) = find(re, path, 0..path.len());
                    (
                        found,
                        Hit {
                            content: Vec::new(),
                            path,
                        },
                    )
                }
                Target::Tag | Target::Value => (false, Hit::default()),
            };
            found.then_some(hit)
        }
        Node::Tag(pattern) => {
            if scope.file && !note.tags.iter().any(|t| pattern.matches(t)) {
                return None;
            }
            let content: Vec<Span> = if note.is_note() {
                note.document()
                    .tags
                    .iter()
                    .filter(|t| within(&t.range, &scope.range) && pattern.matches(&t.name))
                    .map(|t| t.range.clone())
                    .collect()
            } else {
                Vec::new()
            };
            (scope.file || !content.is_empty()).then(|| Hit {
                content,
                path: Vec::new(),
            })
        }
        // Comparisons and `null` only mean something in a property's value.
        Node::Compare(..) | Node::Null => None,
        Node::And(items) => {
            let mut hit = Hit::default();
            for item in items {
                hit.extend(eval(item, note, scope)?);
            }
            Some(hit)
        }
        Node::Or(items) => {
            let mut found = None::<Hit>;
            for item in items {
                if let Some(h) = eval(item, note, scope) {
                    found.get_or_insert_with(Hit::default).extend(h);
                }
            }
            found
        }
        Node::Not(inner) => match eval(inner, note, scope) {
            Some(_) => None,
            None => Some(Hit::default()),
        },
        Node::Unit(unit, inner) => {
            let mut found = None::<Hit>;
            for range in units(note, *unit, &scope.range) {
                let unit_scope = Scope { range, file: false };
                if let Some(h) = eval(inner, note, &unit_scope) {
                    found.get_or_insert_with(Hit::default).extend(h);
                }
            }
            found
        }
        Node::Property { key, value } => {
            let (_, found) = note
                .properties
                .iter()
                .find(|(k, _)| k.to_lowercase() == *key)?;
            if let Some(value) = value
                && !value_matches(value, found)
            {
                return None;
            }
            let content = note
                .document()
                .frontmatter
                .iter()
                .flat_map(|fm| &fm.entries)
                .filter(|e| e.key.to_lowercase() == *key)
                .map(|e| e.key_range.start..e.value_range.end)
                .collect();
            Some(Hit {
                content,
                path: Vec::new(),
            })
        }
    }
}

/// Whether `re` matches in `text[range]`, and the non-empty matches.
fn find(re: &Regex, text: &str, range: Span) -> (bool, Vec<Span>) {
    let mut found = false;
    let mut spans = Vec::new();
    for m in re.find_iter(&text[range.clone()]) {
        found = true;
        if !m.is_empty() {
            spans.push(range.start + m.start()..range.start + m.end());
        }
    }
    (found, spans)
}

fn within(inner: &Span, outer: &Span) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

fn value_matches(node: &Node, value: &Value) -> bool {
    match node {
        Node::All => true,
        Node::Null => matches!(value, Value::Null),
        Node::And(items) => items.iter().all(|n| value_matches(n, value)),
        Node::Or(items) => items.iter().any(|n| value_matches(n, value)),
        Node::Not(inner) => !value_matches(inner, value),
        Node::Text { re, .. } => scalars(value).iter().any(|s| re.is_match(s)),
        Node::Compare(comparison, operand) => scalars(value)
            .iter()
            .any(|s| compare(*comparison, s, operand)),
        Node::Tag(_) | Node::Unit(..) | Node::Property { .. } => false,
    }
}

/// The scalar values in a property, as text.
fn scalars(value: &Value) -> Vec<String> {
    match value {
        Value::Null => Vec::new(),
        Value::Bool(b) => vec![b.to_string()],
        Value::Int(i) => vec![i.to_string()],
        Value::Float(f) => vec![f.to_string()],
        Value::String(s) => vec![s.clone()],
        Value::List(items) => items.iter().flat_map(scalars).collect(),
        Value::Map(entries) => entries.iter().flat_map(|(_, v)| scalars(v)).collect(),
    }
}

/// Compares numerically when both sides are numbers, and as text otherwise
/// (which orders ISO dates correctly).
fn compare(comparison: Comparison, value: &str, operand: &str) -> bool {
    use std::cmp::Ordering;
    let ordering = match (value.trim().parse::<f64>(), operand.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => a.partial_cmp(&b),
        _ => Some(value.to_lowercase().cmp(&operand.to_lowercase())),
    };
    let Some(ordering) = ordering else {
        return false;
    };
    match comparison {
        Comparison::Lt => ordering == Ordering::Less,
        Comparison::Le => ordering != Ordering::Greater,
        Comparison::Gt => ordering == Ordering::Greater,
        Comparison::Ge => ordering != Ordering::Less,
        Comparison::Eq => ordering == Ordering::Equal,
    }
}

/// The lines, blocks, sections or tasks of `note` inside `range`, clipped
/// to it.
fn units(note: &NoteData, unit: Unit, range: &Span) -> Vec<Span> {
    let text = &note.text;
    let clip = |r: Span| -> Option<Span> {
        let clipped = r.start.max(range.start)..r.end.min(range.end);
        (clipped.start < clipped.end).then_some(clipped)
    };
    match unit {
        Unit::Line => igneous_markdown::text::lines_in(text, range)
            .into_iter()
            .filter_map(clip)
            .collect(),
        Unit::Block => {
            let doc = note.document();
            doc.nodes
                .iter()
                .filter(|n| {
                    matches!(
                        n.kind,
                        NodeKind::Heading { .. }
                            | NodeKind::Paragraph
                            | NodeKind::ListItem { .. }
                            | NodeKind::CodeBlock { .. }
                            | NodeKind::Table
                            | NodeKind::HtmlBlock
                            | NodeKind::FootnoteDefinition
                    )
                })
                .map(|n| match n.kind {
                    NodeKind::ListItem { .. } => own_item_range(doc, &n.range),
                    _ => n.range.clone(),
                })
                .filter_map(clip)
                .collect()
        }
        Unit::Section => {
            let doc = note.document();
            let mut starts: Vec<usize> = vec![0];
            starts.extend(doc.headings.iter().map(|h| h.range.start));
            starts.dedup();
            let mut ends: Vec<usize> = starts[1..].to_vec();
            ends.push(text.len());
            starts
                .into_iter()
                .zip(ends)
                .map(|(s, e)| s..e)
                .filter_map(clip)
                .collect()
        }
        Unit::Task(done) => {
            let doc = note.document();
            doc.tasks
                .iter()
                .filter(|t| done.is_none_or(|d| t.is_done() == d))
                .map(|t| own_item_range(doc, &t.item))
                .filter_map(clip)
                .collect()
        }
    }
}

/// A list item without the lists nested inside it, so a sub-item is its own
/// block.
fn own_item_range(doc: &Document, item: &Span) -> Span {
    let nested = doc
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::List { .. }))
        .map(|n| n.range.start)
        .find(|&start| start > item.start && start < item.end);
    item.start..nested.unwrap_or(item.end)
}

fn merge(spans: &mut Vec<Span>) {
    spans.sort_by_key(|s| (s.start, s.end));
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    *spans = merged;
}

/// A line of a note with highlighted matches, for showing search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    /// Zero-based line number.
    pub line: usize,
    /// The line, without its newline.
    pub range: Span,
    /// Matches on this line, as byte ranges in the note.
    pub highlights: Vec<Span>,
}

/// Groups sorted match ranges by line. A match spanning lines is split.
pub fn snippets(text: &str, ranges: &[Span]) -> Vec<Snippet> {
    let mut out: Vec<Snippet> = Vec::new();
    let mut line = 0;
    let mut line_start = 0;
    for range in ranges {
        let mut start = range.start.min(text.len());
        let end = range.end.min(text.len());
        while start < end {
            // Advance to the line containing `start`.
            while let Some(i) = text[line_start..start].find('\n') {
                line_start += i + 1;
                line += 1;
            }
            let line_end = text[line_start..]
                .find('\n')
                .map_or(text.len(), |i| line_start + i);
            let piece = start..end.min(line_end);
            if !piece.is_empty() {
                match out.last_mut() {
                    Some(s) if s.line == line => s.highlights.push(piece),
                    _ => out.push(Snippet {
                        line,
                        range: line_start..line_end,
                        highlights: vec![piece],
                    }),
                }
            }
            start = line_end + 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use igneous_core::VaultPath;

    fn note(path: &str, text: &str) -> NoteData {
        NoteData::from_text(VaultPath::new(path).unwrap(), text)
    }

    fn hits(search: &str, note: &NoteData) -> Option<Vec<String>> {
        let m = Matcher::parse(search, SearchOptions::default()).unwrap();
        m.matches(note).map(|h| {
            h.content
                .iter()
                .map(|r| note.text[r.clone()].to_owned())
                .collect()
        })
    }

    #[test]
    fn words_match_inside_words_and_in_paths() {
        let n = note("Work/Meetings.md", "The meeting is on Monday.\n");
        assert_eq!(hits("meet", &n), Some(vec!["meet".into()]));
        assert_eq!(hits("MONDAY", &n), Some(vec!["Monday".into()]));
        // Found in the path only.
        assert_eq!(hits("work", &n), Some(vec![]));
        assert_eq!(hits("content:work", &n), None);
        assert_eq!(hits("meeting -friday", &n), Some(vec!["meeting".into()]));
        assert_eq!(hits("meeting -monday", &n), None);
        assert_eq!(hits("\"meeting is\"", &n), Some(vec!["meeting is".into()]));
        assert_eq!(hits("match-case:monday", &n), None);
        assert_eq!(hits("tuesday OR monday", &n), Some(vec!["Monday".into()]));
    }

    #[test]
    fn match_case_option() {
        let n = note("a.md", "HappyCat\n");
        let m = Matcher::parse("happycat", SearchOptions { match_case: true }).unwrap();
        assert!(m.matches(&n).is_none());
        let m = Matcher::parse("ignore-case:happycat", SearchOptions { match_case: true }).unwrap();
        assert!(m.matches(&n).is_some());
    }

    #[test]
    fn file_and_path() {
        let n = note("Daily notes/2022-07-01.md", "nothing\n");
        let m = Matcher::parse("file:2022", SearchOptions::default()).unwrap();
        assert_eq!(m.matches(&n).unwrap().path, vec![12..16]);
        assert_eq!(hits("file:daily", &n), None);
        assert_eq!(hits("path:\"Daily notes/2022-07\"", &n), Some(vec![]));
        assert_eq!(hits(r"path:/\d{4}-\d{2}-\d{2}/", &n), Some(vec![]));

        let image = NoteData::new(VaultPath::new("Attachments/cat.jpg").unwrap());
        let m = Matcher::parse("file:.jpg", SearchOptions::default()).unwrap();
        assert!(m.matches(&image).is_some());
        // Plain words never find attachments.
        let m = Matcher::parse("cat", SearchOptions::default()).unwrap();
        assert!(m.matches(&image).is_none());
    }

    #[test]
    fn tags_include_nested_tags() {
        let n = note(
            "a.md",
            "---\ntags: [project]\n---\nWork on #work/igneous and #myjob/work\n",
        );
        assert_eq!(hits("tag:#work", &n), Some(vec!["#work/igneous".into()]));
        assert_eq!(
            hits("tag:work/igneous", &n),
            Some(vec!["#work/igneous".into()])
        );
        assert_eq!(hits("tag:wor", &n), None);
        assert_eq!(hits("tag:project", &n), Some(vec![]));
        assert_eq!(hits("tag:(#nothing OR #project)", &n), Some(vec![]));
    }

    #[test]
    fn lines_blocks_and_sections() {
        let text = "# Cake\nmix the flour\nadd sugar\n\nthen bake\n# Bread\nflour and water\n";
        let n = note("Recipes.md", text);
        assert_eq!(
            hits("line:(mix flour)", &n),
            Some(vec!["mix".into(), "flour".into()])
        );
        assert_eq!(hits("line:(flour sugar)", &n), None);
        assert_eq!(hits("block:(flour sugar)", &n).map(|h| h.len()), Some(2));
        assert_eq!(hits("block:(flour bake)", &n), None);
        assert_eq!(hits("section:(flour bake)", &n).map(|h| h.len()), Some(2));
        assert_eq!(hits("section:(sugar water)", &n), None);
        // No line of the note contains "cheese".
        assert_eq!(hits("-line:cheese", &n), Some(vec![]));
    }

    #[test]
    fn tasks() {
        let text = "- [ ] call Alex\n- [x] email Sam\n  - [ ] call back\n- plain call\n";
        let n = note("Tasks.md", text);
        assert_eq!(
            hits("task:call", &n),
            Some(vec!["call".into(), "call".into()])
        );
        assert_eq!(hits("task-done:call", &n), None);
        assert_eq!(hits("task-done:email", &n), Some(vec!["email".into()]));
        assert_eq!(
            hits("task-todo:(call OR email)", &n).map(|h| h.len()),
            Some(2)
        );
        // The nested task is its own block: "email" and "back" aren't together.
        assert_eq!(hits("task:(email back)", &n), None);
    }

    #[test]
    fn properties() {
        let text = "---\nstatus: Draft\nduration: 3\naliases:\ncount: [1, 12]\n---\nbody\n";
        let n = note("a.md", text);
        assert_eq!(hits("[status]", &n), Some(vec!["status: Draft".into()]));
        assert!(hits("[STATUS:draft]", &n).is_some());
        assert!(hits("[status:Draft OR Published]", &n).is_some());
        assert!(hits("[status:Published]", &n).is_none());
        assert!(hits("[duration:<5]", &n).is_some());
        assert!(hits("[duration:>5]", &n).is_none());
        assert!(hits("[count:>10]", &n).is_some());
        assert!(hits("[aliases:null]", &n).is_some());
        assert!(hits("[status:null]", &n).is_none());
        assert!(hits("[missing]", &n).is_none());
        assert!(hits("body [status:-Published]", &n).is_some());
    }

    #[test]
    fn regex_errors_are_reported() {
        assert!(Matcher::parse("/(unclosed/", SearchOptions::default()).is_err());
    }

    #[test]
    fn snippet_lines() {
        let text = "one two\nthree\nfour two";
        let s = snippets(text, &[4..7, 8..13, 13..18]);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].line, s[0].range.clone()), (0, 0..7));
        assert_eq!(
            (s[1].line, s[1].highlights.len(), s[1].highlights[0].clone()),
            (1, 1, 8..13)
        );
        assert_eq!(
            (s[2].line, s[2].highlights.len(), s[2].highlights[0].clone()),
            (2, 1, 14..18)
        );
    }
}
