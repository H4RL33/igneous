//! Everything a window knows about its vault.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use igneous_core::fs::FileStamp;
use igneous_core::ignore::IgnoreRules;
use igneous_core::settings::{self, VaultSettings};
use igneous_core::vault::EntryKind;
use igneous_core::watch::{VaultEvent, VaultWatcher};
use igneous_core::{Vault, VaultPath};

pub struct VaultContext {
    pub vault: Vault,
    pub settings: VaultSettings,
    /// Set when `.igneous/vault.json` couldn't be read; defaults are in use and
    /// the file is never overwritten.
    pub settings_error: Option<String>,
    watcher: RefCell<Option<VaultWatcher>>,
    /// Every file in the vault (not folders), sorted.
    files: RefCell<Vec<VaultPath>>,
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
    }
}
