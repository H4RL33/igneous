//! After any sequence of changes, applying the watcher's events leaves the
//! index exactly as a full rebuild would.

use std::path::Path;

use igneous_core::watch::VaultEvent;
use igneous_core::{Vault, VaultPath};
use igneous_index::Index;
use proptest::prelude::*;

const FILES: &[&str] = &[
    "A.md",
    "B.md",
    "Dir/C.md",
    "Dir/D.md",
    "Dir/Sub/E.md",
    "Other/F.md",
    "img.png",
    "Dir/img.png",
    "board.canvas",
    ".hidden/G.md",
];
const FOLDERS: &[&str] = &["Dir", "Other", "Moved", "Dir/Sub", "Archive/Old"];
const CONTENTS: &[&str] = &[
    "[[A]] and #tag\n",
    "---\ntags: [x, y/z]\nup: \"[[B]]\"\naliases: [Bee]\n---\n# Heading\n[[C]] ![[img.png]]\n",
    "plain text, no links\n",
    "[c](Dir/C.md) ^block\n- [ ] a task\n- [x] done\n",
    "",
    "[[Dir/Sub/E#Part|alias]] and [[Missing]] and https://example.com\n",
];

#[derive(Debug, Clone)]
enum Op {
    Write { file: usize, content: usize },
    Delete { file: usize },
    Rename { from: usize, to: usize },
    MoveFolder { from: usize, to: usize },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..FILES.len(), 0..CONTENTS.len()).prop_map(|(file, content)| Op::Write { file, content }),
        1 => (0..FILES.len()).prop_map(|file| Op::Delete { file }),
        2 => (0..FILES.len(), 0..FILES.len()).prop_map(|(from, to)| Op::Rename { from, to }),
        1 => (0..FOLDERS.len(), 0..FOLDERS.len()).prop_map(|(from, to)| Op::MoveFolder { from, to }),
    ]
}

fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

/// Performs `op` on the disk and returns what the watcher would report.
fn perform(root: &Path, op: &Op) -> Vec<VaultEvent> {
    match *op {
        Op::Write { file, content } => {
            let path = root.join(FILES[file]);
            let existed = path.exists();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, CONTENTS[content]).unwrap();
            let path = p(FILES[file]);
            vec![if existed {
                VaultEvent::Modified(path)
            } else {
                VaultEvent::Created(path)
            }]
        }
        Op::Delete { file } => {
            if std::fs::remove_file(root.join(FILES[file])).is_err() {
                return Vec::new();
            }
            vec![VaultEvent::Removed(p(FILES[file]))]
        }
        Op::Rename { from, to } => {
            let (src, dest) = (root.join(FILES[from]), root.join(FILES[to]));
            if from == to || !src.is_file() || dest.exists() {
                return Vec::new();
            }
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::rename(src, dest).unwrap();
            vec![VaultEvent::Renamed {
                from: p(FILES[from]),
                to: p(FILES[to]),
            }]
        }
        Op::MoveFolder { from, to } => {
            let (src, dest) = (root.join(FOLDERS[from]), root.join(FOLDERS[to]));
            if !src.is_dir() || dest.exists() || dest.starts_with(&src) {
                return Vec::new();
            }
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::rename(src, dest).unwrap();
            vec![VaultEvent::Renamed {
                from: p(FOLDERS[from]),
                to: p(FOLDERS[to]),
            }]
        }
    }
}

fn rebuilt(vault: &Vault) -> Index {
    let mut index = Index::in_memory().unwrap();
    index.reconcile(vault).unwrap();
    index
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn events_keep_the_index_equal_to_a_rebuild(
        start in proptest::collection::vec((0..FILES.len(), 0..CONTENTS.len()), 0..6),
        ops in proptest::collection::vec(op(), 1..14),
    ) {
        let dir = tempfile::tempdir().unwrap();
        for (file, content) in start {
            let path = dir.path().join(FILES[file]);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, CONTENTS[content]).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        let mut index = rebuilt(&vault);
        for op in &ops {
            let events = perform(dir.path(), op);
            index.apply(&vault, &events).unwrap();
        }
        let fresh = rebuilt(&vault);
        prop_assert_eq!(index.snapshot().unwrap(), fresh.snapshot().unwrap());
        let files: Vec<_> = index.files().iter().cloned().collect();
        let expected: Vec<_> = fresh.files().iter().cloned().collect();
        prop_assert_eq!(files, expected);
    }
}

#[test]
fn a_folder_arriving_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut index = rebuilt(&vault);
    std::fs::create_dir_all(dir.path().join("In/Deep")).unwrap();
    std::fs::write(dir.path().join("In/One.md"), "[[Two]]\n").unwrap();
    std::fs::write(dir.path().join("In/Deep/Two.md"), "two\n").unwrap();
    index
        .apply(&vault, &[VaultEvent::Created(p("In"))])
        .unwrap();
    assert_eq!(
        index.snapshot().unwrap(),
        rebuilt(&vault).snapshot().unwrap()
    );
    assert_eq!(index.backlinks(&p("In/Deep/Two.md")).unwrap().len(), 1);

    std::fs::remove_dir_all(dir.path().join("In")).unwrap();
    index
        .apply(&vault, &[VaultEvent::Removed(p("In"))])
        .unwrap();
    assert!(index.files().is_empty());
}

#[test]
fn renaming_to_another_kind_reindexes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Note.md"), "[[Other]] #tag\n").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut index = rebuilt(&vault);
    std::fs::rename(dir.path().join("Note.md"), dir.path().join("Note.txt")).unwrap();
    index
        .apply(
            &vault,
            &[VaultEvent::Renamed {
                from: p("Note.md"),
                to: p("Note.txt"),
            }],
        )
        .unwrap();
    assert_eq!(
        index.snapshot().unwrap(),
        rebuilt(&vault).snapshot().unwrap()
    );
    assert!(index.tags().unwrap().is_empty());
}
