//! Turning a note's text into the rows the index stores.

use std::sync::LazyLock;

use igneous_markdown::frontmatter::Frontmatter;
use igneous_markdown::{Document, LinkKind, Span, Subpath, Value};
use regex::Regex;

use crate::resolve::target_key;
use crate::value::{self, is_date, is_datetime};

#[derive(Debug, Default)]
pub(crate) struct Record {
    pub links: Vec<LinkRow>,
    pub tags: Vec<TagRow>,
    pub properties: Vec<PropertyRow>,
    pub aliases: Vec<String>,
    pub headings: Vec<HeadingRow>,
    pub blocks: Vec<BlockRow>,
    pub tasks: Vec<TaskRow>,
}

#[derive(Debug)]
pub(crate) struct LinkRow {
    pub raw: String,
    pub target: String,
    /// `None` for external links (URLs), which never resolve to a file.
    pub target_key: Option<String>,
    pub subpath: Option<String>,
    pub display: Option<String>,
    pub kind: &'static str,
    pub embed: bool,
    pub in_frontmatter: bool,
    pub range: Span,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) struct TagRow {
    /// Without the `#`.
    pub tag: String,
    pub range: Span,
    pub in_frontmatter: bool,
}

#[derive(Debug)]
pub(crate) struct PropertyRow {
    pub key: String,
    pub value_json: String,
    pub ty: &'static str,
    pub text: Option<String>,
    pub num: Option<f64>,
    pub date: Option<String>,
}

#[derive(Debug)]
pub(crate) struct HeadingRow {
    pub level: u8,
    pub text: String,
    pub range: Span,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) struct BlockRow {
    pub id: String,
    pub start: usize,
    pub line: usize,
}

#[derive(Debug)]
pub(crate) struct TaskRow {
    pub line: usize,
    pub status: char,
    pub text: String,
}

/// Byte offset → line number (from 0).
pub(crate) struct Lines(Vec<usize>);

impl Lines {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self(starts)
    }

    pub fn line_of(&self, pos: usize) -> usize {
        self.0.partition_point(|&s| s <= pos).saturating_sub(1)
    }
}

/// The text of line `line` (from 0), without its newline.
pub(crate) fn line_text(text: &str, line: usize) -> &str {
    text.split('\n').nth(line).unwrap_or("")
}

pub(crate) fn extract(text: &str) -> Record {
    let doc = igneous_markdown::parse(text);
    extract_doc(text, &doc)
}

pub(crate) fn extract_doc(text: &str, doc: &Document) -> Record {
    let lines = Lines::new(text);
    let mut record = Record::default();

    for link in &doc.links {
        let reference = &link.reference;
        let external =
            matches!(link.kind, LinkKind::Url | LinkKind::Autolink) || reference.is_external();
        record.links.push(LinkRow {
            raw: text[link.range.clone()].to_owned(),
            target: reference.target.clone(),
            target_key: (!external).then(|| target_key(&reference.target)),
            subpath: reference.subpath.as_ref().map(subpath_text),
            display: reference.display.clone(),
            kind: match link.kind {
                LinkKind::Wiki => "wiki",
                LinkKind::Markdown => "markdown",
                LinkKind::Autolink => "autolink",
                LinkKind::Url => "url",
            },
            embed: link.embed,
            in_frontmatter: link.in_frontmatter,
            range: link.range.clone(),
            line: lines.line_of(link.range.start),
        });
    }

    for tag in &doc.tags {
        record.tags.push(TagRow {
            tag: tag.name.clone(),
            range: tag.range.clone(),
            in_frontmatter: false,
        });
    }

    if let Some(fm) = &doc.frontmatter {
        for (tag, range) in frontmatter_tags(text, fm) {
            record.tags.push(TagRow {
                tag,
                range,
                in_frontmatter: true,
            });
        }
        for entry in &fm.entries {
            record
                .properties
                .push(property_row(&entry.key, &entry.value));
            if matches!(entry.key.to_lowercase().as_str(), "aliases" | "alias") {
                record.aliases.extend(
                    entry
                        .value
                        .string_list()
                        .into_iter()
                        .map(|a| a.trim().to_owned())
                        .filter(|a| !a.is_empty()),
                );
            }
        }
    }

    for heading in &doc.headings {
        record.headings.push(HeadingRow {
            level: heading.level,
            text: heading.text.clone(),
            range: heading.range.clone(),
            line: lines.line_of(heading.range.start),
        });
    }
    for block in &doc.block_ids {
        record.blocks.push(BlockRow {
            id: block.id.clone(),
            start: block.range.start,
            line: lines.line_of(block.range.start),
        });
    }
    for task in &doc.tasks {
        let rest = &text[task.marker.end.min(task.item.end)..task.item.end];
        record.tasks.push(TaskRow {
            line: lines.line_of(task.item.start),
            status: task.status,
            text: rest.lines().next().unwrap_or("").trim().to_owned(),
        });
    }
    record
}

/// `#Heading#Sub` or `#^block`.
pub(crate) fn subpath_text(subpath: &Subpath) -> String {
    match subpath {
        Subpath::Block(id) => format!("#^{id}"),
        Subpath::Heading(path) => path.iter().map(|h| format!("#{h}")).collect(),
    }
}

pub(crate) fn parse_subpath(text: &str) -> Option<Subpath> {
    let rest = text.strip_prefix('#')?;
    Some(match rest.strip_prefix('^') {
        Some(block) => Subpath::Block(block.to_owned()),
        None => Subpath::Heading(rest.split('#').map(str::to_owned).collect()),
    })
}

fn property_row(key: &str, value: &Value) -> PropertyRow {
    let (ty, text, num, date) = match value {
        Value::Null => ("null", None, None, None),
        Value::Bool(b) => ("bool", Some(b.to_string()), None, None),
        Value::Int(i) => ("number", Some(i.to_string()), Some(*i as f64), None),
        Value::Float(f) => ("number", Some(f.to_string()), Some(*f), None),
        Value::String(s) => {
            let date = (is_date(s) || is_datetime(s)).then(|| s.clone());
            ("string", Some(s.clone()), None, date)
        }
        Value::List(_) => ("list", Some(value.string_list().join(", ")), None, None),
        Value::Map(_) => ("map", None, None, None),
    };
    PropertyRow {
        key: key.to_owned(),
        value_json: value::encode(value).to_string(),
        ty,
        text,
        num,
        date,
    }
}

/// The tags in the frontmatter's `tags` (or `tag`) property, with the range
/// of each in the note (including a `#` if it was written with one).
///
/// Values may be a list (flow or block) or a string of tags separated by
/// commas or spaces, quoted or not.
pub(crate) fn frontmatter_tags(text: &str, fm: &Frontmatter) -> Vec<(String, Span)> {
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r##"#?[^\s,\[\]'"#]+"##).unwrap());
    let mut tags = Vec::new();
    for entry in &fm.entries {
        if !matches!(entry.key.to_lowercase().as_str(), "tags" | "tag")
            || !matches!(entry.value, Value::String(_) | Value::List(_))
        {
            continue;
        }
        let range = entry.value_range.clone();
        let Some(slice) = text.get(range.clone()) else {
            continue;
        };
        for m in TOKEN.find_iter(slice) {
            let token = m.as_str();
            let name = token.trim_start_matches('#');
            // A block list's `-` markers aren't tags; nor are bare numbers.
            if name.is_empty() || name == "-" || name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            tags.push((
                name.to_owned(),
                range.start + m.start()..range.start + m.end(),
            ));
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let lines = Lines::new("ab\ncd\n\nef");
        assert_eq!(lines.line_of(0), 0);
        assert_eq!(lines.line_of(2), 0);
        assert_eq!(lines.line_of(3), 1);
        assert_eq!(lines.line_of(6), 2);
        assert_eq!(lines.line_of(8), 3);
        assert_eq!(line_text("ab\ncd", 1), "cd");
    }

    #[test]
    fn frontmatter_tag_forms() {
        for (yaml, expected) in [
            ("tags: [a, \"b/c\", '#d']", vec!["a", "b/c", "d"]),
            ("tags:\n  - a\n  - \"#b\"", vec!["a", "b"]),
            ("tags: a, b c", vec!["a", "b", "c"]),
            ("tags: 2026", vec![]),
            ("tag: x", vec!["x"]),
            ("other: [a]", vec![]),
        ] {
            let text = format!("---\n{yaml}\n---\nbody\n");
            let doc = igneous_markdown::parse(&text);
            let found = frontmatter_tags(&text, doc.frontmatter.as_ref().unwrap());
            let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, expected, "{yaml}");
            for (name, range) in &found {
                assert!(text[range.clone()].ends_with(name.as_str()));
            }
        }
    }

    #[test]
    fn records() {
        let text = "---\naliases: [Start]\ntags: [home]\nup: \"[[Index]]\"\ncount: 3\n---\n\
                    # Home\n\nSee [[Note#Part|that]] and [x](sub/Other.md) and https://example.com #tag\n\
                    - [ ] a task ^blk\n";
        let record = extract(text);
        let targets: Vec<(&str, Option<&str>, bool)> = record
            .links
            .iter()
            .map(|l| (l.target.as_str(), l.target_key.as_deref(), l.in_frontmatter))
            .collect();
        assert!(targets.contains(&("Index", Some("index"), true)));
        assert!(targets.contains(&("Note", Some("note"), false)));
        assert!(targets.contains(&("sub/Other.md", Some("other"), false)));
        assert!(targets.contains(&("https://example.com", None, false)));
        let note = record.links.iter().find(|l| l.target == "Note").unwrap();
        assert_eq!(note.subpath.as_deref(), Some("#Part"));
        assert_eq!(note.display.as_deref(), Some("that"));
        assert_eq!(note.line, 8);
        let tags: Vec<(&str, bool)> = record
            .tags
            .iter()
            .map(|t| (t.tag.as_str(), t.in_frontmatter))
            .collect();
        assert_eq!(tags, [("tag", false), ("home", true)]);
        assert_eq!(record.aliases, ["Start"]);
        let count = record.properties.iter().find(|p| p.key == "count").unwrap();
        assert_eq!(count.num, Some(3.0));
        assert_eq!(record.headings[0].text, "Home");
        assert_eq!(record.blocks[0].id, "blk");
        assert_eq!(record.tasks[0].status, ' ');
        assert_eq!(record.tasks[0].text, "a task ^blk");
    }

    #[test]
    fn subpaths_round_trip() {
        for s in ["#A#B", "#^id"] {
            assert_eq!(subpath_text(&parse_subpath(s).unwrap()), s);
        }
    }
}
