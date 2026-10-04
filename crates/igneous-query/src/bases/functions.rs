//! The Bases function catalogue.
//!
//! Functions follow Obsidian's documented behaviour
//! (<https://help.obsidian.md/bases/functions>). Anything missing returns
//! [`EvalError::Unsupported`], which the view shows in the affected cell.
//!
//! Not supported: `html()`, `image()` and `icon()`, which need rich cells.
//! Added beyond Obsidian's list, for custom summaries: `list.mean()`,
//! `list.sum()`, `list.min()`, `list.max()` and `list.median()`.

use std::collections::HashMap;
use std::sync::LazyLock;

use igneous_core::VaultPath;
use igneous_core::path::loose_key;

use super::eval::{Eval, EvalError, invalid};
use super::expr::Expr;
use super::value::{Link, Value, equals, format_number, parse_date, parse_duration, sort_order};

pub(crate) type Function = fn(&mut Eval<'_>, &Value, &[Expr]) -> Result<Value, EvalError>;

/// Every function and method a formula can call.
pub struct Registry {
    globals: HashMap<&'static str, Function>,
    /// Receiver type (`string`, `list`, …, or `any`) → name → function.
    methods: HashMap<&'static str, HashMap<&'static str, Function>>,
}

static STANDARD: LazyLock<Registry> = LazyLock::new(Registry::build);

impl Registry {
    /// The built-in catalogue.
    pub fn standard() -> &'static Registry {
        &STANDARD
    }

    pub(crate) fn global(&self, name: &str) -> Option<Function> {
        self.globals.get(name).copied()
    }

    pub(crate) fn method(&self, type_name: &str, name: &str) -> Option<Function> {
        self.methods.get(type_name)?.get(name).copied()
    }

    /// Global function names, sorted, for completion.
    pub fn global_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.globals.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// Methods callable on a value of `type_name` (as `isType()` names it),
    /// sorted, including those every value has.
    pub fn method_names(&self, type_name: &str) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = [type_name, "any"]
            .iter()
            .filter_map(|t| self.methods.get(t))
            .flat_map(|m| m.keys().copied())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    fn build() -> Registry {
        let mut r = Registry {
            globals: HashMap::new(),
            methods: HashMap::new(),
        };
        let globals: [(&'static str, Function); 16] = [
            ("date", g_date),
            ("duration", g_duration),
            ("escapeHTML", g_escape_html),
            ("file", g_file),
            ("html", |_, _, _| {
                Err(EvalError::Unsupported("html()".into()))
            }),
            ("icon", |_, _, _| {
                Err(EvalError::Unsupported("icon()".into()))
            }),
            ("if", g_if),
            ("image", |_, _, _| {
                Err(EvalError::Unsupported("image()".into()))
            }),
            ("link", g_link),
            ("list", g_list),
            ("max", g_max),
            ("min", g_min),
            ("now", g_now),
            ("number", g_number),
            ("random", g_random),
            ("today", g_today),
        ];
        r.globals.extend(globals);

        let mut add = |ty: &'static str, name: &'static str, f: Function| {
            r.methods.entry(ty).or_default().insert(name, f);
        };
        // Any value
        add("any", "isTruthy", |ev, v, args| {
            no_args(ev, args)?;
            Ok(Value::Bool(v.is_truthy()))
        });
        add("any", "isType", any_is_type);
        add("any", "toString", |ev, v, args| {
            no_args(ev, args)?;
            Ok(Value::String(v.display()))
        });
        add("any", "isEmpty", |ev, v, args| {
            no_args(ev, args)?;
            Ok(Value::Bool(v.is_empty()))
        });

        // Dates
        add("date", "date", date_date);
        add("date", "format", date_format);
        add("date", "time", date_time);
        add("date", "relative", date_relative);

        // Strings
        add("string", "contains", |ev, v, args| {
            let needle = arg(ev, args, 0)?.display();
            Ok(Value::Bool(v.display().contains(&needle)))
        });
        add("string", "containsAll", |ev, v, args| {
            let s = v.display();
            let all = all_args(ev, args)?;
            Ok(Value::Bool(all.iter().all(|n| s.contains(&n.display()))))
        });
        add("string", "containsAny", |ev, v, args| {
            let s = v.display();
            let all = all_args(ev, args)?;
            Ok(Value::Bool(all.iter().any(|n| s.contains(&n.display()))))
        });
        add("string", "startsWith", |ev, v, args| {
            let prefix = arg(ev, args, 0)?.display();
            Ok(Value::Bool(v.display().starts_with(&prefix)))
        });
        add("string", "endsWith", |ev, v, args| {
            let suffix = arg(ev, args, 0)?.display();
            Ok(Value::Bool(v.display().ends_with(&suffix)))
        });
        add("string", "lower", |_, v, _| {
            Ok(Value::String(v.display().to_lowercase()))
        });
        add("string", "upper", |_, v, _| {
            Ok(Value::String(v.display().to_uppercase()))
        });
        add("string", "title", |_, v, _| {
            Ok(Value::String(title_case(&v.display())))
        });
        add("string", "trim", |_, v, _| {
            Ok(Value::String(v.display().trim().to_owned()))
        });
        add("string", "reverse", |_, v, _| {
            Ok(Value::String(v.display().chars().rev().collect()))
        });
        add("string", "repeat", |ev, v, args| {
            let count = number_arg(ev, args, 0)?.unwrap_or(0.0);
            if !(0.0..=10_000.0).contains(&count) {
                return Err(invalid("repeat() needs a count between 0 and 10000"));
            }
            Ok(Value::String(v.display().repeat(count as usize)))
        });
        add("string", "slice", string_slice);
        add("string", "split", string_split);
        add("string", "replace", string_replace);

        // Numbers
        add("number", "abs", |_, v, _| number_op(v, f64::abs));
        add("number", "ceil", |_, v, _| number_op(v, f64::ceil));
        add("number", "floor", |_, v, _| number_op(v, f64::floor));
        add("number", "round", number_round);
        add("number", "toFixed", number_to_fixed);

        // Lists
        add("list", "contains", |ev, v, args| {
            let needle = arg(ev, args, 0)?;
            Ok(Value::Bool(
                list(v).iter().any(|item| equals(item, &needle)),
            ))
        });
        add("list", "containsAll", |ev, v, args| {
            let needles = all_args(ev, args)?;
            let items = list(v);
            Ok(Value::Bool(
                needles.iter().all(|n| items.iter().any(|i| equals(i, n))),
            ))
        });
        add("list", "containsAny", |ev, v, args| {
            let needles = all_args(ev, args)?;
            let items = list(v);
            Ok(Value::Bool(
                needles.iter().any(|n| items.iter().any(|i| equals(i, n))),
            ))
        });
        add("list", "filter", list_filter);
        add("list", "map", list_map);
        add("list", "reduce", list_reduce);
        add("list", "flat", |_, v, _| {
            Ok(Value::List(
                list(v)
                    .iter()
                    .flat_map(|item| match item {
                        Value::List(inner) => inner.clone(),
                        other => vec![other.clone()],
                    })
                    .collect(),
            ))
        });
        add("list", "join", |ev, v, args| {
            let separator = match arg(ev, args, 0)? {
                Value::Null => ",".to_owned(),
                s => s.display(),
            };
            Ok(Value::String(
                list(v)
                    .iter()
                    .map(Value::display)
                    .collect::<Vec<_>>()
                    .join(&separator),
            ))
        });
        add("list", "reverse", |_, v, _| {
            Ok(Value::List(list(v).iter().rev().cloned().collect()))
        });
        add("list", "slice", list_slice);
        add("list", "sort", |_, v, _| {
            let mut items = list(v).to_vec();
            items.sort_by(|a, b| match (a, b) {
                (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
                (Value::Null, _) => std::cmp::Ordering::Greater,
                (_, Value::Null) => std::cmp::Ordering::Less,
                _ => sort_order(a, b),
            });
            Ok(Value::List(items))
        });
        add("list", "unique", |_, v, _| {
            let mut out: Vec<Value> = Vec::new();
            for item in list(v) {
                if !out.iter().any(|seen| equals(seen, item)) {
                    out.push(item.clone());
                }
            }
            Ok(Value::List(out))
        });
        add("list", "mean", |_, v, _| {
            let numbers = numbers(v);
            Ok(if numbers.is_empty() {
                Value::Null
            } else {
                Value::Number(numbers.iter().sum::<f64>() / numbers.len() as f64)
            })
        });
        add("list", "sum", |_, v, _| {
            Ok(Value::Number(numbers(v).iter().sum()))
        });
        add("list", "min", |_, v, _| {
            Ok(numbers(v)
                .into_iter()
                .reduce(f64::min)
                .map_or(Value::Null, Value::Number))
        });
        add("list", "max", |_, v, _| {
            Ok(numbers(v)
                .into_iter()
                .reduce(f64::max)
                .map_or(Value::Null, Value::Number))
        });
        add("list", "median", |_, v, _| {
            Ok(median(numbers(v)).map_or(Value::Null, Value::Number))
        });

        // Links
        add("link", "asFile", |ev, v, _| {
            let Value::Link(link) = v else {
                return Ok(Value::Null);
            };
            Ok(link_path(ev, link).map_or(Value::Null, Value::File))
        });
        add("link", "linksTo", |ev, v, args| {
            let Value::Link(link) = v else {
                return Ok(Value::Bool(false));
            };
            let target = arg(ev, args, 0)?;
            let source = link_path(ev, link).and_then(|p| ev.env.files.get(&p));
            Ok(Value::Bool(
                source.is_some_and(|note| has_link(ev, note, &target)),
            ))
        });

        // Files
        add("file", "asLink", |ev, v, args| {
            let Value::File(path) = v else {
                return Ok(Value::Null);
            };
            let display = match arg(ev, args, 0)? {
                Value::Null => None,
                d => Some(d.display()),
            };
            Ok(Value::Link(Link {
                target: path.as_str().to_owned(),
                display,
                path: Some(path.clone()),
            }))
        });
        add("file", "hasTag", |ev, v, args| {
            let tags: Vec<String> = all_args(ev, args)?
                .iter()
                .flat_map(|t| match t {
                    Value::List(items) => items.iter().map(Value::display).collect(),
                    other => vec![other.display()],
                })
                .map(|t| t.trim_start_matches('#').to_lowercase())
                .collect();
            let Some(note) = file_note(ev, v) else {
                return Ok(Value::Bool(false));
            };
            Ok(Value::Bool(note.tags.iter().any(|tag| {
                let tag = tag.to_lowercase();
                tags.iter().any(|t| {
                    tag == *t
                        || tag
                            .strip_prefix(t.as_str())
                            .is_some_and(|r| r.starts_with('/'))
                })
            })))
        });
        add("file", "hasLink", |ev, v, args| {
            let target = arg(ev, args, 0)?;
            Ok(Value::Bool(
                file_note(ev, v).is_some_and(|note| has_link(ev, note, &target)),
            ))
        });
        add("file", "hasProperty", |ev, v, args| {
            let name = arg(ev, args, 0)?.display();
            Ok(Value::Bool(
                file_note(ev, v).is_some_and(|note| note.property(&name).is_some()),
            ))
        });
        add("file", "inFolder", |ev, v, args| {
            let folder = arg(ev, args, 0)?.display();
            let folder = folder.trim_matches('/');
            let Value::File(path) = v else {
                return Ok(Value::Bool(false));
            };
            if folder.is_empty() {
                return Ok(Value::Bool(true));
            }
            let parent = path.parent().map(|p| p.loose_key()).unwrap_or_default();
            let folder = loose_key(folder);
            Ok(Value::Bool(
                parent == folder || parent.starts_with(&format!("{folder}/")),
            ))
        });

        // Objects
        add("object", "keys", |_, v, _| {
            let Value::Object(entries) = v else {
                return Ok(Value::Null);
            };
            Ok(Value::List(
                entries
                    .iter()
                    .map(|(k, _)| Value::String(k.clone()))
                    .collect(),
            ))
        });
        add("object", "values", |_, v, _| {
            let Value::Object(entries) = v else {
                return Ok(Value::Null);
            };
            Ok(Value::List(
                entries.iter().map(|(_, v)| v.clone()).collect(),
            ))
        });

        // Regular expressions
        add("regexp", "matches", |ev, v, args| {
            let Value::Regex(re) = v else {
                return Ok(Value::Bool(false));
            };
            let text = arg(ev, args, 0)?.display();
            Ok(Value::Bool(re.regex.is_match(&text)))
        });

        r
    }
}

// --- helpers ----------------------------------------------------------------

fn arg(ev: &mut Eval<'_>, args: &[Expr], i: usize) -> Result<Value, EvalError> {
    match args.get(i) {
        Some(expr) => ev.eval(expr),
        None => Ok(Value::Null),
    }
}

fn all_args(ev: &mut Eval<'_>, args: &[Expr]) -> Result<Vec<Value>, EvalError> {
    args.iter().map(|a| ev.eval(a)).collect()
}

fn no_args(_: &mut Eval<'_>, args: &[Expr]) -> Result<(), EvalError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(invalid("this function takes no arguments"))
    }
}

fn number_arg(ev: &mut Eval<'_>, args: &[Expr], i: usize) -> Result<Option<f64>, EvalError> {
    match arg(ev, args, i)? {
        Value::Null => Ok(None),
        v => v
            .as_number()
            .map(Some)
            .ok_or_else(|| invalid(format!("expected a number, not {}", v.display()))),
    }
}

fn list(value: &Value) -> &[Value] {
    match value {
        Value::List(items) => items,
        _ => &[],
    }
}

fn numbers(value: &Value) -> Vec<f64> {
    list(value)
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) if !n.is_nan() => Some(*n),
            _ => None,
        })
        .collect()
}

pub(crate) fn median(mut numbers: Vec<f64>) -> Option<f64> {
    if numbers.is_empty() {
        return None;
    }
    numbers.sort_by(f64::total_cmp);
    let mid = numbers.len() / 2;
    Some(if numbers.len() % 2 == 0 {
        (numbers[mid - 1] + numbers[mid]) / 2.0
    } else {
        numbers[mid]
    })
}

fn number_op(value: &Value, f: fn(f64) -> f64) -> Result<Value, EvalError> {
    match value {
        Value::Number(n) => Ok(Value::Number(f(*n))),
        _ => Ok(Value::Null),
    }
}

/// JavaScript's `Math.round`: halves round up.
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

/// A JavaScript `slice` index: negative counts from the end.
fn slice_index(index: Option<f64>, len: usize, default: usize) -> usize {
    match index {
        None => default,
        Some(i) if i < 0.0 => len.saturating_sub((-i) as usize),
        Some(i) => (i as usize).min(len),
    }
}

fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start_of_word = true;
    for c in text.chars() {
        if start_of_word && c.is_alphabetic() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start_of_word = c.is_whitespace();
    }
    out
}

/// The note behind a `File` value.
fn file_note<'a>(ev: &Eval<'a>, value: &Value) -> Option<&'a crate::NoteData> {
    match value {
        Value::File(path) => ev.env.files.get(path),
        _ => None,
    }
}

fn link_path(ev: &Eval<'_>, link: &Link) -> Option<VaultPath> {
    link.path.clone().or_else(|| {
        ev.env
            .files
            .resolve(&link.target, ev.row.map(|row| &row.path))
    })
}

/// Whether `note` links to `target`: a file, a link or a path or name.
fn has_link(ev: &Eval<'_>, note: &crate::NoteData, target: &Value) -> bool {
    let wanted = match target {
        Value::File(path) => Some(path.clone()),
        Value::Link(link) => link_path(ev, link),
        Value::Null => return false,
        other => ev.env.files.resolve(&other.display(), Some(&note.path)),
    };
    let text = match target {
        Value::Link(link) => link.target.clone(),
        other => other.display(),
    };
    note.links.iter().any(|link| match (&link.path, &wanted) {
        (Some(have), Some(want)) => have == want,
        _ => loose_key(&link.target) == loose_key(&text),
    })
}

// --- global functions ---------------------------------------------------------

fn g_date(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    match arg(ev, args, 0)? {
        v @ (Value::Date(_) | Value::DateTime(_) | Value::Null) => Ok(v),
        v => parse_date(&v.display())
            .ok_or_else(|| invalid(format!("\"{}\" isn't a date", v.display()))),
    }
}

fn g_duration(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    match arg(ev, args, 0)? {
        v @ (Value::Duration(_) | Value::Null) => Ok(v),
        v => parse_duration(&v.display())
            .map(Value::Duration)
            .ok_or_else(|| invalid(format!("\"{}\" isn't a duration", v.display()))),
    }
}

fn g_escape_html(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let text = arg(ev, args, 0)?.display();
    Ok(Value::String(
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;"),
    ))
}

fn g_file(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let from = ev.row.map(|row| &row.path);
    Ok(match arg(ev, args, 0)? {
        v @ Value::File(_) => v,
        Value::Link(link) => link_path(ev, &link).map_or(Value::Null, Value::File),
        Value::Null => Value::Null,
        other => {
            let text = other.display();
            VaultPath::new(&text)
                .ok()
                .filter(|p| ev.env.files.get(p).is_some())
                .or_else(|| ev.env.files.resolve(&text, from))
                .map_or(Value::Null, Value::File)
        }
    })
}

fn g_if(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    if arg(ev, args, 0)?.is_truthy() {
        arg(ev, args, 1)
    } else {
        arg(ev, args, 2)
    }
}

fn g_link(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let target = arg(ev, args, 0)?;
    let display = match arg(ev, args, 1)? {
        Value::Null => None,
        d => Some(d.display()),
    };
    Ok(match target {
        Value::File(path) => Value::Link(Link {
            target: path.as_str().to_owned(),
            display,
            path: Some(path),
        }),
        Value::Link(mut link) => {
            if display.is_some() {
                link.display = display;
            }
            Value::Link(link)
        }
        Value::Null => Value::Null,
        other => {
            let text = other.display();
            let inner = text
                .trim()
                .strip_prefix("[[")
                .and_then(|t| t.strip_suffix("]]"))
                .unwrap_or(text.trim());
            let (target, alias) = match inner.split_once('|') {
                Some((t, a)) => (t.trim(), Some(a.trim().to_owned())),
                None => (inner, None),
            };
            let path = ev.env.files.resolve(target, ev.row.map(|row| &row.path));
            Value::Link(Link {
                target: target.to_owned(),
                display: display.or(alias),
                path,
            })
        }
    })
}

fn g_list(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    Ok(match arg(ev, args, 0)? {
        v @ Value::List(_) => v,
        Value::Null => Value::List(Vec::new()),
        v => Value::List(vec![v]),
    })
}

fn flat_numbers(values: Vec<Value>) -> Vec<f64> {
    values
        .into_iter()
        .flat_map(|v| match v {
            Value::List(items) => items,
            other => vec![other],
        })
        .filter_map(|v| match v {
            Value::Number(n) if !n.is_nan() => Some(n),
            _ => None,
        })
        .collect()
}

fn g_max(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    Ok(flat_numbers(all_args(ev, args)?)
        .into_iter()
        .reduce(f64::max)
        .map_or(Value::Null, Value::Number))
}

fn g_min(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    Ok(flat_numbers(all_args(ev, args)?)
        .into_iter()
        .reduce(f64::min)
        .map_or(Value::Null, Value::Number))
}

fn g_now(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    no_args(ev, args)?;
    Ok(Value::DateTime(ev.env.now.datetime()))
}

fn g_today(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    no_args(ev, args)?;
    Ok(Value::Date(ev.env.now.date()))
}

fn g_number(ev: &mut Eval<'_>, _: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let value = arg(ev, args, 0)?;
    Ok(match &value {
        Value::Null => Value::Null,
        Value::Number(_) => value,
        Value::Bool(b) => Value::Number(f64::from(u8::from(*b))),
        Value::Date(_) | Value::DateTime(_) => {
            let dt = value.as_datetime().expect("a date");
            let ts = ev
                .env
                .tz()
                .to_timestamp(dt)
                .map_err(|e| invalid(e.to_string()))?;
            Value::Number(ts.as_millisecond() as f64)
        }
        Value::Duration(span) => {
            super::value::duration_millis(span).map_or(Value::Null, Value::Number)
        }
        other => Value::Number(
            other
                .display()
                .trim()
                .parse()
                .map_err(|_| invalid(format!("\"{}\" isn't a number", other.display())))?,
        ),
    })
}

fn g_random(ev: &mut Eval<'_>, _: &Value, _: &[Expr]) -> Result<Value, EvalError> {
    use std::hash::{Hash, Hasher};
    // Stable for a row within one run of the view, as in Obsidian.
    let mut hasher = std::hash::DefaultHasher::new();
    ev.env.now.timestamp().as_nanosecond().hash(&mut hasher);
    ev.row.map(|row| row.path.as_str()).hash(&mut hasher);
    Ok(Value::Number(
        (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64,
    ))
}

// --- methods -----------------------------------------------------------------

fn any_is_type(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let wanted = arg(ev, args, 0)?.display().to_lowercase();
    Ok(Value::Bool(v.type_name() == wanted))
}

fn date_date(_: &mut Eval<'_>, v: &Value, _: &[Expr]) -> Result<Value, EvalError> {
    Ok(match v {
        Value::Date(d) => Value::Date(*d),
        Value::DateTime(dt) => Value::Date(dt.date()),
        _ => Value::Null,
    })
}

fn date_format(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let pattern = match arg(ev, args, 0)? {
        Value::Null => "YYYY-MM-DD".to_owned(),
        p => p.display(),
    };
    let Some(dt) = v.as_datetime() else {
        return Ok(Value::Null);
    };
    let zoned = dt
        .to_zoned(ev.env.tz())
        .map_err(|e| invalid(e.to_string()))?;
    Ok(Value::String(igneous_core::datefmt::format(
        &zoned, &pattern,
    )))
}

fn date_time(_: &mut Eval<'_>, v: &Value, _: &[Expr]) -> Result<Value, EvalError> {
    Ok(match v {
        Value::Date(_) => Value::String("00:00:00".into()),
        Value::DateTime(dt) => Value::String(dt.time().strftime("%H:%M:%S").to_string()),
        _ => Value::Null,
    })
}

fn date_relative(ev: &mut Eval<'_>, v: &Value, _: &[Expr]) -> Result<Value, EvalError> {
    let Some(dt) = v.as_datetime() else {
        return Ok(Value::Null);
    };
    let now = ev.env.now.datetime();
    let seconds = now.duration_since(dt).as_secs_f64();
    Ok(Value::String(relative(seconds)))
}

/// Moment's `fromNow()`: "a few seconds ago", "in 3 days", "2 months ago".
pub(crate) fn relative(seconds: f64) -> String {
    let past = seconds >= 0.0;
    let s = seconds.abs();
    let minutes = s / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    let amount = |n: f64, one: &str, many: &str| {
        let n = js_round(n).max(1.0);
        if n == 1.0 {
            one.to_owned()
        } else {
            format!("{} {many}", format_number(n))
        }
    };
    let text = if s < 45.0 {
        "a few seconds".to_owned()
    } else if s < 90.0 {
        "a minute".to_owned()
    } else if minutes < 45.0 {
        amount(minutes, "a minute", "minutes")
    } else if minutes < 90.0 {
        "an hour".to_owned()
    } else if hours < 22.0 {
        amount(hours, "an hour", "hours")
    } else if hours < 36.0 {
        "a day".to_owned()
    } else if days < 26.0 {
        amount(days, "a day", "days")
    } else if days < 45.0 {
        "a month".to_owned()
    } else if days < 320.0 {
        amount(days / 30.4, "a month", "months")
    } else if days < 548.0 {
        "a year".to_owned()
    } else {
        amount(days / 365.0, "a year", "years")
    };
    if past {
        format!("{text} ago")
    } else {
        format!("in {text}")
    }
}

fn string_slice(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let chars: Vec<char> = v.display().chars().collect();
    let start = slice_index(number_arg(ev, args, 0)?, chars.len(), 0);
    let end = slice_index(number_arg(ev, args, 1)?, chars.len(), chars.len());
    Ok(Value::String(if start < end {
        chars[start..end].iter().collect()
    } else {
        String::new()
    }))
}

fn list_slice(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let items = list(v);
    let start = slice_index(number_arg(ev, args, 0)?, items.len(), 0);
    let end = slice_index(number_arg(ev, args, 1)?, items.len(), items.len());
    Ok(Value::List(if start < end {
        items[start..end].to_vec()
    } else {
        Vec::new()
    }))
}

fn string_split(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let text = v.display();
    let separator = arg(ev, args, 0)?;
    let limit = number_arg(ev, args, 1)?.map(|n| n.max(0.0) as usize);
    let parts: Vec<String> = match &separator {
        Value::Regex(re) => re.regex.split(&text).map(str::to_owned).collect(),
        Value::Null => vec![text],
        sep => {
            let sep = sep.display();
            if sep.is_empty() {
                text.chars().map(String::from).collect()
            } else {
                text.split(sep.as_str()).map(str::to_owned).collect()
            }
        }
    };
    Ok(Value::List(
        parts
            .into_iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(Value::String)
            .collect(),
    ))
}

fn string_replace(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let text = v.display();
    let pattern = arg(ev, args, 0)?;
    let replacement = arg(ev, args, 1)?.display();
    Ok(Value::String(match &pattern {
        Value::Regex(re) => {
            let replacement = js_replacement(&replacement);
            if re.global {
                re.regex
                    .replace_all(&text, replacement.as_str())
                    .into_owned()
            } else {
                re.regex.replace(&text, replacement.as_str()).into_owned()
            }
        }
        // A text pattern replaces every occurrence.
        p => text.replace(&p.display(), &replacement),
    }))
}

/// Converts JavaScript's `$1`, `$&` and `$<name>` to the regex crate's
/// `${1}`, `${0}` and `${name}`.
fn js_replacement(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some('&') => {
                chars.next();
                out.push_str("${0}");
            }
            Some(d) if d.is_ascii_digit() => {
                let mut number = String::new();
                while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                    number.push(d);
                    chars.next();
                    if number.len() == 2 {
                        break;
                    }
                }
                out.push_str(&format!("${{{number}}}"));
            }
            Some('<') => {
                chars.next();
                let name: String = chars.by_ref().take_while(|&c| c != '>').collect();
                out.push_str(&format!("${{{name}}}"));
            }
            _ => out.push_str("$$"),
        }
    }
    out
}

fn number_round(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let Value::Number(n) = v else {
        return Ok(Value::Null);
    };
    let digits = number_arg(ev, args, 0)?.unwrap_or(0.0).clamp(0.0, 15.0) as i32;
    let scale = 10f64.powi(digits);
    Ok(Value::Number(js_round(n * scale) / scale))
}

fn number_to_fixed(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let Value::Number(n) = v else {
        return Ok(Value::Null);
    };
    let digits = number_arg(ev, args, 0)?.unwrap_or(0.0).clamp(0.0, 100.0) as usize;
    Ok(Value::String(format!("{n:.digits$}")))
}

fn list_filter(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let Some(predicate) = args.first() else {
        return Err(invalid(
            "filter() needs a condition, such as filter(value > 2)",
        ));
    };
    let mut out = Vec::new();
    for (i, item) in list(v).iter().enumerate() {
        let vars = vec![("value", item.clone()), ("index", Value::Number(i as f64))];
        if ev.with_locals(vars, |ev| ev.eval(predicate))?.is_truthy() {
            out.push(item.clone());
        }
    }
    Ok(Value::List(out))
}

fn list_map(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let Some(mapping) = args.first() else {
        return Err(invalid("map() needs an expression, such as map(value + 1)"));
    };
    let mut out = Vec::new();
    for (i, item) in list(v).iter().enumerate() {
        let vars = vec![("value", item.clone()), ("index", Value::Number(i as f64))];
        out.push(ev.with_locals(vars, |ev| ev.eval(mapping))?);
    }
    Ok(Value::List(out))
}

fn list_reduce(ev: &mut Eval<'_>, v: &Value, args: &[Expr]) -> Result<Value, EvalError> {
    let Some(step) = args.first() else {
        return Err(invalid(
            "reduce() needs an expression, such as reduce(acc + value, 0)",
        ));
    };
    let mut acc = arg(ev, args, 1)?;
    for (i, item) in list(v).iter().enumerate() {
        let vars = vec![
            ("value", item.clone()),
            ("index", Value::Number(i as f64)),
            ("acc", acc),
        ];
        acc = ev.with_locals(vars, |ev| ev.eval(step))?;
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(relative(10.0), "a few seconds ago");
        assert_eq!(relative(-60.0), "in a minute");
        assert_eq!(relative(3.0 * 3600.0), "3 hours ago");
        assert_eq!(relative(3.0 * 86400.0), "3 days ago");
        assert_eq!(relative(-40.0 * 86400.0), "in a month");
        assert_eq!(relative(3.0 * 365.0 * 86400.0), "3 years ago");
    }

    #[test]
    fn replacement_syntax() {
        assert_eq!(js_replacement("$2, $1"), "${2}, ${1}");
        assert_eq!(js_replacement("$&!"), "${0}!");
        assert_eq!(js_replacement("$$5"), "$$5");
        assert_eq!(js_replacement("$<year>-x"), "${year}-x");
    }

    #[test]
    fn title_case_capitalises_words() {
        assert_eq!(title_case("hello wide world"), "Hello Wide World");
    }

    #[test]
    fn catalogue_lists_names() {
        let r = Registry::standard();
        assert!(r.global_names().contains(&"today"));
        let methods = r.method_names("string");
        assert!(methods.contains(&"lower") && methods.contains(&"isTruthy"));
    }
}
