//! Frontmatter values stored as JSON.
//!
//! Scalars and lists map onto JSON directly. A map becomes `{"map": [[key,
//! value], …]}` so its order survives; nothing else is ever stored as a JSON
//! object, so decoding is unambiguous.

use igneous_markdown::Value;
use serde_json::{Value as Json, json};

pub fn encode(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => json!(i),
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::String(s) => Json::String(s.clone()),
        Value::List(items) => Json::Array(items.iter().map(encode).collect()),
        Value::Map(entries) => json!({
            "map": entries
                .iter()
                .map(|(k, v)| json!([k, encode(v)]))
                .collect::<Vec<_>>()
        }),
    }
}

pub fn decode(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) if !n.is_f64() => Value::Int(i),
            _ => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => Value::String(s.clone()),
        Json::Array(items) => Value::List(items.iter().map(decode).collect()),
        Json::Object(object) => Value::Map(
            object
                .get("map")
                .and_then(Json::as_array)
                .map(|pairs| {
                    pairs
                        .iter()
                        .filter_map(|pair| {
                            let [k, v] = pair.as_array()?.as_slice() else {
                                return None;
                            };
                            Some((k.as_str()?.to_owned(), decode(v)))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
    }
}

/// Whether `s` is a date (`YYYY-MM-DD`).
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Whether `s` is a date and time (`YYYY-MM-DDTHH:MM`, optionally with
/// seconds, fractions and an offset; a space may replace the `T`).
pub fn is_datetime(s: &str) -> bool {
    if s.len() < 16 || !s.is_char_boundary(10) || !is_date(&s[..10]) {
        return false;
    }
    let b = s.as_bytes();
    matches!(b[10], b'T' | b' ')
        && b[11].is_ascii_digit()
        && b[12].is_ascii_digit()
        && b[13] == b':'
        && b[14].is_ascii_digit()
        && b[15].is_ascii_digit()
        && s[16..]
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | '+' | '-' | 'Z' | 'z'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let values = [
            Value::Null,
            Value::Bool(true),
            Value::Int(-3),
            Value::Float(1.0),
            Value::Float(2.5),
            Value::String("x".into()),
            Value::List(vec![Value::Int(1), Value::String("a".into())]),
            Value::Map(vec![
                ("z".into(), Value::Int(1)),
                ("a".into(), Value::List(vec![])),
            ]),
        ];
        for value in values {
            assert_eq!(decode(&encode(&value)), value);
        }
    }

    #[test]
    fn dates() {
        assert!(is_date("2026-10-04"));
        assert!(!is_date("2026-1-04"));
        assert!(is_datetime("2026-10-04T09:30"));
        assert!(is_datetime("2026-10-04 09:30:12+01:00"));
        assert!(!is_datetime("2026-10-04"));
        assert!(!is_datetime("2026-10-04Tnope"));
    }
}
