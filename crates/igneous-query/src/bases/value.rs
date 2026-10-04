//! Values in Bases formulas and cells.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::Arc;

use igneous_core::VaultPath;
use igneous_core::path::{loose_key, natural_cmp};
use jiff::civil::{Date, DateTime};
use jiff::{Span, Unit};
use regex::Regex;

/// A value in a formula, a filter or a table cell.
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Date(Date),
    /// A date and time, in the local time zone.
    DateTime(DateTime),
    Duration(Span),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
    Link(Link),
    File(VaultPath),
    Regex(RegexValue),
}

/// A link to a note, a file or a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The target as written: a note name, a path or a URL.
    pub target: String,
    pub display: Option<String>,
    /// The file it resolves to, if any.
    pub path: Option<VaultPath>,
}

/// A regular expression literal such as `/a.c/g`.
#[derive(Debug, Clone)]
pub struct RegexValue {
    pub regex: Arc<Regex>,
    /// The `g` flag: replace every match rather than the first.
    pub global: bool,
}

impl Value {
    /// The type name used by `isType()`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Date(_) | Value::DateTime(_) => "date",
            Value::Duration(_) => "duration",
            Value::List(_) => "list",
            Value::Object(_) => "object",
            Value::Link(_) => "link",
            Value::File(_) => "file",
            Value::Regex(_) => "regexp",
        }
    }

    /// Truthiness, as in JavaScript except that empty lists and objects are
    /// false (so a filter on an empty list property behaves as expected).
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Object(entries) => !entries.is_empty(),
            Value::Date(_)
            | Value::DateTime(_)
            | Value::Duration(_)
            | Value::Link(_)
            | Value::File(_)
            | Value::Regex(_) => true,
        }
    }

    /// Null, an empty string, an empty list or an empty object.
    pub fn is_empty(&self) -> bool {
        match self {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::List(items) => items.is_empty(),
            Value::Object(entries) => entries.is_empty(),
            _ => false,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::String(s) => s.trim().parse().ok(),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    /// The value as a date and time: dates at midnight, and strings that
    /// look like dates.
    pub fn as_datetime(&self) -> Option<DateTime> {
        match self {
            Value::Date(d) => Some(d.to_datetime(jiff::civil::Time::midnight())),
            Value::DateTime(dt) => Some(*dt),
            Value::String(s) => match parse_date(s)? {
                Value::Date(d) => Some(d.to_datetime(jiff::civil::Time::midnight())),
                Value::DateTime(dt) => Some(dt),
                _ => None,
            },
            _ => None,
        }
    }

    fn is_date(&self) -> bool {
        matches!(self, Value::Date(_) | Value::DateTime(_))
    }

    /// The text shown for the value, used by `toString()` and when joining
    /// strings with `+`.
    pub fn display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_number(*n),
            Value::String(s) => s.clone(),
            Value::Date(d) => d.strftime("%Y-%m-%d").to_string(),
            Value::DateTime(dt) => {
                if dt.second() == 0 && dt.subsec_nanosecond() == 0 {
                    dt.strftime("%Y-%m-%d %H:%M").to_string()
                } else {
                    dt.strftime("%Y-%m-%d %H:%M:%S").to_string()
                }
            }
            Value::Duration(span) => format!("{span:#}"),
            Value::List(items) => {
                let mut out = String::new();
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&item.display());
                }
                out
            }
            Value::Object(entries) => {
                let mut out = String::from("{");
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "{key}: {}", value.display());
                }
                out.push('}');
                out
            }
            Value::Link(link) => link.display_text(),
            Value::File(path) => file_title(path),
            Value::Regex(re) => {
                format!(
                    "/{}/{}",
                    re.regex.as_str(),
                    if re.global { "g" } else { "" }
                )
            }
        }
    }
}

impl Link {
    /// The text shown for the link: its display text, or the target's name.
    pub fn display_text(&self) -> String {
        if let Some(display) = &self.display {
            return display.clone();
        }
        match &self.path {
            Some(path) => file_title(path),
            None => self.target.clone(),
        }
    }

    /// Whether the link points at `path`.
    pub fn points_to(&self, path: &VaultPath) -> bool {
        match &self.path {
            Some(p) => p == path,
            None => {
                let target = loose_key(self.target.trim_end_matches(".md"));
                target == loose_key(path.as_str().trim_end_matches(".md"))
                    || target == loose_key(path.stem())
            }
        }
    }
}

/// A file's name, without `.md` for notes.
pub(crate) fn file_title(path: &VaultPath) -> String {
    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("md") => path.stem().to_owned(),
        _ => path.file_name().to_owned(),
    }
}

/// Formats a number as JavaScript does: `3`, `0.5`, `NaN`, `Infinity`.
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n == 0.0 {
        "0".into()
    } else {
        n.to_string()
    }
}

/// Parses `YYYY-MM-DD`, or a date and time such as `2025-01-01 12:00`,
/// `2025-01-01T12:00:00` or `2025-01-01T12:00:00Z` (converted to local time).
pub fn parse_date(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.len() < 10 || !text.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    if text.len() == 10 {
        return text.parse::<Date>().ok().map(Value::Date);
    }
    let normalised = text.replacen(' ', "T", 1);
    if let Ok(dt) = normalised.parse::<DateTime>() {
        return Some(Value::DateTime(dt));
    }
    // With an offset or `Z`: an instant, shown in local time.
    normalised
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|ts| Value::DateTime(jiff::tz::TimeZone::system().to_datetime(ts)))
}

/// Parses a duration such as `1d`, `2 weeks`, `1M 4h` or `-3 days`.
///
/// Units: `y`/`year(s)`, `M`/`month(s)`, `w`/`week(s)`, `d`/`day(s)`,
/// `h`/`hour(s)`, `m`/`minute(s)`, `s`/`second(s)`, `ms`/`millisecond(s)`.
pub fn parse_duration(text: &str) -> Option<Span> {
    let mut rest = text.trim();
    let negative = rest.starts_with('-');
    rest = rest.trim_start_matches(['-', '+']).trim_start();
    if rest.is_empty() {
        return None;
    }
    let mut span = Span::new();
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let amount: f64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();
        let unit_len = rest
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = rest[unit_len..].trim_start_matches([' ', ',']);
        let whole = amount.trunc() as i64;
        let fraction = amount.fract();
        span = match unit {
            "y" | "year" | "years" => span.try_years(span.get_years() as i64 + whole).ok()?,
            "M" | "month" | "months" => span.try_months(span.get_months() as i64 + whole).ok()?,
            "w" | "week" | "weeks" => span.try_weeks(span.get_weeks() as i64 + whole).ok()?,
            "d" | "day" | "days" => span.try_days(span.get_days() as i64 + whole).ok()?,
            "h" | "hour" | "hours" => span.try_hours(span.get_hours() as i64 + whole).ok()?,
            "m" | "minute" | "minutes" => span.try_minutes(span.get_minutes() + whole).ok()?,
            "s" | "second" | "seconds" => span.try_seconds(span.get_seconds() + whole).ok()?,
            "ms" | "millisecond" | "milliseconds" => span
                .try_milliseconds(span.get_milliseconds() + whole)
                .ok()?,
            _ => return None,
        };
        if fraction != 0.0 {
            // Only whole units are supported; "1.5h" becomes 1h 30m.
            let millis = match unit {
                "d" | "day" | "days" => 86_400_000.0,
                "h" | "hour" | "hours" => 3_600_000.0,
                "m" | "minute" | "minutes" => 60_000.0,
                "s" | "second" | "seconds" => 1000.0,
                _ => return None,
            } * fraction;
            span = span
                .try_milliseconds(span.get_milliseconds() + millis.round() as i64)
                .ok()?;
        }
    }
    Some(if negative { span.negate() } else { span })
}

/// A duration in milliseconds. Months and years are measured from the start
/// of 2000, so the result is the same everywhere.
pub fn duration_millis(span: &Span) -> Option<f64> {
    span.total((Unit::Millisecond, Date::constant(2000, 1, 1)))
        .ok()
}

/// Whether a duration has parts smaller than a day.
pub(crate) fn has_time(span: &Span) -> bool {
    span.get_hours() != 0
        || span.get_minutes() != 0
        || span.get_seconds() != 0
        || span.get_milliseconds() != 0
        || span.get_microseconds() != 0
        || span.get_nanoseconds() != 0
}

/// Orders values for comparison operators. `None` when they can't be
/// compared, which makes `<`, `>` and friends false.
pub fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Duration(x), Value::Duration(y)) => {
            duration_millis(x)?.partial_cmp(&duration_millis(y)?)
        }
        (Value::Duration(x), Value::Number(y)) => duration_millis(x)?.partial_cmp(y),
        (Value::Number(x), Value::Duration(y)) => x.partial_cmp(&duration_millis(y)?),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
            a.as_number()?.partial_cmp(&b.as_number()?)
        }
        _ if a.is_date() || b.is_date() => Some(a.as_datetime()?.cmp(&b.as_datetime()?)),
        _ => None,
    }
}

/// `==`: like [`compare`] for numbers, text and dates; links equal files and
/// other links that point to the same place.
pub fn equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Link(x), Value::Link(y)) => match (&x.path, &y.path) {
            (Some(p), Some(q)) => p == q,
            _ => loose_key(&x.target) == loose_key(&y.target),
        },
        (Value::Link(link), Value::File(path)) | (Value::File(path), Value::Link(link)) => {
            link.points_to(path)
        }
        (Value::Link(link), Value::String(s)) | (Value::String(s), Value::Link(link)) => {
            loose_key(&link.target) == loose_key(s) || link.display.as_deref() == Some(s.as_str())
        }
        (Value::File(x), Value::File(y)) => x == y,
        (Value::File(path), Value::String(s)) | (Value::String(s), Value::File(path)) => {
            path.as_str() == s
        }
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equals(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && equals(v, v2)))
        }
        (Value::Regex(x), Value::Regex(y)) => {
            x.regex.as_str() == y.regex.as_str() && x.global == y.global
        }
        _ => compare(a, b) == Some(Ordering::Equal),
    }
}

/// A total order for sorting rows: by type first (booleans, numbers, dates,
/// durations, text, lists, objects), then by value. Text sorts naturally
/// ("Note 2" before "Note 10"). Callers put nulls last themselves.
pub fn sort_order(a: &Value, b: &Value) -> Ordering {
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Bool(_) => 0,
            Value::Number(_) => 1,
            Value::Date(_) | Value::DateTime(_) => 2,
            Value::Duration(_) => 3,
            Value::String(_) | Value::Link(_) | Value::File(_) | Value::Regex(_) => 4,
            Value::List(_) => 5,
            Value::Object(_) => 6,
            Value::Null => 7,
        }
    }
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.total_cmp(y),
        (Value::List(x), Value::List(y)) => x
            .iter()
            .zip(y)
            .map(|(a, b)| sort_order(a, b))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| x.len().cmp(&y.len())),
        _ => match compare(a, b) {
            Some(ordering) if !matches!((a, b), (Value::String(_), Value::String(_))) => ordering,
            _ => natural_cmp(&a.display(), &b.display()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let span = parse_duration("1M 4h 3m").unwrap();
        assert_eq!(
            (span.get_months(), span.get_hours(), span.get_minutes()),
            (1, 4, 3)
        );
        assert_eq!(parse_duration("2 weeks").unwrap().get_weeks(), 2);
        assert_eq!(parse_duration("-3 days").unwrap().get_days(), -3);
        assert_eq!(
            parse_duration("1.5h").unwrap().get_milliseconds(),
            1_800_000
        );
        assert!(parse_duration("soon").is_none());
        assert!(parse_duration("").is_none());
        assert_eq!(
            duration_millis(&parse_duration("1d").unwrap()),
            Some(86_400_000.0)
        );
    }

    #[test]
    fn dates() {
        assert!(matches!(parse_date("2025-01-31"), Some(Value::Date(_))));
        assert!(matches!(
            parse_date("2025-01-31 12:00:00"),
            Some(Value::DateTime(_))
        ));
        assert!(matches!(
            parse_date("2025-01-31T12:00"),
            Some(Value::DateTime(_))
        ));
        assert!(matches!(
            parse_date("2025-01-31T12:00:00Z"),
            Some(Value::DateTime(_))
        ));
        assert!(parse_date("hello world").is_none());
        assert!(parse_date("2025-13-01").is_none());
        assert_eq!(
            parse_date("2025-01-31 08:05").unwrap().display(),
            "2025-01-31 08:05"
        );
    }

    #[test]
    fn numbers_print_like_javascript() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(format_number(f64::INFINITY), "Infinity");
    }

    #[test]
    fn comparisons_coerce_dates_and_numbers() {
        let date = parse_date("2025-01-31").unwrap();
        let s = |s: &str| Value::String(s.into());
        assert_eq!(compare(&date, &s("2025-02-01")), Some(Ordering::Less));
        assert!(equals(&date, &s("2025-01-31")));
        assert_eq!(compare(&Value::Number(5.0), &s("10")), Some(Ordering::Less));
        assert!(compare(&Value::Number(5.0), &s("ten")).is_none());
        assert!(!equals(&Value::Null, &s("")));
        let link = Value::Link(Link {
            target: "Home".into(),
            display: None,
            path: Some(VaultPath::new("Home.md").unwrap()),
        });
        assert!(equals(
            &link,
            &Value::File(VaultPath::new("Home.md").unwrap())
        ));
        assert!(equals(&link, &s("home")));
    }

    #[test]
    fn sorting_is_natural_and_typed() {
        let mut values = [
            Value::String("Note 10".into()),
            Value::Number(2.0),
            Value::String("note 2".into()),
            Value::Number(-1.0),
        ];
        values.sort_by(sort_order);
        let shown: Vec<_> = values.iter().map(Value::display).collect();
        assert_eq!(shown, ["-1", "2", "note 2", "Note 10"]);
    }
}
