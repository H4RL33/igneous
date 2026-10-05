//! The vault's index (igneous-index) on a worker thread: built or caught up
//! when the window opens, kept current from the file watcher, and queried
//! asynchronously. The window also keeps a few small lists from it (notes
//! with their aliases, tags) for completion, which must answer immediately.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use igneous_core::watch::VaultEvent;
use igneous_core::{Vault, VaultPath};
use std::collections::BTreeMap;

use igneous_core::settings::PropertyType;
use igneous_index::{Index, NoteEntry, PropertyInfo};

use crate::worker::Worker;

pub struct IndexService {
    worker: Worker<Option<Index>>,
    vault: Vault,
    ready: Cell<bool>,
    notes: RefCell<Rc<Vec<NoteEntry>>>,
    tags: RefCell<Rc<Vec<(String, usize)>>>,
    data: RefCell<Option<std::sync::Arc<Vec<igneous_query::NoteData>>>>,
    /// Property types the user chose, from `.igneous/vault.json`.
    overrides: RefCell<BTreeMap<String, PropertyType>>,
    properties: RefCell<Rc<Vec<PropertyInfo>>>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

impl IndexService {
    /// Opens the index in `$XDG_CACHE_HOME/igneous/<vault key>/` and catches
    /// it up with the disk in the background.
    pub fn new(vault: &Vault) -> Rc<Self> {
        let dir = glib::user_cache_dir()
            .join("igneous")
            .join(vault.key().as_str());
        let worker = Worker::new("igneous-index", move || match Index::open(&dir) {
            Ok(index) => Some(index),
            Err(e) => {
                tracing::warn!(%e, "can't open the index; keeping it in memory");
                Index::in_memory().ok()
            }
        });
        let service = Rc::new(Self {
            worker,
            vault: vault.clone(),
            ready: Cell::default(),
            notes: RefCell::default(),
            tags: RefCell::default(),
            data: RefCell::default(),
            overrides: RefCell::default(),
            properties: RefCell::default(),
            listeners: RefCell::default(),
        });
        let weak = Rc::downgrade(&service);
        let vault = vault.clone();
        glib::spawn_future_local(async move {
            let Some(this) = weak.upgrade() else { return };
            let result = this
                .worker
                .call(move |index| index.as_mut().map(|i| i.reconcile(&vault)))
                .await;
            match result.flatten() {
                Some(Ok(stats)) => tracing::debug!(?stats, "index caught up"),
                Some(Err(e)) => tracing::warn!(%e, "indexing failed"),
                None => {}
            }
            this.ready.set(true);
            this.changed().await;
        });
        service
    }

    pub fn is_ready(&self) -> bool {
        self.ready.get()
    }

    /// Called after the index changes.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    pub(crate) fn cached_data(&self) -> Option<std::sync::Arc<Vec<igneous_query::NoteData>>> {
        self.data.borrow().clone()
    }

    pub(crate) fn cache_data(&self, data: std::sync::Arc<Vec<igneous_query::NoteData>>) {
        self.data.replace(Some(data));
    }

    async fn changed(&self) {
        self.data.replace(None);
        let overrides = self.overrides.borrow().clone();
        let lists = self
            .query(move |index| {
                (
                    index.notes(),
                    index.tags(),
                    index.property_catalog(&overrides),
                )
            })
            .await;
        if let Some((notes, tags, properties)) = lists {
            self.notes.replace(Rc::new(notes.unwrap_or_default()));
            self.tags.replace(Rc::new(tags.unwrap_or_default()));
            let mut properties = properties.unwrap_or_default();
            properties.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
            self.properties.replace(Rc::new(properties));
        }
        for listener in self.listeners.borrow().iter() {
            listener();
        }
    }

    /// Re-reads the whole vault, then tells the listeners.
    pub async fn rescan(&self) {
        let vault = self.vault.clone();
        let result = self
            .worker
            .call(move |index| index.as_mut().map(|i| i.rescan(&vault)))
            .await;
        match result.flatten() {
            Some(Ok(stats)) => tracing::debug!(?stats, "index rescanned"),
            Some(Err(e)) => tracing::warn!(%e, "rescanning failed"),
            None => {}
        }
        self.changed().await;
    }

    /// Brings the index up to date with changes on disk.
    pub fn apply(self: &Rc<Self>, events: Vec<VaultEvent>) {
        let this = self.clone();
        let vault = self.vault.clone();
        glib::spawn_future_local(async move {
            let result = this
                .worker
                .call(move |index| index.as_mut().map(|i| i.apply(&vault, &events)))
                .await;
            if let Some(Some(Err(e))) = result {
                tracing::warn!(%e, "couldn't update the index");
            }
            this.changed().await;
        });
    }

    /// Runs `f` against the index on its thread. `None` if the index
    /// couldn't be opened at all.
    pub async fn query<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Index) -> R + Send + 'static,
    ) -> Option<R> {
        self.worker
            .call(move |index| index.as_ref().map(f))
            .await
            .flatten()
    }

    /// Queues `f` straight away, ahead of any later updates, so it sees the
    /// index as it is now; the result arrives on the returned channel.
    pub fn submit<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Index) -> R + Send + 'static,
    ) -> async_channel::Receiver<Option<R>> {
        self.worker.submit(move |index| index.as_ref().map(f))
    }

    /// Runs `f` with mutable access (for refactoring plans that read and
    /// then expect the index to be updated by the watcher).
    pub async fn query_mut<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Index) -> R + Send + 'static,
    ) -> Option<R> {
        self.worker
            .call(move |index| index.as_mut().map(f))
            .await
            .flatten()
    }

    /// Every note with its title and aliases (as of the last update).
    pub fn notes(&self) -> Rc<Vec<NoteEntry>> {
        self.notes.borrow().clone()
    }

    /// Every tag with the number of notes using it.
    pub fn tags(&self) -> Rc<Vec<(String, usize)>> {
        self.tags.borrow().clone()
    }

    /// Every property key with its type and how many notes use it, most
    /// used first.
    pub fn properties(&self) -> Rc<Vec<PropertyInfo>> {
        self.properties.borrow().clone()
    }

    /// Sets the user's property types and refreshes the catalogue.
    pub fn set_overrides(self: &Rc<Self>, overrides: BTreeMap<String, PropertyType>) {
        self.overrides.replace(overrides);
        if self.is_ready() {
            let this = self.clone();
            glib::spawn_future_local(async move { this.changed().await });
        }
    }

    /// The aliases of `path`, from the last update.
    pub fn aliases(&self, path: &VaultPath) -> Vec<String> {
        self.notes
            .borrow()
            .iter()
            .find(|n| &n.path == path)
            .map(|n| n.aliases.clone())
            .unwrap_or_default()
    }
}
