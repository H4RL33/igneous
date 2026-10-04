//! Bases against a synthetic vault.

use std::sync::LazyLock;

use igneous_core::VaultPath;
use igneous_query::NoteData;
use igneous_query::bases::{
    BaseFile, Cell, EvalError, RunOptions, ViewData, ViewKind, ViewResult, ViewState, evaluate,
    run_with,
};

fn ms(timestamp: &str) -> i64 {
    timestamp
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond()
}

fn note(path: &str, text: &str) -> NoteData {
    let mut note = NoteData::from_text(VaultPath::new(path).unwrap(), text);
    note.ctime = ms("2025-01-01T09:00:00Z");
    note.mtime = ms("2025-06-01T09:00:00Z");
    note
}

static VAULT: LazyLock<Vec<NoteData>> = LazyLock::new(|| {
    let mut dune = note(
        "Library/Dune.md",
        "---\nstatus: reading\npages: 600\nread: 150\nrating: 5\nstarted: 2025-05-01\n---\n",
    );
    dune.mtime = ms("2025-06-14T12:00:00Z");
    let mut home = note(
        "Home.md",
        "---\nup: \"[[Steam Engine]]\"\nprice: 12.5\nunit price: 3\n---\nSee [[Storage]] and ![[diagram.png]] #home/main\n",
    );
    home.backlinks = vec![VaultPath::new("Course/Storage.md").unwrap()];
    vec![
        note(
            "Inventions/Steam Engine.md",
            "---\ntags: [invention]\nyear: 1712\n---\nSee [[Printing Press]].\n",
        ),
        note(
            "Inventions/Printing Press.md",
            "---\ntags: invention\nyear: 1440\n---\n",
        ),
        note(
            "Inventions/Archive/Telegraph.md",
            "An #invention/communication of 1837.\n",
        ),
        note(
            "Course/Users and Groups.md",
            "---\ntags: [course, lesson]\nscore: 7\n---\n",
        ),
        note(
            "Course/Storage.md",
            "---\ntags: [course, lesson]\nscore: 9\n---\n[[Home]]\n",
        ),
        note("Course/Networking.md", "---\ntags: [course, lesson]\n---\n"),
        note("Course/Exam.md", "---\ntags: [course]\nscore: 3\n---\n"),
        dune,
        note(
            "Library/Emma.md",
            "---\nstatus: queued\npages: 400\nread: 0\nrating: 3\n---\n",
        ),
        note(
            "Library/Ulysses.md",
            "---\nstatus: reading\npages: 700\nread: 525\nrating: 4\n---\n",
        ),
        note("Library/Notes.md", "---\nstatus: reading\n---\n"),
        note(
            "Archive/Old Book.md",
            "---\nstatus: reading\npages: 10\nread: 1\nrating: 2\n---\n",
        ),
        NoteData::new(VaultPath::new("Attachments/diagram.png").unwrap()),
        home,
    ]
});

fn options() -> RunOptions {
    RunOptions {
        now: "2025-06-15T12:00:00[UTC]".parse().unwrap(),
    }
}

fn find(path: &str) -> &'static NoteData {
    VAULT.iter().find(|n| n.path.as_str() == path).unwrap()
}

fn fixture(name: &str) -> BaseFile {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    BaseFile::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn view(base: &BaseFile, index: usize) -> ViewData {
    match run_with(base, index, &VAULT, None, &options()) {
        ViewResult::Ready(data) => data,
        other => panic!("view {index}: {other:?}"),
    }
}

fn names(data: &ViewData) -> Vec<&str> {
    data.rows.iter().map(|r| r.file.stem()).collect()
}

fn show(cell: &Cell) -> String {
    match cell {
        Ok(value) => value.display(),
        Err(error) => format!("error: {error}"),
    }
}

/// Evaluates an expression for a row, as text.
fn eval_for(expression: &str, row: Option<&str>) -> String {
    let row = row.map(find);
    show(&evaluate(expression, None, &VAULT, row, None, &options()))
}

fn eval(expression: &str) -> String {
    eval_for(expression, None)
}

#[test]
fn tag_filtered_table() {
    let base = fixture("inventions.base");
    let data = view(&base, 0);
    assert_eq!(data.kind, ViewKind::Table);
    // Nested tags count: #invention/communication.
    assert_eq!(
        names(&data),
        ["Printing Press", "Steam Engine", "Telegraph"]
    );
    assert_eq!(data.columns.len(), 1);
    assert_eq!(
        (data.columns[0].id.as_str(), data.columns[0].name.as_str()),
        ("file.name", "file name")
    );
    assert_eq!(show(&data.rows[0].cells[0]), "Printing Press.md");
    assert!(data.errors.is_empty());
}

#[test]
fn sorted_table_with_display_names_and_widths() {
    let base = fixture("lessons.base");
    let data = view(&base, 0);
    // Highest score first; no score last.
    assert_eq!(names(&data), ["Storage", "Users and Groups", "Networking"]);
    let headers: Vec<_> = data.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(headers, ["file name", "score (/10)"]);
    assert_eq!(data.columns[0].width, Some(250));
    assert_eq!(data.columns[1].id, "note.score");
    let scores: Vec<_> = data.rows.iter().map(|r| show(&r.cells[1])).collect();
    assert_eq!(scores, ["9", "7", ""]);
}

#[test]
fn groups_formulas_and_summaries() {
    let base = fixture("library.base");
    assert!(base.problems.is_empty(), "{:?}", base.problems);
    let data = view(&base, 0);
    assert_eq!(names(&data), ["Emma", "Ulysses", "Dune", "Notes"]);
    let groups: Vec<_> = data
        .groups
        .iter()
        .map(|g| (g.key.display(), g.rows.clone()))
        .collect();
    assert_eq!(
        groups,
        [("queued".to_owned(), 0..1), ("reading".to_owned(), 1..4)]
    );
    let headers: Vec<_> = data.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(headers, ["Title", "pages", "Progress", "pages_left"]);
    let progress: Vec<_> = data.rows.iter().map(|r| show(&r.cells[2])).collect();
    assert_eq!(progress, ["0", "75", "25", ""]);
    let left: Vec<_> = data.rows.iter().map(|r| show(&r.cells[3])).collect();
    assert_eq!(left, ["400", "175", "450", ""]);

    let summaries: Vec<_> = data
        .summaries
        .iter()
        .map(|s| (s.column, s.name.as_str(), show(&s.value)))
        .collect();
    assert_eq!(
        summaries,
        [
            (1, "Sum", "1700".to_owned()),
            (2, "Average", format!("{}", 100.0 / 3.0)),
            (3, "total", "1025".to_owned()),
        ]
    );
}

#[test]
fn limits_and_cards() {
    let base = fixture("library.base");
    let data = view(&base, 1);
    assert_eq!(data.kind, ViewKind::Cards);
    assert_eq!(names(&data), ["Dune", "Ulysses"]);
    // Every Markdown file outside Archive matched before the limit.
    assert_eq!(data.total, 12);
    assert_eq!(base.views[1].options[0].0, "image");
}

#[test]
fn broken_views_report_problems() {
    let base = fixture("library.base");
    let data = view(&base, 2);
    assert_eq!(data.kind, ViewKind::List);
    assert!(data.rows.is_empty());
    assert_eq!(data.errors.len(), 1, "{:?}", data.errors);
    assert!(data.errors[0].contains("file.hasTag("));

    assert!(matches!(
        run_with(&base, 3, &VAULT, None, &options()),
        ViewResult::Unsupported(kind) if kind == "kanban"
    ));
    assert!(matches!(
        run_with(&base, 9, &VAULT, None, &options()),
        ViewResult::NoSuchView
    ));

    let dune = Some(find("Library/Dune.md"));
    let formula = |name: &str| evaluate(name, Some(&base), &VAULT, dune, None, &options());
    assert_eq!(
        formula("formula.loop_a").unwrap_err(),
        EvalError::Cycle("loop_a".into())
    );
    assert_eq!(
        formula("formula.missing").unwrap_err(),
        EvalError::UnknownFormula("missing".into())
    );
    assert_eq!(show(&formula("formula.label")), "25% of Dune");
}

#[test]
fn operators_and_literals() {
    assert_eq!(eval("1 + 2 * 3"), "7");
    assert_eq!(eval("(1 + 2) * 3"), "9");
    assert_eq!(eval("10 % 4"), "2");
    assert_eq!(eval("1 / 4"), "0.25");
    assert_eq!(eval("\"a\" + 1 + true"), "a1true");
    assert_eq!(eval("2 > 1 && 1 >= 1"), "true");
    assert_eq!(eval("!(1 == 1) || 3 != 3"), "false");
    assert_eq!(eval("null || \"fallback\""), "fallback");
    assert_eq!(eval("\"b\" > \"a\""), "true");
    assert_eq!(eval("[1, 2][1]"), "2");
    assert_eq!(eval("{\"a\": {\"b\": 3}}.a.b"), "3");
    assert_eq!(eval("{\"a\": 1}[\"a\"]"), "1");
}

#[test]
fn dates_and_durations() {
    assert_eq!(
        eval("date(\"2024-12-01\") + \"1M\" + \"4h\" + \"3m\""),
        "2025-01-01 04:03"
    );
    assert_eq!(eval("(now() + \"1d\") - now()"), "86400000");
    assert_eq!(eval("today()"), "2025-06-15");
    assert_eq!(eval("today() + \"7d\""), "2025-06-22");
    assert_eq!(eval("now().hour"), "12");
    assert_eq!(
        eval("date(\"2025-05-27\").format(\"YYYY-MM-DD\")"),
        "2025-05-27"
    );
    assert_eq!(
        eval("now().date().format(\"YYYY-MM-DD HH:mm:ss\")"),
        "2025-06-15 00:00:00"
    );
    assert_eq!(eval("now().time()"), "12:00:00");
    assert_eq!(eval("date(\"2025-06-12 12:00\").relative()"), "3 days ago");
    assert_eq!(eval("date(\"2025-06-15\") > date(\"2025-06-14\")"), "true");
    assert_eq!(eval("date(\"2025-06-15\") == \"2025-06-15\""), "true");
    assert_eq!(eval("now() + (duration(\"1d\") * 2)"), "2025-06-17 12:00");
    assert_eq!(eval("date(\"2025-01-31\").month"), "1");
    assert_eq!(eval("date(\"2025-01-01\") - \"1d\""), "2024-12-31");
    assert_eq!(eval("number(date(\"1970-01-02\"))"), "86400000");
    assert!(eval("date(\"soon\")").starts_with("error:"));
}

#[test]
fn string_functions() {
    for (expression, expected) in [
        ("\"hello\".contains(\"ell\")", "true"),
        ("\"hello\".containsAll(\"h\", \"e\")", "true"),
        ("\"hello\".containsAny(\"x\", \"y\", \"e\")", "true"),
        ("\"hello\".endsWith(\"lo\")", "true"),
        ("\"hello\".startsWith(\"he\")", "true"),
        ("\"\".isEmpty()", "true"),
        ("\"Hello world\".isEmpty()", "false"),
        ("\"Hello\".lower()", "hello"),
        ("\"Hello\".upper()", "HELLO"),
        ("\"a:b:c:d\".replace(/:/, \"-\")", "a-b:c:d"),
        ("\"a:b:c:d\".replace(/:/g, \"-\")", "a-b-c-d"),
        ("\"a:b:c:d\".replace(\":\", \"-\")", "a-b-c-d"),
        (
            "\"John Smith\".replace(/(\\w+) (\\w+)/, \"$2, $1\")",
            "Smith, John",
        ),
        ("\"123\".repeat(2)", "123123"),
        ("\"hello\".reverse()", "olleh"),
        ("\"hello\".slice(1, 4)", "ell"),
        ("\"hello\".slice(-3)", "llo"),
        ("\"a,b,c,d\".split(\",\", 3)", "a, b, c"),
        ("\"a,b,c,d\".split(/,/, 3)", "a, b, c"),
        ("\"hello world\".title()", "Hello World"),
        ("\"  hi  \".trim()", "hi"),
        ("\"héllo\".length", "5"),
        ("escapeHTML(\"<b>&</b>\")", "&lt;b&gt;&amp;&lt;/b&gt;"),
    ] {
        assert_eq!(eval(expression), expected, "{expression}");
    }
}

#[test]
fn number_list_and_any_functions() {
    for (expression, expected) in [
        ("(-5).abs()", "5"),
        ("(2.1).ceil()", "3"),
        ("(2.9).floor()", "2"),
        ("(2.5).round()", "3"),
        ("(2.3333).round(2)", "2.33"),
        ("(3.14159).toFixed(2)", "3.14"),
        ("5.isEmpty()", "false"),
        ("[1,2,3].contains(2)", "true"),
        ("[1,2,3].containsAll(2,3)", "true"),
        ("[1,2,3].containsAny(3,4)", "true"),
        ("[1,2,3,4].filter(value > 2)", "3, 4"),
        ("[1,2,3,4].filter(index == 0)", "1"),
        ("[1,[2,3]].flat()", "1, 2, 3"),
        ("[1,2,3].isEmpty()", "false"),
        ("[1,2,3].join(\",\")", "1,2,3"),
        ("[1,2,3,4].map(value + 1)", "2, 3, 4, 5"),
        ("[1,2,3].reduce(acc + value, 0)", "6"),
        (
            "[1, \"x\", 3, 2].filter(value.isType(\"number\")).reduce(if(acc == null || value > acc, value, acc), null)",
            "3",
        ),
        ("[1,2,3].reverse()", "3, 2, 1"),
        ("[1,2,3,4].slice(1,3)", "2, 3"),
        ("[3, 1, 2].sort()", "1, 2, 3"),
        ("[\"c\", \"a\", \"b\"].sort()", "a, b, c"),
        ("[1,2,2,3].unique()", "1, 2, 3"),
        ("[1,2,3].length", "3"),
        ("[2, 4].mean()", "3"),
        ("1.isTruthy()", "true"),
        ("\"example\".isType(\"string\")", "true"),
        ("true.isType(\"boolean\")", "true"),
        ("today().isType(\"date\")", "true"),
        ("123.toString()", "123"),
        ("{}.isEmpty()", "true"),
        ("{\"a\": 1, \"b\": 2}.keys()", "a, b"),
        ("{\"a\": 1, \"b\": 2}.values()", "1, 2"),
        ("/abc/.matches(\"abcde\")", "true"),
        ("if(false, 1)", ""),
        ("if(0, \"yes\", \"no\")", "no"),
        ("list(\"value\")", "value"),
        ("list([1, 2]).length", "2"),
        ("max(1, 5, 3)", "5"),
        ("min(4, 2, 8)", "2"),
        ("number(\"3.4\")", "3.4"),
        ("number(true)", "1"),
        ("missing.contains(\"x\")", ""),
        ("missing.isEmpty()", "true"),
    ] {
        assert_eq!(eval(expression), expected, "{expression}");
    }
    let random = eval("random()").parse::<f64>().unwrap();
    assert!((0.0..1.0).contains(&random));
    assert_eq!(eval("html(\"<b>\")"), "error: html() isn't supported yet");
    assert_eq!(eval("teleport()"), "error: teleport() isn't supported yet");
    assert_eq!(
        eval("\"x\".teleport()"),
        "error: string.teleport() isn't supported yet"
    );
    assert!(eval("1 +").starts_with("error:"));
}

#[test]
fn files_properties_and_links() {
    let home = Some("Home.md");
    for (expression, expected) in [
        ("file.name", "Home.md"),
        ("file.basename", "Home"),
        ("file.path", "Home.md"),
        ("file.folder", "/"),
        ("file.ext", "md"),
        ("file.tags", "home/main"),
        ("file.hasTag(\"home\")", "true"),
        ("file.hasTag(\"#HOME\", \"other\")", "true"),
        ("file.hasTag(\"hom\")", "false"),
        ("file.hasProperty(\"price\")", "true"),
        ("file.hasProperty(\"cost\")", "false"),
        ("file.hasLink(\"Storage\")", "true"),
        ("file.hasLink(file(\"Course/Storage.md\"))", "true"),
        ("file.hasLink(\"Exam\")", "false"),
        ("file.inFolder(\"\")", "true"),
        ("file.links.length", "2"),
        ("file.embeds", "diagram.png"),
        ("file.backlinks", "Storage"),
        ("file.ctime.format(\"YYYY-MM-DD\")", "2025-01-01"),
        ("file.mtime > now() - \"1 week\"", "false"),
        ("file.asLink()", "Home"),
        ("file.asLink(\"start\")", "start"),
        ("price * 2", "25"),
        ("note.price", "12.5"),
        ("note[\"unit price\"]", "3"),
        ("file.properties.price", "12.5"),
        ("up", "Steam Engine"),
        ("up == link(\"Steam Engine\")", "true"),
        ("up.asFile() == file(\"Steam Engine\")", "true"),
        ("up.asFile().year", "1712"),
        ("up.asFile().file.folder", "Inventions"),
        ("up.linksTo(file(\"Printing Press\"))", "true"),
        (
            "link(\"Printing Press\").asFile().path",
            "Inventions/Printing Press.md",
        ),
        ("file(\"nowhere\")", ""),
    ] {
        assert_eq!(eval_for(expression, home), expected, "{expression}");
    }

    let dune = Some("Library/Dune.md");
    assert_eq!(eval_for("file.mtime > now() - \"1 week\"", dune), "true");
    assert_eq!(eval_for("file.mtime.relative()", dune), "a day ago");
    assert_eq!(eval_for("started < today()", dune), "true");
    assert_eq!(eval_for("started.isType(\"date\")", dune), "true");
    assert_eq!(eval_for("file.inFolder(\"library\")", dune), "true");
    assert_eq!(
        eval_for(
            "file.inFolder(\"Inventions\")",
            Some("Inventions/Archive/Telegraph.md")
        ),
        "true"
    );

    // `this` is the file showing the base.
    let this = Some(find("Course/Storage.md"));
    let with_this = |expression: &str| {
        show(&evaluate(
            expression,
            None,
            &VAULT,
            Some(find("Home.md")),
            this,
            &options(),
        ))
    };
    assert_eq!(with_this("this.file.name"), "Storage.md");
    assert_eq!(with_this("file.hasLink(this)"), "true");
    assert_eq!(with_this("file.hasLink(this.file)"), "true");
    assert_eq!(with_this("this.score"), "9");
}

#[test]
fn every_view_rewrites_to_itself() {
    for name in ["inventions.base", "lessons.base", "library.base"] {
        let base = fixture(name);
        for (i, view) in base.views.iter().enumerate() {
            let state = ViewState {
                order: Some(view.order.clone()),
                column_sizes: Some(view.column_sizes.clone()),
                sort: Some(view.sort.clone()),
            };
            assert_eq!(
                base.set_view_state(i, &state),
                Ok(Vec::new()),
                "{name} view {i}"
            );
        }
    }
}
