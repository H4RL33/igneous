//! Watching a vault for changes made by other programs.
//!
//! Events are debounced, filtered through the vault's ignore rules, and
//! stripped of changes Igneous made itself: after each save the caller records
//! the written contents with [`VaultWatcher::expect_write`], and any later
//! event for that file whose contents still match is dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_debouncer_full::notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::notify::{self, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::fs::FileStamp;
use crate::ignore::IgnoreRules;
use crate::path::VaultPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEvent {
    Created(VaultPath),
    Modified(VaultPath),
    Removed(VaultPath),
    Renamed { from: VaultPath, to: VaultPath },
}

type Expected = Arc<Mutex<HashMap<PathBuf, blake3::Hash>>>;

pub struct VaultWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    expected: Expected,
}

impl VaultWatcher {
    /// Starts watching `root` recursively. `handler` runs on a background
    /// thread with each non-empty batch of events.
    pub fn start(
        root: &Path,
        ignore: IgnoreRules,
        debounce: Duration,
        mut handler: impl FnMut(Vec<VaultEvent>) + Send + 'static,
    ) -> notify::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let expected: Expected = Arc::default();
        let mapper = EventMapper {
            root: root.clone(),
            ignore,
            expected: expected.clone(),
        };
        let mut debouncer =
            new_debouncer(
                debounce,
                None,
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        let events = mapper.map(events.iter().map(|e| &e.event));
                        if !events.is_empty() {
                            handler(events);
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            tracing::warn!(%error, "file watcher error");
                        }
                    }
                },
            )?;
        debouncer.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self {
            _debouncer: debouncer,
            expected,
        })
    }

    /// Records that Igneous itself just wrote `stamp` to `path`.
    pub fn expect_write(&self, path: &Path, stamp: &FileStamp) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.expected.lock().unwrap().insert(path, stamp.hash);
    }
}

struct EventMapper {
    root: PathBuf,
    ignore: IgnoreRules,
    expected: Expected,
}

impl EventMapper {
    fn map<'a>(&self, events: impl Iterator<Item = &'a notify::Event>) -> Vec<VaultEvent> {
        let mut out: Vec<VaultEvent> = Vec::new();
        for event in events {
            let mapped = match event.kind {
                EventKind::Create(_) => self.single(event, VaultEvent::Created),
                EventKind::Remove(_) => self.single(event, VaultEvent::Removed),
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                    match (self.visible(&event.paths[0]), self.visible(&event.paths[1])) {
                        (Some(from), Some(to)) => Some(VaultEvent::Renamed { from, to }),
                        // An atomic save by another program: temp file renamed over the target.
                        (None, Some(to)) => self.changed(&event.paths[1], to),
                        (Some(from), None) => Some(VaultEvent::Removed(from)),
                        (None, None) => None,
                    }
                }
                EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                    self.single(event, VaultEvent::Removed)
                }
                EventKind::Modify(ModifyKind::Name(_)) => self.single(event, VaultEvent::Created),
                EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) => None,
                EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                    let path = event.paths.first();
                    path.and_then(|p| self.visible(p).and_then(|vp| self.changed(p, vp)))
                }
            };
            if let Some(mapped) = mapped
                && out.last() != Some(&mapped)
            {
                out.push(mapped);
            }
        }
        out
    }

    fn single(
        &self,
        event: &notify::Event,
        make: fn(VaultPath) -> VaultEvent,
    ) -> Option<VaultEvent> {
        let path = event.paths.first()?;
        let vp = self.visible(path)?;
        (!self.is_own_write(path)).then(|| make(vp))
    }

    /// A modification, unless the contents are what Igneous last wrote.
    fn changed(&self, path: &Path, vp: VaultPath) -> Option<VaultEvent> {
        (!self.is_own_write(path)).then_some(VaultEvent::Modified(vp))
    }

    fn is_own_write(&self, path: &Path) -> bool {
        let Some(expected) = self.expected.lock().unwrap().get(path).copied() else {
            return false;
        };
        std::fs::read(path).is_ok_and(|bytes| blake3::hash(&bytes) == expected)
    }

    fn visible(&self, path: &Path) -> Option<VaultPath> {
        let vp = VaultPath::from_fs(&self.root, path).ok()?;
        (!self.ignore.is_ignored(&vp)).then_some(vp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{Expect, write_atomic};
    use crate::text::TextFile;
    use std::sync::mpsc;

    const DEBOUNCE: Duration = Duration::from_millis(100);
    const WAIT: Duration = Duration::from_millis(1500);

    fn collect(rx: &mpsc::Receiver<Vec<VaultEvent>>) -> Vec<VaultEvent> {
        let mut all = Vec::new();
        while let Ok(batch) = rx.recv_timeout(WAIT) {
            all.extend(batch);
        }
        all
    }

    fn start(root: &Path) -> (VaultWatcher, mpsc::Receiver<Vec<VaultEvent>>) {
        let (tx, rx) = mpsc::channel();
        let watcher = VaultWatcher::start(root, IgnoreRules::default(), DEBOUNCE, move |events| {
            let _ = tx.send(events);
        })
        .unwrap();
        (watcher, rx)
    }

    #[test]
    fn reports_external_changes_but_not_our_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let note = root.join("note.md");
        std::fs::write(&note, "start\n").unwrap();
        let (watcher, rx) = start(&root);

        // Our own save is suppressed.
        let stamp = write_atomic(&note, &TextFile::new("ours\n"), Expect::Anything).unwrap();
        watcher.expect_write(&note, &stamp);
        let events = collect(&rx);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, VaultEvent::Modified(p) if p.as_str() == "note.md")),
            "own write leaked: {events:?}"
        );

        // Someone else's edit is reported.
        std::fs::write(&note, "theirs\n").unwrap();
        let events = collect(&rx);
        assert!(
            events.contains(&VaultEvent::Modified(VaultPath::new("note.md").unwrap())),
            "{events:?}"
        );
    }

    #[test]
    fn ignores_hidden_paths_and_reports_renames() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        let (_watcher, rx) = start(&root);

        std::fs::create_dir(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/workspace.json"), "{}").unwrap();
        std::fs::rename(root.join("a.md"), root.join("b.md")).unwrap();

        let events = collect(&rx);
        assert!(
            events
                .iter()
                .all(|e| !format!("{e:?}").contains(".obsidian")),
            "{events:?}"
        );
        assert!(
            events.contains(&VaultEvent::Renamed {
                from: VaultPath::new("a.md").unwrap(),
                to: VaultPath::new("b.md").unwrap(),
            }),
            "{events:?}"
        );
    }
}
