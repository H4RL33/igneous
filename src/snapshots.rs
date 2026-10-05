//! File recovery: copies of notes taken as they're saved (at most every few
//! minutes per note), kept outside the vault in
//! `$XDG_DATA_HOME/igneous/snapshots/<vault-key>/` and pruned by age and
//! total size. Each note has a folder named by a hash of its path, holding a
//! `path` file and one `<unix-ms>.md` per snapshot.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use igneous_core::VaultPath;
use igneous_core::settings::RecoverySettings;

use crate::text_page::{Contents, TextPage};
use crate::vault::VaultContext;
use crate::window::Window;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub file: PathBuf,
    /// Unix milliseconds.
    pub taken: i64,
    pub size: u64,
}

pub fn vault_dir(ctx: &VaultContext) -> PathBuf {
    glib::user_data_dir()
        .join("igneous")
        .join("snapshots")
        .join(ctx.vault.key().as_str())
}

fn note_dir(vault_dir: &Path, path: &VaultPath) -> PathBuf {
    let hash = glib::compute_checksum_for_string(glib::ChecksumType::Sha256, path.as_str())
        .map(|h| h.to_string())
        .unwrap_or_default();
    vault_dir.join(&hash[..hash.len().min(32)])
}

fn now_ms() -> i64 {
    glib::DateTime::now_utc().map_or(0, |t| {
        t.to_unix() * 1000 + i64::from(t.microsecond()) / 1000
    })
}

/// A note's snapshots, newest first.
pub fn list_in(vault_dir: &Path, path: &VaultPath) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(note_dir(vault_dir, path)) else {
        return Vec::new();
    };
    let mut found: Vec<Snapshot> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let taken = name.strip_suffix(".md")?.parse().ok()?;
            Some(Snapshot {
                file: entry.path(),
                taken,
                size: entry.metadata().ok()?.len(),
            })
        })
        .collect();
    found.sort_by_key(|s| std::cmp::Reverse(s.taken));
    found
}

/// Records `text` as a snapshot of `path`, unless one was taken within the
/// interval or the newest already holds the same text. Returns whether a
/// snapshot was written.
pub fn record_in(
    vault_dir: &Path,
    path: &VaultPath,
    text: &str,
    settings: &RecoverySettings,
    now: i64,
) -> bool {
    let existing = list_in(vault_dir, path);
    if let Some(newest) = existing.first() {
        let interval = i64::from(settings.interval_minutes) * 60_000;
        if now - newest.taken < interval {
            return false;
        }
        if std::fs::read_to_string(&newest.file).is_ok_and(|t| t == text) {
            return false;
        }
    }
    let dir = note_dir(vault_dir, path);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let _ = std::fs::write(dir.join("path"), path.as_str());
    let written = std::fs::write(dir.join(format!("{now}.md")), text).is_ok();
    prune(vault_dir, settings, now);
    written
}

/// Deletes snapshots older than the age limit, then the oldest until the
/// vault's snapshots fit the size limit.
pub fn prune(vault_dir: &Path, settings: &RecoverySettings, now: i64) {
    let Ok(notes) = std::fs::read_dir(vault_dir) else {
        return;
    };
    let max_age = i64::from(settings.keep_days) * 86_400_000;
    let mut all: Vec<Snapshot> = Vec::new();
    for note in notes.flatten().filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(note.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(taken) = name.strip_suffix(".md").and_then(|n| n.parse::<i64>().ok()) else {
                continue;
            };
            if now - taken > max_age {
                let _ = std::fs::remove_file(file.path());
                continue;
            }
            all.push(Snapshot {
                file: file.path(),
                taken,
                size: file.metadata().map_or(0, |m| m.len()),
            });
        }
    }
    let limit = u64::from(settings.max_megabytes) * 1024 * 1024;
    let mut total: u64 = all.iter().map(|s| s.size).sum();
    all.sort_by_key(|s| s.taken);
    for snapshot in all {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&snapshot.file).is_ok() {
            total -= snapshot.size;
        }
    }
    // Folders left with only their `path` file go too.
    if let Ok(notes) = std::fs::read_dir(vault_dir) {
        for note in notes.flatten() {
            let empty = std::fs::read_dir(note.path())
                .map(|files| files.flatten().all(|f| f.file_name() == "path"))
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir_all(note.path());
            }
        }
    }
}

/// Takes a snapshot of a note that was just saved.
pub fn record(ctx: &VaultContext, path: &VaultPath, text: &str) {
    let settings = ctx.settings.borrow().recovery.clone();
    record_in(&vault_dir(ctx), path, text, &settings, now_ms());
}

fn size_text(bytes: u64) -> String {
    glib::format_size(bytes).to_string()
}

impl Window {
    /// The open note's snapshots, newest first.
    pub fn snapshots(&self, path: &VaultPath) -> Vec<Snapshot> {
        list_in(&vault_dir(self.ctx()), path)
    }

    /// Lists a note's snapshots; choosing one opens it read-only.
    pub fn show_snapshots(&self, path: &VaultPath) {
        let snapshots = self.snapshots(path);
        let name = crate::files::display_name(path, false).0;
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        let dialog = adw::Dialog::builder()
            .title(format!("Snapshots of {name}"))
            .content_width(420)
            .content_height(480)
            .build();
        for snapshot in &snapshots {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&when(snapshot.taken)))
                .subtitle(size_text(snapshot.size))
                .activatable(true)
                .build();
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let window = self.downgrade();
            let dialog_weak = dialog.downgrade();
            let snapshot = snapshot.clone();
            let path = path.clone();
            row.connect_activated(move |_| {
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.close();
                }
                if let Some(window) = window.upgrade() {
                    window.open_snapshot(&path, &snapshot);
                }
            });
            list.append(&row);
        }
        let content: gtk::Widget = if snapshots.is_empty() {
            adw::StatusPage::builder()
                .icon_name("document-revert-symbolic")
                .title("No Snapshots")
                .description("Snapshots are taken as the note is edited, every few minutes")
                .build()
                .upcast()
        } else {
            gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(
                    &adw::Clamp::builder()
                        .child(&list)
                        .margin_start(12)
                        .margin_end(12)
                        .margin_top(6)
                        .margin_bottom(12)
                        .build(),
                )
                .build()
                .upcast()
        };
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(self));
    }

    pub fn open_snapshot(&self, path: &VaultPath, snapshot: &Snapshot) {
        match std::fs::read_to_string(&snapshot.file) {
            Ok(text) => self.open_text_page(
                path,
                Contents::Snapshot {
                    taken: snapshot.taken,
                    when: when(snapshot.taken),
                },
                &text,
            ),
            Err(e) => self.toast(&format!("Couldn’t read the snapshot: {e}")),
        }
    }

    /// Puts the snapshot in the selected tab back into its note, as one
    /// undoable edit.
    pub fn restore_selected_snapshot(&self) {
        let Some(page) = self
            .imp()
            .tab_view
            .selected_page()
            .and_then(|p| p.child().downcast::<TextPage>().ok())
        else {
            return;
        };
        let (Some(path), Some(Contents::Snapshot { .. })) = (page.path(), page.contents()) else {
            return;
        };
        let text = page.text();
        if !self.ctx().abs(&path).is_file() {
            self.toast(&format!("“{path}” no longer exists"));
            return;
        }
        self.open_path(&path, false);
        let Some(note) = self
            .selected_note()
            .filter(|n| n.path().as_ref() == Some(&path))
        else {
            return;
        };
        let old = note.text();
        note.replace_text(&old, &text);
        note.flush();
        self.toast("Restored the snapshot; undo to go back");
    }
}

/// When a snapshot was taken, for people.
fn when(taken: i64) -> String {
    glib::DateTime::from_unix_local(taken / 1000)
        .and_then(|t| t.format("%-d %B %Y, %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(interval: u32, days: u32, mb: u32) -> RecoverySettings {
        RecoverySettings {
            interval_minutes: interval,
            keep_days: days,
            max_megabytes: mb,
            ..RecoverySettings::default()
        }
    }

    #[test]
    fn records_lists_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let note = VaultPath::new("Home.md").unwrap();
        let s = settings(5, 7, 100);
        let t0 = 1_700_000_000_000;
        assert!(record_in(dir.path(), &note, "one", &s, t0));
        // Too soon, then the same text: nothing new.
        assert!(!record_in(dir.path(), &note, "two", &s, t0 + 60_000));
        assert!(!record_in(dir.path(), &note, "one", &s, t0 + 600_000));
        assert!(record_in(dir.path(), &note, "two", &s, t0 + 600_000));
        let found = list_in(dir.path(), &note);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].taken, t0 + 600_000);
        // Eight days on, both are too old.
        prune(dir.path(), &s, t0 + 8 * 86_400_000);
        assert!(list_in(dir.path(), &note).is_empty());
    }

    #[test]
    fn size_limit_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let note = VaultPath::new("Big.md").unwrap();
        let big = "x".repeat(600 * 1024);
        let s = settings(0, 7, 1);
        let t0 = 1_700_000_000_000;
        record_in(dir.path(), &note, &format!("{big}1"), &s, t0);
        record_in(dir.path(), &note, &format!("{big}2"), &s, t0 + 1);
        let found = list_in(dir.path(), &note);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].taken, t0 + 1);
    }
}
