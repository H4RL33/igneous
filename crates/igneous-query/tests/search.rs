//! Search across a synthetic vault.

use igneous_core::VaultPath;
use igneous_query::NoteData;
use igneous_query::search::{Matcher, Query, SearchOptions, parse, plan};

fn vault() -> Vec<NoteData> {
    let note = |path: &str, text: &str| NoteData::from_text(VaultPath::new(path).unwrap(), text);
    vec![
        note(
            "Daily notes/2022-07-01.md",
            "# Plans\n- [ ] call the plumber\n- [x] email Sam about the meeting\n\nWork meeting at 10.\n",
        ),
        note(
            "Projects/Igneous.md",
            "---\nstatus: Draft\ntags: [project, gtk]\n---\n# Goals\nA meeting with the GNOME folks.\n\n# Risks\nPersonal meetup on Friday.\n",
        ),
        note(
            "Recipes/Cake.md",
            "Mix the flour and sugar.\n\n#baking #food/sweet\n",
        ),
        NoteData::new(VaultPath::new("Attachments/photo.jpg").unwrap()),
        NoteData::new(VaultPath::new("Attachments/202209 scan.pdf").unwrap()),
    ]
}

fn results(search: &str) -> Vec<String> {
    let matcher = Matcher::parse(search, SearchOptions::default()).unwrap();
    vault()
        .iter()
        .filter(|n| matcher.matches(n).is_some())
        .map(|n| n.path.stem().to_owned())
        .collect()
}

#[test]
fn documented_examples() {
    assert_eq!(results("meeting work"), ["2022-07-01"]);
    assert_eq!(
        results("meeting OR flour"),
        ["2022-07-01", "Igneous", "Cake"]
    );
    assert_eq!(
        results("meeting work OR meetup personal"),
        ["2022-07-01", "Igneous"]
    );
    assert_eq!(
        results("meeting (work OR gnome)"),
        ["2022-07-01", "Igneous"]
    );
    assert_eq!(results("meeting -work"), ["Igneous"]);
    assert_eq!(results("meeting -(work email)"), ["Igneous"]);
    assert_eq!(results("file:.jpg"), ["photo"]);
    assert_eq!(results("file:202209"), ["202209 scan"]);
    assert_eq!(results("path:\"Daily notes/2022-07\""), ["2022-07-01"]);
    assert_eq!(results("content:\"the flour\""), ["Cake"]);
    assert_eq!(results("match-case:GNOME"), ["Igneous"]);
    assert_eq!(results("match-case:gnome"), Vec::<String>::new());
    assert_eq!(results("tag:#food"), ["Cake"]);
    assert_eq!(results("tag:#sweet"), Vec::<String>::new());
    assert_eq!(results("line:(mix flour)"), ["Cake"]);
    assert_eq!(results("block:(meeting gnome)"), ["Igneous"]);
    assert_eq!(results("section:(meeting friday)"), Vec::<String>::new());
    assert_eq!(results("task:call"), ["2022-07-01"]);
    assert_eq!(results("task-todo:email"), Vec::<String>::new());
    assert_eq!(results("task-done:(email OR call)"), ["2022-07-01"]);
    assert_eq!(results("[status]"), ["Igneous"]);
    assert_eq!(results("[status:Draft OR Published]"), ["Igneous"]);
    assert_eq!(results("[tags:gtk]"), ["Igneous"]);
    assert_eq!(results(r"/\d{2}:\d{2}|at \d+/"), ["2022-07-01"]);
    assert_eq!(results(r"path:/\d{4}-\d{2}-\d{2}/"), ["2022-07-01"]);
}

#[test]
fn plans_list_safe_prefilter_terms() {
    let p = plan(
        &parse("meeting \"work items\" -draft (a OR b) line:(flour sugar) file:x tag:#t").unwrap(),
    );
    assert_eq!(p.fts_terms, ["meeting", "work items", "flour", "sugar"]);
    assert!(!p.needs_document);
    assert!(p.reaches_files);
    let p = plan(&parse("task:call").unwrap());
    assert!(p.needs_document);
    assert!(!p.reaches_files);
    assert_eq!(plan(&Query::Empty).fts_terms, Vec::<String>::new());
}

#[test]
fn hits_highlight_text_and_paths() {
    let notes = vault();
    let matcher = Matcher::parse("meeting tag:#gtk [status]", SearchOptions::default()).unwrap();
    let hit = matcher.matches(&notes[1]).unwrap();
    let shown: Vec<&str> = hit
        .content
        .iter()
        .map(|r| &notes[1].text[r.clone()])
        .collect();
    assert_eq!(shown, ["status: Draft", "meeting"]);
    let matcher = Matcher::parse("igneous", SearchOptions::default()).unwrap();
    let hit = matcher.matches(&notes[1]).unwrap();
    assert_eq!(&notes[1].path.as_str()[hit.path[0].clone()], "Igneous");
    assert_eq!(hit.path.len(), 1);
}
