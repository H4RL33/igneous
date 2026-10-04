//! Rename and tag-rename plans.

use std::collections::HashMap;
use std::path::Path;

use igneous_core::{Vault, VaultPath};
use igneous_index::refactor::{plan_rename, plan_tag_rename};
use igneous_index::{Index, RefactorPlan};
use igneous_markdown::LinkKind;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/resolution");

fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn indexed(dir: &Path) -> (Vault, Index) {
    let vault = Vault::open(dir).unwrap();
    let mut index = Index::in_memory().unwrap();
    index.reconcile(&vault).unwrap();
    (vault, index)
}

fn fixture_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE), dir.path());
    dir
}

/// Every vault link in every note, with where it leads.
fn link_targets(dir: &Path) -> HashMap<VaultPath, Vec<Option<VaultPath>>> {
    let (_, index) = indexed(dir);
    let mut out = HashMap::new();
    for note in index.notes().unwrap() {
        let text = index.text(&note.path).unwrap().unwrap();
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

/// Applies a plan to the disk: edits first, then moves.
fn apply(root: &Path, plan: &RefactorPlan) {
    for file in &plan.files {
        let abs = file.path.to_fs(root);
        assert_eq!(std::fs::read_to_string(&abs).unwrap(), file.text);
        std::fs::write(abs, file.apply()).unwrap();
    }
    for (from, to) in &plan.moves {
        let dest = to.to_fs(root);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::rename(from.to_fs(root), dest).unwrap();
    }
}

/// Renames `from` to `to` with the planned edits, and checks every link
/// still leads where it did (to the moved file, if it moved). Returns the
/// plan.
fn rename_keeps_links(from: &str, to: &str) -> (tempfile::TempDir, RefactorPlan) {
    let dir = fixture_copy();
    let before = link_targets(dir.path());
    let (_, index) = indexed(dir.path());
    let plan = plan_rename(&index, &p(from), &p(to)).unwrap();
    let moved: HashMap<VaultPath, VaultPath> = plan.moves.iter().cloned().collect();
    apply(dir.path(), &plan);
    let after = link_targets(dir.path());
    for (source, targets) in before {
        let source_after = moved.get(&source).cloned().unwrap_or(source.clone());
        let expected: Vec<Option<VaultPath>> = targets
            .into_iter()
            .map(|t| t.map(|t| moved.get(&t).cloned().unwrap_or(t)))
            .collect();
        assert_eq!(
            after[&source_after], expected,
            "links in {source} after renaming {from} to {to}"
        );
    }
    (dir, plan)
}

fn edits_in(plan: &RefactorPlan, path: &str) -> Vec<String> {
    plan.files
        .iter()
        .filter(|f| f.path == p(path))
        .flat_map(|f| {
            f.edits
                .iter()
                .map(|e| format!("{} -> {}", &f.text[e.range.clone()], e.insert))
        })
        .collect()
}

#[test]
fn renaming_a_note() {
    let (dir, plan) = rename_keeps_links("Projects/Note.md", "Projects/Renamed.md");
    assert_eq!(
        edits_in(&plan, "Home.md"),
        [
            "Projects/Note -> Projects/Renamed",
            "Projects/Note.md -> Projects/Renamed.md"
        ]
    );
    // `[[Note]]` beside it becomes the new name; the relative link keeps
    // its `.md`.
    assert_eq!(
        edits_in(&plan, "Projects/Plan.md"),
        ["Note -> Renamed", "Note.md -> Renamed.md"]
    );
    let home = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(home.contains("[[Projects/Renamed#Heading|the note]]"));
    assert!(home.contains("[n](Projects/Renamed.md#Heading)"));
    // `[[Note]]` in Home pointed at Archive/Note and still does: untouched.
    assert!(home.contains("See [[Note]]"));
}

#[test]
fn spaces_brackets_frontmatter_and_embeds() {
    let (dir, plan) = rename_keeps_links("Projects/Plan.md", "Projects/Big Plan.md");
    let home = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(home.contains("up: \"[[Projects/Big Plan]]\""));
    assert!(home.contains("[plan](Projects/Big%20Plan.md)"));
    assert!(home.contains("[plan again](<Projects/Big Plan.md>)"));
    let deep = std::fs::read_to_string(dir.path().join("Projects/Sub/Deep.md")).unwrap();
    assert!(deep.contains("[[Big Plan]]"));
    assert_eq!(
        plan.moves,
        [(p("Projects/Plan.md"), p("Projects/Big Plan.md"))]
    );

    let (dir, _) = rename_keeps_links("Attachments/diagram.png", "Attachments/chart.png");
    let home = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(home.contains("![[chart.png]]"));
    assert!(home.contains("![[Attachments/chart.png|300]]"));
    // An unresolved embed stays as it was.
    assert!(home.contains("![[diagram]]"));
}

#[test]
fn moving_a_folder() {
    let (dir, plan) = rename_keeps_links("Projects/Sub", "Elsewhere/Sub2");
    assert_eq!(
        plan.moves,
        [(p("Projects/Sub/Deep.md"), p("Elsewhere/Sub2/Deep.md"))]
    );
    assert_eq!(
        edits_in(&plan, "Home.md"),
        [
            "Sub/Deep -> Elsewhere/Sub2/Deep",
            "Projects/Sub/Deep -> Elsewhere/Sub2/Deep"
        ]
    );
    let plan_md = std::fs::read_to_string(dir.path().join("Projects/Plan.md")).unwrap();
    assert!(plan_md.contains("[deeper](../Elsewhere/Sub2/Deep.md)"));

    // Relative links inside moved notes are worked out again.
    let (dir, plan) = rename_keeps_links("Projects", "Work/Projects");
    assert_eq!(
        edits_in(&plan, "Projects/Plan.md"),
        ["../Archive/Note.md -> ../../Archive/Note.md"]
    );
    assert!(dir.path().join("Work/Projects/Sub/Deep.md").is_file());
}

#[test]
fn new_names_never_capture_other_links() {
    // A root note named Plan would capture `[[Plan]]` in Deep, which means
    // Projects/Plan: that link is pinned to its full path instead.
    let (dir, plan) = rename_keeps_links("Café.md", "Plan.md");
    assert_eq!(
        edits_in(&plan, "Projects/Sub/Deep.md"),
        ["Plan -> Projects/Plan"]
    );
    let home = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(home.contains("The cafe: [[Plan]] and [[Plan]]."));
}

#[test]
fn renaming_to_the_same_name_or_nothing() {
    let dir = fixture_copy();
    let (_, index) = indexed(dir.path());
    assert!(
        plan_rename(&index, &p("Home.md"), &p("Home.md"))
            .unwrap()
            .is_empty()
    );
    let plan = plan_rename(&index, &p("Nope.md"), &p("Other.md")).unwrap();
    assert!(plan.moves.is_empty() && plan.is_empty());
}

#[test]
fn renaming_tags() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("A.md"),
        "---\ntags: [project, \"#Project/Igneous\", projects]\n---\n\
         #project and #Project/igneous/ui, not #project-x or #projects.\n\
         `#project` in code and\n\n```\n#project\n```\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("B.md"), "tags:\n#other\n").unwrap();
    std::fs::write(
        dir.path().join("C.md"),
        "---\ntags:\n  - Project\n  - x\n---\nbody\n",
    )
    .unwrap();
    let (_, index) = indexed(dir.path());
    let plan = plan_tag_rename(&index, "#project", "work").unwrap();
    assert_eq!(plan.files.len(), 2);
    assert_eq!(plan.edit_count(), 5);
    apply(dir.path(), &plan);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("A.md")).unwrap(),
        "---\ntags: [work, \"#work/Igneous\", projects]\n---\n\
         #work and #work/igneous/ui, not #project-x or #projects.\n\
         `#project` in code and\n\n```\n#project\n```\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("C.md")).unwrap(),
        "---\ntags:\n  - work\n  - x\n---\nbody\n"
    );
    assert!(plan_tag_rename(&index, "nothing", "x").unwrap().is_empty());
}
