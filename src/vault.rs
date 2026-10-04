//! Everything a window knows about its vault.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use igneous_core::fs::FileStamp;
use igneous_core::ignore::IgnoreRules;
use igneous_core::settings::{self, VaultSettings};
use igneous_core::vault::EntryKind;
use igneous_core::watch::{VaultEvent, VaultWatcher};
use igneous_core::{Vault, VaultPath};
use igneous_markdown::LinkRef;

pub struct VaultContext {
    pub vault: Vault,
    pub settings: VaultSettings,
    /// Set when `.igneous/vault.json` couldn't be read; defaults are in use and
    /// the file is never overwritten.
    pub settings_error: Option<String>,
    watcher: RefCell<Option<VaultWatcher>>,
    /// Every file in the vault (not folders), sorted.
    files: RefCell<Vec<VaultPath>>,
    /// Files by loose name, and notes also by loose stem, for resolving links.
    names: RefCell<HashMap<String, Vec<VaultPath>>>,
}

impl VaultContext {
    pub fn open(root: &Path) -> Result<Self, String> {
        let mut vault = Vault::open(root).map_err(|e| e.to_string())?;
        let (settings, settings_error) = match settings::load::<VaultSettings>(&vault.igneous_dir())
        {
            Ok(settings) => (settings, None),
            Err(e) => (VaultSettings::default(), Some(e.to_string())),
        };
        match IgnoreRules::new(&settings.files.excluded) {
            Ok(rules) => vault.set_ignore_rules(rules),
            Err(e) => tracing::warn!(%e, "invalid exclusion pattern"),
        }
        Ok(Self {
            vault,
            settings,
            settings_error,
            watcher: RefCell::default(),
            files: RefCell::default(),
            names: RefCell::default(),
        })
    }

    pub fn root(&self) -> &Path {
        self.vault.root()
    }

    pub fn abs(&self, path: &VaultPath) -> PathBuf {
        self.vault.abs(path)
    }

    pub fn name(&self) -> String {
        self.vault.name()
    }

    /// Starts watching the vault. `handler` runs on the main thread.
    pub fn watch(&self, handler: impl Fn(Vec<VaultEvent>) + 'static) {
        let (sender, receiver) = async_channel::unbounded();
        let watcher = VaultWatcher::start(
            self.vault.root(),
            self.vault.ignore_rules().clone(),
            Duration::from_millis(250),
            move |events| {
                let _ = sender.send_blocking(events);
            },
        );
        match watcher {
            Ok(watcher) => {
                self.watcher.replace(Some(watcher));
                gtk::glib::spawn_future_local(async move {
                    while let Ok(events) = receiver.recv().await {
                        handler(events);
                    }
                });
            }
            Err(e) => tracing::warn!(%e, "can't watch the vault for changes"),
        }
    }

    /// Records a save so the watcher doesn't report it back as a change.
    pub fn expect_write(&self, path: &VaultPath, stamp: &FileStamp) {
        if let Some(watcher) = self.watcher.borrow().as_ref() {
            watcher.expect_write(&self.abs(path), stamp);
        }
    }

    pub fn set_files(&self, files: Vec<VaultPath>) {
        self.files.replace(files);
        self.index_names();
    }

    fn index_names(&self) {
        let mut names: HashMap<String, Vec<VaultPath>> = HashMap::new();
        for file in self.files.borrow().iter() {
            names
                .entry(igneous_core::path::loose_key(file.file_name()))
                .or_default()
                .push(file.clone());
            if file.extension() == Some("md") {
                names
                    .entry(igneous_core::path::loose_key(file.stem()))
                    .or_default()
                    .push(file.clone());
            }
        }
        self.names.replace(names);
    }

    /// The file a link points to, following Obsidian's order: exact path,
    /// path relative to the linking note, then a unique name; ties go to the
    /// same folder, then the shortest path.
    pub fn resolve(&self, link: &LinkRef, from: Option<&VaultPath>) -> Option<VaultPath> {
        let target = link.target.trim().trim_start_matches('/');
        if target.is_empty() {
            return from.cloned();
        }
        let files = self.files.borrow();
        let exists = |candidate: &str| -> Option<VaultPath> {
            let key = igneous_core::path::loose_key(candidate);
            files
                .iter()
                .find(|f| f.as_str() == candidate)
                .or_else(|| files.iter().find(|f| f.loose_key() == key))
                .cloned()
        };
        let has_extension = Path::new(target).extension().is_some_and(|e| {
            let e = e.to_string_lossy();
            !e.is_empty() && e.len() <= 5 && !e.contains(' ')
        });
        let forms: Vec<String> = if has_extension {
            vec![target.to_owned(), format!("{target}.md")]
        } else {
            vec![format!("{target}.md"), target.to_owned()]
        };
        for form in &forms {
            if let Some(found) = exists(form) {
                return Some(found);
            }
        }
        if let Some(dir) = from.and_then(VaultPath::parent) {
            for form in &forms {
                if let Some(joined) = normalise(&format!("{dir}/{form}"))
                    && let Some(found) = exists(&joined)
                {
                    return Some(found);
                }
            }
        }
        let name = target.rsplit('/').next().unwrap_or(target);
        let names = self.names.borrow();
        let mut candidates: Vec<&VaultPath> = names
            .get(&igneous_core::path::loose_key(name))
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        // `folder/Note` also matches `deeper/folder/Note.md`.
        if target.contains('/') {
            let suffix = igneous_core::path::loose_key(&forms[0]);
            candidates.retain(|c| c.loose_key().ends_with(&suffix));
        }
        let folder = from.and_then(VaultPath::parent);
        candidates.sort_by(|a, b| {
            (a.parent() != folder)
                .cmp(&(b.parent() != folder))
                .then(a.as_str().len().cmp(&b.as_str().len()))
                .then_with(|| a.as_str().cmp(b.as_str()))
        });
        candidates.first().map(|c| (*c).clone())
    }

    pub fn files(&self) -> Vec<VaultPath> {
        self.files.borrow().clone()
    }

    /// Lists every file in the vault. Blocking; run it off the main thread.
    pub fn scan(vault: &Vault) -> Vec<VaultPath> {
        vault
            .walk()
            .filter_map(Result::ok)
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| e.path)
            .collect()
    }

    /// Keeps the file list in step with changes on disk.
    pub fn apply_events(&self, events: &[VaultEvent]) {
        let mut files = self.files.borrow_mut();
        for event in events {
            match event {
                VaultEvent::Created(path) | VaultEvent::Modified(path) => {
                    if self.abs(path).is_file() && !files.contains(path) {
                        files.push(path.clone());
                    }
                }
                VaultEvent::Removed(path) => files.retain(|f| !f.starts_with(path)),
                VaultEvent::Renamed { from, to } => {
                    files.retain(|f| !f.starts_with(from));
                    if self.abs(to).is_file() {
                        files.push(to.clone());
                    } else {
                        let vault = &self.vault;
                        files.extend(Self::scan(vault).into_iter().filter(|f| f.starts_with(to)));
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        drop(files);
        self.index_names();
    }
}

/// Resolves `.` and `..` in a `/`-separated path; `None` if it climbs out of
/// the vault.
fn normalise(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}
