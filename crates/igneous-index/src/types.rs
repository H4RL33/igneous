//! Property types, inferred from the values a property has across the vault.

use std::collections::BTreeMap;

use igneous_core::settings::PropertyType;
use igneous_markdown::Value;

use crate::value::{is_date, is_datetime};

/// The type of the property `key`, given every value it has in the vault.
///
/// An override wins. Then `tags`, `aliases` and `cssclasses` are known by
/// name. Otherwise the values decide: all booleans make a checkbox, all
/// numbers a number, all dates a date (a datetime if any has a time), lists
/// (or lists mixed with single strings) a list, and anything else text.
/// Empty values are ignored.
pub fn infer<'a>(
    key: &str,
    values: impl IntoIterator<Item = &'a Value>,
    overrides: &BTreeMap<String, PropertyType>,
) -> PropertyType {
    if let Some(ty) = overrides.get(key) {
        return *ty;
    }
    match key.to_lowercase().as_str() {
        "tags" | "tag" => return PropertyType::Tags,
        "aliases" | "alias" => return PropertyType::Aliases,
        "cssclasses" | "cssclass" => return PropertyType::List,
        _ => {}
    }
    let values: Vec<&Value> = values
        .into_iter()
        .filter(|v| !matches!(v, Value::Null))
        .filter(|v| !matches!(v, Value::String(s) if s.trim().is_empty()))
        .collect();
    if values.is_empty() {
        return PropertyType::Text;
    }
    let all = |f: fn(&Value) -> bool| values.iter().all(|v| f(v));
    if all(|v| matches!(v, Value::Bool(_))) {
        PropertyType::Checkbox
    } else if all(|v| matches!(v, Value::Int(_) | Value::Float(_))) {
        PropertyType::Number
    } else if all(|v| matches!(v, Value::String(s) if is_date(s))) {
        PropertyType::Date
    } else if all(|v| matches!(v, Value::String(s) if is_date(s) || is_datetime(s))) {
        PropertyType::Datetime
    } else if all(|v| matches!(v, Value::List(_) | Value::String(_)))
        && values.iter().any(|v| matches!(v, Value::List(_)))
    {
        PropertyType::List
    } else {
        PropertyType::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Value {
        Value::String(x.into())
    }

    fn ty(key: &str, values: &[Value]) -> PropertyType {
        infer(key, values, &BTreeMap::new())
    }

    #[test]
    fn inference() {
        assert_eq!(
            ty("done", &[Value::Bool(true), Value::Null]),
            PropertyType::Checkbox
        );
        assert_eq!(
            ty("n", &[Value::Int(1), Value::Float(2.5)]),
            PropertyType::Number
        );
        assert_eq!(
            ty("created", &[s("2026-10-04"), s("2026-01-01")]),
            PropertyType::Date
        );
        assert_eq!(
            ty("at", &[s("2026-10-04"), s("2026-10-04T10:00")]),
            PropertyType::Datetime
        );
        assert_eq!(
            ty("up", &[Value::List(vec![s("a")]), s("b")]),
            PropertyType::List
        );
        assert_eq!(ty("author", &[s("me"), Value::Int(3)]), PropertyType::Text);
        assert_eq!(ty("empty", &[Value::Null, s(" ")]), PropertyType::Text);
        assert_eq!(ty("tags", &[s("x")]), PropertyType::Tags);
        assert_eq!(ty("Aliases", &[]), PropertyType::Aliases);
        assert_eq!(ty("cssclasses", &[s("wide")]), PropertyType::List);
    }

    #[test]
    fn overrides_win() {
        let overrides = BTreeMap::from([("created".to_owned(), PropertyType::Text)]);
        assert_eq!(
            infer("created", &[s("2026-10-04")], &overrides),
            PropertyType::Text
        );
    }
}
