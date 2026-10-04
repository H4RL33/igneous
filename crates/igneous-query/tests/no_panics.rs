//! Parsers and matchers must accept anything a user can type.

use igneous_core::VaultPath;
use igneous_query::NoteData;
use igneous_query::bases::{BaseFile, RunOptions, ViewResult, evaluate, run_with};
use igneous_query::search::{Matcher, SearchOptions};
use proptest::prelude::*;

const NOTE: &str = "---\ntags: [a, b/c]\nn: 3\n---\n# H\n- [ ] task one\n- [x] two\n\nline (with) \"quotes\" #tag é\n";

fn syntax() -> impl Strategy<Value = String> {
    // Mostly syntax characters, so the parsers' edge cases come up often.
    proptest::string::string_regex(r#"[a-z0-9 :()\[\]"/\\\-#<>=.,'!&|+*%{}^$é]{0,40}"#).unwrap()
}

proptest! {
    #[test]
    fn searches_never_panic(search in syntax()) {
        let note = NoteData::from_text(VaultPath::new("Folder/Note é.md").unwrap(), NOTE);
        if let Ok(matcher) = Matcher::parse(&search, SearchOptions::default()) {
            if let Some(hit) = matcher.matches(&note) {
                for range in &hit.content {
                    prop_assert!(note.text.is_char_boundary(range.start));
                    prop_assert!(note.text.is_char_boundary(range.end));
                }
            }
        }
    }

    #[test]
    fn expressions_never_panic(expression in syntax()) {
        let notes = [NoteData::from_text(VaultPath::new("Note.md").unwrap(), NOTE)];
        let options = RunOptions { now: "2025-06-15T12:00:00[UTC]".parse().unwrap() };
        let _ = evaluate(&expression, None, &notes, Some(&notes[0]), None, &options);
    }

    #[test]
    fn bases_never_panic(filter in syntax(), order in syntax()) {
        let source = format!(
            "views:\n  - type: table\n    filters:\n      and:\n        - {filter:?}\n    order:\n      - {order:?}\n"
        );
        if let Ok(base) = BaseFile::parse(&source) {
            let notes = [NoteData::from_text(VaultPath::new("Note.md").unwrap(), NOTE)];
            let options = RunOptions { now: "2025-06-15T12:00:00[UTC]".parse().unwrap() };
            prop_assert!(matches!(run_with(&base, 0, &notes, None, &options), ViewResult::Ready(_)));
        }
    }
}
