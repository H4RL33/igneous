//! UI tests on a copy of the fixture vault. They need a display; run them
//! with `build-aux/run-ui-tests.sh`, which uses a private headless session.

use std::path::Path;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use igneous::{NoteState, SyncState, Window};
use igneous_core::VaultPath;
use sourceview::prelude::*;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vaults/basic");

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

/// A fresh copy of the fixture vault, optionally with an autosave delay.
fn vault(autosave_ms: Option<u32>) -> tempfile::TempDir {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
    }
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE), dir.path());
    if let Some(ms) = autosave_ms {
        std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
        std::fs::write(
            dir.path().join(".igneous/vault.json"),
            format!("{{\"version\":1,\"editor\":{{\"autosaveDelayMs\":{ms}}}}}"),
        )
        .unwrap();
    }
    dir
}

/// Opens and shows a window: GTK only closes windows that have been shown.
fn open(dir: &tempfile::TempDir) -> Window {
    let window = Window::for_vault(dir.path()).unwrap();
    window.present();
    window
}

fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

async fn wait(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

#[gtk::test]
async fn edits_autosave_and_keep_line_endings() {
    let dir = vault(Some(100));
    let window = open(&dir);
    window.open_path(&p("Projects/Windows.md"), false);
    let note = window.selected_note().unwrap();
    assert_eq!(
        note.text(),
        "# Windows note\n\nThis note uses CRLF line endings.\n"
    );

    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "More.\n");
    assert_eq!(note.state(), NoteState::Dirty);
    wait(600).await;
    assert_eq!(note.state(), NoteState::Clean);
    assert_eq!(
        std::fs::read(dir.path().join("Projects/Windows.md")).unwrap(),
        b"# Windows note\r\n\r\nThis note uses CRLF line endings.\r\nMore.\r\n"
    );
    window.close();
}

#[gtk::test]
async fn external_changes_reload_clean_notes() {
    let dir = vault(Some(100));
    let window = open(&dir);
    window.open_path(&p("Daily/2026-10-04.md"), false);
    let note = window.selected_note().unwrap();
    wait(300).await;
    std::fs::write(
        dir.path().join("Daily/2026-10-04.md"),
        "# Changed elsewhere\n",
    )
    .unwrap();
    wait(1200).await;
    assert_eq!(note.text(), "# Changed elsewhere\n");
    assert_eq!(note.state(), NoteState::Clean);
    window.close();
}

#[gtk::test]
async fn unsaved_edits_are_never_overwritten() {
    let dir = vault(Some(60_000));
    let window = open(&dir);
    window.open_path(&p("Projects/Ideas.md"), false);
    let note = window.selected_note().unwrap();
    wait(300).await;
    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "Mine.\n");
    std::fs::write(dir.path().join("Projects/Ideas.md"), "Theirs.\n").unwrap();
    wait(1200).await;
    assert_eq!(note.state(), NoteState::ChangedOnDisk);
    note.flush();
    window.close();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Projects/Ideas.md")).unwrap(),
        "Theirs.\n"
    );
}

#[gtk::test]
async fn renames_follow_open_tabs() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Projects/Igneous/Roadmap.md"), false);
    window.rename(&p("Projects/Igneous"), &p("Projects/Igneous app"));
    assert!(dir.path().join("Projects/Igneous app/Roadmap.md").is_file());
    assert_eq!(
        window.tab_paths(),
        vec![p("Projects/Igneous app/Roadmap.md")]
    );
    window.rename(
        &p("Projects/Igneous app/Roadmap.md"),
        &p("Projects/Igneous app/Plan.md"),
    );
    assert_eq!(
        window.selected_path(),
        Some(p("Projects/Igneous app/Plan.md"))
    );
    window.close();
}

#[gtk::test]
async fn workspace_is_restored() {
    let dir = vault(None);
    {
        let window = open(&dir);
        window.open_path(&p("Home.md"), true);
        window.open_path(&p("Projects/Ideas.md"), true);
        window.open_path(&p("Home.md"), false);
        window.close();
    }
    assert!(dir.path().join(".igneous/workspace.json").is_file());
    let window = open(&dir);
    assert_eq!(
        window.tab_paths(),
        vec![p("Home.md"), p("Projects/Ideas.md")]
    );
    assert_eq!(window.selected_path(), Some(p("Home.md")));
    window.close();
}

#[gtk::test]
async fn opening_and_closing_writes_nothing() {
    let dir = vault(None);
    let window = open(&dir);
    wait(300).await;
    window.close();
    assert!(!dir.path().join(".igneous").exists());
    for entry in walk(Path::new(FIXTURE)) {
        let rel = entry.strip_prefix(FIXTURE).unwrap();
        assert_eq!(
            std::fs::read(&entry).unwrap(),
            std::fs::read(dir.path().join(rel)).unwrap(),
            "{} changed",
            rel.display()
        );
    }
}

#[gtk::test]
async fn file_tree_lists_and_follows_the_disk() {
    let dir = vault(None);
    let window = open(&dir);
    assert_eq!(
        window.sidebar_paths(),
        vec![p("Attachments"), p("Daily"), p("Projects"), p("Home.md")]
    );
    std::fs::write(dir.path().join("Inbox.md"), "").unwrap();
    std::fs::remove_file(dir.path().join("Home.md")).unwrap();
    wait(1200).await;
    assert_eq!(
        window.sidebar_paths(),
        vec![p("Attachments"), p("Daily"), p("Projects"), p("Inbox.md")]
    );
    window.close();
}

fn scheme_id(window: &Window) -> String {
    let note = window.selected_note().unwrap();
    note.buffer().style_scheme().unwrap().id().to_string()
}

#[gtk::test]
async fn editor_themes_apply_and_persist() {
    let dir = vault(None);
    {
        let window = open(&dir);
        window.open_path(&p("Home.md"), false);
        assert!(
            scheme_id(&window).starts_with("igneous-adwaita-"),
            "{}",
            scheme_id(&window)
        );
        window.set_editor_theme("catppuccin");
        assert!(scheme_id(&window).starts_with("igneous-catppuccin-"));
        // Notes opened later get it too.
        window.open_path(&p("Projects/Ideas.md"), true);
        assert!(scheme_id(&window).starts_with("igneous-catppuccin-"));
        window.close();
    }
    let appearance = std::fs::read_to_string(dir.path().join(".igneous/appearance.json")).unwrap();
    assert!(
        appearance.contains("\"editorTheme\": \"catppuccin\""),
        "{appearance}"
    );
    let window = open(&dir);
    assert_eq!(window.editor_theme_id(), "catppuccin");
    window.close();
}

#[gtk::test]
async fn vault_themes_are_found() {
    let dir = vault(None);
    std::fs::create_dir_all(dir.path().join(".igneous/themes")).unwrap();
    std::fs::write(
        dir.path().join(".igneous/themes/paper.toml"),
        "name = \"Paper\"\n[light]\nbackground = \"#fafaf7\"\nforeground = \"#222222\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".igneous/themes/broken.toml"), "name = ").unwrap();
    let window = open(&dir);
    let themes = window.themes();
    assert_eq!(themes.get("paper").map(|t| t.name.as_str()), Some("Paper"));
    assert_eq!(themes.errors.len(), 1);
    window.open_path(&p("Home.md"), false);
    window.set_editor_theme("paper");
    assert!(scheme_id(&window).starts_with("igneous-paper-"));
    window.close();
}

// --- Git sync ------------------------------------------------------------------
//
// These rely on the sealed session's own Git configuration (identity, no
// signing), set up by build-aux/headless-session.sh.

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The fixture vault as a repository whose `main` is pushed to a bare
/// remote. The second directory holds the remote and any other clones.
fn synced_vault() -> (tempfile::TempDir, tempfile::TempDir) {
    let dir = vault(Some(100));
    let remotes = tempfile::tempdir().unwrap();
    git(remotes.path(), &["init", "-q", "--bare", "remote.git"]);
    let remote = remotes.path().join("remote.git");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(
        dir.path(),
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
    (dir, remotes)
}

/// Another computer: a clone of the remote.
fn other_clone(remotes: &tempfile::TempDir) -> std::path::PathBuf {
    let path = remotes.path().join("other");
    if !path.exists() {
        git(remotes.path(), &["clone", "-q", "remote.git", "other"]);
    }
    path
}

fn remote_log(remotes: &tempfile::TempDir) -> String {
    git(
        &remotes.path().join("remote.git"),
        &["log", "--format=%s", "main"],
    )
}

/// Waits up to `ms` for `f` to hold.
async fn until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..ms / 50 {
        if f() {
            return true;
        }
        wait(50).await;
    }
    f()
}

#[gtk::test]
async fn git_sync_round_trip() {
    let (dir, remotes) = synced_vault();
    let window = open(&dir);
    assert!(until(5000, || window.sync().is_available()).await);

    window.open_path(&p("Home.md"), false);
    let note = window.selected_note().unwrap();
    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "Synced from Igneous.\n");
    note.flush();

    // The change shows in the Changes pane's data and as a diff.
    window.sync().refresh_now();
    assert!(
        until(3000, || window
            .sync()
            .status()
            .is_some_and(|s| s.entry("Home.md").is_some()))
        .await
    );
    window.open_changes(&p("Home.md"), false, false);
    assert!(
        until(3000, || window
            .selected_tab_text()
            .is_some_and(|t| t.contains("+Synced from Igneous.")))
        .await
    );

    // Sync now commits and pushes.
    WidgetExt::activate_action(&window, "win.sync-now", None).unwrap();
    assert!(
        until(10_000, || remote_log(&remotes)
            .starts_with("vault backup: "))
        .await
    );
    assert!(until(3000, || window.sync().state() == SyncState::Idle).await);
    assert!(!window.sync().status().unwrap().is_dirty());

    // Changes from elsewhere arrive with a pull, and open notes reload.
    window.open_path(&p("Daily/2026-10-04.md"), true);
    let daily = window.selected_note().unwrap();
    let other = other_clone(&remotes);
    std::fs::write(other.join("Daily/2026-10-04.md"), "# Written elsewhere\n").unwrap();
    git(&other, &["commit", "-q", "-am", "elsewhere"]);
    git(&other, &["push", "-q"]);
    WidgetExt::activate_action(&window, "win.pull", None).unwrap();
    assert!(until(10_000, || daily.text() == "# Written elsewhere\n").await);
    assert_eq!(daily.state(), NoteState::Clean);
    window.close();
}

#[gtk::test]
async fn conflicts_pause_sync_until_resolved() {
    let (dir, remotes) = synced_vault();
    let other = other_clone(&remotes);
    let home = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    std::fs::write(other.join("Home.md"), format!("Theirs\n{home}")).unwrap();
    git(&other, &["commit", "-q", "-am", "theirs"]);
    git(&other, &["push", "-q"]);

    let window = open(&dir);
    assert!(until(5000, || window.sync().is_available()).await);
    window.open_path(&p("Home.md"), false);
    let note = window.selected_note().unwrap();
    let buffer = note.buffer();
    buffer.insert(&mut buffer.start_iter(), "Mine\n");

    WidgetExt::activate_action(&window, "win.sync-now", None).unwrap();
    assert!(until(10_000, || window.sync().state() == SyncState::Paused).await);
    assert_eq!(window.sync().status().unwrap().conflicts().count(), 1);
    // The note reloads with the conflict, and the bar resolves it.
    assert!(until(3000, || note.has_conflicts()).await);
    WidgetExt::activate_action(&note, "note.keep-mine", None).unwrap();
    assert_eq!(note.text(), format!("Mine\n{home}"));
    assert!(!note.has_conflicts());
    note.flush();

    // Automatic passes stay off until the merge is committed.
    WidgetExt::activate_action(&window, "win.sync-now", None).unwrap();
    wait(500).await;
    assert_eq!(window.sync().state(), SyncState::Paused);
    let result = window
        .sync()
        .call(|git| {
            git.stage(&["Home.md"])?;
            git.conclude()
        })
        .await
        .unwrap();
    result.unwrap();
    WidgetExt::activate_action(&window, "win.sync-now", None).unwrap();
    assert!(until(10_000, || remote_log(&remotes).lines().count() == 4).await);
    assert_eq!(window.sync().state(), SyncState::Idle);
    git(&other, &["pull", "-q"]);
    assert_eq!(
        std::fs::read_to_string(other.join("Home.md")).unwrap(),
        format!("Mine\n{home}")
    );
    window.close();
}

#[gtk::test]
async fn history_opens_old_versions() {
    let (dir, _remotes) = synced_vault();
    let original = std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    std::fs::write(dir.path().join("Home.md"), "# Rewritten\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "rewrite"]);

    let window = open(&dir);
    assert!(until(5000, || window.sync().is_available()).await);
    let log = window
        .sync()
        .call(|git| git.log("Home.md", 10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(log.len(), 2);
    window.open_version(window.sync(), &p("Home.md"), log[1].clone());
    assert!(
        until(3000, || window.selected_tab_text().as_deref()
            == Some(original.as_str()))
        .await
    );
    window.close();
}

#[gtk::test]
async fn vaults_without_git_hide_sync() {
    let dir = vault(None);
    let window = open(&dir);
    wait(500).await;
    assert!(!window.sync().is_available());
    assert_eq!(window.sync().state(), SyncState::Unavailable);
    window.close();
}

#[gtk::test]
fn application_registers_actions() {
    let app = igneous::Application::new();
    app.register(gtk::gio::Cancellable::NONE).unwrap();
    for action in ["quit", "about", "new-window", "shortcuts"] {
        assert!(
            app.lookup_action(action).is_some(),
            "app.{action} is missing"
        );
    }
    assert_eq!(app.accels_for_action("win.quick-switcher"), ["<Control>o"]);
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Not a test: saves a screenshot of a window for a visual check.
/// `IGNEOUS_SCREENSHOT=out.png build-aux/run-ui-tests.sh -- --ignored screenshot`
#[gtk::test]
#[ignore = "visual check"]
async fn screenshot() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOT") else {
        return;
    };
    let tabs = std::env::var("IGNEOUS_SCREENSHOT_TABS").unwrap_or_else(|_| {
        r#"[{"kind":"note","path":"Home.md"},
            {"kind":"note","path":"Projects/Igneous/Roadmap.md"},
            {"kind":"note","path":"Projects/Ideas.md"}]"#
            .into()
    });
    let width: i32 = std::env::var("IGNEOUS_SCREENSHOT_WIDTH")
        .ok()
        .and_then(|w| w.parse().ok())
        .unwrap_or(1100);
    let dir = vault(None);
    // Expanded folders and three tabs, as if restored from a previous session.
    std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
    std::fs::write(
        dir.path().join(".igneous/workspace.json"),
        format!(
            r#"{{"version":1,"tabs":{tabs},"activeTab":0,
                "sidebar":{{"expanded":["Attachments","Projects","Projects/Igneous"]}}}}"#
        ),
    )
    .unwrap();
    if std::env::var("IGNEOUS_SCREENSHOT_DARK").is_ok() {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    }
    if let Ok(theme) = std::env::var("IGNEOUS_SCREENSHOT_THEME") {
        std::fs::write(
            dir.path().join(".igneous/appearance.json"),
            format!(r#"{{"version":1,"editorTheme":"{theme}"}}"#),
        )
        .unwrap();
    }
    let window = open(&dir);
    window.set_default_size(width, 720);
    wait(1500).await;
    save_png(window.upcast_ref(), &out);
    WidgetExt::activate_action(&window, "win.preferences", None).unwrap();
    wait(800).await;
    save_png(
        window.upcast_ref(),
        &out.replace(".png", "-preferences.png"),
    );
    window.close();

    // The Changes pane and a diff, in a vault with uncommitted changes.
    if std::env::var("IGNEOUS_SCREENSHOT_GIT").is_ok() {
        let (dir, _remotes) = synced_vault();
        std::fs::write(dir.path().join("Home.md"), "# Home\n\nRewritten today.\n").unwrap();
        std::fs::write(dir.path().join("Projects/New idea.md"), "An idea.\n").unwrap();
        std::fs::write(dir.path().join("Projects/Ideas.md"), "Staged.\n").unwrap();
        git(dir.path(), &["add", "Projects/Ideas.md"]);
        let window = open(&dir);
        window.set_default_size(width, 720);
        assert!(until(5000, || window.sync().is_available()).await);
        WidgetExt::activate_action(&window, "win.show-changes", None).unwrap();
        window.open_changes(&p("Home.md"), false, false);
        wait(1500).await;
        save_png(window.upcast_ref(), &out.replace(".png", "-git.png"));
        window.close();
    }

    // The vault picker, with one recent vault.
    let app = igneous::Application::new();
    let picker = igneous::VaultPicker::new(&app);
    picker.present();
    wait(800).await;
    save_png(picker.upcast_ref(), &out.replace(".png", "-picker.png"));
    picker.close();
}

fn save_png(window: &gtk::Window, out: &str) {
    // Paint the whole window: header bars are translucent over its background.
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let texture = window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None);
    texture.save_to_png(out).unwrap();
}

/// Opt-in: opens copies of real vaults, loads every note, closes, and checks
/// that no note changed by a single byte. Prints only counts.
/// `IGNEOUS_CORPUS=/vault/one:/vault/two build-aux/run-ui-tests.sh -p igneous --test ui -- --ignored corpus`
#[gtk::test]
#[ignore = "needs IGNEOUS_CORPUS"]
async fn corpus_round_trip() {
    let Ok(corpus) = std::env::var("IGNEOUS_CORPUS") else {
        return;
    };
    for source in corpus.split(':').filter(|s| !s.is_empty()) {
        let dir = tempfile::tempdir().unwrap();
        copy_visible(Path::new(source), dir.path());
        let before: Vec<(std::path::PathBuf, Vec<u8>)> = walk(dir.path())
            .into_iter()
            .map(|p| {
                let bytes = std::fs::read(&p).unwrap();
                (p, bytes)
            })
            .collect();
        let window = open(&dir);
        wait(300).await;
        let notes: Vec<VaultPath> = before
            .iter()
            .filter(|(p, _)| p.extension().is_some_and(|e| e == "md"))
            .filter_map(|(p, _)| VaultPath::from_fs(dir.path(), p).ok())
            .collect();
        for note in &notes {
            window.open_path(note, false);
        }
        window.close();
        let mut changed = 0;
        for (path, bytes) in &before {
            if std::fs::read(path).ok().as_ref() != Some(bytes) {
                changed += 1;
            }
        }
        let extra: Vec<_> = walk(dir.path())
            .into_iter()
            .filter(|p| !before.iter().any(|(b, _)| b == p))
            .map(|p| p.strip_prefix(dir.path()).unwrap().display().to_string())
            .collect();
        eprintln!(
            "{} files, {} notes opened, {} changed, new files: {:?}",
            before.len(),
            notes.len(),
            changed,
            extra
        );
        assert_eq!(changed, 0);
        assert!(
            extra.iter().all(|p| p == ".igneous/workspace.json"),
            "{extra:?}"
        );
    }
}

/// Copies a vault without its dot-folders (`.git`, `.obsidian`, …).
fn copy_visible(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_visible(&entry.path(), &dest);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}
