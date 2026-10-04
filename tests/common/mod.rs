//! Helpers shared by the UI test files. Every test file runs in the sealed
//! headless session (build-aux/run-ui-tests.sh).

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use igneous::Window;
use igneous_core::VaultPath;

pub const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vaults/basic");

pub fn copy_dir(from: &Path, to: &Path) {
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
pub fn vault(autosave_ms: Option<u32>) -> tempfile::TempDir {
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
pub fn open(dir: &tempfile::TempDir) -> Window {
    let window = Window::for_vault(dir.path()).unwrap();
    window.present();
    window
}

pub fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

pub async fn wait(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

pub fn git(cwd: &Path, args: &[&str]) -> String {
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
pub fn synced_vault() -> (tempfile::TempDir, tempfile::TempDir) {
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
pub fn other_clone(remotes: &tempfile::TempDir) -> std::path::PathBuf {
    let path = remotes.path().join("other");
    if !path.exists() {
        git(remotes.path(), &["clone", "-q", "remote.git", "other"]);
    }
    path
}

pub fn remote_log(remotes: &tempfile::TempDir) -> String {
    git(
        &remotes.path().join("remote.git"),
        &["log", "--format=%s", "main"],
    )
}

/// Waits up to `ms` for `f` to hold.
pub async fn until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..ms / 50 {
        if f() {
            return true;
        }
        wait(50).await;
    }
    f()
}

pub fn save_png(window: &gtk::Window, out: &str) {
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
