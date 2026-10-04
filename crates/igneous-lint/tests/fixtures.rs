//! Before/after cases ported from obsidian-linter (see fixtures/README.md).

use std::path::Path;

use igneous_core::VaultPath;
use igneous_lint::text::apply;
use igneous_lint::{Category, Env, FileInfo, Options, Rule, rules, run_rule};
use jiff::Zoned;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde_json::{Map, Value};

/// Cases that behave differently in Igneous, and why.
const KNOWN_DIFFERENCES: &[(&str, &str, &str)] = &[];

/// Options obsidian-linter passes to a rule from outside its settings.
const RUN_KEYS: &[&str] = &[
    "fileCreatedTime",
    "fileModifiedTime",
    "currentTime",
    "alreadyModified",
    "locale",
    "fileName",
];

struct Case {
    name: String,
    before: String,
    after: String,
    options: Map<String, Value>,
    example: bool,
}

fn cases(rule: &str) -> Vec<Case> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{rule}.json"));
    let json: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    json.into_iter()
        .map(|case| Case {
            name: case["name"].as_str().unwrap().to_owned(),
            before: case["before"].as_str().unwrap().to_owned(),
            after: case["after"].as_str().unwrap().to_owned(),
            options: case["options"].as_object().cloned().unwrap_or_default(),
            example: case["source"] == "example",
        })
        .collect()
}

fn time(value: Option<&Value>) -> Option<Zoned> {
    let text = match value? {
        Value::String(s) => s.as_str(),
        Value::Object(o) => o.get("$time")?.as_str()?,
        _ => return None,
    };
    if let Some(time) = igneous_lint::moment::parse_iso(text, &TimeZone::UTC) {
        return Some(time);
    }
    let civil: DateTime = text.parse().ok()?;
    civil.to_zoned(TimeZone::UTC).ok()
}

/// Runs `rule` the way obsidian-linter's tests do: the rule alone, with the
/// case's options over the rule's defaults.
fn run(rule: &dyn Rule, before: &str, options: &Map<String, Value>) -> String {
    let configured: Map<String, Value> = options
        .iter()
        .filter(|(k, _)| !RUN_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let resolved = Options::resolve(rule.options(), None, &configured);
    let fallback = "2020-01-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap();
    let now = time(options.get("currentTime")).unwrap_or_else(|| fallback.to_zoned(TimeZone::UTC));
    let file = FileInfo {
        created: time(options.get("fileCreatedTime")),
        modified: time(options.get("fileModifiedTime")),
    };
    let path = VaultPath::new("Note.md").unwrap();
    let env = Env {
        now: &now,
        path: &path,
        file: &file,
        already_modified: options
            .get("alreadyModified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let edits = run_rule(rule, &resolved, before, &env);
    apply(before, &edits)
}

#[test]
fn obsidian_linter_fixtures() {
    let mut failures = Vec::new();
    let mut fixed = Vec::new();
    let mut total = 0;
    for rule in rules::all() {
        for case in cases(rule.id()) {
            total += 1;
            let known = KNOWN_DIFFERENCES
                .iter()
                .any(|(id, name, _)| *id == rule.id() && *name == case.name);
            let out = run(*rule, &case.before, &case.options);
            let mut ok = out == case.after;
            // obsidian-linter also checks examples still work below a YAML block.
            const YAML: &str = "---\nfoo: bar\n---\n";
            if ok
                && case.example
                && rule.category() != Category::Yaml
                && !case.before.starts_with("---\n")
            {
                let augmented = run(*rule, &format!("{YAML}{}", case.before), &case.options);
                ok = augmented.contains(&format!("{YAML}{}", case.after))
                    || augmented.contains(&format!("{YAML}\n{}", case.after));
            }
            match (ok, known) {
                (false, false) => failures.push(format!(
                    "{} :: {}\n  before: {:?}\n  wanted: {:?}\n  got:    {:?}",
                    rule.id(),
                    case.name,
                    case.before,
                    case.after,
                    out
                )),
                (true, true) => fixed.push(format!("{} :: {}", rule.id(), case.name)),
                _ => {}
            }
        }
    }
    eprintln!("{total} fixtures, {} failing", failures.len());
    assert!(
        failures.is_empty(),
        "{} of {total} fixtures failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        fixed.is_empty(),
        "now passing, remove from KNOWN_DIFFERENCES:\n{}",
        fixed.join("\n")
    );
}

#[test]
fn every_rule_has_fixtures() {
    for rule in rules::all() {
        assert!(!cases(rule.id()).is_empty(), "{}", rule.id());
    }
}
