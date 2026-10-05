//! The screenshots in `data/screenshots/` (used by the metainfo), taken of a
//! synthetic vault in the sealed headless session. Not a test; regenerate
//! them with `build-aux/screenshots.sh`.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use igneous::Window;
use igneous_core::settings::{GraphGroup, GraphSettings};

mod common;
use common::*;

const WIDTH: i32 = 1280;
const HEIGHT: i32 = 800;

/// A small vault of geology notes: links, tags, properties, a base.
const NOTES: &[(&str, &str)] = &[
    (
        "Home.md",
        "# Rock notes\n\nNotes from the volcanic geology course.\n\n- [[Rock cycle]]: how rocks turn into each other\n- [[Magma]] and [[Lava]]\n- Igneous rocks: [[Basalt]], [[Granite]], [[Obsidian]], [[Pumice]], [[Gabbro]], [[Rhyolite]], [[Andesite]]\n- Minerals: [[Quartz]], [[Feldspar]], [[Olivine]], [[Pyroxene]], [[Mica]]\n- [[Reading list]]\n",
    ),
    (
        "Rocks/Basalt.md",
        "---\nformed: extrusive\nsilica: 48\ntags:\n  - rock/igneous\n  - rock/volcanic\n---\n# Basalt\n\nThe most common volcanic rock: dark, **fine-grained**, and formed when low-silica [[Lava]] cools quickly at the surface. Most of the ocean floor is basalt, and it makes up shield volcanoes such as Mauna Loa.\n\n> [!tip] Telling basalt from gabbro\n> Same minerals, different cooling: [[Gabbro]] cooled slowly underground, so its crystals are big enough to see.\n\n## Minerals\n\n| Mineral | Share |\n|---|--:|\n| [[Feldspar]] (plagioclase) | 50% |\n| [[Pyroxene]] | 35% |\n| [[Olivine]] | 10% |\n\n## Density\n\n$$\n\\rho = \\frac{m}{V} \\approx 3.0\\ \\mathrm{g/cm^3}\n$$\n\n## To do\n\n- [x] Sketch the columnar joints from the field trip\n- [ ] Compare samples with [[Andesite]]\n- [ ] Read the chapter on ==flood basalts==\n\nSee also [[Rock cycle]] and #rock/volcanic.\n",
    ),
    (
        "Rocks/Granite.md",
        "---\nformed: intrusive\nsilica: 72\ntags:\n  - rock/igneous\n---\n# Granite\n\nCoarse-grained and light-coloured: [[Quartz]], [[Feldspar]] and [[Mica]] that crystallised slowly deep underground from [[Magma]].\n",
    ),
    (
        "Rocks/Obsidian.md",
        "---\nformed: extrusive\nsilica: 74\ntags:\n  - rock/igneous\n  - rock/volcanic\n---\n# Obsidian\n\nVolcanic glass: [[Lava]] rich in silica that cooled so fast no crystals could grow. It breaks with sharp, curved (conchoidal) edges.\n\n> [!note]\n> Chemically it's the same as [[Rhyolite]] and [[Granite]]; only the cooling differs.\n\n## Uses\n\n- Blades and arrowheads since the Stone Age\n- Surgical scalpels, sharper than steel\n- Polished mirrors\n\nOver millions of years it slowly crystallises, which is why most obsidian is geologically young.\n",
    ),
    (
        "Rocks/Pumice.md",
        "---\nformed: extrusive\nsilica: 70\ntags:\n  - rock/igneous\n  - rock/volcanic\n---\n# Pumice\n\nFrothy volcanic glass full of gas bubbles, light enough to float. Close kin of [[Obsidian]].\n",
    ),
    (
        "Rocks/Gabbro.md",
        "---\nformed: intrusive\nsilica: 48\ntags:\n  - rock/igneous\n---\n# Gabbro\n\nThe slow-cooled twin of [[Basalt]]: [[Pyroxene]] and [[Feldspar]] in large crystals.\n",
    ),
    (
        "Rocks/Rhyolite.md",
        "---\nformed: extrusive\nsilica: 73\ntags:\n  - rock/igneous\n  - rock/volcanic\n---\n# Rhyolite\n\nThe volcanic equivalent of [[Granite]], rich in [[Quartz]].\n",
    ),
    (
        "Rocks/Andesite.md",
        "---\nformed: extrusive\nsilica: 60\ntags:\n  - rock/igneous\n  - rock/volcanic\n---\n# Andesite\n\nBetween [[Basalt]] and [[Rhyolite]] in silica; typical of volcanoes above subduction zones.\n",
    ),
    (
        "Minerals/Quartz.md",
        "---\ntype: mineral\nhardness: 7\ntags: [mineral]\n---\n# Quartz\n\nSilicon dioxide. Found in [[Granite]] and [[Rhyolite]].\n",
    ),
    (
        "Minerals/Feldspar.md",
        "---\ntype: mineral\nhardness: 6\ntags: [mineral]\n---\n# Feldspar\n\nThe most abundant mineral group in the crust, in nearly every igneous rock: [[Basalt]], [[Granite]], [[Gabbro]].\n",
    ),
    (
        "Minerals/Olivine.md",
        "---\ntype: mineral\nhardness: 6.5\ntags: [mineral]\n---\n# Olivine\n\nGreen and iron-magnesium rich; crystallises first from basaltic [[Magma]].\n",
    ),
    (
        "Minerals/Pyroxene.md",
        "---\ntype: mineral\nhardness: 5.5\ntags: [mineral]\n---\n# Pyroxene\n\nDark minerals in [[Basalt]] and [[Gabbro]].\n",
    ),
    (
        "Minerals/Mica.md",
        "---\ntype: mineral\nhardness: 2.5\ntags: [mineral]\n---\n# Mica\n\nSplits into thin sheets. Common in [[Granite]].\n",
    ),
    (
        "Processes/Magma.md",
        "---\ntype: process\ntags: [process]\n---\n# Magma\n\nMolten rock below the surface. Cooling underground gives intrusive rocks like [[Granite]] and [[Gabbro]]; erupted, it becomes [[Lava]].\n\n![[Crystallisation]]\n",
    ),
    (
        "Processes/Lava.md",
        "---\ntype: process\ntags: [process]\n---\n# Lava\n\n[[Magma]] at the surface. It cools into [[Basalt]], [[Andesite]], [[Rhyolite]], [[Obsidian]] or [[Pumice]].\n",
    ),
    (
        "Processes/Crystallisation.md",
        "---\ntype: process\ntags: [process]\n---\n# Crystallisation\n\nMinerals crystallise from [[Magma]] in order: [[Olivine]] and [[Pyroxene]] first, [[Quartz]] last.\n",
    ),
    (
        "Processes/Rock cycle.md",
        "---\ntype: process\ntags: [process]\n---\n# Rock cycle\n\n[[Magma]] cools into igneous rock, which weathers, is buried and melts again.\n",
    ),
    (
        "Journal/2026-10-04 Field trip.md",
        "# Field trip\n\nCoastal cliffs: columnar [[Basalt]] over a dyke of [[Gabbro]].\n\n- [x] Collect samples\n- [ ] Label photos\n",
    ),
    (
        "Reading list.md",
        "# Reading list\n\n- *Volcanoes*, the chapter on [[Lava]] flows\n- Field guide to [[Minerals/Quartz|quartz]] varieties\n- An article on knapping Obsidian into blades\n",
    ),
    (
        "Rocks.base",
        "filters:\n  and:\n    - file.hasTag(\"rock/igneous\")\nformulas:\n  composition: 'if(silica >= 63, \"felsic\", if(silica >= 52, \"intermediate\", \"mafic\"))'\nproperties:\n  file.name:\n    displayName: Rock\n  note.formed:\n    displayName: Formed\n  note.silica:\n    displayName: Silica %\n  formula.composition:\n    displayName: Composition\nviews:\n  - type: table\n    name: Igneous rocks\n    order:\n      - file.name\n      - formed\n      - silica\n      - formula.composition\n    sort:\n      - property: silica\n        direction: DESC\n    columnSize:\n      file.name: 220\n",
    ),
];

/// The synthetic vault, named "Rock notes", with a workspace restoring
/// `tabs` (and the inspector, if asked).
struct RockVault {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl RockVault {
    fn path(&self) -> &Path {
        &self.root
    }
}

fn rock_vault(tabs: &[&str], inspector: bool) -> RockVault {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Rock notes");
    for (path, text) in NOTES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    write_workspace(&root, tabs, inspector);
    RockVault { _dir: dir, root }
}

fn write_workspace(root: &Path, tabs: &[&str], inspector: bool) {
    let tabs: Vec<String> = tabs
        .iter()
        .map(|t| {
            let kind = if t.ends_with(".base") { "base" } else { "note" };
            format!(r#"{{"kind":"{kind}","path":"{t}"}}"#)
        })
        .collect();
    std::fs::create_dir_all(root.join(".igneous")).unwrap();
    std::fs::write(
        root.join(".igneous/workspace.json"),
        format!(
            r#"{{"version":1,"tabs":[{}],"activeTab":0,
                "inspector":{{"visible":{inspector}}},
                "sidebar":{{"expanded":["Rocks","Minerals"]}}}}"#,
            tabs.join(",")
        ),
    )
    .unwrap();
}

async fn show(vault: &RockVault) -> Window {
    let window = Window::for_vault(vault.path()).unwrap();
    window.present();
    window.set_default_size(WIDTH, HEIGHT);
    assert!(until(5000, || window.index().is_ready()).await);
    wait(1200).await;
    window
}

/// Puts the cursor on plain text below the heading, so no syntax is shown,
/// and scrolls to the top.
async fn settle_note(window: &Window, after: &str) {
    let Some(note) = window.selected_note() else {
        return;
    };
    note.focus_editor();
    let text = note.text();
    if let Some(at) = text.find(after) {
        note.set_cursor_byte(at + after.len());
    }
    wait(300).await;
    let view = note.view();
    let mut start = view.buffer().start_iter();
    view.scroll_to_iter(&mut start, 0.0, true, 0.0, 0.0);
    wait(600).await;
}

fn set_dark(dark: bool) {
    adw::StyleManager::default().set_color_scheme(if dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
}

/// `IGNEOUS_SCREENSHOTS=dir build-aux/run-ui-tests.sh --test screenshots -- --ignored`
#[gtk::test]
#[ignore = "generates data/screenshots"]
async fn metainfo_screenshots() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOTS") else {
        return;
    };
    let out = Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let save = |window: &Window, name: &str| {
        save_png(window.upcast_ref(), out.join(name).to_str().unwrap());
    };

    // Live Preview, light.
    set_dark(false);
    let dir = rock_vault(
        &["Rocks/Basalt.md", "Processes/Magma.md", "Rocks/Obsidian.md"],
        false,
    );
    let window = show(&dir).await;
    settle_note(&window, "Mauna Loa").await;
    save(&window, "live-preview.png");
    window.close();

    // Backlinks beside a note, dark, in Catppuccin.
    set_dark(true);
    let dir = rock_vault(&["Rocks/Obsidian.md", "Rocks/Basalt.md"], true);
    std::fs::write(
        dir.path().join(".igneous/appearance.json"),
        r#"{"version":1,"editorTheme":"catppuccin"}"#,
    )
    .unwrap();
    let window = show(&dir).await;
    settle_note(&window, "curved (conchoidal) edges.").await;
    save(&window, "backlinks-dark.png");
    window.close();

    // The graph, dark, with rocks and minerals in colour groups.
    let dir = rock_vault(&["Home.md"], false);
    let window = show(&dir).await;
    WidgetExt::activate_action(&window, "win.graph", None).unwrap();
    let page = window.graph_page().expect("a graph tab");
    assert!(until(5000, || !page.view().model().graph.is_empty()).await);
    page.set_settings(GraphSettings {
        show_tags: false,
        link_distance: 140.0,
        repel_force: 16.0,
        groups: vec![
            GraphGroup {
                query: "path:Rocks".into(),
                color: None,
            },
            GraphGroup {
                query: "path:Minerals".into(),
                color: None,
            },
            GraphGroup {
                query: "path:Processes".into(),
                color: None,
            },
        ],
        ..page.settings()
    });
    let view = page.view().clone();
    assert!(until(15_000, || view.is_settled()).await);
    view.fit();
    wait(500).await;
    save(&window, "graph.png");
    window.close();

    // A base listing the rocks' properties, light.
    set_dark(false);
    let dir = rock_vault(&["Rocks.base", "Rocks/Granite.md"], false);
    let window = show(&dir).await;
    wait(800).await;
    save(&window, "bases.png");
    window.close();

    // Uncommitted changes and a diff.
    let dir = rock_vault(&["Rocks/Pumice.md"], false);
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "Course notes"]);
    let pumice = dir.path().join("Rocks/Pumice.md");
    let text = std::fs::read_to_string(&pumice).unwrap();
    std::fs::write(
        &pumice,
        text.replace(
            "light enough to float.",
            "light enough to float. Its bubbles are frozen gas that escaped as the lava erupted.",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Rocks/Scoria.md"),
        "---\nformed: extrusive\ntags:\n  - rock/igneous\n---\n# Scoria\n\nLike [[Pumice]], but darker and denser.\n",
    )
    .unwrap();
    let mica = dir.path().join("Minerals/Mica.md");
    std::fs::write(
        &mica,
        "---\ntype: mineral\nhardness: 2.5\ntags: [mineral]\n---\n# Mica\n\nSplits into thin, flexible sheets. Common in [[Granite]].\n",
    )
    .unwrap();
    git(dir.path(), &["add", "Minerals/Mica.md"]);
    let window = show(&dir).await;
    assert!(until(5000, || window.sync().is_available()).await);
    WidgetExt::activate_action(&window, "win.show-changes", None).unwrap();
    window.open_changes(&p("Rocks/Pumice.md"), false, false);
    wait(1500).await;
    save(&window, "changes.png");
    window.close();
}
