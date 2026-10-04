//! Evaluating expressions against a file.

use std::collections::HashMap;

use igneous_core::VaultPath;
use igneous_core::path::loose_key;
use igneous_markdown::Value as YamlValue;
use jiff::Zoned;
use jiff::civil::Time;
use jiff::tz::TimeZone;

use super::expr::{BinaryOp, Expr, ExprError, UnaryOp};
use super::functions::Registry;
use super::value::{self, Link, Value, compare, equals, has_time, parse_date, parse_duration};
use crate::NoteData;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    /// A function, method or view type Igneous doesn't support yet.
    #[error("{0} isn't supported yet")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
    #[error("formula `{0}` depends on itself")]
    Cycle(String),
    #[error("there's no formula called `{0}`")]
    UnknownFormula(String),
    #[error("formula `{name}` can't be read: {error}")]
    Formula { name: String, error: ExprError },
    #[error("`{expression}` can't be read: {error}")]
    Syntax {
        expression: String,
        error: ExprError,
    },
}

pub(crate) fn invalid(message: impl Into<String>) -> EvalError {
    EvalError::Invalid(message.into())
}

/// The files of the vault, with the lookups link resolution needs.
pub(crate) struct Files<'a> {
    pub notes: &'a [NoteData],
    by_path: HashMap<&'a VaultPath, usize>,
    by_loose_path: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

impl<'a> Files<'a> {
    pub fn new(notes: &'a [NoteData]) -> Self {
        let mut files = Files {
            notes,
            by_path: HashMap::with_capacity(notes.len()),
            by_loose_path: HashMap::with_capacity(notes.len()),
            by_name: HashMap::with_capacity(notes.len()),
        };
        for (i, note) in notes.iter().enumerate() {
            files.by_path.insert(&note.path, i);
            files
                .by_loose_path
                .entry(note.path.loose_key())
                .or_default()
                .push(i);
            files
                .by_name
                .entry(loose_key(note.path.file_name()))
                .or_default()
                .push(i);
            if note.is_note() {
                files
                    .by_name
                    .entry(loose_key(note.path.stem()))
                    .or_default()
                    .push(i);
            }
        }
        files
    }

    pub fn get(&self, path: &VaultPath) -> Option<&'a NoteData> {
        self.by_path.get(path).map(|&i| &self.notes[i])
    }

    /// Resolves a link target as Obsidian does: an exact path, then a path
    /// relative to `from`, then a unique name; ambiguous names prefer `from`'s
    /// folder, then the shortest path, then alphabetical order.
    pub fn resolve(&self, target: &str, from: Option<&VaultPath>) -> Option<VaultPath> {
        let mut target = target.trim();
        if let Some(inner) = target.strip_prefix("[[").and_then(|t| t.strip_suffix("]]")) {
            target = inner;
        }
        let target = target.split('|').next().unwrap_or("");
        let target = target.split('#').next().unwrap_or("").trim();
        if target.is_empty() || target.contains("://") {
            return None;
        }
        let pick = |candidates: &[usize]| -> Option<VaultPath> {
            let mut candidates: Vec<&NoteData> =
                candidates.iter().map(|&i| &self.notes[i]).collect();
            let folder = from.and_then(VaultPath::parent);
            candidates.sort_by(|a, b| {
                let same = |n: &NoteData| n.path.parent() == folder;
                same(b)
                    .cmp(&same(a))
                    .then(a.path.as_str().len().cmp(&b.path.as_str().len()))
                    .then_with(|| a.path.cmp(&b.path))
            });
            candidates.first().map(|n| n.path.clone())
        };

        let exact = |path: &str| {
            let key = loose_key(path);
            self.by_loose_path
                .get(&key)
                .or_else(|| self.by_loose_path.get(&format!("{key}.md")))
                .and_then(|c| pick(c))
        };
        let target = target.trim_start_matches("./").trim_start_matches('/');
        if let Some(found) = exact(target) {
            return Some(found);
        }
        if let Some(found) = from
            .and_then(VaultPath::parent)
            .and_then(|folder| folder.join(target).ok())
            .and_then(|path| exact(path.as_str()))
        {
            return Some(found);
        }
        if target.contains('/') {
            let suffix = loose_key(target);
            let matches: Vec<usize> = self
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let key = n.path.loose_key();
                    key.ends_with(&format!("/{suffix}")) || key.ends_with(&format!("/{suffix}.md"))
                })
                .map(|(i, _)| i)
                .collect();
            return pick(&matches);
        }
        self.by_name.get(&loose_key(target)).and_then(|c| pick(c))
    }
}

/// What every evaluation in one run shares.
pub(crate) struct Env<'a> {
    pub files: &'a Files<'a>,
    pub formulas: HashMap<String, Result<Expr, ExprError>>,
    pub this: Option<&'a NoteData>,
    pub now: Zoned,
    pub registry: &'static Registry,
}

impl Env<'_> {
    pub fn tz(&self) -> TimeZone {
        self.now.time_zone().clone()
    }
}

/// Evaluates expressions for one row (or for none, in summaries).
pub(crate) struct Eval<'a> {
    pub(crate) env: &'a Env<'a>,
    pub(crate) row: Option<&'a NoteData>,
    formulas: HashMap<String, Result<Value, EvalError>>,
    in_progress: Vec<String>,
    locals: Vec<(String, Value)>,
}

impl<'a> Eval<'a> {
    pub(crate) fn new(env: &'a Env<'a>, row: Option<&'a NoteData>) -> Self {
        Self {
            env,
            row,
            formulas: HashMap::new(),
            in_progress: Vec::new(),
            locals: Vec::new(),
        }
    }

    pub fn eval(&mut self, expr: &Expr) -> Result<Value, EvalError> {
        match expr {
            Expr::Null => Ok(Value::Null),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::String(s) => Ok(Value::String(s.clone())),
            Expr::Regex { pattern, flags } => {
                let regex = regex::RegexBuilder::new(pattern)
                    .case_insensitive(flags.contains('i'))
                    .multi_line(flags.contains('m'))
                    .dot_matches_new_line(flags.contains('s'))
                    .build()
                    .map_err(|e| {
                        invalid(format!("/{pattern}/ isn't a valid regular expression: {e}"))
                    })?;
                Ok(Value::Regex(value::RegexValue {
                    regex: regex.into(),
                    global: flags.contains('g'),
                }))
            }
            Expr::List(items) => Ok(Value::List(
                items
                    .iter()
                    .map(|e| self.eval(e))
                    .collect::<Result<_, _>>()?,
            )),
            Expr::Object(entries) => Ok(Value::Object(
                entries
                    .iter()
                    .map(|(k, e)| Ok((k.clone(), self.eval(e)?)))
                    .collect::<Result<_, EvalError>>()?,
            )),
            Expr::Ident(name) => Ok(self.ident(name)),
            Expr::Member(receiver, name) => {
                if let Expr::Ident(base) = receiver.as_ref()
                    && !self.is_local(base)
                {
                    match base.as_str() {
                        "note" => return Ok(self.row_property(name)),
                        "formula" => return self.formula(name),
                        "file" => {
                            return Ok(match self.row {
                                Some(row) => self.file_field(row, name).unwrap_or(Value::Null),
                                None => Value::Null,
                            });
                        }
                        _ => {}
                    }
                }
                let value = self.eval(receiver)?;
                Ok(self.member(&value, name))
            }
            Expr::Index(receiver, index) => {
                let index = self.eval(index)?;
                if let (Expr::Ident(base), Value::String(key)) = (receiver.as_ref(), &index)
                    && !self.is_local(base)
                {
                    match base.as_str() {
                        "note" => return Ok(self.row_property(key)),
                        "formula" => return self.formula(key),
                        _ => {}
                    }
                }
                let value = self.eval(receiver)?;
                Ok(match (&value, &index) {
                    (Value::List(items), Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => {
                        items.get(*n as usize).cloned().unwrap_or(Value::Null)
                    }
                    (Value::String(s), Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => s
                        .chars()
                        .nth(*n as usize)
                        .map_or(Value::Null, |c| Value::String(c.to_string())),
                    (_, Value::String(key)) => self.member(&value, key),
                    _ => Value::Null,
                })
            }
            Expr::Call(name, args) => match self.env.registry.global(name) {
                Some(function) => function(self, &Value::Null, args),
                None => Err(EvalError::Unsupported(format!("{name}()"))),
            },
            Expr::Method(receiver, name, args) => {
                let value = self.eval(receiver)?;
                let registry = self.env.registry;
                match registry
                    .method(value.type_name(), name)
                    .or_else(|| registry.method("any", name))
                {
                    Some(function) => function(self, &value, args),
                    // Methods on a missing property give nothing rather than
                    // an error, so `tags.contains("x")` is simply false.
                    None if matches!(value, Value::Null) => Ok(Value::Null),
                    None => Err(EvalError::Unsupported(format!(
                        "{}.{name}()",
                        value.type_name()
                    ))),
                }
            }
            Expr::Unary(op, inner) => {
                let value = self.eval(inner)?;
                Ok(match op {
                    UnaryOp::Not => Value::Bool(!value.is_truthy()),
                    UnaryOp::Neg => match value {
                        Value::Null => Value::Null,
                        Value::Duration(span) => Value::Duration(span.negate()),
                        other => Value::Number(-other.as_number().ok_or_else(|| {
                            invalid(format!("can't negate {}", other.type_name()))
                        })?),
                    },
                })
            }
            Expr::Binary(op, lhs, rhs) => self.binary(*op, lhs, rhs),
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|(n, _)| n == name)
    }

    /// Runs `f` with extra variables (`value`, `index`, `acc`, `values`).
    pub(crate) fn with_locals<T>(
        &mut self,
        vars: Vec<(&str, Value)>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let depth = self.locals.len();
        self.locals.extend(
            vars.into_iter()
                .map(|(name, value)| (name.to_owned(), value)),
        );
        let result = f(self);
        self.locals.truncate(depth);
        result
    }

    fn ident(&self, name: &str) -> Value {
        if let Some((_, value)) = self.locals.iter().rev().find(|(n, _)| n == name) {
            return value.clone();
        }
        match name {
            "file" => self
                .row
                .map_or(Value::Null, |row| Value::File(row.path.clone())),
            "this" => self
                .env
                .this
                .map_or(Value::Null, |this| Value::File(this.path.clone())),
            "note" => self.row.map_or(Value::Null, |row| self.properties(row)),
            _ => self.row_property(name),
        }
    }

    fn row_property(&self, name: &str) -> Value {
        match self.row {
            Some(row) => self.property(row, name),
            None => Value::Null,
        }
    }

    /// A property of `note`, converted: wikilinks become links and
    /// date-like text becomes dates.
    pub(crate) fn property(&self, note: &NoteData, name: &str) -> Value {
        note.property(name)
            .map_or(Value::Null, |v| self.convert(v, &note.path))
    }

    fn properties(&self, note: &NoteData) -> Value {
        Value::Object(
            note.properties
                .iter()
                .map(|(k, v)| (k.clone(), self.convert(v, &note.path)))
                .collect(),
        )
    }

    fn convert(&self, value: &YamlValue, from: &VaultPath) -> Value {
        match value {
            YamlValue::Null => Value::Null,
            YamlValue::Bool(b) => Value::Bool(*b),
            YamlValue::Int(i) => Value::Number(*i as f64),
            YamlValue::Float(f) => Value::Number(*f),
            YamlValue::String(s) => {
                let trimmed = s.trim();
                if let Some(inner) = trimmed
                    .strip_prefix("[[")
                    .and_then(|t| t.strip_suffix("]]"))
                    && !inner.contains("[[")
                {
                    let (target, display) = match inner.split_once('|') {
                        Some((t, d)) => (t.trim(), Some(d.trim().to_owned())),
                        None => (inner.trim(), None),
                    };
                    let target = target.split('#').next().unwrap_or("").to_owned();
                    let path = self.env.files.resolve(&target, Some(from));
                    return Value::Link(Link {
                        target,
                        display,
                        path,
                    });
                }
                parse_date(s).unwrap_or_else(|| Value::String(s.clone()))
            }
            YamlValue::List(items) => {
                Value::List(items.iter().map(|v| self.convert(v, from)).collect())
            }
            YamlValue::Map(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), self.convert(v, from)))
                    .collect(),
            ),
        }
    }

    /// A link to `path`, as `file.links` and friends return.
    pub(crate) fn link_to(&self, target: &str, path: Option<VaultPath>) -> Value {
        Value::Link(Link {
            target: target.to_owned(),
            display: None,
            path,
        })
    }

    pub(crate) fn datetime(&self, millis: i64) -> Value {
        match jiff::Timestamp::from_millisecond(millis) {
            Ok(ts) => Value::DateTime(self.env.tz().to_datetime(ts)),
            Err(_) => Value::Null,
        }
    }

    /// `file.name`, `file.mtime` and the other file properties.
    pub(crate) fn file_field(&self, note: &NoteData, name: &str) -> Option<Value> {
        let path = &note.path;
        Some(match name {
            "name" => Value::String(path.file_name().to_owned()),
            "basename" => Value::String(path.stem().to_owned()),
            "path" => Value::String(path.as_str().to_owned()),
            "folder" => Value::String(
                path.parent()
                    .map_or_else(|| "/".to_owned(), |p| p.as_str().to_owned()),
            ),
            "ext" => Value::String(path.extension().unwrap_or("").to_owned()),
            "size" => Value::Number(note.size as f64),
            "ctime" => self.datetime(note.ctime),
            "mtime" => self.datetime(note.mtime),
            "tags" => Value::List(note.tags.iter().map(|t| Value::String(t.clone())).collect()),
            "links" => Value::List(
                note.links
                    .iter()
                    .map(|l| self.link_to(&l.target, l.path.clone()))
                    .collect(),
            ),
            "embeds" => Value::List(
                note.embeds
                    .iter()
                    .map(|l| self.link_to(&l.target, l.path.clone()))
                    .collect(),
            ),
            "backlinks" => Value::List(
                note.backlinks
                    .iter()
                    .map(|p| self.link_to(p.as_str(), Some(p.clone())))
                    .collect(),
            ),
            "properties" => self.properties(note),
            "file" => Value::File(path.clone()),
            _ => return None,
        })
    }

    /// `receiver.name` for a value.
    fn member(&self, value: &Value, name: &str) -> Value {
        match value {
            Value::File(path) => {
                if name == "file" {
                    return value.clone();
                }
                match self.env.files.get(path) {
                    Some(note) => self
                        .file_field(note, name)
                        .unwrap_or_else(|| self.property(note, name)),
                    None => Value::Null,
                }
            }
            Value::String(s) if name == "length" => Value::Number(s.chars().count() as f64),
            Value::List(items) if name == "length" => Value::Number(items.len() as f64),
            Value::Date(d) => match name {
                "year" => Value::Number(d.year().into()),
                "month" => Value::Number(d.month().into()),
                "day" => Value::Number(d.day().into()),
                "hour" | "minute" | "second" | "millisecond" => Value::Number(0.0),
                _ => Value::Null,
            },
            Value::DateTime(dt) => match name {
                "year" => Value::Number(dt.year().into()),
                "month" => Value::Number(dt.month().into()),
                "day" => Value::Number(dt.day().into()),
                "hour" => Value::Number(dt.hour().into()),
                "minute" => Value::Number(dt.minute().into()),
                "second" => Value::Number(dt.second().into()),
                "millisecond" => Value::Number(dt.millisecond().into()),
                _ => Value::Null,
            },
            Value::Object(entries) => entries
                .iter()
                .find(|(k, _)| k == name)
                .map_or(Value::Null, |(_, v)| v.clone()),
            _ => Value::Null,
        }
    }

    /// The value of `formula.name` for this row.
    pub(crate) fn formula(&mut self, name: &str) -> Result<Value, EvalError> {
        if let Some(cached) = self.formulas.get(name) {
            return cached.clone();
        }
        if self.in_progress.iter().any(|n| n == name) {
            return Err(EvalError::Cycle(name.to_owned()));
        }
        let env = self.env;
        let result = match env.formulas.get(name) {
            None => Err(EvalError::UnknownFormula(name.to_owned())),
            Some(Err(error)) => Err(EvalError::Formula {
                name: name.to_owned(),
                error: error.clone(),
            }),
            Some(Ok(expr)) => {
                self.in_progress.push(name.to_owned());
                let result = self.eval(expr);
                self.in_progress.pop();
                result
            }
        };
        self.formulas.insert(name.to_owned(), result.clone());
        result
    }

    /// A column or sort key: `note.x` (or just `x`), `file.x` or `formula.x`.
    pub fn property_id(&mut self, id: &str) -> Result<Value, EvalError> {
        let id = id.trim();
        if let Some(name) = id.strip_prefix("formula.") {
            return self.formula(name);
        }
        if let Some(name) = id.strip_prefix("file.") {
            return Ok(self
                .row
                .and_then(|row| self.file_field(row, name))
                .unwrap_or(Value::Null));
        }
        Ok(self.row_property(id.strip_prefix("note.").unwrap_or(id)))
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> Result<Value, EvalError> {
        // `&&` and `||` return one of their operands, as in JavaScript.
        if op == BinaryOp::And {
            let left = self.eval(lhs)?;
            return if left.is_truthy() {
                self.eval(rhs)
            } else {
                Ok(left)
            };
        }
        if op == BinaryOp::Or {
            let left = self.eval(lhs)?;
            return if left.is_truthy() {
                Ok(left)
            } else {
                self.eval(rhs)
            };
        }
        let a = self.eval(lhs)?;
        let b = self.eval(rhs)?;
        use std::cmp::Ordering::{Equal, Greater, Less};
        let ordered = |accept: &[std::cmp::Ordering]| {
            Value::Bool(compare(&a, &b).is_some_and(|o| accept.contains(&o)))
        };
        Ok(match op {
            BinaryOp::Eq => Value::Bool(equals(&a, &b)),
            BinaryOp::Ne => Value::Bool(!equals(&a, &b)),
            BinaryOp::Lt => ordered(&[Less]),
            BinaryOp::Le => ordered(&[Less, Equal]),
            BinaryOp::Gt => ordered(&[Greater]),
            BinaryOp::Ge => ordered(&[Greater, Equal]),
            BinaryOp::Add => add(&a, &b)?,
            BinaryOp::Sub => subtract(&a, &b)?,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => arithmetic(op, &a, &b)?,
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        })
    }
}

fn as_duration(value: &Value) -> Option<jiff::Span> {
    match value {
        Value::Duration(span) => Some(*span),
        Value::String(s) => parse_duration(s),
        _ => None,
    }
}

/// Adds a duration to a date. Dates stay dates unless the duration has hours
/// or smaller parts.
pub(crate) fn shift(date: &Value, span: jiff::Span) -> Result<Value, EvalError> {
    let overflow = |e: jiff::Error| invalid(format!("the date is out of range: {e}"));
    match date {
        Value::Date(d) if !has_time(&span) => {
            d.checked_add(span).map(Value::Date).map_err(overflow)
        }
        Value::Date(d) => d
            .to_datetime(Time::midnight())
            .checked_add(span)
            .map(Value::DateTime)
            .map_err(overflow),
        Value::DateTime(dt) => dt.checked_add(span).map(Value::DateTime).map_err(overflow),
        _ => Err(invalid("only dates can be shifted by a duration")),
    }
}

fn add(a: &Value, b: &Value) -> Result<Value, EvalError> {
    Ok(match (a, b) {
        (Value::Date(_) | Value::DateTime(_), _) if as_duration(b).is_some() => {
            shift(a, as_duration(b).unwrap())?
        }
        (Value::Number(x), Value::Number(y)) => Value::Number(x + y),
        (Value::Duration(x), Value::Duration(y)) => Value::Duration(
            x.checked_add((*y, jiff::civil::date(2000, 1, 1)))
                .map_err(|e| invalid(e.to_string()))?,
        ),
        (Value::String(_), _) | (_, Value::String(_)) => {
            Value::String(format!("{}{}", a.display(), b.display()))
        }
        (Value::List(x), Value::List(y)) => Value::List(x.iter().chain(y).cloned().collect()),
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        _ => Err(invalid(format!(
            "can't add {} and {}",
            a.type_name(),
            b.type_name()
        )))?,
    })
}

fn subtract(a: &Value, b: &Value) -> Result<Value, EvalError> {
    let is_date = |v: &Value| matches!(v, Value::Date(_) | Value::DateTime(_));
    Ok(match (a, b) {
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        // The difference between two dates, in milliseconds.
        _ if is_date(a)
            && (is_date(b)
                || (matches!(b, Value::String(_))
                    && b.as_datetime().is_some()
                    && as_duration(b).is_none())) =>
        {
            let (x, y) = (a.as_datetime().unwrap(), b.as_datetime().unwrap());
            Value::Number(x.duration_since(y).as_millis() as f64)
        }
        _ if is_date(a) && as_duration(b).is_some() => shift(a, as_duration(b).unwrap().negate())?,
        (Value::Duration(x), Value::Duration(y)) => Value::Duration(
            x.checked_add((y.negate(), jiff::civil::date(2000, 1, 1)))
                .map_err(|e| invalid(e.to_string()))?,
        ),
        _ => match (a.as_number(), b.as_number()) {
            (Some(x), Some(y)) => Value::Number(x - y),
            _ => Err(invalid(format!(
                "can't subtract {} from {}",
                b.type_name(),
                a.type_name()
            )))?,
        },
    })
}

fn arithmetic(op: BinaryOp, a: &Value, b: &Value) -> Result<Value, EvalError> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return Ok(Value::Null);
    }
    if let (Value::Duration(span), Some(n)) = (a, b.as_number()) {
        let millis = value::duration_millis(span)
            .ok_or_else(|| invalid("this duration can't be measured"))?;
        return Ok(match op {
            BinaryOp::Mul if n.fract() == 0.0 => Value::Duration(
                span.checked_mul(n as i64)
                    .map_err(|e| invalid(e.to_string()))?,
            ),
            BinaryOp::Mul => Value::Number(millis * n),
            BinaryOp::Div => Value::Number(millis / n),
            _ => Value::Number(millis % n),
        });
    }
    let (Some(x), Some(y)) = (a.as_number(), b.as_number()) else {
        return Err(invalid(format!(
            "can't use {} on {} and {}",
            match op {
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                _ => "%",
            },
            a.type_name(),
            b.type_name()
        )));
    };
    Ok(Value::Number(match op {
        BinaryOp::Mul => x * y,
        BinaryOp::Div => x / y,
        _ => x % y,
    }))
}
