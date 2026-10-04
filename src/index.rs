//! The vault's index (igneous-index) on a worker thread: built or caught up
//! when the window opens, kept current from the file watcher, and queried
//! asynchronously. The window also keeps a few small lists from it (notes
//! with their aliases, tags) for completion, which must answer immediately.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use igneous_core::watch::VaultEvent;
use igneous_core::{Vault, VaultPath};
use igneous_index::{Index, NoteEntry};

use crate::worker::Worker;

pub struct IndexService {
    worker: Worker<Option<Index>>,
    vault: Vault,
    ready: Cell<bool>,
    notes: RefCell<Rc<Vec<NoteEntry>>>,
    tags: RefCell<Rc<Vec<(String, usize)>>>,
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

    async fn changed(&self) {
        let lists = self.query(|index| (index.notes(), index.tags())).await;
        if let Some((notes, tags)) = lists {
            self.notes.replace(Rc::new(notes.unwrap_or_default()));
            self.tags.replace(Rc::new(tags.unwrap_or_default()));
        }
        for listener in self.listeners.borrow().iter() {
            listener();
        }
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
