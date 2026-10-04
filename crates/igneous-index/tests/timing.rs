//! Timings on a synthetic 10,000-note vault. Run in release:
//! `cargo test -p igneous-index --release --test timing -- --ignored --nocapture`

use std::time::{Duration, Instant};

use igneous_core::watch::VaultEvent;
use igneous_core::{Vault, VaultPath};
use igneous_index::Index;

const NOTES: usize = 10_000;

fn path(i: usize) -> String {
    format!("Folder {}/Note {i}.md", i % 50)
}

fn note(i: usize, extra: &str) -> String {
    let links: Vec<String> = (1..=5)
        .map(|k| format!("[[Note {}]]", (i * 7 + k * 131) % NOTES))
        .collect();
    format!(
        "---\ncreated: 2026-10-04\ntags: [topic/{}, synthetic]\nrating: {}\n---\n\
         # Note {i}\n\nSome text about topic {} with {}.\n\n## Details\n\n\
         More words here, #tag{} and a task:\n- [ ] do thing {i} ^b{i}\n{extra}",
        i % 20,
        i % 5,
        i % 20,
        links.join(", "),
        i % 30
    )
}

#[test]
#[ignore = "timing; run in release"]
fn ten_thousand_notes() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..NOTES {
        let p = dir.path().join(path(i));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, note(i, "")).unwrap();
    }
    let vault = Vault::open(dir.path()).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let start = Instant::now();
    let mut index = Index::open(cache.path()).unwrap();
    let stats = index.reconcile(&vault).unwrap();
    let cold = start.elapsed();
    assert_eq!(stats.added, NOTES);

    let start = Instant::now();
    let stats = index.reconcile(&vault).unwrap();
    let warm = start.elapsed();
    assert_eq!(stats.unchanged, NOTES);

    let mut total = Duration::ZERO;
    let mut hits = 0;
    for i in (0..NOTES).step_by(100) {
        let target = VaultPath::new(&path(i)).unwrap();
        let start = Instant::now();
        hits += index.backlinks(&target).unwrap().len();
        total += start.elapsed();
    }
    let backlinks = total / 100;
    assert!(hits > 0);

    let mut total = Duration::ZERO;
    for n in 0..20 {
        let i = n * 37;
        let p = VaultPath::new(&path(i)).unwrap();
        std::fs::write(vault.abs(&p), note(i, &format!("\nedit {n} [[Note 1]]\n"))).unwrap();
        let start = Instant::now();
        index.apply(&vault, &[VaultEvent::Modified(p)]).unwrap();
        total += start.elapsed();
    }
    let incremental = total / 20;

    let start = Instant::now();
    let rows = index.note_rows().unwrap();
    let rows_time = start.elapsed();
    assert_eq!(rows.len(), NOTES);

    println!(
        "cold build {cold:?}, warm reconcile {warm:?}, backlinks {backlinks:?}, \
         incremental update {incremental:?}, note_rows {rows_time:?}"
    );
    assert!(cold < Duration::from_secs(10), "cold build {cold:?}");
    assert!(
        backlinks < Duration::from_millis(5),
        "backlinks {backlinks:?}"
    );
    assert!(
        incremental < Duration::from_millis(50),
        "incremental {incremental:?}"
    );
}
