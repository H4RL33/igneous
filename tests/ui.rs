//! UI tests on a copy of the fixture vault. They need a display; run them
//! with `build-aux/run-ui-tests.sh`, which uses a private headless session.

use igneous_core::VaultPath;
use std::path::Path;

use adw::prelude::*;
use igneous::{NoteState, SyncState, Window};
use sourceview::prelude::*;

mod common;
use common::*;

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

#[gtk::test]
async fn back_and_forward_follow_the_tab() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    window.open_path(&p("Projects/Ideas.md"), false);
    window.open_path(&p("Daily/2026-10-04.md"), false);
    assert_eq!(window.tab_paths().len(), 1);
    WidgetExt::activate_action(&window, "win.go-back", None).unwrap();
    assert_eq!(window.selected_path(), Some(p("Projects/Ideas.md")));
    WidgetExt::activate_action(&window, "win.go-back", None).unwrap();
    assert_eq!(window.selected_path(), Some(p("Home.md")));
    WidgetExt::activate_action(&window, "win.go-forward", None).unwrap();
    assert_eq!(window.selected_path(), Some(p("Projects/Ideas.md")));
    // History is saved with the tab.
    window.save_workspace();
    let saved = std::fs::read_to_string(dir.path().join(".igneous/workspace.json")).unwrap();
    assert!(
        saved.contains(
            r#""back": [
        "Home.md"
      ]"#
        ),
        "{saved}"
    );
    window.close();
}

#[gtk::test]
async fn renames_update_links() {
    let dir = vault(None);
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    let backlinks = window
        .index()
        .query(|index| index.backlinks(&p("Projects/Igneous/Roadmap.md")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].source, p("Home.md"));

    window.rename(
        &p("Projects/Igneous/Roadmap.md"),
        &p("Projects/Igneous/Plan.md"),
    );
    let home = || std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(
        until(3000, || home().contains("See [[Plan]] and")).await,
        "{}",
        home()
    );
    // Only the link changed.
    assert_eq!(
        home().replace("[[Plan]]", "[[Roadmap]]"),
        std::fs::read_to_string(Path::new(FIXTURE).join("Home.md")).unwrap()
    );
    window.close();
}

#[gtk::test]
async fn following_a_missing_link_creates_the_note() {
    let dir = vault(None);
    let window = open(&dir);
    let link = igneous_markdown::LinkRef::parse_wiki("Brand new idea", false);
    window.follow_link(&link, Some(&p("Home.md")), false);
    assert!(dir.path().join("Brand new idea.md").is_file());
    assert_eq!(window.selected_path(), Some(p("Brand new idea.md")));
    window.close();
}

// --- Git sync ------------------------------------------------------------------
//
// These rely on the sealed session's own Git configuration (identity, no
// signing), set up by build-aux/headless-session.sh.

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
    if let Ok(extra) = std::env::var("IGNEOUS_SCREENSHOT_EXTRA") {
        let name = Path::new(&extra).file_name().unwrap();
        std::fs::copy(&extra, dir.path().join(name)).unwrap();
    }
    // Expanded folders and three tabs, as if restored from a previous session.
    std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
    std::fs::write(
        dir.path().join(".igneous/workspace.json"),
        format!(
            r#"{{"version":1,"tabs":{tabs},"activeTab":0,
                "inspector":{{"visible":{}}},
                "sidebar":{{"expanded":["Attachments","Projects","Projects/Igneous"]}}}}"#,
            std::env::var("IGNEOUS_SCREENSHOT_INSPECTOR").is_ok()
        ),
    )
    .unwrap();
    if std::env::var("IGNEOUS_SCREENSHOT_DARK").is_ok() {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    }
    if std::env::var("IGNEOUS_SCREENSHOT_MENTIONS").is_ok() {
        std::fs::write(
            dir.path().join(".igneous/vault.json"),
            r#"{"version":1,"editor":{"backlinksInDocument":true}}"#,
        )
        .unwrap();
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
    if let Ok(query) = std::env::var("IGNEOUS_SCREENSHOT_SEARCH") {
        window.search_vault(&query);
        wait(1000).await;
    }
    if std::env::var("IGNEOUS_SCREENSHOT_TOP").is_ok()
        && let Some(note) = window.selected_note()
    {
        let view = note.view();
        let mut start = view.buffer().start_iter();
        view.scroll_to_iter(&mut start, 0.0, true, 0.0, 0.0);
        wait(500).await;
    }
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

/// Opt-in: opens every note of copies of real vaults in Live Preview,
/// scrolls through it and types in it, and reports stalls (main-loop gaps
/// longer than they should be) and GTK warnings. Prints only numbers and
/// widget types, never names or text.
/// `IGNEOUS_CORPUS=/vault/one:/vault/two build-aux/run-ui-tests.sh -p igneous --test ui -- --ignored live_preview_stress --nocapture`
#[gtk::test]
#[ignore = "needs IGNEOUS_CORPUS"]
async fn live_preview_stress() {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let Ok(corpus) = std::env::var("IGNEOUS_CORPUS") else {
        return;
    };
    // GTK warnings, with numbers and addresses taken out.
    static WARNINGS: Mutex<BTreeMap<String, (usize, String)>> = Mutex::new(BTreeMap::new());
    gtk::glib::log_set_writer_func(|level, fields| {
        if matches!(
            level,
            gtk::glib::LogLevel::Warning | gtk::glib::LogLevel::Critical
        ) {
            let message = fields
                .iter()
                .find(|f| f.key() == "MESSAGE")
                .and_then(|f| f.value_str())
                .unwrap_or_default();
            let domain = fields
                .iter()
                .find(|f| f.key() == "GLIB_DOMAIN")
                .and_then(|f| f.value_str())
                .unwrap_or_default();
            if domain != "libenchant" {
                let raw = message.to_owned();
                let message: String = message
                    .split_whitespace()
                    .map(|w| {
                        if w.starts_with("0x") || w.trim_matches(',').parse::<i64>().is_ok() {
                            "#"
                        } else {
                            w
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                WARNINGS
                    .lock()
                    .unwrap()
                    .entry(format!("{domain}: {message}"))
                    .or_insert((0, raw))
                    .0 += 1;
            }
        }
        gtk::glib::LogWriterOutput::Handled
    });
    let warnings = || {
        WARNINGS
            .lock()
            .unwrap()
            .values()
            .map(|v| v.0)
            .sum::<usize>()
    };

    // A watchdog: the main loop should get back to it every few
    // milliseconds. Longer gaps are stalls, attributed to whatever the test
    // was doing when they started.
    let phase = std::rc::Rc::new(std::cell::RefCell::new(String::from("startup")));
    let stalls = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(String, Duration)>::new()));
    {
        let (phase, stalls) = (phase.clone(), stalls.clone());
        let last = std::cell::Cell::new((Instant::now(), String::new()));
        // Under gdb (`handle SIGUSR1 stop nopass`), interrupt stalls as
        // they happen, to see what the main thread is doing.
        static BEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let start = Instant::now();
        if std::env::var("IGNEOUS_STALL_SIGNAL").is_ok() {
            std::thread::spawn(move || {
                let mut signalled = 0;
                loop {
                    std::thread::sleep(Duration::from_millis(40));
                    let beat = BEAT.load(std::sync::atomic::Ordering::Relaxed);
                    let now = start.elapsed().as_millis() as u64;
                    if beat > 0 && now - beat > 150 && beat != signalled {
                        signalled = beat;
                        std::process::Command::new("kill")
                            .args(["-USR1", &std::process::id().to_string()])
                            .status()
                            .ok();
                    }
                }
            });
        }
        gtk::glib::timeout_add_local(Duration::from_millis(5), move || {
            BEAT.store(
                start.elapsed().as_millis() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            let (at, during) = last.replace((Instant::now(), phase.borrow().clone()));
            let gap = at.elapsed();
            if gap > Duration::from_millis(100) {
                stalls.borrow_mut().push((during, gap));
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    let set_phase = |name: String| *phase.borrow_mut() = name;
    let report = |label: &str| {
        for (during, gap) in stalls.borrow_mut().drain(..) {
            eprintln!("stall {gap:?} ({label}; started during {during})");
        }
    };

    let mut worst_overall = Duration::ZERO;
    let pages = ["backlinks", "outgoing", "outline", "graph"];
    for (v, source) in corpus.split(':').filter(|s| !s.is_empty()).enumerate() {
        let dir = tempfile::tempdir().unwrap();
        copy_visible(Path::new(source), dir.path());
        // With the vault's own settings and history, but nowhere to push to.
        for state in [".igneous", ".git"] {
            if Path::new(source).join(state).is_dir() {
                copy_all(&Path::new(source).join(state), &dir.path().join(state));
            }
        }
        if dir.path().join(".git").is_dir() {
            for remote in git(dir.path(), &["remote"]).lines() {
                git(dir.path(), &["remote", "remove", remote]);
            }
            let _ = std::fs::remove_dir_all(dir.path().join(".git/hooks"));
        }
        let limit = std::env::var("IGNEOUS_CORPUS_LIMIT")
            .ok()
            .and_then(|l| l.parse().ok())
            .unwrap_or(usize::MAX);
        let notes: Vec<VaultPath> = walk(dir.path())
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .filter_map(|p| VaultPath::from_fs(dir.path(), &p).ok())
            .take(limit)
            .collect();
        set_phase(format!("vault {v} opening"));
        let started = Instant::now();
        let window = open(&dir);
        window.set_default_size(1400, 900);
        window.show_inspector("backlinks");
        eprintln!("vault {v}: opened in {:?}", started.elapsed());
        set_phase(format!("vault {v} settling"));
        wait(2000).await;
        report(&format!("vault {v} startup"));
        for (n, note_path) in notes.iter().enumerate() {
            let before = warnings();
            set_phase(format!("note {n} opening"));
            let started = Instant::now();
            window.open_path(note_path, false);
            let opening = started.elapsed();
            let Some(note) = window.selected_note() else {
                continue;
            };
            window.show_inspector(pages[n % pages.len()]);
            if let Ok(mode) = std::env::var("IGNEOUS_STRESS_MODE") {
                WidgetExt::activate_action(&window, "win.mode", Some(&mode.to_variant())).unwrap();
            }
            let view = note.view();
            let size = note.text().len();
            // Every pause should take ~16 ms; anything much longer means the
            // main loop was busy.
            let mut worst = opening;
            let mut tick = |started: Instant| worst = worst.max(started.elapsed());
            set_phase(format!("note {n} settling"));
            for _ in 0..5 {
                let t = Instant::now();
                wait(16).await;
                tick(t);
            }
            // Scroll through.
            set_phase(format!("note {n} scrolling"));
            if let Some(adj) = view.vadjustment() {
                let steps = 12;
                for i in 0..=steps {
                    adj.set_value(adj.upper() * f64::from(i) / f64::from(steps));
                    let t = Instant::now();
                    wait(16).await;
                    tick(t);
                }
            }
            // Type in the middle and at the end.
            set_phase(format!("note {n} typing"));
            let buffer = view.buffer();
            for at in [buffer.char_count() / 2, buffer.char_count()] {
                buffer.place_cursor(&buffer.iter_at_offset(at));
                for _ in 0..5 {
                    let t = Instant::now();
                    buffer.insert_at_cursor("x");
                    wait(16).await;
                    tick(t);
                }
            }
            let frames = |count: usize| async move {
                for _ in 0..count {
                    wait(16).await;
                }
            };
            // Let it autosave, and the index and Git catch up.
            set_phase(format!("note {n} saving"));
            frames(90).await;
            // Hide and show the inspector and the sidebar, which animates
            // the text column's width, then shrink and grow the window.
            if n % 10 == 0 {
                let inspector = find_end_split(window.upcast_ref()).unwrap();
                set_phase(format!("note {n} toggling inspector"));
                inspector.set_show_sidebar(false);
                frames(25).await;
                inspector.set_show_sidebar(true);
                frames(25).await;
                set_phase(format!("note {n} toggling sidebar"));
                WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
                frames(25).await;
                WidgetExt::activate_action(&window, "win.toggle-sidebar", None).unwrap();
                frames(25).await;
                set_phase(format!("note {n} resizing"));
                for width in (1000..=1400)
                    .rev()
                    .step_by(40)
                    .chain((1000..=1400).step_by(40))
                {
                    window.set_default_size(width, 900);
                    frames(1).await;
                }
            }
            // Idle for a while: nothing should keep happening.
            set_phase(format!("note {n} idle"));
            let idle = warnings();
            for _ in 0..10 {
                let t = Instant::now();
                wait(16).await;
                tick(t);
            }
            let idle = warnings() - idle;
            set_phase(format!("note {n} closing"));
            note.discard();
            let warned = warnings() - before;
            if worst > Duration::from_millis(80) || warned > 0 {
                let mut kinds = BTreeMap::<String, usize>::new();
                for kind in view.overlay_kinds() {
                    let kind = kind.split(':').next().unwrap_or_default().to_owned();
                    *kinds.entry(kind).or_default() += 1;
                }
                eprintln!(
                    "vault {v} note {n}: {size} bytes, opened in {opening:?}, worst gap \
                     {worst:?}, {warned} warnings ({idle} while idle), widgets {kinds:?}"
                );
            }
            report(&format!("vault {v} note {n}"));
            worst_overall = worst_overall.max(worst);
        }
        set_phase(format!("vault {v} closing"));
        window.close();
        wait(200).await;
        report(&format!("vault {v} closing"));
        eprintln!("vault {v}: {} notes", notes.len());
    }
    eprintln!("worst gap overall: {worst_overall:?}");
    for (count, first) in WARNINGS.lock().unwrap().values() {
        eprintln!("{count:6} like {first}");
    }
}

/// The inspector's split view.
fn find_end_split(widget: &gtk::Widget) -> Option<adw::OverlaySplitView> {
    if let Some(split) = widget.downcast_ref::<adw::OverlaySplitView>()
        && split.sidebar_position() == gtk::PackType::End
    {
        return Some(split.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_end_split(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Copies a folder and everything in it.
fn copy_all(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_all(&entry.path(), &dest);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest).unwrap();
        }
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

/// The header bar's mode button cycles Live Preview, Source and Reading, and
/// says which mode a click switches to.
#[gtk::test]
async fn mode_button_cycles_modes() {
    use igneous_editor::Mode;
    let dir = vault(Some(100));
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    let note = window.selected_note().unwrap();
    let button = find_widget(window.upcast_ref(), &|w| {
        w.downcast_ref::<gtk::Button>()
            .is_some_and(|b| b.action_name().as_deref() == Some("win.cycle-mode"))
    })
    .and_downcast::<gtk::Button>()
    .unwrap();
    assert!(button.is_visible());
    assert_eq!(note.mode(), Mode::Live);
    assert_eq!(button.tooltip_text().as_deref(), Some("Switch to Source"));
    for (mode, next) in [
        (Mode::Source, "Switch to Reading"),
        (Mode::Reading, "Switch to Live Preview"),
        (Mode::Live, "Switch to Source"),
    ] {
        button.emit_clicked();
        assert_eq!(note.mode(), mode);
        assert_eq!(button.tooltip_text().as_deref(), Some(next));
    }
    window.close();
}

/// The first widget under `widget` that `matches`.
fn find_widget(
    widget: &gtk::Widget,
    matches: &dyn Fn(&gtk::Widget) -> bool,
) -> Option<gtk::Widget> {
    if matches(widget) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_widget(&c, matches) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// The vault's font, base size and text width reach every note.
#[gtk::test]
async fn text_style_applies_to_notes() {
    let dir = vault(Some(100));
    let window = open(&dir);
    window.set_default_size(1280, 800);
    window.open_path(&p("Home.md"), false);
    let view = window.selected_note().unwrap().view();
    window.set_text_style(igneous::TextStyle {
        family: Some("Serif".to_owned()),
        size: Some(20.0),
        monospace: None,
        line_width: Some(500),
    });
    // GTK hands Pango the size in pixels (at 96 dpi).
    let points = |d: &gtk::pango::FontDescription| {
        let size = f64::from(d.size()) / f64::from(gtk::pango::SCALE);
        if d.is_size_absolute() {
            size * 72.0 / 96.0
        } else {
            size
        }
    };
    assert!(
        until(3000, || {
            view.pango_context()
                .font_description()
                .is_some_and(|d| (points(&d) - 20.0).abs() < 0.1)
        })
        .await,
        "the base size wasn't applied"
    );
    let family = view
        .pango_context()
        .font_description()
        .and_then(|d| d.family())
        .unwrap();
    assert_eq!(family.as_str(), "Serif");
    // Properties keep the interface font.
    let properties = find_widget(view.upcast_ref(), &|w| w.has_css_class("properties"))
        .expect("Home.md shows its properties");
    let interface = gtk::Settings::default()
        .and_then(|s| s.gtk_font_name())
        .map(|n| gtk::pango::FontDescription::from_string(&n))
        .and_then(|d| d.family())
        .unwrap();
    let properties_family = properties
        .pango_context()
        .font_description()
        .and_then(|d| d.family())
        .unwrap();
    assert_eq!(properties_family, interface);
    assert_ne!(properties_family.as_str(), "Serif");
    // Readable line length keeps the text 500 pixels wide.
    assert!(
        until(3000, || (view.width()
            - view.left_margin()
            - view.right_margin()
            - 500)
            .abs()
            <= 1)
        .await,
        "the text width wasn't applied"
    );
    window.close();
}

/// Presses Enter in a note's editor, through its key handler.
fn press_enter(view: &igneous_editor::NoteView) -> bool {
    use gtk::glib::translate::IntoGlib;
    // Igneous's own handlers, as GTK would offer it to them (the first is
    // the one for widgets inside the note).
    view.observe_controllers()
        .into_iter()
        .filter_map(|c| c.ok().and_downcast::<gtk::EventControllerKey>())
        .filter(|k| {
            matches!(
                k.name().as_deref(),
                Some("igneous-child-keys" | "igneous-input")
            )
        })
        .any(|keys| {
            keys.emit_by_name::<bool>(
                "key-pressed",
                &[
                    &gtk::gdk::Key::Return.into_glib(),
                    &36u32,
                    &gtk::gdk::ModifierType::empty(),
                ],
            )
        })
}

/// `---` and Enter at the top of a note starts its properties, as in
/// Obsidian, instead of drawing a horizontal rule.
#[gtk::test]
async fn three_dashes_start_properties() {
    let dir = vault(Some(100));
    std::fs::write(dir.path().join("Blank.md"), "").unwrap();
    let window = open(&dir);
    window.open_path(&p("Blank.md"), false);
    let note = window.selected_note().unwrap();
    let view = note.view();
    // Typing comes after opening has settled (opening puts the focus in
    // the text once the tab has switched).
    wait(100).await;
    // An empty note offers Add Property before it has any properties.
    assert!(view.overlay_kinds().contains(&"properties".to_owned()));
    view.buffer().insert_at_cursor("---");
    assert!(press_enter(&view), "Enter was left to the text view");
    assert_eq!(note.text(), "---\n---\n");
    // The properties show, with Add Property's menu open.
    assert!(
        until(3000, || find_widget(view.upcast_ref(), &|w| {
            w.downcast_ref::<gtk::PopoverMenu>()
                .is_some_and(|p| p.is_visible())
        })
        .is_some())
        .await,
        "Add Property's menu didn't open"
    );
    assert!(view.overlay_kinds().contains(&"properties".to_owned()));
    // One undo brings the dashes back.
    view.buffer().undo();
    assert_eq!(note.text(), "---");

    // Not in Source mode, and not below the first line.
    note.view().set_mode(igneous_editor::Mode::Source);
    let buffer = view.buffer();
    buffer.place_cursor(&buffer.end_iter());
    assert!(!press_enter(&view));
    note.view().set_mode(igneous_editor::Mode::Live);
    buffer.set_text("Text\n---");
    buffer.place_cursor(&buffer.end_iter());
    assert!(!view.start_properties_for_test());
    note.discard();
    window.close();
}

/// The Add Property row in a note, if it's showing.
/// A menu's labels.
fn labels(menu: &gtk::gio::MenuModel) -> Vec<String> {
    (0..menu.n_items())
        .filter_map(|i| {
            menu.item_attribute_value(i, "label", Some(gtk::glib::VariantTy::STRING))
                .and_then(|v| v.get::<String>())
        })
        .collect()
}

fn add_property_row(view: &igneous_editor::NoteView) -> Option<adw::ActionRow> {
    find_widget(view.upcast_ref(), &|w| {
        w.downcast_ref::<adw::ActionRow>()
            .is_some_and(|row| row.title() == "Add Property")
    })
    .and_downcast()
}

/// Add Property, clicked: a menu of the vault's property names, then New
/// Property… for a name typed in. Real clicks and keys go to the field,
/// not the note around it.
#[gtk::test]
async fn add_property_offers_the_vaults_names_and_new_ones() {
    let dir = vault(Some(100));
    std::fs::write(dir.path().join("Plain.md"), "Some text.\n").unwrap();
    let window = open(&dir);
    window.maximize();
    window.open_path(&p("Plain.md"), false);
    let note = window.selected_note().unwrap();
    let view = note.view();
    assert!(
        until(3000, || add_property_row(&view)
            .is_some_and(|r| r.is_mapped()))
        .await
    );
    // The menu has the names the index finds; opening writes nothing.
    let names_file = dir.path().join(".igneous/properties.json");
    let saved = || std::fs::read_to_string(&names_file).unwrap_or_default();
    let menu_names = || labels(&window.property_names().menu());
    assert!(
        until(5000, || menu_names().len() == 2).await,
        "{:?}",
        menu_names()
    );
    assert!(!names_file.exists());
    let input = RemoteInput::new().await;

    // Clicking opens the names the vault uses (Home.md has these).
    input
        .click(add_property_row(&view).unwrap().upcast_ref(), None)
        .await;
    let open_menu = || {
        find_widget(view.upcast_ref(), &|w| {
            w.downcast_ref::<gtk::PopoverMenu>()
                .is_some_and(|p| p.is_visible())
        })
        .and_downcast::<gtk::PopoverMenu>()
    };
    assert!(
        until(2000, || open_menu().is_some()).await,
        "Add Property's menu"
    );
    let menu = open_menu().unwrap();
    let model = menu.menu_model().unwrap();
    let names: Vec<String> = (0..model.n_items())
        .filter_map(|section| model.item_link(section, "section"))
        .flat_map(|items| {
            (0..items.n_items())
                .filter_map(|i| {
                    items
                        .item_attribute_value(i, "label", Some(gtk::glib::VariantTy::STRING))
                        .and_then(|v| v.get::<String>())
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(names.contains(&"tags".to_owned()), "{names:?}");
    assert!(
        names.ends_with(&[
            "_New Property…".to_owned(),
            "_Refresh Property Names".to_owned()
        ]),
        "{names:?}"
    );
    menu.popdown();

    // New Property… asks for a name; typing goes there, and Enter adds it.
    let row = add_property_row(&view).unwrap();
    WidgetExt::activate_action(&row, "property.new", None).unwrap();
    wait(200).await;
    input.type_text("status").await;
    assert_eq!(note.text(), "Some text.\n", "the typing reached the note");
    input.enter().await;
    assert!(
        until(2000, || note.text().starts_with("---\nstatus:")).await,
        "{:?}",
        note.text()
    );
    assert!(note.text().ends_with("---\nSome text.\n"));
    // A new name joins the menu, and every name is saved with the vault.
    assert_eq!(menu_names().last().map(String::as_str), Some("status"));
    let file: serde_json::Value = serde_json::from_str(&saved()).unwrap();
    let mut names: Vec<&str> = file["names"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["created", "status", "tags"]);

    // An existing name is added straight from the menu, and from then on
    // the note's menu leaves out the names it has.
    let action = |key: &str| {
        igneous_editor::property_name_item(key)
            .attribute_value("action", Some(gtk::glib::VariantTy::STRING))
            .and_then(|a| a.get::<String>())
            .unwrap()
    };
    let old = add_property_row(&view).unwrap();
    WidgetExt::activate_action(&old, &action("tags"), None).unwrap();
    assert!(note.text().contains("\ntags:"), "{:?}", note.text());
    // The properties are rebuilt for the new frontmatter.
    assert!(
        until(2000, || add_property_row(&view)
            .is_some_and(|row| row != old && row.is_mapped()))
        .await
    );
    let row = add_property_row(&view).unwrap();
    assert!(WidgetExt::activate_action(&row, &action("tags"), None).is_err());
    assert!(WidgetExt::activate_action(&row, &action("status"), None).is_err());
    WidgetExt::activate_action(&row, &action("created"), None).unwrap();
    assert!(note.text().contains("\ncreated:"), "{:?}", note.text());
    note.discard();
    window.close();
}

/// Refresh Property Names rescans the vault and remakes the list from the
/// names its notes use now, dropping old ones.
#[gtk::test]
async fn refreshing_property_names_rescans_the_vault() {
    let dir = vault(Some(100));
    std::fs::write(dir.path().join("Plain.md"), "Some text.\n").unwrap();
    let names_file = dir.path().join(".igneous/properties.json");
    std::fs::write(&names_file, r#"{"version":1,"names":["old","tags"]}"#).unwrap();
    let window = open(&dir);
    window.open_path(&p("Plain.md"), false);
    let view = window.selected_note().unwrap().view();
    let menu_names = || labels(&window.property_names().menu());
    // The saved names come first, then what the index finds.
    assert!(
        until(5000, || menu_names() == ["old", "tags", "created"]).await,
        "{:?}",
        menu_names()
    );
    assert!(until(3000, || add_property_row(&view).is_some()).await);

    let row = add_property_row(&view).unwrap();
    WidgetExt::activate_action(&row, "property.refresh", None).unwrap();
    assert!(
        until(5000, || menu_names() == ["created", "tags"]).await,
        "{:?}",
        menu_names()
    );
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&names_file).unwrap()).unwrap();
    assert_eq!(file["names"], serde_json::json!(["created", "tags"]));
    window.selected_note().unwrap().discard();
    window.close();
}
