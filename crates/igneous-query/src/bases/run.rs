//! Running a view: filtering, sorting, grouping and summarising files.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

use igneous_core::VaultPath;
use igneous_core::path::natural_cmp;
use jiff::Zoned;

use super::eval::{Env, Eval, EvalError, Files};
use super::expr::{self, Expr, ExprError};
use super::file::{BaseFile, Direction, Filter, normalise_id};
use super::functions::{Registry, median};
use super::value::{Value, equals, sort_order};
use crate::NoteData;

/// A cell: a value, or why it couldn't be worked out.
pub type Cell = Result<Value, EvalError>;

#[derive(Debug, Clone)]
pub enum ViewResult {
    Ready(ViewData),
    /// A layout Igneous can't show yet, such as `kanban` or `map`.
    Unsupported(String),
    NoSuchView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Table,
    Cards,
    List,
}

#[derive(Debug, Clone)]
pub struct ViewData {
    pub kind: ViewKind,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Set when the view is grouped. Groups cover `rows` in order.
    pub groups: Vec<Group>,
    pub summaries: Vec<Summary>,
    /// How many files matched, before `limit`.
    pub total: usize,
    /// Problems with the view as a whole, such as a filter that can't be
    /// read. Each appears once.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The property, always prefixed: `note.status`, `file.name`, `formula.ppu`.
    pub id: String,
    /// The header text.
    pub name: String,
    /// Width in pixels, if the view sets one.
    pub width: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub file: VaultPath,
    /// One per column.
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub key: Value,
    /// Indices into [`ViewData::rows`].
    pub rows: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Summary {
    /// Index into [`ViewData::columns`].
    pub column: usize,
    pub name: String,
    pub value: Cell,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// What `now()` and `today()` return, and the time zone for file times.
    pub now: Zoned,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { now: Zoned::now() }
    }
}

/// Runs view `view` of `base` over `notes`, which should be every file in
/// the vault. `this` is the file showing the base: the `.base` file itself,
/// or the note embedding it.
pub fn run(
    base: &BaseFile,
    view: usize,
    notes: &[NoteData],
    this: Option<&NoteData>,
) -> ViewResult {
    run_with(base, view, notes, this, &RunOptions::default())
}

pub fn run_with(
    base: &BaseFile,
    view: usize,
    notes: &[NoteData],
    this: Option<&NoteData>,
    options: &RunOptions,
) -> ViewResult {
    let Some(view) = base.views.get(view) else {
        return ViewResult::NoSuchView;
    };
    let kind = match view.kind.as_str() {
        "table" => ViewKind::Table,
        "cards" => ViewKind::Cards,
        "list" => ViewKind::List,
        other => return ViewResult::Unsupported(other.to_owned()),
    };
    let files = Files::new(notes);
    let env = Env {
        files: &files,
        formulas: base
            .formulas
            .iter()
            .map(|(name, source)| (name.clone(), expr::parse(source)))
            .collect(),
        this,
        now: options.now.clone(),
        registry: Registry::standard(),
    };
    let mut errors = Vec::new();

    // Filter.
    let filters: Vec<&Filter> = base.filters.iter().chain(&view.filters).collect();
    let mut parsed = HashMap::new();
    for filter in &filters {
        parse_filter(filter, &mut parsed);
    }
    let mut rows: Vec<Eval<'_>> = Vec::new();
    for note in notes {
        let mut ev = Eval::new(&env, Some(note));
        if filters
            .iter()
            .all(|f| check(f, &mut ev, &parsed, &mut errors))
        {
            rows.push(ev);
        }
    }

    // Sort, grouping first.
    let mut keys = Vec::new();
    if let Some(group) = &view.group_by {
        keys.push((normalise_id(&group.property), group.direction));
    }
    keys.extend(
        view.sort
            .iter()
            .map(|k| (normalise_id(&k.property), k.direction)),
    );
    let mut sorted: Vec<(Vec<Cell>, Eval<'_>)> = rows
        .into_iter()
        .map(|mut ev| {
            let values = keys.iter().map(|(id, _)| ev.property_id(id)).collect();
            (values, ev)
        })
        .collect();
    sorted.sort_by(|(a, ea), (b, eb)| {
        keys.iter()
            .zip(a.iter().zip(b))
            .map(|((_, direction), (x, y))| compare_cells(x, y, *direction))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| {
                let (pa, pb) = (&ea.row.unwrap().path, &eb.row.unwrap().path);
                natural_cmp(pa.file_name(), pb.file_name()).then_with(|| pa.cmp(pb))
            })
    });
    let total = sorted.len();
    if let Some(limit) = view.limit.filter(|&l| l > 0) {
        sorted.truncate(limit);
    }

    // Columns and cells.
    let ids: Vec<String> = if view.order.is_empty() {
        vec!["file.name".to_owned()]
    } else {
        view.order.iter().map(|id| normalise_id(id)).collect()
    };
    let columns: Vec<Column> = ids
        .iter()
        .map(|id| Column {
            id: id.clone(),
            name: base
                .display_name(id)
                .map_or_else(|| default_name(id), str::to_owned),
            width: view
                .column_sizes
                .iter()
                .find(|(key, _)| normalise_id(key) == *id)
                .map(|(_, width)| *width),
        })
        .collect();

    let mut groups: Vec<Group> = Vec::new();
    let mut out_rows = Vec::with_capacity(sorted.len());
    for (i, (key_values, mut ev)) in sorted.into_iter().enumerate() {
        if view.group_by.is_some() {
            let key = key_values[0].clone().unwrap_or(Value::Null);
            match groups.last_mut() {
                Some(group) if same_group(&group.key, &key) => group.rows.end = i + 1,
                _ => groups.push(Group {
                    key,
                    rows: i..i + 1,
                }),
            }
        }
        let cells = ids.iter().map(|id| ev.property_id(id)).collect();
        out_rows.push(Row {
            file: ev.row.unwrap().path.clone(),
            cells,
        });
    }

    let summaries = view
        .summaries
        .iter()
        .filter_map(|(id, name)| {
            let id = normalise_id(id);
            let column = columns.iter().position(|c| c.id == id)?;
            let values: Vec<Value> = out_rows
                .iter()
                .map(|row| row.cells[column].clone().unwrap_or(Value::Null))
                .collect();
            Some(Summary {
                column,
                name: name.clone(),
                value: summarise(name, values, base, &env),
            })
        })
        .collect();

    ViewResult::Ready(ViewData {
        kind,
        columns,
        rows: out_rows,
        groups,
        summaries,
        total,
        errors,
    })
}

/// Evaluates one expression for `row` as a formula in `base` would be, e.g.
/// to preview a formula while it's being written.
pub fn evaluate(
    expression: &str,
    base: Option<&BaseFile>,
    notes: &[NoteData],
    row: Option<&NoteData>,
    this: Option<&NoteData>,
    options: &RunOptions,
) -> Cell {
    let expr = expr::parse(expression).map_err(|error| EvalError::Syntax {
        expression: expression.to_owned(),
        error,
    })?;
    let files = Files::new(notes);
    let env = Env {
        files: &files,
        formulas: base
            .iter()
            .flat_map(|b| &b.formulas)
            .map(|(name, source)| (name.clone(), expr::parse(source)))
            .collect(),
        this,
        now: options.now.clone(),
        registry: Registry::standard(),
    };
    Eval::new(&env, row).eval(&expr)
}

fn parse_filter(filter: &Filter, parsed: &mut HashMap<String, Result<Expr, ExprError>>) {
    match filter {
        Filter::And(items) | Filter::Or(items) | Filter::Not(items) => {
            items.iter().for_each(|f| parse_filter(f, parsed));
        }
        Filter::Expr(source) => {
            parsed
                .entry(source.clone())
                .or_insert_with(|| expr::parse(source));
        }
    }
}

fn check(
    filter: &Filter,
    ev: &mut Eval<'_>,
    parsed: &HashMap<String, Result<Expr, ExprError>>,
    errors: &mut Vec<String>,
) -> bool {
    match filter {
        Filter::And(items) => items.iter().all(|f| check(f, ev, parsed, errors)),
        // An empty "any of" group, as the filter editor creates, doesn't
        // filter anything out.
        Filter::Or(items) => items.is_empty() || items.iter().any(|f| check(f, ev, parsed, errors)),
        Filter::Not(items) => !items.iter().any(|f| check(f, ev, parsed, errors)),
        Filter::Expr(source) => {
            let result = match &parsed[source] {
                Ok(expr) => ev.eval(expr),
                Err(error) => Err(EvalError::Syntax {
                    expression: source.clone(),
                    error: error.clone(),
                }),
            };
            match result {
                Ok(value) => value.is_truthy(),
                Err(error) => {
                    let message = match error {
                        EvalError::Syntax { .. } => error.to_string(),
                        _ => format!("filter `{source}`: {error}"),
                    };
                    if !errors.contains(&message) {
                        errors.push(message);
                    }
                    false
                }
            }
        }
    }
}

/// Orders sort keys. Missing values and errors come last whatever the
/// direction.
fn compare_cells(a: &Cell, b: &Cell, direction: Direction) -> Ordering {
    let missing = |c: &Cell| matches!(c, Err(_) | Ok(Value::Null));
    match (missing(a), missing(b)) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => {
            let ordering = sort_order(a.as_ref().unwrap(), b.as_ref().unwrap());
            match direction {
                Direction::Asc => ordering,
                Direction::Desc => ordering.reverse(),
            }
        }
    }
}

fn same_group(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Null, Value::Null)) || equals(a, b)
}

/// The header for a property without a display name.
fn default_name(id: &str) -> String {
    if let Some(field) = id.strip_prefix("file.") {
        return format!("file {field}");
    }
    id.strip_prefix("note.")
        .or_else(|| id.strip_prefix("formula."))
        .unwrap_or(id)
        .to_owned()
}

fn summarise(name: &str, values: Vec<Value>, base: &BaseFile, env: &Env<'_>) -> Cell {
    if let Some((_, source)) = base.summaries.iter().find(|(n, _)| n == name) {
        let expr = expr::parse(source).map_err(|error| EvalError::Syntax {
            expression: source.clone(),
            error,
        })?;
        let mut ev = Eval::new(env, None);
        return ev.with_locals(vec![("values", Value::List(values))], |ev| ev.eval(&expr));
    }
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) if !n.is_nan() => Some(*n),
            _ => None,
        })
        .collect();
    let mut dates: Vec<&Value> = values
        .iter()
        .filter(|v| matches!(v, Value::Date(_) | Value::DateTime(_)))
        .collect();
    dates.sort_by(|a, b| sort_order(a, b));
    let count =
        |f: &dyn Fn(&Value) -> bool| Value::Number(values.iter().filter(|v| f(v)).count() as f64);
    let number = |n: Option<f64>| n.map_or(Value::Null, Value::Number);
    let mean = (!numbers.is_empty()).then(|| numbers.iter().sum::<f64>() / numbers.len() as f64);
    Ok(match name.to_lowercase().as_str() {
        "average" => number(mean),
        "min" => number(numbers.iter().copied().reduce(f64::min)),
        "max" => number(numbers.iter().copied().reduce(f64::max)),
        "sum" => Value::Number(numbers.iter().sum()),
        "median" => number(median(numbers.clone())),
        "stddev" => number(mean.map(|m| {
            (numbers.iter().map(|x| (x - m).powi(2)).sum::<f64>() / numbers.len() as f64).sqrt()
        })),
        "range" if numbers.is_empty() && dates.len() > 1 => {
            let (first, last) = (dates[0].as_datetime(), dates[dates.len() - 1].as_datetime());
            match (first, last) {
                (Some(first), Some(last)) => Value::Duration(
                    first
                        .until(last)
                        .map_err(|e| EvalError::Invalid(e.to_string()))?,
                ),
                _ => Value::Null,
            }
        }
        "range" => number(
            numbers
                .iter()
                .copied()
                .reduce(f64::max)
                .zip(numbers.iter().copied().reduce(f64::min))
                .map(|(max, min)| max - min),
        ),
        "earliest" => dates.first().map_or(Value::Null, |d| (*d).clone()),
        "latest" => dates.last().map_or(Value::Null, |d| (*d).clone()),
        "checked" => count(&|v| matches!(v, Value::Bool(true))),
        "unchecked" => count(&|v| matches!(v, Value::Bool(false))),
        "empty" => count(&Value::is_empty),
        "filled" => count(&|v| !v.is_empty()),
        "unique" => {
            let mut seen: Vec<&Value> = Vec::new();
            for value in values.iter().filter(|v| !matches!(v, Value::Null)) {
                if !seen.iter().any(|s| equals(s, value)) {
                    seen.push(value);
                }
            }
            Value::Number(seen.len() as f64)
        }
        _ => return Err(EvalError::Unsupported(format!("the summary `{name}`"))),
    })
}
