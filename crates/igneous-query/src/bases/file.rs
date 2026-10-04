//! Reading `.base` files and writing view state back into them.

use igneous_markdown::TextEdit;
use igneous_markdown::Value as YamlValue;
use igneous_markdown::text::{line_end, line_start};
use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};

/// A parsed `.base` file (or the body of a ` ```base ` block).
///
/// Filters, formulas and summaries are kept as expression source and parsed
/// when a view runs, so one broken formula doesn't hide the whole base.
#[derive(Debug, Clone)]
pub struct BaseFile {
    source: String,
    pub filters: Option<Filter>,
    /// Formula name → expression.
    pub formulas: Vec<(String, String)>,
    /// Property ID (`note.price`, `file.ext`, `formula.ppu`) → settings.
    pub properties: Vec<(String, PropertyConfig)>,
    /// Custom summary name → expression over `values`.
    pub summaries: Vec<(String, String)>,
    pub views: Vec<View>,
    /// Parts of the file Igneous couldn't make sense of. They're left as
    /// written.
    pub problems: Vec<String>,
    layouts: Vec<ViewLayout>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertyConfig {
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// Every filter must hold.
    And(Vec<Filter>),
    /// At least one must hold.
    Or(Vec<Filter>),
    /// None may hold.
    Not(Vec<Filter>),
    /// An expression, e.g. `file.hasTag("book")`.
    Expr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// `table`, `cards`, `list`, or another layout such as `kanban`.
    pub kind: String,
    pub name: String,
    pub limit: Option<usize>,
    pub filters: Option<Filter>,
    /// Property IDs of the columns, in order, as written.
    pub order: Vec<String>,
    pub sort: Vec<SortKey>,
    pub group_by: Option<SortKey>,
    /// Property ID → width in pixels.
    pub column_sizes: Vec<(String, u32)>,
    /// Property ID → summary name.
    pub summaries: Vec<(String, String)>,
    /// Other settings, such as a cards view's `image`, as written.
    pub options: Vec<(String, YamlValue)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub property: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Asc,
    Desc,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Asc => "ASC",
            Direction::Desc => "DESC",
        }
    }
}

/// Changes to a view made in the UI. `None` leaves a setting alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewState {
    pub order: Option<Vec<String>>,
    pub column_sizes: Option<Vec<(String, u32)>>,
    pub sort: Option<Vec<SortKey>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BaseError {
    #[error("the base isn't valid YAML: {0}")]
    Yaml(String),
    #[error("a base must be a set of `key: value` settings")]
    NotAMapping,
    #[error("there's no view number {0}")]
    NoSuchView(usize),
    #[error("this view is written on one line, so Igneous can't change it")]
    FlowStyle,
}

/// Where a view's settings are in the source.
#[derive(Debug, Clone, Default)]
struct ViewLayout {
    /// Key name, and the byte range of the key.
    keys: Vec<(String, std::ops::Range<usize>)>,
    flow: bool,
}

impl BaseFile {
    pub fn parse(source: &str) -> Result<BaseFile, BaseError> {
        let mut base = BaseFile {
            source: source.to_owned(),
            filters: None,
            formulas: Vec::new(),
            properties: Vec::new(),
            summaries: Vec::new(),
            views: Vec::new(),
            problems: Vec::new(),
            layouts: Vec::new(),
        };
        if source.trim().is_empty() {
            return Ok(base);
        }
        let docs = MarkedYaml::load_from_str(source).map_err(|e| BaseError::Yaml(e.to_string()))?;
        let Some(root) = docs.first() else {
            return Ok(base);
        };
        let offsets = CharOffsets::new(source);
        let entries = match &root.data {
            YamlData::Mapping(map) => map,
            YamlData::Value(Scalar::Null) => return Ok(base),
            _ => return Err(BaseError::NotAMapping),
        };
        for (key, value) in entries {
            let key = scalar_text(key).unwrap_or_default();
            match key.as_str() {
                "filters" => base.filters = filter(value, &mut base.problems),
                "formulas" => base.formulas = text_map(value, "formulas", &mut base.problems),
                "summaries" => base.summaries = text_map(value, "summaries", &mut base.problems),
                "properties" => {
                    for (id, config) in mapping(value) {
                        let display_name = mapping(config)
                            .into_iter()
                            .find(|(k, _)| scalar_text(k).as_deref() == Some("displayName"))
                            .and_then(|(_, v)| scalar_text(v));
                        base.properties.push((
                            scalar_text(id).unwrap_or_default(),
                            PropertyConfig { display_name },
                        ));
                    }
                }
                "views" => match &value.data {
                    YamlData::Sequence(items) => {
                        for (i, item) in items.iter().enumerate() {
                            match view(item, &offsets, source, &mut base.problems) {
                                Some((view, layout)) => {
                                    base.views.push(view);
                                    base.layouts.push(layout);
                                }
                                None => base
                                    .problems
                                    .push(format!("view {} isn't a set of settings", i + 1)),
                            }
                        }
                    }
                    YamlData::Value(Scalar::Null) => {}
                    _ => base.problems.push("`views` should be a list".into()),
                },
                _ => {}
            }
        }
        Ok(base)
    }

    /// The text the base was parsed from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The view called `name`, for embeds such as `![[Books.base#Reading]]`.
    pub fn view_index(&self, name: &str) -> Option<usize> {
        self.views.iter().position(|v| v.name == name).or_else(|| {
            self.views
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(name))
        })
    }

    /// The display name configured for a property, if any. `id` may be
    /// written with or without its `note.` prefix.
    pub fn display_name(&self, id: &str) -> Option<&str> {
        let id = normalise_id(id);
        self.properties
            .iter()
            .find(|(key, _)| normalise_id(key) == id)
            .and_then(|(_, config)| config.display_name.as_deref())
    }

    /// The edits that write `state` into view `view`, as Obsidian would.
    /// Settings that didn't change produce no edits, and nothing else in the
    /// file is touched.
    pub fn set_view_state(
        &self,
        view: usize,
        state: &ViewState,
    ) -> Result<Vec<TextEdit>, BaseError> {
        let current = self.views.get(view).ok_or(BaseError::NoSuchView(view))?;
        let layout = &self.layouts[view];
        let src = self.source.as_str();

        let mut changes: Vec<(&str, Option<String>)> = Vec::new();
        let Some((_, first)) = layout.keys.first() else {
            return Err(BaseError::FlowStyle);
        };
        if layout.flow {
            return Err(BaseError::FlowStyle);
        }
        let column = first.start - line_start(src, first.start);
        let inner = " ".repeat(column + 2);

        if let Some(order) = &state.order
            && *order != current.order
        {
            let value = if order.is_empty() {
                " []".to_owned()
            } else {
                order
                    .iter()
                    .map(|id| format!("\n{inner}- {}", yaml_scalar(id)))
                    .collect()
            };
            changes.push(("order", Some(value)));
        }
        if let Some(sort) = &state.sort
            && *sort != current.sort
        {
            let value = (!sort.is_empty()).then(|| {
                sort.iter()
                    .map(|key| {
                        format!(
                            "\n{inner}- property: {}\n{inner}  direction: {}",
                            yaml_scalar(&key.property),
                            key.direction.as_str()
                        )
                    })
                    .collect()
            });
            changes.push(("sort", value));
        }
        if let Some(sizes) = &state.column_sizes
            && *sizes != current.column_sizes
        {
            let value = (!sizes.is_empty()).then(|| {
                sizes
                    .iter()
                    .map(|(id, width)| format!("\n{inner}{}: {width}", yaml_scalar(id)))
                    .collect()
            });
            changes.push(("columnSize", value));
        }

        let mut edits = Vec::new();
        let mut appended = String::new();
        for (key, value) in changes {
            match layout.keys.iter().find(|(name, _)| name == key) {
                Some((_, range)) => {
                    let extent = value_extent(src, range);
                    match value {
                        Some(value) => edits.push(TextEdit::replace(extent, value)),
                        None => {
                            let start = line_start(src, range.start);
                            if src[start..range.start].trim().is_empty() {
                                // Remove the key's lines.
                                let end = (extent.end + 1).min(src.len());
                                edits.push(TextEdit::delete(start..end));
                            } else {
                                // The key starts the view's `- ` item, so keep
                                // it and empty it instead.
                                let empty = if key == "sort" { " []" } else { " {}" };
                                edits.push(TextEdit::replace(extent, empty));
                            }
                        }
                    }
                }
                None => {
                    if let Some(value) = value {
                        appended.push_str(&format!("{}{key}:{value}\n", " ".repeat(column)));
                    }
                }
            }
        }
        if !appended.is_empty() {
            let end = layout
                .keys
                .iter()
                .map(|(_, range)| value_extent(src, range).end)
                .max()
                .unwrap_or(src.len());
            if end < src.len() {
                edits.push(TextEdit::insert(end + 1, appended));
            } else {
                edits.push(TextEdit::insert(src.len(), format!("\n{appended}")));
            }
        }
        Ok(edits)
    }
}

/// `price` and `note.price` are the same property.
pub fn normalise_id(id: &str) -> String {
    let id = id.trim();
    if id.starts_with("note.") || id.starts_with("file.") || id.starts_with("formula.") {
        id.to_owned()
    } else {
        format!("note.{id}")
    }
}

fn view(
    node: &MarkedYaml,
    offsets: &CharOffsets,
    source: &str,
    problems: &mut Vec<String>,
) -> Option<(View, ViewLayout)> {
    let YamlData::Mapping(map) = &node.data else {
        return None;
    };
    let start = offsets.at(node.span.start.index());
    let mut layout = ViewLayout {
        keys: Vec::new(),
        flow: source[start..].starts_with('{'),
    };
    let mut view = View {
        kind: String::new(),
        name: String::new(),
        limit: None,
        filters: None,
        order: Vec::new(),
        sort: Vec::new(),
        group_by: None,
        column_sizes: Vec::new(),
        summaries: Vec::new(),
        options: Vec::new(),
    };
    for (key, value) in map {
        let name = scalar_text(key).unwrap_or_default();
        layout.keys.push((name.clone(), offsets.span(key)));
        match name.as_str() {
            "type" => view.kind = scalar_text(value).unwrap_or_default(),
            "name" => view.name = scalar_text(value).unwrap_or_default(),
            "limit" => view.limit = scalar_text(value).and_then(|s| s.trim().parse().ok()),
            "filters" => view.filters = filter(value, problems),
            "order" => {
                view.order = sequence(value)
                    .into_iter()
                    .filter_map(scalar_text)
                    .collect();
            }
            "sort" => {
                view.sort = sequence(value).into_iter().filter_map(sort_key).collect();
            }
            "groupBy" => view.group_by = sort_key(value),
            "columnSize" => {
                view.column_sizes = mapping(value)
                    .into_iter()
                    .filter_map(|(k, v)| {
                        let width = scalar_text(v)?.trim().parse::<f64>().ok()?;
                        Some((scalar_text(k)?, width.round().max(0.0) as u32))
                    })
                    .collect();
            }
            "summaries" => view.summaries = text_map(value, "view summaries", problems),
            _ => view.options.push((name, convert(value))),
        }
    }
    if view.name.is_empty() {
        view.name = view.kind.clone();
    }
    Some((view, layout))
}

fn sort_key(node: &MarkedYaml) -> Option<SortKey> {
    let mut property = None;
    let mut direction = Direction::Asc;
    for (k, v) in mapping(node) {
        match scalar_text(k).as_deref() {
            Some("property") => property = scalar_text(v),
            Some("direction") if scalar_text(v).is_some_and(|d| d.eq_ignore_ascii_case("desc")) => {
                direction = Direction::Desc;
            }
            _ => {}
        }
    }
    Some(SortKey {
        property: property?,
        direction,
    })
}

fn filter(node: &MarkedYaml, problems: &mut Vec<String>) -> Option<Filter> {
    match &node.data {
        YamlData::Mapping(map) => {
            let mut found = None;
            for (key, value) in map {
                let items: Vec<Filter> = match &value.data {
                    YamlData::Sequence(items) => {
                        items.iter().filter_map(|i| filter(i, problems)).collect()
                    }
                    _ => filter(value, problems).into_iter().collect(),
                };
                found = match scalar_text(key).as_deref() {
                    Some("and") => Some(Filter::And(items)),
                    Some("or") => Some(Filter::Or(items)),
                    Some("not") => Some(Filter::Not(items)),
                    other => {
                        problems.push(format!(
                            "unknown filter group `{}`; expected and, or or not",
                            other.unwrap_or("")
                        ));
                        continue;
                    }
                };
            }
            found
        }
        YamlData::Value(Scalar::Null) => None,
        _ => match scalar_text(node) {
            Some(text) => Some(Filter::Expr(text)),
            None => {
                problems.push("a filter should be an expression or an and/or/not group".into());
                None
            }
        },
    }
}

fn text_map(node: &MarkedYaml, what: &str, problems: &mut Vec<String>) -> Vec<(String, String)> {
    match &node.data {
        YamlData::Mapping(_) => mapping(node)
            .into_iter()
            .filter_map(|(k, v)| Some((scalar_text(k)?, scalar_text(v)?)))
            .collect(),
        YamlData::Value(Scalar::Null) => Vec::new(),
        _ => {
            problems.push(format!(
                "`{what}` should be a set of `name: expression` pairs"
            ));
            Vec::new()
        }
    }
}

fn mapping<'a, 'b>(node: &'a MarkedYaml<'b>) -> Vec<(&'a MarkedYaml<'b>, &'a MarkedYaml<'b>)> {
    match &node.data {
        YamlData::Mapping(map) => map.iter().collect(),
        YamlData::Tagged(_, inner) => mapping(inner),
        _ => Vec::new(),
    }
}

fn sequence<'a, 'b>(node: &'a MarkedYaml<'b>) -> Vec<&'a MarkedYaml<'b>> {
    match &node.data {
        YamlData::Sequence(items) => items.iter().collect(),
        YamlData::Tagged(_, inner) => sequence(inner),
        _ => Vec::new(),
    }
}

fn scalar_text(node: &MarkedYaml) -> Option<String> {
    match &node.data {
        YamlData::Value(scalar) => Some(match scalar {
            Scalar::Null => return None,
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

fn convert(node: &MarkedYaml) -> YamlValue {
    match &node.data {
        YamlData::Value(scalar) => match scalar {
            Scalar::Null => YamlValue::Null,
            Scalar::Boolean(b) => YamlValue::Bool(*b),
            Scalar::Integer(i) => YamlValue::Int(*i),
            Scalar::FloatingPoint(f) => YamlValue::Float(**f),
            Scalar::String(s) => YamlValue::String(s.to_string()),
        },
        YamlData::Representation(s, _, _) => YamlValue::String(s.to_string()),
        YamlData::Sequence(items) => YamlValue::List(items.iter().map(convert).collect()),
        YamlData::Mapping(map) => YamlValue::Map(
            map.iter()
                .map(|(k, v)| (scalar_text(k).unwrap_or_default(), convert(v)))
                .collect(),
        ),
        YamlData::Tagged(_, inner) => convert(inner),
        YamlData::Alias(_) | YamlData::BadValue => YamlValue::Null,
    }
}

/// The text of a key's value in block style: from just after the colon to
/// the end of the last line indented under the key (or, for a list, at the
/// key's own indentation). Trailing blank and comment lines aren't included.
fn value_extent(src: &str, key: &std::ops::Range<usize>) -> std::ops::Range<usize> {
    let column = key.start - line_start(src, key.start);
    let key_line_end = line_end(src, key.start);
    let start = src[key.end..key_line_end]
        .find(':')
        .map_or(key_line_end, |i| key.end + i + 1);
    let mut end = key_line_end;
    let mut pos = key_line_end + 1;
    while pos < src.len() {
        let le = line_end(src, pos);
        let line = &src[pos..le];
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            pos = le + 1;
            continue;
        }
        if indent > column || (indent == column && (trimmed.starts_with("- ") || trimmed == "-")) {
            end = le;
            pos = le + 1;
            continue;
        }
        break;
    }
    start..end
}

/// A YAML scalar, quoted only when it has to be.
fn yaml_scalar(text: &str) -> String {
    let plain = !text.is_empty()
        && text.trim() == text
        && !text.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%',
            '@', '`',
        ])
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.ends_with(':')
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
        )
        && text.parse::<f64>().is_err()
        && !text.contains(['\n', '\t']);
    if plain {
        text.to_owned()
    } else {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t");
        format!("\"{escaped}\"")
    }
}

/// saphyr reports positions in characters; these are byte offsets.
struct CharOffsets {
    bytes: Vec<usize>,
}

impl CharOffsets {
    fn new(text: &str) -> Self {
        let mut bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        bytes.push(text.len());
        Self { bytes }
    }

    fn at(&self, char_index: usize) -> usize {
        self.bytes[char_index.min(self.bytes.len() - 1)]
    }

    fn span(&self, node: &MarkedYaml) -> std::ops::Range<usize> {
        self.at(node.span.start.index())..self.at(node.span.end.index())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use igneous_markdown::edit::apply;

    const BOOKS: &str = "\
filters:
  or:
    - file.hasTag(\"tag\")
    - and:
        - file.hasTag(\"book\")
        - file.hasLink(\"Textbook\")
    - not:
        - file.hasTag(\"book\")
        - file.inFolder(\"Required Reading\")
formulas:
  formatted_price: 'if(price, price.toFixed(2) + \" dollars\")'
  ppu: \"(price / age).toFixed(2)\"
properties:
  status:
    displayName: Status
  formula.formatted_price:
    displayName: \"Price\"
summaries:
  customAverage: 'values.mean().round(3)'
views:
  - type: table
    name: \"My table\"
    limit: 10
    groupBy:
      property: note.age
      direction: DESC
    filters:
      and:
        - 'status != \"done\"'
    order:
      - file.name
      - note.age
    summaries:
      formula.ppu: Average
    rowHeight: tall # an unknown setting
  - type: cards
    name: Gallery
    image: note.cover
";

    #[test]
    fn reads_every_section() {
        let base = BaseFile::parse(BOOKS).unwrap();
        assert!(base.problems.is_empty(), "{:?}", base.problems);
        let Some(Filter::Or(items)) = &base.filters else {
            panic!("{:?}", base.filters);
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Filter::Expr("file.hasTag(\"tag\")".into()));
        assert!(matches!(&items[2], Filter::Not(n) if n.len() == 2));
        assert_eq!(
            base.formulas[1],
            ("ppu".into(), "(price / age).toFixed(2)".into())
        );
        assert_eq!(base.display_name("status"), Some("Status"));
        assert_eq!(base.display_name("note.status"), Some("Status"));
        assert_eq!(base.display_name("formula.formatted_price"), Some("Price"));
        assert_eq!(base.summaries[0].0, "customAverage");

        let table = &base.views[0];
        assert_eq!(
            (table.kind.as_str(), table.name.as_str()),
            ("table", "My table")
        );
        assert_eq!(table.limit, Some(10));
        assert_eq!(
            table.group_by,
            Some(SortKey {
                property: "note.age".into(),
                direction: Direction::Desc
            })
        );
        assert_eq!(table.order, ["file.name", "note.age"]);
        assert_eq!(
            table.summaries,
            [("formula.ppu".to_owned(), "Average".to_owned())]
        );
        assert_eq!(
            table.options,
            [("rowHeight".to_owned(), YamlValue::String("tall".into()))]
        );
        assert_eq!(base.views[1].kind, "cards");
        assert_eq!(base.view_index("gallery"), Some(1));
    }

    #[test]
    fn view_state_edits_are_minimal() {
        let base = BaseFile::parse(BOOKS).unwrap();
        let state = ViewState {
            order: Some(vec![
                "file.name".into(),
                "note.age".into(),
                "formula.ppu".into(),
            ]),
            column_sizes: Some(vec![("file.name".into(), 282)]),
            sort: Some(vec![SortKey {
                property: "note.age".into(),
                direction: Direction::Desc,
            }]),
        };
        let edits = base.set_view_state(0, &state).unwrap();
        let out = apply(BOOKS, &edits);
        let expected = BOOKS.replace(
            "      - note.age\n    summaries:\n      formula.ppu: Average\n    rowHeight: tall # an unknown setting\n",
            "      - note.age\n      - formula.ppu\n    summaries:\n      formula.ppu: Average\n    rowHeight: tall # an unknown setting\n    sort:\n      - property: note.age\n        direction: DESC\n    columnSize:\n      file.name: 282\n",
        );
        assert_eq!(out, expected);

        // Writing the same state again changes nothing.
        let again = BaseFile::parse(&out).unwrap();
        assert_eq!(again.set_view_state(0, &state).unwrap(), Vec::new());
        assert_eq!(again.views[0].column_sizes, [("file.name".to_owned(), 282)]);

        // Clearing the sort removes it.
        let cleared = again
            .set_view_state(
                0,
                &ViewState {
                    sort: Some(Vec::new()),
                    ..ViewState::default()
                },
            )
            .unwrap();
        let out2 = apply(&out, &cleared);
        assert!(!out2.contains("sort:"));
        assert!(out2.contains("columnSize:\n      file.name: 282\n"));
    }

    #[test]
    fn appends_to_the_last_view_at_the_end_of_the_file() {
        let src = "views:\n  - type: table\n    name: Table\n    filters:\n      and:\n        - file.hasTag(\"x\")";
        let base = BaseFile::parse(src).unwrap();
        let edits = base
            .set_view_state(
                0,
                &ViewState {
                    order: Some(vec!["file.name".into(), "confidence".into()]),
                    ..ViewState::default()
                },
            )
            .unwrap();
        assert_eq!(
            apply(src, &edits),
            format!("{src}\n    order:\n      - file.name\n      - confidence\n")
        );
    }

    #[test]
    fn replaces_a_list_written_at_the_key_indent() {
        let src = "views:\n- type: table\n  order:\n  - a\n  - b\n  name: T\n";
        let base = BaseFile::parse(src).unwrap();
        assert_eq!(base.views[0].order, ["a", "b"]);
        let edits = base
            .set_view_state(
                0,
                &ViewState {
                    order: Some(vec!["b".into()]),
                    ..ViewState::default()
                },
            )
            .unwrap();
        assert_eq!(
            apply(src, &edits),
            "views:\n- type: table\n  order:\n    - b\n  name: T\n"
        );
    }

    #[test]
    fn flow_style_views_are_read_but_not_edited() {
        let base = BaseFile::parse("views: [{type: table, name: T, order: [a]}]\n").unwrap();
        assert_eq!(base.views[0].order, ["a"]);
        assert_eq!(
            base.set_view_state(
                0,
                &ViewState {
                    order: Some(vec![]),
                    ..ViewState::default()
                }
            ),
            Err(BaseError::FlowStyle)
        );
    }

    #[test]
    fn odd_files() {
        assert!(BaseFile::parse("").unwrap().views.is_empty());
        assert!(
            BaseFile::parse("# just a comment\n")
                .unwrap()
                .views
                .is_empty()
        );
        assert_eq!(
            BaseFile::parse("- a\n").unwrap_err(),
            BaseError::NotAMapping
        );
        assert!(matches!(
            BaseFile::parse("views: [\n"),
            Err(BaseError::Yaml(_))
        ));
        let base = BaseFile::parse("views: 3\nfilters:\n  maybe:\n    - x\n").unwrap();
        assert_eq!(base.problems.len(), 2);
        assert_eq!(base.filters, None);
    }

    #[test]
    fn quoting() {
        assert_eq!(yaml_scalar("file.name"), "file.name");
        assert_eq!(yaml_scalar("note.my prop"), "note.my prop");
        assert_eq!(yaml_scalar("true"), "\"true\"");
        assert_eq!(yaml_scalar("12"), "\"12\"");
        assert_eq!(yaml_scalar("a: b"), "\"a: b\"");
        assert_eq!(yaml_scalar("#x"), "\"#x\"");
        assert_eq!(yaml_scalar("say \"hi\""), "say \"hi\"");
    }
}
