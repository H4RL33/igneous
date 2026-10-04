//! The index on the synthetic fixture vault in `tests/fixtures/resolution`.

use std::collections::BTreeMap;
use std::path::Path;

use igneous_core::settings::PropertyType;
use igneous_core::{Vault, VaultPath};
use igneous_index::{EdgeTarget, Index, Method};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/resolution");

fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

fn fixture() -> (Vault, Index) {
    let vault = Vault::open(FIXTURE).unwrap();
    let mut index = Index::in_memory().unwrap();
    index.reconcile(&vault).unwrap();
    (vault, index)
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

#[test]
fn resolution_matrix() {
    let (_, index) = fixture();
    let files = index.files();
    type Case<'a> = (&'a str, &'a str, Option<(&'a str, Method)>);
    let cases: &[Case] = &[
        // Duplicate names: neither is beside Home, so the shorter path wins.
        ("Home.md", "Note", Some(("Archive/Note.md", Method::Name))),
        (
            "Home.md",
            "Projects/Note",
            Some(("Projects/Note.md", Method::Exact)),
        ),
        (
            "Home.md",
            "Archive/Note.md",
            Some(("Archive/Note.md", Method::Exact)),
        ),
        ("Home.md", "wrong/Note", None),
        // Folders that match the end of the path.
        (
            "Home.md",
            "Sub/Deep",
            Some(("Projects/Sub/Deep.md", Method::Name)),
        ),
        (
            "Home.md",
            "Projects/Sub/Deep",
            Some(("Projects/Sub/Deep.md", Method::Exact)),
        ),
        (
            "Home.md",
            "/Projects/Sub/Deep",
            Some(("Projects/Sub/Deep.md", Method::Exact)),
        ),
        // Case and Unicode normalisation.
        (
            "Home.md",
            "deep",
            Some(("Projects/Sub/Deep.md", Method::Name)),
        ),
        (
            "Home.md",
            "DEEP",
            Some(("Projects/Sub/Deep.md", Method::Name)),
        ),
        ("Home.md", "Café", Some(("Café.md", Method::Exact))),
        ("Home.md", "CAFÉ", Some(("Café.md", Method::Exact))),
        ("Home.md", "Cafe\u{301}", Some(("Café.md", Method::Exact))),
        (
            "Daily/2026-10-04.md",
            "cafe\u{301}",
            Some(("Café.md", Method::Exact)),
        ),
        // Attachments need their extension.
        (
            "Home.md",
            "diagram.png",
            Some(("Attachments/diagram.png", Method::Name)),
        ),
        ("Home.md", "diagram", None),
        (
            "Home.md",
            "Attachments/diagram.png",
            Some(("Attachments/diagram.png", Method::Exact)),
        ),
        // Relative paths, which also find a name in the same folder first.
        (
            "Projects/Plan.md",
            "Note",
            Some(("Projects/Note.md", Method::Relative)),
        ),
        // Neither is beside Deep: the shorter path.
        (
            "Projects/Sub/Deep.md",
            "Note",
            Some(("Archive/Note.md", Method::Name)),
        ),
        (
            "Projects/Plan.md",
            "Note.md",
            Some(("Projects/Note.md", Method::Relative)),
        ),
        (
            "Projects/Plan.md",
            "../Archive/Note.md",
            Some(("Archive/Note.md", Method::Relative)),
        ),
        (
            "Projects/Plan.md",
            "./Sub/Deep.md",
            Some(("Projects/Sub/Deep.md", Method::Relative)),
        ),
        ("Projects/Plan.md", "../../outside.md", None),
        (
            "Projects/Sub/Deep.md",
            "Plan",
            Some(("Projects/Plan.md", Method::Name)),
        ),
        // Aliases aren't link targets; missing notes don't resolve.
        ("Projects/Plan.md", "Start", None),
        ("Home.md", "Nowhere", None),
        // No target: the note itself.
        ("Home.md", "", Some(("Home.md", Method::SameNote))),
    ];
    for (from, target, expected) in cases {
        let got = files.resolve_target(target, &p(from));
        let expected = expected.map(|(path, method)| (p(path), method));
        assert_eq!(got, expected, "{target:?} from {from}");
    }
}

#[test]
fn ignored_files_stay_out() {
    let (_, index) = fixture();
    let paths: Vec<&str> = index.files().iter().map(VaultPath::as_str).collect();
    assert_eq!(
        paths,
        [
            "Archive/Note.md",
            "Attachments/diagram.png",
            "Café.md",
            "Daily/2026-10-04.md",
            "Home.md",
            "Projects/Note.md",
            "Projects/Plan.md",
            "Projects/Sub/Deep.md",
        ]
    );
}

fn sources(hits: &[igneous_index::LinkHit]) -> Vec<&str> {
    hits.iter().map(|h| h.source.as_str()).collect()
}

#[test]
fn backlinks() {
    let (_, index) = fixture();
    let hits = index.backlinks(&p("Projects/Note.md")).unwrap();
    assert_eq!(
        sources(&hits),
        ["Home.md", "Home.md", "Projects/Plan.md", "Projects/Plan.md"]
    );
    assert_eq!(hits[0].line, 9);
    assert!(hits[0].line_text.starts_with("See [[Note]]"));
    assert_eq!(
        sources(&index.backlinks(&p("Archive/Note.md")).unwrap()),
        ["Home.md", "Home.md", "Projects/Plan.md"]
    );
    assert_eq!(
        index.backlinks(&p("Projects/Sub/Deep.md")).unwrap().len(),
        5
    );
    // Linking to itself (`[[#Home]]`) isn't a backlink.
    assert_eq!(
        sources(&index.backlinks(&p("Home.md")).unwrap()),
        ["Daily/2026-10-04.md", "Projects/Plan.md"]
    );
    let plan = index.backlinks(&p("Projects/Plan.md")).unwrap();
    assert_eq!(plan.len(), 4);
    assert!(plan[0].in_frontmatter);
    let diagram = index.backlinks(&p("Attachments/diagram.png")).unwrap();
    assert_eq!(diagram.len(), 2);
    assert!(diagram.iter().all(|h| h.embed));
}

#[test]
fn outgoing_and_unresolved() {
    let (_, index) = fixture();
    let out = index.outgoing(&p("Projects/Plan.md")).unwrap();
    let resolved: Vec<Option<&str>> = out
        .iter()
        .map(|l| l.resolved.as_ref().map(VaultPath::as_str))
        .collect();
    assert_eq!(
        resolved,
        [
            Some("Projects/Note.md"),
            Some("Archive/Note.md"),
            Some("Projects/Note.md"),
            Some("Projects/Sub/Deep.md"),
            Some("Home.md"),
            None,
        ]
    );
    let home = index.outgoing(&p("Home.md")).unwrap();
    let url = home.iter().find(|l| l.external).unwrap();
    assert_eq!(url.reference.target, "https://example.com");
    let headed = home.iter().find(|l| l.raw.contains("#Heading|")).unwrap();
    assert_eq!(headed.reference.display.as_deref(), Some("the note"));

    let unresolved: Vec<(String, Vec<(String, usize)>)> = index
        .unresolved()
        .unwrap()
        .into_iter()
        .map(|u| {
            (
                u.target,
                u.sources
                    .into_iter()
                    .map(|(s, n)| (s.to_string(), n))
                    .collect(),
            )
        })
        .collect();
    let home = |n| vec![("Home.md".to_owned(), n)];
    assert_eq!(
        unresolved,
        [
            ("diagram".to_owned(), home(1)),
            ("missing.md".to_owned(), home(1)),
            ("Nowhere".to_owned(), home(2)),
            ("Start".to_owned(), vec![("Projects/Plan.md".to_owned(), 1)]),
        ]
    );
}

#[test]
fn unlinked_mentions() {
    let (_, index) = fixture();
    let mentions = index.unlinked_mentions(&p("Home.md")).unwrap();
    let found: Vec<(&str, &str)> = mentions
        .iter()
        .map(|m| (m.source.as_str(), &m.line_text[..]))
        .collect();
    assert_eq!(mentions.len(), 3, "{found:?}");
    assert!(
        mentions
            .iter()
            .all(|m| m.source == p("Daily/2026-10-04.md"))
    );
    let text = index.text(&p("Daily/2026-10-04.md")).unwrap().unwrap();
    let words: Vec<&str> = mentions.iter().map(|m| &text[m.range.clone()]).collect();
    assert_eq!(words, ["Home", "start", "front page"]);
}

#[test]
fn tags_properties_and_notes() {
    let (_, index) = fixture();
    assert_eq!(
        index.tags().unwrap(),
        [
            ("daily".to_owned(), 1),
            ("home".to_owned(), 1),
            ("Project/Igneous".to_owned(), 1),
            ("project/igneous/ui".to_owned(), 1),
        ]
    );
    let catalog: Vec<(String, PropertyType, usize)> = index
        .property_catalog(&BTreeMap::new())
        .unwrap()
        .into_iter()
        .map(|p| (p.key, p.ty, p.count))
        .collect();
    assert_eq!(
        catalog,
        [
            ("aliases".to_owned(), PropertyType::Aliases, 1),
            ("created".to_owned(), PropertyType::Date, 1),
            ("rating".to_owned(), PropertyType::Number, 1),
            ("tags".to_owned(), PropertyType::Tags, 1),
            ("up".to_owned(), PropertyType::Text, 1),
        ]
    );
    let notes = index.notes().unwrap();
    assert_eq!(notes.len(), 7);
    let home = notes.iter().find(|n| n.path == p("Home.md")).unwrap();
    assert_eq!(home.title, "Home");
    assert_eq!(home.aliases, ["Start", "Front Page"]);
    assert!(home.mtime > 0);

    let headings = index.headings(&p("Projects/Note.md")).unwrap();
    assert_eq!(headings.len(), 1);
    assert_eq!((headings[0].level, &headings[0].text[..]), (1, "Heading"));
    let blocks = index.blocks(&p("Projects/Note.md")).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].id, "block-1");
    assert_eq!(blocks[0].line_text, "The current note. ^block-1");
    let tasks = index.tasks(&p("Projects/Plan.md")).unwrap();
    let statuses: Vec<char> = tasks.iter().map(|t| t.status).collect();
    assert_eq!(statuses, [' ', 'x']);
    assert_eq!(tasks[0].text, "Write the index");
}

#[test]
fn search_candidates() {
    let (_, index) = fixture();
    let paths = |terms: &[&str]| -> Vec<String> {
        let terms: Vec<String> = terms.iter().map(|t| t.to_string()).collect();
        index
            .search_candidates(&terms)
            .unwrap()
            .into_iter()
            .map(|p| p.to_string())
            .collect()
    };
    assert_eq!(
        paths(&["PLAN"]),
        ["Home.md", "Projects/Plan.md", "Projects/Sub/Deep.md"]
    );
    assert_eq!(paths(&["plan", "Back to"]), ["Projects/Plan.md"]);
    assert_eq!(paths(&["ab"]).len(), 7);
    assert_eq!(paths(&["quote\"d"]).len(), 0);
}

#[test]
fn graph_and_rows() {
    let (_, index) = fixture();
    let edges = index.graph_edges().unwrap();
    assert!(edges.iter().any(|e| e.source == p("Projects/Plan.md")
        && e.target == EdgeTarget::File(p("Projects/Note.md"))));
    assert!(
        edges
            .iter()
            .any(|e| e.source == p("Home.md")
                && e.target == EdgeTarget::Unresolved("Nowhere".into()))
    );
    assert!(
        !edges
            .iter()
            .any(|e| e.source == p("Home.md") && e.target == EdgeTarget::File(p("Home.md")))
    );

    let rows = index.note_rows().unwrap();
    assert_eq!(rows.len(), 8);
    let home = rows.iter().find(|r| r.path == p("Home.md")).unwrap();
    assert!(home.is_note);
    assert_eq!(home.properties.len(), 5);
    assert_eq!(home.tags, ["home", "Project/Igneous", "project/igneous/ui"]);
    assert_eq!(home.embeds, [p("Attachments/diagram.png")]);
    for target in [
        "Archive/Note.md",
        "Projects/Note.md",
        "Projects/Plan.md",
        "Café.md",
    ] {
        assert!(home.links.contains(&p(target)), "{target}");
    }
    assert!(!home.links.contains(&p("Home.md")));
    assert_eq!(
        home.text,
        std::fs::read_to_string(format!("{FIXTURE}/Home.md")).unwrap()
    );
    let note = rows
        .iter()
        .find(|r| r.path == p("Projects/Note.md"))
        .unwrap();
    assert_eq!(note.backlinks, [p("Home.md"), p("Projects/Plan.md")]);
    let diagram = rows
        .iter()
        .find(|r| r.path == p("Attachments/diagram.png"))
        .unwrap();
    assert!(!diagram.is_note && diagram.text.is_empty());
    assert_eq!(diagram.backlinks, [p("Home.md")]);
}

#[test]
fn reopening_reads_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE), dir.path());
    let cache = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut index = Index::open(cache.path()).unwrap();
    let stats = index.reconcile(&vault).unwrap();
    assert_eq!((stats.added, stats.unchanged), (8, 0));
    let snapshot = index.snapshot().unwrap();
    drop(index);

    let mut index = Index::open(cache.path()).unwrap();
    assert_eq!(index.files().len(), 8);
    let stats = index.reconcile(&vault).unwrap();
    assert_eq!((stats.added, stats.updated, stats.unchanged), (0, 0, 8));
    assert_eq!(index.snapshot().unwrap(), snapshot);

    std::fs::write(dir.path().join("New.md"), "[[Home]]\n").unwrap();
    std::fs::remove_file(dir.path().join("Café.md")).unwrap();
    std::fs::write(dir.path().join("Projects/Note.md"), "# Changed\n").unwrap();
    let stats = index.reconcile(&vault).unwrap();
    assert_eq!((stats.added, stats.updated, stats.removed), (1, 1, 1));
    assert_eq!(index.backlinks(&p("Home.md")).unwrap().len(), 3);
}

#[test]
fn rebuilds_other_versions_and_damaged_files() {
    let cache = tempfile::tempdir().unwrap();
    let vault = Vault::open(FIXTURE).unwrap();
    {
        let mut index = Index::open(cache.path()).unwrap();
        index.reconcile(&vault).unwrap();
    }
    let db = cache.path().join("index.sqlite");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    let index = Index::open(cache.path()).unwrap();
    assert!(index.files().is_empty(), "an old schema is thrown away");
    drop(index);

    for name in ["index.sqlite-wal", "index.sqlite-shm"] {
        let _ = std::fs::remove_file(cache.path().join(name));
    }
    std::fs::write(&db, b"this is not a database at all, not even slightly").unwrap();
    let mut index = Index::open(cache.path()).unwrap();
    assert_eq!(index.reconcile(&vault).unwrap().added, 8);
}

#[test]
fn files_that_arent_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Binary.md"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
    std::fs::write(dir.path().join("Linker.md"), "[[Binary]]\n").unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let mut index = Index::in_memory().unwrap();
    index.reconcile(&vault).unwrap();
    assert_eq!(index.text(&p("Binary.md")).unwrap().as_deref(), Some(""));
    assert_eq!(index.backlinks(&p("Binary.md")).unwrap().len(), 1);
}
