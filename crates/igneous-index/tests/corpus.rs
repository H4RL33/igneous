//! Opt-in checks on real vaults. Each vault is copied first; the copy is
//! indexed, queried and refactored, and only counts are printed.
//!
//! `IGNEOUS_CORPUS=/copy/one:/copy/two cargo test -p igneous-index --test corpus -- --ignored --nocapture`

use std::collections::HashMap;
use std::path::Path;

use igneous_core::{Vault, VaultPath};
use igneous_index::refactor::plan_rename;
use igneous_index::{EdgeTarget, Index};
use igneous_markdown::LinkKind;

/// Copies the vault's visible files (as the vault itself lists them).
fn copy_visible(from: &Path, to: &Path) {
    let vault = Vault::open(from).unwrap();
    for entry in vault.walk().filter_map(Result::ok) {
        let dest = entry.path.to_fs(to);
        if entry.kind == igneous_core::vault::EntryKind::Folder {
            std::fs::create_dir_all(dest).unwrap();
        } else if std::fs::metadata(vault.abs(&entry.path)).is_ok_and(|m| m.is_file()) {
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(vault.abs(&entry.path), dest).unwrap();
        }
    }
}

fn indexed(dir: &Path) -> (Vault, Index) {
    let vault = Vault::open(dir).unwrap();
    let mut index = Index::in_memory().unwrap();
    index.reconcile(&vault).unwrap();
    (vault, index)
}

fn link_targets(index: &Index) -> HashMap<VaultPath, Vec<Option<VaultPath>>> {
    let mut out = HashMap::new();
    for note in index.notes().unwrap() {
        let text = index.text(&note.path).unwrap().unwrap_or_default();
        let doc = igneous_markdown::parse(&text);
        let targets = doc
            .links
            .iter()
            .filter(|l| matches!(l.kind, LinkKind::Wiki | LinkKind::Markdown))
            .filter(|l| !l.reference.is_external())
            .map(|l| index.files().resolve(&l.reference, &note.path))
            .collect();
        out.insert(note.path, targets);
    }
    out
}

#[test]
#[ignore = "needs IGNEOUS_CORPUS"]
fn corpus_indexes_and_refactors_cleanly() {
    let Ok(corpus) = std::env::var("IGNEOUS_CORPUS") else {
        return;
    };
    for (n, root) in corpus.split(':').filter(|s| !s.is_empty()).enumerate() {
        let dir = tempfile::tempdir().unwrap();
        copy_visible(Path::new(root), dir.path());
        let (vault, index) = indexed(dir.path());

        // A second, independent build agrees.
        let (_, again) = indexed(dir.path());
        assert_eq!(index.snapshot().unwrap(), again.snapshot().unwrap());

        // Every resolved link points at a file that exists.
        let edges = index.graph_edges().unwrap();
        let mut resolved = 0;
        for edge in &edges {
            if let EdgeTarget::File(path) = &edge.target {
                assert!(vault.abs(path).is_file());
                resolved += 1;
            }
        }
        let notes = index.notes().unwrap();
        let mut backlinks = 0;
        for note in &notes {
            backlinks += index.backlinks(&note.path).unwrap().len();
            index.outgoing(&note.path).unwrap();
        }
        let mut mentions = 0;
        for note in notes.iter().take(25) {
            mentions += index.unlinked_mentions(&note.path).unwrap().len();
        }
        let tags = index.tags().unwrap().len();
        let properties = index.property_catalog(&Default::default()).unwrap().len();
        let unresolved = index.unresolved().unwrap().len();
        let rows = index.note_rows().unwrap().len();

        // Rename a sample of notes, applying each plan, and check every
        // link still leads where it did.
        let mut renamed = 0;
        let mut edited_links = 0;
        let sample: Vec<VaultPath> = notes
            .iter()
            .step_by(7)
            .take(15)
            .map(|n| n.path.clone())
            .collect();
        for from in sample {
            let (_, index) = indexed(dir.path());
            let before = link_targets(&index);
            let to = VaultPath::new(&format!("{} renamed.md", from.with_extension("").as_str()))
                .unwrap();
            if index.files().contains(&to) {
                continue;
            }
            let plan = plan_rename(&index, &from, &to).unwrap();
            for file in &plan.files {
                let abs = file.path.to_fs(dir.path());
                let original = std::fs::read(&abs).unwrap();
                let mut text = igneous_core::TextFile::from_bytes(&original).unwrap();
                assert_eq!(text.text(), file.text);
                text.set_text(file.apply());
                std::fs::write(&abs, text.to_bytes()).unwrap();
                edited_links += file.edits.len();
            }
            for (a, b) in &plan.moves {
                std::fs::rename(a.to_fs(dir.path()), b.to_fs(dir.path())).unwrap();
            }
            let moved: HashMap<_, _> = plan.moves.iter().cloned().collect();
            let (_, after_index) = indexed(dir.path());
            let after = link_targets(&after_index);
            for (source, targets) in before {
                let source_after = moved.get(&source).cloned().unwrap_or(source.clone());
                let expected: Vec<Option<VaultPath>> = targets
                    .into_iter()
                    .map(|t| t.map(|t| moved.get(&t).cloned().unwrap_or(t)))
                    .collect();
                assert_eq!(
                    after[&source_after], expected,
                    "vault {n}: a rename broke links"
                );
            }
            renamed += 1;
        }

        println!(
            "vault {n}: {} files, {} notes, {} links resolved, {unresolved} unresolved targets, \
             {backlinks} backlinks, {mentions} mentions (first 25 notes), {tags} tags, \
             {properties} properties, {rows} rows; {renamed} renames applied ({edited_links} links updated)",
            index.files().len(),
            notes.len(),
            resolved,
        );
    }
}
