//! Opt-in checks against real vaults. Read-only; prints only counts.
//!
//! ```sh
//! IGNEOUS_CORPUS=/path/to/vault-copy:/another cargo test -p igneous-query --release -- --ignored
//! ```

use std::path::Path;
use std::time::Instant;

use igneous_core::VaultPath;
use igneous_query::NoteData;
use igneous_query::bases::{BaseFile, ViewResult, ViewState, run};
use igneous_query::search::{Matcher, SearchOptions};

fn files(root: &Path, dir: &Path, out: &mut Vec<(VaultPath, std::path::PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            files(root, &path, out);
        } else if let Ok(vault_path) = VaultPath::from_fs(root, &path) {
            out.push((vault_path, path));
        }
    }
}

fn load(root: &Path) -> Vec<NoteData> {
    let mut found = Vec::new();
    files(root, root, &mut found);
    found
        .into_iter()
        .map(|(vault_path, path)| {
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            let mut note = if vault_path.extension() == Some("md") {
                let text = std::fs::read(&path)
                    .ok()
                    .and_then(|raw| igneous_core::TextFile::from_bytes(&raw).ok())
                    .map(|file| file.text().to_owned())
                    .unwrap_or_default();
                NoteData::from_text(vault_path, text)
            } else {
                NoteData::new(vault_path)
            };
            note.size = size;
            note
        })
        .collect()
}

#[test]
#[ignore = "needs IGNEOUS_CORPUS"]
fn corpus_searches_and_bases_run() {
    let Ok(corpus) = std::env::var("IGNEOUS_CORPUS") else {
        eprintln!("IGNEOUS_CORPUS not set; skipping");
        return;
    };
    let searches = [
        "the",
        "meeting OR project -draft",
        "tag:#project",
        "[tags]",
        "[created:>2020]",
        "line:(the a)",
        "block:(to do)",
        "section:(the and)",
        "task-todo:the",
        r"/\d{4}-\d{2}-\d{2}/",
        "file:.png",
    ];
    for dir in corpus.split(':').filter(|d| !d.is_empty()) {
        let root = Path::new(dir);
        let notes = load(root);
        let start = Instant::now();
        let mut hits = 0;
        for search in searches {
            let matcher = Matcher::parse(search, SearchOptions::default()).unwrap();
            hits += notes
                .iter()
                .filter(|n| matcher.matches(n).is_some())
                .count();
        }
        eprintln!(
            "{} files, {} searches, {hits} hits, {:?}",
            notes.len(),
            searches.len(),
            start.elapsed()
        );

        for note in notes.iter().filter(|n| n.path.extension() == Some("base")) {
            let source = std::fs::read_to_string(note.path.to_fs(root)).unwrap();
            let base = BaseFile::parse(&source).expect("a corpus base doesn't parse");
            assert!(
                base.problems.is_empty(),
                "a corpus base has problems: {:?}",
                base.problems
            );
            for (i, view) in base.views.iter().enumerate() {
                let state = ViewState {
                    order: Some(view.order.clone()),
                    column_sizes: Some(view.column_sizes.clone()),
                    sort: Some(view.sort.clone()),
                };
                assert_eq!(base.set_view_state(i, &state), Ok(Vec::new()));
                match run(&base, i, &notes, Some(note)) {
                    ViewResult::Ready(data) => {
                        assert!(data.errors.is_empty(), "view errors: {:?}", data.errors);
                        let failed = data
                            .rows
                            .iter()
                            .flat_map(|r| &r.cells)
                            .filter(|c| c.is_err())
                            .count();
                        eprintln!(
                            "base view {i}: {} rows, {} columns, {failed} failed cells",
                            data.rows.len(),
                            data.columns.len()
                        );
                        assert_eq!(failed, 0);
                    }
                    other => panic!("view {i} didn't run: {other:?}"),
                }
            }
        }
    }
}
