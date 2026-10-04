//! The linter as a whole: order, idempotency, protection, settings.

use std::path::Path;

use igneous_core::VaultPath;
use igneous_lint::text::apply;
use igneous_lint::{Category, FileInfo, LintSettings, Linter, Options, RuleConfig, rules};
use jiff::Zoned;
use jiff::tz::TimeZone;
use proptest::prelude::*;
use serde_json::{Map, Value};

fn path() -> VaultPath {
    VaultPath::new("Notes/Note.md").unwrap()
}

fn now() -> Zoned {
    "2026-10-04T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .to_zoned(TimeZone::UTC)
}

/// Every rule on with its default options, except `yaml-timestamp`, which
/// depends on the clock.
fn everything() -> LintSettings {
    let mut settings = LintSettings::default();
    settings.rules.clear();
    for rule in rules::all() {
        if rule.id() == "yaml-timestamp" {
            continue;
        }
        settings.rules.insert(
            rule.id().to_owned(),
            RuleConfig {
                enabled: true,
                options: Map::new(),
            },
        );
    }
    settings
}

fn lint(linter: &Linter, text: &str) -> String {
    let file = FileInfo {
        created: Some(now()),
        modified: Some(now()),
    };
    let result = linter.lint_with(text, &path(), &now(), &file);
    assert_eq!(
        apply(text, &result.edits),
        result.text,
        "edits match the text"
    );
    result.text
}

fn fixture_befores() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let cases: Vec<Value> =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            out.extend(
                cases
                    .iter()
                    .map(|c| c["before"].as_str().unwrap().to_owned()),
            );
        }
    }
    out
}

#[test]
fn rules_run_in_obsidian_linters_order() {
    let linter = Linter::new(&everything());
    let ids: Vec<&str> = linter.rules().map(|r| r.id()).collect();
    assert_eq!(
        &ids[..3],
        [
            "format-tags-in-yaml",
            "escape-yaml-special-characters",
            "move-math-block-indicators-to-their-own-line"
        ]
    );
    assert_eq!(
        &ids[ids.len() - 5..],
        [
            "blockquote-style",
            "force-yaml-escape",
            "trailing-spaces",
            "consecutive-blank-lines",
            "add-blank-line-after-yaml",
        ]
    );
    // The rest by category, then ID.
    let middle: Vec<_> = linter.rules().skip(3).take(ids.len() - 8).collect();
    for pair in middle.windows(2) {
        assert!(
            (pair[0].category(), pair[0].id()) < (pair[1].category(), pair[1].id()),
            "{} before {}",
            pair[0].id(),
            pair[1].id()
        );
    }
    assert_eq!(middle.first().unwrap().category(), Category::Yaml);
    assert_eq!(middle.last().unwrap().category(), Category::Spacing);
}

#[test]
fn linting_twice_changes_nothing_more() {
    let linter = Linter::new(&everything());
    let mut unstable = Vec::new();
    for before in fixture_befores() {
        let once = lint(&linter, &before);
        let twice = lint(&linter, &once);
        if once != twice {
            unstable.push(format!("{before:?}\n  once:  {once:?}\n  twice: {twice:?}"));
        }
    }
    assert!(unstable.is_empty(), "{}", unstable.join("\n"));
}

#[test]
fn each_rule_alone_is_idempotent() {
    let befores = fixture_befores();
    let mut unstable = Vec::new();
    for rule in rules::all() {
        let mut settings = LintSettings::default();
        settings.rules.clear();
        settings.rules.insert(
            rule.id().to_owned(),
            RuleConfig {
                enabled: true,
                options: Map::new(),
            },
        );
        let linter = Linter::new(&settings);
        for before in &befores {
            let once = lint(&linter, before);
            let twice = lint(&linter, &once);
            if once != twice {
                unstable.push(format!("{}: {before:?} → {once:?} → {twice:?}", rule.id()));
            }
        }
    }
    assert!(unstable.is_empty(), "{}", unstable.join("\n"));
}

#[test]
fn protected_regions_are_never_touched() {
    let protected = [
        "```\ncode  with   spaces   \n*emph*   _emph_\n\n\n\n- [ ]   item\n```",
        "`inline   code  `",
        // `move-math-block-indicators-to-their-own-line` may move a block's
        // delimiters; this one's are already in place.
        "$$\nx  =   y   \n\n\n\nz\n$$",
        "$a   +  b$",
        "<% tp.date.now(\"YYYY\")   %>",
        "<!-- linter-disable -->\n#   Heading.\n*a*   __b__   \n\n\n\n<!-- linter-enable -->",
    ];
    let note = format!(
        "---\ntags: [a, a]\n---\n# Title.\n\n{}\n\n*mixed*  and   _styles_   \n",
        protected.join("\n\n")
    );
    let linter = Linter::new(&everything());
    let out = lint(&linter, &note);
    for region in protected {
        if !out.contains(region) {
            let file = FileInfo {
                created: None,
                modified: None,
            };
            let (path, now) = (path(), now());
            let env = igneous_lint::Env {
                now: &now,
                path: &path,
                file: &file,
                already_modified: false,
            };
            let culprits: Vec<&str> = rules::all()
                .iter()
                .filter(|rule| {
                    let options = Options::resolve(rule.options(), None, &Map::new());
                    let edits = igneous_lint::run_rule(**rule, &options, &note, &env);
                    !apply(&note, &edits).contains(region)
                })
                .map(|rule| rule.id())
                .collect();
            panic!("{region:?} changed by {culprits:?}:\n{out}");
        }
    }
    // Something outside them did change.
    assert_ne!(out, note);
}

#[test]
fn options_round_trip_through_json() {
    for rule in rules::all() {
        let options = Options::resolve(rule.options(), None, &Map::new());
        let settings = LintSettings {
            rules: [(
                rule.id().to_owned(),
                RuleConfig {
                    enabled: true,
                    options: options.as_map().clone(),
                },
            )]
            .into(),
            ..LintSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: LintSettings = serde_json::from_str(&json).unwrap();
        let again = Options::resolve(rule.options(), None, &back.rules[rule.id()].options);
        assert_eq!(options, again, "{}", rule.id());
        for spec in rule.options() {
            assert!(options.as_map().contains_key(spec.key), "{}", spec.key);
        }
    }
}

#[test]
fn shared_styles_fill_in_rule_options() {
    let mut settings = everything();
    settings.extra.insert(
        "commonStyles".into(),
        serde_json::json!({ "aliasArrayStyle": "multi-line", "escapeCharacter": "'" }),
    );
    let linter = Linter::new(&settings);
    let out = lint(&linter, "---\naliases: [one, two]\n---\n");
    assert_eq!(out, "---\naliases:\n  - one\n  - two\n---\n");
}

#[test]
fn diagnostics_point_at_the_original_text() {
    let settings = igneous_lint::recommended();
    let linter = Linter::new(&settings);
    let text = "a  \n\n\n\nb\t";
    let file = FileInfo {
        created: None,
        modified: None,
    };
    let result = linter.lint_with(text, &path(), &now(), &file);
    assert_eq!(result.text, "a\n\nb\n");
    let mut found: Vec<(&str, &str)> = result
        .diagnostics
        .iter()
        .map(|d| (d.rule, &text[d.range.clone()]))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("consecutive-blank-lines", "\n\n\n\n"),
            ("line-break-at-document-end", ""),
            ("trailing-spaces", "\t"),
            ("trailing-spaces", "  "),
        ]
    );
}

#[test]
fn invalid_yaml_is_reported_with_every_rule_off() {
    let mut settings = LintSettings::default();
    settings.rules.clear();
    let linter = Linter::new(&settings);
    let result = linter.lint("---\nkey: [unclosed\n---\nbody\n", &path());
    assert!(!result.changed());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].rule, "yaml");
}

#[test]
fn ignored_paths_and_disabled_rules() {
    let mut settings = igneous_lint::recommended();
    settings.ignore_folders = vec!["Notes".into()];
    let linter = Linter::new(&settings);
    assert!(linter.lint("a  \n", &path()).skipped);
    let other = VaultPath::new("Elsewhere/Note.md").unwrap();
    assert_eq!(linter.lint("a  \n", &other).text, "a\n");
    let notes_like = VaultPath::new("NotesExtra/Note.md").unwrap();
    assert!(!linter.lint("a\n", &notes_like).skipped);

    settings.ignore_folders.clear();
    settings.ignore_files = vec!["Notes/Note.md".into()];
    assert!(Linter::new(&settings).lint("a  \n", &path()).skipped);

    let linter = Linter::new(&igneous_lint::recommended());
    let text = "---\ndisabled rules: [trailing-spaces]\n---\na  \n\n\n\nb\n";
    assert_eq!(
        linter.lint(text, &path()).text,
        "---\ndisabled rules: [trailing-spaces]\n---\na  \n\nb\n"
    );
    let all = "---\ndisabled rules: all\n---\na  \n";
    assert!(linter.lint(all, &path()).skipped);
}

#[test]
fn yaml_timestamp_settles() {
    let mut settings = LintSettings::default();
    settings.rules.clear();
    settings.rules.insert(
        "yaml-timestamp".into(),
        RuleConfig {
            enabled: true,
            options: Map::new(),
        },
    );
    let linter = Linter::new(&settings);
    let first = lint(&linter, "# Note\n");
    assert!(first.starts_with("---\ndate created: Sunday, October 4th 2026, 12:00:00 pm\n"));
    assert_eq!(lint(&linter, &first), first);
}

/// Markdown-ish text built from pieces rules care about.
fn note() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("# Heading.".to_owned()),
        Just("### Skipped level!".to_owned()),
        Just("  ## indented".to_owned()),
        Just("- item".to_owned()),
        Just("* other  item".to_owned()),
        Just("- - double".to_owned()),
        Just("-".to_owned()),
        Just("1. first".to_owned()),
        Just("3) third".to_owned()),
        Just("- [ ]   task".to_owned()),
        Just("> quote".to_owned()),
        Just(">quote".to_owned()),
        Just("> > nested".to_owned()),
        Just("> [!note] Callout".to_owned()),
        Just("```\ncode  \n```".to_owned()),
        Just("~~~\nmore\n~~~".to_owned()),
        Just("$$\nx  = 1\n$$".to_owned()),
        Just("text $$y$$ more".to_owned()),
        Just("| a | b |\n| - | - |\n| 1 | 2 |".to_owned()),
        Just("***".to_owned()),
        Just("---".to_owned()),
        Just("*emph* and __strong__".to_owned()),
        Just("hyph- enated".to_owned()),
        Just("[[Link]] and [md](x.md) #tag".to_owned()),
        Just("<% tp %>".to_owned()),
        Just("<!-- linter-disable -->".to_owned()),
        Just("<!-- linter-enable -->".to_owned()),
        Just(String::new()),
        Just("   ".to_owned()),
        Just("word\t".to_owned()),
        "[a-z ]{0,12}",
    ];
    let yaml = prop_oneof![
        Just(String::new()),
        Just("---\ntags: [a, b, a]\naliases: x\n---\n".to_owned()),
        Just("---\n\ntitle: a: b\n\n---\n".to_owned()),
        Just("---\nlist:\n  - one\n  - one\n---\n".to_owned()),
    ];
    (yaml, prop::collection::vec(piece, 0..12), "[\n]{0,3}")
        .prop_map(|(yaml, pieces, end)| format!("{yaml}{}{end}", pieces.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn linting_settles_and_never_panics(text in note()) {
        let linter = Linter::new(&everything());
        let once = lint(&linter, &text);
        let twice = lint(&linter, &once);
        prop_assert_eq!(&once, &twice, "from {:?}", text);
    }
}

/// Opt-in: lints every note under the paths in `IGNEOUS_CORPUS` (read-only)
/// and checks linting never panics and settles after one pass.
#[test]
#[ignore = "needs IGNEOUS_CORPUS"]
fn corpus_settles() {
    let Ok(roots) = std::env::var("IGNEOUS_CORPUS") else {
        return;
    };
    let linter = Linter::new(&everything());
    let mut notes = 0;
    let mut changed = 0;
    let mut unstable = 0;
    for root in std::env::split_paths(&roots) {
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "md") {
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let text = text.replace("\r\n", "\n");
                    notes += 1;
                    let once = lint(&linter, &text);
                    if once != text {
                        changed += 1;
                    }
                    if lint(&linter, &once) != once {
                        unstable += 1;
                        eprintln!("not idempotent: note #{notes}");
                    }
                }
            }
        }
    }
    eprintln!("{notes} notes, {changed} would change, {unstable} unstable");
    assert_eq!(unstable, 0);
}
