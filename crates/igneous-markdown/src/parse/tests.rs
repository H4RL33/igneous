use super::*;
use crate::link::Subpath;
use proptest::prelude::*;

/// One line per node: kind, source text, and hidden markers.
fn nodes(text: &str) -> Vec<String> {
    let doc = parse(text);
    doc.nodes.iter().map(|n| describe(text, n)).collect()
}

fn describe(text: &str, node: &Node) -> String {
    let kind = match &node.kind {
        NodeKind::Heading { level, setext } => {
            format!("H{level}{}", if *setext { "s" } else { "" })
        }
        NodeKind::CodeBlock { lang, .. } => format!("Code({})", lang.as_deref().unwrap_or("")),
        NodeKind::List { ordered } => format!("List({})", if *ordered { "ol" } else { "ul" }),
        NodeKind::ListItem { task, .. } => {
            format!("Item{}", if task.is_some() { "(task)" } else { "" })
        }
        NodeKind::Math { display } => format!("Math{}", if *display { "(display)" } else { "" }),
        NodeKind::Link(_) => "Link".into(),
        NodeKind::Embed(_) => "Embed".into(),
        NodeKind::Tag(_) => "Tag".into(),
        NodeKind::Callout(_) => "Callout".into(),
        NodeKind::BlockId(_) => "BlockId".into(),
        other => format!("{other:?}"),
    };
    let src = text[node.range.clone()].trim_end_matches('\n');
    if node.markers.is_empty() {
        format!("{kind} {src:?}")
    } else {
        let markers: Vec<&str> = node.markers.iter().map(|m| &text[m.clone()]).collect();
        format!("{kind} {src:?} [{}]", markers.join("|"))
    }
}

fn has(list: &[String], expected: &str) -> bool {
    list.iter().any(|n| n == expected)
}

macro_rules! assert_nodes {
    ($text:expr, [$($expected:expr),* $(,)?]) => {{
        let list = nodes($text);
        $(assert!(has(&list, $expected), "missing {:?} in\n{}", $expected, list.join("\n"));)*
    }};
}

#[test]
fn inline_formatting() {
    let text = "a *i* **b** ~~s~~ ==h== `c` ~single~";
    assert_nodes!(
        text,
        [
            "Emphasis \"*i*\" [*|*]",
            "Strong \"**b**\" [**|**]",
            "Strikethrough \"~~s~~\" [~~|~~]",
            "Highlight \"==h==\" [==|==]",
            "InlineCode \"`c`\" [`|`]",
        ]
    );
    assert!(
        !nodes(text)
            .iter()
            .any(|n| n.starts_with("Strikethrough \"~single"))
    );
}

#[test]
fn highlights() {
    assert_nodes!("==a **b** c==", ["Highlight \"==a **b** c==\" [==|==]"]);
    assert_nodes!("x==y==z", ["Highlight \"==y==\" [==|==]"]);
    assert!(
        !nodes("a == b == c")
            .iter()
            .any(|n| n.starts_with("Highlight"))
    );
    assert!(
        !nodes("`==code==`")
            .iter()
            .any(|n| n.starts_with("Highlight"))
    );
}

#[test]
fn headings() {
    let text = "# Title #\n\nSetext\n===\n\n###### Six\n";
    assert_nodes!(
        text,
        [
            "H1 \"# Title #\" [# | #]",
            "H1s \"Setext\\n===\" [===]",
            "H6 \"###### Six\" [###### ]",
        ]
    );
    let doc = parse(text);
    let titles: Vec<_> = doc
        .headings
        .iter()
        .map(|h| (h.level, h.text.as_str()))
        .collect();
    assert_eq!(titles, [(1, "Title"), (1, "Setext"), (6, "Six")]);
}

#[test]
fn wikilinks_and_embeds() {
    let text = "See [[Note#H|alias]], ![[img.png|300]] and [[Plain]].";
    assert_nodes!(
        text,
        [
            "Link \"[[Note#H|alias]]\" [[[Note#H||]]]",
            "Embed \"![[img.png|300]]\" [![[img.png|300]]]",
            "Link \"[[Plain]]\" [[[|]]]",
        ]
    );
    let doc = parse(text);
    let note = &doc.links[0];
    assert_eq!(note.kind, LinkKind::Wiki);
    assert_eq!(note.reference.target, "Note");
    assert_eq!(
        note.reference.subpath,
        Some(Subpath::Heading(vec!["H".into()]))
    );
    assert_eq!(&text[note.display_range.clone().unwrap()], "alias");
    let img = &doc.links[1];
    assert!(img.embed);
    assert_eq!(img.reference.size, Some((300, None)));
    assert_eq!(&text[doc.links[2].display_range.clone().unwrap()], "Plain");
}

#[test]
fn markdown_links() {
    let text = "[text **b**](Note%20A.md#Sec) and <https://x.y> and ![alt](pic.png)";
    assert_nodes!(
        text,
        [
            "Link \"[text **b**](Note%20A.md#Sec)\" [[|](Note%20A.md#Sec)]",
            "Link \"<https://x.y>\" [<|>]",
            "Embed \"![alt](pic.png)\" [![alt](pic.png)]",
        ]
    );
    let doc = parse(text);
    let link = &doc.links[0];
    assert_eq!(link.reference.target, "Note A.md");
    assert_eq!(link.reference.display.as_deref(), Some("text **b**"));
    assert_eq!(&text[link.display_range.clone().unwrap()], "text **b**");
    assert_eq!(doc.links[1].reference.target, "https://x.y");
    assert_eq!(doc.links[2].reference.target, "pic.png");
}

#[test]
fn tags() {
    let text = "#tag #nested/tag x#no #123 #1a (#paren) `#code` #trail/ #ünï\n\n# Heading #intag\n";
    let doc = parse(text);
    let names: Vec<_> = doc.tags.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["tag", "nested/tag", "1a", "trail", "ünï", "intag"]);
    for tag in &doc.tags {
        assert_eq!(&text[tag.range.clone()], format!("#{}", tag.name));
    }
}

#[test]
fn nested_items_have_markers() {
    let text = "- A bullet\n\t- A nested bullet\n\t- [ ] nested task\n";
    let doc = parse(text);
    let items: Vec<_> = doc
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::ListItem { .. }))
        .collect();
    assert_eq!(items.len(), 3);
    for item in &items {
        assert_eq!(&text[item.markers[0].clone()], "-");
    }
    assert_eq!(doc.tasks.len(), 1);
    assert_eq!(&text[doc.tasks[0].marker.clone()], "[ ]");
}

#[test]
fn tasks() {
    let text =
        "- [ ] todo\n- [x] done\n- [/] half\n1. [ ] numbered\n- not [ ] task\n- [ ]\n* [-] star\n";
    let doc = parse(text);
    let tasks: Vec<_> = doc
        .tasks
        .iter()
        .map(|t| (t.status, &text[t.item.clone()]))
        .collect();
    assert_eq!(
        tasks,
        [
            (' ', "- [ ] todo"),
            ('x', "- [x] done"),
            ('/', "- [/] half"),
            (' ', "1. [ ] numbered"),
            (' ', "- [ ]"),
            ('-', "* [-] star"),
        ]
    );
    assert!(doc.tasks[1].is_done());
    assert_eq!(&text[doc.tasks[1].status_offset()..][..1], "x");
    assert_nodes!(
        text,
        [
            "Item(task) \"- [ ] todo\" [-|[ ]]",
            "Item \"- not [ ] task\" [-]"
        ]
    );
}

#[test]
fn callouts_and_quotes() {
    let text = "> [!WARNING]- Careful now\n> body\n\n> [!note]\n> x\n\n> plain\n> > nested\n";
    let doc = parse(text);
    assert_eq!(doc.callouts.len(), 2);
    let warning = &doc.callouts[0];
    assert_eq!(warning.kind, "warning");
    assert_eq!(warning.fold, Some(Fold::Closed));
    assert_eq!(&text[warning.marker.clone()], "[!WARNING]-");
    assert_eq!(&text[warning.title.clone().unwrap()], "Careful now");
    assert_eq!(
        &text[warning.range.clone()],
        "> [!WARNING]- Careful now\n> body"
    );
    assert_eq!(doc.callouts[1].kind, "note");
    assert_eq!(doc.callouts[1].title, None);
    assert_nodes!(
        text,
        [
            "Callout \"> [!WARNING]- Careful now\\n> body\" [> |> ]",
            "Quote \"> plain\\n> > nested\" [> |> > ]",
            "Quote \"> nested\"",
        ]
    );
}

#[test]
fn code_is_opaque() {
    let text = "```rust\nlet x = 1; // #notatag ==no== %%no%% ^noid\n```\n\n    indented #x\n";
    let doc = parse(text);
    assert!(doc.tags.is_empty());
    assert!(doc.comments.is_empty());
    assert!(doc.block_ids.is_empty());
    assert_nodes!(
        text,
        ["Code(rust) \"```rust\\nlet x = 1; // #notatag ==no== %%no%% ^noid\\n```\" [```rust|```]"]
    );
    assert!(!nodes(text).iter().any(|n| n.starts_with("Highlight")));
}

#[test]
fn comments() {
    let text = "a %%hidden [[link]]%% b\n\n%%\nblock\n\npara\n%%\n\n`%%` x %%rest";
    let doc = parse(text);
    let comments: Vec<_> = doc.comments.iter().map(|c| &text[c.clone()]).collect();
    assert_eq!(
        comments,
        ["%%hidden [[link]]%%", "%%\nblock\n\npara\n%%", "%%rest"]
    );
}

#[test]
fn block_ids() {
    let text = "para ^abc-1\n\n> quote\n\n^on-own-line\n\n```\ncode ^nope\n```\n\nnot^id\n";
    let doc = parse(text);
    let ids: Vec<_> = doc.block_ids.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, ["abc-1", "on-own-line"]);
}

#[test]
fn frontmatter_and_body_offsets() {
    let text = "---\nup: \"[[Home|Start]]\"\ntags: [a]\n---\n# Heading\n";
    let doc = parse(text);
    let fm = doc.frontmatter.as_ref().unwrap();
    assert_eq!(fm.entries.len(), 2);
    let link = doc.links.iter().find(|l| l.in_frontmatter).unwrap();
    assert_eq!(link.reference.target, "Home");
    assert_eq!(&text[link.display_range.clone().unwrap()], "Start");
    assert_nodes!(text, ["H1 \"# Heading\" [# ]"]);
    // A `---` later in the body is a rule, not frontmatter.
    let text = "Intro\n\n---\n\nmore\n";
    assert!(parse(text).frontmatter.is_none());
    assert_nodes!(text, ["Rule \"---\" [---]"]);
}

#[test]
fn math() {
    assert_nodes!(
        "$x^2$ and $$y$$",
        ["Math \"$x^2$\" [$|$]", "Math(display) \"$$y$$\" [$$|$$]"]
    );
}

#[test]
fn footnotes() {
    let text = "Text[^1] and ^[inline [nested] note].\n\n[^1]: Def.\n";
    assert_nodes!(
        text,
        [
            "FootnoteReference \"[^1]\"",
            "InlineFootnote \"^[inline [nested] note]\" [^[|]]",
            "FootnoteDefinition \"[^1]: Def.\" [[^1]:]",
        ]
    );
}

#[test]
fn bare_urls() {
    let text = "visit https://example.com/a_b_c. and (https://x.y/(z)) [x](https://no.pe)";
    let doc = parse(text);
    let urls: Vec<_> = doc
        .links
        .iter()
        .filter(|l| l.kind == LinkKind::Url)
        .map(|l| l.reference.target.as_str())
        .collect();
    assert_eq!(urls, ["https://example.com/a_b_c", "https://x.y/(z)"]);
}

#[test]
fn tables() {
    let text = "| a | b |\n|---|---|\n| [[x]] | #t |\n";
    let doc = parse(text);
    assert_eq!(doc.tags[0].name, "t");
    assert_eq!(doc.links[0].reference.target, "x");
    assert_nodes!(text, ["TableCell \" [[x]] \""]);
}

#[test]
fn nodes_are_sorted_outer_first() {
    let doc = parse("> **a *b***\n");
    for pair in doc.nodes.windows(2) {
        let (a, b) = (&pair[0].range, &pair[1].range);
        assert!(a.start < b.start || (a.start == b.start && a.end >= b.end));
    }
}

fn markdownish() -> impl Strategy<Value = String> {
    let pieces = prop_oneof![
        Just("# ".to_owned()),
        Just("**".to_owned()),
        Just("*".to_owned()),
        Just("==".to_owned()),
        Just("~~".to_owned()),
        Just("`".to_owned()),
        Just("[[".to_owned()),
        Just("]]".to_owned()),
        Just("|".to_owned()),
        Just("![[".to_owned()),
        Just("[".to_owned()),
        Just("](".to_owned()),
        Just(")".to_owned()),
        Just("> ".to_owned()),
        Just("> [!note] ".to_owned()),
        Just("- [ ] ".to_owned()),
        Just("1. ".to_owned()),
        Just("```".to_owned()),
        Just("%%".to_owned()),
        Just("$".to_owned()),
        Just("#tag".to_owned()),
        Just(" ^id".to_owned()),
        Just("^[".to_owned()),
        Just("---".to_owned()),
        Just("\n".to_owned()),
        Just("\n\n".to_owned()),
        Just(" ".to_owned()),
        Just("https://a.b/c".to_owned()),
        // Fragments of the input that crashes pulldown-cmark 0.13.4.
        Just("![[]".to_owned()),
        Just(" ]()]]".to_owned()),
        "[a-zé漢𐍈]{1,4}",
    ];
    proptest::collection::vec(pieces, 0..40).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn ranges_are_valid(text in markdownish()) {
        let doc = parse(&text);
        for node in &doc.nodes {
            prop_assert!(node.range.end <= text.len());
            prop_assert!(text.is_char_boundary(node.range.start) && text.is_char_boundary(node.range.end), "{node:?}");
            for m in &node.markers {
                prop_assert!(text.is_char_boundary(m.start) && text.is_char_boundary(m.end), "{node:?}");
                prop_assert!(m.start >= node.range.start && m.end <= node.range.end, "{node:?} in {text:?}");
            }
        }
        for link in &doc.links {
            prop_assert!(text.is_char_boundary(link.range.start) && text.is_char_boundary(link.range.end));
            if let Some(d) = &link.display_range {
                prop_assert!(text.is_char_boundary(d.start) && text.is_char_boundary(d.end));
            }
        }
        for tag in &doc.tags {
            let tag_text = text.get(tag.range.clone()).map(str::to_owned);
            prop_assert_eq!(tag_text, Some(format!("#{}", tag.name)));
        }
    }
}

/// pulldown-cmark 0.13.4 panics in its wikilink handling on some malformed
/// input (upstream issue #1108, fixed on main by PR #1111 but not yet
/// released). parse() must survive it by re-parsing without wikilinks.
#[test]
fn survives_upstream_wikilink_panic() {
    let text = " \u{fffd}  ![[] ]()]]";
    let doc = parse(text);
    assert!(
        doc.degraded,
        "the upstream bug no longer triggers: drop the fallback note"
    );
    let text = "ok [[Link]]\n\n ![[] ]()]]";
    let doc = parse(text);
    assert!(doc.degraded);
    // Everything else is still parsed.
    assert!(
        doc.nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Paragraph))
    );
}
